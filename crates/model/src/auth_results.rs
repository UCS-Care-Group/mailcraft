// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the account's own mail server says of a message's sender, read from the topmost
//! `Authentication-Results` header (RFC 8601) and nothing deeper.
//!
//! The receiving server puts its own header on top; the ones below it were written before, by
//! whoever, and a sender can write any of them. So only the topmost one is read, and only when
//! the server of the account the message is in wrote it ([`from_own_server`]). MailCraft makes
//! no checks of its own (no DNS, no DKIM, no SPF): a server that says nothing says nothing.
//!
//! Two answers come of it:
//!
//! - [`vouched`]: the server says the sender's domain passed DMARC. What the window lets a
//!   remembered sender's pictures come by, and a sender's own picture.
//! - [`failed_check`]: the server says it failed DMARC, or, when the server gives no DMARC
//!   verdict at all, SPF for the sender's domain. A **sender warning**.
//!
//! Everything is bounded: so many headers looked through, so many characters of the value, so
//! many clauses.

use crate::{Account, AccountKind, Message, provider};

/// How many of a message's headers are looked through for the server's verdict, and how many
/// clauses and characters of it are read.
const MAX_HEADERS_READ: usize = 2_000;
const MAX_TOKENS: usize = 200;
const MAX_HEADER_CHARS: usize = 4_000;
/// Longest address of a sender that is read, and the longest domain.
const MAX_SENDER_LEN: usize = 320;
const MAX_DOMAIN_LEN: usize = 253;

/// A sender's address as it is compared: ASCII lower case; `None` for what is no address.
pub fn sender(text: &str) -> Option<String> {
    let text = text.trim();
    // ASCII only, folded as ASCII: Unicode lower-casing maps some letters (the Kelvin sign) onto
    // ASCII ones, which would let a lookalike address stand for another.
    let plain = text.is_ascii() && text.len() <= MAX_SENDER_LEN && text.contains('@') && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    (plain && text.len() >= 3).then(|| text.to_ascii_lowercase())
}

/// A domain as it is compared: what follows the `@` of an address (or the whole of `text`, with
/// or without an `@` in front), in ASCII lower case; `None` for what is no domain (a name with a
/// letter outside ASCII is written the way it goes over the wire, `xn--…`, to be one).
pub fn domain(text: &str) -> Option<String> {
    let text = text.trim();
    let name = text.rsplit_once('@').map_or(text, |(_, name)| name).trim_end_matches('.');
    let plain = !name.is_empty()
        && name.len() <= MAX_DOMAIN_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        && !name.starts_with(['.', '-'])
        && !name.contains("..");
    plain.then(|| name.to_ascii_lowercase())
}

