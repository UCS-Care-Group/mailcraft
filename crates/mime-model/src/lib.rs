// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's messages ([`mailcraft_model::Message`]) as Internet messages: the reader and the
//! writer of `mailcraft-mime`, which work on that crate's own types and are reusable outside the
//! client (#1235), with the conversion to and from the mailbox's model, and [`rich`] text to HTML
//! and back. Nothing here does I/O.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod rich;

use mailcraft_mime as mime;
use mailcraft_model::{Address, Attachment, Body, FlagStatus, Importance, Message, Original};

pub use mailcraft_mime::WriteOptions;

/// Read an Internet message (an `.eml` file, or what a server hands over) as a message of the
/// mailbox: [`mailcraft_mime::parse`], with `received` the `Date` as well. Never fails.
pub fn parse(bytes: &[u8]) -> Message {
    to_model(mime::parse(bytes))
}

/// [`parse`] for a message as its server or a file gave it: a Digitally signed or Encrypted one
/// keeps the bytes it came as ([`mailcraft_mime::parse_fetched`]).
pub fn parse_fetched(bytes: &[u8]) -> Message {
    to_model(mime::parse_fetched(bytes))
}

/// The messages of an mbox file ([`mailcraft_mime::mbox_messages`]).
pub fn parse_mbox(bytes: &[u8]) -> Vec<Message> {
    mbox_messages(bytes).collect()
}

/// The messages of an mbox file one at a time, so that a caller can stop early.
pub fn mbox_messages(bytes: &[u8]) -> impl Iterator<Item = Message> + '_ {
    mime::mbox_messages(bytes).map(to_model)
}

/// Write a message as RFC 5322 + MIME bytes ([`mailcraft_mime::write`]).
pub fn write(message: &Message, opts: &WriteOptions) -> Vec<u8> {
    mime::write(&from_model(message), opts)
}

/// `message` as [`write()`] writes it, for a copy put on a mail server
/// ([`mailcraft_mime::write_placed`]).
pub fn write_placed(message: &Message, opts: &WriteOptions, marker: &str) -> Vec<u8> {
    mime::write_placed(&from_model(message), opts, marker)
}

/// Whether [`write()`] writes `message`'s body as the bytes it came as (#30).
pub fn writes_original(message: &Message) -> bool {
    mime::writes_original(&from_model(message))
}

/// Write messages as an mbox file ([`mailcraft_mime::write_mbox`]).
pub fn write_mbox(messages: &[&Message], opts: &WriteOptions) -> Vec<u8> {
    let converted: Vec<mime::Message> = messages.iter().map(|m| from_model(m)).collect();
    mime::write_mbox(&converted.iter().collect::<Vec<_>>(), opts)
}

/// A message read by `mailcraft-mime` as one of the mailbox, which is new: no id, account or
/// folder, and nothing of a server's. `received` is the `Date`.
pub fn to_model(m: mime::Message) -> Message {
    // Every field named, so a field added to mime's Message fails to compile here until it is carried.
    let mime::Message {
        message_id,
        in_reply_to,
        references,
        from,
        reply_to,
        to,
        cc,
        bcc,
        subject,
        date,
        body,
        hidden_text,
        attachments,
        read,
        flag,
        importance,
        read_receipt,
        categories,
        answered,
        draft,
        headers,
        fresh_stamps,
        original,
    } = m;
    // What is read by name stays on the message; every other line goes to its block, in place.
    let (headers, header_block) = mailcraft_model::headers::split(headers);
    Message {
        message_id,
        in_reply_to,
        references,
        from: address(from),
        reply_to: addresses(reply_to),
        to: addresses(to),
        cc: addresses(cc),
        bcc: addresses(bcc),
        subject,
        date,
        received: date,
        body: Body { text: body.text, html: body.html },
        hidden_text,
        attachments: attachments
            .into_iter()
            .map(|a| Attachment { name: a.name, mime: a.mime, content_id: a.content_id, inline: a.inline, data: a.data, kept: None })
            .collect(),
        read,
        flag: match flag {
            mime::FlagStatus::None => FlagStatus::None,
            mime::FlagStatus::Flagged => FlagStatus::Flagged,
            mime::FlagStatus::Complete => FlagStatus::Complete,
        },
        importance: match importance {
            mime::Importance::Low => Importance::Low,
            mime::Importance::Normal => Importance::Normal,
            mime::Importance::High => Importance::High,
        },
        read_receipt,
        categories,
        answered,
        draft,
        headers,
        header_block,
        fresh_stamps,
        original: original.map(|data| Original { data, kept: None }),
        ..Message::default()
    }
}

/// What of a message of the mailbox an Internet message carries, for `mailcraft-mime` to write.
/// A file or an original kept outside memory is written as what is in memory (nothing): whoever
/// writes such a message brings its bytes in first.
pub fn from_model(m: &Message) -> mime::Message {
    let address = |a: &Address| mime::Address { name: a.name.clone(), email: a.email.clone() };
    let addresses = |list: &[Address]| list.iter().map(address).collect();
    mime::Message {
        message_id: m.message_id.clone(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        from: address(&m.from),
        reply_to: addresses(&m.reply_to),
        to: addresses(&m.to),
        cc: addresses(&m.cc),
        bcc: addresses(&m.bcc),
        subject: m.subject.clone(),
        date: m.date,
        body: mime::Body { text: m.body.text.clone(), html: m.body.html.clone() },
        hidden_text: m.hidden_text.clone(),
        attachments: m
            .attachments
            .iter()
            .map(|a| mime::Attachment {
                name: a.name.clone(),
                mime: a.mime.clone(),
                content_id: a.content_id.clone(),
                inline: a.inline,
                data: a.data.clone(),
            })
            .collect(),
        read: m.read,
        flag: match m.flag {
            FlagStatus::None => mime::FlagStatus::None,
            FlagStatus::Flagged => mime::FlagStatus::Flagged,
            FlagStatus::Complete => mime::FlagStatus::Complete,
        },
        importance: match m.importance {
            Importance::Low => mime::Importance::Low,
            Importance::Normal => mime::Importance::Normal,
            Importance::High => mime::Importance::High,
        },
        read_receipt: m.read_receipt,
        categories: m.categories.clone(),
        answered: m.answered,
        draft: m.draft,
        headers: m.all_headers(),
        fresh_stamps: m.fresh_stamps,
        original: m.original.as_ref().filter(|o| !o.data.is_empty()).map(|o| o.data.clone()),
    }
}

fn address(a: mime::Address) -> Address {
    Address { name: a.name, email: a.email }
}

fn addresses(list: Vec<mime::Address>) -> Vec<Address> {
    list.into_iter().map(address).collect()
}

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod tests;
