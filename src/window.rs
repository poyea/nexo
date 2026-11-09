// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::Path;

use anyhow::Result;
use glasskit::Ui;
use glasskit::events::WindowId;
use glasskit::widget::{Button, Card, Column, Id, Insets, Row, Spacer, Text, TextStyle, kind};
use glasskit::window::{Window, WindowEvent};
use nexo_core::Report;

/// What the user asked for in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SyncNow,
    OpenFolder,
    Close,
}

/// The status line after a sync that ended at `time`, such as `14:05`.
pub fn status_of(report: &Report, time: &str) -> String {
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

pub struct MainWindow {
    window: Window,
    status: Id<kind::Text>,
    problems: Id<kind::Text>,
    sync: Id<kind::Button>,
    open: Id<kind::Button>,
}

impl MainWindow {
    pub fn new(ui: &Ui, output: &Path) -> Result<Self> {
        let mut window = Window::new(ui, "nexo", 520.0, 320.0)?;
        window.set_min_size(400.0, 280.0);
        let title = window.add(Text::new("nexo").style(TextStyle::Title));
        let status = window.add(Text::new("Starting").style(TextStyle::BodyStrong));
        let problems = window.add(Text::new(""));
        let lines = window.add(Column::new().gap(8.0).child(status).child(problems));
        let card = window.add(Card::new().child(lines));
        let folder = window
            .add(Text::new(format!("Folder: {}", output.display())).style(TextStyle::Caption));
        let fill = window.add(Spacer::fill());
        let gap = window.add(Spacer::fill());
        let open = window.add(Button::new("Open folder"));
        let sync = window.add(Button::new("Sync now").accent());
        let buttons = window.add(Row::new().gap(8.0).child(gap).child(open).child(sync));
        let root = window.add(
            Column::new()
                .padding(Insets::all(24.0))
                .gap(12.0)
                .child(title)
                .child(card)
                .child(folder)
                .child(fill)
                .child(buttons),
        );
        window.set_root(root);
        window.show();
        Ok(Self {
            window,
            status,
            problems,
            sync,
            open,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn set_status(&mut self, status: &str, problems: &[String], syncing: bool) {
        self.window.set_text(self.status, status);
        self.window.set_text(self.problems, &problems.join("\n"));
        self.window.set_enabled(self.sync, !syncing);
    }

    /// The actions that the user took since the last call.
    pub fn actions(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Some(event) = self.window.next_event() {
            match event {
                WindowEvent::Clicked(id) if id == self.sync => actions.push(Action::SyncNow),
                WindowEvent::Clicked(id) if id == self.open => actions.push(Action::OpenFolder),
                WindowEvent::CloseRequested => actions.push(Action::Close),
                _ => {}
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_counts_what_happened() {
        let report = Report {
            received: 3,
            sent: 1,
            problems: vec!["a".into(), "b".into()],
        };
        assert_eq!(
            status_of(&report, "14:05"),
            "Synced at 14:05: 3 new, 1 sent, 2 problems"
        );
        assert_eq!(
            status_of(&Report::default(), "09:00"),
            "Synced at 09:00: 0 new"
        );
    }
}
