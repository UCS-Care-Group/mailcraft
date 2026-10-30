// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox kept in step through Exchange Web Services: what a run is asked to do and what it
//! brings back (#252).
//!
//! **Shapes only for now**: nothing speaks the protocol yet, and a run that is asked for answers
//! that it is not built. The shapes are here so that the pieces can be built against them one at a
//! time, as an account's kind and settings ([`crate::AccountKind::Exchange`],
//! [`crate::EwsServer`]) are.
//!
//! The counterpart of [`JmapMailRequest`](super::JmapMailRequest) and
//! [`GraphMailRequest`](super::GraphMailRequest), beside them rather than bent into them. Plain
//! data: the engine builds an [`EwsMailRequest`] and takes in an [`EwsMailResult`] (both pure), and
//! a host with sockets does the talking in between, usually as a task that hands the answer over in
//! pieces. An Exchange server account's folders and messages are known to its server by the ids it
//! hands out ([`crate::AccountKind::server_ids`]); what carries no Graph meaning is used again
//! instead of being copied: [`FolderChange`], [`MessageChange`], [`DraftUp`], [`SendUp`],
//! [`PlaceUp`], [`GraphFolder`], [`GraphMessage`], [`GraphProgress`] and, for messages fetched
//! whole, what that brings ([`GraphFetchResult`](super::GraphFetchResult)).
//!
//! **Sync state.** Where Graph and JMAP continue a folder from a link or a window, Exchange
//! continues from the **sync state** the server gave at the end of the last read
//! (`SyncFolderHierarchy`, `SyncFolderItems`): an opaque string. It is kept in the mailbox file,
//! with what it brought, in one step: the account's is [`crate::EwsServer::hierarchy`], a folder's
//! is [`crate::ServerFolder::sync_state`]. An answer carries them in
//! [`EwsMailResult::hierarchy`] and [`EwsMailResult::states`], and taking a piece in puts a state
//! and the folders and messages it brought in together, like Graph's links; `link` and
//! `caught_up` of Graph's [`MessagePage`](super::MessagePage) are never used for Exchange, which
//! has pages of its own ([`EwsMessagePage`]). A state the server no longer knows
//! (`ErrorInvalidSyncStateData`) is not sent again: the host reads afresh and says so
//! (`afresh`), and what the fresh read doesn't have is removed when it ends.
//!
//! **Folders** come as deltas (`SyncFolderHierarchy` returns created, updated and deleted
//! folders), not as a complete listing: [`EwsFolderDeltas`], keyed by the folders' opaque ids.
//!
//! **Server ids.** An Exchange item is named by an `Id` and a `ChangeKey`, and the change key
//! changes with the item. Here both are one opaque string (`id` of [`ServerMessage`](super::ServerMessage)
//! and of [`GraphFolder`]), whose encoding only `mailcraft-ews` knows; nothing else looks inside
//! it or relies on its shape. It is at most [`MAX_EWS_ID`] bytes, with no control character.
//!
//! No type shows the server's address, the user name, the account's address, a sync state or a
//! message in debug output: counts only. The password that opens the server is handed to the host
//! beside the request and is in neither.

use serde::{Deserialize, Serialize};

use super::{
    DraftDone, DraftUp, FolderChange, FolderChangeDone, GraphFolder, GraphMessage, GraphProgress, MAX_LISTED_FOLDERS, MessageChange,
    MessageChangeDone, PlaceDone, PlaceUp, ReadState, SendDone, SendUp, is_false,
};
use crate::Id;

/// Longest server id of a folder or an item, in bytes ([`crate::AccountKind::Exchange`]'s ids hold
/// an id and a change key in one opaque string).
pub const MAX_EWS_ID: usize = 1024;

/// Most messages one page of an answer holds.
pub const MAX_EWS_PAGE_MESSAGES: usize = 2000;

/// Most pages one piece of an answer holds.
pub const MAX_EWS_PAGES: usize = 10_000;

/// Most problems one piece of an answer reports, and the longest, in characters.
pub const MAX_EWS_ERRORS: usize = 100;
pub const MAX_EWS_ERROR: usize = 1000;

/// Most folders whose sync state a request or an answer carries.
pub const MAX_EWS_STATES: usize = 10_000;

/// Longest sync state, in bytes, that is kept or handed back: the server's own are short, so a
/// longer one is not one.
pub const MAX_EWS_STATE: usize = 8192;

/// Which folders a run reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EwsScope {
    /// The Inbox and the folders named (the server ids of those on screen): the cheap run that
    /// keeps what the owner is looking at fresh.
    Light {
        #[serde(default)]
        also: Vec<String>,
    },
    /// Every folder of the account.
    #[default]
    All,
}

