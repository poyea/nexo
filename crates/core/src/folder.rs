// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

use crate::fsutil::{slug, stable_hash, write_atomically};
use crate::model::{Conversation, Direction, Message};
use crate::source::{Update, is_valid_channel};

/// The names of the files in a conversation directory.
pub mod files {
    /// The conversation as JSON. Nexo finds conversations by this file.
    pub const CONVERSATION: &str = "conversation.json";
    /// All messages, one JSON object on each line, in the order that they came.
    pub const MESSAGES: &str = "messages.jsonl";
    /// The messages for people to read. Nexo writes it again after each change.
    pub const TRANSCRIPT: &str = "conversation.md";
    /// The user writes a reply here. Nexo does not read it.
    pub const DRAFT: &str = "draft.md";
    /// The user renames the draft to this name to send it.
    pub const SEND: &str = "send.md";
    /// A reply that nexo could not send. Rename it to `send.md` to try again.
    pub const UNSENT: &str = "unsent.md";
    /// Why nexo could not send `unsent.md`.
    pub const UNSENT_ERROR: &str = "unsent.error.txt";
    /// The directory of nexo in the root of the folder.
    pub const STATE_DIR: &str = ".nexo";
}

const MAX_SLUG_CHARS: usize = 40;

/// The output folder: `<root>/<channel>/<conversation>/` for each conversation.
///
/// The folder can be in a cloud drive, such as Google Drive for desktop: all writes replace
/// files in full, and the user starts a send with a rename.
pub struct Folder {
    root: PathBuf,
    /// The directory of each conversation, by channel and conversation id.
    known: BTreeMap<(String, String), PathBuf>,
}

/// A reply that the user wants to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReply {
    pub channel: String,
    pub conversation: Conversation,
    pub body: String,
    dir: PathBuf,
}

impl PendingReply {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Folder {
    /// Open the folder, make it if it is not there, and find the conversations in it.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).with_context(|| format!("making {}", root.display()))?;
        let mut folder = Self {
            root,
            known: BTreeMap::new(),
        };
        folder.scan()?;
        Ok(folder)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.root.join(files::STATE_DIR)
    }

    fn scan(&mut self) -> Result<()> {
        for channel in subdirectories(&self.root)? {
            let Some(name) = channel.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !is_valid_channel(name) {
                continue;
            }
            let name = name.to_owned();
            for dir in subdirectories(&channel)? {
                if let Some(conversation) = read_conversation(&dir)? {
                    self.known.insert((name.clone(), conversation.id), dir);
                }
            }
        }
        Ok(())
    }

    /// Keep the new messages of `update`, and give how many were new.
    pub fn merge(&mut self, channel: &str, update: &Update) -> Result<usize> {
        let dir = self.directory_for(channel, &update.conversation);
        std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
        let stored = read_conversation(&dir)?;
        let changed = stored.as_ref() != Some(&update.conversation);
        if changed {
            let json = serde_json::to_string_pretty(&update.conversation)?;
            write_atomically(&dir.join(files::CONVERSATION), json)?;
        }
        let added = add_messages(&dir, &update.conversation, &update.messages, changed)?;
        self.known
            .insert((channel.to_owned(), update.conversation.id.clone()), dir);
        Ok(added)
    }

    /// The replies that wait to be sent, in a fixed order.
    pub fn pending_replies(&self) -> Result<Vec<PendingReply>> {
        let mut replies = Vec::new();
        for ((channel, _), dir) in &self.known {
            let path = dir.join(files::SEND);
            let body = match std::fs::read_to_string(&path) {
                Ok(body) => body,
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("reading {}", path.display()));
                }
            };
            let Some(conversation) = read_conversation(dir)? else {
                continue;
            };
            replies.push(PendingReply {
                channel: channel.clone(),
                conversation,
                body,
                dir: dir.clone(),
            });
        }
        Ok(replies)
    }

    /// Keep the sent message, and remove the reply.
    pub fn record_sent(&mut self, reply: &PendingReply, message: &Message) -> Result<()> {
        add_messages(
            &reply.dir,
            &reply.conversation,
            std::slice::from_ref(message),
            false,
        )?;
        remove_if_there(&reply.dir.join(files::SEND))?;
        remove_if_there(&reply.dir.join(files::UNSENT_ERROR))
    }

    /// Move the reply to `unsent.md`, so that nexo does not try again, and tell why.
    pub fn record_failed(&self, reply: &PendingReply, error: &str) -> Result<()> {
        let send = reply.dir.join(files::SEND);
        let unsent = reply.dir.join(files::UNSENT);
        std::fs::rename(&send, &unsent)
            .with_context(|| format!("moving {} to {}", send.display(), unsent.display()))?;
        write_atomically(&reply.dir.join(files::UNSENT_ERROR), format!("{error}\n"))
    }

    fn directory_for(&self, channel: &str, conversation: &Conversation) -> PathBuf {
        if let Some(dir) = self
            .known
            .get(&(channel.to_owned(), conversation.id.clone()))
        {
            return dir.clone();
        }
        let hash = format!("{:08x}", stable_hash(&conversation.id) as u32);
        let name = match slug(&conversation.title, MAX_SLUG_CHARS) {
            s if s.is_empty() => hash,
            s => format!("{s}-{hash}"),
        };
        self.root.join(channel).join(name)
    }
}

