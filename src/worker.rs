// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use nexo_core::{Card, Engine, Page, Report, Source};

enum Request {
    Sync,
    SetDone {
        channel: String,
        id: String,
        done: bool,
    },
    Browse {
        channel: String,
        query: String,
        page: Option<String>,
        tag: u64,
    },
    Open {
        channel: String,
        id: String,
    },
    Track {
        channel: String,
        id: String,
    },
    Connect(Box<dyn Source>),
    Disconnect(String),
}

/// What a request gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Synced(Report),
    /// A change with no result of its own.
    Changed,
    /// Why a change did not work.
    Failed(String),
    /// A page of the conversations of a channel, with the tag of its request.
    Listed {
        tag: u64,
        page: Result<Page, String>,
    },
    /// A conversation with all its messages, to read.
    Opened {
        id: String,
        conversation: Result<nexo_core::Update, String>,
    },
    /// The conversation is in the folder now, or the reason that it is not.
    Tracked {
        id: String,
        result: Result<(), String>,
    },
}

/// What the engine has for the UI after a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub outcome: Outcome,
    /// All conversations, as they are after the request.
    pub board: Vec<Card>,
    /// The channels that have a list of conversations to pick from.
    pub browsable: Vec<String>,
}

/// The engine on a thread of its own, so that slow channels do not stop the UI.
pub struct Worker {
    requests: Option<Sender<Request>>,
    updates: Receiver<Update>,
    thread: Option<JoinHandle<()>>,
    syncing: bool,
}

