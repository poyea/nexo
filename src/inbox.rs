// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use chrono::{DateTime, Local, Utc};
use glasskit::widget::{
    AnyId, Button, Card, Column, Id, Insets, Length, NavItem, Row, Scroll, Stack, Text, TextBox,
    TextStyle, kind,
};
use glasskit::window::Window;
use nexo_core::{Direction, PAGE_ITEMS, Page, Summary, Update};

/// The most channels that the inbox has a tab for.
const MAX_TABS: usize = 6;
const MAX_TITLE_CHARS: usize = 80;
const MAX_PREVIEW_CHARS: usize = 140;
/// The most characters of a conversation that the reader shows.
const MAX_READER_CHARS: usize = 40_000;
const NO_CHANNEL: &str = "Connect a channel on the Channels page to see its conversations here.";

/// What the user asked for on the inbox page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxAction {
    /// Get a page of the conversations of a channel. The answer has the same `tag`.
    Browse {
        channel: String,
        query: String,
        page: Option<String>,
        tag: u64,
    },
    /// Get a conversation with all its messages, to read.
    Open { channel: String, id: String },
    /// Put a conversation on the board.
    Track { channel: String, id: String },
}

/// A place for one conversation of the list. Its widgets stay, and show a different
/// conversation each time.
struct Slot {
    card: Id<kind::Card>,
    title: Id<kind::Text>,
    note: Id<kind::Text>,
    preview: Id<kind::Text>,
    read: Id<kind::Button>,
    add: Id<kind::Button>,
}

/// The conversation that the reader shows.
struct Reading {
    id: String,
    /// The title in the list, until the conversation is here.
    title: String,
}

/// The inbox: the conversations of a channel, page by page, to read and to put on the
/// board.
pub struct Inbox {
    /// The channels that have a list of conversations.
    channels: Vec<String>,
    /// The channel that the list shows.
    channel: Option<String>,
    /// The search that the list shows.
    query: String,
    /// How to get each page that the user saw: `None` for the first page, then
    /// [`Page::next`] of the page before.
    pages: Vec<Option<String>>,
    /// The index in `pages` of the page that shows, or that loads.
    index: usize,
    /// How to get the page after the one that shows.
    next: Option<String>,
    items: Vec<Summary>,
    /// The tag of the newest request. An answer with another tag is old.
    tag: u64,
    loading: bool,
    /// The ids of the conversations that go to the board now.
    adding: Vec<String>,
    reading: Option<Reading>,

    tab_row: Id<kind::Row>,
    tabs: Vec<Id<kind::NavItem>>,
    search: Id<kind::TextBox>,
    go: Id<kind::Button>,
    status: Id<kind::Text>,
    list: Id<kind::Scroll>,
    slots: Vec<Slot>,
    turn: Id<kind::Row>,
    newer: Id<kind::Button>,
    older: Id<kind::Button>,
    reader: Id<kind::Column>,
    back: Id<kind::Button>,
    reader_add: Id<kind::Button>,
    reader_title: Id<kind::Text>,
    reader_text: Id<kind::Text>,
}

impl Inbox {
    /// Make the widgets, and give the column that holds them.
    pub fn build(w: &mut Window) -> (Self, Id<kind::Column>) {
        let tabs: Vec<Id<kind::NavItem>> = (0..MAX_TABS).map(|_| w.add(NavItem::new(""))).collect();
        let tab_row = w.add(Row::new().gap(4.0).children(tabs.iter().copied()));
        let search = w.add(
            TextBox::new()
                .placeholder("Search")
                .max_length(200)
                .width(Length::Fill(1.0)),
        );
        let go = w.add(Button::new("Search").accent());
        let bar = w.add(Row::new().gap(8.0).child(search).child(go));
        let status = w.add(Text::new(NO_CHANNEL).style(TextStyle::Caption));

        let slots: Vec<Slot> = (0..PAGE_ITEMS).map(|_| slot(w)).collect();
        let newer = w.add(Button::new("Newer"));
        let older = w.add(Button::new("Older"));
        let turn = w.add(Row::new().gap(8.0).child(newer).child(older));
        // Space at the right keeps the cards clear of the scroll bar.
        let list = w.add(
            Scroll::new()
                .gap(8.0)
                .padding(Insets {
                    left: 0.0,
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                })
                .children(slots.iter().map(|slot| slot.card))
                .child(turn),
        );

        let back = w.add(Button::new("Back"));
        let reader_add = w.add(Button::new(""));
        let buttons = w.add(Row::new().gap(8.0).child(back).child(reader_add));
        let reader_title = w.add(Text::new("").style(TextStyle::Subtitle));
        let reader_text = w.add(Text::new(""));
        let text = w.add(
            Scroll::new()
                .padding(Insets {
                    left: 0.0,
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                })
                .child(reader_text),
        );
        let reader = w.add(
            Column::new()
                .gap(8.0)
                .child(buttons)
                .child(reader_title)
                .child(text),
        );

        let views = w.add(
            Stack::new()
                .height(Length::Fill(1.0))
                .child(list)
                .child(reader),
        );
        let column = w.add(
            Column::new()
                .gap(8.0)
                .height(Length::Fill(1.0))
                .child(tab_row)
                .child(bar)
                .child(status)
                .child(views),
        );
        let mut inbox = Self {
            channels: Vec::new(),
            channel: None,
            query: String::new(),
            pages: vec![None],
            index: 0,
            next: None,
            items: Vec::new(),
            tag: 0,
            loading: false,
            adding: Vec::new(),
            reading: None,
            tab_row,
            tabs,
            search,
            go,
            status,
            list,
            slots,
            turn,
            newer,
            older,
            reader,
            back,
            reader_add,
            reader_title,
            reader_text,
        };
        inbox.show(w);
        (inbox, column)
    }

