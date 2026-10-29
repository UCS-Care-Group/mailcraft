// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail put into a Microsoft mailbox from elsewhere: from another of the same owner's mailboxes
//! (#70: a shared mailbox and its owner's, or two shared mailboxes of one owner's), from another
//! Microsoft account, an IMAP account or a folder on this computer (#451).
//!
//! Microsoft Graph moves and copies a message only within its own mailbox. A message moved or
//! copied into another is therefore made in the destination by the destination's own run,
//! at its own address, from the whole message here ([`PlaceUp`]): its fields, its attachments, its
//! read state, flag and categories, and the times it was sent and received, as a message that was
//! received (not a draft). It carries MailCraft's marker (an invisible extended property with a
//! value used once), so that a make whose answer was lost is found and not made twice. Of a move,
//! the original is removed from its own mailbox only once the copy is confirmed there
//! ([`crate::sync::MovingOut`], or a [`crate::Pending::Move`] out of an IMAP account): a copy
//! that fails never loses the message.

use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use super::drafts::{AttachmentUp, DraftBody};
use super::{FlagUp, MessageOutcome};
use crate::{Address, Id, Importance, Message};

/// Most messages one Graph run puts into its mailbox; more go over several runs.
pub const MAX_PLACES: usize = 200;

/// Most bytes of mail (text, HTML and files) one run puts on a server, Graph's or IMAP's (#451):
/// more go over several runs, so that a big import moved into an account isn't read into memory
/// at once. A message bigger than this goes by itself.
pub const PLACE_BYTES: u64 = 64 * 1024 * 1024;

/// A message to make in a folder of the run's mailbox, from elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlaceUp {
    /// The message here.
    pub message: Id,
    /// The destination folder's server id; empty when it is made in the same run (`to_folder`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_folder: Option<Id>,
    /// MailCraft's marker for the copy: a value used once.
    pub marker: String,
    /// It is a move (the original goes once the copy is there), not a copy.
    #[serde(skip_serializing_if = "is_false")]
    pub moved: bool,
    pub subject: String,
    pub from: Address,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub to_list: Vec<Address>,
    /// Without who got a blind copy (Bcc): the copy is for anyone else who can open the mailbox
    /// to see too (#451).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,
    /// `Message-ID` without angle brackets.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    /// When it was sent, and when it was received.
    pub date: Timestamp,
    pub received: Timestamp,
    pub body: DraftBody,
    pub read: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<FlagUp>,
    pub importance: Importance,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentUp>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl PlaceUp {
    /// What of the whole message `m` goes, into the folder `to` (or `to_folder`, made in the run).
    pub fn of(m: &Message, to: String, to_folder: Option<Id>, marker: String, moved: bool, flag: Option<FlagUp>) -> PlaceUp {
        PlaceUp {
            message: m.id,
            to,
            to_folder,
            marker,
            moved,
            subject: m.subject.clone(),
            from: m.from.clone(),
            reply_to: m.reply_to.clone(),
            to_list: m.to.clone(),
            cc: m.cc.clone(),
            message_id: m.message_id.clone(),
            date: m.date,
            received: m.received,
            body: DraftBody::of(m),
            read: m.read,
            flag,
            importance: m.importance,
            categories: m.categories.clone(),
            attachments: m.attachments.iter().map(AttachmentUp::of).collect(),
        }
    }

    /// The same without its body's text and its attachments' data, for an answer.
    pub fn without_data(&self) -> PlaceUp {
        let mut out = self.clone();
        out.body.content.clear();
        for a in &mut out.attachments {
            a.data = Arc::new(Vec::new());
        }
        out
    }
}

/// What became of a [`PlaceUp`]: `Done` with the copy's server id and folder, `Gone` (the
/// destination folder is no more) or `Refused`. One that got no answer, or that the mailbox held
/// back (a shared mailbox's 403 that isn't "no access"), isn't here: it waits for the next run,
/// looked for by its marker first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceDone {
    /// What was asked (without the body's text and the attachments' data).
    pub change: PlaceUp,
    pub outcome: MessageOutcome,
}
