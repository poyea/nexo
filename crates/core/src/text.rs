// Copyright (c) 2026 John Law
// SPDX-License-Identifier: MIT

//! Text that channels get as HTML, as plain text.

/// Change the character references of HTML, such as `&amp;` and `&#39;`, to their characters.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').filter(|&end| end <= 10);
        let decoded = end.and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    let code = match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some(' '),
        _ => name.strip_prefix('#')?,
    };
    let number = match code.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => code.parse().ok()?,
    };
    char::from_u32(number)
}

/// The text of an HTML message: no tags, a line break where a block ends.
pub fn html_to_text(html: &str) -> String {
    const HIDDEN: [&str; 4] = ["script", "style", "head", "title"];
    const BLOCKS: [&str; 12] = [
        "p",
        "div",
        "tr",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "table",
        "blockquote",
    ];
    let mut out = String::new();
    let mut rest = html;
    let mut hidden: Option<&str> = None;
    while let Some(start) = rest.find('<') {
        if hidden.is_none() {
            push_flowed(&mut out, &rest[..start]);
        }
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = rest[..end].to_ascii_lowercase();
        rest = rest.get(end + 1..).unwrap_or("");
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        match hidden {
            Some(open) if closing && name == open => hidden = None,
            Some(_) => {}
            None if !closing && HIDDEN.contains(&name.as_str()) => {
                hidden = HIDDEN.iter().copied().find(|h| *h == name);
            }
            None if !out.is_empty()
                && !out.ends_with('\n')
                && (name == "br" || BLOCKS.contains(&name.as_str())) =>
            {
                out.push('\n');
            }
            None => {}
        }
    }
    if hidden.is_none() {
        push_flowed(&mut out, rest);
    }
    tidy(&decode_entities(&out))
}

/// Add text of HTML, where each run of white space is one space.
fn push_flowed(out: &mut String, text: &str) {
    for c in text.chars() {
        if !c.is_whitespace() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
    }
}

/// No space at the end of a line, and no more than one empty line in sequence.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut empty = 0;
    for line in text.lines().map(str::trim) {
        empty = if line.is_empty() { empty + 1 } else { 0 };
        if empty <= 1 {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_become_characters() {
        assert_eq!(
            decode_entities("Tom &amp; Ann&#39;s &lt;3 &#x4E2D; &nope; &"),
            "Tom & Ann's <3 中 &nope; &"
        );
    }

    #[test]
    fn html_becomes_plain_text() {
        let html = "<html><head><style>p { color: red }</style></head><body>\
                    <!-- note --><p>Hi  Jane,</p><div>See <a href=\"x\">this\n link</a>.<br>Bye\
                    </div><script>alert(1)</script></body></html>";
        assert_eq!(html_to_text(html), "Hi Jane,\nSee this link.\nBye");
    }
}
