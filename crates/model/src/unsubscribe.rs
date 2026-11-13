// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Unsubscribe**: how a message says it can be unsubscribed from, read from its
//! `List-Unsubscribe` header (RFC 2369) and its `List-Unsubscribe-Post` header (RFC 8058).
//!
//! Pure: nothing here is fetched or sent. [`way`] reads the headers and gives the one way the
//! owner is offered, the first of: **one click** (RFC 8058: an `https:` address and
//! `List-Unsubscribe-Post: List-Unsubscribe=One-Click`, for the host to `POST` to once the owner
//! clicks), **by mail** (a `mailto:` address, for a message sent from the account that received
//! it once the owner agrees), **a web page** (an `http:` or `https:` address, only ever opened in
//! the owner's browser). Whether the headers are believed at all is the caller's: anybody can
//! write them, so they count only for mail the owner's server vouches for
//! ([`crate::auth_results::vouched`]) and that has no sender warning.
//!
//! What a message says is hostile: the headers read, the addresses in them and their lengths are
//! bounded, and only `https:`, `http:` and `mailto:` addresses are taken.

use crate::Message;
use crate::auth_results::MAX_HEADERS_READ;
use crate::sender_warnings::percent_decoded;

/// The most addresses read from a `List-Unsubscribe` header.
const MAX_ADDRESSES: usize = 8;
/// The longest address taken (in bytes, as written).
pub const MAX_ADDRESS: usize = 2_048;
/// The longest subject and body a `mailto:` address may give the unsubscribe message.
const MAX_SUBJECT: usize = 200;
const MAX_BODY: usize = 1_000;
/// The longest e-mail address taken from a `mailto:` address.
const MAX_MAILBOX: usize = 254;

/// How a message can be unsubscribed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Way {
    /// RFC 8058: `POST` `List-Unsubscribe=One-Click` to `url` (`https:`).
    OneClick { url: String },
    /// A message to `to`, with the subject and body the address gives (either may be empty).
    Mail { to: String, subject: String, body: String },
    /// A web page, to open in the owner's browser.
    Page { url: String },
}

impl Way {
    /// The stable name agents and scripts read.
    pub fn kind(&self) -> &'static str {
        match self {
            Way::OneClick { .. } => "oneClick",
            Way::Mail { .. } => "mail",
            Way::Page { .. } => "page",
        }
    }
}

/// The value of the first header of `m` named `name` (any letter case), among those read.
fn header<'a>(m: &'a Message, name: &str) -> Option<&'a str> {
    m.headers.iter().take(MAX_HEADERS_READ).find(|(h, _)| h.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// The addresses of a `List-Unsubscribe` value: each between `<` and `>`, with the white space
/// a folded header leaves inside taken out, at most [`MAX_ADDRESSES`], none longer than
/// [`MAX_ADDRESS`].
pub fn addresses(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while out.len() < MAX_ADDRESSES {
        let Some(open) = rest.find('<') else { break };
        let after = rest.get(open + 1..).unwrap_or("");
        let Some(close) = after.find('>') else { break };
        let inside: String = after.get(..close).unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
        if !inside.is_empty() && inside.len() <= MAX_ADDRESS {
            out.push(inside);
        }
        rest = after.get(close + 1..).unwrap_or("");
    }
    out
}

/// Whether `value` is RFC 8058's `List-Unsubscribe=One-Click` (the key in any letter case).
fn one_click(value: &str) -> bool {
    value.trim().split_once('=').is_some_and(|(key, word)| key.trim().eq_ignore_ascii_case("List-Unsubscribe") && word.trim() == "One-Click")
}

/// The way `m` offers to be unsubscribed from, if any ([`Way`]); the headers alone are read.
pub fn way(m: &Message) -> Option<Way> {
    let list = addresses(header(m, "List-Unsubscribe")?);
    let web = |secure: bool| {
        list.iter().find_map(|a| {
            let link = mailcraft_link::WebLink::parse(a)?;
            (!secure || link.is_https()).then(|| link.as_str().to_string())
        })
    };
    if header(m, "List-Unsubscribe-Post").is_some_and(one_click)
        && let Some(url) = web(true)
    {
        return Some(Way::OneClick { url });
    }
    if let Some(mail) = list.iter().find_map(|a| mailto(a)) {
        return Some(mail);
    }
    web(false).map(|url| Way::Page { url })
}

/// A `mailto:` address (RFC 6068) as the message it asks for: its first address, and its
/// `subject` and `body`, each percent-decoded and cut to its bound. `None` for no `mailto:`, or
/// an address that is not one plain mailbox.
pub fn mailto(address: &str) -> Option<Way> {
    let rest = address.get(..7).filter(|s| s.eq_ignore_ascii_case("mailto:")).and_then(|_| address.get(7..))?;
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let to = percent_decoded(to.split(',').next().unwrap_or(""));
    let to = to.trim();
    let (local, domain) = to.rsplit_once('@')?;
    let plain = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ',' | ';' | '@'));
    if to.len() > MAX_MAILBOX || !plain(local) || !plain(domain) || !domain.contains('.') {
        return None;
    }
    let mut subject = String::new();
    let mut body = String::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case("subject") && subject.is_empty() {
            subject = cut(&percent_decoded(value), MAX_SUBJECT, false);
        } else if key.eq_ignore_ascii_case("body") && body.is_empty() {
            body = cut(&percent_decoded(value), MAX_BODY, true);
        }
    }
    Some(Way::Mail { to: to.to_string(), subject, body })
}

/// `text` without control characters (but its line breaks when `lines`), at most `max` characters:
/// a subject is one line, so it can carry no header of its own.
fn cut(text: &str, max: usize, lines: bool) -> String {
    text.chars().filter(|c| !c.is_control() || lines && *c == '\n').take(max).collect()
}