/// The authserv-id (RFC 8601) an `Authentication-Results` value starts with: the first word before
/// the first `;`, after comments, ignoring an optional version number. `None` when the value
/// starts with a result instead (Exchange Online writes no id), or the id is no host name.
fn authserv_id(value: &str) -> Option<String> {
    let mut plain = String::new();
    let mut depth = 0usize;
    for c in value.chars().take(MAX_HEADER_CHARS) {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => break,
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let first = plain.split_whitespace().next()?;
    if first.contains('=') { None } else { domain(first) }
}

/// Whether the topmost `Authentication-Results` header of `m` was written by `account`'s own
/// server: any header for a Microsoft account (Exchange Online puts its own on top, with no id),
/// and for another account one whose authserv-id is a known id of its IMAP host's provider
/// (`provider::authserv_ids`), or the IMAP host's domain one label up, or a name under that.
/// Anything else is not known to be the account's server's.
fn from_own_server(account: Option<&Account>, value: &str) -> bool {
    let Some(account) = account else { return false };
    let host = account.incoming.as_ref().map(|server| server.host.trim().trim_end_matches('.').to_ascii_lowercase());
    if account.kind == AccountKind::Microsoft || host.as_deref().is_some_and(provider::is_microsoft_imap_host) {
        return true;
    }
    let (Some(host), Some(id)) = (host, authserv_id(value)) else { return false };
    if provider::authserv_ids(&host).contains(&id.as_str()) {
        return true;
    }
    let Some((_, rest)) = host.split_once('.') else { return false };
    rest.split('.').count() >= 2 && !provider::is_public_suffix(rest) && (id == rest || id.strip_suffix(rest).is_some_and(|head| head.ends_with('.')))
}

/// The clauses of an `Authentication-Results` value (RFC 8601), each as its words: split at the
/// `;` that is neither in a comment (nested, with `\` quoting) nor in a quoted string, a comment
/// being a gap between words and a quoted string staying whole inside its word. What a sender
/// can put into a value (a `reason="…"`, a `(comment)`) therefore stays inside its own word.
fn clauses(value: &str) -> Vec<Vec<String>> {
    let (mut out, mut words, mut word) = (Vec::new(), Vec::new(), String::new());
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for c in value.chars().take(MAX_HEADER_CHARS) {
        if escaped {
            escaped = false;
            if quoted {
                word.push(c);
            }
            continue;
        }
        match c {
            '\\' if depth > 0 || quoted => escaped = true,
            _ if depth > 0 => match c {
                '(' => depth = depth.saturating_add(1),
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            },
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            _ if quoted => word.push(c),
            '(' => {
                depth = 1;
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            ';' | ' ' | '\t' | '\r' | '\n' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if c == ';' {
                    out.push(std::mem::take(&mut words));
                    if out.len() >= MAX_TOKENS {
                        return out;
                    }
                }
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    out.push(words);
    out
}

/// The clauses of the topmost `Authentication-Results` header of `m`, when `m` is on the server
/// (it has a UID there, or the server's own record of it: mail brought in from a file has the
/// file's word and nothing else) and that header was written by the server of `account`
/// ([`from_own_server`]); with the sender's domain. `None` otherwise: nothing to go by.
fn own_verdict(account: Option<&Account>, m: &Message) -> Option<(Vec<Vec<String>>, String)> {
    if m.uid == 0 && m.server.is_none() {
        return None;
    }
    let (_, value) = m.headers.iter().take(MAX_HEADERS_READ).find(|(name, _)| name.eq_ignore_ascii_case("Authentication-Results"))?;
    if !from_own_server(account, value) {
        return None;
    }
    let sender_domain = sender(&m.from.email).and_then(|address| domain(&address))?;
    Some((clauses(value), sender_domain))
}

/// The value of the property `key` (`header.from`, `smtp.mailfrom`) in a clause's words, with any
/// quotes taken off.
fn property<'a>(words: &'a [String], key: &str) -> Option<&'a str> {
    words.iter().skip(1).find_map(|word| word.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.trim_matches('"')))
}

/// The method and result a clause starts with (`dmarc`, `pass`), in lower case.
fn result(words: &[String]) -> Option<(String, String)> {
    let (method, result) = words.first()?.split_once('=')?;
    Some((method.to_ascii_lowercase(), result.to_ascii_lowercase()))
}

/// Whether the server that received `m` vouches that it came from the domain it says it did:
/// `m` is on the server; the topmost `Authentication-Results` header, written by the server of
/// the account `m` is in ([`own_verdict`]), has a clause whose result is `dmarc=pass` (or
/// Exchange Online's `compauth=pass`), and that clause names the `header.from` it judged, which
/// is the sender's domain or one above it ([`judged_for`]). Only the clause's own words are
/// read, not what a comment or quoted string in the header says. A header the sender could have
/// written is not read.
pub fn vouched(account: Option<&Account>, m: &Message) -> bool {
    let Some((clauses, sender_domain)) = own_verdict(account, m) else { return false };
    clauses.iter().any(|words| {
        let passes = words.first().is_some_and(|result| result.eq_ignore_ascii_case("dmarc=pass") || result.eq_ignore_ascii_case("compauth=pass"));
        passes
            && words.iter().skip(1).any(|word| {
                word.split_once('=').is_some_and(|(key, name)| {
                    key.eq_ignore_ascii_case("header.from")
                        && domain(name.trim_matches('"')).is_some_and(|judged| judged_for(&judged, &sender_domain))
                })
            })
    })
}

/// Which check the server says the sender failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    Dmarc,
    /// SPF for the sender's domain, when the server gave no DMARC verdict.
    Spf,
}

impl Failed {
    /// The method's name as RFC 8601 writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Failed::Dmarc => "dmarc",
            Failed::Spf => "spf",
        }
    }
}

/// Whether the account's own server says the sender of `m` failed its checks: its topmost
/// `Authentication-Results` ([`own_verdict`]) has `dmarc=fail` for the sender's domain (a clause
/// that names no `header.from` judged the `From:` it had, which is the sender's), or, when it has
/// no `dmarc` clause at all, `spf=fail` for a `smtp.mailfrom` at the sender's domain or one under
/// the domain it judged. A server that says nothing, or says it of another domain, gives `None`.
pub fn failed_check(account: Option<&Account>, m: &Message) -> Option<Failed> {
    let (clauses, sender_domain) = own_verdict(account, m)?;
    let mut dmarc_said = false;
    for words in &clauses {
        let Some((method, outcome)) = result(words) else { continue };
        if method != "dmarc" {
            continue;
        }
        dmarc_said = true;
        let of_sender = property(words, "header.from").is_none_or(|judged| domain(judged).is_some_and(|judged| judged_for(&judged, &sender_domain)));
        if outcome == "fail" && of_sender {
            return Some(Failed::Dmarc);
        }
    }
    if dmarc_said {
        return None;
    }
    let spf_fails = clauses.iter().any(|words| {
        result(words).is_some_and(|(method, outcome)| method == "spf" && outcome == "fail")
            && property(words, "smtp.mailfrom").and_then(domain).is_some_and(|judged| judged == sender_domain || judged_for(&sender_domain, &judged))
    });
    spf_fails.then_some(Failed::Spf)
}

/// Whether a verdict on the domain `judged` stands for mail from `sender_domain`: the same name,
/// or a name under it, as Gmail names the organisation's domain (`shop.example` for mail from
/// `news.shop.example`); never a bare suffix such as `co.uk`, which is nobody's organisation.
fn judged_for(judged: &str, sender_domain: &str) -> bool {
    judged == sender_domain
        || (judged.contains('.') && !provider::is_public_suffix(judged) && sender_domain.strip_suffix(judged).is_some_and(|head| head.ends_with('.')))
}
