// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a mail-server synchronisation is asked to do and what it brings back.
//!
//! Plain data, so the pieces stay apart: the engine builds a [`SyncRequest`] and applies a
//! [`SyncResult`] (both pure), and a host with sockets (`mailcraft-net`, on the desktop and in the
//! CLI) does the talking in between, usually on a background thread.

use serde::{Deserialize, Serialize};

use crate::{Account, Address, FlagStatus, FolderCounts, FolderKind, Id, Importance, Message};
use mailcraft_time::Timestamp;

mod drafts;
mod ews;
mod jmap;
mod place;
#[cfg(test)]
mod problems_tests;
mod progress;
mod search;
pub use drafts::*;
pub use ews::*;
pub use jmap::*;
pub use place::*;
pub use progress::*;
pub use search::*;

/// A folder as the mailbox knows it, for the server side to continue from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderState {
    pub id: Id,
    /// Its path on the server; empty for a folder that only exists locally.
    pub remote: String,
    pub kind: FolderKind,
    pub uid_validity: u32,
    /// The highest UID already downloaded.
    pub uid_seen: u32,
    /// The oldest UID of the newest messages the folder was last read down to: older ones were
    /// left out by the limit of how many to have here, and come down when it grows. Between this
    /// and `uid_seen`, what isn't here left the folder here and is not asked for again. 0 until
    /// the folder is read with it (a mailbox from before it was kept).
    pub uid_from: u32,
    /// UIDs of the messages the mailbox has in this folder (to notice deletions and flag changes).
    pub known_uids: Vec<u32>,
    /// UIDs of messages here that are to be downloaded again, whole, to be read anew (#650:
    /// mail kept with U+FFFD). A few at a time; the run asks for them without setting `\Seen`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub again: Vec<u32>,
    /// What the server's STATUS said of the folder when it was last read whole, and is still
    /// good to pass it over by (#276); `None` when it isn't. The engine drops it once it is
    /// too old, so that every folder is read in full now and then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::ImapStatus>,
}

/// A change made here for the server to take on. Server mailboxes are named by their path
/// ([`FolderState::remote`]), messages by their UID there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteOp {
    /// Set or clear flags of a message. Only the flags that were changed here are named; `None`
    /// leaves that flag as the server has it (another program may have changed it meanwhile).
    Flags {
        folder: String,
        uid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flagged: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered: Option<bool>,
    },
    /// The message that is `uid` in `from` belongs in local folder `to_folder` now. With `create`
    /// empty, `to` is that folder's server path. A folder that only exists here is made on the
    /// server first: `create` holds its names below `to`, the server path of the nearest folder
    /// above it that the server has (empty: the top level).
    #[serde(rename_all = "camelCase")]
    Move {
        message: Id,
        from: String,
        uid: u32,
        to: String,
        to_folder: Id,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        create: Vec<String>,
    },
    /// The message that is `uid` in `from` was deleted for good.
    Delete { from: String, uid: u32 },
}

impl RemoteOp {
    /// The server mailbox the change is made in.
    pub fn mailbox(&self) -> &str {
        match self {
            RemoteOp::Flags { folder, .. } => folder,
            RemoteOp::Move { from, .. } | RemoteOp::Delete { from, .. } => from,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncRequest {
    pub account: Account,
    /// Never written to disk or logs.
    #[serde(skip)]
    pub password: String,
    /// The OAuth access token of an account that signs in that way ([`Account::oauth`]), in place
    /// of a password. Never written to disk or logs.
    #[serde(skip)]
    pub access_token: String,
    pub folders: Vec<FolderState>,
    /// Messages waiting in the Outbox, to send first.
    pub outbox: Vec<Message>,
    /// How many of each folder's newest messages to have here (older ones come down when this
    /// grows); 0 means no limit.
    pub max_per_folder: u32,
    /// Only send what waits and tell the server what changed here; bring nothing down. For a
    /// change that should reach the server at once, without reading every folder for it.
    #[serde(skip_serializing_if = "is_false")]
    pub push_only: bool,
    /// Changes made here since the server last heard, to push before anything is downloaded.
    /// A change is only ever made in a mailbox whose `UIDVALIDITY` is still the one `folders`
    /// has for it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<RemoteOp>,
    /// Messages to put on the server, moved or copied into the account's folders from another
    /// account or from this computer (#451); after `ops`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<ImapPlace>,
    /// Messages of a Gmail account's Inbox the owner moved to another of Gmail's categories here
    /// (#911), for Gmail's API: after `ops`, before anything is read. Only for an account that
    /// signs in with Google, and only while moves are switched on; empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gmail_moves: Vec<GmailMove>,
}

/// Most moves between Gmail's categories asked of Gmail's API in one run ([`SyncRequest::gmail_moves`]):
/// the rest wait for the next.
pub const MAX_GMAIL_MOVES: usize = 500;

/// One message moved to another of Gmail's categories here ([`SyncRequest::gmail_moves`]), or one
/// Gmail took the move of ([`SyncResult::gmail_moved`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailMove {
    /// The message here.
    pub message: Id,
    /// Gmail's own id of it (`X-GM-MSGID`), which its API knows it by (in hex).
    pub id: u64,
    /// The category it goes to.
    pub category: crate::GmailCategory,
}

/// A message to put in one of the server's mailboxes ([`SyncRequest::places`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImapPlace {
    /// The whole message: its fields, its body and its files.
    pub message: Message,
    /// Where it goes, as [`RemoteOp::Move`] says it: the local folder `to_folder`, whose server
    /// path is `to`; or, for a folder that only exists here, `create` holds its names below `to`,
    /// the path of the nearest folder above it that the server has (empty: the top level).
    pub to: String,
    pub to_folder: Id,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub create: Vec<String>,
    /// The marker the copy carries ([`crate::Queued::PlaceMessage`]): a copy on the server is
    /// taken for this one only when it carries it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub marker: String,
}

impl SyncRequest {
    /// What the servers are shown to prove who is asking: the access token of an OAuth account,
    /// the password of any other.
    pub fn secret(&self) -> &str {
        if self.account.oauth.is_some() { &self.access_token } else { &self.password }
    }
}

/// The password and the token stay out of debug output.
impl std::fmt::Debug for SyncRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |secret: &str| if secret.is_empty() { "" } else { "…" };
        f.debug_struct("SyncRequest")
            .field("account", &self.account.email)
            .field("password", &hidden(&self.password))
            .field("access_token", &hidden(&self.access_token))
            .field("folders", &self.folders.len())
            .field("outbox", &self.outbox.len())
            .field("ops", &self.ops.len())
            .field("places", &self.places.len())
            .field("gmail_moves", &self.gmail_moves.len())
            .finish()
    }
}

/// What the watch on an IMAP account's Inbox (IDLE, #276) tells the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleNote {
    /// The server says the Inbox has more messages than it had: a Send/Receive of the account
    /// brings them.
    Arrived,
    /// The watch is over, and why.
    Ended(IdleEnd),
}

/// Why a watch on an Inbox ended ([`IdleNote::Ended`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleEnd {
    /// It was told to stop.
    Stopped,
    /// The server can't do IDLE (or won't, for this mailbox): the account keeps to its timer.
    NoIdle,
    /// The server wouldn't have the sign-in, or there was nothing to sign in with: asking again
    /// with the same credentials changes nothing.
    Refused,
    /// The connection kept dropping: the watch gave up for now, not to make a server's life hard.
    TooOften,
}

/// What an OAuth sign-in brought back. Kept in memory only: it has no way to be written out, and
/// it doesn't show in debug output.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Shown to the mail servers; good for about an hour.
    pub access: String,
    /// Trades for a new access token without asking the owner again; may be empty.
    pub refresh: String,
    /// When the access token stops working (Unix seconds).
    pub expires_at: i64,
    /// Who signed in, as the provider names them (may be empty): for the account's display name.
    pub name: String,
    /// The address the provider knows them by (may be empty).
    pub email: String,
    /// The directory they signed in to, as the provider names it (may be empty): Microsoft's
    /// identity token says it, and it tells an organisation's account from a personal one
    /// ([`crate::Directory::of_tenant`]). An identifier, not a secret; kept in memory like the rest.
    pub tenant: String,
}

/// How long before an access token runs out it is renewed rather than used (seconds).
pub const FRESH_MARGIN: i64 = 300;

impl Tokens {
    /// Whether the access token is still good at `now` (Unix seconds), with five minutes to
    /// spare: one that runs out sooner is renewed before a job starts with it, not during it
    /// (#306). `now` is the wall clock, so time the computer spent asleep counts.
    pub fn fresh(&self, now: i64) -> bool {
        !self.access.is_empty() && self.expires_at.saturating_sub(FRESH_MARGIN) > now
    }

    /// Whether these are somebody else's than the account's whose address is `email` and whose
    /// user name is `username` (either may be empty): the provider named who signed in, and it
    /// is neither, whatever the case. Tokens that name nobody are taken to be the account's.
    pub fn someone_else(&self, email: &str, username: &str) -> bool {
        let who = self.email.trim();
        let is = |name: &str| !name.trim().is_empty() && name.trim().eq_ignore_ascii_case(who);
        !who.is_empty() && !is(email) && !is(username)
    }
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |secret: &str| if secret.is_empty() { "" } else { "…" };
        f.debug_struct("Tokens")
            .field("access", &hidden(&self.access))
            .field("refresh", &hidden(&self.refresh))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A message's flags on the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteFlags {
    pub uid: u32,
    pub read: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
}

