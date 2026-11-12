// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use chrono::Utc;
use nexo_core::{Batch, Conversation, Direction, Message, PAGE_ITEMS, Page, Source, Update};
use serde::Deserialize;
use serde_json::json;

use crate::api::{Client, Credentials, Endpoints, Session, status_of};
use crate::mail::Thread;

pub const CHANNEL: &str = "gmail";
/// What the list shows when the user gives no search.
const DEFAULT_QUERY: &str = "in:inbox";
/// How many requests go to Gmail at the same time.
const PARALLEL: usize = 6;
const REPLY_HEADERS: [&str; 6] = [
    "From",
    "To",
    "Reply-To",
    "Subject",
    "Message-ID",
    "References",
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    history_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadList {
    #[serde(default)]
    threads: Vec<ThreadRef>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct ThreadRef {
    id: String,
    #[serde(default)]
    snippet: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryList {
    #[serde(default)]
    history: Vec<History>,
    next_page_token: Option<String>,
    history_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    #[serde(default)]
    messages_added: Vec<Added>,
}

#[derive(Deserialize)]
struct Added {
    message: MailRef,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailRef {
    thread_id: String,
}

#[derive(Deserialize)]
struct Sent {
    id: String,
}

/// The Gmail of one user as a channel.
///
/// The user picks the threads that the folder keeps: the list comes from a search in Gmail,
/// page by page, so the size of the mailbox does not matter. The cursor is the history id
/// of Gmail, and a sync loads only the tracked threads that got a mail after it.
pub struct Gmail {
    client: Client,
}

impl Gmail {
    pub fn new(credentials: Credentials) -> Self {
        Self::with_endpoints(credentials, Endpoints::default())
    }

    pub fn with_endpoints(credentials: Credentials, endpoints: Endpoints) -> Self {
        Self {
            client: Client::new(credentials, endpoints),
        }
    }

    /// The address of the user.
    pub fn email(&self) -> &str {
        self.client.email()
    }
}

impl Source for Gmail {
    fn channel(&self) -> &str {
        CHANNEL
    }

    fn fetch(&mut self, cursor: Option<&str>, tracked: &[String]) -> Result<Batch> {
        let me = self.client.email().to_owned();
        self.client.with(|gmail| {
            let changed = match cursor {
                Some(cursor) => changed_threads(gmail, cursor)?,
                None => None,
            };
            let (ids, cursor) = match changed {
                Some((threads, cursor)) => {
                    let ids = tracked
                        .iter()
                        .filter(|id| threads.contains(*id))
                        .cloned()
                        .collect();
                    (ids, cursor)
                }
                // There is no cursor, or Gmail does not keep the history from it: read the
                // history id first, so that no mail is lost, then load all tracked threads.
                None => {
                    let profile: Profile = gmail.get("profile", &[])?;
                    (tracked.to_vec(), profile.history_id)
                }
            };
            let threads = in_parallel(&ids, |id| full_thread(gmail, id))?;
            Ok(Batch {
                updates: threads
                    .into_iter()
                    .flatten()
                    .map(|thread| thread.into_update(&me))
                    .collect(),
                cursor: Some(cursor),
            })
        })
    }

    fn can_reply(&self) -> bool {
        true
    }

    fn reply(&mut self, conversation: &Conversation, body: &str) -> Result<Message> {
        let me = self.client.email().to_owned();
        let path = format!("threads/{}", conversation.id);
        let mut query = vec![("format", "metadata")];
        query.extend(REPLY_HEADERS.map(|name| ("metadataHeaders", name)));
        let thread: Thread = self.client.with(|gmail| gmail.get(&path, &query))?;
        let mail = json!({"raw": thread.reply(body)?, "threadId": conversation.id});
        // Gmail refuses a request with an old access token before it sends the mail, so a
        // second try cannot send the mail two times.
        let sent: Sent = self
            .client
            .with(|gmail| gmail.post("messages/send", &mail))?;
        Ok(Message {
            id: sent.id,
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
        let query = match query.trim() {
            "" => DEFAULT_QUERY,
            query => query,
        };
        let size = PAGE_ITEMS.to_string();
        let mut params = vec![("q", query), ("maxResults", size.as_str())];
        params.extend(page.map(|token| ("pageToken", token)));
        self.client.with(|gmail| {
            let list: ThreadList = gmail.get("threads", &params)?;
            let items = in_parallel(&list.threads, |found| {
                let path = format!("threads/{}", found.id);
                let query = [
                    ("format", "metadata"),
                    ("metadataHeaders", "Subject"),
                    ("metadataHeaders", "From"),
                ];
                let thread: Thread = gmail.get(&path, &query)?;
                Ok(thread.into_summary(&found.snippet))
            })?;
            Ok(Page {
                items,
                next: list.next_page_token,
            })
        })
    }

    fn load(&mut self, id: &str) -> Result<Update> {
        let me = self.client.email().to_owned();
        let thread = self
            .client
            .with(|gmail| full_thread(gmail, id))?
            .with_context(|| format!("Gmail has no conversation {id}"))?;
        Ok(thread.into_update(&me))
    }
}

/// The thread with the text of its mails. `None` if Gmail does not have it, for example
/// after the user deleted it there.
fn full_thread(gmail: &Session, id: &str) -> Result<Option<Thread>> {
    match gmail.get(&format!("threads/{id}"), &[("format", "full")]) {
        Ok(thread) => Ok(Some(thread)),
        Err(err) if status_of(&err) == Some(404) => Ok(None),
        Err(err) => Err(err),
    }
}

/// The ids of the threads that got a mail after `cursor`, and the cursor for the next time.
/// `None` if Gmail does not keep the history from `cursor`.
fn changed_threads(gmail: &Session, cursor: &str) -> Result<Option<(BTreeSet<String>, String)>> {
    let mut threads = BTreeSet::new();
    let mut newest = cursor.to_owned();
    let mut page: Option<String> = None;
    loop {
        let mut query = vec![
            ("startHistoryId", cursor),
            ("historyTypes", "messageAdded"),
            ("maxResults", "500"),
        ];
        query.extend(page.as_deref().map(|token| ("pageToken", token)));
        let list: HistoryList = match gmail.get("history", &query) {
            Ok(list) => list,
            Err(err) if status_of(&err) == Some(404) => return Ok(None),
            Err(err) => return Err(err),
        };
        let added = list.history.into_iter().flat_map(|h| h.messages_added);
        threads.extend(added.map(|a| a.message.thread_id));
        if let Some(id) = list.history_id {
            newest = id;
        }
        page = list.next_page_token;
        if page.is_none() {
            return Ok(Some((threads, newest)));
        }
    }
}

/// Do `work` for each item on some threads at the same time, and give the results in the
/// order of the items.
fn in_parallel<I: Sync, T: Send>(
    items: &[I],
    work: impl Fn(&I) -> Result<T> + Sync,
) -> Result<Vec<T>> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<T>>>> = Mutex::new(items.iter().map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        return;
                    };
                    let result = work(item);
                    results.lock().expect("no work panics with the lock")[index] = Some(result);
                }
            });
        }
    });
    let results = results.into_inner().expect("no work panics with the lock");
    results
        .into_iter()
        .map(|result| result.expect("each item has a result"))
        .collect()
}

