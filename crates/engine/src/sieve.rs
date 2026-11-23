// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sieve script (RFC 5228) MailCraft keeps on an IMAP account's server, through ManageSieve:
//! written from what the owner set, and read back. One script, [`SCRIPT_NAME`], holds what
//! MailCraft keeps there: for now the Automatic Replies (a `vacation` action, RFC 5230; the time
//! range a `currentdate` test, RFC 5260 with RFC 5231's relational match).
//!
//! **Nothing in a value becomes Sieve.** Every value (a reply, an address, a time) goes into the
//! script as one quoted string ([`quoted`]): a quote or a backslash in it is escaped, and a line
//! break is a line break inside the string; nothing else of the language means anything inside
//! one, so no value can end it early and add a command of its own. The reader ([`tokens`]) is the
//! same grammar read back, bounded, and is what the tests check every script against.
//!
//! **What is read back** is only ever the shape MailCraft writes ([`read_vacation`]); a script
//! that has another shape, or was changed elsewhere, is not taken for MailCraft's own (the host
//! compares its digest with the one kept, `mailcraft_model::SieveKept`).

use mailcraft_model::auto_replies::{AutoReplyStatus, ExternalAudience};
use mailcraft_time::{DateTime, Timestamp};

/// The name of MailCraft's script on the server.
pub const SCRIPT_NAME: &str = "mailcraft";
/// The largest script read back, in bytes (what the ManageSieve client takes).
pub const MAX_SCRIPT: usize = 1 << 20;
/// Most tokens read back from one script.
const MAX_TOKENS: usize = 100_000;
/// How many days pass before the same sender is answered again (RFC 5230 §4.1).
pub const DAYS: u32 = 7;
/// The `:handle` of each reply, which is also how the reader tells them apart.
const INSIDE: &str = "mailcraft-internal";
const OUTSIDE: &str = "mailcraft-external";
/// The first line of every script MailCraft writes.
pub(crate) const HEADER: &str = "# MailCraft: automatic replies kept on this server. Changed elsewhere, MailCraft leaves it as it is.";

