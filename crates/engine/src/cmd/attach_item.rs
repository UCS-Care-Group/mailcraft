// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attach Item and Contact Card: a message from the mailbox, or a contact, attached to the
//! message being written.

use mailcraft_model::Attachment;
use serde_json::{Value, json};

use super::{MAX_ATTACHMENT, MAX_ATTACHMENTS, compose_mut, eml_attachment, numbered};
use crate::cmd::{bad, fail};
use crate::{CmdError, CmdResult, Session, p};

/// The `id` a command takes, which it must have (never the selection).
fn named(v: &Value) -> Result<mailcraft_model::Id, CmdError> {
    p::only(v, &["id"])?;
    p::id_strict(v, "id")?.ok_or_else(|| bad("`id` (number) is required"))
}

/// Attach Item: message `id` attached whole as an `.eml`, as Forward as Attachment attaches it
/// (no Bcc, none of the owner's marks, only the headers that came with it).
pub(super) fn attach_item(s: &mut Session, v: &Value) -> CmdResult {
    let id = named(v)?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    let a = eml_attachment(s, id)?;
    add(s, a)
}

/// Contact Card: contact `id` as a vCard file.
pub(super) fn business_card(s: &mut Session, v: &Value) -> CmdResult {
    let id = named(v)?;
    let contact = s.mailbox.contact(id).ok_or_else(|| failed!("no contact with id {id}", id))?;
    let name = crate::opening::export_name_or(&contact.name(), "vcf", "contact");
    let bytes = mailcraft_vcard::write(&[contact]).into_bytes();
    add(s, Attachment::new(name, "text/vcard", bytes))
}

fn add(s: &mut Session, a: Attachment) -> CmdResult {
    if a.size() > MAX_ATTACHMENT {
        return Err(failed!("{name} is larger than {mb} MB", name = a.name, mb = MAX_ATTACHMENT >> 20));
    }
    let c = compose_mut(s)?;
    if c.attachments.len() >= MAX_ATTACHMENTS {
        return Err(fail("this message already has as many attachments as it can carry"));
    }
    let name = numbered(a.name.clone(), &c.attachments);
    let size = a.size();
    c.attachments.push(Attachment { name: name.clone(), ..a });
    c.dirty = true;
    Ok(json!({"name": name, "size": size, "attachments": c.attachments.len()}))
}
