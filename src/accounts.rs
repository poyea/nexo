// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use anyhow::Result;
use glasskit::events::Waker;
use nexo_core::Source;
use nexo_gmail::{Credentials, Gmail, LinkedIn};
use nexo_mail::{Account, Email};
use nexo_telegram::{App, Login, Progress, Telegram};

use crate::channels::{ChannelsAction, EmailState, GmailState, TelegramState};
use crate::config::Config;
use crate::signin::{self, SignIn, SignedIn, Task};
use crate::vault;
use crate::window::MainWindow;
use crate::worker::Worker;

const GOOGLE_CLOUD: &str = "https://console.cloud.google.com/apis/credentials";
const TELEGRAM_APPS: &str = "https://my.telegram.org/apps";
/// The file next to the app that keeps the sign-in to Telegram.
const TELEGRAM_SESSION: &str = "telegram.session";

/// What a change of the accounts works on.
pub struct Context<'a> {
    pub worker: &'a mut Worker,
    pub window: &'a mut MainWindow,
    pub config: &'a mut Config,
    pub config_path: &'a Path,
}

/// The step of Telegram that runs now on a thread of its own.
enum TelegramTask {
    /// Connect with the sign-in that the session file keeps.
    Open(Task<Result<Option<Telegram>, String>>),
    /// Ask for the code.
    Start(Task<Result<Login, String>>, App),
    /// Give the code or the password.
    Step(Task<(Login, Result<Progress, String>)>, App),
    /// End the sign-in.
    Leave(Task<Result<(), String>>),
}

/// The accounts of the channels: which ones are connected, and the sign-in that runs now.
pub struct Accounts {
    gmail: Option<Credentials>,
    gmail_state: GmailState,
    sign_in: Option<SignIn>,
    email_state: EmailState,
    /// The check of a mail account that runs now, and the account.
    email_check: Option<(Task<Result<(), String>>, Account)>,
    email: Option<Account>,
    telegram: Option<App>,
    telegram_state: TelegramState,
    telegram_task: Option<TelegramTask>,
    /// The sign-in to Telegram that waits for the code or the password of the user.
    telegram_login: Option<(Login, App)>,
    telegram_session: PathBuf,
}

fn words(err: anyhow::Error) -> String {
    format!("{err:#}")
}

impl Accounts {
    /// Read the accounts that the Windows Credential Manager has. `app_dir` is the
    /// directory of the app, for the files that keep a sign-in.
    pub fn load(app_dir: &Path) -> Result<Self> {
        vault::open()?;
        let gmail: Option<Credentials> = vault::load(nexo_gmail::CHANNEL)?;
        let email: Option<Account> = vault::load(nexo_mail::CHANNEL)?;
        let telegram: Option<App> = vault::load(nexo_telegram::CHANNEL)?;
        Ok(Self {
            gmail_state: match &gmail {
                Some(credentials) => GmailState::On(credentials.email.clone()),
                None => GmailState::Off,
            },
            gmail,
            sign_in: None,
            email_state: match &email {
                Some(account) => EmailState::On(account.address.clone()),
                None => EmailState::Off,
            },
            email_check: None,
            email,
            telegram_state: match &telegram {
                Some(_) => TelegramState::Busy("Connecting".into()),
                None => TelegramState::Off,
            },
            telegram,
            telegram_task: None,
            telegram_login: None,
            telegram_session: app_dir.join(TELEGRAM_SESSION),
        })
    }

    /// The sources of the channels that are connected and need no network to start.
    pub fn sources(&self, config: &Config) -> Vec<Box<dyn Source>> {
        let mut sources: Vec<Box<dyn Source>> = Vec::new();
        if let Some(credentials) = &self.gmail {
            sources.push(Box::new(Gmail::new(credentials.clone())));
            if config.linkedin {
                sources.push(Box::new(LinkedIn::new(credentials.clone())));
            }
        }
        if let Some(account) = &self.email {
            sources.push(Box::new(Email::new(account.clone())));
        }
        sources
    }

