// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The crate's own picture of a message: what [`crate::parse()`] reads and [`crate::write()`]
//! writes, with nothing of a mailbox in it (#1235), so that the reader is reusable outside the
//! client. A program keeps its messages in its own types and converts at the edge.

use std::sync::Arc;

/// Seconds since 1970-01-01 UTC.
pub type Timestamp = mailcraft_time::Timestamp;

/// A mailbox in an address field: a display name (empty when there is none) and an address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Address {
    pub name: String,
    pub email: String,
}

impl Address {
    /// An address from a name and an address, each trimmed.
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Address {
        Address { name: name.into().trim().to_string(), email: email.into().trim().to_string() }
    }
}

/// How important the sender says a message is (`Importance`, `X-Priority`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Importance {
    Low,
    #[default]
    Normal,
    High,
}

impl Importance {
    /// An `Importance` value (`low`, `normal`, `high` and their other spellings).
    pub fn parse(s: &str) -> Option<Importance> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "5" | "non-urgent" => Some(Importance::Low),
            "normal" | "3" | "medium" => Some(Importance::Normal),
            "high" | "1" | "urgent" => Some(Importance::High),
            _ => None,
        }
    }
}

/// A message's follow-up flag, as `X-Status` (`F`) and `X-MailCraft-Flag` (`complete`) carry it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlagStatus {
    #[default]
    None,
    Flagged,
    Complete,
}

/// The readable text of a message, and its HTML when it has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Body {
    pub text: String,
    pub html: Option<String>,
}

/// A file of a message (or a picture its HTML shows).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    /// Media type (`application/pdf`).
    pub mime: String,
    /// `Content-ID` without angle brackets, for pictures the HTML body shows (`cid:`).
    pub content_id: String,
    /// Shown inside the body rather than as a file.
    pub inline: bool,
    pub data: Arc<Vec<u8>>,
}

/// An Internet message, as this crate reads and writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// `Message-ID` without angle brackets.
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub from: Address,
    pub reply_to: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    /// From `Date`; 0 when it is missing or unreadable.
    pub date: Timestamp,
    pub body: Body,
    /// The words of what the HTML hides (a preheader, the other copy of a desktop and mobile
    /// pair): kept apart from the text, never to be shown as it.
    pub hidden_text: String,
    pub attachments: Vec<Attachment>,
    /// `Status` has `R`.
    pub read: bool,
    pub flag: FlagStatus,
    pub importance: Importance,
    /// Written as `Disposition-Notification-To` (the sender).
    pub read_receipt: bool,
    /// `Keywords`.
    pub categories: Vec<String>,
    /// `X-Status` has `A`.
    pub answered: bool,
    /// Written from its parts even when it keeps an [`original`](Message::original).
    pub draft: bool,
    /// Every other header field that isn't MIME structure, in order.
    pub headers: Vec<(String, String)>,
    /// An Exchange `AuthAs` stamp ([`crate::AUTH_AS`]) was read from it.
    pub fresh_stamps: bool,
    /// The bytes a Digitally signed or Encrypted message came as, in CRLF lines
    /// ([`crate::parse_fetched`]), which [`crate::write()`] writes its body from.
    pub original: Option<Arc<Vec<u8>>>,
}

impl Message {
    /// [`Message::original`], when it holds any bytes.
    pub fn original_bytes(&self) -> Option<&[u8]> {
        self.original.as_deref().map(Vec::as_slice).filter(|d| !d.is_empty())
    }
}
