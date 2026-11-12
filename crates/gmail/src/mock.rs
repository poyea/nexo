// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! A small Gmail on a local port, with a mailbox that a test fills.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::api::{Credentials, Endpoints};
use crate::text::{base64url, from_base64url, query_pairs};

pub const ME: &str = "me@example.com";
pub const CODE: &str = "code-1";
const REFRESH_TOKEN: &str = "refresh-1";

/// A mail for the mailbox.
pub struct MailBuilder(Value);

/// A plain text mail to the user. `seconds` is its time, from 1970.
pub fn mail(id: &str, from: &str, subject: &str, seconds: i64, text: &str) -> MailBuilder {
    MailBuilder(json!({
        "id": id,
        "labelIds": ["INBOX"],
        "snippet": text,
        "internalDate": (seconds * 1000).to_string(),
        "payload": {
            "mimeType": "multipart/mixed",
            "headers": [
                {"name": "From", "value": from},
                {"name": "To", "value": ME},
                {"name": "Subject", "value": subject},
                {"name": "Message-ID", "value": format!("<{id}@mail.example.com>")},
            ],
            "parts": [
                {"mimeType": "text/plain", "body": {"data": base64url(text.as_bytes())}},
            ],
        },
    }))
}

impl MailBuilder {
    fn labeled(mut self, label: &str) -> Self {
        let labels = self.0["labelIds"].as_array_mut().unwrap();
        labels.push(label.into());
        self
    }

    /// The user sent this mail.
    pub fn sent(self) -> Self {
        self.labeled("SENT")
    }

    pub fn unread(self) -> Self {
        self.labeled("UNREAD")
    }

    pub fn to(mut self, to: &str) -> Self {
        self.0["payload"]["headers"][1]["value"] = to.into();
        self
    }

    /// The mail has only this HTML.
    pub fn html(mut self, html: &str) -> Self {
        self.0["payload"]["parts"] = json!([
            {"mimeType": "text/html", "body": {"data": format!("{}==", base64url(html.as_bytes()))}},
        ]);
        self
    }

    pub fn attachment(mut self, filename: &str) -> Self {
        let parts = self.0["payload"]["parts"].as_array_mut().unwrap();
        parts.push(json!({
            "mimeType": "application/octet-stream",
            "filename": filename,
            "body": {"attachmentId": "a1"},
        }));
        self
    }
}

pub fn thread(id: &str, mails: Vec<MailBuilder>) -> Value {
    let messages: Vec<Value> = mails
        .into_iter()
        .map(|mail| {
            let mut mail = mail.0;
            mail["threadId"] = id.into();
            mail
        })
        .collect();
    json!({"id": id, "messages": messages})
}

#[derive(Default)]
pub struct Mailbox {
    pub threads: Vec<Value>,
    /// The id of each change, and the thread that got a mail.
    history: Vec<(u64, String)>,
    /// A sync from before this id is too old.
    pub oldest_history: u64,
    /// The `raw` values of the mails that the user sent, as text.
    pub sent: Vec<String>,
    pub tokens_given: usize,
    access_token: String,
    /// The paths of all requests, with their queries.
    pub requests: Vec<String>,
}

impl Mailbox {
    fn history_id(&self) -> u64 {
        self.history.last().map_or(100, |(id, _)| *id)
    }

    /// Put a mail in a thread, or in a new thread.
    pub fn deliver(&mut self, thread_id: &str, mail: MailBuilder) {
        let mut mail = mail.0;
        mail["threadId"] = thread_id.into();
        match self.threads.iter_mut().find(|t| t["id"] == thread_id) {
            Some(thread) => thread["messages"].as_array_mut().unwrap().push(mail),
            None => self
                .threads
                .push(json!({"id": thread_id, "messages": [mail]})),
        }
        self.history
            .push((self.history_id() + 1, thread_id.to_owned()));
    }

    /// Make the access token that nexo has now invalid.
    pub fn end_access(&mut self) {
        self.access_token.clear();
    }
}

pub struct Server {
    pub endpoints: Endpoints,
    pub mailbox: Arc<Mutex<Mailbox>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let shared = Arc::clone(&mailbox);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &shared);
            }
        });
        Self {
            endpoints: Endpoints {
                api: format!("{base}/gmail"),
                token: format!("{base}/token"),
                auth: format!("{base}/auth"),
            },
            mailbox,
        }
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            client_id: "client-1".into(),
            client_secret: "secret-1".into(),
            refresh_token: REFRESH_TOKEN.into(),
            email: ME.into(),
        }
    }

    pub fn deliver(&self, thread_id: &str, mail: MailBuilder) {
        self.mailbox.lock().unwrap().deliver(thread_id, mail);
    }

    pub fn requests(&self) -> Vec<String> {
        self.mailbox.lock().unwrap().requests.clone()
    }
}

fn serve(stream: TcpStream, mailbox: &Mutex<Mailbox>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut words = line.split_whitespace();
    let (Some(method), Some(target)) = (words.next(), words.next()) else {
        return;
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let (mut length, mut authorization) = (0, String::new());
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = value.trim().to_owned(),
                _ => {}
            }
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let (status, answer) = {
        let mut mailbox = mailbox.lock().unwrap();
        mailbox.requests.push(format!("{method} {target}"));
        route(&mut mailbox, &method, &target, &authorization, &body)
    };
    let answer = answer.to_string();
    let _ = write!(
        reader.get_mut(),
        "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{answer}",
        answer.len()
    );
}

