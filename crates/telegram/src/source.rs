// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use chrono::DateTime;
use grammers_client::client::{LoginToken, PasswordToken};
use grammers_client::message::Message as Sent;
use grammers_client::peer::Dialog;
use grammers_client::session::types::PeerRef;
use grammers_client::{Client, SenderPool, SignInError, tl};
use grammers_session::storages::SqliteSession;
use nexo_core::{
    Batch, Conversation, Direction, Message, PAGE_ITEMS, Page, Source, Summary, Update,
};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

pub const CHANNEL: &str = "telegram";
/// The most chats that the list has.
const MAX_CHATS: usize = 500;
/// How many of the newest messages of a chat nexo reads at one time.
const NEWEST: usize = 50;
const MAX_PREVIEW_CHARS: usize = 200;
const MEDIA: &str = "[Media]";
const SAVED: &str = "Saved Messages";

/// The API id and hash of the user, from <https://my.telegram.org>.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub api_id: i32,
    pub api_hash: String,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("api_id", &self.api_id)
            .finish_non_exhaustive()
    }
}

/// A connection to Telegram. The runtime runs the network part on a thread of its own.
struct Connection {
    runtime: Runtime,
    client: Client,
}

impl Connection {
    /// `session` is the file that keeps the sign-in.
    fn open(app: &App, session: &Path) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .context("starting the Telegram thread")?;
        let client = runtime.block_on(async {
            let session = SqliteSession::open(session)
                .await
                .map_err(|err| anyhow!("{err}"))
                .with_context(|| format!("opening {}", session.display()))?;
            let SenderPool { runner, handle, .. } = SenderPool::new(Arc::new(session), app.api_id);
            tokio::spawn(runner.run());
            anyhow::Ok(Client::new(handle))
        })?;
        Ok(Self { runtime, client })
    }

    fn is_signed_in(&self) -> Result<bool> {
        self.runtime
            .block_on(self.client.is_authorized())
            .context("asking Telegram for the sign-in")
    }
}

/// What a step of the sign-in gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The sign-in is complete: [`Login::finish`] gives the channel.
    SignedIn,
    /// The account has two-step verification: give the password to [`Login::password`].
    /// The hint is the one that the user wrote for the password.
    NeedsPassword(Option<String>),
    /// The code is wrong. Give the correct one to [`Login::code`].
    WrongCode,
    /// The password is wrong. Give the correct one to [`Login::password`].
    WrongPassword,
}

/// A sign-in to Telegram. Telegram sends a code to the Telegram apps of the user, or as an
/// SMS.
pub struct Login {
    connection: Connection,
    code: LoginToken,
    password: Option<PasswordToken>,
}

impl Login {
    /// Ask Telegram to send the code. `phone` is in the international form, such as
    /// `+85212345678`.
    pub fn start(app: &App, session: &Path, phone: &str) -> Result<Self> {
        let connection = Connection::open(app, session)?;
        let code = connection
            .runtime
            .block_on(connection.client.request_login_code(phone, &app.api_hash))
            .context("asking Telegram for a code")?;
        Ok(Self {
            connection,
            code,
            password: None,
        })
    }

    /// Give the code that Telegram sent.
    pub fn code(&mut self, code: &str) -> Result<Progress> {
        let client = &self.connection.client;
        let result = self
            .connection
            .runtime
            .block_on(client.sign_in(&self.code, code.trim()));
        match result {
            Ok(_) => Ok(Progress::SignedIn),
            Err(SignInError::PasswordRequired(token)) => {
                let hint = token.hint().map(str::to_owned);
                self.password = Some(token);
                Ok(Progress::NeedsPassword(hint))
            }
            Err(SignInError::InvalidCode) => Ok(Progress::WrongCode),
            Err(err) => Err(anyhow!("{err}")).context("signing in to Telegram"),
        }
    }

    /// Give the password of the two-step verification.
    pub fn password(&mut self, password: &str) -> Result<Progress> {
        let token = self
            .password
            .take()
            .context("Telegram did not ask for a password")?;
        let client = &self.connection.client;
        let result = self
            .connection
            .runtime
            .block_on(client.check_password(token, password));
        match result {
            Ok(_) => Ok(Progress::SignedIn),
            Err(SignInError::InvalidPassword(token)) => {
                self.password = Some(token);
                Ok(Progress::WrongPassword)
            }
            Err(err) => Err(anyhow!("{err}")).context("signing in to Telegram"),
        }
    }

    /// The channel, after [`Progress::SignedIn`].
    pub fn finish(self) -> Telegram {
        Telegram::new(self.connection)
    }
}

/// A chat, as the list of chats gave it.
struct Chat {
    peer: PeerRef,
    summary: Summary,
    /// The id of the newest message.
    newest: Option<i32>,
}

