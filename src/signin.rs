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

/// Work on a thread of its own, for a step that can take some seconds.
pub struct Task<T> {
    done: Receiver<T>,
}

impl<T: Send + 'static> Task<T> {
    /// Start the work. `on_done` runs on the other thread when [`Task::poll`] has the
    /// result.
    pub fn start(
        work: impl FnOnce() -> T + Send + 'static,
        on_done: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let (result, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("nexo-task".into())
            .spawn(move || {
                if result.send(work()).is_ok() {
                    on_done();
                }
            })
            .context("starting a thread")?;
        Ok(Self { done })
    }

    /// The result, if the work ended.
    pub fn poll(&self) -> Option<T> {
        self.done.try_recv().ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_gives_its_result_one_time() {
        let (wake, woken) = mpsc::channel();
        let task = Task::start(|| 6 * 7, move || wake.send(()).unwrap()).unwrap();
        woken.recv().unwrap();
        assert_eq!(task.poll(), Some(42));
        assert_eq!(task.poll(), None);
    }
}
