// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use glasskit::widget::{
    AnyId, Button, Card, Column, Id, Length, Row, Text, TextBox, TextStyle, kind,
};
use glasskit::window::Window;
use nexo_gmail::App;
use nexo_mail::{Account, IMAP_PORT, SMTP_PORT, Server};

const GMAIL_SETUP: &str = "Gmail needs an OAuth client of your own Google Cloud project: turn on \
    the Gmail API in the project, make an OAuth client of the type \"Desktop app\", and paste \
    its client ID and client secret here. The sign-in then opens in your browser.";
const MISSING: &str = "Type the client ID and the client secret.";
const EMAIL_SETUP: &str = "For a mailbox with IMAP and SMTP. If the service has app passwords, \
    make one for nexo. A server can have a port, such as 127.0.0.1:1143; without one, nexo \
    uses 993 for IMAP and 465 for SMTP.";

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
    /// Sign in to a mailbox with IMAP and SMTP.
    ConnectEmail(Account),
    DisconnectEmail,
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

/// Where the email channel is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailState {
    Off,
    /// nexo signs in to the servers now.
    Checking,
    /// Connected, with the address of the user.
    On(String),
    /// Not connected, with the reason.
    Failed(String),
}

/// The text boxes and buttons of the email card.
struct EmailCard {
    state: Id<kind::Text>,
    setup: Id<kind::Column>,
    address: Id<kind::TextBox>,
    password: Id<kind::TextBox>,
    imap: Id<kind::TextBox>,
    smtp: Id<kind::TextBox>,
    username: Id<kind::TextBox>,
    connect: Id<kind::Button>,
    disconnect: Id<kind::Button>,
}

impl EmailCard {
    fn build(w: &mut Window) -> (Self, Id<kind::Card>) {
        let title = w.add(Text::new("Email").style(TextStyle::BodyStrong));
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
        let how = w.add(Text::new(EMAIL_SETUP).style(TextStyle::Caption));
        let mut text_box = |hint: &str| {
            w.add(
                TextBox::new()
                    .placeholder(hint)
                    .max_length(200)
                    .width(Length::Fill(1.0)),
            )
        };
        let address = text_box("Mail address");
        let password = text_box("Password, or app password");
        let imap = text_box("IMAP server, such as imap.example.com");
        let smtp = text_box("SMTP server, such as smtp.example.com");
        let username = text_box("User name, if it is not the mail address");
        let connect = w.add(Button::new("Connect").accent());
        let buttons = w.add(Row::new().gap(8.0).child(connect));
        let setup = w.add(
            Column::new()
                .gap(8.0)
                .child(how)
                .child(address)
                .child(password)
                .child(imap)
                .child(smtp)
                .child(username)
                .child(buttons),
        );
        let parts = w.add(Column::new().gap(12.0).child(head).child(setup));
        let card = w.add(Card::new().child(parts));
        let email = Self {
            state,
            setup,
            address,
            password,
            imap,
            smtp,
            username,
            connect,
            disconnect,
        };
        (email, card)
    }

    fn set(&self, w: &mut Window, state: &EmailState) {
        let words = match state {
            EmailState::Off => "Not connected".to_owned(),
            EmailState::Checking => "Signing in to the servers".to_owned(),
            EmailState::On(address) => format!("Connected as {address}"),
            EmailState::Failed(why) => format!("Not connected: {why}"),
        };
        w.set_text(self.state, &words);
        let on = matches!(state, EmailState::On(_));
        let checking = *state == EmailState::Checking;
        w.set_visible(self.setup, !on);
        w.set_visible(self.disconnect, on);
        for id in [
            self.address,
            self.password,
            self.imap,
            self.smtp,
            self.username,
        ] {
            w.set_enabled(id, !checking);
        }
        w.set_enabled(self.connect, !checking);
        if on {
            w.set_text(self.password, "");
        }
    }

    /// The account in the text boxes, or what is wrong with it.
    fn account(&self, w: &Window) -> Result<Account, String> {
        account(
            &w.text(self.address),
            &w.text(self.password),
            &w.text(self.imap),
            &w.text(self.smtp),
            &w.text(self.username),
        )
    }
}

/// The account that the user typed, or what is wrong with it.
fn account(
    address: &str,
    password: &str,
    imap: &str,
    smtp: &str,
    username: &str,
) -> Result<Account, String> {
    let address = address.trim();
    if !address.contains('@') || address.contains(char::is_whitespace) {
        return Err("Type your mail address.".into());
    }
    if password.is_empty() {
        return Err("Type the password.".into());
    }
    let username = match username.trim() {
        "" => address,
        username => username,
    };
    Ok(Account {
        address: address.to_owned(),
        username: username.to_owned(),
        password: password.to_owned(),
        imap: Server::parse(imap, IMAP_PORT).map_err(|why| format!("IMAP server: {why}"))?,
        smtp: Server::parse(smtp, SMTP_PORT).map_err(|why| format!("SMTP server: {why}"))?,
        plaintext: false,
    })
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
    email: EmailCard,
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

        let (email, email_card) = EmailCard::build(w);

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
            email,
        };
        channels.set_gmail(w, &GmailState::Off);
        channels.set_linkedin(w, false, false);
        channels.set_email(w, &EmailState::Off);
        (
            channels,
            vec![
                gmail.into(),
                linkedin.into(),
                email_card.into(),
                about.into(),
            ],
        )
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

    pub fn set_email(&mut self, w: &mut Window, state: &EmailState) {
        self.email.set(w, state);
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
        } else if clicked == self.email.connect {
            match self.email.account(w) {
                Ok(account) => Some(ChannelsAction::ConnectEmail(account)),
                Err(why) => {
                    w.set_text(self.email.state, &why);
                    None
                }
            }
        } else if clicked == self.email.disconnect {
            Some(ChannelsAction::DisconnectEmail)
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
    fn a_typed_account_gets_the_usual_ports_and_the_address_as_user_name() {
        let typed = account(
            " me@example.com ",
            "pw",
            "imap.example.com",
            "127.0.0.1:1025",
            "",
        );
        let typed = typed.unwrap();
        assert_eq!(typed.address, "me@example.com");
        assert_eq!(typed.username, "me@example.com");
        assert_eq!((typed.imap.port, typed.smtp.port), (993, 1025));
        assert!(!typed.plaintext);
        let named = account("me@example.com", "pw", "a.example", "b.example", " me ").unwrap();
        assert_eq!(named.username, "me");
    }

    #[test]
    fn a_wrong_account_tells_what_is_wrong() {
        let wrong = |address, password, imap| account(address, password, imap, "s.example", "");
        assert_eq!(
            wrong("me", "pw", "i.example").unwrap_err(),
            "Type your mail address."
        );
        assert_eq!(
            wrong("me@example.com", "", "i.example").unwrap_err(),
            "Type the password."
        );
        assert!(
            wrong("me@example.com", "pw", "")
                .unwrap_err()
                .starts_with("IMAP server:")
        );
    }

    #[test]
    fn the_linkedin_card_tells_what_it_needs() {
        assert!(linkedin_words(true, true).starts_with("On."));
        assert!(linkedin_words(true, false).contains("connect Gmail"));
        assert!(linkedin_words(false, true).starts_with("Off."));
        assert!(linkedin_words(false, false).contains("needs Gmail"));
    }
}
