// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox kept in step through a Zimbra server's own web service: what a run is asked to do and
//! what it brings back.
//!
//! **Pure data contracts**: these types do no server I/O and remain readable when experimental
//! Zimbra support is switched off. The host sets the engine's session switch from
//! `MAILCRAFT_ZIMBRA=1`; enabled engine commands build requests and apply validated results.
//! Normal Send/Receive still needs a host runner: enabling those commands does not wire one up.
//!
//! The counterpart of [`EwsMailRequest`](super::EwsMailRequest) and
//! [`JmapMailRequest`](super::JmapMailRequest), beside them rather than bent into them. Plain data:
//! the engine builds a [`ZimbraMailRequest`] and takes in a [`ZimbraMailResult`] (both pure), and a
//! host that can reach the server does the talking in between, handing the answer over in pieces.
//! A Zimbra account's folders and messages are known to its server by the ids it hands out
//! ([`crate::AccountKind::server_ids`]), which stay with a message when it moves between folders;
//! what carries no Graph meaning is used again instead of being copied: [`FolderListing`],
//! [`FolderChange`], [`MessageChange`], [`DraftUp`], [`SendUp`], [`GraphFolder`], [`GraphMessage`],
//! [`GraphProgress`] and, for messages fetched whole, what that brings
//! ([`GraphFetchResult`](super::GraphFetchResult)).
//!
//! **The change token.** Where Exchange goes on from a sync state per folder, Zimbra goes on from
//! one **token** for the whole mailbox: what its `SyncRequest` gave at the end of the last run,
//! opaque. It is kept in the mailbox file, with what it brought, in one step
//! ([`crate::ZimbraServer::token`]). An answer says the token it went on from
//! ([`ZimbraMailResult::from`]) and, in the piece that ends the read, the one to keep now
//! ([`ZimbraMailResult::token`]). A token the server no longer takes is not sent again: the host
//! reads afresh and says so, folder by folder ([`ZimbraMessagePage::afresh`]), and what a fresh read
//! of a folder doesn't have is removed when it ends.
//!
//! **Folders** come as a complete listing (`GetFolderRequest` answers the whole tree, which is
//! small): [`FolderListing`], keyed by the folders' ids. Folders of appointments, contacts, tasks
//! and documents are not in it.
//!
//! **Server ids.** Zimbra names an item by a number that is the mailbox's own (`257`), or one of
//! another mailbox it is shared from (`<account id>:257`); both are opaque here: only the crate that
//! speaks to the server knows what they are made of. Each is at most [`MAX_ZIMBRA_ID`] bytes, with
//! no control character.
//!
//! No type shows the server's address, the user name, the account's address, a token or a message
//! in debug output: counts only. The password that opens the server is handed to the host beside
//! the request and is in neither, as is the sign-in token the server gives for it.

use serde::{Deserialize, Serialize};

use super::{
    DraftDone, DraftUp, FolderChange, FolderChangeDone, FolderListing, GraphFolder, GraphMessage, GraphProgress, MAX_LISTED_FOLDERS, MessageChange,
    MessageChangeDone, SendDone, SendUp, Timings, is_false,
};
use crate::Id;

/// Longest server id of a folder or an item, in bytes: a number, or an account's id (a UUID) and a
/// number joined by a colon.
pub const MAX_ZIMBRA_ID: usize = 128;

/// Most messages one page of an answer holds.
pub const MAX_ZIMBRA_PAGE_MESSAGES: usize = 2000;

/// Most pages one piece of an answer holds.
pub const MAX_ZIMBRA_PAGES: usize = 10_000;

/// Most problems one piece of an answer reports, and the longest, in characters.
pub const MAX_ZIMBRA_ERRORS: usize = 100;
pub const MAX_ZIMBRA_ERROR: usize = 1000;