    /// Tell the inbox which channels have a list of conversations. It keeps the channel that
    /// shows, if that channel is there, and else shows the first one. The action, if there
    /// is one, loads the first page of the channel that shows now.
    pub fn set_channels(&mut self, w: &mut Window, browsable: &[String]) -> Option<InboxAction> {
        if browsable == self.channels {
            return None;
        }
        self.channels = browsable.to_vec();
        let kept = self
            .channel
            .as_ref()
            .is_some_and(|channel| browsable.contains(channel));
        if kept {
            self.show(w);
            return None;
        }
        self.open_channel(w, browsable.first().cloned())
    }

    /// Show the first page of a channel, or no channel.
    fn open_channel(&mut self, w: &mut Window, channel: Option<String>) -> Option<InboxAction> {
        self.channel = channel;
        self.query.clear();
        self.items.clear();
        self.adding.clear();
        self.reading = None;
        w.set_text(self.search, "");
        let action = self.browse_first(w);
        self.show(w);
        action
    }

    /// What a click on this widget asks for, if the widget is part of the inbox.
    pub fn clicked(&mut self, w: &mut Window, clicked: AnyId) -> Option<InboxAction> {
        if let Some(index) = self.tabs.iter().position(|tab| clicked == *tab) {
            let channel = self.channels.get(index).cloned()?;
            if self.channel.as_ref() == Some(&channel) {
                return None;
            }
            return self.open_channel(w, Some(channel));
        }
        let channel = self.channel.clone()?;
        let action = if clicked == self.go {
            return self.submitted(w, self.search.into());
        } else if clicked == self.newer && self.index > 0 {
            self.browse(self.index - 1)
        } else if clicked == self.older && self.next.is_some() {
            self.pages.truncate(self.index + 1);
            self.pages.push(self.next.clone());
            self.browse(self.index + 1)
        } else if clicked == self.back {
            self.reading = None;
            None
        } else if clicked == self.reader_add {
            let id = self.reading.as_ref()?.id.clone();
            self.track(channel, id)
        } else {
            let (slot, item) = self
                .slots
                .iter()
                .zip(&self.items)
                .find(|(slot, _)| clicked == slot.read || clicked == slot.add)?;
            let (id, title) = (item.id.clone(), item.title.clone());
            if clicked == slot.add {
                self.track(channel, id)
            } else {
                self.reading = Some(Reading {
                    id: id.clone(),
                    title,
                });
                w.set_text(self.reader_text, "Loading");
                Some(InboxAction::Open { channel, id })
            }
        };
        self.show(w);
        action
    }

    /// What Enter in this widget asks for, if the widget is the search box.
    pub fn submitted(&mut self, w: &mut Window, submitted: AnyId) -> Option<InboxAction> {
        if submitted != self.search {
            return None;
        }
        self.query = w.text(self.search).trim().to_owned();
        self.reading = None;
        let action = self.browse_first(w);
        self.show(w);
        action
    }

    /// Show the page of the request with this tag.
    pub fn listed(&mut self, w: &mut Window, tag: u64, page: Result<Page, String>) {
        if tag != self.tag {
            return;
        }
        self.loading = false;
        match page {
            Ok(page) => {
                self.items = page.items;
                self.items.truncate(PAGE_ITEMS);
                self.next = page.next;
                w.set_text(self.status, &self.heading());
            }
            Err(problem) => {
                self.items.clear();
                self.next = None;
                w.set_text(self.status, &problem);
            }
        }
        self.show(w);
    }

