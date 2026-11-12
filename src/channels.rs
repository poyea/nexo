// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use glasskit::widget::{
    AnyId, Button, Card, Column, Id, Length, Row, Text, TextBox, TextStyle, kind,
};
use glasskit::window::Window;
use nexo_gmail::App;

const GMAIL_SETUP: &str = "Gmail needs an OAuth client of your own Google Cloud project: turn on \
    the Gmail API in the project, make an OAuth client of the type \"Desktop app\", and paste \
    its client ID and client secret here. The sign-in then opens in your browser.";
const MISSING: &str = "Type the client ID and the client secret.";

/// What the user asked for on the channels page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelsAction {
    /// Sign in to Gmail with this OAuth client.
    ConnectGmail(App),
    /// Stop the sign-in that waits for the browser.
    CancelGmail,
    DisconnectGmail,
    /// Show the page of Google Cloud where the user makes the OAuth client.
    OpenGoogleCloud,
    /// Turn the LinkedIn channel on or off.
    SetLinkedIn(bool),
}

/// Where the Gmail channel is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailState {
    Off,
    /// The sign-in page is open in the browser.
    Waiting,
    /// Connected, with the address of the user.
    On(String),
    /// Not connected, with the reason.
    Failed(String),
}

/// The channels page: one card for each channel that nexo has.
pub struct Channels {
    state: Id<kind::Text>,
    setup: Id<kind::Column>,
    client_id: Id<kind::TextBox>,
    client_secret: Id<kind::TextBox>,
    connect: Id<kind::Button>,
    cancel: Id<kind::Button>,
    console: Id<kind::Button>,
    disconnect: Id<kind::Button>,
    linkedin_state: Id<kind::Text>,
    linkedin_switch: Id<kind::Button>,
    linkedin_on: bool,
}

impl Channels {
    /// Make the widgets, and give the cards of the page from the top.
    pub fn build(w: &mut Window) -> (Self, Vec<AnyId>) {
        let title = w.add(Text::new("Gmail").style(TextStyle::BodyStrong));
        let state = w.add(Text::new("").style(TextStyle::Caption));
        let lines = w.add(
            Column::new()
                .width(Length::Fill(1.0))
                .gap(4.0)
                .child(title)
                .child(state),
        );
        let disconnect = w.add(Button::new("Disconnect"));
        let head = w.add(Row::new().gap(8.0).child(lines).child(disconnect));

        let how = w.add(Text::new(GMAIL_SETUP).style(TextStyle::Caption));
        let client_id = w.add(
            TextBox::new()
                .placeholder("Client ID")
                .max_length(200)
                .width(Length::Fill(1.0)),
        );
        let client_secret = w.add(
            TextBox::new()
                .placeholder("Client secret")
                .max_length(200)
                .width(Length::Fill(1.0)),
        );
        let connect = w.add(Button::new("Connect").accent());
        let cancel = w.add(Button::new("Cancel"));
        let console = w.add(Button::new("Open Google Cloud"));
        let buttons = w.add(
            Row::new()
                .gap(8.0)
                .child(connect)
                .child(cancel)
                .child(console),
        );
        let setup = w.add(
            Column::new()
                .gap(8.0)
                .child(how)
                .child(client_id)
                .child(client_secret)
                .child(buttons),
        );
        let parts = w.add(Column::new().gap(12.0).child(head).child(setup));
        let gmail = w.add(Card::new().child(parts));

        let title = w.add(Text::new("LinkedIn").style(TextStyle::BodyStrong));
        let linkedin_state = w.add(Text::new("").style(TextStyle::Caption));
        let lines = w.add(
            Column::new()
                .width(Length::Fill(1.0))
                .gap(4.0)
                .child(title)
                .child(linkedin_state),
        );
        let linkedin_switch = w.add(Button::new(""));
        let row = w.add(Row::new().gap(8.0).child(lines).child(linkedin_switch));
        let linkedin = w.add(Card::new().child(row));

        let about = w.add(
            Text::new(
                "A channel brings the messages of one service into nexo. You pick the \
                 conversations that go on the board, and nexo sends the replies that you write.",
            )
            .style(TextStyle::Caption),
        );
        let mut channels = Self {
            state,
            setup,
            client_id,
            client_secret,
            connect,
            cancel,
            console,
            disconnect,
            linkedin_state,
            linkedin_switch,
            linkedin_on: false,
        };
        channels.set_gmail(w, &GmailState::Off);
        channels.set_linkedin(w, false, false);
        (channels, vec![gmail.into(), linkedin.into(), about.into()])
    }

