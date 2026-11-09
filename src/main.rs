// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod tray;
mod worker;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use glasskit::events::{Event, EventLoop};
use nexo_core::{Engine, Folder};

use tray::{Command, MenuState, Tray};
use worker::Worker;

const APP_NAME: &str = "nexo";
const CONFIG_FILE: &str = "config.toml";
/// How often the loop looks for the end of a sync that runs.
const POLL_EVERY: Duration = Duration::from_millis(250);

const USAGE: &str = "\
Usage: nexo [--once]

  (no option)  Run in the system tray and sync on a schedule.
  --once       Sync one time, print what happened, then exit.
  --help       Show this help.";

fn main() {
    if let Err(err) = run() {
        let message = format!("{err:#}");
        eprintln!("error: {message}");
        glasskit::show_error(APP_NAME, &message);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let once = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--once") => true,
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown option {other:?}\n\n{USAGE}"),
    };

    let Some(_instance) = glasskit::single_instance("nexo.SingleInstance")? else {
        bail!("nexo is already running. Use its tray icon, or quit it first.");
    };

    let config = config::load_or_create(&app_dir()?.join(CONFIG_FILE))?;
    let engine = Engine::new(Folder::open(&config.output)?)?;
    eprintln!("output folder: {}", config.output.display());

    if once {
        let mut engine = engine;
        let report = engine.sync();
        println!("{}", tray::status_of(&report, &now_hhmm()));
        for problem in &report.problems {
            println!("  {problem}");
        }
        return Ok(());
    }
    run_tray(Worker::spawn(engine)?, &config.output, config.sync_every())
}

fn run_tray(mut worker: Worker, output: &Path, every: Duration) -> Result<()> {
    let ui = glasskit::Ui::new()?;
    let mut events = EventLoop::new(&ui)?;
    let mut tray = Tray::new(&events)?;
    let mut state = MenuState {
        status: "Starting".into(),
        autostart: glasskit::autostart::is_enabled(APP_NAME),
        ..MenuState::default()
    };
    let mut next_sync = SystemTime::now();
    loop {
        if !worker.is_busy() && SystemTime::now() >= next_sync {
            worker.start()?;
            state.status = "Syncing".into();
        }
        if let Some(report) = worker.poll()? {
            state.status = tray::status_of(&report, &now_hhmm());
            state.problems = report.problems;
            next_sync = SystemTime::now() + every;
        }
        state.syncing = worker.is_busy();
        tray.set_menu(state.clone());

        let wake = if worker.is_busy() {
            SystemTime::now() + POLL_EVERY
        } else {
            next_sync
        };
        match events.next(Some(wake))? {
            Event::Command(Command::SyncNow) => next_sync = SystemTime::now(),
            Event::Command(Command::OpenFolder) => open_folder(output),
            Event::Command(Command::ToggleAutostart) => {
                let on = !state.autostart;
                glasskit::autostart::set_enabled(APP_NAME, on)
                    .context("could not change Start with Windows")?;
                state.autostart = on;
            }
            Event::Quit => return Ok(()),
            _ => {}
        }
    }
}

fn open_folder(path: &Path) {
    if let Err(err) = std::process::Command::new("explorer").arg(path).spawn() {
        eprintln!("could not open {}: {err}", path.display());
    }
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn app_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the executable")?;
    exe.parent()
        .map(PathBuf::from)
        .context("executable has no parent directory")
}
