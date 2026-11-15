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

/// The names of the lanes of the stages of a conversation, in the order of `Stage::ALL`.
/// The user can change them, add more lanes, and put the lanes in another order.
const LANES: [&str; 3] = ["To do", "Waiting", "Done"];
/// The names of the stages in the file, in the same order.
const STAGES: [&str; 3] = ["to_reply", "waiting", "done"];
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

/// The board as the file keeps it: the lanes, and the tasks of the user in them.
/// The conversations are not in the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    lanes: Vec<SavedLane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct SavedLane {
    /// A lane without a name has the name of its stage.
    #[serde(default)]
    name: Option<String>,
    /// The stage whose conversations are in the lane. A lane of the user has none. A file
    /// with no stage at all is from before the user could move lanes: its first three
    /// lanes are the stages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
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
    add_lane: Id<kind::Button>,
    archived: Id<kind::Button>,
    /// The lane of each stage, in the order of `Stage::ALL`.
    stages: [usize; 3],
    /// The card of each conversation.
    conversations: Vec<(CardKey, Conversation)>,
}

impl Board {
    /// Make the widgets with the tasks of the file, and give the column that holds them.
    pub fn build(w: &mut Window, path: &Path) -> Result<(Self, Id<kind::Column>)> {
        let (saved, stages) = arrange(load(path)?);
        let names: Vec<&str> = saved
            .iter()
            .map(|lane| lane.name.as_deref().unwrap_or_default())
            .collect();
        let mut lanes = Lanes::new(w, &names);
        for lane in stages {
            lanes.set_lane_fixed(lane, true);
        }
        for (lane, saved) in saved.into_iter().enumerate() {
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
        let add_lane = w.add(Button::new("Add lane"));
        let archived = w.add(Button::new("Show archived"));
        let bar = w.add(
            Row::new()
                .gap(8.0)
                .child(gap)
                .child(add_lane)
                .child(archived),
        );
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
            add_lane,
            archived,
            stages,
            conversations: Vec::new(),
        };
        Ok((board, column))
    }

    /// Show these conversations as cards, each in the lane of its stage.
    pub fn set(&mut self, w: &mut Window, cards: &[Conversation]) {
        let same = |a: &Conversation, b: &Conversation| a.channel == b.channel && a.id == b.id;
        let stages = self.stages;
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
            let lane = stages[stage_index(card.stage)];
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

    /// Do what a click on this widget asks for, if the widget is a button of the page.
    pub fn clicked(&mut self, ui: &Ui, w: &mut Window, clicked: AnyId) -> Result<()> {
        if clicked == self.archived {
            let show = !self.lanes.show_archived();
            self.lanes.set_show_archived(w, show);
            let label = if show {
                "Hide archived"
            } else {
                "Show archived"
            };
            w.set_text(self.archived, label);
        } else if clicked == self.add_lane && self.lanes.add_lane_by_user(ui, w)?.is_some() {
            self.save()?;
        }
        Ok(())
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
            BoardEvent::LaneMoved { from, to } => {
                self.stages = self.stages.map(|lane| moved(lane, *from, *to));
                self.save().map(|()| None)
            }
            BoardEvent::LaneRemoved { lane: removed, .. } => {
                self.stages = self.stages.map(|lane| lane - usize::from(lane > *removed));
                self.save().map(|()| None)
            }
            _ => self.save().map(|()| None),
        }
    }

