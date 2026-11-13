// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nexo_core::{
    Batch, Conversation, Direction, Message, PAGE_ITEMS, Page, Source, Summary, Update,
};

use crate::account::Account;
use crate::net::{self, Fetched, INBOX, Outgoing, Part, Session};
use crate::parse::{Mail, base_subject, message_ids, parse};

pub const CHANNEL: &str = "email";
const NO_SUBJECT: &str = "(no subject)";

/// A mail with what the server knows about it.
struct Stored {
    mail: Mail,
    seen: bool,
    /// When the server got the mail, or else the time in its `Date` header.
    time: Option<DateTime<Utc>>,
}

/// What stays between two requests.
#[derive(Default)]
struct Cache {
    /// The folder of sent mail, when nexo asked the server for it. The inner `None` is for
    /// a server that has none.
    sent: Option<Option<String>>,
    /// The search that the list showed last, with the UIDs that it found.
    found: Option<(String, Vec<u32>)>,
}

impl Cache {
    /// The folders that hold the mails of a conversation: the inbox, and the sent mail.
    fn folders(&mut self, session: &mut Session) -> Result<Vec<String>> {
        if self.sent.is_none() {
            self.sent = Some(net::sent_folder(session)?);
        }
        let sent = self.sent.clone().flatten();
        Ok(std::iter::once(INBOX.to_owned()).chain(sent).collect())
    }
}

/// A mailbox with IMAP and SMTP as a channel.
///
/// The user picks the conversations that the folder keeps. A conversation is a thread: the
/// mails that name the same first mail in `References`. Its id is the `Message-ID` of that
/// first mail. The list is the inbox, the newest mail first, or a text search of the inbox.
/// The cursor has the next UID of each folder, and a sync loads only the tracked threads
/// that got a mail after it.
pub struct Email {
    account: Account,
    session: Option<Session>,
    cache: Cache,
}

impl Email {
    pub fn new(account: Account) -> Self {
        Self {
            account,
            session: None,
            cache: Cache::default(),
        }
    }

    /// The address of the user.
    pub fn address(&self) -> &str {
        &self.account.address
    }

    /// Do `work` on the server. A connection from before can be dead, so a failure on one
    /// makes a new connection and does the work one more time.
    fn run<T>(&mut self, work: impl Fn(&mut Session, &mut Cache) -> Result<T>) -> Result<T> {
        let from_before = self.session.is_some();
        let mut result = self.attempt(&work);
        if result.is_err() && from_before {
            self.session = None;
            result = self.attempt(&work);
        }
        if result.is_err() {
            self.session = None;
        }
        result
    }

    fn attempt<T>(&mut self, work: &impl Fn(&mut Session, &mut Cache) -> Result<T>) -> Result<T> {
        let session = match &mut self.session {
            Some(session) => session,
            none => none.insert(net::connect(&self.account)?),
        };
        work(session, &mut self.cache)
    }
}

impl Source for Email {
    fn channel(&self) -> &str {
        CHANNEL
    }

    fn fetch(&mut self, cursor: Option<&str>, tracked: &[String]) -> Result<Batch> {
        let me = self.account.address.clone();
        let before = marks(cursor.unwrap_or_default());
        self.run(|session, cache| {
            let mut now = Vec::new();
            // `None` is for all tracked conversations.
            let mut changed: Option<BTreeSet<String>> = Some(BTreeSet::new());
            for name in cache.folders(session)? {
                let folder = net::examine(session, &name)?;
                let known = before
                    .iter()
                    .find(|mark| mark.folder == name && mark.uid_validity == folder.uid_validity);
                match (known, &mut changed) {
                    (Some(mark), Some(threads)) if folder.uid_next > mark.uid_next => {
                        // `n:*` gives the newest mail also when its UID is less than `n`.
                        let set = format!("{}:*", mark.uid_next);
                        for fetched in net::fetch(session, &set, true, Part::Headers)? {
                            if fetched.uid >= mark.uid_next {
                                threads.insert(stored(&name, &folder, fetched)?.mail.thread);
                            }
                        }
                    }
                    (Some(_), _) => {}
                    (None, _) => changed = None,
                }
                now.push(Mark {
                    folder: name,
                    uid_validity: folder.uid_validity,
                    uid_next: folder.uid_next,
                });
            }
            let mut updates = Vec::new();
            for id in tracked {
                if changed.as_ref().is_none_or(|threads| threads.contains(id))
                    && let Some(update) = thread(session, cache, id, &me)?
                {
                    updates.push(update);
                }
            }
            Ok(Batch {
                updates,
                cursor: Some(cursor_text(&now)),
            })
        })
    }

