// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::fmt;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ureq::Agent;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Get a new access token this long before the end of the old one.
const MARGIN: Duration = Duration::from_secs(60);
/// How long to wait before each new try of a read that Gmail was too busy for.
#[cfg(not(test))]
const RETRY_AFTER: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(2)];
#[cfg(test)]
const RETRY_AFTER: [Duration; 2] = [Duration::from_millis(5), Duration::from_millis(10)];

/// The addresses of the Google services. A test gives the addresses of a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// The Gmail API for the user who signed in.
    pub api: String,
    /// Where a code or a refresh token becomes an access token.
    pub token: String,
    /// The sign-in page.
    pub auth: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://gmail.googleapis.com/gmail/v1/users/me".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        }
    }
}

/// What nexo keeps to use Gmail as one user. It is a secret: keep it out of the folder.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    /// The address of the user.
    pub email: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// A response with a status that is not a success.
#[derive(Debug)]
pub(crate) struct Failure {
    pub status: u16,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for Failure {}

/// The HTTP status of the error, if the server gave one.
pub(crate) fn status_of(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<Failure>().map(|f| f.status)
}

pub(crate) fn agent() -> Agent {
    Agent::new_with_config(
        Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build(),
    )
}

/// The body of a response as JSON, or the error that the server gave.
fn read<T: DeserializeOwned>(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    url: &str,
) -> Result<T> {
    let mut response = response.with_context(|| format!("calling {url}"))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the answer of {url}"))?;
    if !(200..300).contains(&status) {
        return Err(Failure {
            status,
            message: error_message(&text),
        }
        .into());
    }
    serde_json::from_str(&text).with_context(|| format!("reading the answer of {url}"))
}

/// The words of an error from the Gmail API or from the token service.
fn error_message(body: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let error = &json["error"];
    let words = [&error["message"], &json["error_description"], error]
        .into_iter()
        .find_map(|v| v.as_str());
    match words {
        Some(words) => words.to_owned(),
        None => body.chars().take(200).collect::<String>().trim().to_owned(),
    }
}

#[derive(Deserialize)]
pub(crate) struct Token {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "an_hour")]
    pub expires_in: u64,
}

fn an_hour() -> u64 {
    3600
}

pub(crate) fn request_token(agent: &Agent, url: &str, form: &[(&str, &str)]) -> Result<Token> {
    read(agent.post(url).send_form(form.iter().copied()), url)
}

/// The Gmail API with an access token. It can go to other threads.
#[derive(Clone)]
pub(crate) struct Session {
    agent: Agent,
    api: String,
    bearer: String,
}

impl Session {
    pub fn new(agent: Agent, api: &str, access_token: &str) -> Self {
        Self {
            agent,
            api: api.to_owned(),
            bearer: format!("Bearer {access_token}"),
        }
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let url = format!("{}/{path}", self.api);
        let get = || {
            let response = self
                .agent
                .get(&url)
                .header("Authorization", &self.bearer)
                .query_pairs(query.iter().copied())
                .call();
            read(response, &url)
        };
        // A read is safe to do again. Gmail gives 429 for too many requests in a short time.
        let mut result = get();
        for wait in RETRY_AFTER {
            match &result {
                Err(err) if matches!(status_of(err), Some(429 | 500..=599)) => {
                    std::thread::sleep(wait);
                    result = get();
                }
                _ => break,
            }
        }
        result
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        let url = format!("{}/{path}", self.api);
        let response = self
            .agent
            .post(&url)
            .header("Authorization", &self.bearer)
            .send_json(body);
        read(response, &url)
    }
}

/// The Gmail API for one user. It gets a new access token when the old one ends.
pub(crate) struct Client {
    agent: Agent,
    endpoints: Endpoints,
    credentials: Credentials,
    access: Option<(Session, Instant)>,
}

impl Client {
    pub fn new(credentials: Credentials, endpoints: Endpoints) -> Self {
        Self {
            agent: agent(),
            endpoints,
            credentials,
            access: None,
        }
    }

    pub fn email(&self) -> &str {
        &self.credentials.email
    }

    /// Do `work` with a session. If the server does not take the access token, get a new
    /// one and do the work one more time.
    pub fn with<T>(&mut self, work: impl Fn(&Session) -> Result<T>) -> Result<T> {
        let session = self.session()?;
        match work(&session) {
            Err(err) if status_of(&err) == Some(401) => {
                self.access = None;
                work(&self.session()?)
            }
            result => result,
        }
    }

    fn session(&mut self) -> Result<Session> {
        if let Some((session, until)) = &self.access
            && Instant::now() < *until
        {
            return Ok(session.clone());
        }
        let c = &self.credentials;
        let form = [
            ("client_id", c.client_id.as_str()),
            ("client_secret", c.client_secret.as_str()),
            ("refresh_token", c.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        let token =
            request_token(&self.agent, &self.endpoints.token, &form).map_err(
                |err| match status_of(&err) {
                    Some(400 | 401) => err
                        .context("the sign-in has ended: connect Gmail again on the Channels page"),
                    _ => err.context("getting access to Gmail"),
                },
            )?;
        let life = Duration::from_secs(token.expires_in).saturating_sub(MARGIN);
        let session = Session::new(self.agent.clone(), &self.endpoints.api, &token.access_token);
        self.access = Some((session.clone(), Instant::now() + life));
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_of_each_service_is_readable() {
        let api = r#"{"error":{"code":404,"message":"Requested entity was not found."}}"#;
        assert_eq!(error_message(api), "Requested entity was not found.");
        let oauth = r#"{"error":"invalid_grant","error_description":"Token has been revoked."}"#;
        assert_eq!(error_message(oauth), "Token has been revoked.");
        assert_eq!(
            error_message(r#"{"error":"invalid_client"}"#),
            "invalid_client"
        );
        assert_eq!(error_message("  Bad Gateway "), "Bad Gateway");
    }

    #[test]
    fn credentials_do_not_show_their_secrets() {
        let credentials = Credentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
            refresh_token: "token".into(),
            email: "me@example.com".into(),
        };
        let shown = format!("{credentials:?}");
        assert!(shown.contains("me@example.com"));
        assert!(!shown.contains("secret") && !shown.contains("token"));
    }
}
