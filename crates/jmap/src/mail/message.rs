// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One email of a JMAP answer as the message the engine takes in (RFC 8621 §4).
//!
//! A server that does not parse the header for a client serves the raw fields (`headers`, §4.1.2.2) and
//! nothing of `subject`, `from` and the rest, and every server serves those, so they are what a message
//! is read from: the few fields a list needs are put in a header block of their own, one line to a field,
//! and the program's MIME reader (encoded words, address lists, dates) reads that.
//!
//! Everything here reads hostile JSON: a field of the wrong type is a field that is not there, every list
//! and string is cut to a size, and nothing is indexed or unwrapped.

use std::collections::HashMap;

use mailcraft_model::sync::{GraphMessage, ServerMessage};
use mailcraft_model::{Address, FlagStatus};
use mailcraft_time::Timestamp;
use serde_json::Value;

use crate::Id;

/// Longest preview kept, in characters (RFC 8621 §4.1.4 allows a server up to 256).
pub(super) const MAX_PREVIEW: usize = 256;
/// Longest subject and `Message-ID` kept, in characters (RFC 5322's line limit).
const MAX_TEXT: usize = 998;
/// Most addresses kept of a `To` or a `Cc`, and longest name and address.
const MAX_PEOPLE: usize = 500;
const MAX_NAME: usize = 256;
const MAX_EMAIL: usize = 320;
/// Most header fields looked at in one email (a server sends 500 at most).
const MAX_FIELDS: usize = 500;
/// Longest arrival date looked at.
const MAX_DATE: usize = 64;

/// The header fields a message list needs: the name to match, the name to write, how many bytes of it are
/// read in all (together, at most 64 KB), and whether a second field of that name adds to the first.
const FIELDS: [(&str, &str, usize, bool); 6] = [
    ("message-id", "Message-ID", 2_048, false),
    ("subject", "Subject", 8_192, false),
    ("from", "From", 4_096, false),
    ("to", "To", 24_576, true),
    ("cc", "Cc", 24_576, true),
    ("date", "Date", 256, false),
];

/// The marks an email has among its keywords (RFC 8621 §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Marks {
    pub read: bool,
    pub flagged: bool,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
}

impl Marks {
    /// `$seen`, `$flagged`, `$answered`, `$forwarded` and `$draft` among an email's keywords, in any letter
    /// case; any other keyword is none of the program's business and nothing of it is kept. All of the
    /// keywords may be looked at (how many there are is bounded by the size of the answer, and the look
    /// ends when all five have been found): the first 64 only would be the first 64 in the order of their
    /// names, and a server could fill that with names that sort before `$seen`.
    pub fn of(keywords: Option<&Value>) -> Marks {
        let mut marks = Marks::default();
        let Some(map) = keywords.and_then(Value::as_object) else { return marks };
        for (name, on) in map {
            if *on != Value::Bool(true) {
                continue;
            }
            let is = |keyword: &str| name.eq_ignore_ascii_case(keyword);
            marks.read |= is("$seen");
            marks.flagged |= is("$flagged");
            marks.answered |= is("$answered");
            marks.forwarded |= is("$forwarded");
            marks.draft |= is("$draft");
            if marks.read && marks.flagged && marks.answered && marks.forwarded && marks.draft {
                break;
            }
        }
        marks
    }
}

/// Whether the window of `mailbox` lists an email whose `mailboxIds` are these: an email in several
/// mailboxes is listed by the window of the first of them and no other, so that the folders do not take it
/// from one another run after run. The first is the one of the lowest rank (`claimable` has the rank of each
/// mailbox of the list: the Inbox, then the special mailboxes, then the rest), and among those of one rank
/// the one with the smallest id (by its bytes); without a list, the smallest id. Only the mailboxes of the
/// list count (`claimable`, when the list could be had): a view that is no place for mail, or a mailbox the
/// list does not have, claims nothing, or one with a small id would take every email from the one it is in.
/// An email that says nothing of where it lies (or only ids that mean nothing) belongs to the mailbox it
/// was found in.
pub(super) fn claims(mailbox: &str, mailbox_ids: Option<&Value>, claimable: Option<&HashMap<String, usize>>) -> bool {
    let Some(map) = mailbox_ids.and_then(Value::as_object) else { return true };
    let rank = |id: &str| match claimable {
        _ if Id::new(id).is_err() => None,
        None => Some(0),
        Some(all) => all.get(id).copied(),
    };
    let first = map.iter().filter(|(_, on)| **on == Value::Bool(true)).filter_map(|(id, _)| Some((rank(id)?, id.as_str()))).min();
    first.is_none_or(|(_, first)| first == mailbox)
}

/// What the server says of an email: its id, the mailbox it was listed in and its marks.
fn server(id: &str, mailbox: &str, marks: Marks, size: u64) -> ServerMessage {
    ServerMessage {
        id: id.to_string(),
        folder: mailbox.to_string(),
        read: marks.read,
        answered: marks.answered,
        forwarded: marks.forwarded,
        flag: if marks.flagged { FlagStatus::Flagged } else { FlagStatus::None },
        size,
        ..Default::default()
    }
}

/// An email this mailbox has already: its marks and nothing else (the engine takes nothing else of it).
pub(super) fn known(id: &str, mailbox: &str, marks: Marks) -> GraphMessage {
    GraphMessage { server: server(id, mailbox, marks, 0), ..Default::default() }
}