impl RemoteFlags {
    /// What `message` shows now, as a server would hold it under `uid`. A follow-up flag that
    /// is complete is no flag there.
    pub fn of(message: &Message, uid: u32) -> RemoteFlags {
        RemoteFlags { uid, read: message.read, flagged: message.flag == FlagStatus::Flagged, answered: message.answered, draft: message.draft }
    }

    /// The same flags under another UID (the message moved).
    pub fn at(self, uid: u32) -> RemoteFlags {
        RemoteFlags { uid, ..self }
    }
}

/// A message as a Microsoft account's server knows it (ADR 0003): its server id (Graph's
/// immutable id, which stays the same when the message moves within the mailbox) and what the
/// server last said of the properties a change made here is worked out from. The difference
/// between these and the message as it is now is what a run sends (R3.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerMessage {
    pub id: String,
    /// The server id of the folder it is in.
    pub folder: String,
    pub read: bool,
    /// The server's `$answered` and `$forwarded` marks as last said (JMAP keywords). A Graph
    /// server says neither: they stay false for a Microsoft account, and nothing is written.
    #[serde(skip_serializing_if = "is_false")]
    pub answered: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub forwarded: bool,
    pub flag: FlagStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    pub importance: Importance,
    /// The server said it left `folder` (deleted, or moved out: Graph's `@removed`). It is kept
    /// until every folder's delta has ended in the run, in case it turns up in another folder;
    /// still gone from every folder then, it is removed here (R3 spec S9).
    #[serde(skip_serializing_if = "is_false")]
    pub gone: bool,
    /// Its size on the server in bytes, when the server said (0: not said): what the background
    /// fetch's limit is held against (R3 spec S12).
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub size: u64,
    /// The follow-up flag's dates as the server has them (R3.3, S16): when it was flagged, when
    /// it is due, and when it was marked complete. A file from before has none.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub flag_start: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub flag_due: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub flag_completed: Option<Timestamp>,
    /// The server's change key of it as last seen: for a draft, what tells an edit made elsewhere
    /// from the server's copy of one made here (R3.4.1, S26). Empty when not known.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// The server's id of the conversation it is in (#795): Graph's `conversationId`, EWS's
    /// `ConversationId`, JMAP's `threadId`. Opaque, never shown, and sent nowhere but back to
    /// its own server; what [`crate::ThreadIndex`] links a conversation by, with the headers.
    /// Printable ASCII of at most [`MAX_THREAD_ID`] bytes ([`thread_id`]); empty when the server
    /// hasn't said (a listing from before it was asked for), and it goes with the rest of this
    /// when the message leaves the account.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread: String,
    /// It was asked for its `thread` after the listing that brought it without one (the look-up of
    /// #795), and the server gave none, or refused [`MAX_THREAD_TRIES`] times: it isn't asked again.
    #[serde(skip_serializing_if = "is_false")]
    pub thread_asked: bool,
    /// How many runs its server refused to say its `thread` (an error for the message, or the
    /// whole question refused; never a busy server or a lost connection): at
    /// [`MAX_THREAD_TRIES`] it is marked [`ServerMessage::thread_asked`], so that a server that
    /// never will isn't asked at every run.
    #[serde(skip_serializing_if = "is_zero")]
    pub thread_tries: u32,
}

/// Most runs a message's server may refuse to say its conversation before it isn't asked again.
pub const MAX_THREAD_TRIES: u32 = 3;

/// The longest server conversation id kept ([`ServerMessage::thread`]), in bytes.
pub const MAX_THREAD_ID: usize = 256;

/// A server's conversation id as it is kept ([`ServerMessage::thread`]): printable ASCII of at
/// most [`MAX_THREAD_ID`] bytes, else none. What a server or a file says is hostile.
pub fn thread_id(said: &str) -> String {
    let said = said.trim();
    if said.len() <= MAX_THREAD_ID && said.bytes().all(|b| b.is_ascii_graphic()) { said.to_string() } else { String::new() }
}

/// Most messages one run asks the conversation id of (#795): the messages listed before it was
/// asked for, a thousand a run, newest first, until every one has it or was asked once.
pub const MAX_THREAD_ASKS: usize = 1_000;

/// What a server said of the conversation of a message it was asked about ([`MAX_THREAD_ASKS`]):
/// the message's server id and the conversation id, empty when the server gave none (or no
/// longer has the message). `refused` when the server refused to say ([`MAX_THREAD_TRIES`]). A
/// message the server didn't answer for isn't here, and is asked again in a later run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThreadOf {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread: String,
    #[serde(skip_serializing_if = "is_false")]
    pub refused: bool,
}

impl ThreadOf {
    /// What a server refused to say of the message `id`.
    pub fn refused(id: &str) -> ThreadOf {
        ThreadOf { id: id.to_string(), thread: String::new(), refused: true }
    }
}

/// The server's copy of a message that is **being moved** out of a Microsoft account
/// ([`Message::moving_out`], R3 spec S19): the account and the copy's server id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MovingOut {
    pub account: Id,
    pub id: String,
    /// The server id of the folder the copy is in there, when it is known: where the message goes
    /// back to when its move into another mailbox of the same owner's is refused (#70).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// The server said the copy left that folder (deleted, or moved on by another program) and
    /// hasn't shown it in another since: there may be nothing there to go back to (#451).
    #[serde(skip_serializing_if = "is_false")]
    pub left: bool,
    /// `left` was said by a listing of that folder afresh that hasn't ended (#30): the copy may
    /// still be there, in a page to come, and counts as one that may hold the bytes the message
    /// came as until the listing ends without it.
    #[serde(skip_serializing_if = "is_false")]
    pub listing: bool,
}

/// Whether two names are the same category's. Outlook's categories are matched by name and a
/// mailbox can't have two whose names differ in case only, so neither can a message: names are
/// compared without regard to case (in every script, not ASCII only).
pub fn same_category(a: &str, b: &str) -> bool {
    a == b || a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// A message's categories after a run, when they were changed here and may have been changed on
/// the server too (R3 spec S17, ADR 0003 as amended): what was added and what was taken away
/// here since the last synced list (`synced`) is applied to the server's list as it is now, so
/// that a category the server added meanwhile stays.
///
/// - Names are matched as [`same_category`] does; a name given twice counts once.
/// - The server's order is kept, with what was added here after it in the order it has here.
/// - A category on the server's list is written as the server writes it; one added here as it
///   is written here.
/// - Added on both sides: there once. Taken away on both sides: gone.
pub fn merge_categories(synced: &[String], here: &[String], server: &[String]) -> Vec<String> {
    let has = |list: &[String], name: &str| list.iter().any(|c| same_category(c, name));
    let mut merged: Vec<String> = Vec::new();
    for name in server {
        let taken_away = has(synced, name) && !has(here, name);
        if !taken_away && !has(&merged, name) {
            merged.push(name.clone());
        }
    }
    for name in here {
        let added = !has(synced, name);
        if added && !has(&merged, name) {
            merged.push(name.clone());
        }
    }
    merged
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

/// Messages of a Microsoft account to fetch whole (their MIME), by server id, in this order
/// (R3 spec S12).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphFetchRequest {
    pub account: Id,
    pub email: String,
    /// `email` is a shared mailbox that the owner of the token may open: its requests go to
    /// `/users/{email}/…` rather than `/me/…` ([`crate::Account::shared_of`]).
    #[serde(skip_serializing_if = "is_false")]
    pub shared: bool,
    pub ids: Vec<String>,
    /// Nobody is waiting: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// What fetching brought: each message's MIME as it came, and what the server no longer has.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphFetchResult {
    pub account: Id,
    pub email: String,
    /// For a JMAP account, the address of the server's session resource the messages were fetched
    /// from (their request's): they are taken in only while the account still names that server.
    /// Empty for a Microsoft account's. It may carry the account's address, so it is never logged.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fetched: Vec<GraphFetched>,
    /// Server ids the server answered "not found" for (deleted or moved meanwhile; the next run
    /// says which).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Server ids the server wouldn't give whole although it has them (a `400`, say): the
    /// background passes them over for the rest of the session, so that one message can't hold
    /// up the rest. Opening one still asks for it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server could not be reached: whatever wasn't fetched waits until it can.
    #[serde(skip_serializing_if = "is_false")]
    pub offline: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
}

/// One message fetched whole.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphFetched {
    pub id: String,
    pub mime: Vec<u8>,
}

/// A value from a mailbox file, or `None` when what the file holds in its place isn't one (a
/// file from elsewhere, or from a later version): the mailbox still opens.
pub(crate) fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    // (Held as JSON first, then read from it as the file is, every list element in it charged to
    // the budget of the file being read: `load_limit`.)
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(crate::load_limit::from_value(&value).ok())
}