impl EwsScope {
    /// A word for it that names no folder.
    fn describe(&self) -> String {
        match self {
            EwsScope::All => "all".into(),
            EwsScope::Light { also } => format!("light+{}", also.len()),
        }
    }
}

/// Where a folder's mail goes on from: the sync state the server gave at the end of the last read
/// of the folder (`SyncFolderItems`), by the folder's server id ([`crate::ServerFolder::sync_state`]).
/// In an answer, the state to keep now, taken in together with the page that brought it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsFolderState {
    /// The folder's server id.
    pub folder: String,
    /// The sync state: opaque.
    pub state: String,
}

/// What a run for a [`crate::AccountKind::Exchange`] account is asked to do: the changes made here
/// go up, what waits in the Outbox is sent, the server's folders are listed if they should be, and
/// the folders in `scope` are read from their sync states.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsMailRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against, and the mailbox whose requests
    /// are counted together.
    pub email: String,
    /// The address of the Exchange Web Services endpoint ([`crate::EwsServer::url`]). Never logged.
    pub url: String,
    /// The name the password is sent with ([`crate::EwsServer::username`]); empty means `email`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The fingerprint of the one certificate the owner trusts for this account besides the
    /// system's ([`crate::EwsServer::trusted`]); empty when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// Read the server's folder hierarchy in this run (`SyncFolderHierarchy` from `hierarchy`): the
    /// first run, a run that reads every folder, and the run after one that brought no folders.
    #[serde(skip_serializing_if = "is_false")]
    pub list_folders: bool,
    /// Where the folder hierarchy goes on from ([`crate::EwsServer::hierarchy`]); empty reads it
    /// from its start.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hierarchy: String,
    /// Which folders are read.
    pub scope: EwsScope,
    /// Where each folder's mail goes on from; a folder that isn't here is read from its start.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<EwsFolderState>,
    /// The changes of folders made here, first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FolderChange>,
    /// The changes of messages made here, after the folder changes and before the folders are read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageChange>,
    /// The drafts saved here that the server hasn't all of.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<DraftUp>,
    /// What waits in the Outbox, sent after the drafts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendUp>,
    /// Messages to make in its folders, moved or copied here from another mailbox of the same
    /// owner's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<PlaceUp>,
    /// The Inbox pass: the run is only the Inbox's changes from its sync state, and nothing else.
    #[serde(skip_serializing_if = "is_false")]
    pub inbox_pass: bool,
    /// A full sweep: every folder is read, none is passed over for having the same counts as
    /// before.
    #[serde(skip_serializing_if = "is_false")]
    pub full: bool,
    /// Nobody is waiting for this run: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// Messages of an Exchange server account to fetch whole (their MIME content), by server id, in
/// this order: the counterpart of [`GraphFetchRequest`](super::GraphFetchRequest).
///
/// It says where the server is, as an [`EwsMailRequest`] does, because a host that has to connect
/// for it has no other way to know. What it brings back is the same
/// [`GraphFetchResult`](super::GraphFetchResult) a Microsoft account's fetch brings. The password
/// goes beside the request and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsFetchRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the Exchange Web Services endpoint. Never logged.
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

/// What `SyncFolderHierarchy` brought: the folders created or updated (the server's whole record of
/// each, by its opaque id) and the ids of the folders deleted since the state the run began from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsFolderDeltas {
    /// Folders created or updated.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<GraphFolder>,
    /// The ids of the folders deleted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<String>,
    /// The state the run began from was lost (the server no longer knew it, or there was none): this
    /// is every folder the server has, and a folder of the account that isn't in `changed` is
    /// removed when the run ends. Otherwise it is a delta and nothing is removed for not being in it.
    #[serde(skip_serializing_if = "is_false")]
    pub afresh: bool,
}

/// One page of a folder's changes (`SyncFolderItems`). The state to go on from is not here but in
/// [`EwsMailResult::states`], taken in with it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsMessagePage {
    /// The folder's server id.
    pub folder: String,
    /// Messages created or updated, whole as the server has them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<GraphMessage>,
    /// The server ids of the messages deleted from the folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Messages whose read state alone changed (`SyncFolderItems`' `ReadFlagChange`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<ReadState>,
    /// This page reaches the last change the server has (`IncludesLastItemInRange`): the folder is
    /// up to date after it.
    #[serde(skip_serializing_if = "is_false")]
    pub last: bool,
    /// The folder's state was lost (`ErrorInvalidSyncStateData`): this page begins a fresh read of
    /// the folder, and what the fresh read doesn't have is removed when it ends (`last`).
    #[serde(skip_serializing_if = "is_false")]
    pub afresh: bool,
}

