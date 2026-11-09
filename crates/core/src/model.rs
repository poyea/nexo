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