    /// Connect to Telegram now, and wait for it. `None` if Telegram is not connected.
    pub fn telegram_now(&self) -> Result<Option<Telegram>> {
        match &self.telegram {
            Some(app) => Telegram::open(app, &self.telegram_session),
            None => Ok(None),
        }
    }

    /// Start the channels that need the network to start. [`Accounts::poll`] takes the
    /// result.
    pub fn start(&mut self, waker: Waker) {
        let Some(app) = self.telegram.clone() else {
            return;
        };
        let session = self.telegram_session.clone();
        let open = move || Telegram::open(&app, &session).map_err(words);
        match Task::start(open, move || waker.wake()) {
            Ok(task) => self.telegram_task = Some(TelegramTask::Open(task)),
            Err(err) => self.telegram_state = TelegramState::Failed(words(err)),
        }
    }

    /// Show the state of each channel on the channels page.
    pub fn show(&self, window: &mut MainWindow, config: &Config) {
        window.set_gmail(&self.gmail_state);
        window.set_linkedin(config.linkedin, self.gmail.is_some());
        window.set_email(&self.email_state);
        window.set_telegram(&self.telegram_state);
    }

    /// Do what the user asked for on the channels page.
    pub fn act(&mut self, action: ChannelsAction, waker: Waker, cx: Context) -> Result<()> {
        let wake = move || waker.wake();
        match action {
            ChannelsAction::ConnectGmail(app) => {
                self.gmail_state = match SignIn::start(app, wake) {
                    Ok(started) => {
                        self.sign_in = Some(started);
                        GmailState::Waiting
                    }
                    Err(err) => GmailState::Failed(words(err)),
                };
            }
            ChannelsAction::CancelGmail => {
                if let Some(waiting) = &self.sign_in {
                    waiting.cancel();
                }
            }
            ChannelsAction::DisconnectGmail => match vault::remove(nexo_gmail::CHANNEL) {
                Ok(()) => {
                    self.gmail = None;
                    self.gmail_state = GmailState::Off;
                    cx.worker.disconnect(nexo_gmail::CHANNEL.to_owned())?;
                    // LinkedIn reads the same Gmail.
                    cx.worker.disconnect(nexo_gmail::LINKEDIN.to_owned())?;
                }
                Err(err) => self.gmail_state = GmailState::Failed(words(err)),
            },
            ChannelsAction::OpenGoogleCloud => open_page(GOOGLE_CLOUD),
            ChannelsAction::SetLinkedIn(on) => match cx.config.save_linkedin(cx.config_path, on) {
                Ok(()) => match (&self.gmail, on) {
                    (Some(credentials), true) => {
                        cx.worker
                            .connect(Box::new(LinkedIn::new(credentials.clone())))?;
                    }
                    _ => cx.worker.disconnect(nexo_gmail::LINKEDIN.to_owned())?,
                },
                Err(err) => glasskit::show_error(crate::APP_NAME, &words(err)),
            },
            ChannelsAction::ConnectEmail(account) => {
                // The sign-in to the servers can take some seconds.
                let checked = account.clone();
                let check = move || nexo_mail::check(&checked).map_err(words);
                self.email_state = match Task::start(check, wake) {
                    Ok(task) => {
                        self.email_check = Some((task, account));
                        EmailState::Checking
                    }
                    Err(err) => EmailState::Failed(words(err)),
                };
            }
            ChannelsAction::DisconnectEmail => match vault::remove(nexo_mail::CHANNEL) {
                Ok(()) => {
                    self.email = None;
                    self.email_state = EmailState::Off;
                    cx.worker.disconnect(nexo_mail::CHANNEL.to_owned())?;
                }
                Err(err) => self.email_state = EmailState::Failed(words(err)),
            },
            ChannelsAction::OpenTelegramApps => open_page(TELEGRAM_APPS),
            ChannelsAction::StartTelegram { app, phone } => {
                let (asked, session) = (app.clone(), self.telegram_session.clone());
                let start = move || Login::start(&asked, &session, &phone).map_err(words);
                self.telegram_login = None;
                self.telegram_state = match Task::start(start, wake) {
                    Ok(task) => {
                        self.telegram_task = Some(TelegramTask::Start(task, app));
                        TelegramState::Busy("Asking Telegram for a code".into())
                    }
                    Err(err) => TelegramState::Failed(words(err)),
                };
            }
            ChannelsAction::TelegramCode(code) => {
                self.telegram_step(wake, move |login| login.code(&code));
            }
            ChannelsAction::TelegramPassword(password) => {
                self.telegram_step(wake, move |login| login.password(&password));
            }
            ChannelsAction::CancelTelegram => {
                self.telegram_login = None;
                self.telegram_state = TelegramState::Off;
            }
            ChannelsAction::DisconnectTelegram => match vault::remove(nexo_telegram::CHANNEL) {
                Ok(()) => {
                    cx.worker.disconnect(nexo_telegram::CHANNEL.to_owned())?;
                    self.telegram_state = TelegramState::Off;
                    if let Some(app) = self.telegram.take() {
                        // Tell Telegram that the sign-in ended, then remove its file.
                        let session = self.telegram_session.clone();
                        let leave = move || {
                            let signed_out = match Telegram::open(&app, &session) {
                                Ok(Some(telegram)) => telegram.sign_out(),
                                Ok(None) => Ok(()),
                                Err(err) => Err(err),
                            };
                            let _ = std::fs::remove_file(&session);
                            signed_out.map_err(words)
                        };
                        if let Ok(task) = Task::start(leave, wake) {
                            self.telegram_task = Some(TelegramTask::Leave(task));
                        }
                    }
                }
                Err(err) => self.telegram_state = TelegramState::Failed(words(err)),
            },
        }
        self.show(cx.window, cx.config);
        Ok(())
    }

