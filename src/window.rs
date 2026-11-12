// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use anyhow::Result;
use glasskit::Ui;
use glasskit::events::WindowId;
use glasskit::tray::Icon;
use glasskit::widget::{
    Align, AnyId, Button, Card, Column, Id, Insets, Length, NavItem, Row, Scroll, Spacer, Stack,
    Text, TextBox, TextStyle, kind,
};
use glasskit::window::{Window, WindowEvent};
use nexo_core::Report;

use crate::board::{Board, BoardAction};
use crate::channels::{Channels, ChannelsAction, GmailState};
use crate::config::{Config, Interval, Unit};
use crate::inbox::{Inbox, InboxAction};

const SIDE_WIDTH: f32 = 208.0;
const ICON_SIZE: u32 = 64;
const INTERVAL_HINT: &str = "Type a number from 1, and pick seconds or minutes.";

/// What the user asked for in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SyncNow,
    OpenFolder,
    ToggleAutostart,
    SetInterval(Interval),
    Board(BoardAction),
    Inbox(InboxAction),
    Channels(ChannelsAction),
    Close,
}

/// What the overview page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub syncing: bool,
    /// The clock time of the last sync, such as `14:05`, and its report.
    pub last: Option<(String, Report)>,
}

impl View {
    fn headline(&self) -> String {
        match &self.last {
            _ if self.syncing => "Syncing".into(),
            None => "Starting".into(),
            Some((_, report)) => match report.problems.len() {
                0 => "Up to date".into(),
                1 => "1 problem".into(),
                n => format!("{n} problems"),
            },
        }
    }

    fn detail(&self) -> String {
        match &self.last {
            None => "No sync yet".into(),
            Some((time, _)) => format!("Last sync at {time}"),
        }
    }

    /// New messages, sent replies, and problems of the last sync.
    fn counts(&self) -> [usize; 3] {
        self.last.as_ref().map_or([0; 3], |(_, report)| {
            [report.received, report.sent, report.problems.len()]
        })
    }

    fn problems(&self) -> String {
        self.last
            .as_ref()
            .map(|(_, report)| report.problems.join("\n"))
            .unwrap_or_default()
    }
}

/// One line for a sync that ended at `time`, such as `14:05`.
pub fn summary(report: &Report, time: &str) -> String {
    let mut parts = vec![format!("{} new", report.received)];
    if report.sent > 0 {
        parts.push(format!("{} sent", report.sent));
    }
    match report.problems.len() {
        0 => {}
        1 => parts.push("1 problem".into()),
        n => parts.push(format!("{n} problems")),
    }
    format!("Synced at {time}: {}", parts.join(", "))
}

/// An item of the side list, and the page that it shows.
struct Tab {
    item: Id<kind::NavItem>,
    page: Id<kind::Column>,
}

pub struct MainWindow {
    window: Window,
    tabs: Vec<Tab>,
    headline: Id<kind::Text>,
    detail: Id<kind::Text>,
    counts: [Id<kind::Text>; 3],
    problems_card: Id<kind::Card>,
    problems: Id<kind::Text>,
    sync: Id<kind::Button>,
    open: Id<kind::Button>,
    autostart: Id<kind::Button>,
    autostart_state: Id<kind::Text>,
    interval_title: Id<kind::Text>,
    interval_note: Id<kind::Text>,
    interval_box: Id<kind::TextBox>,
    units: [(Id<kind::NavItem>, Unit); 2],
    unit: Unit,
    board: Board,
    inbox: Inbox,
    channels: Channels,
}