/// What a run for a [`crate::AccountKind::Zimbra`] account is asked to do: the changes made here go
/// up, what waits in the Outbox is sent, the server's folders are listed, and the mailbox's
/// changes since `token` are read (all of it, folder by folder, when there is no token).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraMailRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the server's web client ([`crate::ZimbraServer::url`]). Never logged.
    pub url: String,
    /// The name the password is sent with ([`crate::ZimbraServer::username`]); empty means `email`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The fingerprint of the one certificate the owner trusts for this account besides the
    /// system's ([`crate::ZimbraServer::trusted`]); empty when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// Where the mailbox's changes go on from ([`crate::ZimbraServer::token`]); empty reads every
    /// folder from its start.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// The changes of folders made here, first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FolderChange>,
    /// The changes of messages made here, after the folder changes and before anything is read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageChange>,
    /// The drafts saved here that the server hasn't all of.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<DraftUp>,
    /// What waits in the Outbox, sent after the drafts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendUp>,
    /// The folders whose fresh read a run before this one had to stop in the middle of, and where
    /// it stopped: they are read to their end first. Kept in the mailbox file with the token
    /// ([`crate::ZimbraServer::resume`]), so a folder too large for one run is finished over as
    /// many runs as it takes instead of failing every one of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resume: Vec<ZimbraResume>,
    /// Nobody is waiting for this run: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// Where the fresh read of one folder goes on from (`ZimbraMailRequest::resume`,
/// `ZimbraMailResult::resume`): the folder's server id, and the cursor its last read page ended
/// on — the last item's own id and its sort value — or an empty cursor to read it from its start.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraResume {
    /// The folder's server id.
    pub folder: String,
    /// The id of the item the last page ended on; empty reads the folder from its start.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    /// The sort value the last page ended on (the item's `d`, in milliseconds).
    pub sort: u64,
}

/// Flags only: no id of a folder or an item shows.
impl std::fmt::Debug for ZimbraResume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZimbraResume").field("folder", &!self.folder.is_empty()).field("cursor", &!self.cursor.is_empty()).finish()
    }
}

/// Messages of a Zimbra account to fetch whole (their MIME text), by server id, in this order: the
/// counterpart of [`GraphFetchRequest`](super::GraphFetchRequest). It says where the server is, as
/// a [`ZimbraMailRequest`] does; what it brings back is a
/// [`GraphFetchResult`](super::GraphFetchResult). The password goes beside it and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraFetchRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the server's web client. Never logged.
    pub url: String,
    /// The name the password is sent with; empty means `email`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The fingerprint of the one certificate the owner trusts for this account; empty when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// The server ids of the messages to fetch, in the order they are wanted.
    pub ids: Vec<String>,
    /// Nobody is waiting: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// One page of what a run read: either a folder read from its start, or the mailbox's changes
/// since the token the run went on from.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraMessagePage {
    /// For a folder read from its start: the folder's server id. Empty for a page of changes, whose
    /// messages each say their folder ([`super::ServerMessage::folder`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// Messages new or changed, as headers and a preview, with the server's marks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<GraphMessage>,
    /// The server ids of the messages deleted from the mailbox.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// This page begins a fresh read of `folder`: what the read doesn't bring by its `last` page is
    /// removed then.
    #[serde(skip_serializing_if = "is_false")]
    pub afresh: bool,
    /// This page ends its read: the folder's, or the changes'.
    #[serde(skip_serializing_if = "is_false")]
    pub last: bool,
}