    /// Show a conversation in the reader, if the reader waits for it.
    pub fn opened(&mut self, w: &mut Window, id: &str, conversation: Result<Update, String>) {
        if self.reading.as_ref().is_none_or(|r| r.id != id) {
            return;
        }
        match conversation {
            Ok(update) => {
                w.set_text(self.reader_title, &update.conversation.title);
                w.set_text(self.reader_text, &reader_text(&update));
            }
            Err(problem) => w.set_text(self.reader_text, &problem),
        }
    }

    /// A conversation went to the board, or did not.
    pub fn tracked(&mut self, w: &mut Window, id: &str, result: &Result<(), String>) {
        self.adding.retain(|adding| adding != id);
        match result {
            Ok(()) => {
                for item in self.items.iter_mut().filter(|item| item.id == id) {
                    item.tracked = true;
                }
            }
            Err(problem) => w.set_text(self.status, problem),
        }
        self.show(w);
    }

    fn track(&mut self, channel: String, id: String) -> Option<InboxAction> {
        if self.is_tracked(&id) || self.adding.contains(&id) {
            return None;
        }
        self.adding.push(id.clone());
        Some(InboxAction::Track { channel, id })
    }

    fn is_tracked(&self, id: &str) -> bool {
        self.items.iter().any(|item| item.id == id && item.tracked)
    }

    fn browse_first(&mut self, w: &mut Window) -> Option<InboxAction> {
        self.pages = vec![None];
        self.next = None;
        let action = self.browse(0);
        if action.is_none() {
            w.set_text(self.status, NO_CHANNEL);
        }
        action
    }

    fn browse(&mut self, index: usize) -> Option<InboxAction> {
        let channel = self.channel.clone()?;
        self.index = index;
        self.tag += 1;
        self.loading = true;
        Some(InboxAction::Browse {
            channel,
            query: self.query.clone(),
            page: self.pages.get(index).cloned().flatten(),
            tag: self.tag,
        })
    }

    /// The line over the list: what the list shows.
    fn heading(&self) -> String {
        let what = match (self.query.as_str(), self.items.len()) {
            ("", 0) => "No conversations".to_owned(),
            (query, 0) => format!("Nothing for \"{query}\""),
            ("", _) => format!("Page {}", self.index + 1),
            (query, _) => format!("Page {} for \"{query}\"", self.index + 1),
        };
        match &self.channel {
            Some(channel) => format!("{channel} - {what}"),
            None => what,
        }
    }

    /// Make the widgets agree with the state.
    fn show(&mut self, w: &mut Window) {
        w.set_visible(self.tab_row, self.channels.len() > 1);
        for (index, tab) in self.tabs.iter().enumerate() {
            let channel = self.channels.get(index);
            w.set_visible(*tab, channel.is_some());
            if let Some(channel) = channel {
                w.set_text(*tab, &tab_label(channel));
                w.set_selected(*tab, self.channel.as_ref() == Some(channel));
            }
        }
        let connected = self.channel.is_some();
        w.set_enabled(self.search, connected);
        w.set_enabled(self.go, connected && !self.loading);
        if self.loading {
            w.set_text(self.status, "Loading");
        }
        let reading = self.reading.is_some();
        w.set_visible(self.list, !reading);
        w.set_visible(self.reader, reading);

        let mut items = self.items.iter();
        for slot in &self.slots {
            let item = items.next();
            w.set_visible(slot.card, item.is_some());
            if let Some(item) = item {
                w.set_text(slot.title, &cut(&item.title, MAX_TITLE_CHARS));
                w.set_text(slot.note, &note(item));
                w.set_text(slot.preview, &cut(&item.preview, MAX_PREVIEW_CHARS));
                let adding = self.adding.contains(&item.id);
                w.set_text(slot.add, add_label(item.tracked, adding));
                w.set_enabled(slot.add, !item.tracked && !adding);
            }
        }
        w.set_visible(self.turn, !self.items.is_empty() || self.index > 0);
        w.set_enabled(self.newer, !self.loading && self.index > 0);
        w.set_enabled(self.older, !self.loading && self.next.is_some());

        if let Some(reading) = &self.reading {
            let tracked = self.is_tracked(&reading.id);
            let adding = self.adding.contains(&reading.id);
            w.set_text(self.reader_title, &reading.title);
            w.set_text(self.reader_add, add_label(tracked, adding));
            w.set_enabled(self.reader_add, !tracked && !adding);
        }
    }
}

