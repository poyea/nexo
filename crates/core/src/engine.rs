// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::folder::{Folder, PendingReply};
use crate::fsutil::write_atomically;
use crate::source::{Source, is_valid_channel};

const STATE_FILE: &str = "state.json";

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// New messages from all channels.
    pub received: usize,
    pub sent: usize,
    /// One line for each thing that went wrong. The sync does the rest all the same.
    pub problems: Vec<String>,
}

/// The cursor of each channel. It is in the folder, so it moves with the folder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    cursors: BTreeMap<String, String>,
}

/// The channels and the folder, and the sync between them.
pub struct Engine {
    folder: Folder,
    sources: Vec<Box<dyn Source>>,
    state: State,
    state_path: PathBuf,
}

impl Engine {
    pub fn new(folder: Folder) -> Result<Self> {
        let state_path = folder.state_dir().join(STATE_FILE);
        let state = match std::fs::read_to_string(&state_path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("reading {}", state_path.display()))?,
            Err(err) if err.kind() == ErrorKind::NotFound => State::default(),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", state_path.display()));
            }
        };
        Ok(Self {
            folder,
            sources: Vec::new(),
            state,
            state_path,
        })
    }

    pub fn folder(&self) -> &Folder {
        &self.folder
    }

    pub fn add(&mut self, source: Box<dyn Source>) -> Result<()> {
        let channel = source.channel();
        if !is_valid_channel(channel) {
            bail!("{channel:?} is not a valid channel name");
        }
        if self.sources.iter().any(|s| s.channel() == channel) {
            bail!("there are two sources for the channel {channel:?}");
        }
        self.sources.push(source);
        Ok(())
    }

    pub fn channels(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|s| s.channel())
    }

    /// Pull new messages from all channels, then send the replies that wait.
    pub fn sync(&mut self) -> Report {
        let mut report = Report::default();
        for i in 0..self.sources.len() {
            let channel = self.sources[i].channel().to_owned();
            if let Err(err) = self.pull(i, &mut report) {
                report.problems.push(format!("{channel}: {err:#}"));
            }
        }
        match self.folder.pending_replies() {
            Ok(replies) => {
                for reply in replies {
                    self.send(&reply, &mut report);
                }
            }
            Err(err) => report.problems.push(format!("finding replies: {err:#}")),
        }
        report
    }

    fn pull(&mut self, index: usize, report: &mut Report) -> Result<()> {
        let source = &mut self.sources[index];
        let channel = source.channel().to_owned();
        let cursor = self.state.cursors.get(&channel).map(String::as_str);
        let batch = source.fetch(cursor)?;
        for update in &batch.updates {
            report.received += self.folder.merge(&channel, update)?;
        }
        if let Some(cursor) = batch.cursor {
            self.state.cursors.insert(channel, cursor);
            self.save_state()?;
        }
        Ok(())
    }

    fn send(&mut self, reply: &PendingReply, report: &mut Report) {
        let title = &reply.conversation.title;
        let outcome = match self
            .sources
            .iter_mut()
            .find(|s| s.channel() == reply.channel)
        {
            None => Err(format!("nexo has no source for {}", reply.channel)),
            Some(source) if !source.can_reply() => {
                Err(format!("{} cannot send replies", reply.channel))
            }
            Some(_) if reply.body.trim().is_empty() => Err("the reply is empty".to_owned()),
            Some(source) => source
                .reply(&reply.conversation, reply.body.trim())
                .map_err(|err| format!("{err:#}")),
        };
        let recorded = match &outcome {
            Ok(message) => self.folder.record_sent(reply, message),
            Err(why) => self.folder.record_failed(reply, why),
        };
        match (outcome, recorded) {
            (Ok(_), Ok(())) => report.sent += 1,
            (Ok(_), Err(err)) => report
                .problems
                .push(format!("{}/{title}: sent, but {err:#}", reply.channel)),
            (Err(why), Ok(())) => report
                .problems
                .push(format!("{}/{title}: not sent: {why}", reply.channel)),
            (Err(why), Err(err)) => report.problems.push(format!(
                "{}/{title}: not sent: {why}; {err:#}",
                reply.channel
            )),
        }
    }

    fn save_state(&self) -> Result<()> {
        let dir = self.folder.state_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
        write_atomically(&self.state_path, serde_json::to_string_pretty(&self.state)?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::anyhow;

    use super::*;
    use crate::folder::files;
    use crate::folder::tests::{TempDir, conversation, message};
    use crate::model::{Conversation, Direction, Message};
    use crate::source::{Batch, Update};

    /// A channel in memory. The test keeps a handle to see what it sent.
    #[derive(Clone, Default)]
    struct Fake {
        name: &'static str,
        inbox: Arc<Mutex<Vec<(Conversation, Message)>>>,
        sent: Arc<Mutex<Vec<String>>>,
        cursors: Arc<Mutex<Vec<Option<String>>>>,
        replies: bool,
        broken: bool,
    }

    impl Source for Fake {
        fn channel(&self) -> &str {
            self.name
        }

        fn fetch(&mut self, cursor: Option<&str>) -> Result<Batch> {
            self.cursors.lock().unwrap().push(cursor.map(str::to_owned));
            if self.broken {
                return Err(anyhow!("offline"));
            }
            let inbox = std::mem::take(&mut *self.inbox.lock().unwrap());
            let count = inbox.len();
            Ok(Batch {
                updates: inbox
                    .into_iter()
                    .map(|(conversation, m)| Update {
                        conversation,
                        messages: vec![m],
                    })
                    .collect(),
                cursor: (count > 0).then(|| format!("after-{count}")),
            })
        }

        fn can_reply(&self) -> bool {
            self.replies
        }

        fn reply(&mut self, _: &Conversation, body: &str) -> Result<Message> {
            self.sent.lock().unwrap().push(body.to_owned());
            let mut m = message("sent-1", 30, body);
            m.direction = Direction::Out;
            Ok(m)
        }
    }

    fn only_dir(root: &std::path::Path, channel: &str) -> PathBuf {
        let mut dirs: Vec<_> = std::fs::read_dir(root.join(channel))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(dirs.len(), 1);
        dirs.remove(0)
    }

    #[test]
    fn sync_pulls_and_keeps_the_cursor() {
        let tmp = TempDir::new("engine-pull");
        let fake = Fake {
            name: "chat",
            ..Fake::default()
        };
        fake.inbox
            .lock()
            .unwrap()
            .push((conversation("c1", "Jane"), message("m1", 0, "Hi")));
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(fake.clone())).unwrap();
        assert_eq!(engine.sync().received, 1);
        assert_eq!(engine.sync().received, 0);

        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(fake.clone())).unwrap();
        engine.sync();
        let cursors = fake.cursors.lock().unwrap().clone();
        assert_eq!(
            cursors,
            [None, Some("after-1".into()), Some("after-1".into())]
        );
    }

    #[test]
    fn sync_sends_replies_where_the_channel_can() {
        let tmp = TempDir::new("engine-send");
        let fake = Fake {
            name: "mail",
            replies: true,
            ..Fake::default()
        };
        fake.inbox
            .lock()
            .unwrap()
            .push((conversation("c1", "Jane"), message("m1", 0, "Hi")));
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(fake.clone())).unwrap();
        engine.sync();

        let dir = only_dir(&tmp.0, "mail");
        std::fs::write(dir.join(files::SEND), "Hello back\n").unwrap();
        let report = engine.sync();
        assert_eq!(report.sent, 1, "{:?}", report.problems);
        assert_eq!(*fake.sent.lock().unwrap(), ["Hello back"]);
        assert!(!dir.join(files::SEND).exists());
    }

    #[test]
    fn a_channel_without_replies_keeps_the_text_as_unsent() {
        let tmp = TempDir::new("engine-noreply");
        let fake = Fake {
            name: "chat",
            ..Fake::default()
        };
        fake.inbox
            .lock()
            .unwrap()
            .push((conversation("c1", "Jane"), message("m1", 0, "Hi")));
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(fake)).unwrap();
        engine.sync();

        let dir = only_dir(&tmp.0, "chat");
        std::fs::write(dir.join(files::SEND), "Hello back").unwrap();
        let report = engine.sync();
        assert_eq!(report.sent, 0);
        assert_eq!(report.problems.len(), 1);
        assert!(dir.join(files::UNSENT).exists());
        assert!(engine.sync().problems.is_empty());
    }

    #[test]
    fn one_broken_channel_does_not_stop_the_others() {
        let tmp = TempDir::new("engine-broken");
        let broken = Fake {
            name: "a",
            broken: true,
            ..Fake::default()
        };
        let good = Fake {
            name: "b",
            ..Fake::default()
        };
        good.inbox
            .lock()
            .unwrap()
            .push((conversation("c1", "Jane"), message("m1", 0, "Hi")));
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        engine.add(Box::new(broken)).unwrap();
        engine.add(Box::new(good)).unwrap();
        let report = engine.sync();
        assert_eq!(report.received, 1);
        assert_eq!(report.problems, ["a: offline"]);
    }

    #[test]
    fn add_refuses_bad_and_double_channels() {
        let tmp = TempDir::new("engine-add");
        let mut engine = Engine::new(Folder::open(&tmp.0).unwrap()).unwrap();
        let named = |name| {
            Box::new(Fake {
                name,
                ..Fake::default()
            })
        };
        assert!(engine.add(named("Bad Name")).is_err());
        engine.add(named("chat")).unwrap();
        assert!(engine.add(named("chat")).is_err());
        assert_eq!(engine.channels().collect::<Vec<_>>(), ["chat"]);
    }
}
