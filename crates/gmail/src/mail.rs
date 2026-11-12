// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nexo_core::{Conversation, Direction, Message, Summary, Update, decode_entities, html_to_text};
use serde::Deserialize;

use crate::text::{base64_lines, base64url, decode_words, encode_words, from_base64url};

const NO_SUBJECT: &str = "(no subject)";

/// A thread, as the Gmail API gives it.
#[derive(Debug, Deserialize)]
pub(crate) struct Thread {
    pub id: String,
    #[serde(default)]
    pub messages: Vec<Mail>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mail {
    pub id: String,
    #[serde(default)]
    label_ids: Vec<String>,
    /// Milliseconds from 1970, as text.
    #[serde(default)]
    internal_date: String,
    #[serde(default)]
    payload: Part,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    headers: Vec<Header>,
    #[serde(default)]
    body: Body,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Debug, Default, Deserialize)]
struct Body {
    #[serde(default)]
    data: Option<String>,
}

impl Mail {
    fn header(&self, name: &str) -> Option<&str> {
        self.payload
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    fn has_label(&self, label: &str) -> bool {
        self.label_ids.iter().any(|l| l == label)
    }

    pub fn subject(&self) -> String {
        decode_words(self.header("Subject").unwrap_or_default())
            .trim()
            .to_owned()
    }

    /// The name of the sender, or the address if the sender gave no name.
    pub fn sender_name(&self) -> String {
        self.author()
    }

    /// The text of the mail. See [`body_text`].
    pub fn text(&self) -> String {
        body_text(&self.payload)
    }

    pub fn sent_at(&self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_millis(self.internal_date.parse().ok()?)
    }

    fn direction(&self) -> Direction {
        if self.has_label("SENT") {
            Direction::Out
        } else {
            Direction::In
        }
    }

    fn from(&self) -> Option<Person> {
        people(self.header("From").unwrap_or_default())
            .into_iter()
            .next()
    }

    fn author(&self) -> String {
        self.from().map(|p| p.display()).unwrap_or_default()
    }
}

impl Thread {
    fn subject(&self) -> String {
        self.messages
            .first()
            .and_then(|m| m.header("Subject"))
            .map(decode_words)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| NO_SUBJECT.to_owned())
    }

    /// The thread as a conversation with all its messages. `me` is the address of the user.
    pub fn into_update(self, me: &str) -> Update {
        let mut participants: Vec<Person> = Vec::new();
        for mail in &self.messages {
            for name in ["From", "To", "Cc"] {
                for person in people(mail.header(name).unwrap_or_default()) {
                    let known = participants.iter().any(|p| p.is(&person.address));
                    if !known && !person.is(me) {
                        participants.push(person);
                    }
                }
            }
        }
        let conversation = Conversation {
            title: self.subject(),
            participants: participants.iter().map(Person::display).collect(),
            id: self.id,
        };
        let messages = self
            .messages
            .iter()
            .map(|mail| Message {
                id: mail.id.clone(),
                author: mail.author(),
                sent_at: mail.sent_at().unwrap_or_default(),
                direction: mail.direction(),
                body: body_text(&mail.payload),
            })
            .collect();
        Update {
            conversation,
            messages,
        }
    }

    /// The thread as a line in a list. `snippet` is the start of its newest message, as the
    /// list of threads gives it.
    pub fn into_summary(self, snippet: &str) -> Summary {
        let newest = self.messages.last();
        Summary {
            title: self.subject(),
            from: newest.map(Mail::author).unwrap_or_default(),
            latest_at: newest.and_then(Mail::sent_at),
            preview: decode_entities(snippet),
            unread: self.messages.iter().any(|m| m.has_label("UNREAD")),
            messages: self.messages.len(),
            tracked: false,
            id: self.id,
        }
    }

