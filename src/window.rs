// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use glasskit::Ui;
use glasskit::events::WindowId;
use glasskit::tray::Icon;
use glasskit::widget::{
    Align, AnyId, Button, Card, Column, Id, Insets, Length, NavItem, Row, Scroll, SideBar, Spacer,
    Stack, Text, TextBox, TextStyle, kind,
};
use glasskit::window::{Window, WindowEvent};
use nexo_core::Report;

use crate::board::{Board, BoardAction};
use crate::channels::{Channels, ChannelsAction, EmailState, GmailState, TelegramState};
use crate::config::{Config, Interval, Look, Side, Unit};
use crate::inbox::{Inbox, InboxAction};
use crate::usage::{Counts, size_words};

const SIDE_WIDTH: f32 = 208.0;
const ICON_SIZE: u32 = 64;
const INTERVAL_HINT: &str = "Type a number from 1, and pick seconds or minutes.";
const LOOK_HINT: &str = "How much of the desktop shows through the window.";
const SIDE_HINT: &str = "The list of pages at the left: with names, or only icons.";
const TRAY_HINT: &str = "In the notification area, nexo syncs and tells you about new messages.";
const NOTIFY_HINT: &str = "A notification of Windows when a sync brings new messages.";
/// The pages of the side list, each with its icon in Segoe Fluent Icons.
const PAGES: [(&str, &str); 4] = [
    ("Overview", "\u{E80F}"),
    ("Inbox", "\u{E715}"),
    ("Board", "\u{E8A9}"),
    ("Settings", "\u{E713}"),
];
/// The groups of the settings page.
const GROUPS: [&str; 4] = ["General", "Sync and data", "Appearance", "Channels"];
const COUNTS: [&str; 4] = ["Syncs", "New messages", "Replies sent", "Problems"];

/// What the user asked for in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SyncNow,
    OpenFolder,
    ToggleAutostart,
    SetInterval(Interval),
    SetLook(Look),
    SetSide(Side),
    /// Keep nexo in the notification area when the window closes, or stop it.
    SetTray(bool),
    SetNotify(bool),
    Board(BoardAction),
    Inbox(InboxAction),
    Channels(ChannelsAction),
    Close,
}

