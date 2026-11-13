// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! A small IMAP server and a small SMTP server on local ports, with a mailbox that a test
//! fills. The SMTP server puts each mail that it gets in the folder of sent mail.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use chrono::DateTime;

use crate::account::{Account, Server as Address};

pub const ME: &str = "me@example.com";
const SENT: &str = "Sent";

/// A mail for the mailbox.
pub struct MailBuilder {
    id: String,
    from: String,
    to: String,
    subject: String,
    seconds: i64,
    answers: Option<String>,
}

/// A plain text mail with the text `Text of <id>`. `seconds` is its time, from 1970.
pub fn mail(id: &str, from: &str, to: &str, subject: &str, seconds: i64) -> MailBuilder {
    MailBuilder {
        id: id.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        subject: subject.to_owned(),
        seconds,
        answers: None,
    }
}

impl MailBuilder {
    /// The mail is a reply to the first mail of a thread.
    pub fn answers(mut self, id: &str) -> Self {
        self.answers = Some(id.to_owned());
        self
    }

    fn raw(&self) -> String {
        let date = DateTime::from_timestamp(self.seconds, 0)
            .unwrap()
            .to_rfc2822();
        let mut raw = format!(
            "Message-ID: <{}@example.com>\r\nFrom: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {date}\r\n",
            self.id, self.from, self.to, self.subject
        );
        if let Some(first) = &self.answers {
            raw.push_str(&format!(
                "In-Reply-To: <{first}@example.com>\r\nReferences: <{first}@example.com>\r\n"
            ));
        }
        raw.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        raw.push_str(&format!("Text of {}\r\n", self.id));
        raw
    }
}

struct Stored {
    uid: u32,
    seen: bool,
    seconds: i64,
    raw: String,
}

struct Folder {
    name: String,
    uid_validity: u32,
    uid_next: u32,
    mails: Vec<Stored>,
}

impl Folder {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            uid_validity: 1,
            uid_next: 1,
            mails: Vec::new(),
        }
    }

    fn add(&mut self, raw: String, seconds: i64, seen: bool) {
        self.mails.push(Stored {
            uid: self.uid_next,
            seen,
            seconds,
            raw,
        });
        self.uid_next += 1;
    }
}

struct Mailbox {
    folders: Vec<Folder>,
    /// The mails that the SMTP server got.
    sent: Vec<String>,
    /// The commands that the IMAP server got.
    commands: Vec<String>,
    connections: Vec<TcpStream>,
}

impl Mailbox {
    fn folder(&mut self, name: &str) -> Option<&mut Folder> {
        self.folders
            .iter_mut()
            .find(|folder| folder.name.eq_ignore_ascii_case(name))
    }
}

pub struct Server {
    imap: u16,
    smtp: u16,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl Server {
    pub fn start() -> Self {
        let mailbox = Arc::new(Mutex::new(Mailbox {
            folders: vec![Folder::new("INBOX"), Folder::new(SENT)],
            sent: Vec::new(),
            commands: Vec::new(),
            connections: Vec::new(),
        }));
        let listen = |serve: fn(TcpStream, &Mutex<Mailbox>)| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let shared = Arc::clone(&mailbox);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || serve(stream, &shared));
                }
            });
            port
        };
        Self {
            imap: listen(imap),
            smtp: listen(smtp),
            mailbox: Arc::clone(&mailbox),
        }
    }

    pub fn account(&self) -> Account {
        let local = |port| Address {
            host: "127.0.0.1".into(),
            port,
        };
        Account {
            address: ME.into(),
            username: ME.into(),
            password: "secret".into(),
            imap: local(self.imap),
            smtp: local(self.smtp),
            plaintext: true,
        }
    }

    /// Put a mail in a folder. The mail is not read.
    pub fn deliver(&self, folder: &str, mail: MailBuilder) {
        let mut mailbox = self.mailbox.lock().unwrap();
        let folder = mailbox.folder(folder).unwrap();
        folder.add(mail.raw(), mail.seconds, false);
    }

    /// Give all folders new UIDs, as a server does after it made a mailbox again.
    pub fn renumber(&self) {
        for folder in &mut self.mailbox.lock().unwrap().folders {
            folder.uid_validity += 1;
            for mail in &mut folder.mails {
                mail.uid += 100;
            }
            folder.uid_next += 100;
        }
    }

    /// Close the connections that are open, as a server does after some time.
    pub fn drop_connections(&self) {
        for stream in self.mailbox.lock().unwrap().connections.drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub fn sent(&self) -> Vec<String> {
        self.mailbox.lock().unwrap().sent.clone()
    }

    pub fn commands(&self) -> Vec<String> {
        self.mailbox.lock().unwrap().commands.clone()
    }
}

