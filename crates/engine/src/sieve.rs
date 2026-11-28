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

use std::ops::Range;

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

/// A string list (`["a", "b"]`) of values, each one [`quoted`].
pub fn list<S: AsRef<str>>(items: &[S]) -> String {
    let items: Vec<String> = items.iter().map(|i| quoted(i.as_ref())).collect();
    format!("[{}]", items.join(", "))
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
    Some(lex(script)?.toks.into_iter().map(|(t, _)| t).collect())
}

/// A comment of a script, where it is: its bytes, from its `#` or `/*` to the end of its line
/// (the line break not included) or past its `*/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub at: Range<usize>,
    /// A `/* … */` one (else a `#` one).
    pub bracketed: bool,
}

impl Comment {
    /// What it says, without its `#` or `/*` and `*/`: a part of `script`, the text it was read
    /// from (empty for another text).
    pub fn text<'a>(&self, script: &'a str) -> &'a str {
        let inner = if self.bracketed {
            self.at.start.checked_add(2).zip(self.at.end.checked_sub(2)).filter(|(a, b)| a <= b)
        } else {
            self.at.start.checked_add(1).map(|a| (a, self.at.end)).filter(|(a, b)| a <= b)
        };
        inner.and_then(|(a, b)| script.get(a..b)).unwrap_or_default()
    }
}

/// A script's tokens with where each is (its bytes), and its comments, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lexed {
    pub toks: Vec<(Tok, Range<usize>)>,
    pub comments: Vec<Comment>,
}

/// [`tokens`] with where each is, and the comments between them (counted with the tokens towards
/// [`MAX_TOKENS`]).
pub fn lex(script: &str) -> Option<Lexed> {
    lex_or_why(script).ok()
}

/// Why a script couldn't be read into tokens ([`lex_or_why`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexFault {
    /// Larger than [`MAX_SCRIPT`].
    TooLarge,
    /// More than [`MAX_TOKENS`] tokens and comments.
    TooMany,
    /// A `/*` with no `*/`.
    Comment,
    /// A `"` with no closing one.
    String,
    /// A `text:` block with no line that is a lone `.`.
    Text,
    /// Something other than spaces and a comment after `text:` on its line.
    TextStart,
    /// A `:` that no name follows.
    Tag,
    /// A number larger than 2⁶⁴ − 1 (with its K, M or G).
    Number,
    /// A character that begins no token.
    Char,
}

/// Where a script stopped being Sieve, and why: `at` is the bytes of what is wrong (the opening
/// `"`, `/*` or `text:` of what isn't closed), on character boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub at: Range<usize>,
    pub fault: LexFault,
}

