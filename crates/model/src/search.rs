// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box's query language: words, `"exact phrases"`, and `keyword:value` filters.
//!
//! `budget from:ada has:attachment -draft after:2026-10-01`. Every term must match; `-` in front
//! of a term excludes what it matches. Keywords: `from`, `to`, `cc`, `subject`, `body`,
//! `category`, `attachment` (file name), `has:attachment`, `is:unread|read|flagged|important`,
//! `importance:high|normal|low`, `before:`, `after:`, `on:` (dates, `YYYY-MM-DD` or `M/D/YYYY`).
//! A space after a keyword's colon is allowed (`from: ada`); a keyword with nothing after it yet
//! asks for nothing. Inside double quotes, `\"` is a double quote and `\\` a backslash.
//!
//! A word matches the start of a word, ignoring case and accents (`meet` finds "Meeting",
//! `gvozdar` finds "Gvožđar"); a phrase, its words one after another ([`crate::words`]). `from:`,
//! `to:` and `cc:` with a whole address (`from:ada@example.org`, or with a quoted name,
//! `from:"\"ada smith\"@example.org"`) match that address and no other, ignoring case (#301): an
//! international domain written in `xn--` form and the same domain in its own letters are two
//! addresses (`ada@xn--bcher-kva.example` doesn't find `ada@bücher.example`). A category matches
//! anywhere in its name. Text with no letters or digits in it (`-`, `@`) asks for nothing.

use std::ops::Range;

use mailcraft_time::{Date, DateTime};

use crate::words::{self, ATTACHMENT, BODY, CC, FROM, SUBJECT, TO};
use crate::{FlagStatus, Importance, Message, contains_ci};

/// Longest query read and most terms kept (the rest is ignored).
const MAX_QUERY: usize = 2_000;
const MAX_TERMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// Words, one after another, in the parts of a message `fields` names.
    Words {
        fields: u8,
        words: Vec<String>,
        /// The same words as the owner wrote them (case and accents as typed), for a server's
        /// search ([`Query::for_server`]).
        raw: Vec<String>,
    },
    /// One whole address, in small letters, in the parts of a message `fields` names (`FROM`,
    /// `TO`, `CC`): the address itself, not words of it (#301). Its `words` are what a word
    /// index narrows the messages by before the address is compared.
    Address {
        fields: u8,
        address: String,
        words: Vec<String>,
    },
    Category(String),
    HasAttachment,
    Unread,
    Read,
    Flagged,
    Importance(Importance),
    Before(Date),
    After(Date),
    On(Date),
}

impl Kind {
    /// Whether it asks for nothing: words (or an address) with no letters or digits in them.
    fn asks_nothing(&self) -> bool {
        matches!(self, Kind::Words { words, .. } | Kind::Address { words, .. } if words.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    negate: bool,
    kind: Kind,
}

/// A parsed search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    terms: Vec<Term>,
}

/// Split on spaces outside double quotes; the quotes are dropped, and inside them `\"` is a
/// double quote and `\\` a backslash ([`keyword_value`]). Each token with where it stands in `s`
/// (bytes, its quotes included).
fn tokens(s: &str) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    let (mut start, mut end) = (0, 0);
    let mut chars = s.char_indices().take(MAX_QUERY).peekable();
    while let Some((i, c)) = chars.next() {
        // (Inside quotes the token has begun already: at its opening quote.)
        if c == '\\'
            && quoted
            && let Some((j, escaped)) = chars.next_if(|(_, next)| matches!(next, '"' | '\\'))
        {
            end = j.saturating_add(escaped.len_utf8());
            cur.push(escaped);
            continue;
        }
        if c.is_whitespace() && !quoted {
            if any || !cur.is_empty() {
                out.push((std::mem::take(&mut cur), start..end));
            }
            any = false;
            continue;
        }
        if !any && cur.is_empty() {
            start = i;
        }
        end = i.saturating_add(c.len_utf8());
        if c == '"' {
            quoted = !quoted;
            any = true;
        } else {
            cur.push(c);
        }
    }
    if any || !cur.is_empty() {
        out.push((cur, start..end));
    }
    out
}

/// A keyword and its value as they stand in the text of a query (bytes): what the search box
/// draws as one piece (#196). `key` is the keyword and its colon (after a `-`, when there is one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeywordSpan {
    pub range: Range<usize>,
    pub key: Range<usize>,
}

