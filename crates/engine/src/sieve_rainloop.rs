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
//! and after it `/* END:FILTER */`. The filter's settings are read, and where its comments are;
//! the Sieve between is what the server runs, and is read as Sieve
//! ([`crate::cmd::sieve_elsewhere`]).
//!
//! **Written again (B5).** A filter whose settings are all ones this version knows ([`TESTS`],
//! [`CONDITIONS_TYPES`], [`ACTION_TYPES`], the keys of [`KEYS`]) and whose Sieve is the one those
//! settings give ([`Settings::sieve`], modulo trailing white space) can be changed here: its
//! settings and its Sieve are written again together, in the shape such a webmail writes them
//! (the keys in its order, the base64 in lines of 74, the Sieve indented by four spaces), so that
//! the webmail shows the change. What isn't known, or was changed by hand, stays read-only
//! ([`ReadOnly`]); everything outside the filter changed stays byte for byte. The shape was learnt
//! from scripts such a webmail wrote, and nothing else: the table of what is known grows as more
//! is seen.
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
#[derive(Clone, Default, PartialEq, Eq)]
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
    /// Its settings, when they could be read: a JSON object.
    pub settings: Option<Value>,
}

/// The settings stay out of debug output: they are the server's, and may hold addresses.
impl std::fmt::Debug for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filter")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("header", &self.header)
            .field("end", &self.end)
            .field("settings", &self.settings.is_some())
            .finish()
    }
}

impl Filter {
    /// All its bytes, from its `/*` to the end of its `/* END:FILTER */`, when it has that.
    pub fn whole(&self) -> Option<Range<usize>> {
        self.end.as_ref().map(|e| self.header.start..e.end)
    }
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
        f.settings = Some(json);
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

// ---------------------------------------------------------------------------------------------
// Written again (B5).

/// The keys of a filter's settings, in the order such a webmail writes them. A filter with another
/// key, or without one of these, is read-only.
pub const KEYS: [&str; 13] = [
    "ID",
    "Enabled",
    "Name",
    "Conditions",
    "ConditionsType",
    "ActionType",
    "ActionValue",
    "ActionValueSecond",
    "ActionValueThird",
    "ActionValueFourth",
    "Keep",
    "Stop",
    "MarkAsRead",
];
/// The keys of one condition, in that order.
pub const CONDITION_KEYS: [&str; 4] = ["Field", "Type", "Value", "ValueSecond"];

/// The conditions known: `Field` and `Type`, with the test each is written as. Only what was seen
/// in scripts such a webmail wrote; another one leaves its filter read-only until a sample shows
/// how it is written.
pub const TESTS: &[Test] = &[
    // Seen: `header :is ["X-Spam-Flag"] "YES"`.
    Test { field: "Header", kind: "EqualTo", sieve: "header :is" },
    // Inferred from the two seen ones: the same test, `:contains` as for Recipient.
    Test { field: "Header", kind: "Contains", sieve: "header :contains" },
    // Seen: `header :contains ["To", "CC"] "list@lists.example.org"`.
    Test { field: "Recipient", kind: "Contains", sieve: "header :contains" },
];
/// How the conditions go together: only "Any" was seen (`anyof`, or the one test alone).
pub const CONDITIONS_TYPES: &[&str] = &["Any"];
/// What a filter does: only "MoveTo" was seen (`fileinto`, with `Keep` and `Stop` set and
/// `MarkAsRead` not).
pub const ACTION_TYPES: &[&str] = &["MoveTo"];
/// The headers a Recipient condition looks in, as such a webmail writes them.
const RECIPIENT_HEADERS: &str = r#"["To", "CC"]"#;
/// The most conditions a filter written here has.
pub const MAX_CONDITIONS: usize = 64;
/// The longest value of a condition, or folder, written here, in characters.
pub const MAX_VALUE: usize = 1_000;
/// The width of the base64 lines, as such a webmail writes them.
const WIDTH: usize = 74;
/// The text that ends a filter block.
pub const END: &str = "/* END:FILTER */";

/// One kind of condition this version knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Test {
    /// Its `Field`.
    pub field: &'static str,
    /// Its `Type`.
    pub kind: &'static str,
    /// The Sieve test it is written as, before its header names and value.
    pub sieve: &'static str,
}

