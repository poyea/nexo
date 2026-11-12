// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

/// The longest encoded word that a mail header can have.
const MAX_ENCODED_WORD: usize = 75;

pub(crate) fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Gmail gives base64url with or without padding.
pub(crate) fn from_base64url(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// Percent-encode a value for a URL query.
pub(crate) fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The names and values of a URL query, such as `a=1&b=2`.
pub(crate) fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (url_decode(name), url_decode(value))
        })
        .collect()
}

/// Change the encoded words of a mail header, such as `=?UTF-8?B?4pyT?=`, to their text.
pub(crate) fn decode_words(header: &str) -> String {
    let mut out = String::new();
    let mut rest = header;
    let mut after_word = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        let Some((text, len)) = encoded_word(&rest[start..]) else {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            after_word = false;
            continue;
        };
        // White space between two encoded words is not part of the text.
        if !(after_word && before.trim().is_empty()) {
            out.push_str(before);
        }
        out.push_str(&text);
        rest = &rest[start + len..];
        after_word = true;
    }
    out.push_str(rest);
    out
}

/// The text of the encoded word at the start of `text`, and the length of the word.
fn encoded_word(text: &str) -> Option<(String, usize)> {
    let mut parts = text[2..].splitn(3, '?');
    let charset = parts.next()?.to_ascii_lowercase();
    let encoding = parts.next()?;
    let tail = parts.next()?;
    let end = tail.find("?=")?;
    let payload = &tail[..end];
    let bytes = match encoding {
        "B" | "b" => STANDARD.decode(payload).ok()?,
        "Q" | "q" => q_decode(payload),
        _ => return None,
    };
    let decoded = match charset.as_str() {
        "utf-8" | "utf8" | "us-ascii" => String::from_utf8_lossy(&bytes).into_owned(),
        "iso-8859-1" | "latin1" | "windows-1252" => bytes.iter().map(|&b| char::from(b)).collect(),
        _ => return None,
    };
    let len = 2 + charset.len() + 1 + encoding.len() + 1 + end + 2;
    Some((decoded, len))
}

fn q_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'=', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'_', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

/// A value for a mail header: as it is if it is ASCII, else as encoded words.
pub(crate) fn encode_words(value: &str) -> String {
    if value.is_ascii() {
        return value.to_owned();
    }
    const FRAME: usize = "=?UTF-8?B??=".len();
    let max_bytes = (MAX_ENCODED_WORD - FRAME) / 4 * 3;
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in value.chars() {
        if chunk.len() + c.len_utf8() > max_bytes {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    words.push(chunk);
    words
        .iter()
        .map(|word| format!("=?UTF-8?B?{}?=", STANDARD.encode(word)))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// Base64 for the body of a mail, in lines of 76 characters.
pub(crate) fn base64_lines(bytes: &[u8]) -> String {
    let encoded = STANDARD.encode(bytes);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 38);
    for line in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_values_go_there_and_back() {
        let value = "a b&c=d/é+";
        assert_eq!(url_encode(value), "a%20b%26c%3Dd%2F%C3%A9%2B");
        assert_eq!(url_decode(&url_encode(value)), value);
        assert_eq!(
            query_pairs("code=4%2F0A&state=x+y&flag"),
            [
                ("code".to_owned(), "4/0A".to_owned()),
                ("state".to_owned(), "x y".to_owned()),
                ("flag".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn encoded_words_become_text() {
        assert_eq!(decode_words("Plain subject"), "Plain subject");
        assert_eq!(decode_words("=?UTF-8?B?5pel5pys?= news"), "日本 news");
        assert_eq!(
            decode_words("=?utf-8?Q?Caf=C3=A9_au_lait?="),
            "Café au lait"
        );
        assert_eq!(
            decode_words("=?ISO-8859-1?Q?Andr=E9?= <a@b.c>"),
            "André <a@b.c>"
        );
        assert_eq!(decode_words("=?UTF-8?B?5pel?= =?UTF-8?B?5pys?="), "日本");
        assert_eq!(decode_words("1 =? 2 =?x?"), "1 =? 2 =?x?");
    }

    #[test]
    fn a_header_value_goes_there_and_back() {
        assert_eq!(encode_words("Re: Offer"), "Re: Offer");
        let long = "Re: 日本語の件名はとても長いので二つ以上の語に分かれます";
        let encoded = encode_words(long);
        assert!(encoded.is_ascii());
        assert!(encoded.split("\r\n ").count() > 1);
        assert!(encoded.split("\r\n ").all(|w| w.len() <= MAX_ENCODED_WORD));
        assert_eq!(decode_words(&encoded.replace("\r\n", "")), long);
    }

    #[test]
    fn base64_has_short_lines() {
        let lines = base64_lines(&[b'a'; 100]);
        assert!(lines.lines().all(|line| line.len() <= 76));
        assert_eq!(lines.lines().count(), 2);
        assert_eq!(from_base64url(&base64url(b"hi?")).unwrap(), b"hi?");
        assert_eq!(from_base64url("aGk_").unwrap(), b"hi?");
        assert_eq!(from_base64url("aGk=").unwrap(), b"hi");
    }
}