impl MainWindow {
    pub fn new(ui: &Ui, config: &Config, autostart: bool) -> Result<Self> {
        let mut w = Window::new(ui, "nexo", 1000.0, 680.0)?;
        w.set_min_size(760.0, 660.0);

        let headline = w.add(Text::new("Starting").style(TextStyle::Subtitle));
        let detail = w.add(Text::new("No sync yet").style(TextStyle::Caption));
        let sync = w.add(Button::new("Sync now").accent());
        let status = setting(&mut w, headline, detail, &[sync.into()]);
        let counts = [(); 3].map(|()| w.add(Text::new("0").style(TextStyle::Title)));
        let stats: Vec<Id<kind::Card>> = ["New messages", "Replies sent", "Problems"]
            .into_iter()
            .zip(counts)
            .map(|(label, count)| {
                let label = w.add(Text::new(label).style(TextStyle::Caption));
                let lines = w.add(Column::new().gap(2.0).child(count).child(label));
                w.add(Card::new().width(Length::Fill(1.0)).child(lines))
            })
            .collect();
        let stats = w.add(Row::new().gap(12.0).align(Align::Stretch).children(stats));
        let problems_title = w.add(Text::new("Problems").style(TextStyle::BodyStrong));
        let problems = w.add(Text::new(""));
        let lines = w.add(Column::new().gap(8.0).child(problems_title).child(problems));
        let problems_card = w.add(Card::new().child(lines));
        w.set_visible(problems_card, false);
        let overview = page(
            &mut w,
            "Overview",
            vec![status.into(), stats.into(), problems_card.into()],
        );

        let (board, lanes) = Board::build(&mut w);
        // Each lane scrolls on its own, so the page does not.
        let board_page = framed(&mut w, "Board", lanes.into());

        let (inbox, list) = Inbox::build(&mut w);
        let inbox_page = framed(&mut w, "Inbox", list.into());

        let (channels, cards) = Channels::build(&mut w);
        let channels_page = page(&mut w, "Channels", cards);

        let folder_title = w.add(Text::new("Output folder"));
        let folder =
            w.add(Text::new(config.output.display().to_string()).style(TextStyle::Caption));
        let open = w.add(Button::new("Open folder"));
        let folder = setting(&mut w, folder_title, folder, &[open.into()]);
        let interval = config.interval();
        let interval_title = w.add(Text::new(interval.describe()));
        let interval_note = w.add(Text::new(INTERVAL_HINT).style(TextStyle::Caption));
        let interval_box = w.add(TextBox::new().max_length(5).width(Length::Fixed(72.0)));
        w.set_text(interval_box, &interval.every.to_string());
        let units = [
            (w.add(NavItem::new("Seconds")), Unit::Seconds),
            (w.add(NavItem::new("Minutes")), Unit::Minutes),
        ];
        let schedule = setting(
            &mut w,
            interval_title,
            interval_note,
            &[interval_box.into(), units[0].0.into(), units[1].0.into()],
        );
        let autostart_title = w.add(Text::new("Start with Windows"));
        let autostart_state = w.add(Text::new("").style(TextStyle::Caption));
        let autostart_button = w.add(Button::new(""));
        let startup = setting(
            &mut w,
            autostart_title,
            autostart_state,
            &[autostart_button.into()],
        );
        let how = w.add(
            Text::new(
                "To change the folder, edit config.toml next to the app, then start nexo again.",
            )
            .style(TextStyle::Caption),
        );
        let settings = page(
            &mut w,
            "Settings",
            vec![folder.into(), schedule.into(), startup.into(), how.into()],
        );

        let brand = w.add(Text::new("nexo").style(TextStyle::Subtitle));
        let brand = w.add(
            Column::new()
                .padding(Insets {
                    left: 16.0,
                    top: 8.0,
                    right: 16.0,
                    bottom: 12.0,
                })
                .child(brand),
        );
        let items = [
            w.add(NavItem::new("Overview")),
            w.add(NavItem::new("Inbox")),
            w.add(NavItem::new("Board")),
            w.add(NavItem::new("Channels")),
            w.add(NavItem::new("Settings")),
        ];
        let gap = w.add(Spacer::fill());
        let side = w.add(
            Column::new()
                .width(Length::Fixed(SIDE_WIDTH))
                .gap(4.0)
                .child(brand)
                .child(items[0])
                .child(items[1])
                .child(items[2])
                .child(items[3])
                .child(gap)
                .child(items[4]),
        );
        let pages = [overview, inbox_page, board_page, channels_page, settings];
        let stack = w.add(Stack::new().width(Length::Fill(1.0)).children(pages));
        let root = w.add(
            Row::new()
                .padding(Insets::all(8.0))
                .gap(8.0)
                .align(Align::Stretch)
                .child(side)
                .child(stack),
        );
        w.set_root(root);

        let mut this = Self {
            window: w,
            tabs: items
                .into_iter()
                .zip(pages)
                .map(|(item, page)| Tab { item, page })
                .collect(),
            headline,
            detail,
            counts,
            problems_card,
            problems,
            sync,
            open,
            autostart: autostart_button,
            autostart_state,
            interval_title,
            interval_note,
            interval_box,
            units,
            unit: interval.unit,
            board,
            inbox,
            channels,
        };
        this.show_unit();
        this.show_tab(0);
        this.set_autostart(autostart);
        this.window.set_icon(&app_icon(ICON_SIZE)?)?;
        this.window.show();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn set_view(&mut self, view: &View) {
        self.window.set_text(self.headline, &view.headline());
        self.window.set_text(self.detail, &view.detail());
        for (id, count) in self.counts.into_iter().zip(view.counts()) {
            self.window.set_text(id, &count.to_string());
        }
        let problems = view.problems();
        self.window.set_text(self.problems, &problems);
        self.window
            .set_visible(self.problems_card, !problems.is_empty());
        self.window.set_enabled(self.sync, !view.syncing);
    }

    /// Show these conversations on the board.
    pub fn set_board(&mut self, cards: &[nexo_core::Card]) {
        self.board.set(&mut self.window, cards);
    }

    /// Tell the inbox which channels have a list of conversations. The action, if there is
    /// one, loads the first page of a channel that is new.
    pub fn set_browsable(&mut self, channels: &[String]) -> Option<Action> {
        self.inbox
            .set_channels(&mut self.window, channels)
            .map(Action::Inbox)
    }

    /// Show a page of conversations in the inbox. `tag` is the tag of its request.
    pub fn show_listed(&mut self, tag: u64, page: Result<nexo_core::Page, String>) {
        self.inbox.listed(&mut self.window, tag, page);
    }

    /// Show a conversation in the reader of the inbox.
    pub fn show_opened(&mut self, id: &str, conversation: Result<nexo_core::Update, String>) {
        self.inbox.opened(&mut self.window, id, conversation);
    }

    /// Show in the inbox that a conversation went to the board, or did not.
    pub fn show_tracked(&mut self, id: &str, result: &Result<(), String>) {
        self.inbox.tracked(&mut self.window, id, result);
    }

    pub fn set_gmail(&mut self, state: &GmailState) {
        self.channels.set_gmail(&mut self.window, state);
    }

    pub fn set_autostart(&mut self, on: bool) {
        let (state, label) = if on {
            ("On", "Turn off")
        } else {
            ("Off", "Turn on")
        };
        self.window.set_text(self.autostart_state, state);
        self.window.set_text(self.autostart, label);
    }

    /// The actions that the user took since the last call.
    pub fn actions(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Some(event) = self.window.next_event() {
            match event {
                WindowEvent::Clicked(id) => {
                    if let Some(tab) = self.tabs.iter().position(|t| id == t.item) {
                        self.show_tab(tab);
                    } else if id == self.sync {
                        actions.push(Action::SyncNow);
                    } else if id == self.open {
                        actions.push(Action::OpenFolder);
                    } else if id == self.autostart {
                        actions.push(Action::ToggleAutostart);
                    } else if let Some(&(_, unit)) = self.units.iter().find(|(u, _)| id == *u) {
                        self.unit = unit;
                        self.show_unit();
                        actions.extend(self.read_interval());
                    } else if let Some(action) = self.board.action(id) {
                        actions.push(Action::Board(action));
                    } else if let Some(action) = self.inbox.clicked(&mut self.window, id) {
                        actions.push(Action::Inbox(action));
                    } else if let Some(action) = self.channels.clicked(&mut self.window, id) {
                        actions.push(Action::Channels(action));
                    }
                }
                WindowEvent::Submitted(id) => {
                    let action = self.inbox.submitted(&mut self.window, id);
                    actions.extend(action.map(Action::Inbox));
                }
                WindowEvent::Changed(id) if id == self.interval_box => {
                    actions.extend(self.read_interval());
                }
                WindowEvent::CloseRequested => actions.push(Action::Close),
                _ => {}
            }
        }
        actions
    }

    fn show_unit(&mut self) {
        for (item, unit) in self.units {
            self.window.set_selected(item, unit == self.unit);
        }
    }

    /// The interval in the text box, if it is valid. If not, the card says why.
    fn read_interval(&mut self) -> Option<Action> {
        let typed = self.window.text(self.interval_box);
        let digits = Interval::digits(&typed);
        if digits != typed {
            self.window.set_text(self.interval_box, &digits);
        }
        match Interval::parse(&digits, self.unit) {
            Ok(interval) => {
                self.window
                    .set_text(self.interval_title, &interval.describe());
                self.window.set_text(self.interval_note, INTERVAL_HINT);
                Some(Action::SetInterval(interval))
            }
            Err(why) => {
                self.window.set_text(self.interval_note, &why);
                None
            }
        }
    }

    fn show_tab(&mut self, shown: usize) {
        for (i, tab) in self.tabs.iter().enumerate() {
            self.window.set_selected(tab.item, i == shown);
            self.window.set_visible(tab.page, i == shown);
        }
    }
}

/// A page: a title, then its parts from the top. The parts scroll if the window is short.
fn page(w: &mut Window, title: &str, parts: Vec<AnyId>) -> Id<kind::Column> {
    let body = w.add(Scroll::new().gap(12.0).children(parts));
    framed(w, title, body.into())
}

/// A page: a title, then a body that fills the rest of the page.
fn framed(w: &mut Window, title: &str, body: AnyId) -> Id<kind::Column> {
    let title = w.add(Text::new(title).style(TextStyle::Title));
    w.add(
        Column::new()
            .padding(Insets {
                left: 16.0,
                top: 8.0,
                right: 16.0,
                bottom: 16.0,
            })
            .gap(12.0)
            .child(title)
            .child(body),
    )
}

/// Two rings that link, in blue: the icon of the window.
fn app_icon(size: u32) -> glasskit::Result<Icon> {
    let s = size as f32;
    let centers = [(s * 0.36, s * 0.5), (s * 0.64, s * 0.5)];
    let (outer, inner) = (s * 0.30, s * 0.18);
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cover = centers
                .iter()
                .map(|(cx, cy)| {
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    (outer - d + 0.5).clamp(0.0, 1.0) * (d - inner + 0.5).clamp(0.0, 1.0)
                })
                .fold(0.0f32, f32::max);
            let a = (cover * 255.0) as u32;
            pixels.push((a << 24) | (0x2E << 16) | (0x86 << 8) | 0xDE);
        }
    }
    Icon::from_argb(size, pixels)
}

