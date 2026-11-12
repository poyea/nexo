// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::{Credentials, Endpoints, Session, agent, request_token};
use crate::text::{base64url, query_pairs, url_encode};

/// Read mail, and send mail. Nexo does not change or delete mail.
const SCOPES: &str =
    "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.send";
/// How long the user has to sign in.
const SIGN_IN_TIME: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(100);
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// The OAuth client of a Google Cloud project, of the type "Desktop app".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    email_address: String,
}

/// Let the user sign in to Google in the browser, and give the credentials for Gmail.
///
/// `open` gets the address of the sign-in page and shows it in the browser. The call waits
/// until the user ends the sign-in, or until `cancel` is true.
pub fn sign_in(
    app: &App,
    endpoints: &Endpoints,
    open: impl FnOnce(&str) -> Result<()>,
    cancel: &AtomicBool,
) -> Result<Credentials> {
    // Google sends the browser back to this port with a code.
    let listener = TcpListener::bind("127.0.0.1:0").context("opening a port for the sign-in")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = random_text(32)?;
    let state = random_text(16)?;
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    let params = [
        ("client_id", app.client_id.as_str()),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", SCOPES),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &state),
        // Without these two, Google gives no refresh token.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("{name}={}", url_encode(value)))
        .collect();
    open(&format!("{}?{}", endpoints.auth, query.join("&"))).context("opening the browser")?;
    let code = wait_for_code(&listener, &state, cancel)?;

    let agent = agent();
    let form = [
        ("code", code.as_str()),
        ("client_id", &app.client_id),
        ("client_secret", &app.client_secret),
        ("redirect_uri", &redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &verifier),
    ];
    let token = request_token(&agent, &endpoints.token, &form).context("ending the sign-in")?;
    let refresh_token = token
        .refresh_token
        .context("Google gave no refresh token")?;
    let profile: Profile = Session::new(agent, &endpoints.api, &token.access_token)
        .get("profile", &[])
        .context("reading the Gmail address")?;
    Ok(Credentials {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        refresh_token,
        email: profile.email_address,
    })
}

fn random_text(bytes: usize) -> Result<String> {
    let mut buffer = vec![0; bytes];
    getrandom::fill(&mut buffer).map_err(|err| anyhow::anyhow!("getting random bytes: {err}"))?;
    Ok(base64url(&buffer))
}

fn wait_for_code(listener: &TcpListener, state: &str, cancel: &AtomicBool) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + SIGN_IN_TIME;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("the sign-in was stopped");
        }
        if Instant::now() >= deadline {
            bail!("the sign-in took too long");
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(outcome) = answer(stream, state) {
                    return outcome;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(err) => return Err(err).context("waiting for the sign-in"),
        }
    }
}

/// Read one request of the browser. It gives the code, or the reason that Google gave none.
/// `None` is for a request that is not the end of the sign-in.
fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < MAX_REQUEST_BYTES {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buffer[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    let query = query_pairs(target.split_once('?').map_or("", |(_, query)| query));
    let param = |name: &str| {
        query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let outcome = match (param("code"), param("error")) {
        _ if param("state") != Some(state) => None,
        (Some(code), _) => Some(Ok(code.to_owned())),
        (None, Some(error)) => Some(Err(anyhow::anyhow!("Google gave no access: {error}"))),
        (None, None) => None,
    };
    let (status, words) = match &outcome {
        Some(Ok(_)) => (
            "200 OK",
            "nexo has access to Gmail. You can close this tab.",
        ),
        Some(Err(_)) => (
            "200 OK",
            "nexo got no access to Gmail. You can close this tab.",
        ),
        None => ("404 Not Found", "Nothing here."),
    };
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>nexo</title>\
         <body style=\"font-family: sans-serif; margin: 3em\"><p>{words}</p></body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{page}",
        page.len()
    );
    outcome
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::mock::{CODE, ME, Server};

    fn app() -> App {
        App {
            client_id: "client-1".into(),
            client_secret: "secret-1".into(),
        }
    }

    /// What the browser does: go to the address that the sign-in page sends it to.
    fn browser(url: &str, answer: &str) -> String {
        let query = query_pairs(url.split_once('?').unwrap().1);
        let param = |name: &str| query.iter().find(|(n, _)| n == name).unwrap().1.clone();
        assert_eq!(param("code_challenge_method"), "S256");
        assert_eq!(param("access_type"), "offline");
        assert!(param("scope").contains("gmail.readonly"));
        let target = format!(
            "{}/?state={}&{answer}",
            param("redirect_uri"),
            param("state")
        );
        let address = target
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        // A browser can also ask for other things first.
        let path = target.split_once(address).unwrap().1;
        write!(stream, "GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let mut page = String::new();
        stream.read_to_string(&mut page).unwrap();
        page
    }

    #[test]
    fn a_sign_in_gives_the_credentials() {
        let server = Server::start();
        let (pages, shown) = mpsc::channel();
        let open = |url: &str| {
            let url = url.to_owned();
            std::thread::spawn(move || {
                let _ = pages.send(browser(&url, &format!("code={CODE}")));
            });
            Ok(())
        };
        let credentials =
            sign_in(&app(), &server.endpoints, open, &AtomicBool::new(false)).unwrap();
        assert_eq!(credentials, server.credentials());
        assert_eq!(credentials.email, ME);
        assert!(shown.recv().unwrap().contains("nexo has access to Gmail"));
    }

    #[test]
    fn a_refusal_ends_the_sign_in() {
        let server = Server::start();
        let open = |url: &str| {
            let url = url.to_owned();
            std::thread::spawn(move || browser(&url, "error=access_denied"));
            Ok(())
        };
        let err = sign_in(&app(), &server.endpoints, open, &AtomicBool::new(false)).unwrap_err();
        assert!(format!("{err:#}").contains("access_denied"), "{err:#}");
    }

    #[test]
    fn the_user_can_stop_the_sign_in() {
        let server = Server::start();
        let cancel = AtomicBool::new(true);
        let err = sign_in(&app(), &server.endpoints, |_| Ok(()), &cancel).unwrap_err();
        assert!(format!("{err:#}").contains("stopped"));
    }

    #[test]
    fn a_request_with_a_wrong_state_is_not_the_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            write!(stream, "GET /?state=other&code=x HTTP/1.1\r\n\r\n").unwrap();
            let mut page = String::new();
            stream.read_to_string(&mut page).unwrap();
            page
        });
        let (stream, _) = listener.accept().unwrap();
        assert!(answer(stream, "mine").is_none());
        assert!(client.join().unwrap().starts_with("HTTP/1.1 404"));
    }
}