/// The text of `text` without the quotes of IMAP around it.
fn unquoted(text: &str) -> String {
    let text = text.trim();
    match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => text.to_owned(),
    }
}

/// The first quoted string of `text`.
fn first_quoted(text: &str) -> String {
    let Some(start) = text.find('"') else {
        return String::new();
    };
    let mut out = String::new();
    let mut escaped = false;
    for c in text[start + 1..].chars() {
        match (escaped, c) {
            (true, c) => {
                out.push(c);
                escaped = false;
            }
            (false, '\\') => escaped = true,
            (false, '"') => break,
            (false, c) => out.push(c),
        }
    }
    out
}

fn headers(raw: &str) -> &str {
    raw.split_once("\r\n\r\n").map_or(raw, |(head, _)| head)
}

fn imap(stream: TcpStream, mailbox: &Mutex<Mailbox>) {
    let mut out = stream.try_clone().unwrap();
    mailbox
        .lock()
        .unwrap()
        .connections
        .push(stream.try_clone().unwrap());
    let _ = out.write_all(b"* OK IMAP4rev1 ready\r\n");
    let mut selected = String::new();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            return;
        };
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let upper = rest.to_ascii_uppercase();
        let mut mailbox = mailbox.lock().unwrap();
        let hidden = upper.starts_with("LOGIN");
        mailbox.commands.push(if hidden {
            "LOGIN".to_owned()
        } else {
            rest.to_owned()
        });
        let mut answer = String::new();
        let mut status = "OK done".to_owned();
        if upper.starts_with("CAPABILITY") {
            answer.push_str("* CAPABILITY IMAP4rev1\r\n");
        } else if upper.starts_with("LOGIN") {
            if rest.contains("wrong") {
                status = "NO the password is wrong".to_owned();
            }
        } else if upper.starts_with("LIST") {
            for folder in &mailbox.folders {
                let mark = if folder.name == SENT { " \\Sent" } else { "" };
                answer.push_str(&format!(
                    "* LIST (\\HasNoChildren{mark}) \"/\" \"{}\"\r\n",
                    folder.name
                ));
            }
        } else if upper.starts_with("EXAMINE") || upper.starts_with("SELECT") {
            let name = unquoted(rest.split_once(' ').map_or("", |(_, name)| name));
            match mailbox.folder(&name) {
                Some(folder) => {
                    selected = folder.name.clone();
                    answer.push_str(&format!(
                        "* {} EXISTS\r\n* 0 RECENT\r\n* FLAGS (\\Seen)\r\n\
                         * OK [UIDVALIDITY {}] valid\r\n* OK [UIDNEXT {}] next\r\n",
                        folder.mails.len(),
                        folder.uid_validity,
                        folder.uid_next
                    ));
                    status = "OK [READ-ONLY] done".to_owned();
                }
                None => status = "NO no such folder".to_owned(),
            }
        } else if upper.starts_with("UID SEARCH") {
            let key = first_quoted(rest).to_lowercase();
            let by_header = upper.contains("HEADER");
            let folder = mailbox.folder(&selected).unwrap();
            let uids: Vec<String> = folder
                .mails
                .iter()
                .filter(|mail| {
                    let text = if by_header {
                        headers(&mail.raw)
                    } else {
                        &mail.raw
                    };
                    upper.ends_with("ALL") || text.to_lowercase().contains(&key)
                })
                .map(|mail| mail.uid.to_string())
                .collect();
            answer.push_str(&format!("* SEARCH {}\r\n", uids.join(" ")));
        } else if upper.starts_with("UID FETCH") || upper.starts_with("FETCH") {
            let by_uid = upper.starts_with("UID");
            let mut words = rest.split(' ').skip(if by_uid { 2 } else { 1 });
            let set = words.next().unwrap_or_default();
            let only_headers = upper.contains("[HEADER]");
            let folder = mailbox.folder(&selected).unwrap();
            let largest = match by_uid {
                true => folder.mails.last().map_or(0, |mail| mail.uid),
                false => folder.mails.len() as u32,
            };
            let wanted = |n: u32| {
                set.split(',').any(|part| {
                    let number = |text: &str| match text {
                        "*" => largest,
                        text => text.parse().unwrap_or(0),
                    };
                    let (low, high) = match part.split_once(':') {
                        Some((low, high)) => (number(low), number(high)),
                        None => (number(part), number(part)),
                    };
                    (low.min(high)..=low.max(high)).contains(&n)
                })
            };
            for (index, mail) in folder.mails.iter().enumerate() {
                let position = index as u32 + 1;
                if !wanted(if by_uid { mail.uid } else { position }) {
                    continue;
                }
                let date = DateTime::from_timestamp(mail.seconds, 0)
                    .unwrap()
                    .format("%d-%b-%Y %H:%M:%S +0000");
                let (section, data) = match only_headers {
                    true => ("BODY[HEADER]", format!("{}\r\n\r\n", headers(&mail.raw))),
                    false => ("BODY[]", mail.raw.clone()),
                };
                answer.push_str(&format!(
                    "* {position} FETCH (UID {} FLAGS ({}) INTERNALDATE \"{date}\" {section} {{{}}}\r\n{data})\r\n",
                    mail.uid,
                    if mail.seen { "\\Seen" } else { "" },
                    data.len()
                ));
            }
        } else if upper.starts_with("LOGOUT") {
            answer.push_str("* BYE\r\n");
        }
        drop(mailbox);
        if out
            .write_all(format!("{answer}{tag} {status}\r\n").as_bytes())
            .is_err()
        {
            return;
        }
    }
}

