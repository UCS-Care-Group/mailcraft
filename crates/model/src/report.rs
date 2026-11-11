// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A problem report sent from Help › Report a Problem to the project's report relay (#956):
//! what it holds, the relay's rules for each part, and the ways it can fail to go. Plain data:
//! the window fills it, a host with sockets (`mailcraft_net::report`) sends it, and only the
//! owner's own click on Send report ever does.
//!
//! A report holds the title, what happened, the version, the platform, a GitHub username when
//! the owner gave one, and the diagnostics when they left them ticked. Nothing else: no account,
//! address, mailbox name, machine name or identifier.
//!
//! The rules are the relay's own, so that a report it would refuse is refused here, before
//! anything goes: lengths are counted as the relay counts them (UTF-16 code units for the text,
//! bytes for the diagnostics), and a part that is too long is refused, never cut.

/// Most bytes of the whole request, as JSON.
pub const MAX_REQUEST: usize = 64 * 1024;
/// Longest title, in UTF-16 code units (as the relay counts).
pub const MAX_TITLE: usize = 200;
/// Longest text of what happened, in UTF-16 code units.
pub const MAX_TEXT: usize = 6_000;
/// Most bytes of the diagnostics.
pub const MAX_DIAGNOSTICS: usize = 50 * 1024;
/// Longest version and platform, in UTF-16 code units.
pub const MAX_LINE: usize = 120;
/// Most bytes the diagnostics take written in JSON ([`fit_diagnostics`]): with the longest title
/// and text there is still room for them in [`MAX_REQUEST`].
pub const DIAGNOSTICS_ROOM: usize = 40 * 1024;
/// Lines at the top of the diagnostics (the version and the platform) always kept by
/// [`fit_diagnostics`].
const HEAD_LINES: usize = 8;

/// What is sent, once the owner presses Send report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProblemReport {
    pub title: String,
    pub text: String,
    pub version: String,
    pub platform: String,
    /// A GitHub username to mention if the report is made public, without its `@`.
    pub username: Option<String>,
    pub diagnostics: Option<String>,
}

/// A part of a report, as the relay names it when it refuses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Text,
    Version,
    Platform,
    Username,
    Diagnostics,
    /// The whole request.
    Request,
}

impl Field {
    /// The relay's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Text => "text",
            Field::Version => "version",
            Field::Platform => "platform",
            Field::Username => "username",
            Field::Diagnostics => "diagnostics",
            Field::Request => "request",
        }
    }

    /// The part the relay names; `None` for a name it doesn't use.
    pub fn named(name: &str) -> Option<Field> {
        [Field::Title, Field::Text, Field::Version, Field::Platform, Field::Username, Field::Diagnostics, Field::Request]
            .into_iter()
            .find(|f| f.name() == name)
    }
}

/// Why a report wasn't sent, or wasn't taken. The draft is kept for every one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotSent {
    /// A part breaks the relay's rules (said here before sending, or by the relay: 400).
    Refused(Field),
    /// Too many reports from here today, or from everyone this hour (429).
    RateLimited,
    /// The project has switched reports off for now (503 `disabled`).
    Disabled,
    /// The relay isn't working just now (503, or another 5xx).
    Unavailable,
    /// The relay took the report and couldn't file it (502 `failed`).
    Failed,
    /// The relay couldn't be reached: offline, or its name or address didn't answer.
    Unreachable,
    /// No answer in the time given. The report may have arrived.
    TooSlow,
    /// An answer that isn't one of the relay's (too long, not JSON, an unknown status). The
    /// report may have arrived.
    Garbled,
}

/// Characters the relay refuses in any part: the C0 controls but tab, line feed and carriage
/// return, and DEL.
pub fn refused_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}')
}

/// `text` without the characters the relay refuses (an invisible control character pasted in).
fn without_controls(text: &str) -> String {
    text.chars().filter(|c| !refused_control(*c)).collect()
}

/// `text` without white space at either end, nor a byte-order mark (which the relay trims too).
fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `text` on one line: line breaks and control characters become spaces, then trimmed.
fn one_line(text: &str) -> String {
    trim(&text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>()).to_string()
}

