// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The email channel of nexo, for a mailbox with IMAP and SMTP.
//!
//! - [`Account`]: the servers and the sign-in of a mailbox.
//! - [`check`]: sign in to both servers, to see that an account works.
//! - [`Email`]: the [`Source`](nexo_core::Source). The user looks through the inbox and
//!   picks threads. A sync keeps the picked threads up to date, and a reply goes out in its
//!   thread.

mod account;
#[cfg(test)]
mod mock;
mod net;
mod parse;
mod source;

pub use account::{Account, IMAP_PORT, SMTP_PORT, Server};
pub use net::check;
pub use source::{CHANNEL, Email};