    fn can_reply(&self) -> bool {
        true
    }

    fn reply(&mut self, conversation: &Conversation, body: &str) -> Result<Message> {
        let me = self.account.address.clone();
        let id = conversation.id.clone();
        let mails = self
            .run(|session, cache| thread_mails(session, cache, &id))?
            .into_iter()
            .map(|stored| stored.mail)
            .collect::<Vec<_>>();
        let incoming = mails.iter().rev().find(|mail| !mail.is_from(&me));
        let newest = incoming
            .or(mails.last())
            .context("the mailbox does not have the conversation")?;
        let to = match incoming {
            Some(mail) if !mail.reply_to_raw.trim().is_empty() => &mail.reply_to_raw,
            Some(mail) => &mail.from_raw,
            None => &newest.to_raw,
        };
        let subject = match base_subject(&newest.subject) {
            subject if subject.is_empty() => format!("Re: {NO_SUBJECT}"),
            subject => format!("Re: {subject}"),
        };
        let has_id = !message_ids(&newest.id).is_empty();
        let references = match has_id {
            true => format!("{} {}", newest.references_raw.trim(), newest.id),
            false => String::new(),
        };
        let domain = me
            .rsplit_once('@')
            .map_or("localhost", |(_, domain)| domain);
        let sent_id = format!("<{}@{domain}>", random_name()?);
        net::send(
            &self.account,
            &Outgoing {
                id: &sent_id,
                to: to.trim(),
                subject: &subject,
                in_reply_to: has_id.then_some(newest.id.as_str()),
                references: references.trim(),
                body,
            },
        )?;
        Ok(Message {
            id: sent_id,
            author: me,
            sent_at: Utc::now(),
            direction: Direction::Out,
            body: body.to_owned(),
        })
    }

    fn can_browse(&self) -> bool {
        true
    }

    fn browse(&mut self, query: &str, page: Option<&str>) -> Result<Page> {
        let query = query.trim().to_owned();
        let page: Option<u32> = page.and_then(|token| token.parse().ok());
        self.run(|session, cache| {
            let folder = net::examine(session, INBOX)?;
            let size = PAGE_ITEMS as u32;
            let (fetched, next) = if query.is_empty() {
                // The page is a range of positions: `page` is the position of its newest mail.
                let high = page.unwrap_or(folder.exists).min(folder.exists);
                if high == 0 {
                    return Ok(Page::default());
                }
                let low = high.saturating_sub(size - 1).max(1);
                let fetched = net::fetch(session, &format!("{low}:{high}"), false, Part::Headers)?;
                (fetched, (low > 1).then(|| low - 1))
            } else {
                // The page is a part of the UIDs that the search found: `page` is its start.
                let uids = match (&cache.found, page) {
                    (Some((found, uids)), Some(_)) if *found == query => uids.clone(),
                    _ => {
                        let charset = if query.is_ascii() {
                            ""
                        } else {
                            "CHARSET UTF-8 "
                        };
                        let criteria = format!("{charset}TEXT {}", net::quoted(&query));
                        let uids = net::search(session, &criteria)?;
                        cache.found = Some((query.clone(), uids.clone()));
                        uids
                    }
                };
                let start = page.unwrap_or(0) as usize;
                let end = (start + PAGE_ITEMS).min(uids.len());
                let wanted = uids.get(start..end).unwrap_or_default();
                let fetched = net::fetch(session, &net::uid_set(wanted), true, Part::Headers)?;
                (fetched, (end < uids.len()).then_some(end as u32))
            };
            let mut mails = fetched
                .into_iter()
                .map(|fetched| stored(INBOX, &folder, fetched))
                .collect::<Result<Vec<_>>>()?;
            mails.sort_by_key(|mail| std::cmp::Reverse(mail.time));
            Ok(Page {
                items: summaries(&mails),
                next: next.map(|token| token.to_string()),
            })
        })
    }