/// The SHA-256 digest of a script, in lower-case hexadecimal: what an account keeps of the script
/// MailCraft last wrote ([`mailcraft_model::Account::sieve_written`]), as `mailcraft_net::sieve`
/// compares it.
pub fn digest(script: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(script.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// One token of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    /// `:days`, without its colon.
    Tag(String),
    /// A quoted string or a `text:` block, as its value (escapes and dot-stuffing taken out).
    Str(String),
    Num(u64),
    /// `[` `]` `(` `)` `{` `}` `,` `;`
    Punct(char),
}

/// A value as a Sieve quoted string: `"` and `\` escaped, every line break CRLF, and nothing but
/// tabs among the control characters (a NUL could end the string in some readers).
pub fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What [`quoted`] makes of a value, as the reader gives it back: line breaks as `\n` (the
/// reader's own), the control characters it leaves out gone.
pub fn as_written(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The tokens of a script (RFC 5228 §8.1), or `None` for one that isn't Sieve, or is larger than
/// [`MAX_SCRIPT`] or has more than [`MAX_TOKENS`]. Line breaks inside strings come back as `\n`.
pub fn tokens(script: &str) -> Option<Vec<Tok>> {
    if script.len() > MAX_SCRIPT {
        return None;
    }
    let mut out = Vec::new();
    let mut rest = script;
    loop {
        if out.len() > MAX_TOKENS {
            return None;
        }
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        let Some(c) = rest.chars().next() else { return Some(out) };
        match c {
            '#' => rest = rest.find('\n').and_then(|at| rest.get(at..)).unwrap_or(""),
            '/' if rest.starts_with("/*") => {
                let end = rest.get(2..)?.find("*/")?;
                rest = rest.get(end.checked_add(4)?..)?;
            }
            '[' | ']' | '(' | ')' | '{' | '}' | ',' | ';' => {
                out.push(Tok::Punct(c));
                rest = rest.get(1..)?;
            }
            '"' => {
                let (value, after) = quoted_value(rest.get(1..)?)?;
                out.push(Tok::Str(value));
                rest = after;
            }
            ':' => {
                let word = identifier(rest.get(1..)?)?;
                rest = rest.get(word.len().checked_add(1)?..)?;
                out.push(Tok::Tag(word.to_ascii_lowercase()));
            }
            '0'..='9' => {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                let mut n: u64 = rest.get(..digits)?.parse().ok()?;
                rest = rest.get(digits..)?;
                let scale = match rest.chars().next() {
                    Some('K' | 'k') => 1 << 10,
                    Some('M' | 'm') => 1 << 20,
                    Some('G' | 'g') => 1 << 30,
                    _ => 1,
                };
                if scale > 1 {
                    n = n.checked_mul(scale)?;
                    rest = rest.get(1..)?;
                }
                out.push(Tok::Num(n));
            }
            _ => {
                let word = identifier(rest)?;
                rest = rest.get(word.len()..)?;
                if word.eq_ignore_ascii_case("text") && rest.starts_with(':') {
                    let (value, after) = text_block(rest.get(1..)?)?;
                    out.push(Tok::Str(value));
                    rest = after;
                } else {
                    out.push(Tok::Ident(word.to_ascii_lowercase()));
                }
            }
        }
    }
}

/// The identifier at the start of `s` (a letter or `_`, then letters, digits and `_`).
fn identifier(s: &str) -> Option<&str> {
    let first = s.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let len = s.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    s.get(..len)
}

/// A quoted string's value from just after its opening quote, and what follows it.
fn quoted_value(s: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = s.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((value, s.get(at.checked_add(1)?..)?)),
            // `\"` and `\\`; any other escaped character is itself (RFC 5228 §2.4.2).
            '\\' => value.push(chars.next()?.1),
            '\r' => {}
            c => value.push(c),
        }
    }
    None
}

/// A `text:` block from just after its colon: the lines up to the one that is a lone `.`, with a
/// line's leading dot of two taken out (RFC 5228 §2.4.2), and what follows it.
fn text_block(s: &str) -> Option<(String, &str)> {
    // Spaces, and a comment, may follow `text:` on its line.
    let line_end = s.find('\n')?;
    let first = s.get(..line_end)?.trim_matches([' ', '\t', '\r']);
    if !(first.is_empty() || first.starts_with('#')) {
        return None;
    }
    let mut rest = s.get(line_end.checked_add(1)?..)?;
    let mut value = String::new();
    loop {
        let end = rest.find('\n')?;
        let line = rest.get(..end)?.strip_suffix('\r').unwrap_or(rest.get(..end)?);
        rest = rest.get(end.checked_add(1)?..)?;
        if line == "." {
            return Some((value, rest));
        }
        value.push_str(line.strip_prefix('.').filter(|_| line.starts_with("..")).unwrap_or(line));
        value.push('\n');
    }
}

/// The Automatic Replies to write into the script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vacation {
    pub status: AutoReplyStatus,
    /// The time range, for [`AutoReplyStatus::Scheduled`].
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    /// The reply to people at the account's own domain, and to everybody else.
    pub internal: String,
    pub external: String,
    /// Who outside gets one: [`ExternalAudience::ContactsOnly`] can't be written (the server
    /// doesn't have the owner's contacts), and is refused before it gets here.
    pub audience: ExternalAudience,
}

/// The account a script is written for: its domain ("inside the organisation") and its own
/// addresses (which the server takes as the owner's, RFC 5230 §4.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    pub domain: String,
    pub addresses: Vec<String>,
}

/// `at` as the script compares it with the server's clock: `YYYY-MM-DDTHH:MM:SS`, in UTC (the
/// `currentdate` test is told the zone `+0000`; the server's own value then carries `+00:00`
/// after it, which compares as later than the bare time, so a start is reached on its second
/// and an end is past on its second).
fn utc(at: Timestamp) -> String {
    DateTime::from_timestamp(at, 0).iso()
}

