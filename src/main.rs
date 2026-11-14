// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod board;
mod channels;
mod config;
mod inbox;
mod notice;
mod signin;
mod usage;
mod vault;
mod window;
mod worker;

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result, bail};
use glasskit::events::{Event, EventLoop};
use glasskit::tray::{Icon, MenuItem, Tray};
use nexo_core::{Engine, Folder};

use accounts::{Accounts, Context as Cx};
use board::BoardAction;
use config::Config;
use inbox::InboxAction;
use usage::Usage;
use window::{Action, MainWindow, View};
use worker::{Outcome, Worker};

const APP_NAME: &str = "nexo";
const CONFIG_FILE: &str = "config.toml";
const BOARD_FILE: &str = "board.json";
const USAGE_FILE: &str = "usage.json";

/// A command of the menu of the notification area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Open,
    SyncNow,
}

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
        bail!("nexo is already running. Its icon is in the notification area.");
    };

    let app_dir = app_dir()?;
    let config_path = app_dir.join(CONFIG_FILE);
    let config = config::load_or_create(&config_path)?;
    let mut engine = Engine::new(Folder::open(&config.output)?)?;
    eprintln!("output folder: {}", config.output.display());
    let accounts = Accounts::load(&app_dir)?;
    for source in accounts.sources(&config) {
        engine.add(source)?;
    }

    if once {
        if let Some(telegram) = accounts.telegram_now()? {
            engine.add(Box::new(telegram))?;
        }
        let report = engine.sync();
        println!("{}", window::summary(&report, &now_hhmm()));
        for problem in &report.problems {
            println!("  {problem}");
        }
        return Ok(());
    }
    run_window(engine, config, &config_path, accounts)
}

