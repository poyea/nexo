// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod window;
mod worker;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use glasskit::events::{Event, EventLoop};
use nexo_core::{Engine, Folder};

use window::{Action, MainWindow};
use worker::Worker;

const APP_NAME: &str = "nexo";
const CONFIG_FILE: &str = "config.toml";
/// How often the loop looks for the end of a sync that runs.
const POLL_EVERY: Duration = Duration::from_millis(250);

const USAGE: &str = "\
Usage: nexo [--once]

  (no option)  Open the window and sync on a schedule.
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
        bail!("nexo is already running.");
    };

    let config = config::load_or_create(&app_dir()?.join(CONFIG_FILE))?;
    let engine = Engine::new(Folder::open(&config.output)?)?;
    eprintln!("output folder: {}", config.output.display());

    if once {
        let mut engine = engine;
        let report = engine.sync();
        println!("{}", window::status_of(&report, &now_hhmm()));
        for problem in &report.problems {
            println!("  {problem}");
        }
        return Ok(());
    }
    run_window(Worker::spawn(engine)?, &config.output, config.sync_every())
}

fn run_window(mut worker: Worker, output: &Path, every: Duration) -> Result<()> {
    let ui = glasskit::Ui::new()?;
    let mut events: EventLoop<()> = EventLoop::new(&ui)?;
    let mut window = MainWindow::new(&ui, output)?;
    let mut status = String::from("Starting");
    let mut problems = Vec::new();
    let mut shown = None;
    let mut next_sync = SystemTime::now();
    loop {
        if !worker.is_busy() && SystemTime::now() >= next_sync {
            worker.start()?;
            status = "Syncing".into();
        }
        if let Some(report) = worker.poll()? {
            status = window::status_of(&report, &now_hhmm());
            problems = report.problems;
            next_sync = SystemTime::now() + every;
        }
        let state = (status.clone(), problems.clone(), worker.is_busy());
        if shown.as_ref() != Some(&state) {
            window.set_status(&state.0, &state.1, state.2);
            shown = Some(state);
        }

        let wake = if worker.is_busy() {
            SystemTime::now() + POLL_EVERY
        } else {
            next_sync
        };
        match events.next(Some(wake))? {
            Event::Window(id) if id == window.id() => {
                for action in window.actions() {
                    match action {
                        Action::SyncNow => next_sync = SystemTime::now(),
                        Action::OpenFolder => open_folder(output),
                        Action::Close => return Ok(()),
                    }
                }
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