/// One condition of a filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub test: Test,
    /// What it looks for.
    pub value: String,
    /// For a Header condition, the header's name; else empty.
    pub second: String,
}

/// A filter's settings, all of them known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub id: String,
    pub enabled: bool,
    pub name: String,
    pub conditions: Vec<Condition>,
    /// The folder it moves mail to, as the server names it.
    pub folder: String,
}

/// Why a filter block can't be changed here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// Its settings can't be read, or it has no end.
    Unreadable,
    /// It has a setting this version doesn't know.
    Unknown,
    /// Its Sieve isn't the one its settings give: it was changed by hand.
    Disagrees,
    /// It moves mail to a folder MailCraft doesn't have.
    Folder,
    /// Its conditions can't be shown as one condition of a rule.
    Shape,
}

impl ReadOnly {
    /// Why, in words.
    pub fn said(self) -> &'static str {
        match self {
            ReadOnly::Unreadable => "its settings in the script can't be read",
            ReadOnly::Unknown => "it uses settings of your webmail that MailCraft doesn't know yet",
            ReadOnly::Disagrees => "its Sieve doesn't match its settings: it was changed by hand",
            ReadOnly::Folder => "it moves mail to a folder MailCraft doesn't have",
            ReadOnly::Shape => "its conditions can't be shown in the rule builder",
        }
    }
}

/// Whether `name` can be a header's name (RFC 5322 §2.2: printable US-ASCII but the colon).
pub fn header_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_VALUE && name.bytes().all(|b| (33..=126).contains(&b) && b != b':')
}

/// Whether `text` can be a value written here: something, with no control character, not too long.
pub fn value_ok(text: &str) -> bool {
    !text.trim().is_empty() && !text.chars().any(char::is_control) && text.chars().count() <= MAX_VALUE
}

