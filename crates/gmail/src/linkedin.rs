// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;

use anyhow::Result;
use nexo_core::{Batch, Conversation, Direction, Message, Source, Update};
use serde::Deserialize;

use crate::api::{Client, Credentials, Endpoints, status_of};
use crate::mail::Mail;
use crate::source::in_parallel;

pub const LINKEDIN: &str = "linkedin";
/// The addresses that LinkedIn sends the notification of a message from.
const SENDERS: &str = "from:(messages-noreply@linkedin.com OR \
    messaging-digest-noreply@linkedin.com OR inmail-hit-reply@linkedin.com)";
/// How far back the first sync looks.
const FIRST_SYNC: &str = "newer_than:30d";
const PAGE: &str = "100";
/// The most mails that one sync reads.
const MAX_MAILS: usize = 500;
const NO_TEXT: &str = "(The email of LinkedIn has no text. Open LinkedIn to read the message.)";
/// The names that LinkedIn itself sends mail with.
const OWN_NAMES: [&str; 4] = [
    "linkedin",
    "linkedin messaging",
    "linkedin messages",
    "linkedin notifications",
];
/// A subject that ends with one of these starts with the name of the person.
const SUBJECT_ENDS: [&str; 6] = [
    " sent you a new message",
    " sent you a message",
    " just messaged you",
    " sent you an inmail",
    " sent you a new inmail",
    " replied to your message",
];
const SUBJECT_STARTS: [&str; 2] = ["new message from ", "you have a new message from "];
/// The text of the message ends where a line starts with one of these.
const FOOTERS: [&str; 7] = [
    "this email was intended for",
    "you are receiving",
    "you're receiving",
    "unsubscribe",
    "learn why we included this",
    "© ",
    "(c) ",
];
/// A line with only one of these, with or without a link, is not part of the message.
const LINK_LABELS: [&str; 6] = [
    "view message",
    "view messages",
    "view conversation",
    "see all messages",
    "reply",
    "reply to this message",
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailList {
    #[serde(default)]
    messages: Vec<MailRef>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct MailRef {
    id: String,
}

/// The LinkedIn messages of the user as a channel.
///
/// LinkedIn has no API for the messages of a person, so this channel reads the notification
/// emails that LinkedIn sends, in the Gmail of the user. Each person is one conversation. It
/// cannot send: the user replies on LinkedIn. The cursor is the time of the newest email, in
/// milliseconds.
pub struct LinkedIn {
    client: Client,
}

impl LinkedIn {
    /// `credentials` are those of the Gmail that gets the emails of LinkedIn.
    pub fn new(credentials: Credentials) -> Self {
        Self::with_endpoints(credentials, Endpoints::default())
    }

    pub fn with_endpoints(credentials: Credentials, endpoints: Endpoints) -> Self {
        Self {
            client: Client::new(credentials, endpoints),
        }
    }
}

impl Source for LinkedIn {
    fn channel(&self) -> &str {
        LINKEDIN
    }

    fn fetch(&mut self, cursor: Option<&str>, _: &[String]) -> Result<Batch> {
        let since: Option<i64> = cursor.and_then(|c| c.parse().ok());
        let query = match since {
            Some(millis) => format!("{SENDERS} after:{}", millis / 1000),
            None => format!("{SENDERS} {FIRST_SYNC}"),
        };
        self.client.with(|gmail| {
            let mut ids = Vec::new();
            let mut page: Option<String> = None;
            loop {
                let mut params = vec![("q", query.as_str()), ("maxResults", PAGE)];
                params.extend(page.as_deref().map(|token| ("pageToken", token)));
                let list: MailList = gmail.get("messages", &params)?;
                ids.extend(list.messages.into_iter().map(|m| m.id));
                page = list.next_page_token;
                if page.is_none() || ids.len() >= MAX_MAILS {
                    break;
                }
            }
            let mails = in_parallel(&ids, |id| {
                match gmail.get::<Mail>(&format!("messages/{id}"), &[("format", "full")]) {
                    Ok(mail) => Ok(Some(mail)),
                    Err(err) if status_of(&err) == Some(404) => Ok(None),
                    Err(err) => Err(err),
                }
            })?;

            let mut newest = since;
            let mut people: BTreeMap<String, Update> = BTreeMap::new();
            for mail in mails.into_iter().flatten() {
                let sent_at = mail.sent_at().unwrap_or_default();
                newest = newest.max(Some(sent_at.timestamp_millis()));
                let subject = mail.subject();
                let Some(person) = person(&mail.sender_name(), &subject) else {
                    continue;
                };
                let update = people
                    .entry(person.to_lowercase())
                    .or_insert_with_key(|id| Update {
                        conversation: Conversation {
                            id: id.clone(),
                            title: person.clone(),
                            participants: vec![person.clone()],
                        },
                        messages: Vec::new(),
                    });
                update.messages.push(Message {
                    id: mail.id.clone(),
                    author: person,
                    sent_at,
                    direction: Direction::In,
                    body: message_text(&mail.text(), &subject),
                });
            }
            Ok(Batch {
                updates: people.into_values().collect(),
                cursor: newest.map(|millis| millis.to_string()),
            })
        })
    }
}

/// The person who wrote the message, from the name of the sender and the subject of the
/// email. `None` if the email is not about a message of one person.
fn person(from_name: &str, subject: &str) -> Option<String> {
    let name = from_name.trim();
    let name = strip_suffix_ignore_case(name, " via linkedin").unwrap_or(name);
    let own = OWN_NAMES.contains(&name.to_lowercase().as_str());
    if !name.is_empty() && !own && !name.contains('@') {
        return Some(name.to_owned());
    }
    let subject = subject.trim().trim_end_matches(['.', '!']);
    let lower = subject.to_lowercase();
    let from_end = SUBJECT_ENDS
        .iter()
        .find(|end| lower.ends_with(*end))
        .map(|end| &subject[..subject.len() - end.len()]);
    let from_start = SUBJECT_STARTS
        .iter()
        .find(|start| lower.starts_with(*start))
        .map(|start| &subject[start.len()..]);
    from_end
        .or(from_start)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let split = text.len().checked_sub(suffix.len())?;
    (text.is_char_boundary(split) && text[split..].eq_ignore_ascii_case(suffix))
        .then(|| text[..split].trim_end())
}

/// The text of the message, without the links and the footer that LinkedIn adds.
fn message_text(body: &str, subject: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in body.lines().map(str::trim) {
        let lower = line.to_lowercase();
        if FOOTERS.iter().any(|footer| lower.starts_with(footer)) {
            break;
        }
        let label = lower.split("http").next().unwrap_or_default();
        let label = label.trim().trim_end_matches(':').trim();
        let only_link = label.is_empty() && lower.starts_with("http");
        if only_link || LINK_LABELS.contains(&label) || line == subject.trim() {
            continue;
        }
        if line.chars().all(|c| c == '-' || c == '_' || c == '=') && !line.is_empty() {
            continue;
        }
        if line.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    let text = lines.join("\n").trim().to_owned();
    if text.is_empty() {
        NO_TEXT.to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use nexo_core::{Engine, Folder, Stage, files};

    use super::*;
    use crate::mock::{Server, mail};

    const BODY: &str = "Jane Doe sent you a new message\r\n\r\n\
        Hi John,\r\n\r\nAre you free on Monday?\r\nReply when you can.\r\n\r\n\
        View message: https://www.linkedin.com/comm/messaging/thread/1\r\n\
        https://www.linkedin.com/comm/messaging/\r\n\r\n\
        ----------------------------------------\r\n\r\n\
        This email was intended for John Law (Engineer).\r\n\
        Learn why we included this: https://www.linkedin.com/help\r\n\
        Unsubscribe: https://www.linkedin.com/unsub\r\n\
        © 2026 LinkedIn Corporation";

    #[test]
    fn the_person_comes_from_the_sender_or_the_subject() {
        let found = |from: &str, subject: &str| person(from, subject);
        assert_eq!(found("Jane Doe via LinkedIn", "x"), Some("Jane Doe".into()));
        assert_eq!(
            found("Jane Doe Via LinkedIn ", "x"),
            Some("Jane Doe".into())
        );
        assert_eq!(
            found("LinkedIn", "Jane Doe sent you a new message"),
            Some("Jane Doe".into())
        );
        assert_eq!(
            found("LinkedIn Messaging", "New message from 陳大文."),
            Some("陳大文".into())
        );
        assert_eq!(
            found("messages-noreply@linkedin.com", "Ann just messaged you!"),
            Some("Ann".into())
        );
        assert_eq!(found("LinkedIn", "You have 3 new notifications"), None);
        assert_eq!(found("", ""), None);
    }

    #[test]
    fn the_text_has_no_links_and_no_footer() {
        assert_eq!(
            message_text(BODY, "Jane Doe sent you a new message"),
            "Hi John,\n\nAre you free on Monday?\nReply when you can."
        );
        assert_eq!(message_text("View message: https://x\n", "s"), NO_TEXT);
    }

    #[test]
    fn each_person_is_one_conversation_that_cannot_send() {
        let server = Server::start();
        let from = "Jane Doe via LinkedIn <messaging-digest-noreply@linkedin.com>";
        server.deliver(
            "t1",
            mail("m1", from, "Jane Doe sent you a new message", 1_000, BODY),
        );
        server.deliver(
            "t2",
            mail(
                "m2",
                from,
                "Jane Doe sent you a new message",
                2_000,
                "Second",
            ),
        );
        server.deliver(
            "t3",
            mail(
                "m3",
                "LinkedIn <messages-noreply@linkedin.com>",
                "Ann Lee just messaged you",
                3_000,
                "Hello",
            ),
        );
        server.deliver(
            "t4",
            mail("m4", "Shop <shop@example.com>", "Sale", 4_000, "Buy"),
        );
        server.deliver(
            "t5",
            mail(
                "m5",
                "LinkedIn <messages-noreply@linkedin.com>",
                "You have 3 new notifications",
                5_000,
                "See them",
            ),
        );

        let tmp = std::env::temp_dir().join(format!("nexo-linkedin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut engine = Engine::new(Folder::open(&tmp).unwrap()).unwrap();
        let source = LinkedIn::with_endpoints(server.credentials(), server.endpoints.clone());
        engine.add(Box::new(source)).unwrap();

        let report = engine.sync();
        assert_eq!(
            (report.received, report.problems.len()),
            (3, 0),
            "{report:?}"
        );
        let board = engine.board();
        let seen: Vec<_> = board
            .iter()
            .map(|c| (c.channel.as_str(), c.title.as_str(), c.stage))
            .collect();
        assert_eq!(
            seen,
            [
                (LINKEDIN, "Ann Lee", Stage::ToReply),
                (LINKEDIN, "Jane Doe", Stage::ToReply),
            ]
        );
        assert_eq!(board[1].latest.as_ref().unwrap().body, "Second");

        // The next sync asks only for the emails after the newest one.
        let before = server.requests().len();
        assert_eq!(engine.sync().received, 0);
        let asked = server.requests()[before..].join(" ");
        assert!(asked.contains("5000"), "{asked}");
        server.deliver(
            "t6",
            mail(
                "m6",
                from,
                "Jane Doe sent you a new message",
                6_000,
                "Third",
            ),
        );
        assert_eq!(engine.sync().received, 1);

        std::fs::write(board[1].dir.join(files::SEND), "Yes").unwrap();
        let report = engine.sync();
        assert_eq!(report.sent, 0);
        assert!(report.problems[0].contains("cannot send"), "{report:?}");
        assert!(board[1].dir.join(files::UNSENT).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