/// The keywords of `s` that [`Query::parse`] reads as keywords, with their values, in order: not
/// `re: budget`, `is:banana` or a keyword with nothing after it yet, which it reads as text or as
/// nothing.
pub fn keyword_spans(s: &str) -> Vec<KeywordSpan> {
    let mut out = Vec::new();
    let mut tokens = tokens(s).into_iter().take(MAX_TERMS).peekable();
    while let Some((token, at)) = tokens.next() {
        let body = token.strip_prefix('-').filter(|rest| !rest.is_empty()).unwrap_or(&token);
        let (body, range) = match body.strip_suffix(':') {
            Some(key) if is_keyword(key) => match tokens.next_if(|(next, _)| !starts_with_keyword(next)) {
                Some((value, next)) => (format!("{body}{value}"), at.start..next.end),
                None => continue,
            },
            _ => (body.to_string(), at.clone()),
        };
        let shown = keyword_kind(&body).is_some_and(|kind| !kind.asks_nothing());
        let colon = s.get(at.clone()).and_then(|t| t.find(':')).map(|c| at.start.saturating_add(c).saturating_add(1));
        if let Some(colon) = colon.filter(|_| shown) {
            let key_start = if s.get(at.start..).is_some_and(|t| t.starts_with('-')) { at.start.saturating_add(1) } else { at.start };
            out.push(KeywordSpan { range, key: key_start..colon });
        }
    }
    out
}

/// Where the search box draws a pill (#196): around a keyword and its colon (after a `-`, when
/// there is one: `-is:`), never its value. A pill is a keyword [`keyword_spans`] finds, or a
/// known keyword whose value is still being typed: nothing after its colon yet at the end of the
/// query, or a value the cursor (a byte offset, `None` while the box isn't being edited) is in or
/// at the end of, so `is:u` on the way to `is:unread` keeps its pill. Bytes, in order.
pub fn keyword_pills(s: &str, cursor: Option<usize>) -> Vec<Range<usize>> {
    let read: Vec<usize> = keyword_spans(s).into_iter().map(|k| k.range.start).collect();
    let all: Vec<(String, Range<usize>)> = tokens(s).into_iter().take(MAX_TERMS).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some((_, at)) = all.get(i) {
        i = i.saturating_add(1);
        let raw = s.get(at.clone()).unwrap_or_default();
        let dash = usize::from(raw.starts_with('-') && raw.len() > 1);
        let body = raw.get(dash..).unwrap_or_default();
        let Some(colon) = body.find(':').filter(|&c| body.get(..c).is_some_and(is_keyword)) else { continue };
        let head = at.start..at.start.saturating_add(dash).saturating_add(colon).saturating_add(1);
        // `from: stuart`: the value is the next word, unless that is a keyword of its own.
        let mut end = at.end;
        let mut last = i >= all.len();
        if head.end == at.end
            && let Some((_, next)) = all.get(i).filter(|(next, _)| !starts_with_keyword(next))
        {
            end = next.end;
            i = i.saturating_add(1);
            last = false;
        }
        let typing = cursor.is_some_and(|c| at.start <= c && c <= end);
        if read.contains(&at.start) || typing || (last && head.end == end) {
            out.push(head);
        }
    }
    out
}

/// What `body` (`key:value`) asks for when its key is a keyword the search box knows and its value
/// one the keyword takes; `None`: it is text.
fn keyword_kind(body: &str) -> Option<Kind> {
    let (key, value) = body.split_once(':').filter(|(_, v)| !v.is_empty())?;
    let v = value.to_string();
    let words_in = |fields: u8, v: &str| Kind::Words { fields, words: words::words(v), raw: raw_words(v) };
    let address_in = |fields: u8, v: &str| match is_address(v) {
        true => Kind::Address { fields, address: address_key(v), words: words::words(v) },
        false => words_in(fields, v),
    };
    Some(match key.to_ascii_lowercase().as_str() {
        "from" => address_in(FROM, &v),
        "to" => address_in(TO, &v),
        "cc" => address_in(CC, &v),
        "subject" => words_in(SUBJECT, &v),
        "body" => words_in(BODY, &v),
        "category" | "categories" => Kind::Category(v),
        "attachment" | "attachments" => words_in(ATTACHMENT, &v),
        "has" | "hasattachment" | "hasattachments" => match v.to_ascii_lowercase().as_str() {
            "attachment" | "attachments" | "yes" | "true" => Kind::HasAttachment,
            _ => return None,
        },
        "is" => match v.to_ascii_lowercase().as_str() {
            "unread" => Kind::Unread,
            "read" => Kind::Read,
            "flagged" => Kind::Flagged,
            "important" => Kind::Importance(Importance::High),
            _ => return None,
        },
        "importance" => Kind::Importance(Importance::parse(&v)?),
        "before" => Kind::Before(parse_date(&v)?),
        "after" => Kind::After(parse_date(&v)?),
        "on" | "received" | "sent" => Kind::On(parse_date(&v)?),
        // `10:30`, `re: budget`, a URL: plain text.
        _ => return None,
    })
}

