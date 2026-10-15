//! Shared HTML helpers for the website readers: a tag tokenizer for
//! machine-generated markup (every attribute quoted) and entity decoding.
//! Malformed input ends the token stream early; nothing here panics.

/// Plain text of an HTML snippet: tags become spaces, entities decoded,
/// whitespace collapsed.
pub(crate) fn text(html: &str) -> String {
    let mut s = String::new();
    for t in tokens(html) {
        match t {
            Tok::Text(t) => s.push_str(&decode_entities(t)),
            _ => s.push(' '),
        }
    }
    squash(&s)
}

pub(crate) fn squash(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || c == '\u{a0}' || c == '\u{200b}')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Tokenizer — machine-generated markup, every attribute quoted. Malformed input
// ends the stream early; it never panics.

pub(crate) enum Tok<'a> {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
    },
    End {
        name: String,
    },
    Text(&'a str),
}

pub(crate) fn tokens(html: &str) -> Vec<Tok<'_>> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    while i < b.len() {
        let next = b.get(i + 1).copied().unwrap_or(0);
        if b[i] != b'<' || !(next.is_ascii_alphabetic() || next == b'/' || next == b'!') {
            i += 1;
            continue;
        }
        if text_start < i {
            out.push(Tok::Text(&html[text_start..i]));
        }
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(b.len(), |j| i + j + 3);
        } else if next == b'!' {
            i = html[i..].find('>').map_or(b.len(), |j| i + j + 1);
        } else if next == b'/' {
            let end = html[i..].find('>').map_or(b.len(), |j| i + j);
            out.push(Tok::End {
                name: html[i + 2..end].trim().to_ascii_lowercase(),
            });
            i = (end + 1).min(b.len());
        } else {
            match start_tag(html, i) {
                Some((tok, end)) => {
                    // Raw-text elements: skip their content wholesale.
                    let raw = matches!(&tok, Tok::Start { name, .. } if name == "script" || name == "style");
                    out.push(tok);
                    i = end;
                    if raw {
                        i = html[i..].find("</").map_or(b.len(), |j| i + j);
                    }
                }
                None => return out,
            }
        }
        text_start = i;
    }
    if text_start < b.len() {
        out.push(Tok::Text(&html[text_start..]));
    }
    out
}

/// `<name attr="v" …>` at `i` (an ASCII `<`); the token and the index after `>`.
fn start_tag(html: &str, i: usize) -> Option<(Tok<'_>, usize)> {
    let b = html.as_bytes();
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    let name = html[i + 1..j].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        match b.get(j)? {
            b'>' => return Some((Tok::Start { name, attrs }, j + 1)),
            b'/' => {
                j += 1;
                continue;
            }
            _ => {}
        }
        let k = j;
        while j < b.len() && !b[j].is_ascii_whitespace() && !matches!(b[j], b'=' | b'>' | b'/') {
            j += 1;
        }
        if j == k {
            // Not an attribute-name byte (`=` with no name). Step over the
            // whole char — `j` must stay on a char boundary.
            j += html[j..].chars().next()?.len_utf8();
            continue;
        }
        let key = html[k..j].to_string();
        let mut val = String::new();
        if b.get(j) == Some(&b'=') {
            j += 1;
            match *b.get(j)? {
                q @ (b'"' | b'\'') => {
                    let end = j + 1 + html[j + 1..].find(q as char)?;
                    val = decode_entities(&html[j + 1..end]);
                    j = end + 1;
                }
                _ => {
                    let k = j;
                    while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                        j += 1;
                    }
                    val = decode_entities(&html[k..j]);
                }
            }
        }
        attrs.push((key, val));
    }
}

pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // `&` and `;` are ASCII, so slicing between them stays on boundaries.
        let semi = rest.bytes().take(12).position(|c| c == b';');
        let decoded = semi.and_then(|semi| {
            let ent = &rest[1..semi];
            let c = match ent {
                "quot" => Some('"'),
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "rsquo" => Some('\u{2019}'),
                "lsquo" => Some('\u{2018}'),
                "rdquo" => Some('\u{201d}'),
                "ldquo" => Some('\u{201c}'),
                "ndash" => Some('\u{2013}'),
                "mdash" => Some('\u{2014}'),
                "hellip" => Some('\u{2026}'),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
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

/// Text after `start` up to `end`; `None` if either is missing.
pub(crate) fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)?;
    Some(&s[i..i + j])
}