/// How the relay counts a length: UTF-16 code units.
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Whether `name` (an `@` before it allowed) is a GitHub username: 1–39 letters, digits or single
/// hyphens, not at either end.
pub fn is_github_username(name: &str) -> bool {
    let name = name.strip_prefix('@').unwrap_or(name);
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

impl ProblemReport {
    /// A report of what the window holds, tidied as the relay would read it: each part trimmed,
    /// the title, version and platform on one line, invisible control characters left out, the
    /// username without its `@` (none when empty) and empty diagnostics none. Nothing is cut.
    pub fn new(title: &str, text: &str, version: &str, platform: &str, username: &str, diagnostics: Option<&str>) -> ProblemReport {
        let username = trim(username);
        let username = trim(username.strip_prefix('@').unwrap_or(username));
        ProblemReport {
            title: one_line(title),
            text: trim(&without_controls(text)).to_string(),
            version: one_line(version),
            platform: one_line(platform),
            username: (!username.is_empty()).then(|| username.to_string()),
            diagnostics: diagnostics.map(without_controls).filter(|d| !d.is_empty()),
        }
    }

    /// The first part the relay would refuse, by its rules (the whole request's size apart:
    /// [`ProblemReport::json`]).
    pub fn check(&self) -> Result<(), Field> {
        let line = |v: &str, max: usize| !trim(v).is_empty() && units(v) <= max && !v.chars().any(|c| c == '\r' || c == '\n' || refused_control(c));
        if !line(&self.title, MAX_TITLE) {
            return Err(Field::Title);
        }
        if trim(&self.text).is_empty() || units(trim(&self.text)) > MAX_TEXT || self.text.chars().any(refused_control) {
            return Err(Field::Text);
        }
        if !line(&self.version, MAX_LINE) {
            return Err(Field::Version);
        }
        if !line(&self.platform, MAX_LINE) {
            return Err(Field::Platform);
        }
        if self.username.as_deref().is_some_and(|u| !is_github_username(u)) {
            return Err(Field::Username);
        }
        if self.diagnostics.as_deref().is_some_and(|d| d.len() > MAX_DIAGNOSTICS || d.chars().any(refused_control)) {
            return Err(Field::Diagnostics);
        }
        Ok(())
    }

    /// The request's body: the parts in JSON, with the empty `website` the relay expects (a field
    /// a person never fills, which tells it a bot). Refused, never cut, when a part breaks the
    /// relay's rules or the whole is over [`MAX_REQUEST`].
    pub fn json(&self) -> Result<String, Field> {
        self.check()?;
        let mut body = serde_json::Map::new();
        let mut put = |k: &str, v: &str| {
            body.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        };
        put("title", &self.title);
        put("text", &self.text);
        put("version", &self.version);
        put("platform", &self.platform);
        if let Some(username) = &self.username {
            put("username", username);
        }
        if let Some(diagnostics) = &self.diagnostics {
            put("diagnostics", diagnostics);
        }
        put("website", "");
        let text = serde_json::Value::Object(body).to_string();
        if text.len() > MAX_REQUEST {
            return Err(Field::Request);
        }
        Ok(text)
    }
}

/// Bytes `c` takes in a JSON string.
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

fn json_len_of(text: &str) -> usize {
    text.chars().map(json_len).fold(0usize, usize::saturating_add)
}

/// The diagnostics as they are sent: whole when they fit the relay's room for them
/// ([`DIAGNOSTICS_ROOM`] in JSON, [`MAX_DIAGNOSTICS`] in all); else the first lines (the version
/// and the platform) and as many of the last as fit, with a line saying how many were left out
/// between. The window shows what this gives, and that is what goes.
pub fn fit_diagnostics(text: &str) -> String {
    let text = without_controls(text);
    let room = DIAGNOSTICS_ROOM.min(MAX_DIAGNOSTICS);
    if json_len_of(&text) <= room {
        return text;
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let head: Vec<&str> = lines.iter().take(HEAD_LINES).copied().collect();
    let gap_room = 80;
    let mut used = head.iter().map(|l| json_len_of(l)).fold(0usize, usize::saturating_add).saturating_add(gap_room);
    let mut tail: Vec<&str> = Vec::new();
    for line in lines.iter().skip(head.len()).rev() {
        let len = json_len_of(line);
        if used.saturating_add(len) > room {
            break;
        }
        used = used.saturating_add(len);
        tail.push(line);
    }
    tail.reverse();
    let left_out = lines.len().saturating_sub(head.len()).saturating_sub(tail.len());
    let mut out: String = head.concat();
    if left_out > 0 {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("… {left_out} earlier lines left out to fit the report\n"));
    }
    out.push_str(&tail.concat());
    // A head of very long lines is cut too, at a character, never past the room.
    if json_len_of(&out) > room {
        let mut kept = String::new();
        let mut len = 0usize;
        for c in out.chars() {
            len = len.saturating_add(json_len(c));
            if len > room {
                break;
            }
            kept.push(c);
        }
        out = kept;
    }
    out
}