/// What a run for a Zimbra account brought back, or a piece of it.
///
/// A run arrives in pieces, in the order things were done (`partial` on all but the last), each
/// whole in itself: taking the pieces in one after the other, as far as they come, leaves the
/// mailbox as far along as the run got.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraMailResult {
    pub account: Id,
    /// The address of the account the run was for: an answer is not taken in by another account
    /// that happens to have the id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// The address of the server the run read (its request's): an answer is not taken in once the
    /// account names another server. Never logged.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// What became of the changes of folders sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FolderChangeDone>,
    /// What became of the changes of messages sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageChangeDone>,
    /// What became of the drafts taken up.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<DraftDone>,
    /// What became of the messages sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sent: Vec<SendDone>,
    /// Every folder of mail the server has, when they could be listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listing: Option<FolderListing>,
    /// The ids of the server's own folders (the Inbox and the other special folders), which are
    /// never renamed, moved or deleted ([`crate::Folder::server_owned`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub well_known: Vec<String>,
    /// The pages read, in the order they came: each is taken in by itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<ZimbraMessagePage>,
    /// The token the run went on from (its request's; empty: none). A piece is taken in only while
    /// the account is at that token, so that a run can never move the mailbox past what was lost.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// The token that goes on from this run, in the piece that ends its read: kept with it, in one
    /// step. Empty: none is kept.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// Where each folder's unfinished fresh read goes on from, in the same piece as `token`
    /// ([`ZimbraResume`]): kept with it in one step. `Some`, even when empty, in the piece that
    /// carries the token; `None` in every other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<Vec<ZimbraResume>>,
    /// Problems worth telling the owner; empty when all went well.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server said it was busy and to stay away this many seconds: all work for the account
    /// waits that long.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
    /// More of this run comes after this piece.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// Nothing went wrong in the run and it was not stopped: everything it meant to read was read
    /// to its end.
    #[serde(skip_serializing_if = "is_false")]
    pub whole: bool,
    /// The owner stopped the run (Cancel) after the piece in hand: the next one goes on from there.
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// How far the run has got, for the status bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<GraphProgress>,
    /// The server turned the password down: the owner is asked for it again, and the run is not
    /// repeated by itself.
    #[serde(skip_serializing_if = "is_false")]
    pub auth_refused: bool,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of a certificate the system doesn't trust
    /// and the account doesn't either: the run stopped there, and the owner is asked whether to
    /// trust it for this account.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub untrusted: String,
    /// How long this piece's part of the run took, and how many requests it made: measured by the
    /// host, which makes the requests.
    #[serde(skip_serializing_if = "Timings::is_empty")]
    pub timings: Timings,
}

/// Whether `id` may be a server id of a folder or an item: not empty, at most [`MAX_ZIMBRA_ID`]
/// bytes, no control character. Nothing else is assumed of it.
pub fn zimbra_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ZIMBRA_ID && !id.chars().any(char::is_control)
}

/// Longest subject, `Message-ID`, preview, name and address, in bytes, and the most people in a
/// list and categories, that are taken in at all (they are cut to what is shown after that).
const MAX_TEXT: usize = 16 * 1024;
const MAX_PEOPLE: usize = 1_000;
const MAX_CATEGORIES: usize = 200;

/// Whether a message of a page is within the sizes of its text.
fn message_content_ok(m: &GraphMessage) -> bool {
    let person = |a: &crate::Address| a.name.len() <= MAX_TEXT && a.email.len() <= MAX_TEXT;
    m.subject.len() <= MAX_TEXT
        && m.preview.len() <= MAX_TEXT
        && m.message_id.len() <= MAX_TEXT
        && m.server.change_key.len() <= MAX_ZIMBRA_ID
        && m.server.categories.len() <= MAX_CATEGORIES
        && m.server.categories.iter().all(|c| c.len() <= MAX_TEXT)
        && person(&m.from)
        && m.to.len() <= MAX_PEOPLE
        && m.cc.len() <= MAX_PEOPLE
        && m.to.iter().chain(&m.cc).all(person)
}

impl ZimbraMailResult {
    /// Whether the server answered a request of the run's with what it holds (its folders, or a
    /// page of messages): it took the password the run went with.
    pub fn server_answered(&self) -> bool {
        self.listing.is_some() || !self.pages.is_empty()
    }