fn handle(engine: &mut Engine, request: Request) -> Outcome {
    let words = |err: anyhow::Error| format!("{err:#}");
    let changed = |result: Result<()>| match result {
        Ok(()) => Outcome::Changed,
        Err(err) => Outcome::Failed(words(err)),
    };
    match request {
        Request::Sync => Outcome::Synced(engine.sync()),
        Request::SetDone { channel, id, done } => changed(engine.set_done(&channel, &id, done)),
        Request::Browse {
            channel,
            query,
            page,
            tag,
        } => Outcome::Listed {
            tag,
            page: engine
                .browse(&channel, &query, page.as_deref())
                .map_err(words),
        },
        Request::Open { channel, id } => Outcome::Opened {
            conversation: engine.load(&channel, &id).map_err(words),
            id,
        },
        Request::Track { channel, id } => Outcome::Tracked {
            result: engine.track(&channel, &id).map_err(words),
            id,
        },
        Request::Connect(source) => {
            let channel = source.channel().to_owned();
            engine.remove(&channel);
            changed(engine.add(source))
        }
        Request::Disconnect(channel) => {
            engine.remove(&channel);
            Outcome::Changed
        }
    }
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
                    let update = Update {
                        outcome: handle(&mut engine, request),
                        board: engine.board(),
                        browsable: engine.browsable().map(str::to_owned).collect(),
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

    /// Ask for a page of the conversations of a channel. The answer has the same `tag`.
    pub fn browse(
        &mut self,
        channel: String,
        query: String,
        page: Option<String>,
        tag: u64,
    ) -> Result<()> {
        self.send(Request::Browse {
            channel,
            query,
            page,
            tag,
        })
    }

    /// Ask for a conversation with all its messages, to read.
    pub fn open(&mut self, channel: String, id: String) -> Result<()> {
        self.send(Request::Open { channel, id })
    }

    /// Put a conversation in the folder.
    pub fn track(&mut self, channel: String, id: String) -> Result<()> {
        self.send(Request::Track { channel, id })
    }

    /// Add a channel, in place of the one with the same name if there is one.
    pub fn connect(&mut self, source: Box<dyn Source>) -> Result<()> {
        self.send(Request::Connect(source))
    }

    /// Remove a channel. Its conversations stay in the folder.
    pub fn disconnect(&mut self, channel: String) -> Result<()> {
        self.send(Request::Disconnect(channel))
    }

    /// The next update that is ready, if there is one.
    pub fn poll(&mut self) -> Result<Option<Update>> {
        match self.updates.try_recv() {
            Ok(update) => {
                if matches!(update.outcome, Outcome::Synced(_)) {
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

    use chrono::{TimeZone, Utc};
    use nexo_core::{Batch, Conversation, Direction, Folder, Message, Summary};

    use super::*;

    /// A channel with one conversation to pick.
    struct Shop;

    impl Source for Shop {
        fn channel(&self) -> &str {
            "shop"
        }

        fn fetch(&mut self, _: Option<&str>, _: &[String]) -> Result<Batch> {
            Ok(Batch::default())
        }

        fn can_browse(&self) -> bool {
            true
        }

        fn browse(&mut self, _: &str, _: Option<&str>) -> Result<Page> {
            let item = Summary {
                id: "o1".into(),
                title: "Order".into(),
                from: "Shop".into(),
                latest_at: None,
                preview: "Thanks".into(),
                unread: true,
                messages: 1,
                tracked: false,
            };
            Ok(Page {
                items: vec![item],
                next: None,
            })
        }

        fn load(&mut self, id: &str) -> Result<nexo_core::Update> {
            if id != "o1" {
                bail!("no conversation {id}");
            }
            Ok(nexo_core::Update {
                conversation: Conversation {
                    id: id.into(),
                    title: "Order".into(),
                    participants: vec!["Shop".into()],
                },
                messages: vec![Message {
                    id: "m1".into(),
                    author: "Shop".into(),
                    sent_at: Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap(),
                    direction: Direction::In,
                    body: "Thanks".into(),
                }],
            })
        }
    }

    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        worker: Worker,
        ready: mpsc::Receiver<()>,
        _dir: TempDir,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("nexo-worker-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let engine = Engine::new(Folder::open(&dir).unwrap()).unwrap();
            let (wake, ready) = mpsc::channel();
            let worker = Worker::spawn(engine, move || wake.send(()).unwrap()).unwrap();
            Self {
                worker,
                ready,
                _dir: TempDir(dir),
            }
        }

        fn next(&mut self) -> Update {
            self.ready.recv().unwrap();
            self.worker.poll().unwrap().unwrap()
        }
    }

    #[test]
    fn each_request_gives_one_update() {
        let mut f = Fixture::new("requests");
        f.worker.sync().unwrap();
        f.worker.sync().unwrap();
        assert!(f.worker.is_syncing());
        let update = f.next();
        assert_eq!(update.outcome, Outcome::Synced(Report::default()));
        assert!(update.board.is_empty());
        assert!(!f.worker.is_syncing());
        assert!(f.worker.poll().unwrap().is_none());

        f.worker
            .set_done("chat".into(), "no-such".into(), true)
            .unwrap();
        let Outcome::Failed(problem) = f.next().outcome else {
            panic!("the mark of a conversation that is not there must fail");
        };
        assert!(problem.contains("no-such"));
        assert!(!f.worker.is_syncing());

        let Fixture { worker, ready, .. } = f;
        drop(worker);
        assert!(ready.try_recv().is_err());
    }

    #[test]
    fn the_user_browses_reads_and_picks() {
        let mut f = Fixture::new("browse");
        f.worker.connect(Box::new(Shop)).unwrap();
        let update = f.next();
        assert_eq!(update.outcome, Outcome::Changed);
        assert_eq!(update.browsable, ["shop"]);

        f.worker
            .browse("shop".into(), String::new(), None, 7)
            .unwrap();
        let Outcome::Listed { tag: 7, page } = f.next().outcome else {
            panic!("a browse gives a page with its tag");
        };
        assert_eq!(page.unwrap().items[0].title, "Order");

        f.worker.open("shop".into(), "o1".into()).unwrap();
        let update = f.next();
        let Outcome::Opened { conversation, .. } = update.outcome else {
            panic!("an open gives the conversation");
        };
        assert_eq!(conversation.unwrap().messages[0].body, "Thanks");
        assert!(update.board.is_empty());

        f.worker.track("shop".into(), "o1".into()).unwrap();
        let update = f.next();
        assert_eq!(
            update.outcome,
            Outcome::Tracked {
                id: "o1".into(),
                result: Ok(())
            }
        );
        assert_eq!(update.board.len(), 1);

        f.worker.track("shop".into(), "o2".into()).unwrap();
        let Outcome::Tracked { result, .. } = f.next().outcome else {
            panic!("a track gives its result");
        };
        assert!(result.unwrap_err().contains("o2"));

        f.worker.disconnect("shop".into()).unwrap();
        let update = f.next();
        assert!(update.browsable.is_empty());
        assert_eq!(update.board.len(), 1);
    }
}
