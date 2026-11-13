// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The IMAP and SMTP servers of an account.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use imap::types::Flag;
use imap::{ClientBuilder, ConnectionMode};
use imap_proto::NameAttribute;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Mailboxes};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};

use crate::account::{Account, IMAP_PORT, SMTP_PORT};

const TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const INBOX: &str = "INBOX";
/// The names of the folder of sent mail, for a server that does not mark it.
const SENT_NAMES: [&str; 6] = [
    "sent",
    "sent items",
    "sent messages",
    "sent mail",
    "inbox.sent",
    "[gmail]/sent mail",
];

pub(crate) type Session = imap::Session<imap::Connection>;

/// Connect to the IMAP server and sign in.
pub(crate) fn connect(account: &Account) -> Result<Session> {
    let server = &account.imap;
    let mode = match (account.plaintext, server.port) {
        (true, _) => ConnectionMode::Plaintext,
        (false, IMAP_PORT) => ConnectionMode::Tls,
        (false, _) => ConnectionMode::StartTls,
    };
    let client = ClientBuilder::new(server.host.as_str(), server.port)
        .mode(mode)
        .danger_skip_tls_verify(server.is_local())
        .connect()
        .with_context(|| format!("connecting to {}:{}", server.host, server.port))?;
    client
        .login(&account.username, &account.password)
        .map_err(|(err, _)| anyhow!("{err}"))
        .with_context(|| format!("signing in to {}", server.host))
}

/// What a folder has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Folder {
    /// How many mails.
    pub exists: u32,
    /// The UIDs of the folder are those of before only if this is the same.
    pub uid_validity: u32,
    /// The UID that the next new mail gets.
    pub uid_next: u32,
}

/// Open a folder to read it.
pub(crate) fn examine(session: &mut Session, name: &str) -> Result<Folder> {
    let mailbox = session
        .examine(name)
        .with_context(|| format!("opening the folder {name}"))?;
    Ok(Folder {
        exists: mailbox.exists,
        uid_validity: mailbox.uid_validity.unwrap_or(0),
        uid_next: mailbox.uid_next.unwrap_or(0),
    })
}

/// The folder of sent mail, if the server has one.
pub(crate) fn sent_folder(session: &mut Session) -> Result<Option<String>> {
    let names = session
        .list(Some(""), Some("*"))
        .context("listing the folders")?;
    let selectable = |attributes: &[NameAttribute]| !attributes.contains(&NameAttribute::NoSelect);
    let marked = names
        .iter()
        .find(|name| name.attributes().contains(&NameAttribute::Sent));
    let by_name = || {
        names.iter().find(|name| {
            selectable(name.attributes())
                && SENT_NAMES.contains(&name.name().to_lowercase().as_str())
        })
    };
    Ok(marked.or_else(by_name).map(|name| name.name().to_owned()))
}

/// A mail as the server gave it.
pub(crate) struct Fetched {
    pub uid: u32,
    pub seen: bool,
    /// When the server got the mail.
    pub received: Option<DateTime<Utc>>,
    /// The mail, or only its headers.
    pub raw: Vec<u8>,
}

/// What [`fetch`] gets of each mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Headers,
    All,
}

/// Get the mails of `set` from the open folder. `set` has UIDs if `by_uid` is true, else
/// positions in the folder. The mails stay unread.
pub(crate) fn fetch(
    session: &mut Session,
    set: &str,
    by_uid: bool,
    part: Part,
) -> Result<Vec<Fetched>> {
    if set.is_empty() {
        return Ok(Vec::new());
    }
    let query = match part {
        Part::Headers => "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])",
        Part::All => "(UID FLAGS INTERNALDATE BODY.PEEK[])",
    };
    let fetches = if by_uid {
        session.uid_fetch(set, query)
    } else {
        session.fetch(set, query)
    }
    .context("reading mails")?;
    Ok(fetches
        .iter()
        .filter_map(|fetch| {
            let raw = match part {
                Part::Headers => fetch.header(),
                Part::All => fetch.body(),
            };
            Some(Fetched {
                uid: fetch.uid?,
                seen: fetch.flags().contains(&Flag::Seen),
                received: fetch.internal_date().map(|date| date.with_timezone(&Utc)),
                raw: raw?.to_vec(),
            })
        })
        .collect())
}

