// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts of a Microsoft account up through Graph (R3.4.1; R3 spec S23–S26).
//!
//! A draft written here is made in the server's Drafts carrying MailCraft's marker (an invisible
//! extended property with a value used once), so that a make whose answer was lost is found by it
//! instead of made twice (S23). Saved again, it is changed in place, field by field, its
//! attachments added and removed (S24). What the server last had of it is kept with the draft
//! ([`ServerDraft::synced`]): the difference is what goes up, and an edit made elsewhere while one
//! made here waits keeps both (S26).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Address, Attachment, Id, Importance, Message};

/// Most drafts one Graph run takes up; more go over several runs.
pub const MAX_DRAFTS: usize = 200;

/// FNV-1a of `bytes`: how a body or an attachment is told from another without being kept twice.
/// The same on every machine and in every version, as it is written into mailbox files.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// What kind of answer a draft is, when it was begun as one to a message the server has: it is
/// made with Graph's create-reply, create-reply-all or create-forward, so that it stays in that
/// message's conversation (S23).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftKind {
    #[default]
    Reply,
    ReplyAll,
    Forward,
}

/// The message on the server a draft answers or forwards: its server id, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftOf {
    pub id: String,
    pub kind: DraftKind,
}

/// An attachment as the server has it on a draft: the server's id (empty when it isn't known,
/// for a draft that came down), its name, and the [`fingerprint`] of what it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncedAttachment {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    pub hash: u64,
}

/// What the server last had of the parts of a draft that are written here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftState {
    pub subject: String,
    /// The [`fingerprint`] of the body as it goes up ([`DraftBody::of`]).
    pub body: u64,
    pub to: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<Address>,
    pub importance: Importance,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SyncedAttachment>,
}

impl DraftState {
    /// The draft `m` as it is here now.
    pub fn of(m: &Message) -> DraftState {
        DraftState {
            subject: m.subject.clone(),
            body: DraftBody::of(m).hash,
            to: m.to.clone(),
            cc: m.cc.clone(),
            bcc: m.bcc.clone(),
            importance: m.importance,
            attachments: m
                .attachments
                .iter()
                .map(|a| SyncedAttachment { id: String::new(), name: a.name.clone(), hash: attachment_hash(a) })
                .collect(),
        }
    }
}

/// The [`fingerprint`] of an attachment: what it holds, and how the body shows it.
pub fn attachment_hash(a: &Attachment) -> u64 {
    let head = format!("{}\u{0}{}\u{0}{}\u{0}{}\u{0}", a.name, a.mime, a.content_id, a.inline);
    fingerprint(head.as_bytes()) ^ fingerprint(&a.data).rotate_left(1)
}

/// A draft of a Microsoft account as its server knows it (R3.4.1), kept with the draft here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerDraft {
    /// MailCraft's marker on the server's copy: a value used once, by which a make whose answer
    /// was lost is found (S23).
    pub marker: String,
    /// The message on the server it answers or forwards, when it was begun as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<DraftOf>,
    /// What the server last had of it; `None` while nothing of it has gone up (or that isn't
    /// known): then everything goes.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "super::lenient")]
    pub synced: Option<DraftState>,
    /// It was saved here since the server last had all of it: it goes up with the next run.
    #[serde(skip_serializing_if = "is_false")]
    pub pending: bool,
    /// How many times it was saved here: a run's answer settles it only when it wasn't saved
    /// again meanwhile.
    #[serde(skip_serializing_if = "is_zero")]
    pub saves: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A draft's body as it goes up: HTML when it has HTML, else its text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftBody {
    pub html: bool,
    pub content: String,
    pub hash: u64,
}

impl DraftBody {
    pub fn of(m: &Message) -> DraftBody {
        let (html, content) = match &m.body.html {
            Some(html) => (true, html.clone()),
            None => (false, m.body.text.clone()),
        };
        let hash = fingerprint(content.as_bytes()) ^ u64::from(html);
        DraftBody { html, content, hash }
    }
}

/// An attachment of a draft as it goes up. What comes back names it without what it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AttachmentUp {
    pub name: String,
    pub mime: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content_id: String,
    #[serde(skip_serializing_if = "is_false")]
    pub inline: bool,
    pub hash: u64,
    /// How large it is, in bytes (also when `data` was left out of an answer).
    pub size: u64,
    #[serde(with = "crate::b64", skip_serializing_if = "no_data")]
    pub data: Arc<Vec<u8>>,
}

fn no_data(data: &Arc<Vec<u8>>) -> bool {
    data.is_empty()
}

impl AttachmentUp {
    pub fn of(a: &Attachment) -> AttachmentUp {
        AttachmentUp {
            name: a.name.clone(),
            mime: a.mime.clone(),
            content_id: a.content_id.clone(),
            inline: a.inline,
            hash: attachment_hash(a),
            size: a.data.len() as u64,
            data: a.data.clone(),
        }
    }
}