    fn load(&mut self, id: &str) -> Result<Update> {
        let me = self.account.address.clone();
        self.run(|session, cache| thread(session, cache, id, &me))?
            .with_context(|| format!("the mailbox has no conversation {id}"))
    }
}

/// The id of a mail that has no `Message-ID`: where the mail is in the mailbox.
fn made_id(folder: &str, uid_validity: u32, uid: u32) -> String {
    format!("{folder}/{uid_validity}/{uid}")
}

fn stored(name: &str, folder: &net::Folder, fetched: Fetched) -> Result<Stored> {
    let mail = parse(
        &fetched.raw,
        &made_id(name, folder.uid_validity, fetched.uid),
    )?;
    Ok(Stored {
        time: fetched.received.or(mail.date),
        seen: fetched.seen,
        mail,
    })
}

/// One line for each thread of these mails, in the order of the mails.
fn summaries(mails: &[Stored]) -> Vec<Summary> {
    let mut items: Vec<Summary> = Vec::new();
    for stored in mails {
        let mail = &stored.mail;
        match items.iter_mut().find(|item| item.id == mail.thread) {
            Some(item) => {
                item.messages += 1;
                item.unread |= !stored.seen;
            }
            None => items.push(Summary {
                id: mail.thread.clone(),
                title: title(&mail.subject),
                from: mail.author(),
                latest_at: stored.time,
                preview: String::new(),
                unread: !stored.seen,
                messages: 1,
                tracked: false,
            }),
        }
    }
    items
}

fn title(subject: &str) -> String {
    match base_subject(subject) {
        subject if subject.is_empty() => NO_SUBJECT.to_owned(),
        subject => subject,
    }
}

/// The mails of a conversation from all its folders, the oldest first.
fn thread_mails(session: &mut Session, cache: &mut Cache, id: &str) -> Result<Vec<Stored>> {
    let mut mails: Vec<Stored> = Vec::new();
    for name in cache.folders(session)? {
        let folder = net::examine(session, &name)?;
        let uids = if message_ids(id).is_empty() {
            // The id is the place of a mail without a `Message-ID`.
            let place = id.rsplit_once('/').and_then(|(rest, uid)| {
                let found = rest == format!("{name}/{}", folder.uid_validity);
                found.then(|| uid.parse().ok()).flatten()
            });
            place.into_iter().collect()
        } else {
            let key = net::quoted(id);
            net::search(
                session,
                &format!(
                    "OR OR HEADER Message-ID {key} HEADER References {key} HEADER In-Reply-To {key}"
                ),
            )?
        };
        for fetched in net::fetch(session, &net::uid_set(&uids), true, Part::All)? {
            let stored = stored(&name, &folder, fetched)?;
            if !mails.iter().any(|m| m.mail.id == stored.mail.id) {
                mails.push(stored);
            }
        }
    }
    mails.sort_by_key(|mail| mail.time);
    Ok(mails)
}

/// A conversation with all its messages. `None` if the mailbox does not have it.
fn thread(session: &mut Session, cache: &mut Cache, id: &str, me: &str) -> Result<Option<Update>> {
    let mails = thread_mails(session, cache, id)?;
    let Some(first) = mails.first() else {
        return Ok(None);
    };
    let mut participants: Vec<String> = Vec::new();
    let mut addresses: Vec<&str> = vec![me];
    for stored in &mails {
        let mail = &stored.mail;
        for person in mail.from.iter().chain(&mail.to).chain(&mail.cc) {
            if !addresses.iter().any(|known| person.is(known)) {
                addresses.push(&person.address);
                participants.push(person.display());
            }
        }
    }
    let conversation = Conversation {
        id: id.to_owned(),
        title: title(&first.mail.subject),
        participants,
    };
    let messages = mails
        .iter()
        .map(|stored| Message {
            id: stored.mail.id.clone(),
            author: stored.mail.author(),
            sent_at: stored.time.unwrap_or_default(),
            direction: if stored.mail.is_from(me) {
                Direction::Out
            } else {
                Direction::In
            },
            body: stored.mail.text.clone(),
        })
        .collect();
    Ok(Some(Update {
        conversation,
        messages,
    }))
}

/// Where a folder was at the end of a sync.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mark {
    folder: String,
    uid_validity: u32,
    uid_next: u32,
}