    fn lane_of(&self, stage: Stage) -> usize {
        self.stages[stage_index(stage)]
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
        let done = lane == self.lane_of(Stage::Done);
        if done == (card.stage == Stage::Done) {
            let own = self.lane_of(card.stage);
            if lane != own {
                self.lanes.move_card(w, key, own, usize::MAX);
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
                name: self.lanes.lane_name(lane).map(str::to_owned),
                stage: self
                    .stages
                    .iter()
                    .position(|&at| at == lane)
                    .map(|stage| STAGES[stage].to_owned()),
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

/// The place of a stage in `Stage::ALL`.
fn stage_index(stage: Stage) -> usize {
    match stage {
        Stage::ToReply => 0,
        Stage::Waiting => 1,
        Stage::Done => 2,
    }
}

/// The index of a lane after the lane at `from` went to `to`.
fn moved(lane: usize, from: usize, to: usize) -> usize {
    if lane == from {
        return to;
    }
    let without = lane - usize::from(lane > from);
    without + usize::from(without >= to)
}

/// The lanes of a file in their order, each with a name, and the lane of each stage. A
/// stage that has no lane in the file gets a new lane at the end.
fn arrange(saved: Saved) -> (Vec<SavedLane>, [usize; 3]) {
    let mut lanes = saved.lanes;
    let known = |lane: &SavedLane| lane.stage.as_deref().is_some_and(|s| STAGES.contains(&s));
    if !lanes.iter().any(known) {
        for (lane, stage) in lanes.iter_mut().zip(STAGES) {
            lane.stage = Some(stage.to_owned());
        }
    }
    let stages = STAGES.map(|stage| {
        let found = lanes
            .iter()
            .position(|lane| lane.stage.as_deref() == Some(stage));
        found.unwrap_or_else(|| {
            lanes.push(SavedLane {
                stage: Some(stage.to_owned()),
                ..SavedLane::default()
            });
            lanes.len() - 1
        })
    });
    for (index, lane) in lanes.iter_mut().enumerate() {
        let stage = stages.iter().position(|&at| at == index);
        lane.stage = stage.map(|stage| STAGES[stage].to_owned());
        let name = lane.name.as_deref().map(str::trim).unwrap_or_default();
        lane.name = Some(match (name, stage) {
            ("", Some(stage)) => LANES[stage].to_owned(),
            ("", None) => "Lane".to_owned(),
            (name, _) => name.to_owned(),
        });
    }
    (lanes, stages)
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

    fn lane(name: Option<&str>, stage: Option<&str>) -> SavedLane {
        SavedLane {
            name: name.map(str::to_owned),
            stage: stage.map(str::to_owned),
            ..SavedLane::default()
        }
    }

    fn names(lanes: &[SavedLane]) -> Vec<&str> {
        lanes.iter().map(|l| l.name.as_deref().unwrap()).collect()
    }

    #[test]
    fn a_lane_of_the_user_keeps_its_name_in_the_file() {
        let saved = Saved {
            lanes: vec![
                lane(None, Some("done")),
                SavedLane {
                    color: Some("teal".into()),
                    ..lane(Some("Ideas"), None)
                },
            ],
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), saved);
        assert_eq!(text.matches("stage").count(), 1);
    }

    #[test]
    fn an_empty_file_gives_the_lanes_of_the_stages() {
        let (lanes, stages) = arrange(Saved::default());
        assert_eq!(names(&lanes), LANES);
        assert_eq!(stages, Stage::ALL.map(stage_index));
        assert_eq!(lanes[2].stage.as_deref(), Some("done"));
    }

    #[test]
    fn a_file_without_stages_has_them_in_its_first_lanes() {
        let saved = Saved {
            lanes: vec![
                lane(Some("Reply"), None),
                lane(None, None),
                lane(None, None),
                lane(Some("Ideas"), None),
            ],
        };
        let (lanes, stages) = arrange(saved);
        assert_eq!(names(&lanes), ["Reply", "Waiting", "Done", "Ideas"]);
        assert_eq!(stages, [0, 1, 2]);
        assert_eq!(lanes[3].stage, None);
    }

    #[test]
    fn the_lanes_of_the_stages_keep_the_order_of_the_file() {
        let saved = Saved {
            lanes: vec![
                lane(Some("Finished"), Some("done")),
                lane(Some("Ideas"), None),
                lane(None, Some("to_reply")),
                lane(Some("Copy"), Some("done")),
                lane(None, Some("later")),
            ],
        };
        let (lanes, stages) = arrange(saved);
        assert_eq!(
            names(&lanes),
            ["Finished", "Ideas", "To do", "Copy", "Lane", "Waiting"]
        );
        assert_eq!(stages, [2, 5, 0]);
        assert_eq!(lanes[3].stage, None);
        assert_eq!(lanes[4].stage, None);
    }

    #[test]
    fn a_lane_that_moves_changes_the_index_of_the_lanes_between() {
        // The lanes A B C D: A goes to the end, which gives B C D A.
        assert_eq!([0, 1, 2, 3].map(|lane| moved(lane, 0, 3)), [3, 0, 1, 2]);
        // D goes to the start, which gives D A B C.
        assert_eq!([0, 1, 2, 3].map(|lane| moved(lane, 3, 0)), [1, 2, 3, 0]);
        // B goes after C, which gives A C B D.
        assert_eq!([0, 1, 2, 3].map(|lane| moved(lane, 1, 2)), [0, 2, 1, 3]);
    }

    #[test]
    fn each_stage_has_a_name_for_its_lane_and_for_the_file() {
        assert_eq!(Stage::ALL.map(stage_index), [0, 1, 2]);
        assert_eq!(LANES[stage_index(Stage::Done)], "Done");
        assert_eq!(STAGES[stage_index(Stage::ToReply)], "to_reply");
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
        assert_eq!(saved.lanes[0].name, None);
        assert_eq!(tint(saved.lanes[0].color.as_deref()), Some(Tint::Blue));
        assert_eq!(saved.lanes[0].cards[0].title, "Plan");
        assert_eq!(saved.lanes[0].cards[0].color, None);
        assert_eq!(color(Tint::Blue), "blue");
        assert_eq!(tint(Some("none")), None);
    }
}
