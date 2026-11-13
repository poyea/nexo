// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mailparse::{DispositionType, MailAddr, MailHeaderMap, ParsedMail};
use nexo_core::html_to_text;

/// A name and an address from a header such as `From`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Person {
    pub name: String,
    pub address: String,
}

impl Person {
    pub fn display(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            self.name.clone()
        }
    }

    pub fn is(&self, address: &str) -> bool {
        self.address.eq_ignore_ascii_case(address)
    }
}

/// One mail of the mailbox.
#[derive(Debug, Clone)]
pub(crate) struct Mail {
    /// The Message-ID with its angle brackets, or the id that nexo made for a mail without
    /// one.
    pub id: String,
    /// The id of the conversation: the Message-ID of the first mail of the thread.
    pub thread: String,
    pub subject: String,
    pub from: Vec<Person>,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    /// The headers as they are in the mail, for a reply.
    pub from_raw: String,
    pub to_raw: String,
    pub reply_to_raw: String,
    pub references_raw: String,
    /// The time in the `Date` header.
    pub date: Option<DateTime<Utc>>,
    /// The text of the mail. It is empty when nexo read only the headers.
    pub text: String,
}

impl Mail {
    pub fn author(&self) -> String {
        self.from.first().map(Person::display).unwrap_or_default()
    }

    pub fn is_from(&self, address: &str) -> bool {
        self.from.iter().any(|person| person.is(address))
    }
}

/// Read a mail, or only its headers. `made_id` is the id for a mail without a Message-ID.
pub(crate) fn parse(raw: &[u8], made_id: &str) -> Result<Mail> {
    let mail = mailparse::parse_mail(raw).context("reading a mail")?;
    let header = |name: &str| mail.headers.get_first_value(name).unwrap_or_default();
    let id = message_ids(&header("Message-ID"))
        .into_iter()
        .next()
        .unwrap_or_else(|| made_id.to_owned());
    let references_raw = header("References");
    let thread = message_ids(&references_raw)
        .into_iter()
        .chain(message_ids(&header("In-Reply-To")))
        .next()
        .unwrap_or_else(|| id.clone());
    let date = Some(header("Date"))
        .filter(|date| !date.trim().is_empty())
        .and_then(|date| mailparse::dateparse(&date).ok())
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0));
    Ok(Mail {
        id,
        thread,
        subject: header("Subject").trim().to_owned(),
        from: people(&header("From")),
        to: people(&header("To")),
        cc: people(&header("Cc")),
        from_raw: header("From"),
        to_raw: header("To"),
        reply_to_raw: header("Reply-To"),
        references_raw,
        date,
        text: body_text(&mail),
    })
}