    /// Whether this piece of an answer is within what is taken in, or why not. An answer comes from
    /// a host and may say anything: every list, string and id is held to a size here, before any of
    /// it is used. A piece that is refused is taken in by no part, so a cut-short answer removes
    /// nothing.
    pub fn check(&self) -> Result<(), &'static str> {
        const TOO_MANY: &str = "has more in it than is taken in";
        const ODD_ID: &str = "has a server id that can't be one";
        if self.email.chars().count() > crate::ZimbraServer::MAX_USERNAME || self.url.chars().count() > crate::ZimbraServer::MAX_URL {
            return Err("names an account or a server that is too long");
        }
        if crate::ZimbraServer::check_username(&self.username).is_err() {
            return Err("has a user name that can't be one");
        }
        if self.pages.len() > MAX_ZIMBRA_PAGES || self.errors.len() > MAX_ZIMBRA_ERRORS {
            return Err(TOO_MANY);
        }
        if self.changes.len() > super::MAX_FOLDER_CHANGES
            || self.messages.len() > super::MAX_MESSAGE_CHANGES
            || self.drafts.len() > MAX_ZIMBRA_PAGE_MESSAGES
            || self.sent.len() > MAX_ZIMBRA_PAGE_MESSAGES
        {
            return Err(TOO_MANY);
        }
        if !crate::ZimbraServer::token_ok(&self.from) || !crate::ZimbraServer::token_ok(&self.token) {
            return Err("has a change token that can't be one");
        }
        // The token and the folders' resume list travel in one piece: a token without it would
        // move the run on while half-read folders lose where they stopped.
        if !self.token.is_empty() && self.resume.is_none() {
            return Err("has a token but no resume");
        }
        if self.errors.iter().any(|e| e.chars().take(MAX_ZIMBRA_ERROR + 1).count() > MAX_ZIMBRA_ERROR) {
            return Err("has a problem told at too great a length");
        }
        // A change handed back is trusted to name its ids: one that can't be a server id, or a
        // text longer than is taken in, makes the whole piece suspect.
        let said = |s: &String| s.chars().take(MAX_ZIMBRA_ERROR + 1).count() <= MAX_ZIMBRA_ERROR;
        let names = |ns: &[String]| ns.len() <= MAX_CATEGORIES && ns.iter().all(|n| n.len() <= MAX_TEXT);
        let change_ok = |change: &super::MessageChange| -> bool {
            let id_ok = |id: &str| zimbra_id_ok(id) || super::marker_of(id).is_some_and(|m| m.len() <= MAX_ZIMBRA_ID);
            match change {
                super::MessageChange::Update { id, categories, .. } => {
                    id_ok(id) && categories.as_ref().is_none_or(|c| names(&c.here) && names(&c.synced))
                }
                super::MessageChange::Move { id, to, .. } | super::MessageChange::Copy { id, to, .. } => {
                    id_ok(id) && (to.is_empty() || zimbra_id_ok(to))
                }
                super::MessageChange::Delete { id } | super::MessageChange::MovedOut { id, .. } => id_ok(id),
            }
        };
        let outcome_ok = |outcome: &super::MessageOutcome| match outcome {
            super::MessageOutcome::Done { categories, folder, id, .. } => {
                categories.as_ref().is_none_or(|ns| names(ns)) && (folder.is_empty() || zimbra_id_ok(folder)) && (id.is_empty() || zimbra_id_ok(id))
            }
            _ => true,
        };
        if self
            .messages
            .iter()
            .any(|done| !change_ok(&done.change) || !outcome_ok(&done.outcome) || done.there.as_ref().is_some_and(|m| !message_content_ok(m)))
        {
            return Err("has a change of a message that can't be one");
        }
        let folder_change_ok = |change: &super::FolderChange| match change {
            super::FolderChange::Create { name, parent, .. } => name.len() <= MAX_TEXT && (parent.is_empty() || zimbra_id_ok(parent)),
            super::FolderChange::Rename { id, name, .. } => zimbra_id_ok(id) && name.len() <= MAX_TEXT,
            super::FolderChange::Move { id, parent, .. } => zimbra_id_ok(id) && (parent.is_empty() || zimbra_id_ok(parent)),
            super::FolderChange::Delete { id, shown } => {
                zimbra_id_ok(id)
                    && shown.name.len() <= MAX_TEXT
                    && shown.folders.len() <= MAX_LISTED_FOLDERS
                    && shown.messages.len() <= MAX_ZIMBRA_PAGE_MESSAGES
            }
        };
        let folder_ok = |folder: &GraphFolder| {
            (folder.id.is_empty() || zimbra_id_ok(&folder.id))
                && (folder.parent.is_empty() || zimbra_id_ok(&folder.parent))
                && folder.name.len() <= MAX_TEXT
        };
        if self.changes.iter().any(|done| {
            !folder_change_ok(&done.change)
                || match &done.outcome {
                    super::ChangeOutcome::Done { folder } => !folder_ok(folder),
                    super::ChangeOutcome::Refused { why } | super::ChangeOutcome::OwnFolder { why } => !said(why),
                    _ => false,
                }
        }) {
            return Err("has a change of a folder that can't be one");
        }
        if self.drafts.iter().any(|done| {
            !(done.id.is_empty() || zimbra_id_ok(&done.id) || super::marker_of(&done.id).is_some())
                || !(done.folder.is_empty() || zimbra_id_ok(&done.folder))
                || done.change_key.len() > MAX_ZIMBRA_ID
                || done.change.marker.len() > MAX_ZIMBRA_ID
                || done.change.subject.as_ref().is_some_and(|s| s.len() > MAX_TEXT)
        }) || self.sent.iter().any(|done| {
            let said_ok = !matches!(&done.outcome, super::SendOutcome::Held { why, .. } if !said(why));
            let left_ok = done.draft_left.is_empty() || zimbra_id_ok(&done.draft_left) || super::marker_of(&done.draft_left).is_some();
            !said_ok || !left_ok
        }) {
            return Err("has a draft or a send that can't be one");
        }
        if crate::ZimbraServer::check_fingerprint(&self.untrusted).is_err() {
            return Err("has a certificate fingerprint that can't be one");
        }
        if self.well_known.len() > MAX_LISTED_FOLDERS || !self.well_known.iter().all(|id| zimbra_id_ok(id)) {
            return Err(ODD_ID);
        }
        if self.resume.as_ref().is_some_and(|resume| {
            resume.len() > MAX_LISTED_FOLDERS || resume.iter().any(|e| !zimbra_id_ok(&e.folder) || (!e.cursor.is_empty() && !zimbra_id_ok(&e.cursor)))
        }) {
            return Err(ODD_ID);
        }
        if let Some(listing) = &self.listing {
            if listing.folders.len() > MAX_LISTED_FOLDERS {
                return Err(TOO_MANY);
            }
            let odd = |f: &GraphFolder| !zimbra_id_ok(&f.id) || (!f.parent.is_empty() && !zimbra_id_ok(&f.parent)) || f.name.len() > 4096;
            if listing.folders.iter().any(odd) {
                return Err(ODD_ID);
            }
        }
        for page in &self.pages {
            if page.messages.len() > MAX_ZIMBRA_PAGE_MESSAGES || page.removed.len() > MAX_ZIMBRA_PAGE_MESSAGES {
                return Err(TOO_MANY);
            }
            if page.messages.iter().any(|m| !message_content_ok(m)) {
                return Err("has a message whose text is longer than is taken in");
            }
            // A page of changes names each message's folder; a folder's page names its folder.
            let message_odd = |m: &GraphMessage| !zimbra_id_ok(&m.server.id) || (!m.server.folder.is_empty() && !zimbra_id_ok(&m.server.folder));
            if (!page.folder.is_empty() && !zimbra_id_ok(&page.folder))
                || (page.afresh && page.folder.is_empty())
                || page.messages.iter().any(message_odd)
                || !page.removed.iter().all(|id| zimbra_id_ok(id))
            {
                return Err(ODD_ID);
            }
        }
        Ok(())
    }

    /// Take in the next piece of the same run, for a caller that takes a run in at once: the pages
    /// and the rest in the order they came, and what the last piece says of the end.
    pub fn absorb(&mut self, piece: ZimbraMailResult) {
        self.account = piece.account;
        for (mine, theirs) in [
            (&mut self.email, piece.email),
            (&mut self.url, piece.url),
            (&mut self.username, piece.username),
            (&mut self.from, piece.from),
            (&mut self.token, piece.token),
        ] {
            if !theirs.is_empty() {
                *mine = theirs;
            }
        }
        if let Some(resume) = piece.resume {
            self.resume = Some(resume);
        }
        self.changes.extend(piece.changes);
        self.messages.extend(piece.messages);
        self.drafts.extend(piece.drafts);
        self.sent.extend(piece.sent);
        if let Some(listing) = piece.listing {
            self.listing = Some(listing);
        }
        self.well_known.extend(piece.well_known);
        self.pages.extend(piece.pages);
        self.errors.extend(piece.errors);
        self.busy_for = piece.busy_for.or(self.busy_for);
        (self.partial, self.whole, self.stopped) = (piece.partial, piece.whole, piece.stopped);
        if piece.progress.is_some() {
            self.progress = piece.progress;
        }
        self.auth_refused |= piece.auth_refused;
        if !piece.untrusted.is_empty() {
            self.untrusted = piece.untrusted;
        }
        self.timings.absorb(&piece.timings);
    }
}

