// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use glasskit::Ui;
use glasskit::kanban::{Board as Lanes, BoardEvent, CardData, CardKey};
use glasskit::widget::{AnyId, Button, Column, Id, Length, Row, Spacer, Tint, kind};
use glasskit::window::{Window, WindowEvent};
use nexo_core::{Card as Conversation, Stage};
use serde::{Deserialize, Serialize};

const LANES: [&str; 3] = ["To do", "Waiting", "Done"];
const MAX_TITLE_CHARS: usize = 60;

/// What the user asked for on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardAction {
    /// Open the directory of a conversation.
    Open(PathBuf),
    SetDone {
        channel: String,
        id: String,
        done: bool,
    },
}

/// The board as the file keeps it: the colors of the lanes, and the tasks of the user.
/// The conversations are not in the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    lanes: Vec<SavedLane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct SavedLane {
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    cards: Vec<SavedCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedCard {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    color: Option<String>,
}

/// The board: lanes with the tasks of the user and the conversations as cards.
pub struct Board {
    lanes: Lanes,
    path: PathBuf,
    archived: Id<kind::Button>,
    /// The card of each conversation.
    conversations: Vec<(CardKey, Conversation)>,
}

impl Board {
    /// Make the widgets with the tasks of the file, and give the column that holds them.
    pub fn build(w: &mut Window, path: &Path) -> Result<(Self, Id<kind::Column>)> {
        let mut lanes = Lanes::new(w, &LANES);
        for (lane, saved) in load(path)?.lanes.into_iter().enumerate() {
            lanes.set_lane_tint(w, lane, tint(saved.color.as_deref()));
            for card in saved.cards {
                let card = CardData {
                    title: card.title,
                    body: card.body,
                    pinned: card.pinned,
                    archived: card.archived,
                    tint: tint(card.color.as_deref()),
                    locked: false,
                };
                lanes.add_card(w, lane, card);
            }
        }
        let gap = w.add(Spacer::fill());
        let archived = w.add(Button::new("Show archived"));
        let bar = w.add(Row::new().child(gap).child(archived));
        let column = w.add(
            Column::new()
                .gap(8.0)
                .height(Length::Fill(1.0))
                .child(bar)
                .child(lanes.widget()),
        );
        let board = Self {
            lanes,
            path: path.to_owned(),
            archived,
            conversations: Vec::new(),
        };
        Ok((board, column))
    }

    /// Show these conversations as cards, each in the lane of its stage.
    pub fn set(&mut self, w: &mut Window, cards: &[Conversation]) {
        let same = |a: &Conversation, b: &Conversation| a.channel == b.channel && a.id == b.id;
        let lanes = &mut self.lanes;
        self.conversations.retain(|(key, old)| {
            let stays = cards.iter().any(|card| same(card, old));
            if !stays {
                lanes.remove_card(w, *key);
            }
            stays
        });
        for card in cards {
            let data = CardData {
                locked: true,
                ..CardData::new(title(&card.title), note(card))
            };
            let lane = lane_of(card.stage);
            match self
                .conversations
                .iter_mut()
                .find(|(_, old)| same(card, old))
            {
                Some((key, old)) => {
                    lanes.set_card(w, *key, data);
                    if lanes.place(*key).is_some_and(|(at, _)| at != lane) {
                        lanes.move_card(w, *key, lane, usize::MAX);
                    }
                    *old = card.clone();
                }
                None => {
                    if let Some(key) = lanes.add_card(w, lane, data) {
                        self.conversations.push((key, card.clone()));
                    }
                }
            }
        }
    }

    /// What a click on this widget does, if the widget is the button of the page.
    pub fn clicked(&mut self, w: &mut Window, clicked: AnyId) {
        if clicked == self.archived {
            let show = !self.lanes.show_archived();
            self.lanes.set_show_archived(w, show);
            let label = if show {
                "Hide archived"
            } else {
                "Show archived"
            };
            w.set_text(self.archived, label);
        }
    }

    /// Give an event of the window to the board. A change of a task goes to the file. The
    /// action, if there is one, is for a conversation.
    pub fn handle(
        &mut self,
        ui: &Ui,
        w: &mut Window,
        event: &WindowEvent,
    ) -> Result<Option<BoardAction>> {
        let Some(change) = self.lanes.handle(ui, w, event)? else {
            return Ok(None);
        };
        let conversation = |key: &CardKey| {
            let found = self.conversations.iter().find(|(k, _)| k == key);
            found.map(|(_, card)| card.clone())
        };
        match &change {
            BoardEvent::Clicked(key) => Ok(conversation(key).map(|c| BoardAction::Open(c.dir))),
            BoardEvent::Moved(key) => match conversation(key) {
                Some(card) => Ok(self.dropped(w, *key, &card)),
                None => self.save().map(|()| None),
            },
            _ => self.save().map(|()| None),
        }
    }