#[cfg(test)]
mod tests {
    use nexo_core::{Engine, Folder, Stage, files};

    use super::*;
    use crate::mock::{Server, mail};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nexo-gmail-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A mailbox with `count` threads: thread `t7` has the subject `Topic 7`.
    fn mailbox(count: i64) -> Server {
        let server = Server::start();
        for n in 1..=count {
            let from = format!("Person {n} <p{n}@example.com>");
            let mut first = mail(&format!("m{n}"), &from, &format!("Topic {n}"), n, "Hello");
            if n % 2 == 0 {
                first = first.unread();
            }
            server.deliver(&format!("t{n}"), first);
        }
        server
    }

    fn gmail(server: &Server) -> Gmail {
        Gmail::with_endpoints(server.credentials(), server.endpoints.clone())
    }

    fn engine(server: &Server, tmp: &TempDir) -> Engine {
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(gmail(server))).unwrap();
        engine
    }

    #[test]
    fn a_large_mailbox_shows_page_by_page() {
        let server = mailbox(60);
        let mut gmail = gmail(&server);
        let first = gmail.browse("", None).unwrap();
        assert_eq!(first.items.len(), PAGE_ITEMS);
        assert_eq!(first.items[0].id, "t60");
        assert_eq!(first.items[0].title, "Topic 60");
        assert_eq!(first.items[0].from, "Person 60");
        assert_eq!(first.items[0].preview, "Hello");
        assert!(first.items[0].unread && !first.items[1].unread);

        let second = gmail.browse("", first.next.as_deref()).unwrap();
        assert_eq!(second.items[0].id, "t35");
        let third = gmail.browse("", second.next.as_deref()).unwrap();
        assert_eq!(third.items.len(), 10);
        assert_eq!(third.next, None);

        let found = gmail.browse(" topic 42 ", None).unwrap();
        let ids: Vec<_> = found.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["t42"]);
        // One access token is enough for all of this.
        assert_eq!(server.mailbox.lock().unwrap().tokens_given, 1);
    }

    #[test]
    fn only_the_picked_threads_come_to_the_folder() {
        let server = mailbox(30);
        let tmp = TempDir::new("pick");
        let mut engine = engine(&server, &tmp);
        assert_eq!(engine.sync().received, 0);
        assert!(engine.board().is_empty());

        engine.track(CHANNEL, "t7").unwrap();
        let board = engine.board();
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].title, "Topic 7");
        assert_eq!(board[0].stage, Stage::ToReply);
        assert!(engine.browse(CHANNEL, "topic 7", None).unwrap().items[0].tracked);

        // New mail comes in a tracked thread, and in a thread that is not tracked.
        server.deliver(
            "t7",
            mail(
                "m7b",
                "Person 7 <p7@example.com>",
                "Re: Topic 7",
                100,
                "More",
            ),
        );
        server.deliver(
            "t8",
            mail(
                "m8b",
                "Person 8 <p8@example.com>",
                "Re: Topic 8",
                101,
                "More",
            ),
        );
        let before = server.requests().len();
        let report = engine.sync();
        assert_eq!((report.received, report.problems.len()), (1, 0));
        let requests = server.requests()[before..].to_vec();
        assert!(
            requests.iter().any(|r| r.contains("/threads/t7?")),
            "{requests:?}"
        );
        assert!(
            !requests.iter().any(|r| r.contains("/threads/t8")),
            "{requests:?}"
        );
        assert_eq!(engine.board().len(), 1);
        assert_eq!(engine.board()[0].latest.as_ref().unwrap().body, "More");

        let quiet = server.requests().len();
        assert_eq!(engine.sync().received, 0);
        assert_eq!(server.requests().len(), quiet + 1);
    }

    #[test]
    fn a_reply_goes_out_in_the_thread_and_comes_back_only_once() {
        let server = mailbox(3);
        let tmp = TempDir::new("reply");
        let mut engine = engine(&server, &tmp);
        engine.sync();
        engine.track(CHANNEL, "t2").unwrap();
        let dir = engine.board()[0].dir.clone();
        std::fs::write(dir.join(files::SEND), "Yes, on Monday.\n").unwrap();

        let report = engine.sync();
        assert_eq!((report.sent, report.problems.len()), (1, 0), "{report:?}");
        let sent = server.mailbox.lock().unwrap().sent.clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("To: Person 2 <p2@example.com>\r\nSubject: Re: Topic 2\r\n"));
        assert!(sent[0].contains("In-Reply-To: <m2@mail.example.com>\r\n"));
        assert_eq!(engine.board()[0].stage, Stage::Waiting);

        // The next sync sees the sent mail in the history. The folder has it already.
        assert_eq!(engine.sync().received, 0);
        let transcript = std::fs::read_to_string(dir.join(files::TRANSCRIPT)).unwrap();
        assert_eq!(transcript.matches("Yes, on Monday.").count(), 1);
    }

    #[test]
    fn a_lost_or_old_cursor_loads_the_tracked_threads_again() {
        let server = mailbox(5);
        let tmp = TempDir::new("cursor");
        let mut engine = engine(&server, &tmp);
        engine.sync();
        engine.track(CHANNEL, "t1").unwrap();
        server.deliver(
            "t1",
            mail(
                "m1b",
                "Person 1 <p1@example.com>",
                "Re: Topic 1",
                100,
                "More",
            ),
        );
        server.mailbox.lock().unwrap().oldest_history = 1_000;
        let report = engine.sync();
        assert_eq!(
            (report.received, report.problems.len()),
            (1, 0),
            "{report:?}"
        );

        server.deliver(
            "t1",
            mail(
                "m1c",
                "Person 1 <p1@example.com>",
                "Re: Topic 1",
                200,
                "Last",
            ),
        );
        std::fs::remove_file(tmp.0.join(files::STATE_DIR).join("state.json")).unwrap();
        server.mailbox.lock().unwrap().oldest_history = 0;
        let mut engine = self::engine(&server, &tmp);
        assert_eq!(engine.sync().received, 1);
        assert_eq!(engine.sync().received, 0);
    }

    #[test]
    fn a_thread_that_gmail_no_longer_has_does_not_stop_the_sync() {
        let server = mailbox(2);
        let tmp = TempDir::new("gone");
        let mut engine = engine(&server, &tmp);
        engine.track(CHANNEL, "t1").unwrap();
        engine.track(CHANNEL, "t2").unwrap();
        server.mailbox.lock().unwrap().threads.remove(0);
        let report = engine.sync();
        assert!(report.problems.is_empty(), "{report:?}");
        assert_eq!(engine.board().len(), 2);
        assert!(engine.load(CHANNEL, "t1").is_err());
    }

    #[test]
    fn a_new_access_token_replaces_one_that_gmail_refuses() {
        let server = mailbox(1);
        let mut gmail = gmail(&server);
        gmail.browse("", None).unwrap();
        server.mailbox.lock().unwrap().end_access();
        assert_eq!(gmail.browse("", None).unwrap().items.len(), 1);
        assert_eq!(server.mailbox.lock().unwrap().tokens_given, 2);
    }

    #[test]
    fn a_read_goes_again_when_gmail_is_too_busy() {
        let server = mailbox(2);
        let mut gmail = gmail(&server);
        gmail.browse("", None).unwrap();
        server.mailbox.lock().unwrap().busy = 2;
        assert_eq!(gmail.browse("", None).unwrap().items.len(), 2);

        server.mailbox.lock().unwrap().busy = 3;
        let err = gmail.load("t1").unwrap_err();
        assert!(format!("{err:#}").contains("Too many requests"), "{err:#}");
    }

    #[test]
    fn an_ended_sign_in_tells_the_user_what_to_do() {
        let server = mailbox(1);
        let mut credentials = server.credentials();
        credentials.refresh_token = "revoked".into();
        let mut gmail = Gmail::with_endpoints(credentials, server.endpoints.clone());
        let err = gmail.browse("", None).unwrap_err();
        let words = format!("{err:#}");
        assert!(words.contains("connect Gmail again"), "{words}");
        assert!(words.contains("expired or revoked"), "{words}");
    }

    #[test]
    fn results_are_in_the_order_of_the_items() {
        let items: Vec<u32> = (0..50).collect();
        let doubled = in_parallel(&items, |n| Ok(n * 2)).unwrap();
        assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
        assert!(
            in_parallel(&items, |n| if *n == 7 {
                anyhow::bail!("no")
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(in_parallel(&[] as &[u32], |_| Ok(())).unwrap().is_empty());
    }
}