    /// A reply to the newest message of another person, as the `raw` value of the Gmail
    /// API. If all messages are from the user, the reply goes to the same people as the
    /// newest message.
    pub fn reply(&self, body: &str) -> Result<String> {
        let incoming = self
            .messages
            .iter()
            .rev()
            .find(|m| m.direction() == Direction::In);
        let to = match incoming {
            Some(mail) => mail.header("Reply-To").or_else(|| mail.header("From")),
            None => self.messages.last().and_then(|m| m.header("To")),
        }
        .map(str::trim)
        .filter(|to| !to.is_empty())
        .context("the conversation has no address to reply to")?;
        let newest = incoming.or(self.messages.last());
        let subject = self.subject();
        let subject = if subject.to_ascii_lowercase().starts_with("re:") {
            subject
        } else {
            format!("Re: {subject}")
        };

        let mut mail = format!("To: {}\r\n", one_line(to));
        mail.push_str(&format!("Subject: {}\r\n", encode_words(&subject)));
        if let Some(id) = newest.and_then(|m| m.header("Message-ID")) {
            let id = one_line(id);
            let before = newest
                .and_then(|m| m.header("References"))
                .map(one_line)
                .unwrap_or_default();
            mail.push_str(&format!("In-Reply-To: {id}\r\n"));
            mail.push_str(&format!(
                "References: {}\r\n",
                format!("{before} {id}").trim()
            ));
        }
        mail.push_str("MIME-Version: 1.0\r\n");
        mail.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        mail.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
        mail.push_str(&base64_lines(body.as_bytes()));
        Ok(base64url(mail.as_bytes()))
    }
}

/// A header value on one line, so that it cannot start a second header.
fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A name and an address from a header such as `From`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Person {
    name: String,
    address: String,
}

impl Person {
    fn display(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            self.name.clone()
        }
    }

    fn is(&self, address: &str) -> bool {
        self.address.eq_ignore_ascii_case(address)
    }
}

/// The people of a header such as `"Doe, Jane" <jane@example.com>, john@example.com`.
pub(crate) fn people(header: &str) -> Vec<Person> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let (mut quoted, mut angled) = (false, false);
    for c in header.chars() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            ',' if !quoted && !angled => {
                parts.push(std::mem::take(&mut part));
                continue;
            }
            _ => {}
        }
        part.push(c);
    }
    parts.push(part);
    parts
        .iter()
        .filter_map(|part| {
            let part = part.trim();
            let (name, address) = match (part.rfind('<'), part.rfind('>')) {
                (Some(open), Some(close)) if open < close => {
                    (&part[..open], &part[open + 1..close])
                }
                _ => ("", part),
            };
            let name = decode_words(name.trim().trim_matches('"').trim());
            let address = address.trim().to_owned();
            (!address.is_empty()).then_some(Person { name, address })
        })
        .collect()
}

/// The text of a mail: its plain text part, or else its HTML part as text. The names of
/// its attachments are at the end.
fn body_text(payload: &Part) -> String {
    let mut text = find_text(payload, "text/plain")
        .or_else(|| find_text(payload, "text/html").map(|html| html_to_text(&html)))
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .trim()
        .to_owned();
    let mut names = Vec::new();
    attachments(payload, &mut names);
    for name in names {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("[Attachment: {name}]"));
    }
    text
}

