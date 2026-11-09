// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use crate::model::{Direction, Message};

/// The column of a conversation on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The newest message is from another person.
    ToReply,
    /// The newest message is from the user.
    Waiting,
    /// The user marked the conversation as done, and no message came after that.
    Done,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::ToReply, Stage::Waiting, Stage::Done];

    /// `done_at` is the id of the newest message when the user marked the conversation as
    /// done. An empty id is for a conversation that had no messages then.
    pub(crate) fn of(latest: Option<&Message>, done_at: Option<&str>) -> Self {
        let newest_id = latest.map_or("", |m| m.id.as_str());
        match (done_at, latest.map(|m| m.direction)) {
            (Some(mark), _) if mark == newest_id => Stage::Done,
            (_, Some(Direction::In)) => Stage::ToReply,
            (_, Some(Direction::Out) | None) => Stage::Waiting,
        }
    }
}

/// One conversation on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub channel: String,
    /// The id of the conversation in its channel.
    pub id: String,
    pub title: String,
    /// The directory of the conversation.
    pub dir: PathBuf,
    pub stage: Stage,
    /// The newest message.
    pub latest: Option<Message>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::tests::message;

    #[test]
    fn the_newest_message_gives_the_stage() {
        let incoming = message("m1", 0, "Hi");
        let mut outgoing = message("m2", 1, "Hello");
        outgoing.direction = Direction::Out;
        assert_eq!(Stage::of(Some(&incoming), None), Stage::ToReply);
        assert_eq!(Stage::of(Some(&outgoing), None), Stage::Waiting);
        assert_eq!(Stage::of(None, None), Stage::Waiting);
    }

    #[test]
    fn done_lasts_until_a_new_message() {
        let first = message("m1", 0, "Hi");
        let second = message("m2", 1, "Still there?");
        assert_eq!(Stage::of(Some(&first), Some("m1")), Stage::Done);
        assert_eq!(Stage::of(Some(&second), Some("m1")), Stage::ToReply);
        assert_eq!(Stage::of(None, Some("")), Stage::Done);
        assert_eq!(Stage::of(Some(&first), Some("")), Stage::ToReply);
    }
}