/// The cursor: one line for each folder, with tabs between its parts.
fn cursor_text(marks: &[Mark]) -> String {
    let lines: Vec<String> = marks
        .iter()
        .map(|m| format!("{}\t{}\t{}", m.folder, m.uid_validity, m.uid_next))
        .collect();
    lines.join("\n")
}

fn marks(cursor: &str) -> Vec<Mark> {
    cursor
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some(Mark {
                folder: parts.next()?.to_owned(),
                uid_validity: parts.next()?.parse().ok()?,
                uid_next: parts.next()?.parse().ok()?,
            })
        })
        .collect()
}

fn random_name() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("getting random bytes: {err}"))?;
    if bytes.is_empty() {
        bail!("no random bytes");
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use nexo_core::{Engine, Folder, Stage, files};

    use super::*;
    use crate::mock::{ME, Server, mail};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nexo-mail-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A mailbox with `count` mails in the inbox: mail `n` has the subject `Topic n`.
    fn mailbox(count: i64) -> Server {
        let server = Server::start();
        for n in 1..=count {
            let from = format!("Person {n} <p{n}@example.com>");
            server.deliver(
                INBOX,
                mail(&format!("m{n}"), &from, ME, &format!("Topic {n}"), n),
            );
        }
        server
    }

    fn engine(server: &Server, tmp: &TempDir) -> Engine {
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(Email::new(server.account()))).unwrap();
        engine
    }

    #[test]
    fn the_cursor_goes_there_and_back() {
        let now = vec![
            Mark {
                folder: "INBOX".into(),
                uid_validity: 7,
                uid_next: 42,
            },
            Mark {
                folder: "Sent Items".into(),
                uid_validity: 1,
                uid_next: 3,
            },
        ];
        assert_eq!(marks(&cursor_text(&now)), now);
        assert!(marks("").is_empty());
        assert!(marks("INBOX\tseven\t1").is_empty());
    }

    #[test]
    fn a_large_inbox_shows_page_by_page() {
        let server = mailbox(60);
        let mut email = Email::new(server.account());
        let first = email.browse("", None).unwrap();
        assert_eq!(first.items.len(), PAGE_ITEMS);
        assert_eq!(first.items[0].title, "Topic 60");
        assert_eq!(first.items[0].from, "Person 60");
        assert_eq!(first.items[0].id, "<m60@example.com>");
        assert!(first.items[0].unread);

        let second = email.browse("", first.next.as_deref()).unwrap();
        assert_eq!(second.items[0].title, "Topic 35");
        let third = email.browse("", second.next.as_deref()).unwrap();
        assert_eq!(third.items.len(), 10);
        assert_eq!(third.next, None);

        let found = email.browse(" topic 4 ", None).unwrap();
        let titles: Vec<_> = found.items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles.len(), 11);
        assert_eq!(titles[0], "Topic 49");
        assert_eq!(titles[10], "Topic 4");
        assert!(
            email
                .browse("no such words", None)
                .unwrap()
                .items
                .is_empty()
        );
        assert_eq!(
            Email::new(Server::start().account())
                .browse("", None)
                .unwrap(),
            Page::default()
        );
    }

    #[test]
    fn the_mails_of_one_thread_are_one_line() {
        let server = mailbox(1);
        let reply = mail("m1b", "Ann <ann@example.com>", ME, "Re: Topic 1", 5).answers("m1");
        server.deliver(INBOX, reply);
        let mut email = Email::new(server.account());
        let page = email.browse("", None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].from, "Ann");
        assert_eq!(page.items[0].messages, 2);
        assert_eq!(page.items[0].title, "Topic 1");
    }

    #[test]
    fn only_the_picked_threads_come_to_the_folder() {
        let server = mailbox(30);
        let tmp = TempDir::new("pick");
        let mut engine = engine(&server, &tmp);
        assert_eq!(engine.sync().received, 0);
        assert!(engine.board().is_empty());

        let id = "<m7@example.com>";
        engine.track(CHANNEL, id).unwrap();
        let board = engine.board();
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].title, "Topic 7");
        assert_eq!(board[0].stage, Stage::ToReply);
        assert_eq!(board[0].latest.as_ref().unwrap().body, "Text of m7");

        let from = "Person 7 <p7@example.com>";
        server.deliver(
            INBOX,
            mail("m7b", from, ME, "Re: Topic 7", 100).answers("m7"),
        );
        server.deliver(
            INBOX,
            mail("m8b", "Person 8 <p8@example.com>", ME, "Re: Topic 8", 101).answers("m8"),
        );
        let report = engine.sync();
        assert_eq!(
            (report.received, report.problems.len()),
            (1, 0),
            "{report:?}"
        );
        assert_eq!(engine.board().len(), 1);
        assert_eq!(
            engine.board()[0].latest.as_ref().unwrap().body,
            "Text of m7b"
        );

        let before = server.commands().len();
        assert_eq!(engine.sync().received, 0);
        let quiet = server.commands()[before..].join("\n");
        assert!(
            !quiet.contains("FETCH") && !quiet.contains("SEARCH"),
            "{quiet}"
        );
    }

    #[test]
    fn a_reply_goes_out_in_the_thread_and_comes_back_only_once() {
        let server = mailbox(3);
        let tmp = TempDir::new("reply");
        let mut engine = engine(&server, &tmp);
        engine.sync();
        engine.track(CHANNEL, "<m2@example.com>").unwrap();
        let dir = engine.board()[0].dir.clone();
        std::fs::write(dir.join(files::SEND), "Yes, on Monday.\n").unwrap();

        let report = engine.sync();
        assert_eq!((report.sent, report.problems.len()), (1, 0), "{report:?}");
        let sent = server.sent();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].contains("To: \"Person 2\" <p2@example.com>\r\n"),
            "{}",
            sent[0]
        );
        assert!(sent[0].contains("Subject: Re: Topic 2\r\n"));
        assert!(sent[0].contains("In-Reply-To: <m2@example.com>\r\n"));
        assert!(sent[0].contains("References: <m2@example.com>\r\n"));
        assert!(sent[0].contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(sent[0].contains("Yes, on Monday."));
        assert_eq!(engine.board()[0].stage, Stage::Waiting);

        // The server keeps the sent mail. The next sync sees it, and the folder has it.
        assert_eq!(engine.sync().received, 0);
        let transcript = std::fs::read_to_string(dir.join(files::TRANSCRIPT)).unwrap();
        assert_eq!(transcript.matches("Yes, on Monday.").count(), 1);
        assert!(transcript.contains(&format!("**{ME}** (you)")));
    }

    #[test]
    fn new_uids_or_a_lost_cursor_load_the_tracked_threads_again() {
        let server = mailbox(5);
        let tmp = TempDir::new("cursor");
        let mut engine = engine(&server, &tmp);
        engine.sync();
        engine.track(CHANNEL, "<m1@example.com>").unwrap();
        let from = "Person 1 <p1@example.com>";
        server.deliver(
            INBOX,
            mail("m1b", from, ME, "Re: Topic 1", 100).answers("m1"),
        );
        server.renumber();
        let report = engine.sync();
        assert_eq!(
            (report.received, report.problems.len()),
            (1, 0),
            "{report:?}"
        );

        server.deliver(
            INBOX,
            mail("m1c", from, ME, "Re: Topic 1", 200).answers("m1"),
        );
        std::fs::remove_file(tmp.0.join(files::STATE_DIR).join("state.json")).unwrap();
        let mut engine = self::engine(&server, &tmp);
        assert_eq!(engine.sync().received, 1);
        assert_eq!(engine.sync().received, 0);
    }

    #[test]
    fn a_dead_connection_is_made_again() {
        let server = mailbox(2);
        let mut email = Email::new(server.account());
        assert_eq!(email.browse("", None).unwrap().items.len(), 2);
        server.drop_connections();
        assert_eq!(email.browse("", None).unwrap().items.len(), 2);
        assert!(email.load("<no-such@example.com>").is_err());
    }

    #[test]
    fn a_wrong_password_is_an_error_with_the_server_in_it() {
        let server = mailbox(1);
        let mut account = server.account();
        account.password = "wrong".into();
        let err = Email::new(account.clone()).browse("", None).unwrap_err();
        assert!(
            format!("{err:#}").contains("signing in to 127.0.0.1"),
            "{err:#}"
        );
        assert!(net::check(&account).is_err());
        net::check(&server.account()).unwrap();
    }
}