fn find_text(part: &Part, mime_type: &str) -> Option<String> {
    if part.filename.is_empty()
        && part.mime_type.eq_ignore_ascii_case(mime_type)
        && let Some(bytes) = part.body.data.as_deref().and_then(from_base64url)
    {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    part.parts.iter().find_map(|p| find_text(p, mime_type))
}

fn attachments<'a>(part: &'a Part, names: &mut Vec<&'a str>) {
    if !part.filename.is_empty() {
        names.push(&part.filename);
    }
    for child in &part.parts {
        attachments(child, names);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{mail, thread};

    fn parsed(value: serde_json::Value) -> Thread {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn people_have_names_and_addresses() {
        let found = people(
            r#""Doe, Jane" <jane@example.com>, john@example.com, =?UTF-8?B?5pel?= <n@x.jp>"#,
        );
        let shown: Vec<String> = found.iter().map(Person::display).collect();
        assert_eq!(shown, ["Doe, Jane", "john@example.com", "日"]);
        assert!(found[0].is("JANE@example.com"));
        assert!(people("").is_empty());
    }

    #[test]
    fn a_thread_becomes_a_conversation() {
        let json = thread(
            "t1",
            vec![
                mail(
                    "m1",
                    "Jane Doe <jane@example.com>",
                    "Offer",
                    1_000,
                    "Hi John",
                ),
                mail("m2", "me@example.com", "Re: Offer", 2_000, "Hi Jane").sent(),
            ],
        );
        let update = parsed(json).into_update("me@example.com");
        assert_eq!(update.conversation.id, "t1");
        assert_eq!(update.conversation.title, "Offer");
        assert_eq!(update.conversation.participants, ["Jane Doe"]);
        let seen: Vec<_> = update
            .messages
            .iter()
            .map(|m| {
                (
                    m.id.as_str(),
                    m.author.as_str(),
                    m.direction,
                    m.body.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("m1", "Jane Doe", Direction::In, "Hi John"),
                ("m2", "me@example.com", Direction::Out, "Hi Jane"),
            ]
        );
        assert_eq!(update.messages[0].sent_at.timestamp(), 1_000);
    }

    #[test]
    fn an_html_mail_shows_as_text_with_its_attachments() {
        let json = thread(
            "t1",
            vec![
                mail("m1", "shop@example.com", "", 1_000, "")
                    .html("<p>Your <b>order</b></p><p>Thanks &amp; bye</p>")
                    .attachment("invoice.pdf"),
            ],
        );
        let update = parsed(json).into_update("me@example.com");
        assert_eq!(update.conversation.title, NO_SUBJECT);
        assert_eq!(
            update.messages[0].body,
            "Your order\nThanks & bye\n[Attachment: invoice.pdf]"
        );
    }

    #[test]
    fn a_summary_tells_of_the_newest_message() {
        let json = thread(
            "t1",
            vec![
                mail("m1", "Jane <jane@example.com>", "Offer", 1_000, "Hi"),
                mail("m2", "Ann <ann@example.com>", "Re: Offer", 2_000, "Me too").unread(),
            ],
        );
        let summary = parsed(json).into_summary("Me too, it&#39;s good");
        assert_eq!(summary.title, "Offer");
        assert_eq!(summary.from, "Ann");
        assert_eq!(summary.latest_at.unwrap().timestamp(), 2_000);
        assert_eq!(summary.preview, "Me too, it's good");
        assert!(summary.unread);
        assert_eq!(summary.messages, 2);
    }

    #[test]
    fn a_reply_goes_to_the_newest_other_person_in_the_thread() {
        let json = thread(
            "t1",
            vec![
                mail("m1", "Jane <jane@example.com>", "Offer", 1_000, "Hi"),
                mail("m2", "me@example.com", "Re: Offer", 2_000, "Hello").sent(),
            ],
        );
        let raw = parsed(json).reply("Yes, 日本").unwrap();
        let text = String::from_utf8(from_base64url(&raw).unwrap()).unwrap();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            headers.lines().collect::<Vec<_>>(),
            [
                "To: Jane <jane@example.com>",
                "Subject: Re: Offer",
                "In-Reply-To: <m1@mail.example.com>",
                "References: <m1@mail.example.com>",
                "MIME-Version: 1.0",
                "Content-Type: text/plain; charset=utf-8",
                "Content-Transfer-Encoding: base64",
            ]
        );
        let body: String = body.lines().collect();
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body);
        assert_eq!(decoded.unwrap(), "Yes, 日本".as_bytes());
    }

    #[test]
    fn a_reply_to_my_own_thread_goes_to_the_same_people() {
        let json = thread(
            "t1",
            vec![
                mail("m1", "me@example.com", "Ping", 1_000, "Hello")
                    .sent()
                    .to("me-to@example.com"),
            ],
        );
        let raw = parsed(json).reply("And again").unwrap();
        let text = String::from_utf8(from_base64url(&raw).unwrap()).unwrap();
        assert!(text.starts_with("To: me-to@example.com\r\nSubject: Re: Ping\r\n"));
    }
}