/// An address as two of them are compared: trimmed, with ASCII letters in small letters only
/// (other letters, accents and an international domain's `xn--` form are left as they are).
pub fn address_key(v: &str) -> String {
    v.trim().to_ascii_lowercase()
}

/// Whether `v` is one whole address and nothing else (`name@domain`): a name of parts between
/// dots, each of letters (accents too), digits and `!#$%&'*+-/=?^_`{|}~`, or a name in double
/// quotes (`"ada smith"@example.org`, with `\"` and `\\` inside), an `@`, and a domain of parts
/// between dots, each of letters, digits and `-` that starts and ends with a letter or digit.
/// Only such a value after `from:`, `to:` or `cc:` is an address; one with more round it
/// (`<ada@example.org>`, `ada@example.org,`) is words, as before.
pub fn is_address(v: &str) -> bool {
    let letter = |c: char| c.is_alphanumeric() || words::is_mark(c);
    let atom = |part: &str| !part.is_empty() && part.chars().all(|c| letter(c) || "!#$%&'*+-/=?^_`{|}~".contains(c));
    let label = |part: &str| {
        part.chars().next().is_some_and(char::is_alphanumeric)
            && part.chars().last().is_some_and(letter)
            && part.chars().all(|c| letter(c) || c == '-')
    };
    let domain = match v.strip_prefix('"') {
        Some(quoted) => after_quotes(quoted).and_then(|rest| rest.strip_prefix('@')),
        None => v.split_once('@').filter(|(name, _)| name.split('.').all(atom)).map(|(_, domain)| domain),
    };
    domain.is_some_and(|domain| domain.split('.').all(label)) && words::words(v).len() >= 2
}

/// What follows the closing quote of `quoted` (the text after an opening one), where `\` makes
/// the character after it part of the name; `None` when it isn't closed or has a control
/// character in it.
fn after_quotes(quoted: &str) -> Option<&str> {
    let mut escaped = false;
    for (i, c) in quoted.char_indices() {
        if c.is_control() {
            return None;
        }
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return quoted.get(i.saturating_add(1)..),
            _ => {}
        }
    }
    None
}

/// `value` as it is to be written after a keyword for the search to read it back as it is: in
/// double quotes, with `"` and `\` in it written `\"` and `\\`, when it has a space, a quote or a
/// backslash in it (`from:"\"ada smith\"@example.org"`); otherwise as it is.
pub fn keyword_value(value: &str) -> String {
    if !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len().saturating_add(2));
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Whether a server's search takes `word` as it is: letters, digits and `@._-'+`, which keep an
/// address whole ([`raw_words`]), and no longer than a word that goes to a server.
fn server_takes(word: &str) -> bool {
    !word.is_empty() && word.chars().count() <= MAX_SERVER_WORD && word.chars().all(|c| c.is_alphanumeric() || SERVER_WORD.contains(c))
}

/// What a word for a server's search may have besides letters and digits (`+` only inside an
/// address, `ada+news@example.org`).
const SERVER_WORD: &str = "@._-'+";

/// Most words of one term, and most characters of one word, that go to a server.
const MAX_SERVER_WORDS: usize = 8;
const MAX_SERVER_WORD: usize = 64;
/// Most terms that go to a server; the rest are checked here.
const MAX_SERVER_TERMS: usize = 16;

/// The words of `text` as a server's search is to be given them: runs of letters, digits and
/// `@._-'` (so that an address stays whole), each trimmed of what isn't a letter or a digit at
/// its ends, in the case and with the accents they were typed in. A `+` parts words, except in a
/// whole address (`ada+news@example.org`, #301), which is one word.
fn raw_words(text: &str) -> Vec<String> {
    let trim = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    text.split(|c: char| !(c.is_alphanumeric() || SERVER_WORD.contains(c)))
        .map(trim)
        .flat_map(|run| match is_address(&run) {
            true => vec![run],
            false => run.split('+').map(trim).collect(),
        })
        .filter(|w| !w.is_empty())
        .take(MAX_SERVER_WORDS)
        .map(|w| w.chars().take(MAX_SERVER_WORD).collect())
        .collect()
}

