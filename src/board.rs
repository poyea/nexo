// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use chrono::Local;
use glasskit::widget::{
    Align, AnyId, Button, Card, Column, Id, Insets, Length, Row, Scroll, Text, TextStyle, kind,
};
use glasskit::window::Window;
use nexo_core::{Card as Conversation, Stage};

/// How many cards a lane shows. A lane scrolls, and the rest after these is a count.
const SLOTS: usize = 40;
const MAX_TITLE_CHARS: usize = 32;

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

/// A place for one card. Its widgets stay, and show a different conversation each time.
struct Slot {
    card: Id<kind::Card>,
    title: Id<kind::Text>,
    note: Id<kind::Text>,
    open: Id<kind::Button>,
    mark: Id<kind::Button>,
    shown: Option<Conversation>,
}

struct Lane {
    stage: Stage,
    heading: Id<kind::Text>,
    slots: Vec<Slot>,
    /// A note under the cards: that the lane is empty, or how many cards do not show.
    rest: Id<kind::Text>,
}

/// The board: one lane for each stage, with the conversations as cards.
pub struct Board {
    lanes: Vec<Lane>,
}

impl Board {
    /// Make the widgets, and give the row that holds the lanes.
    pub fn build(w: &mut Window) -> (Self, Id<kind::Row>) {
        let lanes: Vec<(Lane, Id<kind::Column>)> =
            Stage::ALL.into_iter().map(|stage| lane(w, stage)).collect();
        let row = w.add(
            Row::new()
                .gap(12.0)
                .align(Align::Start)
                .height(Length::Fill(1.0))
                .children(lanes.iter().map(|(_, column)| *column)),
        );
        let mut board = Self {
            lanes: lanes.into_iter().map(|(lane, _)| lane).collect(),
        };
        board.set(w, &[]);
        (board, row)
    }

    /// Show these conversations. They are in the order that the lanes show them.
    pub fn set(&mut self, w: &mut Window, cards: &[Conversation]) {
        for lane in &mut self.lanes {
            let mut mine = cards.iter().filter(|c| c.stage == lane.stage);
            let total = mine.clone().count();
            w.set_text(lane.heading, &heading(lane.stage, total));
            for slot in &mut lane.slots {
                slot.shown = mine.next().cloned();
                w.set_visible(slot.card, slot.shown.is_some());
                if let Some(card) = &slot.shown {
                    w.set_text(slot.title, &title(&card.title));
                    w.set_text(slot.note, &note(card));
                    w.set_text(slot.mark, mark_label(card.stage));
                }
            }
            let rest = rest(total);
            w.set_visible(lane.rest, !rest.is_empty());
            w.set_text(lane.rest, &rest);
        }
    }

    /// What a click on this widget asks for, if the widget is a button of the board.
    pub fn action(&self, clicked: AnyId) -> Option<BoardAction> {
        let slots = self.lanes.iter().flat_map(|lane| &lane.slots);
        for slot in slots {
            let Some(card) = &slot.shown else {
                continue;
            };
            if clicked == slot.open {
                return Some(BoardAction::Open(card.dir.clone()));
            }
            if clicked == slot.mark {
                return Some(BoardAction::SetDone {
                    channel: card.channel.clone(),
                    id: card.id.clone(),
                    done: card.stage != Stage::Done,
                });
            }
        }
        None
    }
}

fn lane(w: &mut Window, stage: Stage) -> (Lane, Id<kind::Column>) {
    let heading = w.add(Text::new("").style(TextStyle::BodyStrong));
    let slots: Vec<Slot> = (0..SLOTS).map(|_| slot(w)).collect();
    let rest = w.add(Text::new("").style(TextStyle::Caption));
    // Space at the right keeps the cards clear of the scroll bar.
    let cards = w.add(
        Scroll::new()
            .gap(8.0)
            .padding(Insets {
                left: 0.0,
                top: 0.0,
                right: 10.0,
                bottom: 0.0,
            })
            .children(slots.iter().map(|slot| slot.card))
            .child(rest),
    );
    let column = w.add(
        Column::new()
            .width(Length::Fill(1.0))
            .gap(8.0)
            .child(heading)
            .child(cards),
    );
    let lane = Lane {
        stage,
        heading,
        slots,
        rest,
    };
    (lane, column)
}

fn slot(w: &mut Window) -> Slot {
    let title = w.add(Text::new("").style(TextStyle::BodyStrong));
    let note = w.add(Text::new("").style(TextStyle::Caption));
    let open = w.add(Button::new("Open"));
    let mark = w.add(Button::new(""));
    let buttons = w.add(Row::new().gap(8.0).child(open).child(mark));
    let lines = w.add(
        Column::new()
            .gap(4.0)
            .align(Align::Start)
            .child(title)
            .child(note)
            .child(buttons),
    );
    let card = w.add(Card::new().child(lines));
    Slot {
        card,
        title,
        note,
        open,
        mark,
        shown: None,
    }
}

fn heading(stage: Stage, total: usize) -> String {
    let name = match stage {
        Stage::ToReply => "To reply",
        Stage::Waiting => "Waiting",
        Stage::Done => "Done",
    };
    format!("{name}  {total}")
}

fn mark_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Done => "Reopen",
        Stage::ToReply | Stage::Waiting => "Done",
    }
}

fn rest(total: usize) -> String {
    match total.saturating_sub(SLOTS) {
        _ if total == 0 => "Nothing here".into(),
        0 => String::new(),
        more => format!("and {more} more"),
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
    fn headings_count_the_cards() {
        assert_eq!(heading(Stage::ToReply, 3), "To reply  3");
        assert_eq!(heading(Stage::Done, 0), "Done  0");
    }

    #[test]
    fn rest_tells_of_an_empty_lane_and_of_hidden_cards() {
        assert_eq!(rest(0), "Nothing here");
        assert_eq!(rest(1), "");
        assert_eq!(rest(SLOTS), "");
        assert_eq!(rest(SLOTS + 3), "and 3 more");
    }

    #[test]
    fn a_long_title_is_cut() {
        assert_eq!(title("Jane Doe"), "Jane Doe");
        let long = "A group with a very long name of many people";
        let cut = title(long);
        assert_eq!(cut, "A group with a very long name...");
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn done_cards_can_be_opened_again() {
        assert_eq!(mark_label(Stage::Done), "Reopen");
        assert_eq!(mark_label(Stage::ToReply), "Done");
        assert_eq!(mark_label(Stage::Waiting), "Done");
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
}