/// [`lex`], saying where and why a script that isn't Sieve stops being it.
pub fn lex_or_why(script: &str) -> Result<Lexed, LexError> {
    if script.len() > MAX_SCRIPT {
        return Err(LexError { at: 0..0, fault: LexFault::TooLarge });
    }
    let mut out = Lexed::default();
    let mut rest = script;
    let at = |rest: &str| script.len().saturating_sub(rest.len());
    loop {
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        let start = at(rest);
        if out.toks.len().saturating_add(out.comments.len()) > MAX_TOKENS {
            return Err(LexError { at: start..start, fault: LexFault::TooMany });
        }
        let Some(c) = rest.chars().next() else { return Ok(out) };
        let one = start..start.saturating_add(c.len_utf8());
        let fail = |fault: LexFault, len: usize| LexError { at: start..start.saturating_add(len), fault };
        let char_fault = LexError { at: one.clone(), fault: LexFault::Char };
        match c {
            '#' => {
                let line = rest.find('\n').unwrap_or(rest.len());
                let line = if rest.get(..line).is_some_and(|l| l.ends_with('\r')) { line.saturating_sub(1) } else { line };
                rest = rest.get(line..).ok_or(char_fault)?;
                out.comments.push(Comment { at: start..at(rest), bracketed: false });
                continue;
            }
            '/' if rest.starts_with("/*") => {
                let end = rest.get(2..).and_then(|r| r.find("*/")).ok_or(fail(LexFault::Comment, 2))?;
                rest = end.checked_add(4).and_then(|e| rest.get(e..)).ok_or(fail(LexFault::Comment, 2))?;
                out.comments.push(Comment { at: start..at(rest), bracketed: true });
                continue;
            }
            '[' | ']' | '(' | ')' | '{' | '}' | ',' | ';' => {
                rest = rest.get(1..).ok_or(char_fault)?;
                out.toks.push((Tok::Punct(c), start..at(rest)));
            }
            '"' => {
                let (value, after) = rest.get(1..).and_then(quoted_value).ok_or(fail(LexFault::String, 1))?;
                rest = after;
                out.toks.push((Tok::Str(value), start..at(rest)));
            }
            ':' => {
                let word = rest.get(1..).and_then(identifier).ok_or(fail(LexFault::Tag, 1))?;
                rest = word.len().checked_add(1).and_then(|e| rest.get(e..)).ok_or(fail(LexFault::Tag, 1))?;
                out.toks.push((Tok::Tag(word.to_ascii_lowercase()), start..at(rest)));
            }
            '0'..='9' => {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                let number = fail(LexFault::Number, digits);
                let mut n: u64 = rest.get(..digits).and_then(|d| d.parse().ok()).ok_or(number.clone())?;
                rest = rest.get(digits..).ok_or(number.clone())?;
                let scale = match rest.chars().next() {
                    Some('K' | 'k') => 1 << 10,
                    Some('M' | 'm') => 1 << 20,
                    Some('G' | 'g') => 1 << 30,
                    _ => 1,
                };
                if scale > 1 {
                    n = n.checked_mul(scale).ok_or(fail(LexFault::Number, digits.saturating_add(1)))?;
                    rest = rest.get(1..).ok_or(number)?;
                }
                out.toks.push((Tok::Num(n), start..at(rest)));
            }
            _ => {
                let word = identifier(rest).ok_or(char_fault)?;
                rest = rest.get(word.len()..).ok_or(fail(LexFault::Char, word.len()))?;
                if word.eq_ignore_ascii_case("text") && rest.starts_with(':') {
                    let (value, after) = text_block(rest.get(1..).unwrap_or_default()).map_err(|fault| fail(fault, 5))?;
                    rest = after;
                    out.toks.push((Tok::Str(value), start..at(rest)));
                } else {
                    out.toks.push((Tok::Ident(word.to_ascii_lowercase()), start..at(rest)));
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
fn text_block(s: &str) -> Result<(String, &str), LexFault> {
    // Spaces, and a comment, may follow `text:` on its line.
    let line_end = s.find('\n').ok_or(LexFault::Text)?;
    let first = s.get(..line_end).unwrap_or_default().trim_matches([' ', '\t', '\r']);
    if !(first.is_empty() || first.starts_with('#')) {
        return Err(LexFault::TextStart);
    }
    let mut rest = line_end.checked_add(1).and_then(|e| s.get(e..)).ok_or(LexFault::Text)?;
    let mut value = String::new();
    loop {
        let end = rest.find('\n').ok_or(LexFault::Text)?;
        let whole = rest.get(..end).ok_or(LexFault::Text)?;
        let line = whole.strip_suffix('\r').unwrap_or(whole);
        rest = end.checked_add(1).and_then(|e| rest.get(e..)).ok_or(LexFault::Text)?;
        if line == "." {
            return Ok((value, rest));
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

/// A rule of the owner's as one block of the script ([`crate::cmd::sieve_rules`]): its id (said
/// in a comment above it), its Sieve, ending with a line break, and the extensions it needs. A
/// Sieve rule's name is said in a comment of its own too (`name`), so that another program
/// showing the script shows it; empty for the other rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleScript {
    pub id: mailcraft_model::Id,
    pub text: String,
    pub requires: Vec<String>,
    pub name: String,
}

/// The longest name said in a rule's comment, in characters.
const MAX_COMMENT_NAME: usize = 200;

/// `name` as a `#` comment's text can hold it (#2057): one line, with no control or format
/// character, no line or paragraph separator, and no `*/` (should the comment ever be read as a
/// bracketed one), at most [`MAX_COMMENT_NAME`] characters.
pub fn comment_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !mailcraft_names::is_format(*c))
        .take(MAX_COMMENT_NAME)
        .collect();
    // ("* /" makes no new "*/": the space parts what it joins.)
    kept.replace("*/", "* /").trim().to_string()
}

/// `text` with every line break CRLF, as a script has them, and ending with one: a `#` comment on
/// its last line then ends there, and can't take in what follows it.
pub fn crlf_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    if !out.ends_with("\r\n") {
        out.push_str("\r\n");
    }
    out
}

/// The extensions a script MailCraft writes may need, in the order its `require` names them.
pub const EXTENSIONS: [&str; 7] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags", "body"];

/// The script for `vacation` on `owner`'s account, without rules.
pub fn vacation_script(vacation: &Vacation, owner: &Owner) -> String {
    script(vacation, owner, &[])
}

/// MailCraft's whole script: the Automatic Replies first, so that no rule's `stop` can keep a
/// reply from going (as in Outlook, where rules don't stop the out-of-office reply), then the
/// rules in their order. One writer for all of it: a change of the replies keeps the rules, and a
/// change of a rule keeps the replies. The extensions it needs are the caller's to check.
pub fn script(vacation: &Vacation, owner: &Owner, rules: &[RuleScript]) -> String {
    let mut out = format!("{HEADER}\r\n");
    let scheduled = vacation.status == AutoReplyStatus::Scheduled && vacation.start.is_some() && vacation.end.is_some();
    let replies = vacation.status != AutoReplyStatus::Disabled;
    let wanted = |e: &str| rules.iter().any(|r| r.requires.iter().any(|x| x == e));
    let mut needed: Vec<&str> = EXTENSIONS
        .into_iter()
        .filter(|e| match *e {
            "vacation" => replies || wanted(e),
            "date" | "relational" => (replies && scheduled) || wanted(e),
            other => wanted(other),
        })
        .collect();
    // Then what a Sieve rule needs besides, each once, in the order the rules first need it: one
    // `require` for the whole script (RFC 5228 §3.2), before any other command.
    for e in rules.iter().flat_map(|r| &r.requires) {
        if !needed.contains(&e.as_str()) {
            needed.push(e);
        }
    }
    if !needed.is_empty() {
        let names: Vec<String> = needed.iter().map(|e| quoted(e)).collect();
        out.push_str(&format!("require [{}];\r\n", names.join(", ")));
    }
    if replies {
        out.push_str(&vacation_section(vacation, owner, scheduled));
    } else {
        out.push_str("# Automatic replies are off.\r\n");
    }
    for rule in rules {
        out.push_str(&rule_section(rule));
    }
    out
}

/// A rule's part of the script: the comment with its id, the one with a Sieve rule's name, then
/// its Sieve, ending with a line break.
pub fn rule_section(rule: &RuleScript) -> String {
    let mut out = format!("# rule {}\r\n", rule.id);
    let name = comment_name(&rule.name);
    if !name.is_empty() {
        out.push_str(&format!("# {RULE_NAME}{name}\r\n"));
    }
    out.push_str(&crlf_lines(&rule.text));
    out
}

/// What the comment naming a Sieve rule starts with, after its `# `.
const RULE_NAME: &str = "Sieve rule: ";

/// The replies, to go first in the script.
fn vacation_section(vacation: &Vacation, owner: &Owner, scheduled: bool) -> String {
    let mut out = String::new();
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
    // The replies' section alone: it ends where the first rule begins (`# rule <id>`), and a
    // Sieve rule after it may have a `vacation` of its own (#2057).
    let lexed = lex(script)?;
    let rules_from =
        lexed.comments.iter().find(|c| !c.bracketed && c.text(script).trim_start().starts_with("rule ")).map_or(usize::MAX, |c| c.at.start);
    let tokens: Vec<Tok> = lexed.toks.into_iter().take_while(|(_, at)| at.start < rules_from).map(|(t, _)| t).collect();
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
