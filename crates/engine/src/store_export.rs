// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One self-contained mailbox, written a message at a time. Loading every message first defeats
//! the store's content budget; serializing the resulting mailbox into a Vec doubles that cost.

use std::cell::Cell;
use std::io::Write;
use std::sync::Arc;

use mailcraft_model::{Id, Mailbox, Message};
use serde::ser::{Error as _, SerializeMap};
use serde::{Serialize, Serializer};

pub(super) fn write(out: &mut impl Write, mailbox: &Mailbox, load: impl Fn(Id) -> Result<Arc<Message>, String>, limit: u64) -> Result<u64, String> {
    // Keep the model's serialization of every other field, including fields added later. Only
    // the message map is replaced by a serializer that brings in one message and releases it.
    let mut shell = mailbox.clone();
    shell.messages = Default::default();
    let serde_json::Value::Object(mut fields) = serde_json::to_value(&shell).map_err(|e| e.to_string())? else {
        return Err("the mailbox metadata is not an object".into());
    };
    fields.remove("messages");
    let message_left = Cell::new(None);
    let mut out = Limited { out, bytes: 0, limit, message_left: &message_left };
    let mut serializer = serde_json::Serializer::new(&mut out);
    let mut map = serde::Serializer::serialize_map(&mut serializer, Some(fields.len().saturating_add(1))).map_err(|e| e.to_string())?;
    for (name, value) in &fields {
        map.serialize_entry(name, value).map_err(|e| e.to_string())?;
    }
    map.serialize_entry("messages", &Messages { mailbox, load, message_left: &message_left }).map_err(|e| e.to_string())?;
    map.end().map_err(|e| e.to_string())?;
    Ok(out.bytes)
}

struct Messages<'a, F> {
    mailbox: &'a Mailbox,
    load: F,
    message_left: &'a Cell<Option<u64>>,
}

impl<F: Fn(Id) -> Result<Arc<Message>, String>> Serialize for Messages<'_, F> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.mailbox.messages.len()))?;
        for id in self.mailbox.messages.keys() {
            let message = (self.load)(*id).map_err(S::Error::custom)?;
            map.serialize_key(id)?;
            // The streaming reader starts its item gate after the key, before the colon.
            // Share that boundary with the writer, without a second pass or a JSON buffer.
            self.message_left.set(Some(super::stream::MAX_ITEM));
            // Use the model's whole-mailbox path, including compatibility with older readers.
            map.serialize_value(&mailcraft_model::messages::as_written(&message))?;
            self.message_left.set(None);
        }
        map.end()
    }
}

/// Enforce the serialized size (including base64, headers and originals) before writing beyond
/// the limit. A failure drops the AtomicFile, preserving the previous destination.
struct Limited<'a, W> {
    out: &'a mut W,
    bytes: u64,
    limit: u64,
    message_left: &'a Cell<Option<u64>>,
}

impl<W: Write> Write for Limited<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() as u64 > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(format!(
                "the mailbox is more than one file holds ({} MB); nothing was written. Save it to another folder instead, which keeps its content beside the mailbox file",
                self.limit >> 20
            )));
        }
        if self.message_left.get().is_some_and(|left| buf.len() as u64 > left) {
            return Err(std::io::Error::other(format!(
                "a message is more than the mailbox reader holds ({} MB); nothing was written. Save to another folder instead, which keeps content beside the mailbox file",
                super::stream::MAX_ITEM >> 20
            )));
        }
        let written = self.out.write(buf)?;
        self.bytes = self.bytes.saturating_add(written as u64);
        if let Some(left) = self.message_left.get() {
            self.message_left.set(Some(left.saturating_sub(written as u64)));
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
#[path = "store_export_tests.rs"]
mod tests;
