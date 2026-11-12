// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use anyhow::{Result, bail};

use crate::model::{Conversation, Message, Page};

/// New messages of one conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub conversation: Conversation,
    pub messages: Vec<Message>,
}

/// What one fetch gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub updates: Vec<Update>,
    /// Give this to the next fetch. `None` keeps the cursor that the engine has.
    pub cursor: Option<String>,
}

/// One channel of messages.
///
/// The engine calls a source from a worker thread, so a source must be `Send`. A fetch can
/// give a message again: the folder keeps each message id only once.
pub trait Source: Send {
    /// A short name, such as `chat`. It is the name of the channel directory, so it can
    /// have only lowercase ASCII letters, digits, and `-`.
    fn channel(&self) -> &str;

    /// Give the messages after `cursor`, or all messages when `cursor` is `None`.
    ///
    /// `tracked` has the ids of the conversations that the folder has for this channel. A
    /// source that the user picks conversations from gives updates only for these. Other
    /// sources ignore it and give all conversations.
    fn fetch(&mut self, cursor: Option<&str>, tracked: &[String]) -> Result<Batch>;

    /// The user can look through the conversations of the channel, and pick the ones that
    /// the folder keeps. Such a source has [`Source::browse`] and [`Source::load`].
    fn can_browse(&self) -> bool {
        false
    }

    /// Give one page of the conversations that match `query`, the newest first. An empty
    /// query is for the default list of the channel. `page` is [`Page::next`] of the page
    /// before, or `None` for the first page. The words of a query are those of the channel.
    fn browse(&mut self, query: &str, page: Option<&str>) -> Result<Page> {
        let _ = (query, page);
        bail!("{} has no list of conversations", self.channel())
    }

    /// Give the conversation `id` with all its messages.
    fn load(&mut self, id: &str) -> Result<Update> {
        let _ = id;
        bail!("{} cannot load one conversation", self.channel())
    }

    fn can_reply(&self) -> bool {
        false
    }

    /// Send `body` to the conversation, and give the message as the channel keeps it.
    fn reply(&mut self, conversation: &Conversation, body: &str) -> Result<Message> {
        let _ = (conversation, body);
        bail!("{} cannot send replies", self.channel())
    }
}

pub(crate) fn is_valid_channel(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_safe_directory_names() {
        assert!(is_valid_channel("chat"));
        assert!(is_valid_channel("e-mail2"));
        assert!(!is_valid_channel(""));
        assert!(!is_valid_channel("-x"));
        assert!(!is_valid_channel("Chat"));
        assert!(!is_valid_channel("a/b"));
        assert!(!is_valid_channel(".nexo"));
    }
}