/// Whether a server was given less of a term than was typed (as many words as it takes, or a word
/// as long, and so perhaps cut short) in parts of a message that a message found on the server
/// has here whole: its people and its subject. Such a term is looked at here too; one in the
/// body or an attachment's name isn't, as such a message has only a preview of its body here and
/// no attachments, so looking would drop what the server rightly found.
fn cut_short(fields: u8, raw: &[String]) -> bool {
    let whole_here = fields & !(FROM | TO | CC | SUBJECT) == 0;
    whole_here && (raw.len() >= MAX_SERVER_WORDS || raw.iter().any(|w| w.chars().count() >= MAX_SERVER_WORD))
}

/// The part of a message a server's search is asked to look in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerField {
    From,
    To,
    Cc,
    Subject,
    Body,
    Attachment,
}

/// A term of a query a server's search understands (Microsoft Graph's, R6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerTerm {
    /// Words one after another (a phrase, when there are several), as the owner wrote them: in
    /// one part of a message, or (`None`) wherever the server looks by itself.
    Words {
        field: Option<ServerField>,
        words: Vec<String>,
    },
    HasAttachment,
    /// High or low (a server's search has no word for normal).
    Importance(Importance),
}

/// A query split for a server: what it understands, and what is left to check here against what
/// comes back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerQuery {
    pub terms: Vec<ServerTerm>,
    pub local: Query,
}

fn parse_date(s: &str) -> Option<Date> {
    if let Some(d) = Date::parse_iso(s) {
        return Some(d);
    }
    let mut parts = s.split('/');
    let (m, d, y) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    Date::new(y, m, d)
}

/// The keywords the search box knows (what comes before a `:`), in small letters.
const KEYWORDS: &[&str] = &[
    "from",
    "to",
    "cc",
    "subject",
    "body",
    "category",
    "categories",
    "attachment",
    "attachments",
    "has",
    "hasattachment",
    "hasattachments",
    "is",
    "importance",
    "before",
    "after",
    "on",
    "received",
    "sent",
];