/// [`lenient`] for a value that has a "nothing was said" of its own: that, when what the file
/// holds in its place isn't one.
pub(crate) fn lenient_or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    lenient(deserializer).map(Option::unwrap_or_default)
}

/// A list from a mailbox file without the entries that aren't what they should be.
pub(crate) fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let serde_json::Value::Array(entries) = serde_json::Value::deserialize(deserializer)? else { return Ok(Vec::new()) };
    Ok(entries.iter().filter_map(|entry| crate::load_limit::from_value(entry).ok()).collect())
}

/// What changed in one server folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderDelta {
    /// The local folder, or `None` for a server folder the mailbox doesn't have yet.
    pub id: Option<Id>,
    pub remote: String,
    /// What the server says the folder is for (`\Sent`, `\Trash`…), [`FolderKind::Custom`] otherwise.
    pub kind: FolderKind,
    pub uid_validity: u32,
    pub uid_seen: u32,
    /// How far down the folder is read now ([`FolderState::uid_from`]), said by the last piece
    /// of it; 0 says nothing.
    pub uid_from: u32,
    /// Newly downloaded messages (`uid` set; `folder` and `account` are filled in when applied).
    pub new: Vec<Message>,
    /// Current flags of messages the mailbox already had.
    pub flags: Vec<RemoteFlags>,
    /// UIDs the mailbox has that the server no longer does.
    pub removed: Vec<u32>,
    /// This piece ends the folder's reading and everything that was wanted of it came (#504):
    /// the folder has been read whole once ([`crate::Folder::read_whole`]).
    #[serde(skip_serializing_if = "is_false")]
    pub complete: bool,
    /// With the piece that ends a folder's reading whole: what the server's STATUS said of it
    /// before it was read ([`crate::Folder::imap_status`], #276).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::ImapStatus>,
    /// Messages of [`FolderState::again`] downloaded again (`uid` set): only their subject,
    /// names and text are taken from them (#650).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refetched: Vec<Message>,
    /// UIDs of [`FolderState::again`] that are settled without a new copy (the message is too
    /// large to download), so that they are not asked for at every run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refetch_done: Vec<u32>,
    /// Of [`FolderDelta::refetch_done`], those too large to download (#30): what waits for the
    /// bytes they came as can't have them here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refetch_too_large: Vec<u32>,
    /// Of [`FolderDelta::refetch_done`], those the server refused (#30): asked for again by a run
    /// after, so many times.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refetch_refused: Vec<u32>,
    /// For a Gmail account's Inbox: which of Gmail's categories each message of it is in, as
    /// Gmail's searches said this run (#911), and Gmail's id of it. Said once, with the piece that
    /// ends the folder's reading; empty from every other server and folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gmail: Vec<GmailSorted>,
    /// For a folder of a Gmail account: Gmail's labels of the messages the mailbox already had
    /// (`X-GM-LABELS`, #1153), as the server listed them with their flags; a new message comes
    /// with its own in [`Message::gmail_labels`]. Empty from every other server.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gmail_labels: Vec<GmailLabels>,
}

/// Gmail's labels of one message the mailbox already had ([`FolderDelta::gmail_labels`], #1153).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GmailLabels {
    pub uid: u32,
    /// As the server wrote them; cleaned when they are taken in ([`crate::gmail::tidy_labels`]).
    pub labels: Vec<String>,
}

/// What Gmail said of one message of a Gmail account's Inbox ([`FolderDelta::gmail`], #911).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailSorted {
    pub uid: u32,
    /// The category Gmail put it in.
    pub category: crate::GmailCategory,
    /// Gmail's own id of it (`X-GM-MSGID`); 0 when the server gave none.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub id: u64,
}

/// A folder of mail the server has, as its list names it ([`SyncResult::listed`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListedFolder {
    /// Its path on the server.
    pub remote: String,
    pub kind: FolderKind,
    /// The path of the folder it lies in; empty for one at the top.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent: String,
    /// The last part of its name, as people read it.
    pub name: String,
}

/// A flag change the server took on ([`RemoteOp::Flags`]): the flags that were set or cleared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlagsPushed {
    pub folder: String,
    pub uid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flagged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered: Option<bool>,
}

/// A move that is finished ([`RemoteOp::Move`]): the server carried it out, or the message was
/// no longer where it was to be moved from, which leaves nothing to do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Moved {
    pub message: Id,
    pub from: String,
    pub uid: u32,
    pub to_folder: Id,
    /// The message's UID in its new mailbox, when the server said (`COPYUID`); 0 otherwise.
    #[serde(skip_serializing_if = "is_zero")]
    pub new_uid: u32,
    /// `UIDVALIDITY` of the mailbox `new_uid` belongs to; 0 when the server didn't say.
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_validity: u32,
}

/// A deletion that is finished ([`RemoteOp::Delete`]): the message is gone from the server, or
/// marked as deleted there, or was gone already.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Deleted {
    pub from: String,
    pub uid: u32,
}

/// A message the server took ([`SyncRequest::places`]), in the mailbox of local folder
/// `to_folder`: its UID there and that mailbox's `UIDVALIDITY` when the server said them
/// (`APPENDUID`), 0 when it didn't, and the flags it was given (the message may have changed
/// here since).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Placed {
    pub message: Id,
    pub to_folder: Id,
    pub uid: u32,
    pub uid_validity: u32,
    pub flags: RemoteFlags,
    /// The message put there had the bytes it came as whole (#30, [`Message::original`]).
    #[serde(skip_serializing_if = "is_false")]
    pub original: bool,
}

/// A local folder that has a mailbox on the server now ([`RemoteOp::Move`] with `create`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Created {
    pub folder: Id,
    /// The new mailbox's path on the server.
    pub remote: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What a synchronisation brought back, or a piece of it.
///
/// A host that wants to show mail as it arrives is handed a run's answer in pieces, in the order
/// things were done (`partial` on all but the last). Every piece is whole in itself: taking the
/// pieces in one after the other, as far as they come, leaves the mailbox as far along as the
/// run got, and what a later piece would have brought is asked for again by the next run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncResult {
    pub account: Id,
    /// The address of the account the run was for: an answer is not taken in by another account
    /// that happens to have the id (in a mailbox opened while the run was under way).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub folders: Vec<FolderDelta>,
    /// Every folder of mail the server has, parents and children, said once near the start of a
    /// run: the tree is there to see before its mail is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub listed: Vec<ListedFolder>,
    /// Ids of Outbox messages that were sent.
    pub sent: Vec<Id>,
    /// Those of `sent` that have a copy in the server's mailbox for sent mail, or will have: put
    /// there in this run, or filed by a server that keeps what it sends. The copy comes down with
    /// that mailbox, now or later.
    ///
    /// In an answer that comes in pieces this is said with `sent`, before the copy is put there,
    /// so that a mailbox which only hears that much knows the copy for what it is when it shows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filed: Vec<Id>,
    /// Messages an earlier piece said were `filed` whose copy didn't get to the server after all
    /// (it wouldn't take it, or has no mailbox for sent mail): the copy here is the one there is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unfiled: Vec<Id>,
    /// Outbox messages whose send was cut off after they were handed to the outgoing server
    /// (the connection broke, no answer came): whether they went is not known, so they are held
    /// for the owner, never sent again by itself (AU3-26).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unsure: Vec<Id>,
    /// What became of [`SyncRequest::ops`]. An op that isn't here failed (see `errors`) or wasn't
    /// tried, and is still to be pushed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flagged: Vec<FlagsPushed>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<Moved>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<Deleted>,
    /// Mailboxes made on the server, whether or not the move they were made for went through.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<Created>,
    /// What became of [`SyncRequest::places`]: the messages the server took. One that isn't here
    /// nor in `unplaced` wasn't tried, or its answer was lost, and is put there again next time.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<Placed>,
    /// Messages the server wouldn't take (see `errors`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unplaced: Vec<Id>,
    /// What became of [`SyncRequest::gmail_moves`]: the moves Gmail took (#911). One that isn't
    /// here failed (see `errors`) or wasn't tried, and is asked for again next time.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gmail_moved: Vec<GmailMove>,
    /// The moves Gmail refused on their own, one message at a time (#911): asked for again next
    /// time, until Gmail has refused it [`crate::gmail::MAX_GMAIL_REFUSALS`] times.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gmail_refused: Vec<GmailMove>,
    /// How many of Gmail's messages had their moves not sent this run (the run's requests to
    /// Gmail's API ran out); they wait for the next. Counted by Gmail's id: two copies here of
    /// one message are one.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub gmail_unsent: usize,
    /// Problems worth telling the user (one per failed step); empty when all went well. In a
    /// piece of an answer: the problems since the piece before. In English: what the CLI, MCP
    /// and the log read.
    pub errors: Vec<String>,
    /// The same problems as texts the window says in its language, for those a host said that
    /// way ([`SyncResult::problem`]); [`SyncResult::problems`] pairs them with `errors`. Never
    /// sent anywhere: an answer that crosses as JSON has the English only.
    #[serde(skip)]
    pub said: Vec<mailcraft_i18n::Text>,
    /// This is a piece of an answer, and more follows. The last piece doesn't have it.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// One short line on how far the run has got (`Inbox: 150 of 500`), for a status bar. Empty
    /// in the last piece, whose summary says how it went.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub progress: String,
    /// The same as numbers (what `progress` is made of): which step, which folder, how many of
    /// how many. None in the last piece, and from a host that only says the line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<SyncStep>,
    /// The run was called off before it had done all there was to do (in the last piece).
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// How long this piece's part of the run took, and how many requests it made (#276).
    #[serde(skip_serializing_if = "Timings::is_empty")]
    pub timings: Timings,
}