fn subdirectories(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn read_conversation(dir: &Path) -> Result<Option<Conversation>> {
    let path = dir.join(files::CONVERSATION);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("reading {}", path.display())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn read_messages(path: &Path) -> Result<Vec<Message>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(n, line)| {
            serde_json::from_str(line)
                .with_context(|| format!("reading line {} of {}", n + 1, path.display()))
        })
        .collect()
}

/// Append the messages that are not there yet, and write the transcript again if there is a
/// change. Give how many messages were new.
fn add_messages(
    dir: &Path,
    conversation: &Conversation,
    messages: &[Message],
    force_transcript: bool,
) -> Result<usize> {
    let path = dir.join(files::MESSAGES);
    let mut all = read_messages(&path)?;
    let mut ids: HashSet<String> = all.iter().map(|m| m.id.clone()).collect();
    let new: Vec<&Message> = messages
        .iter()
        .filter(|m| ids.insert(m.id.clone()))
        .collect();
    if !new.is_empty() {
        let mut lines = String::new();
        for message in &new {
            lines.push_str(&serde_json::to_string(message)?);
            lines.push('\n');
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(lines.as_bytes()))
            .with_context(|| format!("writing {}", path.display()))?;
        all.extend(new.iter().map(|m| (*m).clone()));
    }
    if !new.is_empty() || force_transcript {
        write_atomically(&dir.join(files::TRANSCRIPT), transcript(conversation, &all))?;
    }
    Ok(new.len())
}

fn transcript(conversation: &Conversation, messages: &[Message]) -> String {
    let mut sorted: Vec<&Message> = messages.iter().collect();
    sorted.sort_by(|a, b| a.sent_at.cmp(&b.sent_at).then_with(|| a.id.cmp(&b.id)));
    let mut out = format!("# {}\n\n", conversation.title);
    if !conversation.participants.is_empty() {
        let _ = writeln!(out, "{}\n", conversation.participants.join(", "));
    }
    for message in sorted {
        let arrow = match message.direction {
            Direction::In => "",
            Direction::Out => " (you)",
        };
        let time = message
            .sent_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M");
        let _ = writeln!(out, "---\n\n**{}**{arrow} - {time}\n", message.author);
        let _ = writeln!(out, "{}\n", message.body.trim_end());
    }
    let _ = writeln!(
        out,
        "---\n\n_To reply, write the text in `{}` in this folder, then rename the file to `{}`._",
        files::DRAFT,
        files::SEND
    );
    out
}

