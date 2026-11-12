// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod board;
mod channels;
mod config;
mod inbox;
mod signin;
mod vault;
mod window;
mod worker;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use glasskit::events::{Event, EventLoop};
use nexo_core::{Engine, Folder};
use nexo_gmail::{Credentials, Gmail};

use board::BoardAction;
use channels::{ChannelsAction, GmailState};
use config::Config;
use inbox::InboxAction;
use signin::{SignIn, SignedIn};
use window::{Action, MainWindow, View};
use worker::{Outcome, Worker};

const APP_NAME: &str = "nexo";
const CONFIG_FILE: &str = "config.toml";
const GOOGLE_CLOUD: &str = "https://console.cloud.google.com/apis/credentials";

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

    let config_path = app_dir()?.join(CONFIG_FILE);
    let config = config::load_or_create(&config_path)?;
    let mut engine = Engine::new(Folder::open(&config.output)?)?;
    eprintln!("output folder: {}", config.output.display());
    vault::open()?;
    let gmail: Option<Credentials> = vault::load(nexo_gmail::CHANNEL)?;
    if let Some(credentials) = &gmail {
        engine.add(Box::new(Gmail::new(credentials.clone())))?;
    }

    if once {
        let report = engine.sync();
        println!("{}", window::summary(&report, &now_hhmm()));
        for problem in &report.problems {
            println!("  {problem}");
        }
        return Ok(());
    }
    let gmail = gmail.map_or(GmailState::Off, |c| GmailState::On(c.email));
    run_window(engine, config, &config_path, gmail)
}

fn run_window(
    engine: Engine,
    mut config: Config,
    config_path: &Path,
    gmail: GmailState,
) -> Result<()> {
    let ui = glasskit::Ui::new()?;
    let mut events: EventLoop<()> = EventLoop::new(&ui)?;
    let waker = events.waker();
    let browsable: Vec<String> = engine.browsable().map(str::to_owned).collect();
    let mut worker = Worker::spawn(engine, move || waker.wake())?;
    let mut autostart = glasskit::autostart::is_enabled(APP_NAME);
    let mut window = MainWindow::new(&ui, &config, autostart)?;
    window.set_gmail(&gmail);
    if let Some(action) = window.set_browsable(&browsable) {
        act(action, &mut worker)?;
    }
    let mut sign_in: Option<SignIn> = None;
    let mut view = View::default();
    let mut shown = None;
    let mut board = None;
    let mut next_sync = SystemTime::now();
    loop {
        if !worker.is_syncing() && SystemTime::now() >= next_sync {
            worker.sync()?;
        }
        if let Some(ended) = sign_in.as_ref().and_then(SignIn::poll) {
            sign_in = None;
            let state = match ended {
                SignedIn::Yes(credentials) => {
                    match vault::save(nexo_gmail::CHANNEL, &credentials) {
                        Ok(()) => {
                            let email = credentials.email.clone();
                            worker.connect(Box::new(Gmail::new(credentials)))?;
                            next_sync = SystemTime::now();
                            GmailState::On(email)
                        }
                        Err(err) => GmailState::Failed(format!("{err:#}")),
                    }
                }
                SignedIn::Stopped => GmailState::Off,
                SignedIn::Failed(why) => GmailState::Failed(why),
            };
            window.set_gmail(&state);
        }
        while let Some(update) = worker.poll()? {
            match update.outcome {
                Outcome::Synced(report) => {
                    view.last = Some((now_hhmm(), report));
                    next_sync = SystemTime::now() + config.interval().duration();
                }
                Outcome::Changed => {}
                Outcome::Failed(problem) => glasskit::show_error(APP_NAME, &problem),
                Outcome::Listed { tag, page } => window.show_listed(tag, page),
                Outcome::Opened { id, conversation } => window.show_opened(&id, conversation),
                Outcome::Tracked { id, result } => window.show_tracked(&id, &result),
            }
            if board.as_ref() != Some(&update.board) {
                window.set_board(&update.board);
                board = Some(update.board);
            }
            if let Some(action) = window.set_browsable(&update.browsable) {
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
                        Action::Board(BoardAction::Open(dir)) => open_folder(&dir),
                        Action::Channels(ChannelsAction::ConnectGmail(app)) => {
                            let waker = events.waker();
                            let state = match SignIn::start(app, move || waker.wake()) {
                                Ok(started) => {
                                    sign_in = Some(started);
                                    GmailState::Waiting
                                }
                                Err(err) => GmailState::Failed(format!("{err:#}")),
                            };
                            window.set_gmail(&state);
                        }
                        Action::Channels(ChannelsAction::CancelGmail) => {
                            if let Some(waiting) = &sign_in {
                                waiting.cancel();
                            }
                        }
                        Action::Channels(ChannelsAction::DisconnectGmail) => {
                            let state = match vault::remove(nexo_gmail::CHANNEL) {
                                Ok(()) => {
                                    worker.disconnect(nexo_gmail::CHANNEL.to_owned())?;
                                    GmailState::Off
                                }
                                Err(err) => GmailState::Failed(format!("{err:#}")),
                            };
                            window.set_gmail(&state);
                        }
                        Action::Channels(ChannelsAction::OpenGoogleCloud) => {
                            if let Err(err) = signin::open_url(GOOGLE_CLOUD) {
                                glasskit::show_error(APP_NAME, &format!("{err:#}"));
                            }
                        }
                        action
                        @ (Action::Board(BoardAction::SetDone { .. }) | Action::Inbox(_)) => {
                            act(action, &mut worker)?;
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
