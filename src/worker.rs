// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use nexo_core::{Engine, Report};

/// The engine on a thread of its own, so that slow channels do not stop the UI.
pub struct Worker {
    requests: Option<Sender<()>>,
    reports: Receiver<Report>,
    thread: Option<JoinHandle<()>>,
    busy: bool,
}

impl Worker {
    /// `on_report` runs on the sync thread each time that a report is ready for
    /// [`Worker::poll`].
    pub fn spawn(mut engine: Engine, on_report: impl Fn() + Send + 'static) -> Result<Self> {
        let (requests, todo) = mpsc::channel::<()>();
        let (done, reports) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("nexo-sync".into())
            .spawn(move || {
                for () in todo {
                    if done.send(engine.sync()).is_err() {
                        return;
                    }
                    on_report();
                }
            })
            .context("starting the sync thread")?;
        Ok(Self {
            requests: Some(requests),
            reports,
            thread: Some(thread),
            busy: false,
        })
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Start a sync, unless one runs now.
    pub fn start(&mut self) -> Result<()> {
        if self.busy {
            return Ok(());
        }
        self.requests
            .as_ref()
            .and_then(|r| r.send(()).ok())
            .context("the sync thread has stopped")?;
        self.busy = true;
        Ok(())
    }

    /// The report of the sync that ended, if one ended.
    pub fn poll(&mut self) -> Result<Option<Report>> {
        match self.reports.try_recv() {
            Ok(report) => {
                self.busy = false;
                Ok(Some(report))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => bail!("the sync thread has stopped"),
        }
    }
}

impl Drop for Worker {
    /// Let a sync that runs now end, so that no file stays half written.
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
    fn a_sync_gives_one_report() {
        let dir = std::env::temp_dir().join(format!("nexo-worker-{}", std::process::id()));
        let engine = Engine::new(Folder::open(&dir).unwrap()).unwrap();
        let (ready, readied) = mpsc::channel();
        let mut worker = Worker::spawn(engine, move || ready.send(()).unwrap()).unwrap();
        worker.start().unwrap();
        worker.start().unwrap();
        assert!(worker.is_busy());
        readied.recv().unwrap();
        assert_eq!(worker.poll().unwrap(), Some(Report::default()));
        assert!(!worker.is_busy());
        assert!(worker.poll().unwrap().is_none());
        drop(worker);
        assert!(readied.try_recv().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