/// The Telegram account of a person as a channel.
///
/// The user picks the chats that the folder keeps. A sync reads the list of chats, which
/// has the newest message of each chat, and loads only the picked chats whose newest message
/// is not the one of the cursor. The cursor has the id of the newest message of each picked
/// chat.
pub struct Telegram {
    connection: Connection,
    /// The chats, the one with the newest message first.
    chats: Vec<Chat>,
}

impl Telegram {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            chats: Vec::new(),
        }
    }

    /// Connect with the sign-in that the session file keeps. `None` if the file has no
    /// sign-in, or if the user ended it in Telegram.
    pub fn open(app: &App, session: &Path) -> Result<Option<Self>> {
        let connection = Connection::open(app, session)?;
        Ok(connection.is_signed_in()?.then(|| Self::new(connection)))
    }

    /// End the sign-in at Telegram. The session file is of no use after this.
    pub fn sign_out(self) -> Result<()> {
        let Connection { runtime, client } = self.connection;
        runtime
            .block_on(client.sign_out())
            .map(drop)
            .context("ending the sign-in at Telegram")
    }

    /// Read the list of chats again.
    fn refresh(&mut self) -> Result<()> {
        let client = &self.connection.client;
        let dialogs = self.connection.runtime.block_on(async {
            let mut found = Vec::new();
            let mut dialogs = client.iter_dialogs();
            while found.len() < MAX_CHATS
                && let Some(dialog) = dialogs.next().await?
            {
                found.push(dialog);
            }
            Ok::<_, grammers_client::InvocationError>(found)
        });
        let dialogs = dialogs.context("reading the chats of Telegram")?;
        self.chats = dialogs.iter().map(chat).collect();
        Ok(())
    }

    /// The chat with this id. It reads the list of chats again if the chat is not there.
    fn chat(&mut self, id: &str) -> Result<&Chat> {
        if !self.chats.iter().any(|chat| chat.summary.id == id) {
            self.refresh()?;
        }
        self.chats
            .iter()
            .find(|chat| chat.summary.id == id)
            .with_context(|| format!("Telegram has no chat {id}"))
    }

    /// The newest messages of a chat, as a conversation.
    fn load_chat(&mut self, id: &str) -> Result<Update> {
        let chat = self.chat(id)?;
        let (peer, title) = (chat.peer, chat.summary.title.clone());
        let client = &self.connection.client;
        let messages = self.connection.runtime.block_on(async {
            let mut found = Vec::new();
            let mut messages = client.iter_messages(peer).limit(NEWEST);
            while let Some(message) = messages.next().await? {
                found.push(message);
            }
            Ok::<_, grammers_client::InvocationError>(found)
        });
        let messages = messages.with_context(|| format!("reading the chat {title}"))?;
        Ok(Update {
            conversation: Conversation {
                id: id.to_owned(),
                participants: vec![title.clone()],
                title: title.clone(),
            },
            messages: messages.iter().map(|m| message(m, &title)).collect(),
        })
    }
}

impl Source for Telegram {
    fn channel(&self) -> &str {
        CHANNEL
    }

    fn fetch(&mut self, cursor: Option<&str>, tracked: &[String]) -> Result<Batch> {
        if tracked.is_empty() {
            return Ok(Batch::default());
        }
        self.refresh()?;
        let before = newest_ids(cursor.unwrap_or_default());
        let mut now = BTreeMap::new();
        let mut updates = Vec::new();
        for id in tracked {
            let Some(chat) = self.chats.iter().find(|chat| chat.summary.id == *id) else {
                continue;
            };
            let newest = chat.newest.unwrap_or(0);
            if before.get(id) != Some(&newest) {
                updates.push(self.load_chat(id)?);
            }
            now.insert(id.clone(), newest);
        }
        Ok(Batch {
            updates,
            cursor: Some(cursor_text(&now)),
        })
    }

    fn can_reply(&self) -> bool {
        true
    }

    fn reply(&mut self, conversation: &Conversation, body: &str) -> Result<Message> {
        let chat = self.chat(&conversation.id)?;
        let (peer, title) = (chat.peer, chat.summary.title.clone());
        let sent = self
            .connection
            .runtime
            .block_on(self.connection.client.send_message(peer, body))
            .with_context(|| format!("sending to {title}"))?;
        Ok(message(&sent, &title))
    }

    fn can_browse(&self) -> bool {
        true
    }

