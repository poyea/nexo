// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use nexo_core::{Card, Engine, Report};

enum Request {
    Sync,
    SetDone {
        channel: String,
        id: String,
        done: bool,
    },
}

/// What the engine has for the UI after a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The report, if the request was a sync.
    pub report: Option<Report>,
    /// All conversations, as they are after the request.
    pub board: Vec<Card>,
    /// Why a request that was not a sync did not work.
    pub problem: Option<String>,
}

/// The engine on a thread of its own, so that slow channels do not stop the UI.
pub struct Worker {
    requests: Option<Sender<Request>>,
    updates: Receiver<Update>,
    thread: Option<JoinHandle<()>>,
    syncing: bool,
}

impl Worker {
    /// `on_update` runs on the engine thread each time that an update is ready for
    /// [`Worker::poll`].
    pub fn spawn(mut engine: Engine, on_update: impl Fn() + Send + 'static) -> Result<Self> {
        let (requests, todo) = mpsc::channel();
        let (done, updates) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("nexo-engine".into())
            .spawn(move || {
                for request in todo {
                    let (report, problem) = match request {
                        Request::Sync => (Some(engine.sync()), None),
                        Request::SetDone { channel, id, done } => {
                            let result = engine.set_done(&channel, &id, done);
                            (None, result.err().map(|err| format!("{err:#}")))
                        }
                    };
                    let update = Update {
                        report,
                        board: engine.board(),
                        problem,
                    };
                    if done.send(update).is_err() {
                        return;
                    }
                    on_update();
                }
            })
            .context("starting the engine thread")?;
        Ok(Self {
            requests: Some(requests),
            updates,
            thread: Some(thread),
            syncing: false,
        })
    }

    /// A sync runs now, or waits to run.
    pub fn is_syncing(&self) -> bool {
        self.syncing
    }

    /// Start a sync, unless one runs now.
    pub fn sync(&mut self) -> Result<()> {
        if self.syncing {
            return Ok(());
        }
        self.send(Request::Sync)?;
        self.syncing = true;
        Ok(())
    }

    /// Mark a conversation as done, or remove the mark.
    pub fn set_done(&mut self, channel: String, id: String, done: bool) -> Result<()> {
        self.send(Request::SetDone { channel, id, done })
    }

    /// The next update that is ready, if there is one.
    pub fn poll(&mut self) -> Result<Option<Update>> {
        match self.updates.try_recv() {
            Ok(update) => {
                if update.report.is_some() {
                    self.syncing = false;
                }
                Ok(Some(update))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => bail!("the engine thread has stopped"),
        }
    }

    fn send(&self, request: Request) -> Result<()> {
        self.requests
            .as_ref()
            .and_then(|r| r.send(request).ok())
            .context("the engine thread has stopped")
    }
}

impl Drop for Worker {
    /// Let a request that runs now end, so that no file stays half written.
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use nexo_core::Folder;

    #[test]
    fn each_request_gives_one_update() {
        let dir = std::env::temp_dir().join(format!("nexo-worker-{}", std::process::id()));
        let engine = Engine::new(Folder::open(&dir).unwrap()).unwrap();
        let (ready, readied) = mpsc::channel();
        let mut worker = Worker::spawn(engine, move || ready.send(()).unwrap()).unwrap();
        worker.sync().unwrap();
        worker.sync().unwrap();
        assert!(worker.is_syncing());
        readied.recv().unwrap();
        let update = worker.poll().unwrap().unwrap();
        assert_eq!(update.report, Some(Report::default()));
        assert!(update.board.is_empty());
        assert!(!worker.is_syncing());
        assert!(worker.poll().unwrap().is_none());

        worker
            .set_done("chat".into(), "no-such".into(), true)
            .unwrap();
        readied.recv().unwrap();
        let update = worker.poll().unwrap().unwrap();
        assert_eq!(update.report, None);
        assert!(update.problem.unwrap().contains("no-such"));
        assert!(!worker.is_syncing());

        drop(worker);
        assert!(readied.try_recv().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
