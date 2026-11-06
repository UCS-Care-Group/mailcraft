// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Send warnings** in a session (#509): the questions `compose.send` asks before a message goes,
//! measured against the known correspondents and the owner's organisation that the sender
//! warnings gather (`crate::sender_warnings`). The rules are `mailcraft_model::send_warnings`.
//!
//! `compose.send` refuses a message with a warning unless it is told to send anyway
//! (`"confirmed": true`, as every question answered is) or to move the people outside the
//! organisation to Bcc first (`"moveToBcc": true`). The window asks the owner and says which; an
//! agent's send is asked about in the window, and an agent can't say either for itself.

use mailcraft_model::send_warnings::{self, SendWarning};
use mailcraft_model::{Address, Id, format_list};
use serde_json::Value;

use crate::Session;

/// The parameter that sends a message with a warning anyway: the owner's yes.
pub const CONFIRMED: &str = "confirmed";
/// The parameter that moves the people outside the organisation from To and Cc to Bcc first.
pub const MOVE_TO_BCC: &str = "moveToBcc";

/// Whether `params` say `key` (`true`, and nothing else).
pub fn said(params: &Value, key: &str) -> bool {
    params.get(key).and_then(Value::as_bool) == Some(true)
}

/// The recipient lines of a message being written, as typed, and the message it replies to or
/// forwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub original: Option<Id>,
}

impl Lines {
    /// Those of the current message being written.
    pub fn of(s: &Session) -> Option<Lines> {
        Lines::named(s, &Value::Null)
    }

    /// Those of the message being written that a call with `params` is for (its `compose`, else
    /// the current one).
    pub fn named(s: &Session, params: &Value) -> Option<Lines> {
        let c = s.compose_named(params)?;
        // A draft opened again knows its original by what it answers.
        let original = c.source.or_else(|| c.draft.and_then(|d| s.mailbox.message(d)).and_then(|d| d.answers.as_ref()).map(|a| a.message));
        Some(Lines { to: c.to.clone(), cc: c.cc.clone(), bcc: c.bcc.clone(), original })
    }
}

fn valid(line: &str) -> Vec<Address> {
    Address::parse_list(line).into_iter().filter(Address::is_valid).collect()
}

impl Session {
    /// The send warnings of the message being written that `compose.send` with `params` would
    /// send (its `compose`, else the current one), as it would send it (`moveToBcc` moves
    /// first); none when that message isn't there.
    pub fn send_warnings(&mut self, params: &Value) -> Vec<SendWarning> {
        let Some(lines) = Lines::named(self, params) else { return Vec::new() };
        let lines = if said(params, MOVE_TO_BCC) { self.outsiders_to_bcc(&lines) } else { lines };
        self.send_warnings_of(&lines)
    }

    /// The send warnings of a message to `lines`.
    pub fn send_warnings_of(&mut self, lines: &Lines) -> Vec<SendWarning> {
        let facts = self.known_facts();
        let seen = lines.original.and_then(|id| self.mailbox.message(id)).map(send_warnings::seen_on).unwrap_or_default();
        send_warnings::check(&valid(&lines.to), &valid(&lines.cc), &valid(&lines.bcc), &facts, &seen)
    }

    /// `lines` with the people outside the organisation moved from To and Cc to Bcc, their names
    /// kept; what isn't an address stays where it was typed.
    pub fn outsiders_to_bcc(&mut self, lines: &Lines) -> Lines {
        let facts = self.known_facts();
        let (was_to, was_cc) = (Address::parse_list(&lines.to), Address::parse_list(&lines.cc));
        let (to, cc, bcc) = send_warnings::outsiders_to_bcc(&facts, &was_to, &was_cc, &Address::parse_list(&lines.bcc));
        // Nobody to move: the lines stay as they were typed.
        if to.len() == was_to.len() && cc.len() == was_cc.len() {
            return lines.clone();
        }
        Lines { to: format_list(&to), cc: format_list(&cc), bcc: format_list(&bcc), original: lines.original }
    }
}

/// `warnings` as data for a refusal: each one's kind and what it is about.
pub fn describe(warnings: &[SendWarning]) -> String {
    warnings
        .iter()
        .map(|w| match w {
            SendWarning::LookalikeRecipient { address, like } => format!("{} {address} (like {like})", w.kind()),
            SendWarning::Outsiders { count, domains, .. } => format!("{} ({count} at {domains} domains in To and Cc)", w.kind()),
            SendWarning::TooManyRecipients { checked } => format!("{} (only the first {checked} different ones were checked)", w.kind()),
        })
        .collect::<Vec<_>>()
        .join("; ")
}