fn remove_if_there(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nexo-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn conversation(id: &str, title: &str) -> Conversation {
        Conversation {
            id: id.into(),
            title: title.into(),
            participants: vec![title.into()],
        }
    }

    pub fn message(id: &str, minute: u32, body: &str) -> Message {
        Message {
            id: id.into(),
            author: "Jane".into(),
            sent_at: Utc.with_ymd_and_hms(2026, 11, 9, 12, minute, 0).unwrap(),
            direction: Direction::In,
            body: body.into(),
        }
    }

    fn update(conversation: Conversation, messages: Vec<Message>) -> Update {
        Update {
            conversation,
            messages,
        }
    }

    #[test]
    fn merge_keeps_each_message_once() {
        let tmp = TempDir::new("merge");
        let mut folder = Folder::open(&tmp.0).unwrap();
        let jane = conversation("c1", "Jane Doe");
        let first = update(
            jane.clone(),
            vec![message("m1", 0, "Hi"), message("m1", 0, "Hi")],
        );
        assert_eq!(folder.merge("chat", &first).unwrap(), 1);
        let second = update(
            jane,
            vec![message("m1", 0, "Hi"), message("m2", 1, "Still there?")],
        );
        assert_eq!(folder.merge("chat", &second).unwrap(), 1);

        let dirs = subdirectories(&tmp.0.join("chat")).unwrap();
        assert_eq!(dirs.len(), 1);
        let name = dirs[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("jane-doe-"), "{name}");
        let stored = read_messages(&dirs[0].join(files::MESSAGES)).unwrap();
        assert_eq!(stored.len(), 2);
        let text = std::fs::read_to_string(dirs[0].join(files::TRANSCRIPT)).unwrap();
        assert!(text.starts_with("# Jane Doe\n"));
        assert!(text.find("Hi").unwrap() < text.find("Still there?").unwrap());
    }

    #[test]
    fn a_new_title_keeps_the_directory() {
        let tmp = TempDir::new("title");
        let mut folder = Folder::open(&tmp.0).unwrap();
        folder
            .merge("chat", &update(conversation("c1", "Jane"), vec![]))
            .unwrap();
        folder
            .merge("chat", &update(conversation("c1", "Jane Doe"), vec![]))
            .unwrap();
        let dirs = subdirectories(&tmp.0.join("chat")).unwrap();
        assert_eq!(dirs.len(), 1);
        let text = std::fs::read_to_string(dirs[0].join(files::TRANSCRIPT)).unwrap();
        assert!(text.starts_with("# Jane Doe\n"));
    }

    #[test]
    fn open_finds_the_conversations_again() {
        let tmp = TempDir::new("reopen");
        Folder::open(&tmp.0)
            .unwrap()
            .merge("chat", &update(conversation("c1", "Jane"), vec![]))
            .unwrap();
        let dir = subdirectories(&tmp.0.join("chat")).unwrap().remove(0);
        let renamed = tmp.0.join("chat").join("my-jane");
        std::fs::rename(&dir, &renamed).unwrap();

        let mut folder = Folder::open(&tmp.0).unwrap();
        folder
            .merge(
                "chat",
                &update(conversation("c1", "Jane"), vec![message("m1", 0, "Hi")]),
            )
            .unwrap();
        assert_eq!(subdirectories(&tmp.0.join("chat")).unwrap(), vec![renamed]);
    }

    #[test]
    fn only_send_md_is_a_pending_reply() {
        let tmp = TempDir::new("pending");
        let mut folder = Folder::open(&tmp.0).unwrap();
        folder
            .merge("chat", &update(conversation("c1", "Jane"), vec![]))
            .unwrap();
        let dir = subdirectories(&tmp.0.join("chat")).unwrap().remove(0);
        std::fs::write(dir.join(files::DRAFT), "Not yet").unwrap();
        assert!(folder.pending_replies().unwrap().is_empty());

        std::fs::rename(dir.join(files::DRAFT), dir.join(files::SEND)).unwrap();
        let replies = folder.pending_replies().unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body, "Not yet");
        assert_eq!(replies[0].channel, "chat");
    }

    #[test]
    fn sent_and_failed_replies_leave_send_md() {
        let tmp = TempDir::new("outcome");
        let mut folder = Folder::open(&tmp.0).unwrap();
        folder
            .merge("chat", &update(conversation("c1", "Jane"), vec![]))
            .unwrap();
        let dir = subdirectories(&tmp.0.join("chat")).unwrap().remove(0);

        std::fs::write(dir.join(files::SEND), "One").unwrap();
        let reply = folder.pending_replies().unwrap().remove(0);
        folder.record_failed(&reply, "offline").unwrap();
        assert!(!dir.join(files::SEND).exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(files::UNSENT)).unwrap(),
            "One"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(files::UNSENT_ERROR)).unwrap(),
            "offline\n"
        );

        std::fs::rename(dir.join(files::UNSENT), dir.join(files::SEND)).unwrap();
        let reply = folder.pending_replies().unwrap().remove(0);
        let mut sent = message("m9", 5, "One");
        sent.direction = Direction::Out;
        folder.record_sent(&reply, &sent).unwrap();
        assert!(!dir.join(files::SEND).exists());
        assert!(!dir.join(files::UNSENT_ERROR).exists());
        let text = std::fs::read_to_string(dir.join(files::TRANSCRIPT)).unwrap();
        assert!(text.contains("**Jane** (you)"));
    }

    #[test]
    fn a_broken_line_is_an_error() {
        let tmp = TempDir::new("broken");
        std::fs::create_dir_all(&tmp.0).unwrap();
        let path = tmp.0.join(files::MESSAGES);
        std::fs::write(&path, "{\"id\":\n").unwrap();
        let err = read_messages(&path).unwrap_err();
        assert!(format!("{err:#}").contains("line 1"));
    }

    #[test]
    fn other_directories_are_not_channels() {
        let tmp = TempDir::new("others");
        std::fs::create_dir_all(tmp.0.join(".nexo").join("x")).unwrap();
        std::fs::create_dir_all(tmp.0.join("Notes").join("y")).unwrap();
        assert!(Folder::open(&tmp.0).unwrap().known.is_empty());
    }
}