/// Whether `id` is an id such a webmail gives: letters and digits.
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Settings {
    /// The settings `json` says, when every key and value is one this version knows.
    pub fn read(json: &Value) -> Result<Settings, ReadOnly> {
        let o = json.as_object().ok_or(ReadOnly::Unreadable)?;
        if o.len() != KEYS.len() || !KEYS.iter().all(|k| o.contains_key(*k)) {
            return Err(ReadOnly::Unknown);
        }
        let text = |k: &str| o.get(k).and_then(Value::as_str).ok_or(ReadOnly::Unknown);
        let flag = |k: &str| o.get(k).and_then(Value::as_bool).ok_or(ReadOnly::Unknown);
        let id = text("ID")?;
        let name = text("Name")?;
        if !id_ok(id) || name.chars().any(char::is_control) || name.chars().count() > MAX_VALUE {
            return Err(ReadOnly::Unknown);
        }
        let known = CONDITIONS_TYPES.contains(&text("ConditionsType")?)
            && ACTION_TYPES.contains(&text("ActionType")?)
            && ["ActionValueSecond", "ActionValueThird", "ActionValueFourth"].iter().all(|k| text(k).is_ok_and(str::is_empty))
            && flag("Keep")?
            && flag("Stop")?
            && !flag("MarkAsRead")?;
        let folder = text("ActionValue")?;
        if !known || !value_ok(folder) {
            return Err(ReadOnly::Unknown);
        }
        let list = o.get("Conditions").and_then(Value::as_array).ok_or(ReadOnly::Unknown)?;
        if list.is_empty() || list.len() > MAX_CONDITIONS {
            return Err(ReadOnly::Unknown);
        }
        let conditions = list.iter().map(Condition::read).collect::<Option<Vec<_>>>().ok_or(ReadOnly::Unknown)?;
        Ok(Settings { id: id.to_string(), enabled: flag("Enabled")?, name: name.to_string(), conditions, folder: folder.to_string() })
    }

    /// Them as JSON, the keys in the webmail's order.
    pub fn json(&self) -> String {
        let s = |t: &str| serde_json::to_string(t).unwrap_or_else(|_| "\"\"".to_string());
        let conditions: Vec<String> = self
            .conditions
            .iter()
            .map(|c| {
                format!("{{\"Field\":{},\"Type\":{},\"Value\":{},\"ValueSecond\":{}}}", s(c.test.field), s(c.test.kind), s(&c.value), s(&c.second))
            })
            .collect();
        format!(
            "{{\"ID\":{},\"Enabled\":{},\"Name\":{},\"Conditions\":[{}],\"ConditionsType\":\"Any\",\"ActionType\":\"MoveTo\",\"ActionValue\":{},\"ActionValueSecond\":\"\",\"ActionValueThird\":\"\",\"ActionValueFourth\":\"\",\"Keep\":true,\"Stop\":true,\"MarkAsRead\":false}}",
            s(&self.id),
            self.enabled,
            s(&self.name),
            conditions.join(","),
            s(&self.folder)
        )
    }

    /// Its `BEGIN:FILTER` comment, its lines ended by `nl`.
    pub fn begin_comment(&self, nl: &str) -> String {
        let b64 = crate::cmd::base64_encode(self.json().as_bytes());
        // (Base64 is ASCII: every 74 bytes are 74 characters.)
        let lines: Vec<&str> = b64.as_bytes().chunks(WIDTH).filter_map(|c| std::str::from_utf8(c).ok()).collect();
        let body = lines.join(nl);
        format!("/*{nl}BEGIN:FILTER:{id}{nl}BEGIN:HEADER{nl}{body}{nl}END:HEADER{nl}*/", id = self.id)
    }

    /// The Sieve it is written as when it is on, its lines ended by `\n`.
    pub fn sieve(&self) -> String {
        let tests: Vec<String> = self.conditions.iter().map(Condition::sieve).collect();
        let test = match tests.as_slice() {
            [one] => one.clone(),
            many => format!("anyof(\n    {}\n)", many.join(",\n    ")),
        };
        format!("if {test}\n{{\n    fileinto {};\n    stop;\n}}", quote(&self.folder))
    }

    /// The whole block, its lines ended by `nl`: switched off, as [`off_block`] writes it.
    pub fn block(&self, nl: &str) -> String {
        let header = self.begin_comment(nl);
        if !self.enabled {
            return off_block(&header, nl);
        }
        format!("{header}{nl}{nl}{}{nl}{nl}{END}", self.sieve().replace('\n', nl))
    }

    /// Whether `between`, the text between its `BEGIN:FILTER` comment and its `END:FILTER`, is
    /// what its settings give, line by line, without the white space lines end and the text begins
    /// and ends with: when it isn't, the block was changed by hand.
    pub fn agrees(&self, between: &str) -> bool {
        let written = if self.enabled { self.sieve() } else { String::new() };
        lines(between) == lines(&written)
    }
}

/// The block of a filter switched off: its `BEGIN:FILTER` comment `header`, then its end.
///
/// **Not seen yet.** How such a webmail writes a filter that is off hasn't been seen in a script:
/// this is MailCraft's reading of it, as [`crate::cmd::sieve_elsewhere`] reads one (`Enabled`
/// false, and no Sieve between the comments). Change it here alone once a sample shows the
/// webmail's own way.
pub fn off_block(header: &str, nl: &str) -> String {
    format!("{header}{nl}{nl}{END}")
}

/// `text`'s lines without the white space each ends with, without the empty lines at either end.
fn lines(text: &str) -> Vec<&str> {
    let all: Vec<&str> = text.lines().map(str::trim_end).collect();
    let first = all.iter().position(|l| !l.is_empty()).unwrap_or(all.len());
    let last = all.iter().rposition(|l| !l.is_empty()).map_or(first, |l| l.saturating_add(1));
    all.get(first..last.max(first)).unwrap_or_default().to_vec()
}

impl Condition {
    fn read(json: &Value) -> Option<Condition> {
        let o = json.as_object()?;
        if o.len() != CONDITION_KEYS.len() || !CONDITION_KEYS.iter().all(|k| o.contains_key(*k)) {
            return None;
        }
        let text = |k: &str| o.get(k).and_then(Value::as_str);
        let (field, kind, value, second) = (text("Field")?, text("Type")?, text("Value")?, text("ValueSecond")?);
        let test = *TESTS.iter().find(|t| t.field == field && t.kind == kind)?;
        let second_ok = if test.field == "Header" { header_name(second) } else { second.is_empty() };
        (second_ok && value_ok(value)).then(|| Condition { test, value: value.to_string(), second: second.to_string() })
    }