/// A draft's attachments when they changed here: every one it has here, and those the server
/// was last known to have (S24).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AttachmentsUp {
    pub here: Vec<AttachmentUp>,
    pub synced: Vec<SyncedAttachment>,
}

/// A draft to take up (S23, S24): made when `id` is empty, else changed in place; only the
/// fields that differ from what the server last had are named.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftUp {
    pub message: Id,
    /// The server's id of it; empty for one to make.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub marker: String,
    /// The server's change key of it as last seen: a draft whose key is another now was changed
    /// elsewhere, and isn't written over (S26).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// Made as an answer to, or a forward of, this message of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<DraftOf>,
    /// Its [`ServerDraft::saves`] when this was worked out.
    pub saves: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<DraftBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<Address>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<Address>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<Address>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importance: Option<Importance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<AttachmentsUp>,
}

impl DraftUp {
    /// Whether it changes anything of the message itself (rather than its attachments).
    pub fn has_fields(&self) -> bool {
        self.subject.is_some() || self.body.is_some() || self.to.is_some() || self.cc.is_some() || self.bcc.is_some() || self.importance.is_some()
    }

    /// The same without what its attachments hold, for an answer.
    pub fn without_data(&self) -> DraftUp {
        let mut out = self.clone();
        if let Some(up) = out.attachments.as_mut() {
            for a in &mut up.here {
                a.data = Arc::new(Vec::new());
            }
        }
        if let Some(body) = out.body.as_mut() {
            body.content.clear();
        }
        out
    }
}

/// What became of a [`DraftUp`]. A draft that got no answer at all isn't here: it goes up again
/// next run, looked for by its marker first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftDone {
    /// What was asked (without the body's text and the attachments' data).
    pub change: DraftUp,
    /// The server's id of the draft: made, found by its marker, or as it was. Empty when the
    /// server has none (yet).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The server's id of the folder it is in, when the server said.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// The server's change key of it after all of this, when it was read.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// The fields named went up.
    #[serde(skip_serializing_if = "is_false")]
    pub fields: bool,
    /// The attachments the server has on it now, when they were brought in step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<SyncedAttachment>>,
    /// The server no longer has the draft (404), or won't take it (403, 400): the status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<u16>,
    /// The server's copy was changed elsewhere since it was last seen: nothing was written, and
    /// both are to be kept (S26).
    #[serde(skip_serializing_if = "is_false")]
    pub conflict: bool,
}

/// Most messages one Graph run sends; more go with the next.
pub const MAX_SENDS: usize = 100;

/// A message of the Outbox to send (R3.4.2, S27): as the draft the server has (brought up to date
/// first) when it was saved as one, else with `sendMail` as MIME.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SendUp {
    pub message: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftUp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<Box<Message>>,
}

/// What became of a message sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendOutcome {
    /// Not tried, or not taken (the server busy, the draft not whole there yet): next run.
    #[default]
    Waits,
    /// It went: the server files it in Sent Items.
    Sent,
    /// **Held** (S28): `lost` when it isn't known whether it went (the answer was lost); else the
    /// server refused it, and `why` says so in plain words. Not sent again by itself.
    #[serde(rename_all = "camelCase")]
    Held { lost: bool, why: String },
}

/// The `why` of a message held because Exchange won't let the owner send as the shared mailbox it
/// was written in, nor on its behalf (Graph's `ErrorSendAsDenied`, #70). Said with the mailbox's
/// address where the owner reads it; nothing is ever sent from another address instead.
pub const SEND_AS_DENIED: &str = "you don't have permission to send as this shared mailbox";

/// What stands for a draft of an Exchange server account in a delete that is queued when its
/// server id is not known (its make was not answered): the draft's marker, found in Drafts by
/// the run that deletes it (E8). Never a server id: ids do not begin so.
pub fn marker_id(marker: &str) -> String {
    format!("{MARKER_ID}{marker}")
}

/// The marker a [`marker_id`] stands for, or `None` for a server id.
pub fn marker_of(id: &str) -> Option<&str> {
    id.strip_prefix(MARKER_ID).filter(|marker| !marker.is_empty())
}

const MARKER_ID: &str = "marker:";

/// A message sent and what became of it, with what became of its draft on the way.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SendDone {
    pub message: Id,
    pub outcome: SendOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftDone>,
    /// An Exchange server account's draft that was sent as a message of its own: its server id
    /// when the server still has it after a send that is clear (taking it away failed), else
    /// empty; the draft's [`marker_id`] when its id was not known and could not be found. The
    /// takeaway is queued as an ordinary delete; the send is not made again.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub draft_left: String,
}