fn smtp(stream: TcpStream, mailbox: &Mutex<Mailbox>) {
    let mut out = stream.try_clone().unwrap();
    let _ = out.write_all(b"220 mock ESMTP\r\n");
    let mut lines = BufReader::new(stream).lines();
    while let Some(Ok(line)) = lines.next() {
        let upper = line.to_ascii_uppercase();
        let answer = if upper.starts_with("EHLO") {
            "250-mock\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
        } else if upper.starts_with("AUTH") {
            "235 signed in\r\n"
        } else if upper.starts_with("DATA") {
            let _ = out.write_all(b"354 go on\r\n");
            let mut raw = String::new();
            for line in lines.by_ref() {
                let Ok(line) = line else {
                    return;
                };
                if line == "." {
                    break;
                }
                raw.push_str(line.strip_prefix('.').unwrap_or(&line));
                raw.push_str("\r\n");
            }
            let mut mailbox = mailbox.lock().unwrap();
            mailbox.sent.push(raw.clone());
            let folder = mailbox.folder(SENT).unwrap();
            folder.add(raw, 4_000_000_000, true);
            "250 queued\r\n"
        } else if upper.starts_with("QUIT") {
            let _ = out.write_all(b"221 bye\r\n");
            return;
        } else {
            "250 ok\r\n"
        };
        if out.write_all(answer.as_bytes()).is_err() {
            return;
        }
    }
}