/// Counts and flags: neither a server's id nor a token is ever shown.
impl std::fmt::Debug for ZimbraMessagePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZimbraMessagePage")
            .field("folder", &!self.folder.is_empty())
            .field("messages", &self.messages.len())
            .field("removed", &self.removed.len())
            .field("afresh", &self.afresh)
            .field("last", &self.last)
            .finish()
    }
}

/// Counts only: the server's address, the user name, the account's address, every id and the
/// token, and every word of a message stay out of debug output.
impl std::fmt::Debug for ZimbraMailRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZimbraMailRequest")
            .field("account", &self.account)
            .field("trusted", &!self.trusted.is_empty())
            .field("token", &!self.token.is_empty())
            .field("changes", &self.changes.len())
            .field("messages", &self.messages.len())
            .field("drafts", &self.drafts.len())
            .field("sends", &self.sends.len())
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as a run's request: no address, and no id of a message.
impl std::fmt::Debug for ZimbraFetchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZimbraFetchRequest")
            .field("account", &self.account)
            .field("trusted", &!self.trusted.is_empty())
            .field("ids", &self.ids.len())
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as the request's.
impl std::fmt::Debug for ZimbraMailResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let messages: usize = self.pages.iter().map(|page| page.messages.len()).sum();
        f.debug_struct("ZimbraMailResult")
            .field("account", &self.account)
            .field("changes", &self.changes.len())
            .field("messages_changed", &self.messages.len())
            .field("drafts", &self.drafts.len())
            .field("sent", &self.sent.len())
            .field("listing", &self.listing.as_ref().map(|listing| (listing.folders.len(), listing.complete)))
            .field("pages", &self.pages.len())
            .field("messages", &messages)
            .field("token", &!self.token.is_empty())
            .field("resume", &self.resume.as_ref().map_or(0, Vec::len))
            .field("errors", &self.errors.len())
            .field("busy_for", &self.busy_for)
            .field("partial", &self.partial)
            .field("whole", &self.whole)
            .field("stopped", &self.stopped)
            .field("progress", &self.progress.is_some())
            .field("auth_refused", &self.auth_refused)
            .field("untrusted", &!self.untrusted.is_empty())
            .field("timings", &self.timings)
            .finish()
    }
}

#[cfg(test)]
#[path = "zimbra_tests.rs"]
mod tests;