fn is_keyword(key: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// Whether `token` (with a `-` in front or not) is a known keyword and its colon: `subject:`,
/// `-from:ada`.
fn starts_with_keyword(token: &str) -> bool {
    let body = token.strip_prefix('-').unwrap_or(token);
    body.split_once(':').is_some_and(|(key, _)| is_keyword(key))
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut terms = Vec::new();
        let mut tokens = tokens(s).into_iter().map(|(token, _)| token).take(MAX_TERMS).peekable();
        while let Some(token) = tokens.next() {
            let (negate, body) = match token.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, token.as_str()),
            };
            // `from: stuart` is `from:stuart`, as Outlook reads it: a keyword with nothing after
            // its colon takes what comes next. With nothing next (the owner is still typing it),
            // or another keyword, it asks for nothing (#194).
            let joined;
            let body = match body.strip_suffix(':') {
                Some(key) if is_keyword(key) => match tokens.next_if(|next| !starts_with_keyword(next)) {
                    Some(value) => {
                        joined = format!("{body}{value}");
                        joined.as_str()
                    }
                    None => continue,
                },
                _ => body,
            };
            let kind = keyword_kind(body).unwrap_or_else(|| Kind::Words { fields: words::ALL, words: words::words(body), raw: raw_words(body) });
            // Text with no words in it asks for nothing.
            if !kind.asks_nothing() {
                terms.push(Term { negate, kind });
            }
        }
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term reads a message's text (a bare word, or `body:`): such a search wants the
    /// bodies of the messages it looks at in memory.
    pub fn reads_bodies(&self) -> bool {
        (0..self.terms.len()).any(|i| self.term_reads_body(i))
    }

    /// How many terms it has.
    pub fn len(&self) -> usize {
        self.terms.len()
    }

    /// Term `i`, when it is words: the parts of a message it looks in ([`words::ALL`] for a bare
    /// word) and its words, folded, one after another. Other terms (`is:`, dates, `category:`)
    /// say `None`.
    pub fn words_of(&self, i: usize) -> Option<(u8, &[String])> {
        match &self.terms.get(i)?.kind {
            Kind::Words { fields, words, .. } | Kind::Address { fields, words, .. } => Some((*fields, words)),
            _ => None,
        }
    }

    /// Whether term `i` is a whole address (`from:ada@example.org`): its words only narrow the
    /// messages, and the address itself is looked at on each.
    pub fn is_address_term(&self, i: usize) -> bool {
        matches!(self.terms.get(i).map(|t| &t.kind), Some(Kind::Address { .. }))
    }

    /// Whether term `i` reads a message's text.
    pub fn term_reads_body(&self, i: usize) -> bool {
        self.words_of(i).is_some_and(|(fields, _)| fields & BODY != 0)
    }

    /// The query split for a server's search (R6.2): words, phrases and `from:`, `to:`, `cc:`,
    /// `subject:`, `body:` and `attachment:` with them, `has:attachment`, and `importance:` high
    /// or low go to the server; a term with `-` in front, `is:`, `category:`, dates, normal
    /// importance, and whatever is past [`MAX_SERVER_TERMS`] are left to check here. A message the
    /// server finds for the first matches the whole query when it matches the second.
    pub fn for_server(&self) -> ServerQuery {
        let mut out = ServerQuery::default();
        for term in &self.terms {
            let sent = match (&term.kind, term.negate) {
                (_, true) => None,
                // A server reads a whole address after `from:`, `to:` or `cc:` as that address,
                // where here such a term is words (`from:<ada@example.org>` finds
                // adam@example.org): it is looked at here alone.
                (Kind::Words { fields: FROM | TO | CC, raw, .. }, false) if raw.iter().any(|w| is_address(w)) => None,
                (Kind::Words { fields, raw, .. }, false) if !raw.is_empty() => {
                    let field = match *fields {
                        words::ALL => Some(None),
                        FROM => Some(Some(ServerField::From)),
                        TO => Some(Some(ServerField::To)),
                        CC => Some(Some(ServerField::Cc)),
                        SUBJECT => Some(Some(ServerField::Subject)),
                        BODY => Some(Some(ServerField::Body)),
                        ATTACHMENT => Some(Some(ServerField::Attachment)),
                        _ => None,
                    };
                    field.map(|field| ServerTerm::Words { field, words: raw.clone() })
                }
                // An address a server's search can't be given as it is (one with `!#$%…` or too
                // long) is looked at here alone.
                (Kind::Address { fields, address, .. }, false) if server_takes(address) => {
                    let field = match *fields {
                        FROM => Some(ServerField::From),
                        TO => Some(ServerField::To),
                        CC => Some(ServerField::Cc),
                        _ => None,
                    };
                    field.map(|field| ServerTerm::Words { field: Some(field), words: vec![address.clone()] })
                }
                (Kind::HasAttachment, false) => Some(ServerTerm::HasAttachment),
                (Kind::Importance(i @ (Importance::High | Importance::Low)), false) => Some(ServerTerm::Importance(*i)),
                _ => None,
            };
            match sent {
                Some(sent) if out.terms.len() < MAX_SERVER_TERMS => {
                    out.terms.push(sent);
                    // A whole address is looked at here too: a server's search may find it
                    // inside a longer one; and so are words the server was given less of.
                    let cut = matches!(&term.kind, Kind::Words { fields, raw, .. } if cut_short(*fields, raw));
                    if cut || matches!(term.kind, Kind::Address { .. }) {
                        out.local.terms.push(term.clone());
                    }
                }
                _ => out.local.terms.push(term.clone()),
            }
        }
        out
    }

    /// Whether `m` matches every term. Dates compare in a zone `offset_minutes` east of UTC.
    pub fn matches(&self, m: &Message, offset_minutes: i32) -> bool {
        self.matches_with(m, offset_minutes, |_| None)
    }

    /// [`Query::matches`], with what `known` says of a term (whether `m` has what term `i` asks
    /// for, before its `-`; a word index's answer) in place of looking at `m`; `None`: look.
    pub fn matches_with(&self, m: &Message, offset_minutes: i32, known: impl Fn(usize) -> Option<bool>) -> bool {
        let day = || DateTime::from_timestamp(m.date, offset_minutes).date;
        self.terms.iter().enumerate().all(|(i, t)| {
            let hit = known(i).unwrap_or_else(|| match &t.kind {
                Kind::Words { fields, words, .. } => {
                    words::texts(m, &m.body.text, |field, text| field & fields != 0 && words::starts_in(text, words))
                }
                Kind::Address { fields, address, .. } => {
                    let is = |a: &crate::Address| address_key(&a.email) == *address;
                    (fields & FROM != 0 && is(&m.from)) || (fields & TO != 0 && m.to.iter().any(is)) || (fields & CC != 0 && m.cc.iter().any(is))
                }
                Kind::Category(x) => m.categories.iter().any(|c| contains_ci(c, x)),
                Kind::HasAttachment => m.has_attachments(),
                Kind::Unread => !m.read,
                Kind::Read => m.read,
                Kind::Flagged => m.flag == FlagStatus::Flagged,
                Kind::Importance(i) => m.importance == *i,
                Kind::Before(d) => day() < *d,
                Kind::After(d) => day() > *d,
                Kind::On(d) => day() == *d,
            });
            hit != t.negate
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Address, Attachment};

    fn msg() -> Message {
        Message {
            from: Address::new("Ada Lovelace", "ada@example.org"),
            to: vec![Address::new("Charles", "charles@example.org")],
            cc: vec![Address::new("", "team@example.org")],
            subject: "Budget review: Q4".into(),
            body: crate::Body { text: "Numbers attached. See the engine estimate.".into(), html: None },
            attachments: vec![Attachment::new("estimate.xlsx", "application/octet-stream", vec![1, 2, 3])],
            categories: vec!["Blue category".into()],
            // 2026-10-11T23:30:00Z
            date: 1_791_761_400,
            ..Default::default()
        }
    }

    fn hit(q: &str) -> bool {
        Query::parse(q).matches(&msg(), 0)
    }

    #[test]
    fn words_and_phrases_must_all_match() {
        assert!(hit("budget"));
        assert!(hit("BUDGET engine"));
        assert!(hit("\"budget review\""));
        assert!(!hit("\"review budget\""));
        assert!(!hit("budget invoice"));
        assert!(hit("estimate.xlsx") && hit("charles") && hit("team@"));
        assert!(hit(""));
        assert!(Query::parse("  \"\"  ").is_empty());
    }

    #[test]
    fn keywords_filter_fields() {
        assert!(hit("from:ada") && !hit("from:charles"));
        assert!(hit("to:charles") && hit("cc:team") && !hit("to:team"));
        assert!(hit("subject:q4") && !hit("subject:engine") && hit("body:engine"));
        assert!(hit("from:\"Ada Lovelace\""));
        assert!(hit("has:attachment") && hit("hasattachment:yes") && hit("attachment:estimate"));
        assert!(hit("is:unread") && !hit("is:read") && !hit("is:flagged"));
        assert!(hit("category:blue") && !hit("category:red"));
        assert!(hit("importance:normal") && !hit("is:important"));
    }

    #[test]
    fn negation_and_dates() {
        assert!(hit("budget -invoice") && !hit("budget -engine") && !hit("-from:ada") && hit("-is:read"));
        assert!(hit("on:2026-10-11") && hit("after:2026-10-10") && hit("before:10/12/2026"));
        assert!(!hit("after:2026-10-11") && !hit("before:2026-10-11"));
        // In a zone an hour east it is already the 12th.
        assert!(Query::parse("on:2026-10-12").matches(&msg(), 60));
    }

    /// R6.2: a word matches the start of a word, whatever its case and accents; inside a word it
    /// doesn't (it used to: substring matching).
    #[test]
    fn words_match_word_starts_ignoring_case_and_accents() {
        assert!(hit("budg") && hit("rev") && hit("ESTIM") && hit("lovelace"));
        assert!(!hit("udget") && !hit("view") && !hit("ngine") && !hit("ovelace"));
        assert!(hit("\"budget rev\"") && hit("from:\"ada love\"") && !hit("from:\"ada charles\""));
        let m = Message { subject: "Sjednica u četvrtak".into(), from: Address::new("Đorđe Gvožđar", "dg@example.org"), ..Default::default() };
        for q in ["gvozdar", "from:GVOŽĐAR", "dorde", "cetvrtak", "četvrtak", "subject:sjed"] {
            assert!(Query::parse(q).matches(&m, 0), "{q}");
        }
        // Punctuation alone asks for nothing; with words, the words.
        assert!(Query::parse("- @ ... \"\"").is_empty());
        assert!(hit("ada@") && hit("@example") && !hit("@nowhere"));
    }

    #[test]
    fn what_is_known_of_a_term_stands_in_for_looking() {
        let q = Query::parse("budget -invoice is:unread");
        assert_eq!(q.len(), 3);
        assert_eq!(q.words_of(0), Some((words::ALL, ["budget".to_string()].as_slice())));
        assert!(q.words_of(2).is_none() && q.term_reads_body(1) && !q.term_reads_body(2));
        let m = msg();
        assert!(q.matches(&m, 0));
        // An index that says the message has "invoice": excluded; that it lacks "budget": too.
        assert!(!q.matches_with(&m, 0, |i| (i == 1).then_some(true)));
        assert!(!q.matches_with(&m, 0, |i| (i == 0).then_some(false)));
        assert!(!Query::parse("subject:x").reads_bodies() && Query::parse("body:x").reads_bodies());
    }

    /// R6.2: what a server's search understands goes there; the rest is checked here, and the
    /// two together are the whole query.
    #[test]
    fn a_query_splits_into_what_a_server_understands_and_what_is_checked_here() {
        let split =
            Query::parse(r#"Budget from:ada@example.org subject:"Q4 plan" has:attachment is:unread -lunch importance:low category:red"#).for_server();
        let words = |field, w: &[&str]| ServerTerm::Words { field, words: w.iter().map(|w| w.to_string()).collect() };
        assert_eq!(
            split.terms,
            [
                words(None, &["Budget"]),
                words(Some(ServerField::From), &["ada@example.org"]),
                words(Some(ServerField::Subject), &["Q4", "plan"]),
                ServerTerm::HasAttachment,
                ServerTerm::Importance(Importance::Low),
            ]
        );
        assert_eq!(split.local.len(), 4, "the address (#301), is:unread, -lunch, category:red");
        assert!(!split.local.matches(&msg(), 0), "its category is blue");
        let unread_red = Message { categories: vec!["Red".into()], ..msg() };
        assert!(split.local.matches(&unread_red, 0));
        // Accents and case as typed; punctuation at a word's ends trimmed; an address whole.
        assert_eq!(Query::parse("Gvožđar, (ada@example.org).").for_server().terms, [words(None, &["Gvožđar"]), words(None, &["ada@example.org"])]);
        // Nothing for the server: all of it is checked here.
        let none = Query::parse("is:flagged -x after:2026-01-01 importance:normal").for_server();
        assert!(none.terms.is_empty() && none.local.len() == 4);
        // At most so many terms go; the rest stay here.
        let many = Query::parse(&"word ".repeat(40)).for_server();
        assert_eq!((many.terms.len(), many.local.len()), (MAX_SERVER_TERMS, 40 - MAX_SERVER_TERMS));
    }

    /// #194: a space after a keyword's colon is allowed, as in Outlook; a keyword with nothing
    /// after it yet, while the owner types, asks for nothing; a partial word is a word start.
    #[test]
    fn a_keyword_may_have_a_space_after_its_colon() {
        let m = Message { subject: "[IQA] Audit summary".into(), from: Address::new("Stuart Example", "stuart@example.org"), ..Default::default() };
        let on = |q: &str| Query::parse(q).matches(&m, 0);
        for q in [
            "from: stuart",
            "from: stuart subject: iqa",
            "FROM:  Stuart   Subject: IQA",
            "from: stuart subject:",
            "from: stuart su",
            "subject: \"iqa audit\"",
            "subject: \"iqa audit s\"",
            "subject: from:stuart",
        ] {
            assert!(on(q), "{q}");
        }
        for q in ["from: ada", "from: stuart subject: budget", "subject: stuart", "-from: stuart", "from: stuart sz"] {
            assert!(!on(q), "{q}");
        }
        assert!(on("-from: ada"));
        assert_eq!(Query::parse("from: stuart subject: iqa"), Query::parse("from:stuart subject:iqa"));
        assert_eq!(Query::parse("-from: stuart"), Query::parse("-from:stuart"));
        // Nothing after the keyword (yet): no condition, the rest as it is.
        assert!(Query::parse("subject:").is_empty() && Query::parse("from: subject:").is_empty() && Query::parse("-is:").is_empty());
        assert_eq!(Query::parse("from: stuart subject:"), Query::parse("from:stuart"));
        assert_eq!(Query::parse("subject: from:stuart"), Query::parse("from:stuart"));
        // A word ending in a colon that is no keyword is still text.
        assert_eq!(Query::parse("re: budget").len(), 2);
        assert_eq!(Query::parse("is: unread"), Query::parse("is:unread"));
        assert_eq!(Query::parse("before: 2026-10-12"), Query::parse("before:2026-10-12"));
        // The server is given the same.
        let split = Query::parse("from: stuart subject: iqa").for_server();
        let words = |field, w: &str| ServerTerm::Words { field: Some(field), words: vec![w.to_string()] };
        assert_eq!(split.terms, [words(ServerField::From, "stuart"), words(ServerField::Subject, "iqa")]);
        assert!(split.local.is_empty());
    }

    /// #194: `subject:iqa` finds the word wherever it stands among punctuation.
    #[test]
    fn a_subject_word_is_found_among_punctuation() {
        for subject in ["[IQA] Audit dates", "IQA-2026 review", "RE: IQA", "Fwd: (iqa)", "Re: Re: IQA."] {
            let m = Message { subject: subject.into(), ..Default::default() };
            assert!(Query::parse("subject:iqa").matches(&m, 0) && Query::parse("subject: IQA").matches(&m, 0), "{subject}");
        }
        assert!(!Query::parse("subject:iqa").matches(&Message { subject: "LIQA notes".into(), ..Default::default() }, 0));
    }

    /// #196: where the keywords the parser reads stand in the text, for the search box's pills.
    #[test]
    fn keywords_are_found_where_they_stand() {
        fn spans(q: &str) -> Vec<(&str, &str)> {
            keyword_spans(q).into_iter().map(|k| (&q[k.range], &q[k.key])).collect()
        }
        assert_eq!(spans("from: stuart subject: iqa"), [("from: stuart", "from:"), ("subject: iqa", "subject:")]);
        assert_eq!(spans("budget  -from:ada  is:unread"), [("-from:ada", "from:"), ("is:unread", "is:")]);
        assert_eq!(spans(r#"subject:"q4 plan" x"#), [(r#"subject:"q4 plan""#, "subject:")]);
        assert_eq!(spans(r#"subject: "q4 plan""#), [(r#"subject: "q4 plan""#, "subject:")]);
        assert_eq!(spans("Grüße from:Đorđe"), [("from:Đorđe", "from:")]);
        // What the parser reads as text or as nothing is no keyword.
        for q in ["re: budget", "is:banana", "subject:", "from: subject:", "from:@@", "10:30", "budget", ""] {
            assert!(spans(q).is_empty(), "{q}");
        }
        assert_eq!(spans("from: subject:iqa"), [("subject:iqa", "subject:")]);
        // Hostile text: never a panic, every range on a character boundary.
        for q in ["\"from:\"x", "-", "-:", "\u{301}from:\u{301}", "from:\"", &"from: ".repeat(500)] {
            for k in keyword_spans(q) {
                assert!(q.get(k.range.clone()).is_some() && q.get(k.key.clone()).is_some(), "{q}");
            }
        }
    }

    /// #196: the pills are around the keywords alone, and a keyword being typed keeps its pill.
    #[test]
    fn pills_are_around_the_keywords() {
        fn heads(q: &str, cursor: Option<usize>) -> Vec<&str> {
            keyword_pills(q, cursor).into_iter().map(|r| &q[r]).collect()
        }
        assert_eq!(heads("from: stuart smith subject: iqa", None), ["from:", "subject:"]);
        assert_eq!(heads("budget -is:unread From:ada", None), ["-is:", "From:"]);
        // As soon as the colon is typed, and while a value is on its way.
        assert_eq!(heads("from:", None), ["from:"]);
        assert_eq!(heads("budget subject:", Some(15)), ["subject:"]);
        assert_eq!(heads("is:u", Some(4)), ["is:"]);
        assert_eq!(heads("is: u", Some(5)), ["is:"]);
        // Left as what the parser reads as text: no pill.
        assert!(heads("is:u", None).is_empty());
        assert!(heads("is:banana x", Some(11)).is_empty());
        assert_eq!(heads("from: subject:iqa", None), ["subject:"]);
        for q in ["re: budget", "10:30", "nosuch:thing", "-", "-:", "", ":"] {
            assert!(heads(q, None).is_empty(), "{q}");
        }
        // Hostile text: never a panic, every range on a character boundary.
        for q in ["\"from:\"x", "\u{301}from:\u{301}", "-from:\"", "Đ-is:", &"from: ".repeat(500)] {
            for c in [None, Some(0), Some(1), Some(q.len()), Some(usize::MAX)] {
                for r in keyword_pills(q, c) {
                    assert!(q.get(r).is_some(), "{q}");
                }
            }
        }
    }

    #[test]
    fn unknown_keywords_and_junk_are_text() {
        assert!(hit("review: q4"));
        assert!(!hit("nosuch:thing"));
        assert!(!hit("before:someday"));
        assert!(Query::parse("-").matches(&Message { subject: "a - b".into(), ..Default::default() }, 0));
        let long = "x ".repeat(10_000);
        assert!(Query::parse(&long).terms.len() <= MAX_TERMS);
        let _ = Query::parse("\"unterminated from:").matches(&msg(), i32::MAX);
    }
}