/// What a run for an Exchange server account brought back, or a piece of it.
///
/// A run arrives in pieces, in the order things were done (`partial` on all but the last), each
/// whole in itself: taking the pieces in one after the other, as far as they come, leaves the
/// mailbox as far along as the run got.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsMailResult {
    pub account: Id,
    /// The address of the account the run was for: an answer is not taken in by another account
    /// that happens to have the id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// The address of the endpoint the run read (its request's): an answer is not taken in once
    /// the account names another server. Never logged.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
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
    /// What became of the messages put into its folders from another mailbox of the same owner's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<PlaceDone>,
    /// What changed in the server's folders since the hierarchy's state of the request, when they
    /// could be read at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders: Option<EwsFolderDeltas>,
    /// Where the folder hierarchy goes on from now ([`crate::EwsServer::hierarchy`]): taken in
    /// with `folders`, and empty when this piece read none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hierarchy: String,
    /// The pages of messages read, in the order they came: each is taken in by itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<EwsMessagePage>,
    /// Where each folder read goes on from now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<EwsFolderState>,
    /// Problems worth telling the owner; empty when all went well.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server said it was busy and to stay away this many seconds (`ErrorServerBusy`): all
    /// work for the account waits that long.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
    /// More of this run comes after this piece.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// Nothing went wrong in the run and it was not stopped: every folder it meant to read was
    /// read to its end.
    #[serde(skip_serializing_if = "is_false")]
    pub whole: bool,
    /// The owner stopped the run (Cancel) after the piece in hand: the next one goes on from there.
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// How far the run has got, for the status bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<GraphProgress>,
    /// The server turned the password down (a 401): the owner is asked for it again, and the run
    /// is not repeated by itself.
    #[serde(skip_serializing_if = "is_false")]
    pub auth_refused: bool,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of a certificate the system doesn't trust
    /// and the account doesn't either: the run stopped there, and the owner is asked whether to
    /// trust it for this account.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub untrusted: String,
}

/// Whether `id` may be a server id of a folder or an item: not empty, at most [`MAX_EWS_ID`] bytes,
/// no control character. Nothing else is assumed of it.
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_EWS_ID && !id.chars().any(char::is_control)
}

/// Whether `state` may be a sync state: at most [`MAX_EWS_STATE`] bytes, no control character.
fn state_ok(state: &str) -> bool {
    state.len() <= MAX_EWS_STATE && !state.chars().any(char::is_control)
}