fn failure(status: u16, message: &str) -> (u16, Value) {
    (
        status,
        json!({"error": {"code": status, "message": message}}),
    )
}

fn route(
    mailbox: &mut Mailbox,
    method: &str,
    target: &str,
    authorization: &str,
    body: &str,
) -> (u16, Value) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query_pairs(query);
    let param = |name: &str| {
        query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    if path == "/token" {
        return token(mailbox, &query_pairs(body));
    }
    if mailbox.access_token.is_empty()
        || authorization != format!("Bearer {}", mailbox.access_token)
    {
        return failure(401, "Invalid Credentials");
    }
    let path = path.strip_prefix("/gmail/").unwrap_or(path);
    match (method, path) {
        ("GET", "profile") => (
            200,
            json!({"emailAddress": ME, "historyId": mailbox.history_id().to_string()}),
        ),
        ("GET", "threads") => {
            let q = param("q").unwrap_or_default().to_lowercase();
            let size: usize = param("maxResults")
                .and_then(|n| n.parse().ok())
                .unwrap_or(100);
            let skip: usize = param("pageToken").and_then(|n| n.parse().ok()).unwrap_or(0);
            let newest = |thread: &Value| thread["messages"].as_array().unwrap().last().cloned();
            let time = |thread: &Value| -> i64 {
                newest(thread)
                    .and_then(|m| m["internalDate"].as_str()?.parse().ok())
                    .unwrap_or(0)
            };
            let mut found: Vec<&Value> = mailbox
                .threads
                .iter()
                .filter(|thread| {
                    let subject = thread["messages"][0]["payload"]["headers"][2]["value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase();
                    q == "in:inbox" || subject.contains(&q)
                })
                .collect();
            found.sort_by_key(|thread| std::cmp::Reverse(time(thread)));
            let threads: Vec<Value> = found
                .iter()
                .skip(skip)
                .take(size)
                .map(|thread| {
                    let snippet = newest(thread).map(|m| m["snippet"].clone());
                    json!({"id": thread["id"], "snippet": snippet})
                })
                .collect();
            let mut answer = json!({"threads": threads});
            if skip + size < found.len() {
                answer["nextPageToken"] = (skip + size).to_string().into();
            }
            (200, answer)
        }
        ("GET", "history") => {
            let start: u64 = param("startHistoryId")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if start < mailbox.oldest_history {
                return failure(404, "Requested entity was not found.");
            }
            let history: Vec<Value> = mailbox
                .history
                .iter()
                .filter(|(id, _)| *id > start)
                .map(|(id, thread)| {
                    json!({
                        "id": id.to_string(),
                        "messagesAdded": [{"message": {"id": "x", "threadId": thread}}],
                    })
                })
                .collect();
            (
                200,
                json!({"history": history, "historyId": mailbox.history_id().to_string()}),
            )
        }
        ("POST", "messages/send") => {
            let sent: Value = serde_json::from_str(body).unwrap_or_default();
            let (Some(raw), Some(thread_id)) = (sent["raw"].as_str(), sent["threadId"].as_str())
            else {
                return failure(400, "Missing raw or threadId");
            };
            let raw = String::from_utf8(from_base64url(raw).unwrap_or_default()).unwrap();
            mailbox.sent.push(raw);
            let id = format!("sent-{}", mailbox.sent.len());
            let seconds = 4_000_000_000 + mailbox.sent.len() as i64;
            mailbox.deliver(thread_id, mail(&id, ME, "Re:", seconds, "(sent)").sent());
            (200, json!({"id": id, "threadId": thread_id}))
        }
        ("GET", path) => {
            let id = path.strip_prefix("threads/").unwrap_or_default();
            match mailbox.threads.iter().find(|t| t["id"] == id) {
                Some(thread) => (200, thread.clone()),
                None => failure(404, "Requested entity was not found."),
            }
        }
        _ => failure(404, "No such method"),
    }
}

fn token(mailbox: &mut Mailbox, form: &[(String, String)]) -> (u16, Value) {
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map_or("", |(_, v)| v.as_str())
    };
    let good = match field("grant_type") {
        "refresh_token" => field("refresh_token") == REFRESH_TOKEN,
        "authorization_code" => {
            field("code") == CODE
                && !field("code_verifier").is_empty()
                && field("redirect_uri").starts_with("http://127.0.0.1:")
        }
        _ => false,
    };
    if !good || field("client_secret") != "secret-1" {
        return (
            400,
            json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked.",
            }),
        );
    }
    mailbox.tokens_given += 1;
    mailbox.access_token = format!("access-{}", mailbox.tokens_given);
    let mut answer = json!({"access_token": mailbox.access_token, "expires_in": 3600});
    if field("grant_type") == "authorization_code" {
        answer["refresh_token"] = REFRESH_TOKEN.into();
    }
    (200, answer)
}