    /// The Sieve test it is written as.
    fn sieve(&self) -> String {
        let names = if self.test.field == "Header" { format!("[{}]", quote(&self.second)) } else { RECIPIENT_HEADERS.to_string() };
        format!("{} {names} {}", self.test.sieve, quote(&self.value))
    }

    /// The known condition of `field` and `kind`, with `value` and `second`.
    pub fn new(field: &str, kind: &str, value: &str, second: &str) -> Option<Condition> {
        let test = *TESTS.iter().find(|t| t.field == field && t.kind == kind)?;
        Some(Condition { test, value: value.to_string(), second: second.to_string() })
    }
}

/// A Sieve quoted string of `text` (which has no control characters): `"` and `\` escaped.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars().filter(|c| !c.is_control()) {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The line end a script uses: CRLF when each of its lines ends so, else LF.
pub fn line_end(text: &str) -> &'static str {
    let lf = text.matches('\n').count();
    if lf > 0 && text.matches("\r\n").count() == lf { "\r\n" } else { "\n" }
}

/// `text` with `extension` in its `require`: the first `require` given it, in the shape of its
/// list (`["a","b"]` gains `,"c"`; `"a"` becomes `["a","c"]`), or a `require` of its own before
/// everything when it has none. Unchanged when one already asks for it; `None` when it isn't Sieve.
pub fn with_require(text: &str, extension: &str, nl: &str) -> Option<String> {
    let lexed = crate::sieve::lex(text)?;
    let toks = &lexed.toks;
    let mut first: Option<usize> = None;
    let mut at = 0usize;
    while let Some((tok, _)) = toks.get(at) {
        if matches!(tok, crate::sieve::Tok::Ident(w) if w.eq_ignore_ascii_case("require")) {
            first.get_or_insert(at);
            let mut i = at.saturating_add(1);
            while let Some((t, _)) = toks.get(i) {
                match t {
                    crate::sieve::Tok::Str(s) if s.eq_ignore_ascii_case(extension) => return Some(text.to_string()),
                    crate::sieve::Tok::Punct(';') => break,
                    _ => {}
                }
                i = i.saturating_add(1);
            }
            at = i;
        }
        at = at.saturating_add(1);
    }
    let new = quote(extension);
    let Some(first) = first else {
        return Some(format!("require [{new}];{nl}{nl}{text}"));
    };
    let next = first.saturating_add(1);
    match toks.get(next) {
        Some((crate::sieve::Tok::Str(_), span)) => {
            let old = text.get(span.clone())?;
            Some(format!("{}[{old},{new}]{}", text.get(..span.start)?, text.get(span.end..)?))
        }
        Some((crate::sieve::Tok::Punct('['), _)) => {
            let close = toks.iter().skip(next).position(|(t, _)| *t == crate::sieve::Tok::Punct(']'))?.saturating_add(next);
            let end = toks.get(close)?.1.start;
            // The list's own separator: what stands between its first two strings.
            let sep = match (toks.get(next.saturating_add(1)), toks.get(next.saturating_add(3))) {
                (Some((crate::sieve::Tok::Str(_), a)), Some((crate::sieve::Tok::Str(_), b))) if a.end <= b.start => {
                    text.get(a.end..b.start).filter(|s| s.trim() == ",").unwrap_or(",")
                }
                _ => ",",
            };
            let empty = close == next.saturating_add(1);
            let sep = if empty { "" } else { sep };
            Some(format!("{}{sep}{new}{}", text.get(..end)?, text.get(end..)?))
        }
        _ => None,
    }
}

/// A new filter's id: 32 lowercase letters and digits, as such a webmail gives one, from the
/// SHA-256 of `seed`, and none of `taken`.
pub fn new_id(seed: &str, taken: &[&str]) -> String {
    use sha2::Digest;
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut round: u32 = 0;
    loop {
        let hash = sha2::Sha256::digest(format!("{seed}\u{0}{round}").as_bytes());
        let id: String = hash.iter().take(32).map(|b| char::from(DIGITS.get(usize::from(*b) % DIGITS.len()).copied().unwrap_or(b'0'))).collect();
        if !taken.contains(&id.as_str()) || round >= 1_000 {
            return id;
        }
        round = round.saturating_add(1);
    }
}