/// The script for `vacation` on `owner`'s account. The extensions it needs are the caller's to
/// check: `vacation`, and `date` and `relational` for a time range.
pub fn vacation_script(vacation: &Vacation, owner: &Owner) -> String {
    let mut out = format!("{HEADER}\r\n");
    let scheduled = vacation.status == AutoReplyStatus::Scheduled && vacation.start.is_some() && vacation.end.is_some();
    match vacation.status {
        AutoReplyStatus::Disabled => {
            out.push_str("# Automatic replies are off.\r\nkeep;\r\n");
            return out;
        }
        _ if scheduled => out.push_str("require [\"vacation\", \"date\", \"relational\"];\r\n"),
        _ => out.push_str("require [\"vacation\"];\r\n"),
    }
    let addresses: Vec<String> = owner.addresses.iter().map(|a| quoted(a)).collect();
    let reply = |handle: &str, text: &str| {
        format!("vacation :days {DAYS} :addresses [{}] :handle {} {};\r\n", addresses.join(", "), quoted(handle), quoted(text))
    };
    let mut body = String::new();
    let outside = vacation.audience != ExternalAudience::None;
    if owner.domain.is_empty() {
        // No domain to tell the organisation by: everybody gets the reply for outside, if anyone.
        if outside {
            body.push_str(&reply(OUTSIDE, &vacation.external));
        }
    } else {
        body.push_str(&format!("if address :domain :is \"from\" {} {{\r\n    {}}}", quoted(&owner.domain), reply(INSIDE, &vacation.internal)));
        if outside {
            body.push_str(&format!(" else {{\r\n    {}}}", reply(OUTSIDE, &vacation.external)));
        }
        body.push_str("\r\n");
    }
    if let (true, Some(start), Some(end)) = (scheduled, vacation.start, vacation.end) {
        out.push_str(&format!(
            "if allof (currentdate :zone \"+0000\" :value \"ge\" \"iso8601\" {}, currentdate :zone \"+0000\" :value \"lt\" \"iso8601\" {}) {{\r\n",
            quoted(&utc(start)),
            quoted(&utc(end))
        ));
        for line in body.split_inclusive("\r\n") {
            out.push_str("    ");
            out.push_str(line);
        }
        out.push_str("}\r\n");
    } else {
        out.push_str(&body);
    }
    out
}

/// The Automatic Replies a script MailCraft wrote holds, read back through [`tokens`]; `None` for
/// a script that isn't one of MailCraft's shape.
pub fn read_vacation(script: &str) -> Option<Vacation> {
    if !script.starts_with(HEADER) {
        return None;
    }
    let tokens = tokens(script)?;
    let mut vacation = Vacation {
        status: AutoReplyStatus::Disabled,
        start: None,
        end: None,
        internal: String::new(),
        external: String::new(),
        audience: ExternalAudience::None,
    };
    let mut replies = 0;
    let mut at = 0;
    while let Some(token) = tokens.get(at) {
        match token {
            Tok::Ident(word) if word == "vacation" => {
                // `vacation` and its arguments, up to the `;`: the handle says which reply it is,
                // and the last string is the reply.
                let end = tokens.get(at..)?.iter().position(|t| *t == Tok::Punct(';'))?.checked_add(at)?;
                let args = tokens.get(at.checked_add(1)?..end)?;
                let handle = args.iter().position(|t| *t == Tok::Tag("handle".into())).and_then(|i| args.get(i.checked_add(1)?));
                let Some(Tok::Str(text)) = args.last() else { return None };
                match handle {
                    Some(Tok::Str(h)) if h == INSIDE => vacation.internal = text.clone(),
                    Some(Tok::Str(h)) if h == OUTSIDE => {
                        vacation.external = text.clone();
                        vacation.audience = ExternalAudience::All;
                    }
                    _ => return None,
                }
                replies += 1;
                at = end;
            }
            Tok::Ident(word) if word == "currentdate" => {
                // currentdate :zone "+0000" :value "ge"|"lt" "iso8601" "<time>"
                let args = tokens.get(at.checked_add(1)?..at.checked_add(7)?)?;
                let (Some(Tok::Str(how)), Some(Tok::Str(time))) = (args.get(3), args.get(5)) else { return None };
                let (wall, _) = mailcraft_time::parse_iso(time)?;
                let instant = wall.to_timestamp(0);
                match how.as_str() {
                    "ge" => vacation.start = Some(instant),
                    "lt" => vacation.end = Some(instant),
                    _ => return None,
                }
                at = at.checked_add(7)?;
            }
            _ => at = at.checked_add(1)?,
        }
    }
    vacation.status = match (replies, vacation.start, vacation.end) {
        (0, _, _) => AutoReplyStatus::Disabled,
        (_, Some(_), Some(_)) => AutoReplyStatus::Scheduled,
        _ => AutoReplyStatus::AlwaysEnabled,
    };
    Some(vacation)
}