/// What the overview page and the sync group of the settings show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub syncing: bool,
    /// The clock time of the last sync, such as `14:05`, and its report.
    pub last: Option<(String, Report)>,
    /// How long the last sync took.
    pub took: Option<Duration>,
    /// The syncs of this run of nexo, and of all runs.
    pub session: Counts,
    pub total: Counts,
    /// The day of the first sync of all runs, such as `2026-11-14`.
    pub since: String,
    /// The number of conversations in the output folder.
    pub conversations: usize,
    /// The size of the output folder.
    pub folder_bytes: Option<u64>,
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

    /// The time of the last sync, and how long it took.
    fn timing(&self) -> String {
        match self.took {
            Some(took) if self.last.is_some() => {
                format!("{}, in {:.1} s", self.detail(), took.as_secs_f64())
            }
            _ => self.detail(),
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

    fn total_title(&self) -> String {
        format!("All syncs since {}", self.since)
    }

    /// What the output folder holds.
    fn stored(&self) -> String {
        let conversations = match self.conversations {
            1 => "1 conversation".to_owned(),
            n => format!("{n} conversations"),
        };
        match self.folder_bytes {
            Some(bytes) => format!("{conversations}, {} on the disk", size_words(bytes)),
            None => format!("{conversations}. The size shows after the first sync."),
        }
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

/// An item of a list of pages, and the page that it shows.
struct Tab {
    item: Id<kind::NavItem>,
    page: AnyId,
}

pub struct MainWindow {
    window: Window,
    ui: Ui,
    tabs: Vec<Tab>,
    /// The groups of the settings page.
    groups: Vec<Tab>,
    /// The state of the sync, on the overview page and in the settings.
    headlines: [Id<kind::Text>; 2],
    detail: Id<kind::Text>,
    timing: Id<kind::Text>,
    counts: [Id<kind::Text>; 3],
    session: [Id<kind::Text>; 4],
    total_title: Id<kind::Text>,
    total: [Id<kind::Text>; 4],
    stored: Id<kind::Text>,
    problems_card: Id<kind::Card>,
    problems: Id<kind::Text>,
    syncs: [Id<kind::Button>; 2],
    open: Id<kind::Button>,
    autostart: Id<kind::Button>,
    autostart_state: Id<kind::Text>,
    interval_title: Id<kind::Text>,
    interval_note: Id<kind::Text>,
    interval_box: Id<kind::TextBox>,
    units: [(Id<kind::NavItem>, Unit); 2],
    unit: Unit,
    looks: [(Id<kind::NavItem>, Look); 3],
    look: Look,
    sides: [(Id<kind::NavItem>, Side); 2],
    side: Side,
    trays: [(Id<kind::NavItem>, bool); 2],
    notifies: [(Id<kind::NavItem>, bool); 2],
    bar: Id<kind::SideBar>,
    board: Board,
    inbox: Inbox,
    channels: Channels,
}

impl MainWindow {
    pub fn new(ui: &Ui, config: &Config, board: &Path, autostart: bool) -> Result<Self> {
        let mut w = Window::new(ui, "nexo", 1000.0, 680.0)?;
        w.set_min_size(760.0, 660.0);

        let headline = w.add(Text::new("Starting").style(TextStyle::Subtitle));
        let detail = w.add(Text::new("No sync yet").style(TextStyle::Caption));
        let sync = w.add(Button::new("Sync now").accent());
        let status = setting(&mut w, headline, detail, &[sync.into()]);
        let (last_stats, counts) = stats(&mut w, ["New messages", "Replies sent", "Problems"]);
        let problems_title = w.add(Text::new("Problems").style(TextStyle::BodyStrong));
        let problems = w.add(Text::new(""));
        let lines = w.add(Column::new().gap(8.0).child(problems_title).child(problems));
        let problems_card = w.add(Card::new().child(lines));
        w.set_visible(problems_card, false);
        let overview = page(
            &mut w,
            "Overview",
            vec![status.into(), last_stats.into(), problems_card.into()],
        );

        let (board, lanes) = Board::build(&mut w, board)?;
        // Each lane scrolls on its own, so the page does not.
        let board_page = framed(&mut w, "Board", lanes.into());

        let (inbox, list) = Inbox::build(&mut w);
        let inbox_page = framed(&mut w, "Inbox", list.into());

        let folder_title = w.add(Text::new("Output folder"));
        let folder =
            w.add(Text::new(config.output.display().to_string()).style(TextStyle::Caption));
        let open = w.add(Button::new("Open folder"));
        let folder = setting(&mut w, folder_title, folder, &[open.into()]);
        let autostart_title = w.add(Text::new("Start with Windows"));
        let autostart_state = w.add(Text::new("").style(TextStyle::Caption));
        let autostart_button = w.add(Button::new(""));
        let startup = setting(
            &mut w,
            autostart_title,
            autostart_state,
            &[autostart_button.into()],
        );
        let (tray, trays) = choice(
            &mut w,
            "When you close the window",
            TRAY_HINT,
            [("Keep in tray", true), ("Exit", false)],
        );
        let (notify, notifies) = choice(
            &mut w,
            "Notifications",
            NOTIFY_HINT,
            [("On", true), ("Off", false)],
        );
        let how = w.add(
            Text::new(
                "To change the folder, edit config.toml next to the app, then start nexo again.",
            )
            .style(TextStyle::Caption),
        );
        let general = scroll(
            &mut w,
            vec![
                folder.into(),
                startup.into(),
                tray.into(),
                notify.into(),
                how.into(),
            ],
        );

        let sync_headline = w.add(Text::new("Starting"));
        let timing = w.add(Text::new("No sync yet").style(TextStyle::Caption));
        let sync_again = w.add(Button::new("Sync now").accent());
        let sync_status = setting(&mut w, sync_headline, timing, &[sync_again.into()]);
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
        let session_title = w.add(Text::new("This session").style(TextStyle::BodyStrong));
        let (session_stats, session) = stats(&mut w, COUNTS);
        let total_title = w.add(Text::new("All syncs").style(TextStyle::BodyStrong));
        let (total_stats, total) = stats(&mut w, COUNTS);
        let stored_title = w.add(Text::new("Stored data"));
        let stored = w.add(Text::new("").style(TextStyle::Caption));
        let stored_card = setting(&mut w, stored_title, stored, &[]);
        let sync_group = scroll(
            &mut w,
            vec![
                sync_status.into(),
                schedule.into(),
                session_title.into(),
                session_stats.into(),
                total_title.into(),
                total_stats.into(),
                stored_card.into(),
            ],
        );

        let (look, looks) = choice(
            &mut w,
            "Window background",
            LOOK_HINT,
            Look::ALL.map(|look| (look.label(), look)),
        );
        w.set_material(config.look.material());
        let (side_setting, sides) = choice(
            &mut w,
            "Side list",
            SIDE_HINT,
            Side::ALL.map(|side| (side.label(), side)),
        );
        let appearance = scroll(&mut w, vec![look.into(), side_setting.into()]);

        let (channels, cards) = Channels::build(&mut w);
        let channels_group = scroll(&mut w, cards);

        let group_items = GROUPS.map(|name| w.add(NavItem::new(name)));
        let group_row = w.add(Row::new().gap(4.0).children(group_items));
        let group_pages = [general, sync_group, appearance, channels_group];
        let group_stack = w.add(Stack::new().height(Length::Fill(1.0)).children(group_pages));
        let groups = w.add(
            Column::new()
                .gap(12.0)
                .height(Length::Fill(1.0))
                .child(group_row)
                .child(group_stack),
        );
        let settings = framed(&mut w, "Settings", groups.into());

        let items = PAGES.map(|(name, icon)| w.add(NavItem::new(name).icon(icon)));
        let gap = w.add(Spacer::fill());
        let bar = w.add(
            SideBar::new()
                .title("nexo")
                .expanded_width(SIDE_WIDTH)
                .mode(config.side.mode())
                .modes(Side::ALL.map(Side::mode))
                .gap(4.0)
                .children(items[..3].iter().copied())
                .child(gap)
                .child(items[3]),
        );
        let pages = [overview, inbox_page, board_page, settings];
        let stack = w.add(Stack::new().width(Length::Fill(1.0)).children(pages));
        let root = w.add(
            Row::new()
                .padding(Insets::all(8.0))
                .gap(8.0)
                .align(Align::Stretch)
                .child(bar)
                .child(stack),
        );
        w.set_root(root);

        let tab = |(item, page): (Id<kind::NavItem>, AnyId)| Tab { item, page };
        let mut this = Self {
            window: w,
            ui: ui.clone(),
            tabs: items
                .into_iter()
                .zip(pages.map(AnyId::from))
                .map(tab)
                .collect(),
            groups: group_items
                .into_iter()
                .zip(group_pages.map(AnyId::from))
                .map(tab)
                .collect(),
            headlines: [headline, sync_headline],
            detail,
            timing,
            counts,
            session,
            total_title,
            total,
            stored,
            problems_card,
            problems,
            syncs: [sync, sync_again],
            open,
            autostart: autostart_button,
            autostart_state,
            interval_title,
            interval_note,
            interval_box,
            units,
            unit: interval.unit,
            looks,
            look: config.look,
            sides,
            side: config.side,
            trays,
            notifies,
            bar,
            board,
            inbox,
            channels,
        };
        this.show_unit();
        this.show_look();
        this.show_side();
        this.set_tray(config.tray);
        this.set_notify(config.notify);
        this.show_tab(0);
        this.show_group(0);
        this.set_autostart(autostart);
        this.window.set_icon(&app_icon(ICON_SIZE)?)?;
        Ok(this)
    }

    /// Show the window, and bring it to the front.
    pub fn show(&self) {
        self.window.show();
    }

    /// Hide the window. nexo goes on in the notification area.
    pub fn hide(&self) {
        self.window.hide();
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn set_view(&mut self, view: &View) {
        for headline in self.headlines {
            self.window.set_text(headline, &view.headline());
        }
        self.window.set_text(self.detail, &view.detail());
        self.window.set_text(self.timing, &view.timing());
        for (id, count) in self.counts.into_iter().zip(view.counts()) {
            self.window.set_text(id, &count.to_string());
        }
        for (ids, counts) in [(self.session, view.session), (self.total, view.total)] {
            for (id, count) in ids.into_iter().zip(counts.values()) {
                self.window.set_text(id, &count.to_string());
            }
        }
        self.window.set_text(self.total_title, &view.total_title());
        self.window.set_text(self.stored, &view.stored());
        let problems = view.problems();
        self.window.set_text(self.problems, &problems);
        self.window
            .set_visible(self.problems_card, !problems.is_empty());
        for sync in self.syncs {
            self.window.set_enabled(sync, !view.syncing);
        }
    }

    /// Show these conversations on the board.
    pub fn set_board(&mut self, cards: &[nexo_core::Card]) {
        self.board.set(&mut self.window, cards);
    }

    /// Tell the inbox which channels have a list of conversations, and which can send
    /// replies. The action, if there is one, loads the first page of a channel that is new.
    pub fn set_channels(&mut self, browsable: &[String], repliable: &[String]) -> Option<Action> {
        self.inbox
            .set_channels(&mut self.window, browsable, repliable)
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

    /// Show in the inbox that a reply went out, or did not. The action, if there is one,
    /// loads the conversation again.
    pub fn show_replied(&mut self, id: &str, result: &Result<(), String>) -> Option<Action> {
        self.inbox
            .replied(&mut self.window, id, result)
            .map(Action::Inbox)
    }

    pub fn set_gmail(&mut self, state: &GmailState) {
        self.channels.set_gmail(&mut self.window, state);
    }

    pub fn set_linkedin(&mut self, on: bool, gmail: bool) {
        self.channels.set_linkedin(&mut self.window, on, gmail);
    }

    pub fn set_email(&mut self, state: &EmailState) {
        self.channels.set_email(&mut self.window, state);
    }

    pub fn set_telegram(&mut self, state: &TelegramState) {
        self.channels.set_telegram(&mut self.window, state);
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

    pub fn set_tray(&mut self, on: bool) {
        for (item, value) in self.trays {
            self.window.set_selected(item, value == on);
        }
    }

    pub fn set_notify(&mut self, on: bool) {
        for (item, value) in self.notifies {
            self.window.set_selected(item, value == on);
        }
    }

    /// The actions that the user took since the last call.
    pub fn actions(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Some(event) = self.window.next_event() {
            match self.board.handle(&self.ui, &mut self.window, &event) {
                Ok(action) => actions.extend(action.map(Action::Board)),
                Err(err) => glasskit::show_error("nexo", &format!("{err:#}")),
            }
            match event {
                WindowEvent::Clicked(id) => {
                    if let Err(err) = self.board.clicked(&self.ui, &mut self.window, id) {
                        glasskit::show_error("nexo", &format!("{err:#}"));
                    }
                    if let Some(tab) = self.tabs.iter().position(|t| id == t.item) {
                        self.show_tab(tab);
                    } else if let Some(group) = self.groups.iter().position(|t| id == t.item) {
                        self.show_group(group);
                    } else if let Some(&(_, on)) = self.trays.iter().find(|(t, _)| id == *t) {
                        self.set_tray(on);
                        actions.push(Action::SetTray(on));
                    } else if let Some(&(_, on)) = self.notifies.iter().find(|(n, _)| id == *n) {
                        self.set_notify(on);
                        actions.push(Action::SetNotify(on));
                    } else if let Some(&(_, side)) = self.sides.iter().find(|(s, _)| id == *s) {
                        self.side = side;
                        self.show_side();
                        actions.push(Action::SetSide(side));
                    } else if self.syncs.iter().any(|sync| id == *sync) {
                        actions.push(Action::SyncNow);
                    } else if id == self.open {
                        actions.push(Action::OpenFolder);
                    } else if id == self.autostart {
                        actions.push(Action::ToggleAutostart);
                    } else if let Some(&(_, unit)) = self.units.iter().find(|(u, _)| id == *u) {
                        self.unit = unit;
                        self.show_unit();
                        actions.extend(self.read_interval());
                    } else if let Some(&(_, look)) = self.looks.iter().find(|(l, _)| id == *l) {
                        self.look = look;
                        self.show_look();
                        actions.push(Action::SetLook(look));
                    } else if let Some(action) = self.inbox.clicked(&mut self.window, id) {
                        actions.push(Action::Inbox(action));
                    } else if let Some(action) = self.channels.clicked(&mut self.window, id) {
                        actions.push(Action::Channels(action));
                    }
                }
                WindowEvent::Submitted(id) => {
                    let action = self.inbox.submitted(&mut self.window, id);
                    actions.extend(action.map(Action::Inbox));
                    let action = self.channels.submitted(&mut self.window, id);
                    actions.extend(action.map(Action::Channels));
                }
                WindowEvent::Changed(id) if id == self.interval_box => {
                    actions.extend(self.read_interval());
                }
                WindowEvent::SideModeChanged(id) if id == self.bar => {
                    self.side = Side::of(self.window.side_mode(self.bar));
                    self.show_side();
                    actions.push(Action::SetSide(self.side));
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

    fn show_look(&mut self) {
        for (item, look) in self.looks {
            self.window.set_selected(item, look == self.look);
        }
        self.window.set_material(self.look.material());
    }

    fn show_side(&mut self) {
        for (item, side) in self.sides {
            self.window.set_selected(item, side == self.side);
        }
        self.window.set_side_mode(self.bar, self.side.mode());
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
        show_one(&mut self.window, &self.tabs, shown);
    }

    fn show_group(&mut self, shown: usize) {
        show_one(&mut self.window, &self.groups, shown);
    }
}

/// Show the page of one tab, and mark its item.
fn show_one(w: &mut Window, tabs: &[Tab], shown: usize) {
    for (i, tab) in tabs.iter().enumerate() {
        w.set_selected(tab.item, i == shown);
        w.set_visible(tab.page, i == shown);
    }
}

/// A page: a title, then its parts from the top. The parts scroll if the window is short.
fn page(w: &mut Window, title: &str, parts: Vec<AnyId>) -> Id<kind::Column> {
    let body = scroll(w, parts);
    framed(w, title, body.into())
}

/// Parts from the top, which scroll if the window is short.
fn scroll(w: &mut Window, parts: Vec<AnyId>) -> Id<kind::Scroll> {
    // Space at the right keeps the parts clear of the scroll bar.
    let clear = Insets {
        left: 0.0,
        top: 0.0,
        right: 10.0,
        bottom: 0.0,
    };
    w.add(Scroll::new().gap(12.0).padding(clear).children(parts))
}

/// A row of cards, each with a number and its label, and the texts of the numbers.
fn stats<const N: usize>(
    w: &mut Window,
    labels: [&str; N],
) -> (Id<kind::Row>, [Id<kind::Text>; N]) {
    let counts = labels.map(|_| w.add(Text::new("0").style(TextStyle::Title)));
    let cards: Vec<Id<kind::Card>> = labels
        .into_iter()
        .zip(counts)
        .map(|(label, count)| {
            let label = w.add(Text::new(label).style(TextStyle::Caption));
            let lines = w.add(Column::new().gap(2.0).child(count).child(label));
            w.add(Card::new().width(Length::Fill(1.0)).child(lines))
        })
        .collect();
    let row = w.add(Row::new().gap(12.0).align(Align::Stretch).children(cards));
    (row, counts)
}

/// A setting with one item for each value, of which one is selected.
fn choice<T: Copy, const N: usize>(
    w: &mut Window,
    title: &str,
    note: &str,
    values: [(&str, T); N],
) -> (Id<kind::Card>, [(Id<kind::NavItem>, T); N]) {
    let title = w.add(Text::new(title));
    let note = w.add(Text::new(note).style(TextStyle::Caption));
    let items = values.map(|(label, value)| (w.add(NavItem::new(label)), value));
    let card = setting(w, title, note, &items.map(|(item, _)| item.into()));
    (card, items)
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

/// Two rings that link, in blue: the icon of the window and of the notification area.
pub fn app_icon(size: u32) -> glasskit::Result<Icon> {
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
        assert_eq!(view.timing(), "No sync yet");
        assert_eq!(
            view.stored(),
            "0 conversations. The size shows after the first sync."
        );
        assert_eq!(view.counts(), [0, 0, 0]);
        view.syncing = true;
        assert_eq!(view.headline(), "Syncing");
    }

    #[test]
    fn view_after_a_sync() {
        let good = View {
            last: Some(("14:05".into(), report(3, 1, &[]))),
            took: Some(Duration::from_millis(1240)),
            since: "2026-11-14".into(),
            conversations: 1,
            folder_bytes: Some(2048),
            ..View::default()
        };
        assert_eq!(good.timing(), "Last sync at 14:05, in 1.2 s");
        assert_eq!(good.total_title(), "All syncs since 2026-11-14");
        assert_eq!(good.stored(), "1 conversation, 2.0 KB on the disk");
        assert_eq!(good.headline(), "Up to date");
        assert_eq!(good.detail(), "Last sync at 14:05");
        assert_eq!(good.counts(), [3, 1, 0]);
        assert_eq!(good.problems(), "");

        let bad = View {
            last: Some((
                "14:05".into(),
                report(0, 0, &["chat: offline", "mail: no key"]),
            )),
            ..View::default()
        };
        assert_eq!(bad.headline(), "2 problems");
        assert_eq!(bad.counts(), [0, 0, 2]);
        assert_eq!(bad.problems(), "chat: offline\nmail: no key");
    }
}