fn run_window(
    engine: Engine,
    mut config: Config,
    config_path: &Path,
    mut accounts: Accounts,
) -> Result<()> {
    let ui = glasskit::Ui::new()?;
    let mut events: EventLoop<Command> = EventLoop::new(&ui)?;
    let waker = events.waker();
    let browsable: Vec<String> = engine.browsable().map(str::to_owned).collect();
    let repliable: Vec<String> = engine.repliable().map(str::to_owned).collect();
    let mut worker = Worker::spawn(engine, move || waker.wake())?;
    let mut autostart = glasskit::autostart::is_enabled(APP_NAME);
    let board_path = config_path.with_file_name(BOARD_FILE);
    let mut window = MainWindow::new(&ui, &config, &board_path, autostart)?;
    let mut tray = Tray::new(&events, APP_NAME, &window::app_icon(Icon::tray_size())?)?;
    tray.set_menu(&[
        MenuItem::command(Command::Open, "Open nexo"),
        MenuItem::command(Command::SyncNow, "Sync now"),
        MenuItem::Separator,
        MenuItem::Quit("Quit".into()),
    ]);
    tray.set_click(Some(Command::Open));
    window.show();
    let mut usage = Usage::load(&config_path.with_file_name(USAGE_FILE));
    accounts.start(events.waker());
    accounts.show(&mut window, &config);
    if let Some(action) = window.set_channels(&browsable, &repliable) {
        act(action, &mut worker)?;
    }
    let mut view = View {
        total: usage.total,
        since: usage.since(),
        ..View::default()
    };
    let mut shown = None;
    let mut board: Option<Vec<nexo_core::Card>> = None;
    let mut next_sync = SystemTime::now();
    let mut sync_started = Instant::now();
    loop {
        if !worker.is_syncing() && SystemTime::now() >= next_sync {
            worker.sync()?;
            sync_started = Instant::now();
        }
        let cx = Cx {
            worker: &mut worker,
            window: &mut window,
            config: &mut config,
            config_path,
        };
        if accounts.poll(cx)? {
            next_sync = SystemTime::now();
        }
        while let Some(update) = worker.poll()? {
            match update.outcome {
                Outcome::Synced(report) => {
                    if let Err(err) = usage.record(&report) {
                        eprintln!("{err:#}");
                    }
                    let time = now_hhmm();
                    tray.set_tooltip(&format!("nexo - {}", window::summary(&report, &time)));
                    if config.notify && report.received > 0 {
                        let (title, text) =
                            notice::new_messages(report.received, board.as_deref(), &update.board);
                        tray.notify(&title, &text);
                    }
                    view.took = Some(sync_started.elapsed());
                    view.session = usage.session;
                    view.total = usage.total;
                    view.last = Some((time, report));
                    next_sync = SystemTime::now() + config.interval().duration();
                }
                Outcome::Changed => {}
                Outcome::Failed(problem) => glasskit::show_error(APP_NAME, &problem),
                Outcome::Listed { tag, page } => window.show_listed(tag, page),
                Outcome::Opened { id, conversation } => window.show_opened(&id, conversation),
                Outcome::Tracked { id, result } => window.show_tracked(&id, &result),
                Outcome::Replied { id, result } => {
                    if let Some(action) = window.show_replied(&id, &result) {
                        act(action, &mut worker)?;
                    }
                }
            }
            if update.folder_bytes.is_some() {
                view.folder_bytes = update.folder_bytes;
            }
            view.conversations = update.board.len();
            if board.as_ref() != Some(&update.board) {
                window.set_board(&update.board);
                board = Some(update.board);
            }
            if let Some(action) = window.set_channels(&update.browsable, &update.repliable) {
                act(action, &mut worker)?;
            }
        }
        view.syncing = worker.is_syncing();
        if shown.as_ref() != Some(&view) {
            window.set_view(&view);
            shown = Some(view.clone());
        }

        // While a sync runs, its end wakes the loop.
        let deadline = (!worker.is_syncing()).then_some(next_sync);
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
                        Action::SetInterval(interval) if interval != config.interval() => {
                            match config.save_interval(config_path, interval) {
                                Ok(()) => next_sync = SystemTime::now() + interval.duration(),
                                Err(err) => glasskit::show_error(APP_NAME, &format!("{err:#}")),
                            }
                        }
                        Action::SetInterval(_) => {}
                        Action::SetLook(look) => {
                            if let Err(err) = config.save_look(config_path, look) {
                                glasskit::show_error(APP_NAME, &format!("{err:#}"));
                            }
                        }
                        Action::SetSide(side) => {
                            if let Err(err) = config.save_side(config_path, side) {
                                glasskit::show_error(APP_NAME, &format!("{err:#}"));
                            }
                        }
                        Action::SetTray(on) => {
                            if let Err(err) = config.save_tray(config_path, on) {
                                glasskit::show_error(APP_NAME, &format!("{err:#}"));
                            }
                        }
                        Action::SetNotify(on) => {
                            if let Err(err) = config.save_notify(config_path, on) {
                                glasskit::show_error(APP_NAME, &format!("{err:#}"));
                            }
                        }
                        Action::Board(BoardAction::Open(dir)) => open_folder(&dir),
                        Action::Channels(action) => {
                            let cx = Cx {
                                worker: &mut worker,
                                window: &mut window,
                                config: &mut config,
                                config_path,
                            };
                            accounts.act(action, events.waker(), cx)?;
                        }
                        action
                        @ (Action::Board(BoardAction::SetDone { .. }) | Action::Inbox(_)) => {
                            act(action, &mut worker)?;
                        }
                        Action::Close if config.tray => window.hide(),
                        Action::Close => return Ok(()),
                    }
                }
            }
            Event::Command(Command::Open) => window.show(),
            Event::Command(Command::SyncNow) => next_sync = SystemTime::now(),
            Event::Quit => return Ok(()),
            _ => {}
        }
    }
}

/// Do an action that is only a request to the engine.
fn act(action: Action, worker: &mut Worker) -> Result<()> {
    match action {
        Action::Board(BoardAction::SetDone { channel, id, done }) => {
            worker.set_done(channel, id, done)
        }
        Action::Inbox(InboxAction::Browse {
            channel,
            query,
            page,
            tag,
        }) => worker.browse(channel, query, page, tag),
        Action::Inbox(InboxAction::Open { channel, id }) => worker.open(channel, id),
        Action::Inbox(InboxAction::Track { channel, id }) => worker.track(channel, id),
        Action::Inbox(InboxAction::Reply { channel, id, body }) => worker.reply(channel, id, body),
        _ => Ok(()),
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
