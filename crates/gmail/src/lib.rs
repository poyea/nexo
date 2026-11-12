// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! The Gmail channel of nexo.
//!
//! - [`sign_in`]: the user signs in to Google in the browser, and nexo gets [`Credentials`].
//! - [`Gmail`]: the [`Source`](nexo_core::Source). The user looks through the mailbox with
//!   the search of Gmail and picks threads. A sync keeps the picked threads up to date, and
//!   a reply goes out in its thread.

mod api;
mod auth;
mod mail;
#[cfg(test)]
mod mock;
mod source;
mod text;

pub use api::{Credentials, Endpoints};
pub use auth::{App, sign_in};
pub use source::{CHANNEL, Gmail};