impl SyncResult {
    /// Tell a problem: its English among `errors`, and the text it is made of for the window.
    pub fn problem(&mut self, said: mailcraft_i18n::Text) {
        self.errors.push(said.to_string());
        self.said.push(said);
    }

    /// Each of `errors` as a text to say in the interface's language: the one a host told it
    /// with ([`SyncResult::problem`]) when its English is that line still, else the line as it
    /// is. (A line changed after it was told, a password blanked out of it say, is said as it
    /// now stands.)
    pub fn problems(&self) -> Vec<mailcraft_i18n::Text> {
        let mut unused: Vec<Option<&mailcraft_i18n::Text>> = self.said.iter().map(Some).collect();
        self.errors
            .iter()
            .map(|line| {
                let found = unused.iter_mut().find(|t| t.is_some_and(|t| t.to_string() == *line)).and_then(Option::take);
                found.cloned().unwrap_or_else(|| mailcraft_i18n::Text::verbatim(line.clone()))
            })
            .collect()
    }

    pub fn new_count(&self) -> usize {
        self.folders.iter().map(|f| f.new.len()).sum()
    }

    /// How many changes made here the server took on.
    pub fn pushed_count(&self) -> usize {
        self.flagged.len()
            + self.moved.len()
            + self.deleted.len()
            + self.created.len()
            + self.placed.len()
            + self.unplaced.len()
            + self.gmail_moved.len()
    }

    /// Whether there is anything in it for a mailbox to take in. A piece with only a line of
    /// progress, or only problems to tell, has nothing.
    pub fn has_news(&self) -> bool {
        !(self.folders.is_empty()
            && self.listed.is_empty()
            && self.sent.is_empty()
            && self.filed.is_empty()
            && self.unfiled.is_empty()
            && self.unsure.is_empty()
            && self.gmail_refused.is_empty()
            && self.gmail_unsent == 0
            && self.pushed_count() == 0)
    }

    /// Add the piece that came after this one, so that the two are one answer: taking it in is
    /// the same as taking in the one and then the other. What a mailbox's pieces say about it
    /// comes together under its name, its new messages in the order they came.
    pub fn absorb(&mut self, later: SyncResult) {
        // Every field is taken in below: a field added later fails to compile here until it is.
        let SyncResult {
            account: _,
            email: _,
            folders: _,
            listed: _,
            sent: _,
            filed: _,
            unfiled: _,
            unsure: _,
            flagged: _,
            moved: _,
            deleted: _,
            created: _,
            placed: _,
            unplaced: _,
            gmail_moved: _,
            gmail_refused: _,
            gmail_unsent: _,
            errors: _,
            said: _,
            partial: _,
            progress: _,
            step: _,
            stopped: _,
            timings: _,
        } = &later;
        (self.account, self.email) = (later.account, later.email);
        (self.partial, self.progress, self.step, self.stopped) = (later.partial, later.progress, later.step, later.stopped);
        self.timings.absorb(&later.timings);
        if !later.listed.is_empty() {
            self.listed = later.listed;
        }
        self.sent.extend(later.sent);
        // A copy that didn't get there after all is one that was never said to.
        self.filed.retain(|id| !later.unfiled.contains(id));
        self.filed.extend(later.filed);
        self.unfiled.extend(later.unfiled);
        self.unsure.extend(later.unsure);
        self.flagged.extend(later.flagged);
        self.moved.extend(later.moved);
        self.deleted.extend(later.deleted);
        self.created.extend(later.created);
        self.placed.extend(later.placed);
        self.unplaced.extend(later.unplaced);
        self.gmail_moved.extend(later.gmail_moved);
        self.gmail_refused.extend(later.gmail_refused);
        self.gmail_unsent = self.gmail_unsent.saturating_add(later.gmail_unsent);
        self.errors.extend(later.errors);
        self.said.extend(later.said);
        for delta in later.folders {
            match self.folders.iter_mut().find(|f| f.remote == delta.remote && f.uid_validity == delta.uid_validity) {
                Some(folder) => {
                    // Every field named, so that one added later can't be left out of this
                    // (#991: what was read again came with the last piece, and was lost).
                    let FolderDelta {
                        id,
                        // The same mailbox, as the earlier piece named it.
                        remote: _,
                        kind: _,
                        uid_validity: _,
                        uid_seen,
                        uid_from,
                        new,
                        flags,
                        removed,
                        complete,
                        status,
                        refetched,
                        refetch_done,
                        refetch_too_large,
                        refetch_refused,
                        gmail,
                        gmail_labels,
                    } = delta;
                    folder.id = folder.id.or(id);
                    // The mailbox is read as far as the piece that got furthest says.
                    folder.uid_seen = folder.uid_seen.max(uid_seen);
                    if uid_from != 0 {
                        folder.uid_from = uid_from;
                    }
                    folder.new.extend(new);
                    folder.flags.extend(flags);
                    folder.removed.extend(removed);
                    folder.refetched.extend(refetched);
                    folder.refetch_done.extend(refetch_done);
                    folder.refetch_too_large.extend(refetch_too_large);
                    folder.refetch_refused.extend(refetch_refused);
                    folder.gmail.extend(gmail);
                    folder.gmail_labels.extend(gmail_labels);
                    folder.complete |= complete;
                    if status.is_some() {
                        folder.status = status;
                    }
                }
                None => self.folders.push(delta),
            }
        }
    }

    /// `3 new messages, 1 sent`, `Up to date`, or the first error.
    ///
    /// Before the result is applied, "new" is everything that came down, including the server's
    /// copy of a message that is already here (one that was just sent, or moved on a server that
    /// doesn't say where it put it). What applying it answers is exact.
    pub fn summary(&self) -> String {
        summary_line(self.errors.first().map(String::as_str), self.new_count(), self.sent.len())
    }
}

/// `3 new messages, 1 sent`, `Up to date`, or the error.
pub fn summary_line(error: Option<&str>, new: usize, sent: usize) -> String {
    summary_line_in(mailcraft_i18n::Lang::English, error, new, sent)
}

/// [`summary_line`] in `lang` (an error is said as it came).
pub fn summary_line_in(lang: mailcraft_i18n::Lang, error: Option<&str>, new: usize, sent: usize) -> String {
    if let Some(e) = error {
        return e.to_string();
    }
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let template = match (new, sent) {
        (0, 0) => lang.t("Up to date"),
        (_, 0) => lang.tn(count(new), "{n} new message", "{n} new messages"),
        (0, _) => lang.tn(count(sent), "{sent} message sent", "{sent} messages sent"),
        (_, _) => lang.tn(count(new), "{n} new message, {sent} sent", "{n} new messages, {sent} sent"),
    };
    mailcraft_i18n::fmt(template, &[("n", &new.to_string()), ("sent", &sent.to_string())])
}

// ---- Microsoft Graph (R3) ----
//
// A Microsoft account is kept in step through Graph alone (ADR 0002), and its folders and
// messages are known by their server ids (ADR 0003). These are its own request and answer,
// beside the IMAP ones above rather than bent into them: no UIDs, no paths.

/// Most changes to folders one Graph run is asked to make; more go over several runs.
pub const MAX_FOLDER_CHANGES: usize = 500;
/// Most folders a listing holds; a mailbox with more is listed in part (and nothing is removed
/// for being missing from it).
pub const MAX_LISTED_FOLDERS: usize = 10_000;

/// A change made here to a folder of a Microsoft account, for its server to take on: worked out
/// from the difference between the folder and what the server last said of it
/// ([`crate::ServerFolder`]), or waiting in [`crate::Mailbox::queued`]. Folders are named by
/// their server ids; a parent that is empty is the top of the mailbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderChange {
    /// The folder `folder` only exists here: the server makes it, called `name`, under the
    /// folder `parent`; or, when `parent_folder` names a folder of this mailbox that is made in
    /// the same run, under that one once it is.
    #[serde(rename_all = "camelCase")]
    Create {
        folder: Id,
        name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_folder: Option<Id>,
    },
    /// The folder `folder`, which the server calls `id`, is called `name` now.
    Rename { folder: Id, id: String, name: String },
    /// The folder `folder`, which the server calls `id`, lies in `parent` now. (Delete Folder is
    /// one of these: into Deleted Items, as Outlook does.)
    Move {
        folder: Id,
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        parent: String,
    },
    /// The folder the server calls `id` was deleted for good here. `shown` is what the owner
    /// had of it when it was (AU3-H7): the host sends the delete only when the server holds
    /// nothing in it or under it beyond that ([`ChangeOutcome::NotShown`] otherwise).
    Delete {
        id: String,
        #[serde(default, skip_serializing_if = "Shown::is_empty")]
        shown: Shown,
    },
}

