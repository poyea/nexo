// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use anyhow::{Context, Result};
use nexo_gmail::{App, Credentials, Endpoints};

/// How a sign-in ended.
#[derive(Debug)]
pub enum SignedIn {
    Yes(Credentials),
    /// The user stopped it.
    Stopped,
    Failed(String),
}

/// A sign-in to Gmail on a thread of its own: it waits for the user in the browser.
pub struct SignIn {
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<Credentials, String>>,
}

impl SignIn {
    /// Open the sign-in page in the browser. `on_done` runs on the other thread when
    /// [`SignIn::poll`] has the result.
    pub fn start(app: App, on_done: impl Fn() + Send + 'static) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let (result, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("nexo-sign-in".into())
            .spawn(move || {
                let outcome = nexo_gmail::sign_in(&app, &Endpoints::default(), open_url, &stop)
                    .map_err(|err| format!("{err:#}"));
                if result.send(outcome).is_ok() {
                    on_done();
                }
            })
            .context("starting the sign-in thread")?;
        Ok(Self { cancel, done })
    }

    /// Stop the wait for the browser. The result comes a moment later.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The result, if the sign-in ended.
    pub fn poll(&self) -> Option<SignedIn> {
        let stopped = self.cancel.load(Ordering::Relaxed);
        match self.done.try_recv() {
            Ok(Ok(credentials)) => Some(SignedIn::Yes(credentials)),
            Ok(Err(_)) if stopped => Some(SignedIn::Stopped),
            Ok(Err(why)) => Some(SignedIn::Failed(why)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(SignedIn::Failed("the sign-in thread has stopped".into()))
            }
        }
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Show a web page in the browser of the user.
pub fn open_url(url: &str) -> Result<()> {
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(drop)
        .with_context(|| format!("opening {url}"))
}
