// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use anyhow::Result;
use glasskit::events::EventLoop;
use glasskit::tray::{self, Icon, MenuItem};
use nexo_core::Report;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    SyncNow,
    OpenFolder,
    ToggleAutostart,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuState {
    pub status: String,
    pub problems: Vec<String>,
    pub syncing: bool,
    pub autostart: bool,
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

pub struct Tray {
    inner: tray::Tray<Command>,
    state: Option<MenuState>,
}

impl Tray {
    pub fn new(events: &EventLoop<Command>) -> Result<Self> {
        Ok(Self {
            inner: tray::Tray::new(events, "nexo", &app_icon(Icon::tray_size())?)?,
            state: None,
        })
    }

    /// Show this state. The menu is made again only when the state changes.
    pub fn set_menu(&mut self, state: MenuState) {
        if self.state.as_ref() == Some(&state) {
            return;
        }
        self.inner.set_tooltip(&format!("nexo - {}", state.status));
        self.inner.set_menu(&menu_items(&state));
        self.state = Some(state);
    }
}

fn menu_items(state: &MenuState) -> Vec<MenuItem<Command>> {
    let mut items = vec![MenuItem::Label(state.status.clone())];
    if !state.problems.is_empty() {
        items.push(MenuItem::Submenu {
            label: "Problems".into(),
            items: state
                .problems
                .iter()
                .map(|p| MenuItem::Label(p.clone()))
                .collect(),
        });
    }
    items.extend([
        MenuItem::Separator,
        MenuItem::command(Command::SyncNow, "Sync now").enabled(!state.syncing),
        MenuItem::command(Command::OpenFolder, "Open folder"),
        MenuItem::command(Command::ToggleAutostart, "Start with Windows").checked(state.autostart),
        MenuItem::Separator,
        MenuItem::Quit("Quit".into()),
    ]);
    items
}

/// Two rings that link, in blue.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn command(items: &[MenuItem<Command>], wanted: Command) -> Option<&MenuItem<Command>> {
        items
            .iter()
            .find(|item| matches!(item, MenuItem::Command { command, .. } if *command == wanted))
    }

    #[test]
    fn sync_now_is_off_while_a_sync_runs() {
        let syncing = MenuState {
            syncing: true,
            ..Default::default()
        };
        assert!(matches!(
            command(&menu_items(&syncing), Command::SyncNow),
            Some(MenuItem::Command { enabled: false, .. })
        ));
    }

    #[test]
    fn problems_show_only_when_there_are_some() {
        let has_problems = |state: &MenuState| {
            menu_items(state)
                .iter()
                .any(|i| matches!(i, MenuItem::Submenu { label, .. } if label == "Problems"))
        };
        assert!(!has_problems(&MenuState::default()));
        assert!(has_problems(&MenuState {
            problems: vec!["chat: offline".into()],
            ..Default::default()
        }));
    }

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

    #[test]
    fn icon_has_the_asked_size() {
        assert_eq!(app_icon(20).unwrap().size(), 20);
    }
}
