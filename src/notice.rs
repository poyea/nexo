// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use nexo_core::{Card, Direction};

/// The most names that a notification shows.
const MAX_NAMES: usize = 3;

/// The title and the text of the notification for a sync that brought `received` new
/// messages. `old` and `new` are the conversations before and after the sync: the text
/// names the conversations whose newest message is new and from another person.
pub fn new_messages(received: usize, old: Option<&[Card]>, new: &[Card]) -> (String, String) {
    let title = match received {
        1 => "1 new message".to_owned(),
        n => format!("{n} new messages"),
    };
    let names = old.map_or_else(Vec::new, |old| changed(old, new));
    let text = match names.as_slice() {
        [] => "Open nexo to read.".to_owned(),
        names if names.len() <= MAX_NAMES => names.join(", "),
        names => format!(
            "{} and {} more",
            names[..MAX_NAMES].join(", "),
            names.len() - MAX_NAMES
        ),
    };
    (title, text)
}

/// The titles of the conversations with a newest message that `old` does not have, from
/// another person.
fn changed<'a>(old: &[Card], new: &'a [Card]) -> Vec<&'a str> {
    let newest = |card: &Card| card.latest.as_ref().map(|m| m.id.clone());
    new.iter()
        .filter(|card| {
            let incoming = card
                .latest
                .as_ref()
                .is_some_and(|m| m.direction == Direction::In);
            let before = old
                .iter()
                .find(|o| o.channel == card.channel && o.id == card.id);
            incoming && before.is_none_or(|before| newest(before) != newest(card))
        })
        .map(|card| card.title.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use nexo_core::{Message, Stage};

    use super::*;

    fn card(title: &str, message: Option<(&str, Direction)>) -> Card {
        Card {
            channel: "chat".into(),
            id: title.to_lowercase(),
            title: title.into(),
            dir: PathBuf::from("C:\\nexo\\chat"),
            stage: Stage::ToReply,
            latest: message.map(|(id, direction)| Message {
                id: id.into(),
                author: title.into(),
                sent_at: Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap(),
                direction,
                body: "Hi".into(),
            }),
        }
    }

    #[test]
    fn the_text_names_the_conversations_with_new_messages() {
        let old = [
            card("Jane", Some(("m1", Direction::In))),
            card("Bob", Some(("m1", Direction::In))),
            card("Eve", None),
        ];
        let new = [
            card("Jane", Some(("m2", Direction::In))),
            card("Bob", Some(("m1", Direction::In))),
            card("Eve", Some(("m1", Direction::Out))),
            card("Ann", Some(("m1", Direction::In))),
        ];
        assert_eq!(
            new_messages(2, Some(&old), &new),
            ("2 new messages".to_owned(), "Jane, Ann".to_owned())
        );
    }

    #[test]
    fn a_long_list_of_names_is_cut() {
        let new: Vec<Card> = ["A", "B", "C", "D", "E"]
            .map(|title| card(title, Some(("m1", Direction::In))))
            .into();
        let (_, text) = new_messages(5, Some(&[]), &new);
        assert_eq!(text, "A, B, C and 2 more");
    }

    #[test]
    fn without_names_the_text_says_what_to_do() {
        let new = [card("Jane", Some(("m1", Direction::In)))];
        assert_eq!(
            new_messages(1, None, &new),
            ("1 new message".to_owned(), "Open nexo to read.".to_owned())
        );
        let (_, text) = new_messages(1, Some(&new), &new);
        assert_eq!(text, "Open nexo to read.");
    }
}
