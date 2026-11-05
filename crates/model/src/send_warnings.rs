// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Send warnings**: what MailCraft asks about before a message goes (#509).
//!
//! - A **lookalike recipient**: an address the owner has never written to and doesn't keep as a
//!   contact, whose domain is one character (or one look-alike character) away from a known
//!   correspondent's, or whose part before the @ is one character away from a known address at
//!   the same domain. The domains are compared as a lookalike sender's are
//!   ([`Facts::lookalike_domain`]), so the two warnings agree.
//! - **Outsiders who would see each other**: [`MIN_OUTSIDERS`] or more people outside the
//!   **owner's organisation**, at [`MIN_OUTSIDE_DOMAINS`] or more domains, in To and Cc. Not when
//!   every one of them was already on the message replied to or forwarded (From, To, Cc): they saw
//!   each other's addresses there. For an owner with personal accounts only (no organisation),
//!   everybody but the owner is outside.
//!
//! The **known correspondents** are those of the sender warnings ([`Facts`]): the contacts and
//! whoever mail in Sent Items or the Outbox went to; never somebody who only wrote to the owner,
//! as a lookalike domain would then be "known" after one message from it. Pure and cheap: nothing
//! here looks anything up, and every loop is bounded.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::confusables::{as_seen, skeleton};
use crate::sender_warnings::{Facts, MIN_LOOKALIKE_NAME, address_key, registrable, sender_domain, wire, within_one_edit};
use crate::{Address, Message};

/// The fewest people outside the organisation who, seeing each other, are asked about.
pub const MIN_OUTSIDERS: usize = 5;
/// The fewest domains those people are at.
pub const MIN_OUTSIDE_DOMAINS: usize = 2;
/// The most different recipients looked at. A message to more is asked about as such
/// ([`SendWarning::TooManyRecipients`]): what isn't looked at isn't let through unasked.
pub const MAX_RECIPIENTS: usize = 1_000;
/// The most entries of the recipient lines read, the same address named again included; past
/// that, too many as well.
pub(crate) const MAX_NAMED: usize = 100_000;
/// The most lookalike recipients said, and the most outsiders named.
pub const MAX_SAID: usize = 20;

/// A reason to ask before a message goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendWarning {
    /// `address` looks like `like`: a known correspondent's domain, or a known address.
    LookalikeRecipient { address: String, like: String },
    /// `count` people outside the organisation, at `domains` domains, would see each other's
    /// addresses in To and Cc; the first [`MAX_SAID`] of them.
    Outsiders { count: usize, domains: usize, addresses: Vec<String> },
    /// More recipients than are looked at ([`MAX_RECIPIENTS`] different ones, or more entries
    /// in the lines than are read): those past them weren't checked; `checked` different ones were.
    TooManyRecipients { checked: usize },
}

impl SendWarning {
    /// Its kind, as the control channel and MCP name it.
    pub fn kind(&self) -> &'static str {
        match self {
            SendWarning::LookalikeRecipient { .. } => "lookalikeRecipient",
            SendWarning::Outsiders { .. } => "outsidersSeeEachOther",
            SendWarning::TooManyRecipients { .. } => "tooManyRecipients",
        }
    }

    /// As data: `{"kind", …}`.
    pub fn json(&self) -> Value {
        match self {
            SendWarning::LookalikeRecipient { address, like } => json!({"kind": self.kind(), "address": address, "like": like}),
            SendWarning::Outsiders { count, domains, addresses } => {
                json!({"kind": self.kind(), "count": count, "domains": domains, "addresses": addresses})
            }
            SendWarning::TooManyRecipients { checked } => json!({"kind": self.kind(), "checked": checked}),
        }
    }
}

/// The different recipients of `lines`, each once (by [`address_key`], else as written in lower
/// case), in the order they come; and whether there were more than are looked at.
fn unique<'a>(lines: &[&'a [Address]]) -> (Vec<&'a Address>, bool) {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (named, a) in lines.iter().flat_map(|line| line.iter()).enumerate() {
        if named >= MAX_NAMED {
            return (out, true);
        }
        let key = address_key(&a.email).unwrap_or_else(|| a.email.trim().to_lowercase());
        if seen.insert(key) {
            if out.len() >= MAX_RECIPIENTS {
                return (out, true);
            }
            out.push(a);
        }
    }
    (out, false)
}

/// The known correspondent `address` looks like but isn't (a domain, or an address), when it is
/// neither known nor the owner's own.
pub fn lookalike(facts: &Facts, address: &str) -> Option<String> {
    let key = address_key(address)?;
    if facts.knows(&key) || facts.is_own(address) {
        return None;
    }
    let (local, domain) = key.rsplit_once('@')?;
    if let Some(like) = facts.lookalike_domain(domain) {
        return Some(wire(&like));
    }
    let chars: Vec<char> = local.chars().collect();
    if chars.len() < MIN_LOOKALIKE_NAME {
        return None;
    }
    let seen = skeleton(local);
    let dotless = DOTS_IGNORED.contains(&domain);
    facts
        .locals
        .get(domain)?
        .iter()
        .filter(|theirs| theirs.text != local && theirs.chars.len() >= MIN_LOOKALIKE_NAME && theirs.chars.len().abs_diff(chars.len()) <= 1)
        // Numbered mailboxes (`sales1`, `sales2`) are each other's neighbours, not lookalikes; and
        // where the provider ignores dots, `mayaokafor` is `maya.okafor`.
        .filter(|theirs| !differs_by_digits_only(&chars, &theirs.chars) && !(dotless && theirs.text.replace('.', "") == local.replace('.', "")))
        .find(|theirs| within_one_edit(&chars, &theirs.chars) || skeleton(&theirs.text) == seen)
        .map(|theirs| format!("{}@{}", theirs.text, wire(domain)))
}