/// The UIDs of the mails of the open folder that match, the largest first.
pub(crate) fn search(session: &mut Session, criteria: &str) -> Result<Vec<u32>> {
    let mut uids: Vec<u32> = session
        .uid_search(criteria)
        .context("searching the mailbox")?
        .into_iter()
        .collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    Ok(uids)
}

/// A text as a quoted string of IMAP.
pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\r' | '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A list of UIDs as a set of IMAP, such as `4,7,9`.
pub(crate) fn uid_set(uids: &[u32]) -> String {
    let uids: Vec<String> = uids.iter().map(u32::to_string).collect();
    uids.join(",")
}

fn smtp(account: &Account) -> Result<SmtpTransport> {
    let server = &account.smtp;
    let tls = || {
        TlsParameters::builder(server.host.clone())
            .dangerous_accept_invalid_certs(server.is_local())
            .build()
            .context("setting up TLS")
    };
    let tls = match (account.plaintext, server.port) {
        (true, _) => Tls::None,
        (false, SMTP_PORT) => Tls::Wrapper(tls()?),
        (false, _) => Tls::Required(tls()?),
    };
    Ok(SmtpTransport::builder_dangerous(server.host.as_str())
        .port(server.port)
        .tls(tls)
        .credentials(Credentials::new(
            account.username.clone(),
            account.password.clone(),
        ))
        .timeout(Some(TIMEOUT))
        .build())
}

/// Sign in to both servers, to see that the account works.
pub fn check(account: &Account) -> Result<()> {
    let mut session = connect(account)?;
    examine(&mut session, INBOX)?;
    let _ = session.logout();
    let server = &account.smtp;
    smtp(account)?
        .test_connection()
        .map_err(|err| anyhow!("{err}"))
        .with_context(|| format!("connecting to {}:{}", server.host, server.port))?;
    Ok(())
}

/// A mail to send.
pub(crate) struct Outgoing<'a> {
    /// The `Message-ID`, with its angle brackets.
    pub id: &'a str,
    /// The people to send to, as a header value.
    pub to: &'a str,
    pub subject: &'a str,
    /// The `Message-ID` of the mail that this one answers.
    pub in_reply_to: Option<&'a str>,
    pub references: &'a str,
    pub body: &'a str,
}

pub(crate) fn send(account: &Account, mail: &Outgoing) -> Result<()> {
    let from: Mailbox = account
        .address
        .parse()
        .with_context(|| format!("{:?} is not a mail address", account.address))?;
    let to: Mailboxes = mail
        .to
        .parse()
        .with_context(|| format!("{:?} is not a list of mail addresses", mail.to))?;
    let mut builder = Message::builder()
        .from(from)
        .subject(mail.subject)
        .message_id(Some(mail.id.to_owned()))
        .header(ContentType::TEXT_PLAIN);
    for mailbox in to {
        builder = builder.to(mailbox);
    }
    if let Some(id) = mail.in_reply_to {
        builder = builder.in_reply_to(id.to_owned());
    }
    if !mail.references.is_empty() {
        builder = builder.references(mail.references.to_owned());
    }
    let message = builder
        .body(mail.body.to_owned())
        .context("making the mail")?;
    smtp(account)?
        .send(&message)
        .map_err(|err| anyhow!("{err}"))
        .with_context(|| format!("sending through {}", account.smtp.host))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imap_texts_are_safe() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("a \"b\" \\ c\r\nd"), "\"a \\\"b\\\" \\\\ c  d\"");
        assert_eq!(uid_set(&[9, 7, 4]), "9,7,4");
        assert_eq!(uid_set(&[]), "");
    }
}
