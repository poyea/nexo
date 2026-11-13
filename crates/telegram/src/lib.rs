// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The Telegram channel of nexo, for the account of a person.
//!
//! - [`App`]: the API id and hash that Telegram gives a developer at <https://my.telegram.org>.
//! - [`Login`]: the sign-in in steps: a phone number, the code that Telegram sends, and the
//!   password if the account has two-step verification.
//! - [`Telegram`]: the [`Source`](nexo_core::Source). The user looks through the chats and
//!   picks some. A sync keeps the picked chats up to date, and a reply goes to its chat.
//!
//! The session file keeps the sign-in. It is a secret: keep it out of the folder.

mod source;

pub use source::{App, CHANNEL, Login, Progress, Telegram, parse_api_id};