/// What the owner had of a folder deleted for good here, and of everything under it, when it was
/// deleted (AU3-H7): the server's ids of those folders and of their messages, each kept as its
/// [`Shown::key`], and the folder's name for the line that says it wasn't deleted. Before the
/// server is asked to delete the folder, which takes with it whatever the server holds there, the
/// host reads what it holds, and anything not here (mail that arrived since, a folder made
/// elsewhere) keeps the folder. One from a file of before has nothing: the folder goes only when
/// the server holds nothing in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Shown {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<u64>,
}

/// Most folders, and most messages, a [`Shown`] keeps, and that the host reads of a folder before
/// deleting it: past either, the folder isn't deleted on the server.
pub const MAX_SHOWN_FOLDERS: usize = MAX_LISTED_FOLDERS;
pub const MAX_SHOWN_MESSAGES: usize = 500_000;
/// Longest name a [`Shown`] keeps, in characters.
const MAX_SHOWN_NAME: usize = 80;

impl Shown {
    /// What `name` (the folder's), `folders` and `messages` (server ids) come to, held to the
    /// caps (what is past them is left out, and the server then holds what isn't shown).
    pub fn of<'a>(name: &str, folders: impl IntoIterator<Item = &'a str>, messages: impl IntoIterator<Item = &'a str>) -> Self {
        fn keys<'a>(ids: impl Iterator<Item = &'a str>, most: usize) -> Vec<u64> {
            let mut out: Vec<u64> = ids.take(most).map(Shown::key).collect();
            out.sort_unstable();
            out.dedup();
            out
        }
        Shown {
            name: name.chars().filter(|c| !c.is_control()).take(MAX_SHOWN_NAME).collect(),
            folders: keys(folders.into_iter(), MAX_SHOWN_FOLDERS),
            messages: keys(messages.into_iter(), MAX_SHOWN_MESSAGES),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.folders.is_empty() && self.messages.is_empty()
    }

    /// A server id as it is kept: FNV-1a, 64 bits, of its bytes. (A name for telling ids apart,
    /// not a secret: the ids are the mailbox's own.)
    pub fn key(id: &str) -> u64 {
        id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
    }

    /// The keys of the folders and of the messages as sets, for the host's check (held to the
    /// caps, whatever a file said).
    pub fn sets(&self) -> (std::collections::HashSet<u64>, std::collections::HashSet<u64>) {
        let set = |keys: &[u64], most: usize| keys.iter().take(most).copied().collect();
        (set(&self.folders, MAX_SHOWN_FOLDERS), set(&self.messages, MAX_SHOWN_MESSAGES))
    }
}

impl FolderChange {
    /// The folder of this mailbox the change is about; `None` for one that is gone here.
    pub fn folder(&self) -> Option<Id> {
        match self {
            FolderChange::Create { folder, .. } | FolderChange::Rename { folder, .. } | FolderChange::Move { folder, .. } => Some(*folder),
            FolderChange::Delete { .. } => None,
        }
    }
}

/// What a Graph run for a Microsoft account is asked to do: the folder changes made here go up,
/// then every folder of the mailbox is listed. The access token is handed to the host's Graph
/// code beside this, never in it: nothing here is a secret, and it may be logged by count.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphMailRequest {
    pub account: Id,
    /// The account's address: the mailbox whose requests are counted together (Microsoft allows
    /// a few at once per mailbox), and what the answer is checked against.
    pub email: String,
    /// `email` is a shared mailbox that the owner of the token may open (Full Access): every
    /// request goes to `/users/{email}/…` rather than `/me/…`, with the owner's token, which
    /// covers `Mail.ReadWrite.Shared` ([`crate::Account::shared_of`]).
    #[serde(skip_serializing_if = "is_false")]
    pub shared: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FolderChange>,
    /// Nobody is waiting for this run: it gets the share of requests background work has.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
    /// Where each folder's mail goes on from (the folder's server id and its link, and whether
    /// the link is a delta link); a folder that isn't here is read from its start (R3 spec S10).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<GraphLink>,
    /// The changes of messages made here, after the folder changes and before the listing
    /// (R3 spec S16).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageChange>,
    /// The drafts saved here that the server hasn't all of, after the changes of messages
    /// (R3.4.1, S23–S25).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<DraftUp>,
    /// What waits in the Outbox, sent after the drafts (R3.4.2).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendUp>,
    /// Messages to make in its folders, moved or copied here from another mailbox of the same
    /// owner's (#70).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<PlaceUp>,
    /// The Inbox pass (#276): the run is only the delta of this folder (the Inbox, from its
    /// stored delta link in `links`) and nothing else: no folder changes, no listing,
    /// nothing sent. Every piece of it is partial; the host ends it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbox_pass: Option<GraphFolder>,
    /// A full sweep (#276): every folder's delta is read, none is passed over for having the
    /// same counts as before. The first run after the mailbox is opened, then one an hour.
    #[serde(skip_serializing_if = "is_false")]
    pub full: bool,
    /// Messages listed before their conversation ids were asked for, by server id: the server is
    /// asked for those ids after the rest of the run, at most [`MAX_THREAD_ASKS`] (#795).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<String>,
}

/// Most changes of messages one Graph run is asked to make; more go over several runs.
pub const MAX_MESSAGE_CHANGES: usize = 10_000;

/// A change made here to a message of a Microsoft account, for its server to take on (R3 spec
/// S16): worked out from the difference between the message and its last synced state
/// ([`ServerMessage`]), or waiting in [`crate::Mailbox::queued`]. Messages are named by their
/// server ids, folders by theirs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageChange {
    /// Properties of the message `message` (the server's `id`) were changed here: only those are
    /// named, and they go as one change.
    #[serde(rename_all = "camelCase")]
    Update {
        message: Id,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flag: Option<FlagUp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        categories: Option<CategoriesUp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        importance: Option<Importance>,
    },
    /// The message lies in another folder here: the folder the server calls `to`, or, when
    /// `to_folder` names a folder of this mailbox that is made in the same run, that one once it is.
    #[serde(rename_all = "camelCase")]
    Move {
        message: Id,
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_folder: Option<Id>,
    },
    /// The message was copied here into another folder of the account (`copy` is the copy here):
    /// the server copies it, after giving the source MailCraft's invisible marker of the value
    /// `marker`, by which a copy whose answer was lost is found (S21).
    #[serde(rename_all = "camelCase")]
    Copy {
        copy: Id,
        id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_folder: Option<Id>,
        marker: String,
    },
    /// The message was deleted for good here: the server deletes it (Graph's delete, which keeps
    /// it recoverable for the retention period; never past it, S19).
    Delete { id: String },
    /// The message `message` was moved out of the account, is fetched whole and saved here: the
    /// server's copy goes (S19).
    #[serde(rename_all = "camelCase")]
    MovedOut { message: Id, id: String },
}

impl MessageChange {
    /// The server id of the message it is about.
    pub fn id(&self) -> &str {
        match self {
            MessageChange::Update { id, .. }
            | MessageChange::Move { id, .. }
            | MessageChange::Copy { id, .. }
            | MessageChange::Delete { id }
            | MessageChange::MovedOut { id, .. } => id,
        }
    }
}

/// A follow-up flag as it goes up: its status and its dates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FlagUp {
    pub status: FlagStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<Timestamp>,
}

/// Categories changed here: the last synced list and the list here. The server's list as it is
/// when the change is made is read first, and what was added and taken away here is applied to
/// it ([`merge_categories`], S17).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoriesUp {
    pub synced: Vec<String>,
    pub here: Vec<String>,
}

/// What the server made of a [`MessageChange`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageOutcome {
    /// Done. `categories`: the list the message has on the server now (for a change of them);
    /// `flag`: the flag as it went; `folder`: the server id of the folder a move or a copy put
    /// it in; `id`: the server id of a copy it made.
    #[serde(rename_all = "camelCase")]
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        categories: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flag: Option<FlagUp>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        folder: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
    },
    /// The server no longer has the message (404).
    Gone,
    /// The server won't (403 not allowed, 400 not a change it makes): `status` says which.
    Refused { status: u16 },
}

/// A change of a message and what became of it. One that isn't answered (the server was busy,
/// the connection went) isn't here: it is worked out again for the next run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageChangeDone {
    pub change: MessageChange,
    pub outcome: MessageOutcome,
    /// For a delete for good the server refused: the message as the server has it, read after
    /// the refusal, by which it is put back here (S20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub there: Option<Box<GraphMessage>>,
}

/// Where a folder's message delta goes on from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphLink {
    pub folder: String,
    pub link: String,
    /// It is a delta link (the folder's round has ended once), not the next page of a round.
    #[serde(skip_serializing_if = "is_false")]
    pub caught_up: bool,
    /// The folder's counts when the round this delta link ended began
    /// ([`crate::ServerFolder::counts`]): the same again in this run's listing, the folder may
    /// be passed over (#276).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<FolderCounts>,
}

