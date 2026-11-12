// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::path::Path;

use anyhow::Result;
use glasskit::events::Waker;
use nexo_core::Source;
use nexo_gmail::{Credentials, Gmail, LinkedIn};

use crate::channels::{ChannelsAction, GmailState};
use crate::config::Config;
use crate::signin::{self, SignIn, SignedIn};
use crate::vault;
use crate::window::MainWindow;
use crate::worker::Worker;

const GOOGLE_CLOUD: &str = "https://console.cloud.google.com/apis/credentials";

/// What a change of the accounts works on.
pub struct Context<'a> {
    pub worker: &'a mut Worker,
    pub window: &'a mut MainWindow,
    pub config: &'a mut Config,
    pub config_path: &'a Path,
}

/// The accounts of the channels: which ones are connected, and the sign-in that runs now.
pub struct Accounts {
    gmail: Option<Credentials>,
    gmail_state: GmailState,
    sign_in: Option<SignIn>,
}

impl Accounts {
    /// Read the accounts that the Windows Credential Manager has.
    pub fn load() -> Result<Self> {
        vault::open()?;
        let gmail: Option<Credentials> = vault::load(nexo_gmail::CHANNEL)?;
        Ok(Self {
            gmail_state: match &gmail {
                Some(credentials) => GmailState::On(credentials.email.clone()),
                None => GmailState::Off,
            },
            gmail,
            sign_in: None,
        })
    }

    /// The sources of the channels that are connected.
    pub fn sources(&self, config: &Config) -> Vec<Box<dyn Source>> {
        let mut sources: Vec<Box<dyn Source>> = Vec::new();
        if let Some(credentials) = &self.gmail {
            sources.push(Box::new(Gmail::new(credentials.clone())));
            if config.linkedin {
                sources.push(Box::new(LinkedIn::new(credentials.clone())));
            }
        }
        sources
    }

    /// Show the state of each channel on the channels page.
    pub fn show(&self, window: &mut MainWindow, config: &Config) {
        window.set_gmail(&self.gmail_state);
        window.set_linkedin(config.linkedin, self.gmail.is_some());
    }

    /// Do what the user asked for on the channels page.
    pub fn act(&mut self, action: ChannelsAction, waker: Waker, cx: Context) -> Result<()> {
        match action {
            ChannelsAction::ConnectGmail(app) => {
                self.gmail_state = match SignIn::start(app, move || waker.wake()) {
                    Ok(started) => {
                        self.sign_in = Some(started);
                        GmailState::Waiting
                    }
                    Err(err) => GmailState::Failed(format!("{err:#}")),
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
                Err(err) => self.gmail_state = GmailState::Failed(format!("{err:#}")),
            },
            ChannelsAction::OpenGoogleCloud => {
                if let Err(err) = signin::open_url(GOOGLE_CLOUD) {
                    glasskit::show_error(crate::APP_NAME, &format!("{err:#}"));
                }
            }
            ChannelsAction::SetLinkedIn(on) => match cx.config.save_linkedin(cx.config_path, on) {
                Ok(()) => match (&self.gmail, on) {
                    (Some(credentials), true) => {
                        cx.worker
                            .connect(Box::new(LinkedIn::new(credentials.clone())))?;
                    }
                    _ => cx.worker.disconnect(nexo_gmail::LINKEDIN.to_owned())?,
                },
                Err(err) => glasskit::show_error(crate::APP_NAME, &format!("{err:#}")),
            },
        }
        self.show(cx.window, cx.config);
        Ok(())
    }

    /// Take the result of a sign-in, if one ended. It gives true when a channel is new, so
    /// that a sync must run now.
    pub fn poll(&mut self, cx: Context) -> Result<bool> {
        let Some(ended) = self.sign_in.as_ref().and_then(SignIn::poll) else {
            return Ok(false);
        };
        self.sign_in = None;
        let mut connected = false;
        self.gmail_state = match ended {
            SignedIn::Yes(credentials) => match vault::save(nexo_gmail::CHANNEL, &credentials) {
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
                Err(err) => GmailState::Failed(format!("{err:#}")),
            },
            SignedIn::Stopped => GmailState::Off,
            SignedIn::Failed(why) => GmailState::Failed(why),
        };
        self.show(cx.window, cx.config);
        Ok(connected)
    }
}