    /// A conversation went to another lane. Only the Done lane has a meaning for it: the
    /// newest message gives its other lanes. A drop with no meaning puts the card back.
    fn dropped(
        &mut self,
        w: &mut Window,
        key: CardKey,
        card: &Conversation,
    ) -> Option<BoardAction> {
        let (lane, _) = self.lanes.place(key)?;
        let done = lane == lane_of(Stage::Done);
        if done == (card.stage == Stage::Done) {
            if lane != lane_of(card.stage) {
                self.lanes
                    .move_card(w, key, lane_of(card.stage), usize::MAX);
            }
            return None;
        }
        Some(BoardAction::SetDone {
            channel: card.channel.clone(),
            id: card.id.clone(),
            done,
        })
    }

    fn save(&self) -> Result<()> {
        let lanes = (0..self.lanes.lane_count())
            .map(|lane| SavedLane {
                color: self.lanes.lane_tint(lane).map(color),
                cards: self
                    .lanes
                    .cards(lane)
                    .into_iter()
                    .filter(|(_, card)| !card.locked)
                    .map(|(_, card)| SavedCard {
                        title: card.title,
                        body: card.body,
                        pinned: card.pinned,
                        archived: card.archived,
                        color: card.tint.map(color),
                    })
                    .collect(),
            })
            .collect();
        let json = serde_json::to_string_pretty(&Saved { lanes })?;
        nexo_core::write_atomically(&self.path, json)
            .with_context(|| format!("saving {}", self.path.display()))
    }
}

/// A file that is not there gives an empty board.
fn load(path: &Path) -> Result<Saved> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Saved::default()),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn tint(name: Option<&str>) -> Option<Tint> {
    Tint::from_name(name?)
}

fn color(tint: Tint) -> String {
    tint.name().to_lowercase()
}

fn lane_of(stage: Stage) -> usize {
    match stage {
        Stage::ToReply => 0,
        Stage::Waiting => 1,
        Stage::Done => 2,
    }
}

/// The title, cut to a length that keeps the card small.
fn title(full: &str) -> String {
    if full.chars().count() <= MAX_TITLE_CHARS {
        return full.to_owned();
    }
    let cut: String = full.chars().take(MAX_TITLE_CHARS - 3).collect();
    format!("{}...", cut.trim_end())
}

/// The channel, and the time of the newest message.
fn note(card: &Conversation) -> String {
    match &card.latest {
        Some(message) => {
            let time = message.sent_at.with_timezone(&Local);
            format!("{} - {}", card.channel, time.format("%Y-%m-%d %H:%M"))
        }
        None => format!("{} - no messages", card.channel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stage_has_a_lane() {
        let lanes = Stage::ALL.map(lane_of);
        assert_eq!(lanes, [0, 1, 2]);
        assert_eq!(LANES[lane_of(Stage::Done)], "Done");
    }

    #[test]
    fn a_long_title_is_cut() {
        assert_eq!(title("Jane Doe"), "Jane Doe");
        let long = "A group with a very long name of many people from all the teams of the firm";
        let cut = title(long);
        assert!(cut.ends_with("..."));
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn a_card_without_messages_says_so() {
        let card = Conversation {
            channel: "chat".into(),
            id: "c1".into(),
            title: "Jane".into(),
            dir: PathBuf::from("C:\\nexo\\chat\\jane"),
            stage: Stage::Waiting,
            latest: None,
        };
        assert_eq!(note(&card), "chat - no messages");
    }

    #[test]
    fn a_file_that_is_not_there_gives_an_empty_board() {
        let path = std::env::temp_dir().join(format!("nexo-no-board-{}.json", std::process::id()));
        assert_eq!(load(&path).unwrap(), Saved::default());
    }

    #[test]
    fn a_task_needs_only_a_title_in_the_file() {
        let path = std::env::temp_dir().join(format!("nexo-board-{}.json", std::process::id()));
        let text = r#"{"lanes": [{"color": "Blue", "cards": [{"title": "Plan"}]}, {}]}"#;
        std::fs::write(&path, text).unwrap();
        let saved = load(&path);
        std::fs::remove_file(&path).unwrap();
        let saved = saved.unwrap();
        assert_eq!(saved.lanes.len(), 2);
        assert_eq!(tint(saved.lanes[0].color.as_deref()), Some(Tint::Blue));
        assert_eq!(saved.lanes[0].cards[0].title, "Plan");
        assert_eq!(saved.lanes[0].cards[0].color, None);
        assert_eq!(color(Tint::Blue), "blue");
        assert_eq!(tint(Some("none")), None);
    }
}