/// A message as a folder's delta gives it: what the message list needs (R3 spec S9).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphMessage {
    /// The server's state of it: its immutable id, the folder, read, flag, categories, importance.
    pub server: ServerMessage,
    /// `Message-ID` without angle brackets.
    pub message_id: String,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,
    /// When it was sent, and when it arrived.
    pub sent: Timestamp,
    pub received: Timestamp,
    pub preview: String,
    #[serde(skip_serializing_if = "is_false")]
    pub has_attachments: bool,
    /// It is a draft (Graph's `isDraft`); `None` when the server didn't say, which changes nothing
    /// here (#611).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
}

/// One page of a folder's message delta, taken in whole with the link that goes on from it, so
/// that a mailbox file never holds the one without the other (R3 spec S10).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MessagePage {
    /// The folder's server id.
    pub folder: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<GraphMessage>,
    /// The server ids of the messages that left the folder (`@removed`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Messages whose read state alone changed: Exchange Online says one by its id and `isRead`,
    /// nothing else (#148).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<ReadState>,
    /// Where the folder goes on from after this page.
    pub link: String,
    /// `link` is a delta link: this page ended the round.
    #[serde(skip_serializing_if = "is_false")]
    pub caught_up: bool,
    /// The server no longer took the folder's link (`410 Gone`, `syncStateNotFound`): this page
    /// begins a fresh listing, and what the complete listing doesn't have is removed when it ends
    /// (R3 spec S11).
    #[serde(skip_serializing_if = "is_false")]
    pub afresh: bool,
    /// For a page that ends a round begun after this run's listing: the folder's counts in that
    /// listing, kept with the link ([`crate::ServerFolder::counts`], #276). Otherwise `None`,
    /// and the counts kept for the folder are forgotten with its old link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<FolderCounts>,
}

/// A message's read state as a folder's delta says it alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReadState {
    /// Its server id.
    pub id: String,
    pub read: bool,
}

/// One folder of a Microsoft account's mailbox, as the server lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphFolder {
    /// The server's id.
    pub id: String,
    /// The server id of the folder it lies in; empty for one at the top of the mailbox.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent: String,
    /// Its name, whole: it may hold `/` or `\`, and is never split into a path.
    pub name: String,
    /// Which special folder it is, when the server says it is one of them.
    pub kind: FolderKind,
    /// How many messages it has, and how many of them are unread, as the server counts them:
    /// for progress (R3 spec S15).
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub total: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub unread: u32,
    /// The server gave both counts (#276). A count it didn't give reads as 0 above, so a folder
    /// whose counts aren't known is never passed over as unchanged.
    #[serde(skip_serializing_if = "is_false")]
    pub counted: bool,
    /// It is one of the server's own folders (a well-known one, such as Conversation History),
    /// which can't be deleted or moved (#756).
    #[serde(skip_serializing_if = "is_false")]
    pub well_known: bool,
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Every folder the server has, parents before what lies in them. Hidden folders and search
/// folders are not among them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderListing {
    pub folders: Vec<GraphFolder>,
    /// The listing went to its end: a folder that isn't in it is one the server doesn't have.
    /// A listing that was cut short (a page that didn't come, a limit reached) only adds and
    /// updates; nothing is removed for being missing from it (R3 spec S8).
    pub complete: bool,
}

/// What the server made of a [`FolderChange`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeOutcome {
    /// Done: the folder as the server has it now (for a deletion, empty).
    Done { folder: GraphFolder },
    /// The server no longer has the folder: there is nothing to do (the listing removes it).
    Gone,
    /// The server won't (403, 400, a name that is taken there): the folder is put back to what
    /// the server has, and this says why, in plain words.
    Refused { why: String },
    /// A deletion that wasn't sent (AU3-H7): the server holds `items` folders and messages in the
    /// folder or under it that weren't there when the owner deleted it (mail that arrived since,
    /// a folder made elsewhere). The folder comes back as with a refusal.
    NotShown { items: u32 },
    /// The server won't because the folder is one of its own (#1069): a default folder it keeps,
    /// such as RSS Feeds, that no well-known name it answers to tells apart beforehand (Graph's
    /// list of well-known folder names, learn.microsoft.com/graph/api/resources/mailfolder, has
    /// no `rssfeeds`). It is put back as with a refusal, and marked as the server's own
    /// ([`crate::ServerFolder::well_known`]) so it isn't offered for renaming, moving or deleting
    /// again; this says why, in plain words.
    OwnFolder { why: String },
}

/// The codes a Microsoft server answers a move or deletion of one of its own folders with (EWS's
/// `ResponseCode` list, learn.microsoft.com/exchange/client-developer/web-service-reference/responsecode:
/// `ErrorDeleteDistinguishedFolder`, `ErrorMoveDistinguishedFolder`), read in an EWS answer and
/// in Graph's `error.code` alike, Graph's codes being Exchange's (#1069).
pub const OWN_FOLDER_CODES: [&str; 2] = ["ErrorDeleteDistinguishedFolder", "ErrorMoveDistinguishedFolder"];

/// A change and what became of it. A change that isn't answered (the server was busy, the
/// connection went) is not here: it is worked out again for the next run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChangeDone {
    pub change: FolderChange,
    pub outcome: ChangeOutcome,
}

/// What a Graph run for a Microsoft account brought back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphMailResult {
    pub account: Id,
    /// The address of the account the run was for: an answer is not taken in by another account
    /// that happens to have the id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<FolderChangeDone>,
    /// What became of the changes of messages sent (R3.3).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageChangeDone>,
    /// What became of the drafts taken up (R3.4.1).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<DraftDone>,
    /// What became of the messages sent (R3.4.2).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sent: Vec<SendDone>,
    /// What became of the messages put into it from another mailbox of the same owner's (#70).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<PlaceDone>,
    /// Every folder of the mailbox, when it could be listed at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listing: Option<FolderListing>,
    /// Problems worth telling the owner; empty when all went well.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server said it was busy and to stay away this many seconds: all Graph work for the
    /// account waits that long (R3 spec S14).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
    /// Pages of message delta, in the order they came: each is taken in by itself (R3 spec S10).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<MessagePage>,
    /// More of this run comes after this piece: a run arrives in pieces (the folders, then a page
    /// at a time), each taken in as it comes.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// Every folder's delta reached its end in this run, and the run's listing of folders went to
    /// its end: a message the server said left its folder and that turned up in no other is
    /// removed now (R3 spec S9, S11). Never after a listing cut short (#305).
    #[serde(skip_serializing_if = "is_false")]
    pub whole: bool,
    /// How far the run has got, for the status bar (R3 spec S15).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<GraphProgress>,
    /// The owner stopped the run (Cancel) after the page in hand: the next one goes on from there.
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// For a shared mailbox: why the server won't open it for the owner of the token, by Graph's
    /// own code ([`SharedRefusal`]). A host doesn't ask again by itself until the owner does.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub refused: Option<SharedRefusal>,
    /// How long this piece's part of the run took, and how many requests it made (#276).
    #[serde(skip_serializing_if = "Timings::is_empty")]
    pub timings: Timings,
    /// The piece is of the Inbox pass ([`GraphMailRequest::inbox_pass`], #276): it is never the
    /// end of the account's part of the run, and never says the mailbox was read whole.
    #[serde(skip_serializing_if = "is_false")]
    pub inbox_pass: bool,
    /// What the server said of the conversations of the messages the run asked about
    /// ([`GraphMailRequest::threads`], #795).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadOf>,
}

/// Why Microsoft won't open a shared mailbox for the owner of the token (#70), told apart by
/// Graph's own error code rather than by the status alone: a 403 for a quota or a missing
/// consent is none of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharedRefusal {
    /// `ErrorAccessDenied`: the owner has no Full Access to it (never had, or it was taken away).
    NoAccess,
    /// `ErrorInvalidUser`: there is no mailbox at the address.
    NoMailbox,
    /// `MailboxNotEnabledForRESTAPI`: the mailbox can't be reached through Microsoft Graph.
    NotThroughGraph,
}

impl SharedRefusal {
    /// What it is, in words that name no mailbox (they may reach a log).
    pub fn said(self) -> &'static str {
        match self {
            SharedRefusal::NoAccess => "Microsoft won't open this shared mailbox for you: you need Full Access to it",
            SharedRefusal::NoMailbox => "Microsoft has no mailbox at this address",
            SharedRefusal::NotThroughGraph => "this mailbox can't be reached through Microsoft Graph (MailboxNotEnabledForRESTAPI)",
        }
    }
}

impl GraphMailResult {
    /// Take in the next piece of the same run, for a caller that takes a run in at once: the
    /// pages in the order they came, and what the last piece says of the end.
    pub fn absorb(&mut self, piece: GraphMailResult) {
        self.changes.extend(piece.changes);
        self.messages.extend(piece.messages);
        self.drafts.extend(piece.drafts);
        self.sent.extend(piece.sent);
        if piece.listing.is_some() {
            self.listing = piece.listing;
        }
        self.threads.extend(piece.threads);
        self.errors.extend(piece.errors);
        self.busy_for = piece.busy_for.or(self.busy_for);
        self.pages.extend(piece.pages);
        (self.partial, self.whole, self.stopped) = (piece.partial, piece.whole, piece.stopped);
        self.refused = piece.refused.or(self.refused);
        self.timings.absorb(&piece.timings);
        if piece.progress.is_some() {
            self.progress = piece.progress;
        }
    }
}