/// Domains whose mailboxes ignore dots before the @.
const DOTS_IGNORED: [&str; 2] = ["gmail.com", "googlemail.com"];

/// Whether `a` and `b`, of the same length, differ only where both have a digit.
fn differs_by_digits_only(a: &[char], b: &[char]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y || (x.is_ascii_digit() && y.is_ascii_digit()))
}

/// `address` as a warning shows it: what draws nothing left out, at most [`MAX_SHOWN`]
/// characters before the @, and its domain written as it goes over the wire (`xn--…`), so that a
/// look-alike letter shows for what it is.
pub fn shown(address: &str) -> String {
    match address_key(address).as_deref().and_then(|key| key.rsplit_once('@')) {
        Some((local, domain)) => format!("{}@{}", as_seen(local, MAX_SHOWN), wire(domain)),
        None => as_seen(address, MAX_SHOWN),
    }
}

/// The most characters of an address before the @ that a warning shows.
const MAX_SHOWN: usize = 100;

/// Whether `address` is outside the owner's organisation: not the owner's own, and at none of
/// the organisation's domains (for an owner without one, anybody but the owner).
pub fn is_outsider(facts: &Facts, address: &str) -> bool {
    !facts.is_own(address) && sender_domain(address).is_some_and(|domain| !facts.is_inside(&domain))
}

/// The addresses that were on `m` for everyone on it to see (From, To, Cc), as [`address_key`]
/// has them.
pub fn seen_on(m: &Message) -> HashSet<String> {
    std::iter::once(&m.from).chain(&m.to).chain(&m.cc).take(MAX_RECIPIENTS).filter_map(|a| address_key(&a.email)).collect()
}

/// The outsiders in `to` and `cc` who would see each other, when they are enough to ask about
/// and not all of them were in `seen` already.
pub fn outsiders(facts: &Facts, to: &[Address], cc: &[Address], seen: &HashSet<String>) -> Option<SendWarning> {
    let mut out = Vec::new();
    let mut domains = HashSet::new();
    // Each once, so that the same address named again doesn't take another's place; more than
    // are looked at is a warning of its own ([`check`]).
    for a in unique(&[to, cc]).0 {
        let Some(key) = address_key(&a.email) else { continue };
        if !is_outsider(facts, &a.email) {
            continue;
        }
        if let Some((_, domain)) = key.rsplit_once('@') {
            domains.insert(registrable(domain).to_string());
        }
        out.push(key);
    }
    if out.len() < MIN_OUTSIDERS || domains.len() < MIN_OUTSIDE_DOMAINS || out.iter().all(|a| seen.contains(a)) {
        return None;
    }
    Some(SendWarning::Outsiders { count: out.len(), domains: domains.len(), addresses: out.iter().take(MAX_SAID).map(|a| shown(a)).collect() })
}

/// The send warnings of a message to `to`, `cc` and `bcc`, measured against `facts`; `seen`: the
/// addresses on the message it replies to or forwards ([`seen_on`]). Lookalikes first, in the
/// order the recipients come; then [`SendWarning::TooManyRecipients`] when there were more than
/// are looked at. Every recipient looked at is looked at once, however often it is named; at most
/// [`MAX_SAID`] lookalikes are said, and saying no more never makes a warning none.
pub fn check(to: &[Address], cc: &[Address], bcc: &[Address], facts: &Facts, seen: &HashSet<String>) -> Vec<SendWarning> {
    let (all, too_many) = unique(&[to, cc, bcc]);
    let checked = all.len();
    let mut out: Vec<SendWarning> = all
        .into_iter()
        .filter_map(|a| lookalike(facts, &a.email).map(|like| SendWarning::LookalikeRecipient { address: shown(&a.email), like }))
        .take(MAX_SAID)
        .collect();
    out.extend(outsiders(facts, to, cc, seen));
    if too_many {
        out.push(SendWarning::TooManyRecipients { checked });
    }
    out
}

/// Whether any of `warnings` is about outsiders, which moving them to Bcc settles.
pub fn has_outsiders(warnings: &[SendWarning]) -> bool {
    warnings.iter().any(|w| matches!(w, SendWarning::Outsiders { .. }))
}

/// `to` and `cc` with the outsiders taken out, and `bcc` with them put at its end (each once;
/// names kept): Move to Bcc.
pub fn outsiders_to_bcc(facts: &Facts, to: &[Address], cc: &[Address], bcc: &[Address]) -> (Vec<Address>, Vec<Address>, Vec<Address>) {
    let mut in_bcc: HashSet<String> = bcc.iter().filter_map(|a| address_key(&a.email)).collect();
    let mut moved = bcc.to_vec();
    let mut keep = |line: &[Address]| -> Vec<Address> {
        line.iter()
            .filter(|a| {
                if !is_outsider(facts, &a.email) {
                    return true;
                }
                if address_key(&a.email).is_none_or(|key| in_bcc.insert(key)) {
                    moved.push((*a).clone());
                }
                false
            })
            .cloned()
            .collect()
    };
    let (to, cc) = (keep(to), keep(cc));
    (to, cc, moved)
}
