//! The `.env` subset (integrations DESIGN §2.1): `KEY=VALUE` with an optional `export ` prefix,
//! single quotes literal, `\n`, `\"` and `\\` in double quotes, `#` at the start of a line or after
//! whitespace in an unquoted value, no expansion and no multi-line values.

use std::collections::HashMap;
use std::sync::Arc;

use crate::problems::Problems;
use crate::tree::Origin;

/// One `KEY=VALUE` line, with the line number that becomes the value's origin.
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) line: u32,
}

/// Every entry of `text`, or every problem: a malformed line, and a key written twice, which names
/// both lines.
pub(crate) fn parse(text: &str, file: &Arc<str>) -> Result<Vec<Entry>, Problems> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut first_seen: HashMap<String, u32> = HashMap::new();
    let mut problems = Problems::new();

    for (index, raw) in text.lines().enumerate() {
        let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let origin = Origin::File { path: file.clone(), line, column: 0 };
        let content = raw.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let content = match content.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
            _ => content,
        };

        let Some((key, rest)) = content.split_once('=') else {
            problems.at_origin("", origin, "expected KEY=VALUE");
            continue;
        };
        let key = key.trim_end();
        if !valid_key(key) {
            problems.at_origin("", origin, "the key may hold only letters, digits, `_`, `.` and `-`, and may not be empty");
            continue;
        }
        let value = match parse_value(rest) {
            Ok(value) => value,
            Err(message) => {
                problems.at_origin(key, origin, message);
                continue;
            }
        };

        match first_seen.get(key) {
            Some(first) => {
                problems.at_origin(key, origin, format!("key is written twice, at lines {first} and {line}"));
            }
            None => {
                first_seen.insert(key.to_owned(), line);
                entries.push(Entry { key: key.to_owned(), value, line });
            }
        }
    }

    problems.into_result().map(|()| entries)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The value after `=`: quoted, or bare up to a comment.
fn parse_value(rest: &str) -> Result<String, &'static str> {
    let trimmed = rest.trim_start();
    if let Some(body) = trimmed.strip_prefix('\'') {
        let end = body.find('\'').ok_or("single quote is not closed")?;
        return after_quote(&body[end + 1..]).map(|()| body[..end].to_owned());
    }
    if let Some(body) = trimmed.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = body.char_indices();
        while let Some((at, c)) = chars.next() {
            match c {
                '"' => return after_quote(&body[at + 1..]).map(|()| out),
                '\\' => match chars.next() {
                    Some((_, 'n')) => out.push('\n'),
                    Some((_, '"')) => out.push('"'),
                    Some((_, '\\')) => out.push('\\'),
                    Some(_) => return Err("escape is not one of \\n, \\\" and \\\\"),
                    None => break,
                },
                other => out.push(other),
            }
        }
        return Err("double quote is not closed");
    }

    // A `#` begins a comment only after whitespace, so `KEY=a#b` keeps the `#`.
    let mut previous_space = rest.starts_with(char::is_whitespace);
    let mut end = rest.len();
    for (at, c) in rest.char_indices() {
        if c == '#' && previous_space {
            end = at;
            break;
        }
        previous_space = c.is_whitespace();
    }
    Ok(rest[..end].trim().to_owned())
}

/// What may follow a closing quote: nothing, or a comment after whitespace.
fn after_quote(tail: &str) -> Result<(), &'static str> {
    let trimmed = tail.trim_start();
    if trimmed.is_empty() || (trimmed.starts_with('#') && trimmed.len() != tail.len()) {
        return Ok(());
    }
    Err("unexpected text after the closing quote")
}