/// How far a run has got: the folder being read (its server id), how many of its messages came in
/// this run and how many it has, and which folder of how many it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphProgress {
    pub folder: String,
    pub read: u32,
    pub total: u32,
    pub number: u32,
    pub folders: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R3.2: what a folder's delta brings is kept with the message and the folder, and a file
    /// from before reads as whole messages, no link, and the default download limit.
    #[test]
    fn delta_state_reads_from_older_files_and_round_trips() {
        let old: Message = serde_json::from_str(r#"{"id": 4, "subject": "Hi", "server": {"id": "AAkA", "folder": "F1"}}"#).unwrap();
        assert!(!old.unfetched && !old.server.as_ref().unwrap().gone);
        let account: Account = serde_json::from_str(r#"{"email": "ada@example.org", "downloadInFull": "everything"}"#).unwrap();
        assert_eq!((account.download_in_full, account.download_in_full.limit()), (crate::DownloadInFull::Everything, None));
        let account: Account = serde_json::from_str(r#"{"email": "ada@example.org", "downloadInFull": "upTo999Mb"}"#).unwrap();
        assert_eq!(account.download_in_full.limit(), Some(10 << 20), "unknown: the default");
        assert!(!serde_json::to_string(&account).unwrap().contains("downloadInFull"));
        let folder: crate::ServerFolder = serde_json::from_str(r#"{"id": "F1", "name": "Inbox", "delta": "https://x/d"}"#).unwrap();
        assert!(!folder.caught_up);

        let mut m = old.clone();
        m.unfetched = true;
        if let Some(server) = m.server.as_mut() {
            server.gone = true;
        }
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""gone":true"#) && json.contains(r#""unfetched":true"#), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), m);
        let page =
            MessagePage { folder: "F1".into(), removed: vec!["AAkB".into()], link: "https://x/d".into(), caught_up: true, ..Default::default() };
        let result = GraphMailResult { pages: vec![page], ..Default::default() };
        assert_eq!(serde_json::from_str::<GraphMailResult>(&serde_json::to_string(&result).unwrap()).unwrap(), result);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn merged(synced: &[&str], here: &[&str], server: &[&str]) -> Vec<String> {
        merge_categories(&names(synced), &names(here), &names(server))
    }

    /// R3.3 (S17): what was added and taken away here goes onto the server's list as it is now.
    #[test]
    fn categories_merge_what_changed_here_into_the_servers_list() {
        // Nothing changed anywhere.
        assert_eq!(merged(&["Red"], &["Red"], &["Red"]), ["Red"]);
        // Added here; the server added another meanwhile: both stay, the server's first.
        assert_eq!(merged(&["Red"], &["Red", "Blue"], &["Red", "Green"]), ["Red", "Green", "Blue"]);
        // Taken away here, and the server added one: the server's addition stays.
        assert_eq!(merged(&["Red", "Blue"], &["Blue"], &["Red", "Blue", "Green"]), ["Blue", "Green"]);
        // Taken away on both sides.
        assert_eq!(merged(&["Red", "Blue"], &["Blue"], &["Blue"]), ["Blue"]);
        // Added on both sides: once.
        assert_eq!(merged(&[], &["Blue"], &["Blue"]), ["Blue"]);
        // Taken away on the server and kept here untouched: the server's way.
        assert_eq!(merged(&["Red", "Blue"], &["Red", "Blue"], &["Blue"]), ["Blue"]);
        // Taken away here, added again on the server... it was on the synced list, so it goes.
        assert_eq!(merged(&["Red"], &[], &["Red"]), Vec::<String>::new());
        // The server's order is kept, additions follow in the order they have here.
        assert_eq!(merged(&["A"], &["C", "A", "B"], &["Z", "A", "Y"]), ["Z", "A", "Y", "C", "B"]);
    }

    #[test]
    fn categories_are_matched_by_name_whatever_the_case_and_counted_once() {
        assert!(same_category("Red", "red") && same_category("ÉTÉ", "été") && !same_category("Red", "Red "));
        // Spelled differently here and there: the server's spelling, once.
        assert_eq!(merged(&[], &["blue"], &["Blue"]), ["Blue"]);
        // Taken away here under another spelling.
        assert_eq!(merged(&["Red", "Blue"], &["BLUE"], &["red", "Blue"]), ["Blue"]);
        // Duplicates in any list count once.
        assert_eq!(merged(&["Red", "red"], &["Red", "Red", "Green", "green"], &["Red", "RED", "Blue"]), ["Red", "Blue", "Green"]);
        assert_eq!(merged(&[], &[], &[]), Vec::<String>::new());
    }

    /// R3.3: the queue of copies and deletes for good, and a message being moved out of the
    /// account, are kept in the mailbox file; a file from before opens as it was.
    #[test]
    fn changes_up_read_from_older_files_and_round_trip() {
        let old: Message = serde_json::from_str(r#"{"id": 4, "subject": "Hi", "server": {"id": "AAkA", "folder": "F1"}}"#).unwrap();
        assert!(old.moving_out.is_none() && !old.incomplete);
        // Something else in its place (a later version's): the message still opens.
        let odd: Message = serde_json::from_str(r#"{"id": 4, "movingOut": 7, "incomplete": true}"#).unwrap();
        assert!(odd.moving_out.is_none() && odd.incomplete);

        let mut m = old.clone();
        m.moving_out = Some(MovingOut { account: 2, id: "AAkA".into(), ..Default::default() });
        m.incomplete = true;
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""movingOut":{"account":2,"id":"AAkA"}"#) && json.contains(r#""incomplete":true"#), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), m);
        assert!(!serde_json::to_string(&old).unwrap().contains("movingOut"));

        // A mailbox of 0.4.0 (folder deletes only, or no queue at all) opens as it was.
        let before = r#"{"version": 1, "queued": [{"deleteFolder": {"account": 1, "id": "F9"}}]}"#;
        let mb: crate::Mailbox = serde_json::from_str(before).unwrap();
        assert_eq!(mb.queued, [crate::Queued::DeleteFolder { account: 1, id: "F9".into(), shown: Default::default() }]);
        let mb: crate::Mailbox = serde_json::from_str(r#"{"version": 1}"#).unwrap();
        assert!(mb.queued.is_empty());
        // A kind of change from a later version is left out; the rest is kept.
        let later = r#"{"version": 1, "queued": [{"somethingNew": {"x": 1}}, {"deleteMessage": {"account": 1, "id": "AAkB"}}]}"#;
        let mb: crate::Mailbox = serde_json::from_str(later).unwrap();
        assert_eq!(mb.queued, [crate::Queued::DeleteMessage { account: 1, id: "AAkB".into(), kept: None }]);

        let mut mb = crate::Mailbox::default();
        mb.queued = vec![
            crate::Queued::CopyMessage { account: 1, id: "AAkA".into(), to: "F2".into(), copy: 12, marker: "mc-0f3a".into() },
            crate::Queued::DeleteMessage { account: 1, id: "AAkB".into(), kept: None },
        ];
        let json = serde_json::to_string(&mb).unwrap();
        assert!(json.contains(r#"{"copyMessage":{"account":1,"id":"AAkA","to":"F2","copy":12,"marker":"mc-0f3a"}}"#), "{json}");
        assert_eq!(serde_json::from_str::<crate::Mailbox>(&json).unwrap().queued, mb.queued);
        assert!(mb.queued.iter().all(|q| q.account() == 1));
    }

    #[test]
    fn the_password_never_shows() {
        let req = SyncRequest {
            password: "hunter2".into(),
            account: Account { email: "ada@example.org".into(), ..Default::default() },
            ..Default::default()
        };
        assert!(!format!("{req:?}").contains("hunter2"));
        assert!(!serde_json::to_string(&req).unwrap().contains("hunter2"));
        assert_eq!(req.secret(), "hunter2");
    }

    #[test]
    fn a_token_never_shows_and_stands_in_for_the_password() {
        let account = Account { email: "ada@example.org".into(), oauth: Some(crate::OAuth::default()), ..Default::default() };
        let req = SyncRequest { account, password: "hunter2".into(), access_token: "tok-en-123".into(), ..Default::default() };
        assert_eq!(req.secret(), "tok-en-123");
        assert!(!format!("{req:?}").contains("tok-en-123"));
        assert!(!serde_json::to_string(&req).unwrap().contains("tok-en-123"));
        let tokens = Tokens { access: "tok-en-123".into(), refresh: "re-fresh-456".into(), expires_at: 1_000, ..Default::default() };
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("tok-en-123") && !shown.contains("re-fresh-456"));
        assert!(tokens.fresh(650) && !tokens.fresh(700) && !tokens.fresh(950) && !Tokens::default().fresh(0));
        // (#103) Who signed in, against the account.
        let ada = Tokens { email: " Ada@Example.org ".into(), ..Default::default() };
        assert!(!ada.someone_else("ada@example.org", "") && !ada.someone_else("ada.alias@example.org", "ADA@example.org"));
        assert!(ada.someone_else("admin@example.org", "") && ada.someone_else("admin@example.org", "admin@example.org"));
        assert!(ada.someone_else("", "") && !Tokens::default().someone_else("admin@example.org", ""));
    }

    #[test]
    fn an_account_without_oauth_reads_and_writes_as_before() {
        let plain: Account = serde_json::from_str(r#"{"email":"ada@example.org"}"#).unwrap();
        assert_eq!(plain.oauth, None);
        assert!(!serde_json::to_string(&plain).unwrap().contains("oauth"));
        let signed: Account = serde_json::from_str(r#"{"email":"ada@example.org","oauth":{"provider":"microsoft","tenant":"consumers"}}"#).unwrap();
        assert_eq!(signed.oauth.as_ref().map(|o| o.tenant.as_str()), Some("consumers"));
        assert_eq!(signed.login(), "ada@example.org");
    }

    #[test]
    fn changes_for_the_server_read_and_write_as_json() {
        let ops = vec![
            RemoteOp::Flags { folder: "INBOX".into(), uid: 7, read: Some(true), flagged: None, answered: Some(false) },
            RemoteOp::Move { message: 12, from: "INBOX".into(), uid: 7, to: "Archive".into(), to_folder: 4, create: vec![] },
            RemoteOp::Move { message: 13, from: "INBOX".into(), uid: 8, to: String::new(), to_folder: 5, create: vec!["Receipts".into()] },
            RemoteOp::Delete { from: "Trash".into(), uid: 2 },
        ];
        assert_eq!(ops.iter().map(RemoteOp::mailbox).collect::<Vec<_>>(), ["INBOX", "INBOX", "INBOX", "Trash"]);
        let req = SyncRequest { password: "hunter2".into(), ops, ..Default::default() };
        let json = serde_json::to_string(&req).unwrap();
        // A flag that wasn't changed here isn't named at all.
        assert!(json.contains(r#"{"flags":{"folder":"INBOX","uid":7,"read":true,"answered":false}}"#), "{json}");
        assert!(json.contains(r#"{"move":{"message":12,"from":"INBOX","uid":7,"to":"Archive","toFolder":4}}"#), "{json}");
        assert!(json.contains(r#""toFolder":5,"create":["Receipts"]"#) && json.contains(r#"{"delete":{"from":"Trash","uid":2}}"#), "{json}");
        let back: SyncRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ops, req.ops);
        assert!(format!("{req:?}").contains("ops: 4") && !format!("{req:?}").contains("hunter2"));
        assert!(!serde_json::to_string(&SyncRequest::default()).unwrap().contains("ops"));

        let result = SyncResult {
            flagged: vec![FlagsPushed { folder: "INBOX".into(), uid: 7, read: Some(true), ..Default::default() }],
            moved: vec![Moved { message: 12, from: "INBOX".into(), uid: 7, to_folder: 4, new_uid: 31, uid_validity: 9 }, Moved::default()],
            deleted: vec![Deleted { from: "Trash".into(), uid: 2 }],
            created: vec![Created { folder: 5, remote: "Receipts".into() }],
            filed: vec![3],
            ..Default::default()
        };
        assert_eq!(result.pushed_count(), 5);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r#""toFolder":4,"newUid":31,"uidValidity":9"#), "{json}");
        assert_eq!(serde_json::from_str::<SyncResult>(&json).unwrap(), result);
        // A result from before any of this was there reads as one that pushed nothing.
        let old: SyncResult = serde_json::from_str(r#"{"account": 3, "sent": [1], "errors": []}"#).unwrap();
        assert_eq!((old.pushed_count(), old.filed.len()), (0, 0));
        let plain = serde_json::to_string(&old).unwrap();
        assert!(!plain.contains("moved") && !plain.contains("filed"), "{plain}");
    }

    #[test]
    fn the_pieces_of_an_answer_come_together() {
        let m = |uid: u32| Message { uid, ..Default::default() };
        let delta =
            |remote: &str, uid_seen, new: Vec<Message>| FolderDelta { remote: remote.into(), uid_validity: 7, uid_seen, new, ..Default::default() };
        let pieces = [
            SyncResult { account: 3, partial: true, progress: "Sent 1 of 1".into(), sent: vec![9], filed: vec![9], ..Default::default() },
            // A line of progress and a problem to tell: nothing for a mailbox to take in.
            SyncResult { account: 3, partial: true, progress: "Connecting".into(), errors: vec!["One".into()], ..Default::default() },
            SyncResult {
                account: 3,
                partial: true,
                progress: "Inbox: 2 of 4".into(),
                unfiled: vec![9],
                flagged: vec![FlagsPushed::default()],
                folders: vec![FolderDelta {
                    flags: vec![RemoteFlags { uid: 1, ..Default::default() }],
                    removed: vec![2],
                    ..delta("INBOX", 4, vec![m(7), m(8)])
                }],
                ..Default::default()
            },
            SyncResult {
                account: 3,
                email: "ada@example.org".into(),
                stopped: true,
                errors: vec!["Two".into()],
                folders: vec![FolderDelta { id: Some(5), ..delta("INBOX", 8, vec![m(5), m(6)]) }, delta("Sent", 1, vec![m(1)])],
                ..Default::default()
            },
        ];
        assert_eq!(pieces.iter().map(SyncResult::has_news).collect::<Vec<_>>(), [true, false, true, true]);
        let mut whole = SyncResult::default();
        assert!(!whole.has_news());
        pieces.into_iter().for_each(|piece| whole.absorb(piece));
        // It is the last piece that says whose answer it is and how the run ended.
        assert_eq!(
            (whole.account, whole.email.as_str(), whole.partial, whole.progress.as_str(), whole.stopped),
            (3, "ada@example.org", false, "", true)
        );
        assert_eq!((whole.sent.as_slice(), whole.flagged.len(), whole.errors.join(", ").as_str()), (&[9][..], 1, "One, Two"));
        // A copy that didn't get to the server after all was never there.
        assert_eq!((whole.filed.len(), whole.unfiled.as_slice()), (0, &[9][..]));
        // What was said of a mailbox comes together: read as far as the piece that got furthest,
        // its new messages in the order they came.
        let inbox = &whole.folders[0];
        assert_eq!((inbox.id, inbox.uid_seen, inbox.flags.len(), inbox.removed.as_slice()), (Some(5), 8, 1, &[2][..]));
        assert_eq!(inbox.new.iter().map(|m| m.uid).collect::<Vec<_>>(), [7, 8, 5, 6]);
        assert_eq!((whole.folders.len(), whole.new_count()), (2, 5));
        // A mailbox that was made anew meanwhile is another mailbox.
        whole.absorb(SyncResult { folders: vec![FolderDelta { uid_validity: 8, ..delta("INBOX", 1, vec![m(1)]) }], ..Default::default() });
        assert_eq!((whole.folders.len(), whole.folders[0].uid_seen, whole.folders[2].uid_seen), (3, 8, 1));

        // What a piece says of itself is written only when there is something to say, and an
        // answer from before there were pieces reads as a whole one.
        let piece = SyncResult { partial: true, progress: "Inbox: 1 of 2".into(), stopped: true, unfiled: vec![4], ..Default::default() };
        let json = serde_json::to_string(&piece).unwrap();
        assert!(json.contains(r#""partial":true,"progress":"Inbox: 1 of 2","stopped":true"#) && json.contains(r#""unfiled":[4]"#), "{json}");
        assert_eq!(serde_json::from_str::<SyncResult>(&json).unwrap(), piece);
        let plain = serde_json::to_string(&SyncResult::default()).unwrap();
        assert!(!["partial", "progress", "stopped", "unfiled"].iter().any(|word| plain.contains(word)), "{plain}");
        let old: SyncResult = serde_json::from_str(r#"{"account": 3, "sent": [1], "errors": []}"#).unwrap();
        assert!(!old.partial && !old.stopped && old.progress.is_empty() && old.unfiled.is_empty() && old.has_news());
    }

    #[test]
    fn the_flags_a_server_would_hold() {
        let m = Message { read: true, flag: FlagStatus::Complete, answered: true, ..Default::default() };
        // A follow-up that is done with is no flag on a server.
        assert_eq!(RemoteFlags::of(&m, 7), RemoteFlags { uid: 7, read: true, flagged: false, answered: true, draft: false });
        let flagged = Message { flag: FlagStatus::Flagged, draft: true, ..Default::default() };
        assert_eq!(RemoteFlags::of(&flagged, 0).at(9), RemoteFlags { uid: 9, read: false, flagged: true, answered: false, draft: true });
    }

    #[test]
    fn summaries() {
        assert_eq!(summary_line(None, 1, 2), "1 new message, 2 sent");
        assert_eq!(summary_line(Some("Couldn't sign in"), 1, 2), "Couldn't sign in");
        let mut r = SyncResult::default();
        assert_eq!(r.summary(), "Up to date");
        r.folders.push(FolderDelta { new: vec![Message::default(), Message::default()], ..Default::default() });
        assert_eq!(r.summary(), "2 new messages");
        r.sent.push(1);
        assert_eq!(r.summary(), "2 new messages, 1 sent");
        r.errors.push("Couldn't sign in".into());
        assert_eq!(r.summary(), "Couldn't sign in");
    }
}