impl EwsMailResult {
    /// Whether this piece of an answer is within what is taken in, or why not. An answer comes
    /// from a host and may say anything: every list, string and id is held to a size here, before
    /// any of it is used. A piece that is refused is taken in by no part, so a cut-short answer
    /// removes nothing.
    pub fn check(&self) -> Result<(), &'static str> {
        const TOO_MANY: &str = "has more in it than is taken in";
        const ODD_ID: &str = "has a server id that can't be one";
        if self.email.len() > crate::EwsServer::MAX_USERNAME || self.url.len() > crate::EwsServer::MAX_URL {
            return Err("names an account or a server that is too long");
        }
        if self.states.len() > MAX_EWS_STATES || self.pages.len() > MAX_EWS_PAGES || self.errors.len() > MAX_EWS_ERRORS {
            return Err(TOO_MANY);
        }
        if self.changes.len() > super::MAX_FOLDER_CHANGES
            || self.messages.len() > super::MAX_MESSAGE_CHANGES
            || self.drafts.len() > MAX_EWS_PAGE_MESSAGES
            || self.sent.len() > MAX_EWS_PAGE_MESSAGES
            || self.placed.len() > MAX_EWS_PAGE_MESSAGES
        {
            return Err(TOO_MANY);
        }
        if !state_ok(&self.hierarchy) {
            return Err("has a folder state that can't be one");
        }
        if self.errors.iter().any(|e| e.chars().take(MAX_EWS_ERROR + 1).count() > MAX_EWS_ERROR) {
            return Err("has a problem told at too great a length");
        }
        if crate::EwsServer::check_fingerprint(&self.untrusted).is_err() {
            return Err("has a certificate fingerprint that can't be one");
        }
        for state in &self.states {
            if !id_ok(&state.folder) {
                return Err(ODD_ID);
            }
            if !state_ok(&state.state) {
                return Err("has a sync state that can't be one");
            }
        }
        if let Some(folders) = &self.folders {
            if folders.changed.len() > MAX_LISTED_FOLDERS || folders.deleted.len() > MAX_LISTED_FOLDERS {
                return Err(TOO_MANY);
            }
            let odd = |f: &GraphFolder| !id_ok(&f.id) || (!f.parent.is_empty() && !id_ok(&f.parent)) || f.name.len() > 4096;
            if folders.changed.iter().any(odd) || !folders.deleted.iter().all(|id| id_ok(id)) {
                return Err(ODD_ID);
            }
        }
        for page in &self.pages {
            if page.messages.len() > MAX_EWS_PAGE_MESSAGES || page.removed.len() > MAX_EWS_PAGE_MESSAGES || page.read.len() > MAX_EWS_PAGE_MESSAGES {
                return Err(TOO_MANY);
            }
            let message_odd = |m: &GraphMessage| !id_ok(&m.server.id) || (!m.server.folder.is_empty() && !id_ok(&m.server.folder));
            if !id_ok(&page.folder)
                || page.messages.iter().any(message_odd)
                || !page.removed.iter().all(|id| id_ok(id))
                || !page.read.iter().all(|r| id_ok(&r.id))
            {
                return Err(ODD_ID);
            }
        }
        Ok(())
    }

    /// Take in the next piece of the same run, for a caller that takes a run in at once: the pages
    /// and the rest in the order they came, and what the last piece says of the end.
    pub fn absorb(&mut self, piece: EwsMailResult) {
        self.account = piece.account;
        if !piece.email.is_empty() {
            self.email = piece.email;
        }
        if !piece.url.is_empty() {
            self.url = piece.url;
        }
        self.changes.extend(piece.changes);
        self.messages.extend(piece.messages);
        self.drafts.extend(piece.drafts);
        self.sent.extend(piece.sent);
        self.placed.extend(piece.placed);
        if let Some(more) = piece.folders {
            let folders = self.folders.get_or_insert_with(EwsFolderDeltas::default);
            folders.changed.extend(more.changed);
            folders.deleted.extend(more.deleted);
            folders.afresh |= more.afresh;
        }
        if !piece.hierarchy.is_empty() {
            self.hierarchy = piece.hierarchy;
        }
        self.pages.extend(piece.pages);
        self.states.extend(piece.states);
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
    }
}

/// A folder's sync state is the server's to say and never ours to show.
impl std::fmt::Debug for EwsFolderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsFolderState").field("state", &!self.state.is_empty()).finish()
    }
}

/// Counts only: the server's address, the user name, the account's address, every id and sync
/// state, and every word of a message stay out of debug output.
impl std::fmt::Debug for EwsMailRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsMailRequest")
            .field("account", &self.account)
            .field("trusted", &!self.trusted.is_empty())
            .field("list_folders", &self.list_folders)
            .field("scope", &self.scope.describe())
            .field("states", &self.states.len())
            .field("changes", &self.changes.len())
            .field("messages", &self.messages.len())
            .field("drafts", &self.drafts.len())
            .field("sends", &self.sends.len())
            .field("places", &self.places.len())
            .field("inbox_pass", &self.inbox_pass)
            .field("full", &self.full)
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as a run's request: no address, and no id of a message.
impl std::fmt::Debug for EwsFetchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsFetchRequest")
            .field("account", &self.account)
            .field("trusted", &!self.trusted.is_empty())
            .field("ids", &self.ids.len())
            .field("background", &self.background)
            .finish()
    }
}

/// Counts only, as the request's.
impl std::fmt::Debug for EwsMailResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let messages: usize = self.pages.iter().map(|page| page.messages.len()).sum();
        f.debug_struct("EwsMailResult")
            .field("account", &self.account)
            .field("changes", &self.changes.len())
            .field("messages_changed", &self.messages.len())
            .field("drafts", &self.drafts.len())
            .field("sent", &self.sent.len())
            .field("placed", &self.placed.len())
            .field("folders", &self.folders.as_ref().map(|folders| (folders.changed.len(), folders.deleted.len(), folders.afresh)))
            .field("pages", &self.pages.len())
            .field("messages", &messages)
            .field("states", &self.states.len())
            .field("errors", &self.errors.len())
            .field("busy_for", &self.busy_for)
            .field("partial", &self.partial)
            .field("whole", &self.whole)
            .field("stopped", &self.stopped)
            .field("progress", &self.progress.is_some())
            .field("auth_refused", &self.auth_refused)
            .field("untrusted", &!self.untrusted.is_empty())
            .finish()
    }
}

#[cfg(test)]
#[path = "ews_tests.rs"]
mod tests;
