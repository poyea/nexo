// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod window;
mod worker;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use glasskit::events::{Event, EventLoop};
use nexo_core::{Engine, Folder};

use config::Config;
use window::{Action, MainWindow, View};
use worker::Worker;

const APP_NAME: &str = "nexo";
const CONFIG_FILE: &str = "config.toml";

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
        println!("{}", window::summary(&report, &now_hhmm()));
        for problem in &report.problems {
            println!("  {problem}");
        }
        return Ok(());
    }
    run_window(engine, &config)
}

fn run_window(engine: Engine, config: &Config) -> Result<()> {
    let ui = glasskit::Ui::new()?;
    let mut events: EventLoop<()> = EventLoop::new(&ui)?;
    let waker = events.waker();
    let mut worker = Worker::spawn(engine, move || waker.wake())?;
    let mut autostart = glasskit::autostart::is_enabled(APP_NAME);
    let mut window = MainWindow::new(&ui, config, autostart)?;
    let mut view = View::default();
    let mut shown = None;
    let mut next_sync = SystemTime::now();
    loop {
        if !worker.is_busy() && SystemTime::now() >= next_sync {
            worker.start()?;
        }
        if let Some(report) = worker.poll()? {
            view.last = Some((now_hhmm(), report));
            next_sync = SystemTime::now() + config.sync_every();
        }
        view.syncing = worker.is_busy();
        if shown.as_ref() != Some(&view) {
            window.set_view(&view);
            shown = Some(view.clone());
        }

        // While a sync runs, its end wakes the loop.
        let deadline = (!worker.is_busy()).then_some(next_sync);
        match events.next(deadline)? {
            Event::Window(id) if id == window.id() => {
                for action in window.actions() {
                    match action {
                        Action::SyncNow => next_sync = SystemTime::now(),
                        Action::OpenFolder => open_folder(&config.output),
                        Action::ToggleAutostart => {
                            match glasskit::autostart::set_enabled(APP_NAME, !autostart) {
                                Ok(()) => autostart = !autostart,
                                Err(err) => glasskit::show_error(
                                    APP_NAME,
                                    &format!("could not change Start with Windows: {err}"),
                                ),
                            }
                            window.set_autostart(autostart);
                        }
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
