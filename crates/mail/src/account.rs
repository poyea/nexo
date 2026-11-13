// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::fmt;

use serde::{Deserialize, Serialize};

pub const IMAP_PORT: u16 = 993;
pub const SMTP_PORT: u16 = 465;

/// The name and the port of a mail server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
}

impl Server {
    /// The server that the user typed, such as `imap.example.com` or `127.0.0.1:1143`, or
    /// what is wrong with it. Without a port, it has `default_port`.
    pub fn parse(text: &str, default_port: u16) -> Result<Self, String> {
        let text = text.trim();
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) => match port.parse() {
                Ok(port) => (host, port),
                Err(_) => return Err(format!("{port:?} is not a port number.")),
            },
            None => (text, default_port),
        };
        if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
            return Err(format!("{text:?} is not the name of a server."));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }

    /// The server is on this computer, as the bridge of a mail service is. Such a server
    /// has a certificate that it made itself, so nexo does not check the certificate.
    pub(crate) fn is_local(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
}

/// What nexo keeps to use a mailbox. It is a secret: keep it out of the folder.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The mail address of the user.
    pub address: String,
    /// The name to sign in with. Most services use the address.
    pub username: String,
    pub password: String,
    /// The IMAP server. Port 993 is TLS from the start, other ports are STARTTLS.
    pub imap: Server,
    /// The SMTP server. Port 465 is TLS from the start, other ports are STARTTLS.
    pub smtp: Server,
    /// No encryption. It is only for a test server.
    #[serde(default)]
    pub plaintext: bool,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("address", &self.address)
            .field("imap", &self.imap)
            .field("smtp", &self.smtp)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_has_a_name_and_a_port() {
        let parsed = |text| Server::parse(text, IMAP_PORT);
        let server = |host: &str, port| Server {
            host: host.into(),
            port,
        };
        assert_eq!(
            parsed(" imap.example.com "),
            Ok(server("imap.example.com", 993))
        );
        assert_eq!(parsed("127.0.0.1:1143"), Ok(server("127.0.0.1", 1143)));
        assert!(parsed("").is_err());
        assert!(parsed("imap.example.com:imap").is_err());
        assert!(parsed("https://example.com").is_err());
        assert!(server("localhost", 1143).is_local());
        assert!(!server("imap.example.com", 993).is_local());
    }

    #[test]
    fn an_account_does_not_show_its_password() {
        let account = Account {
            address: "me@example.com".into(),
            username: "me@example.com".into(),
            password: "hunter2".into(),
            imap: Server::parse("imap.example.com", IMAP_PORT).unwrap(),
            smtp: Server::parse("smtp.example.com", SMTP_PORT).unwrap(),
            plaintext: false,
        };
        let shown = format!("{account:?}");
        assert!(shown.contains("me@example.com") && !shown.contains("hunter2"));
    }
}