    /// Do a step of the sign-in to Telegram on a thread of its own.
    fn telegram_step(
        &mut self,
        wake: impl Fn() + Send + 'static,
        step: impl FnOnce(&mut Login) -> Result<Progress> + Send + 'static,
    ) {
        let Some((mut login, app)) = self.telegram_login.take() else {
            return;
        };
        let work = move || {
            let progress = step(&mut login).map_err(words);
            (login, progress)
        };
        self.telegram_state = match Task::start(work, wake) {
            Ok(task) => {
                self.telegram_task = Some(TelegramTask::Step(task, app));
                TelegramState::Busy("Signing in".into())
            }
            Err(err) => TelegramState::Failed(words(err)),
        };
    }

    /// Take the result of a sign-in, if one ended. It gives true when a channel is new, so
    /// that a sync must run now.
    pub fn poll(&mut self, cx: Context) -> Result<bool> {
        let mut connected = false;
        let before = (
            self.gmail_state.clone(),
            self.email_state.clone(),
            self.telegram_state.clone(),
        );
        if let Some(ended) = self.sign_in.as_ref().and_then(SignIn::poll) {
            self.sign_in = None;
            self.gmail_state = match ended {
                SignedIn::Yes(credentials) => {
                    match vault::save(nexo_gmail::CHANNEL, &credentials) {
                        Ok(()) => {
                            cx.worker
                                .connect(Box::new(Gmail::new(credentials.clone())))?;
                            if cx.config.linkedin {
                                cx.worker
                                    .connect(Box::new(LinkedIn::new(credentials.clone())))?;
                            }
                            connected = true;
                            let email = credentials.email.clone();
                            self.gmail = Some(credentials);
                            GmailState::On(email)
                        }
                        Err(err) => GmailState::Failed(words(err)),
                    }
                }
                SignedIn::Stopped => GmailState::Off,
                SignedIn::Failed(why) => GmailState::Failed(why),
            };
        }
        let checked = self.email_check.as_ref().and_then(|(task, _)| task.poll());
        if let Some(result) = checked
            && let Some((_, account)) = self.email_check.take()
        {
            let saved =
                result.and_then(|()| vault::save(nexo_mail::CHANNEL, &account).map_err(words));
            self.email_state = match saved {
                Ok(()) => {
                    cx.worker.connect(Box::new(Email::new(account.clone())))?;
                    connected = true;
                    let address = account.address.clone();
                    self.email = Some(account);
                    EmailState::On(address)
                }
                Err(why) => EmailState::Failed(why),
            };
        }
        connected |= self.poll_telegram(cx.worker)?;
        let now = (&self.gmail_state, &self.email_state, &self.telegram_state);
        if now != (&before.0, &before.1, &before.2) {
            self.show(cx.window, cx.config);
        }
        Ok(connected)
    }

