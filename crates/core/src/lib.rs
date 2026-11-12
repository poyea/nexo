// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The core of nexo: channels give messages, a folder keeps them, and the engine syncs the two.
//!
//! - [`Source`]: one channel of messages. It gives new messages and can send replies.
//! - [`Folder`]: the output. Each conversation is a directory with a transcript. The user
//!   writes a reply in the same directory.
//! - [`Engine`]: one sync pulls new messages from all sources and sends the waiting replies.
//!   It also lists the conversations of a channel as a [`Page`] of [`Summary`] lines, and
//!   tracks the ones that the user picks: only these are in the folder.
//! - [`Card`]: a conversation on the board, in the [`Stage`] that its newest message gives.

mod board;
mod engine;
mod folder;
mod fsutil;
mod model;
mod source;

pub use board::{Card, Stage};
pub use engine::{Engine, Report};
pub use folder::{Folder, PendingReply, files};
pub use fsutil::write_atomically;
pub use model::{Conversation, Direction, Message, PAGE_ITEMS, Page, Summary};
pub use source::{Batch, Source, Update};
