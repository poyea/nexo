// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A thread of messages in one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// The id that the channel gives. It must not change.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub participants: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// From another person to the user.
    In,
    /// From the user.
    Out,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The id that the channel gives. It is unique in its conversation.
    pub id: String,
    pub author: String,
    pub sent_at: DateTime<Utc>,
    pub direction: Direction,
    pub body: String,
}

/// One conversation as a line in a list. It has no message bodies, so a channel can give
/// many of them at low cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The id of the conversation in its channel.
    pub id: String,
    pub title: String,
    /// Who wrote the newest message.
    pub from: String,
    pub latest_at: Option<DateTime<Utc>>,
    /// The start of the newest message.
    pub preview: String,
    pub unread: bool,
    pub messages: usize,
    /// The folder has this conversation. The engine sets it.
    pub tracked: bool,
}

/// The most conversations that a source gives in one page.
pub const PAGE_ITEMS: usize = 25;

/// One page of a list of conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub items: Vec<Summary>,
    /// Give this to get the page after this one. `None` after the last page.
    pub next: Option<String>,
}