/// The ids of a header such as `References`, each with its angle brackets.
pub(crate) fn message_ids(header: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = header;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else {
            break;
        };
        let id = &rest[start..=start + len];
        if !id.contains(char::is_whitespace) && id.len() > 2 {
            ids.push(id.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
    ids
}

fn people(header: &str) -> Vec<Person> {
    let Ok(list) = mailparse::addrparse(header) else {
        return Vec::new();
    };
    let single = |info: &mailparse::SingleInfo| Person {
        name: info
            .display_name
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        address: info.addr.clone(),
    };
    list.iter()
        .flat_map(|addr| match addr {
            MailAddr::Single(info) => vec![single(info)],
            MailAddr::Group(group) => group.addrs.iter().map(single).collect(),
        })
        .collect()
}

/// The subject without the marks of a reply or a forward at its start, such as `Re:`.
pub(crate) fn base_subject(subject: &str) -> String {
    let mut rest = subject.trim();
    loop {
        let lower = rest.to_ascii_lowercase();
        let mark = ["re:", "fw:", "fwd:", "aw:"]
            .into_iter()
            .find(|mark| lower.starts_with(mark));
        match mark {
            Some(mark) => rest = rest[mark.len()..].trim_start(),
            None => return rest.to_owned(),
        }
    }
}

/// The text of a mail: its plain text part, or else its HTML part as text. The names of
/// its attachments are at the end.
fn body_text(mail: &ParsedMail) -> String {
    let mut text = find_text(mail, "text/plain")
        .or_else(|| find_text(mail, "text/html").map(|html| html_to_text(&html)))
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .trim()
        .to_owned();
    let mut names = Vec::new();
    attachments(mail, &mut names);
    for name in names {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("[Attachment: {name}]"));
    }
    text
}

fn is_attachment(part: &ParsedMail) -> bool {
    part.get_content_disposition().disposition == DispositionType::Attachment
}

fn find_text(part: &ParsedMail, mime_type: &str) -> Option<String> {
    if part.subparts.is_empty() {
        let fits = part.ctype.mimetype.eq_ignore_ascii_case(mime_type) && !is_attachment(part);
        return fits.then(|| part.get_body().ok()).flatten();
    }
    part.subparts.iter().find_map(|p| find_text(p, mime_type))
}

fn attachments(part: &ParsedMail, names: &mut Vec<String>) {
    if is_attachment(part) {
        let disposition = part.get_content_disposition();
        let name = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"));
        names.push(name.cloned().unwrap_or_else(|| "file".to_owned()));
    }
    for child in &part.subparts {
        attachments(child, names);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mail_has_its_headers_and_its_text() {
        let raw = "Message-ID: <m2@example.com>\r\n\
            In-Reply-To: <m1@example.com>\r\n\
            References: <m0@example.com> <m1@example.com>\r\n\
            From: =?UTF-8?B?5pel5pys?= <jp@example.com>\r\n\
            To: \"Law, John\" <me@example.com>, ann@example.com\r\n\
            Subject: Re: =?utf-8?Q?Caf=C3=A9?=\r\n\
            Date: Mon, 9 Nov 2026 12:00:00 +0000\r\n\
            Content-Type: text/plain; charset=iso-8859-1\r\n\
            Content-Transfer-Encoding: quoted-printable\r\n\r\n\
            Andr=E9 says hi\r\n";
        let mail = parse(raw.as_bytes(), "made").unwrap();
        assert_eq!(mail.id, "<m2@example.com>");
        assert_eq!(mail.thread, "<m0@example.com>");
        assert_eq!(mail.subject, "Re: Café");
        assert_eq!(mail.author(), "日本");
        let to: Vec<String> = mail.to.iter().map(Person::display).collect();
        assert_eq!(to, ["Law, John", "ann@example.com"]);
        assert!(mail.to[0].is("ME@example.com"));
        assert_eq!(mail.date.unwrap().to_rfc3339(), "2026-11-09T12:00:00+00:00");
        assert_eq!(mail.text, "André says hi");
    }

    #[test]
    fn a_mail_without_ids_gets_one_and_is_its_own_thread() {
        let mail = parse(b"Subject: Hi\r\n\r\nText", "made-7").unwrap();
        assert_eq!(
            (mail.id.as_str(), mail.thread.as_str()),
            ("made-7", "made-7")
        );
        assert!(mail.from.is_empty() && mail.date.is_none());
    }

    #[test]
    fn html_and_attachments_show_as_text() {
        let raw = "Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
            --b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
            <p>Your <b>order</b></p><p>Thanks &amp; bye</p>\r\n\
            --b\r\nContent-Type: application/pdf; name=\"invoice.pdf\"\r\n\
            Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nAAAA\r\n--b--\r\n";
        let mail = parse(raw.as_bytes(), "made").unwrap();
        assert_eq!(
            mail.text,
            "Your order\nThanks & bye\n[Attachment: invoice.pdf]"
        );
    }

    #[test]
    fn ids_and_subjects_are_cleaned() {
        assert_eq!(
            message_ids("<a@x> junk <b@y>\r\n <c@z>"),
            ["<a@x>", "<b@y>", "<c@z>"]
        );
        assert!(message_ids("no ids < here").is_empty());
        assert_eq!(base_subject("RE: Fwd: re:  Offer"), "Offer");
        assert_eq!(base_subject("Regarding the offer"), "Regarding the offer");
    }
}