/// A card with a title and a note at the left, and controls at the right.
fn setting(
    w: &mut Window,
    title: Id<kind::Text>,
    note: Id<kind::Text>,
    controls: &[AnyId],
) -> Id<kind::Card> {
    let lines = w.add(
        Column::new()
            .width(Length::Fill(1.0))
            .gap(4.0)
            .child(title)
            .child(note),
    );
    let row = w.add(
        Row::new()
            .gap(8.0)
            .child(lines)
            .children(controls.iter().copied()),
    );
    w.add(Card::new().child(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(received: usize, sent: usize, problems: &[&str]) -> Report {
        Report {
            received,
            sent,
            problems: problems.iter().map(|&p| p.to_owned()).collect(),
        }
    }

    #[test]
    fn icon_has_the_asked_size() {
        assert_eq!(app_icon(ICON_SIZE).unwrap().size(), ICON_SIZE);
    }

    #[test]
    fn summary_counts_what_happened() {
        assert_eq!(
            summary(&report(3, 1, &["a", "b"]), "14:05"),
            "Synced at 14:05: 3 new, 1 sent, 2 problems"
        );
        assert_eq!(
            summary(&report(0, 0, &[]), "09:00"),
            "Synced at 09:00: 0 new"
        );
    }

    #[test]
    fn view_before_the_first_sync() {
        let mut view = View::default();
        assert_eq!(view.headline(), "Starting");
        assert_eq!(view.detail(), "No sync yet");
        assert_eq!(view.counts(), [0, 0, 0]);
        view.syncing = true;
        assert_eq!(view.headline(), "Syncing");
    }

    #[test]
    fn view_after_a_sync() {
        let good = View {
            syncing: false,
            last: Some(("14:05".into(), report(3, 1, &[]))),
        };
        assert_eq!(good.headline(), "Up to date");
        assert_eq!(good.detail(), "Last sync at 14:05");
        assert_eq!(good.counts(), [3, 1, 0]);
        assert_eq!(good.problems(), "");

        let bad = View {
            syncing: false,
            last: Some((
                "14:05".into(),
                report(0, 0, &["chat: offline", "mail: no key"]),
            )),
        };
        assert_eq!(bad.headline(), "2 problems");
        assert_eq!(bad.counts(), [0, 0, 2]);
        assert_eq!(bad.problems(), "chat: offline\nmail: no key");
    }
}
