// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The filter blocks of a Sieve script a webmail of the RainLoop kind (RainLoop, SnappyMail) wrote
//! (#9999), as they are seen in such scripts: the script has the line `# RAINLOOP:SIEVE`, and
//! before each filter's Sieve a comment
//!
//! ```text
//! /*
//! BEGIN:FILTER:<id>
//! BEGIN:HEADER
//! <the filter's settings as JSON, in base64, over several lines>
//! END:HEADER
//! */
//! ```
//!
//! and after it `/* END:FILTER */`. Only the filter's name, whether it is on, and where its
//! comments are (their bytes, for a later editor of these blocks) are read; the Sieve between is
//! what the server runs, and is read as Sieve ([`crate::cmd::sieve_elsewhere`]).
//!
//! **Everything here comes from the server, and is hostile.** The base64 and its JSON are read
//! within caps ([`MAX_HEADER`], [`MAX_JSON`]); a name is display text alone, never an
//! instruction: one line, its control and format characters taken out, at most [`MAX_NAME`]
//! characters ([`mailcraft_names::incoming`]). What can't be read is left out, never guessed.

use std::ops::Range;

use serde_json::Value;

use crate::sieve::Comment;

/// The line that says a script is one such webmail's (the whole line, a `#` comment).
pub const SIGNATURE: &str = "# RAINLOOP:SIEVE";
/// The longest name kept, in characters.
pub const MAX_NAME: usize = 200;
/// The longest id kept, in characters.
const MAX_ID: usize = 100;
/// The most base64 read for one filter (white space left out), in bytes.
pub const MAX_HEADER: usize = 48 * 1024;
/// The most JSON read for one filter, in bytes.
pub const MAX_JSON: usize = 32 * 1024;
/// The most filters read from one script.
pub const MAX_FILTERS: usize = 1_000;

/// One filter block of the script.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Its id, as the webmail gave it (cleaned as a name is).
    pub id: String,
    /// Its name, to be shown, when the JSON has one that is something once cleaned.
    pub name: Option<String>,
    /// Whether the JSON says it is on, when it says.
    pub enabled: Option<bool>,
    /// The bytes of its `BEGIN:FILTER` comment.
    pub header: Range<usize>,
    /// The bytes of its `/* END:FILTER */` comment, when it has one before the next filter.
    pub end: Option<Range<usize>>,
}

/// Whether `script`, whose comments are `comments` ([`crate::sieve::lex`]), has the line
/// [`SIGNATURE`]: as a comment of its own, never inside a string.
pub fn signed(script: &str, comments: &[Comment]) -> bool {
    comments.iter().any(|c| !c.bracketed && script.get(c.at.clone()).is_some_and(|t| t.trim_end() == SIGNATURE))
}

/// The filter blocks of `script`, from its comments `comments`, in order (at most
/// [`MAX_FILTERS`]). It need not be [`signed`]; whoever reads them asks that first.
pub fn filters(script: &str, comments: &[Comment]) -> Vec<Filter> {
    let mut out: Vec<Filter> = Vec::new();
    let mut open = false;
    for c in comments.iter().filter(|c| c.bracketed) {
        let text = c.text(script).trim();
        if text == "END:FILTER" {
            if open && let Some(last) = out.last_mut() {
                last.end = Some(c.at.clone());
            }
            open = false;
        } else if let Some(f) = begin(text, c.at.clone()) {
            if out.len() >= MAX_FILTERS {
                break;
            }
            out.push(f);
            open = true;
        }
    }
    out
}

/// A `BEGIN:FILTER` comment's text, read: `None` when it isn't one.
fn begin(text: &str, at: Range<usize>) -> Option<Filter> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let id = lines.next()?.strip_prefix("BEGIN:FILTER:")?;
    let mut f = Filter { id: mailcraft_names::incoming(id, MAX_ID).unwrap_or_default(), header: at, ..Default::default() };
    if lines.next() != Some("BEGIN:HEADER") {
        return Some(f);
    }
    let mut base64 = String::new();
    let mut ended = false;
    for line in lines {
        if line == "END:HEADER" {
            ended = true;
            break;
        }
        if base64.len().saturating_add(line.len()) > MAX_HEADER {
            return Some(f);
        }
        base64.extend(line.chars().filter(|c| !c.is_whitespace()));
    }
    if let Some(json) = ended.then(|| settings(&base64)).flatten() {
        f.name = json.get("Name").and_then(Value::as_str).and_then(|n| mailcraft_names::incoming(n, MAX_NAME));
        f.enabled = json.get("Enabled").and_then(Value::as_bool);
    }
    Some(f)
}

/// A filter's settings: the JSON object its base64 says, within the caps.
pub fn settings(base64: &str) -> Option<Value> {
    if base64.len() > MAX_HEADER {
        return None;
    }
    let bytes = mailcraft_charset::base64::STANDARD.decode(base64).ok()?;
    if bytes.len() > MAX_JSON {
        return None;
    }
    // (serde_json reads at most 128 levels deep, so no JSON runs the stack out.)
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.is_object().then_some(v)
}