    /// Take the result of the step of Telegram that ran. It gives true when Telegram is
    /// connected now.
    fn poll_telegram(&mut self, worker: &mut Worker) -> Result<bool> {
        let Some(task) = self.telegram_task.take() else {
            return Ok(false);
        };
        let on = TelegramState::On;
        match task {
            TelegramTask::Open(task) => match task.poll() {
                Some(Ok(Some(telegram))) => {
                    worker.connect(Box::new(telegram))?;
                    self.telegram_state = on;
                    return Ok(true);
                }
                Some(Ok(None)) => {
                    self.telegram_state =
                        TelegramState::Failed("the sign-in has ended: sign in again".into());
                }
                Some(Err(why)) => self.telegram_state = TelegramState::Failed(why),
                None => self.telegram_task = Some(TelegramTask::Open(task)),
            },
            TelegramTask::Start(task, app) => match task.poll() {
                Some(Ok(login)) => {
                    self.telegram_login = Some((login, app));
                    self.telegram_state = TelegramState::Code(None);
                }
                Some(Err(why)) => self.telegram_state = TelegramState::Failed(why),
                None => self.telegram_task = Some(TelegramTask::Start(task, app)),
            },
            TelegramTask::Step(task, app) => match task.poll() {
                Some((login, Ok(Progress::SignedIn))) => {
                    match vault::save(nexo_telegram::CHANNEL, &app) {
                        Ok(()) => {
                            worker.connect(Box::new(login.finish()))?;
                            self.telegram = Some(app);
                            self.telegram_state = on;
                            return Ok(true);
                        }
                        Err(err) => self.telegram_state = TelegramState::Failed(words(err)),
                    }
                }
                Some((login, Ok(progress))) => {
                    self.telegram_login = Some((login, app));
                    self.telegram_state = match progress {
                        Progress::WrongCode => {
                            TelegramState::Code(Some("That code is not correct.".into()))
                        }
                        Progress::NeedsPassword(hint) => {
                            TelegramState::Password { hint, wrong: false }
                        }
                        Progress::WrongPassword => TelegramState::Password {
                            hint: None,
                            wrong: true,
                        },
                        Progress::SignedIn => on,
                    };
                }
                Some((_, Err(why))) => self.telegram_state = TelegramState::Failed(why),
                None => self.telegram_task = Some(TelegramTask::Step(task, app)),
            },
            TelegramTask::Leave(task) => match task.poll() {
                Some(Ok(())) => {}
                Some(Err(why)) => {
                    self.telegram_state = TelegramState::Failed(format!(
                        "nexo is disconnected, but Telegram still lists it under Devices: {why}"
                    ));
                }
                None => self.telegram_task = Some(TelegramTask::Leave(task)),
            },
        }
        Ok(false)
    }
}

fn open_page(url: &str) {
    if let Err(err) = signin::open_url(url) {
        glasskit::show_error(crate::APP_NAME, &words(err));
    }
}