    fn browse(&mut self, query: &str, page: Option<&str>) -> Result<Page> {
        let start: usize = page.and_then(|token| token.parse().ok()).unwrap_or(0);
        if start == 0 || self.chats.is_empty() {
            self.refresh()?;
        }
        let query = query.trim().to_lowercase();
        let found: Vec<&Summary> = self
            .chats
            .iter()
            .map(|chat| &chat.summary)
            .filter(|summary| summary.title.to_lowercase().contains(&query))
            .collect();
        let end = (start + PAGE_ITEMS).min(found.len());
        Ok(Page {
            items: found
                .get(start..end)
                .unwrap_or_default()
                .iter()
                .map(|summary| (*summary).clone())
                .collect(),
            next: (end < found.len()).then(|| end.to_string()),
        })
    }

    fn load(&mut self, id: &str) -> Result<Update> {
        self.load_chat(id)
    }
}

fn chat(dialog: &Dialog) -> Chat {
    let peer = dialog.peer();
    let title = peer.name().unwrap_or(SAVED).to_owned();
    let last = dialog.last_message.as_ref();
    let unread = match &dialog.raw {
        tl::enums::Dialog::Dialog(dialog) => dialog.unread_count > 0,
        _ => false,
    };
    Chat {
        peer: dialog.peer_ref(),
        newest: last.map(Sent::id),
        summary: Summary {
            id: dialog.peer_id().to_string(),
            from: last.map(|m| author(m, &title)).unwrap_or_default(),
            latest_at: last.and_then(|m| DateTime::from_timestamp(m.date().as_second(), 0)),
            preview: last.map(|m| preview(&text(m))).unwrap_or_default(),
            unread,
            messages: 1,
            tracked: false,
            title,
        },
    }
}

/// Who wrote the message: the user, the person, or else the chat.
fn author(message: &Sent, chat: &str) -> String {
    if message.outgoing() {
        return "You".to_owned();
    }
    message
        .sender()
        .and_then(|sender| sender.name())
        .unwrap_or(chat)
        .to_owned()
}

/// The text of the message, with a mark if it has a picture or a file.
fn text(message: &Sent) -> String {
    body(message.text(), message.media().is_some())
}

fn body(text: &str, media: bool) -> String {
    match (text.trim(), media) {
        ("", true) => MEDIA.to_owned(),
        (text, true) => format!("{text}\n{MEDIA}"),
        (text, false) => text.to_owned(),
    }
}

fn preview(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(MAX_PREVIEW_CHARS).collect()
}

fn message(sent: &Sent, chat: &str) -> Message {
    Message {
        id: sent.id().to_string(),
        author: author(sent, chat),
        sent_at: DateTime::from_timestamp(sent.date().as_second(), 0).unwrap_or_default(),
        direction: if sent.outgoing() {
            Direction::Out
        } else {
            Direction::In
        },
        body: text(sent),
    }
}

/// The cursor: one line for each picked chat, with its id, a tab, and the id of its newest
/// message.
fn cursor_text(newest: &BTreeMap<String, i32>) -> String {
    let lines: Vec<String> = newest
        .iter()
        .map(|(chat, message)| format!("{chat}\t{message}"))
        .collect();
    lines.join("\n")
}

fn newest_ids(cursor: &str) -> BTreeMap<String, i32> {
    cursor
        .lines()
        .filter_map(|line| {
            let (chat, message) = line.split_once('\t')?;
            Some((chat.to_owned(), message.parse().ok()?))
        })
        .collect()
}

/// The API id that the user typed, or what is wrong with it.
pub fn parse_api_id(text: &str) -> Result<i32> {
    match text.trim().parse() {
        Ok(id) if id > 0 => Ok(id),
        _ => bail!("the API id is a number, such as 1234567"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_goes_there_and_back() {
        let newest = BTreeMap::from([("-100123".to_owned(), 77), ("42".to_owned(), 5)]);
        assert_eq!(cursor_text(&newest), "-100123\t77\n42\t5");
        assert_eq!(newest_ids(&cursor_text(&newest)), newest);
        assert!(newest_ids("").is_empty());
        assert!(newest_ids("42\tfive\nnothing").is_empty());
    }

    #[test]
    fn a_message_with_media_says_so() {
        assert_eq!(body(" Hi ", false), "Hi");
        assert_eq!(body("", true), MEDIA);
        assert_eq!(body("Look", true), format!("Look\n{MEDIA}"));
        assert_eq!(preview("One\n\n two   three"), "One two three");
        assert_eq!(preview(&"a".repeat(500)).len(), MAX_PREVIEW_CHARS);
    }

    #[test]
    fn the_app_does_not_show_its_hash() {
        let app = App {
            api_id: 1234567,
            api_hash: "0123abcd".into(),
        };
        let shown = format!("{app:?}");
        assert!(shown.contains("1234567") && !shown.contains("0123abcd"));
        assert_eq!(parse_api_id(" 1234567 ").unwrap(), 1234567);
        assert!(parse_api_id("abc").is_err() && parse_api_id("0").is_err());
    }
}