    pub fn set_gmail(&mut self, w: &mut Window, state: &GmailState) {
        let words = match state {
            GmailState::Off => "Not connected".to_owned(),
            GmailState::Waiting => "Waiting for the sign-in in your browser".to_owned(),
            GmailState::On(email) => format!("Connected as {email}"),
            GmailState::Failed(why) => format!("Not connected: {why}"),
        };
        w.set_text(self.state, &words);
        let on = matches!(state, GmailState::On(_));
        let waiting = *state == GmailState::Waiting;
        w.set_visible(self.setup, !on);
        w.set_visible(self.disconnect, on);
        w.set_visible(self.cancel, waiting);
        for id in [self.client_id, self.client_secret] {
            w.set_enabled(id, !waiting);
        }
        w.set_enabled(self.connect, !waiting);
        if on {
            w.set_text(self.client_secret, "");
        }
    }

    /// Show if the LinkedIn channel is on, and if the Gmail that it reads is connected.
    pub fn set_linkedin(&mut self, w: &mut Window, on: bool, gmail: bool) {
        self.linkedin_on = on;
        w.set_text(self.linkedin_state, linkedin_words(on, gmail));
        w.set_text(
            self.linkedin_switch,
            if on { "Turn off" } else { "Turn on" },
        );
        w.set_enabled(self.linkedin_switch, on || gmail);
    }

    /// What a click on this widget asks for, if the widget is part of the page.
    pub fn clicked(&mut self, w: &mut Window, clicked: AnyId) -> Option<ChannelsAction> {
        if clicked == self.connect {
            let app = App {
                client_id: w.text(self.client_id).trim().to_owned(),
                client_secret: w.text(self.client_secret).trim().to_owned(),
            };
            if app.client_id.is_empty() || app.client_secret.is_empty() {
                w.set_text(self.state, MISSING);
                return None;
            }
            Some(ChannelsAction::ConnectGmail(app))
        } else if clicked == self.cancel {
            Some(ChannelsAction::CancelGmail)
        } else if clicked == self.disconnect {
            Some(ChannelsAction::DisconnectGmail)
        } else if clicked == self.linkedin_switch {
            Some(ChannelsAction::SetLinkedIn(!self.linkedin_on))
        } else if clicked == self.console {
            Some(ChannelsAction::OpenGoogleCloud)
        } else {
            None
        }
    }
}

fn linkedin_words(on: bool, gmail: bool) -> &'static str {
    match (on, gmail) {
        (true, true) => {
            "On. The LinkedIn messages that Gmail got a notification email for come to the \
             board. To reply, open LinkedIn."
        }
        (true, false) => "On, but it needs Gmail: connect Gmail to get LinkedIn messages.",
        (false, true) => {
            "Off. nexo can read the notification emails that LinkedIn sends to your Gmail."
        }
        (false, false) => "Off. It needs Gmail: nexo reads the emails that LinkedIn sends you.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linkedin_card_tells_what_it_needs() {
        assert!(linkedin_words(true, true).starts_with("On."));
        assert!(linkedin_words(true, false).contains("connect Gmail"));
        assert!(linkedin_words(false, true).starts_with("Off."));
        assert!(linkedin_words(false, false).contains("needs Gmail"));
    }
}