fn slot(w: &mut Window) -> Slot {
    let title = w.add(Text::new("").style(TextStyle::BodyStrong));
    let note = w.add(Text::new("").style(TextStyle::Caption));
    let preview = w.add(Text::new("").style(TextStyle::Caption));
    let lines = w.add(
        Column::new()
            .width(Length::Fill(1.0))
            .gap(2.0)
            .child(title)
            .child(note)
            .child(preview),
    );
    let read = w.add(Button::new("Read"));
    let add = w.add(Button::new(""));
    let row = w.add(Row::new().gap(8.0).child(lines).child(read).child(add));
    let card = w.add(
        Card::new()
            .padding(Insets::symmetric(16.0, 10.0))
            .child(row),
    );
    Slot {
        card,
        title,
        note,
        preview,
        read,
        add,
    }
}

/// The name of a channel with a capital letter at its start.
fn tab_label(channel: &str) -> String {
    let mut chars = channel.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn add_label(tracked: bool, adding: bool) -> &'static str {
    match (tracked, adding) {
        (true, _) => "On board",
        (false, true) => "Adding",
        (false, false) => "Add to board",
    }
}

/// The text, cut to a length that keeps a card small.
fn cut(full: &str, max_chars: usize) -> String {
    let full = full.trim();
    if full.chars().count() <= max_chars {
        return full.to_owned();
    }
    let cut: String = full.chars().take(max_chars - 3).collect();
    format!("{}...", cut.trim_end())
}

fn local(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Who wrote the newest message and when, and what is special about the conversation.
fn note(item: &Summary) -> String {
    let mut parts = vec![item.from.clone()];
    parts.extend(item.latest_at.map(local));
    if item.messages > 1 {
        parts.push(format!("{} messages", item.messages));
    }
    if item.unread {
        parts.push("unread".into());
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" - ")
}

/// The messages of a conversation as one text, the oldest first.
fn reader_text(update: &Update) -> String {
    let mut messages: Vec<_> = update.messages.iter().collect();
    messages.sort_by_key(|m| m.sent_at);
    let mut out = String::new();
    if !update.conversation.participants.is_empty() {
        out.push_str(&update.conversation.participants.join(", "));
        out.push_str("\n\n");
    }
    for message in messages {
        let you = match message.direction {
            Direction::In => "",
            Direction::Out => " (you)",
        };
        out.push_str(&format!(
            "{}{you} - {}\n\n{}\n\n",
            message.author,
            local(message.sent_at),
            message.body.trim()
        ));
    }
    if out.chars().count() > MAX_READER_CHARS {
        out = out.chars().take(MAX_READER_CHARS).collect();
        out.push_str("\n\nThe conversation is longer than what shows here.");
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use nexo_core::{Conversation, Message};

    use super::*;

    fn summary() -> Summary {
        Summary {
            id: "t1".into(),
            title: "Offer".into(),
            from: "Jane".into(),
            latest_at: None,
            preview: "Hi".into(),
            unread: false,
            messages: 1,
            tracked: false,
        }
    }

    #[test]
    fn a_tab_has_the_name_of_its_channel() {
        assert_eq!(tab_label("gmail"), "Gmail");
        assert_eq!(tab_label(""), "");
    }

    #[test]
    fn a_long_text_is_cut() {
        assert_eq!(cut(" Offer ", 10), "Offer");
        assert_eq!(cut("A very long subject line", 12), "A very lo...");
        assert_eq!(cut("A very long subject line", 12).chars().count(), 12);
    }

    #[test]
    fn the_note_tells_what_is_special() {
        assert_eq!(note(&summary()), "Jane");
        let busy = Summary {
            unread: true,
            messages: 3,
            from: String::new(),
            ..summary()
        };
        assert_eq!(note(&busy), "3 messages - unread");
    }

    #[test]
    fn the_button_tells_where_the_conversation_is() {
        assert_eq!(add_label(false, false), "Add to board");
        assert_eq!(add_label(false, true), "Adding");
        assert_eq!(add_label(true, false), "On board");
        assert_eq!(add_label(true, true), "On board");
    }

    #[test]
    fn the_reader_shows_the_oldest_message_first() {
        let message = |id: &str, minute, direction, body: &str| Message {
            id: id.into(),
            author: "Jane".into(),
            sent_at: Utc.with_ymd_and_hms(2026, 11, 9, 12, minute, 0).unwrap(),
            direction,
            body: body.into(),
        };
        let update = Update {
            conversation: Conversation {
                id: "t1".into(),
                title: "Offer".into(),
                participants: vec!["Jane".into(), "Ann".into()],
            },
            messages: vec![
                message("m2", 5, Direction::Out, "Second\n"),
                message("m1", 0, Direction::In, "First"),
            ],
        };
        let text = reader_text(&update);
        assert!(text.starts_with("Jane, Ann\n\nJane - "));
        assert!(text.find("First").unwrap() < text.find("Jane (you) - ").unwrap());
        assert!(text.ends_with("Second"));
    }
}