/// An email this mailbox has not: its header fields, size, arrival, preview and marks.
pub(super) fn whole(id: &str, mailbox: &str, entry: &Value, marks: Marks) -> GraphMessage {
    let fields = Fields::read(entry.get("headers"));
    let received = entry
        .get("receivedAt")
        .and_then(Value::as_str)
        .filter(|date| date.len() <= MAX_DATE)
        .and_then(|date| mailcraft_time::iso_to_timestamp(date, 0));
    GraphMessage {
        server: server(id, mailbox, marks, entry.get("size").and_then(Value::as_u64).unwrap_or(0)),
        message_id: fields.message_id,
        subject: fields.subject,
        from: fields.from,
        to: fields.to,
        cc: fields.cc,
        sent: fields.date.or(received).unwrap_or(0),
        received: received.or(fields.date).unwrap_or(0),
        preview: line(entry.get("preview").and_then(Value::as_str).unwrap_or_default(), MAX_PREVIEW),
        has_attachments: entry.get("hasAttachment").and_then(Value::as_bool).unwrap_or(false),
        draft: marks.draft,
    }
}

/// What the header says of a message.
#[derive(Default)]
struct Fields {
    message_id: String,
    subject: String,
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    date: Option<Timestamp>,
}

impl Fields {
    fn read(headers: Option<&Value>) -> Fields {
        let block = block(headers);
        if block.is_empty() {
            return Fields::default();
        }
        let message = mailcraft_mime::parse(block.as_bytes());
        Fields {
            message_id: message.message_id.chars().filter(|c| !c.is_control() && !c.is_whitespace()).take(MAX_TEXT).collect(),
            subject: line(&message.subject, MAX_TEXT),
            from: person(message.from).unwrap_or_default(),
            to: people(message.to),
            cc: people(message.cc),
            // (0 is what the reader says of a date that is missing or makes no sense.)
            date: Some(message.date).filter(|date| *date != 0),
        }
    }
}

fn people(list: Vec<Address>) -> Vec<Address> {
    list.into_iter().take(MAX_PEOPLE).filter_map(person).collect()
}

fn person(address: Address) -> Option<Address> {
    let (name, email) = (line(&address.name, MAX_NAME), line(&address.email, MAX_EMAIL));
    (!name.is_empty() || !email.is_empty()).then_some(Address { name, email })
}

/// `text` as one line: every run of blanks and control characters a single space, trimmed, at most `most`
/// characters.
pub(super) fn line(text: &str, most: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut blank = false;
    for c in text.chars() {
        if c == ' ' || c.is_control() {
            blank = !out.is_empty();
            continue;
        }
        // The blank that was waiting for this character comes only with it.
        let need = if blank { 2 } else { 1 };
        if count.saturating_add(need) > most {
            break;
        }
        if blank {
            out.push(' ');
            blank = false;
        }
        out.push(c);
        count = count.saturating_add(need);
    }
    out
}

/// The fields of `headers` (the raw header fields of RFC 8621 §4.1.2.2: a list of `{name, value}`, the
/// value everything after the colon with its folds) that a list needs, as a header block the MIME reader
/// takes: each on one line (a fold is a blank, so a value cannot end the block or start another field),
/// under a name of ours. The first of a field that is only once in a message; all of a `To` or a `Cc`, to
/// the room a field has. At most 500 fields are looked at, and at most 64 KB are kept.
fn block(headers: Option<&Value>) -> String {
    let mut out = String::new();
    let Some(list) = headers.and_then(Value::as_array) else { return out };
    let mut used = [0usize; FIELDS.len()];
    for field in list.iter().take(MAX_FIELDS) {
        let (Some(name), Some(value)) = (field.get("name").and_then(Value::as_str), field.get("value").and_then(Value::as_str)) else {
            continue;
        };
        let Some(slot) = FIELDS.iter().position(|(wanted, ..)| name.eq_ignore_ascii_case(wanted)) else { continue };
        let (Some((_, written, room, repeats)), Some(spent)) = (FIELDS.get(slot), used.get_mut(slot)) else { continue };
        if *spent > 0 && !*repeats {
            continue;
        }
        let text = folded(value, room.saturating_sub(*spent), *repeats);
        if text.is_empty() {
            continue;
        }
        *spent = spent.saturating_add(text.len());
        out.push_str(written);
        out.push_str(": ");
        out.push_str(&text);
        out.push_str("\r\n");
    }
    if !out.is_empty() {
        out.push_str("\r\n");
    }
    out
}

/// `value` on one line (a fold or any control character is one blank, a run of blanks is one) of at most
/// `room` bytes; a list of addresses that is cut loses the address it was cut in, not half of it (and so a
/// list whose first address is longer than the room is nothing).
fn folded(value: &str, room: usize, list: bool) -> String {
    let mut out = String::new();
    let mut blank = false;
    for c in value.chars() {
        if c == ' ' || c.is_control() {
            blank = !out.is_empty();
            continue;
        }
        let gap = usize::from(blank);
        if out.len().saturating_add(gap).saturating_add(c.len_utf8()) > room {
            if list {
                out.truncate(out.rfind([',', ';']).unwrap_or(0));
            }
            break;
        }
        if blank {
            out.push(' ');
            blank = false;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests;
