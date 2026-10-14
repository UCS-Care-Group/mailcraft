//! What a mail-server synchronisation is asked to do and what it brings back.
//!
//! Plain data, so the pieces stay apart: the engine builds a [`SyncRequest`] and applies a
//! [`SyncResult`] (both pure), and a host with sockets (`mailcraft-net`, on the desktop and in the
//! CLI) does the talking in between, usually on a background thread.

use serde::{Deserialize, Serialize};

use crate::{Account, FlagStatus, FolderKind, Id, Message};

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
    /// UIDs of the messages the mailbox has in this folder (to notice deletions and flag changes).
    pub known_uids: Vec<u32>,
    /// How the server's mailbox stood when it was last read ([`FolderMark`]).
    pub mark: FolderMark,
}

/// What a synchronisation notes about a server mailbox beyond how far it was read, so that the
/// next one does no more than it has to.
///
/// A mailbox that was read through, with nothing left to bring, keeps its `UIDNEXT` and how many
/// messages it had. When the server says the same of it the next time (and the same
/// `UIDVALIDITY`), nothing came and nothing went: the flags of its newest messages are looked at,
/// and those of all the others only every few runs, instead of every message's each time (a
/// server without CONDSTORE, such as Exchange, has no cheaper way to tell what changed).
///
/// Messages left for later because a run takes only so many of a mailbox (the newest win) are
/// below `backfill`: later runs bring them, newest first, as far as their limit allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderMark {
    /// `UIDNEXT` of the mailbox when it was last read through; 0 while it isn't (the next run
    /// reads it whole).
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_next: u32,
    /// How many messages it had then.
    #[serde(skip_serializing_if = "is_zero")]
    pub exists: u32,
    /// Runs since the flags of every message were last read.
    #[serde(skip_serializing_if = "is_zero")]
    pub quick_runs: u32,
    /// Messages at or below this UID that the mailbox doesn't have were left for later runs;
    /// 0: none.
    #[serde(skip_serializing_if = "is_zero")]
    pub backfill: u32,
}

impl FolderMark {
    pub fn is_empty(&self) -> bool {
        *self == FolderMark::default()
    }
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
    /// Categories given to or taken from the message that is `uid` in `folder`, as IMAP keywords
    /// ([`category_keyword`]). Only those that were changed here are named; the server's other
    /// keywords stay as they are.
    Categories {
        folder: String,
        uid: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
    },
}

/// A message here that the server mailbox of its folder doesn't have yet: a draft written here,
/// a copy, mail brought in from a file. The host puts it there (APPEND), or, when `copy_from`
/// names a message the server has with the same content, copies that one (UID COPY), which
/// moves no bytes and loses nothing of a message that was only downloaded in part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    /// The message as it is here (its `id` and `folder` say which it is).
    pub message: Message,
    /// The server path of its folder.
    pub to: String,
    /// A mailbox where the server has the same message, and its UID there; empty and 0 when
    /// there is none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copy_from: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub copy_uid: u32,
    /// The message may only be copied, never put there from here: what is here of it is not all
    /// of it (it was too large to download).
    #[serde(default, skip_serializing_if = "is_false")]
    pub copy_only: bool,
}

/// An [`Upload`] that is done: the server has the message in the mailbox `to`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Uploaded {
    pub message: Id,
    /// The local folder it was in.
    pub folder: Id,
    pub to: String,
    /// Its UID there, when the server said (`APPENDUID`, `COPYUID`), or when a message with its
    /// Message-ID was there already; 0 otherwise: the copy is then known by its Message-ID when
    /// it comes down.
    #[serde(skip_serializing_if = "is_zero")]
    pub uid: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_validity: u32,
    /// The Message-ID it went with: a message here that has another one by now (a draft saved
    /// again meanwhile) is not the one the server has.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    /// The flags it has there.
    pub flags: RemoteFlags,
    /// It was copied from another of the server's messages, keywords and all, rather than put
    /// there from here.
    #[serde(skip_serializing_if = "is_false")]
    pub copied: bool,
}

/// A change of categories the server took on ([`RemoteOp::Categories`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoriesPushed {
    pub folder: String,
    pub uid: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

/// The keywords a server keeps on a message, as categories ([`keyword_category`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteCategories {
    pub uid: u32,
    pub names: Vec<String>,
}

/// Longest category name that goes to a server as a keyword, or comes from one, in characters.
pub const MAX_KEYWORD_CATEGORY: usize = 64;
/// Most categories one message takes from the server's keywords.
pub const MAX_MESSAGE_KEYWORDS: usize = 16;

/// Keywords that servers and other programs use for something other than a category.
const NOT_CATEGORIES: [&str; 8] = ["junk", "nonjunk", "notjunk", "forwarded", "mdnsent", "redirected", "phishing", "unknown"];

/// The IMAP keyword a category goes to a server as: its name with spaces as `_`
/// (`Red category` is `Red_category`). Only a name of printable ASCII, without `_`, without the
/// characters a keyword can't hold, not starting with `$` (which servers keep for their own) and
/// not too long has one: any other category stays here only. The name read back from the
/// keyword ([`keyword_category`]) is the name.
pub fn category_keyword(name: &str) -> Option<String> {
    let name = name.trim();
    let usable = !name.is_empty()
        && name.len() <= MAX_KEYWORD_CATEGORY
        && !name.starts_with('$')
        && !name.contains("  ")
        && name.chars().all(|c| c == ' ' || (c.is_ascii_graphic() && !"(){%*\"\\]_[".contains(c)))
        && !NOT_CATEGORIES.iter().any(|other| name.eq_ignore_ascii_case(other));
    usable.then(|| name.replace(' ', "_"))
}

/// The category a keyword the server keeps on a message stands for (see [`category_keyword`]);
/// `None` for a system flag (`\Seen`), a keyword of the server's or another program's own
/// (`$Forwarded`, `Junk`), and anything that isn't a name of printable ASCII of a sensible length.
/// What comes from a server is the server's word: nothing else gets through.
pub fn keyword_category(keyword: &str) -> Option<String> {
    let name = keyword.replace('_', " ");
    category_keyword(&name).filter(|back| back == keyword).map(|_| name)
}

/// A change made here to a folder the server has, or is to have ([`SyncRequest::folder_ops`]).
/// Mailboxes are named by their path here ([`FolderState::remote`]); the host spells the new
/// path as the server wants it (its own delimiter, modified UTF-7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderOp {
    /// Make a mailbox for local folder `folder`, called `name`, inside the mailbox `parent`
    /// (empty: at the top level).
    Create { folder: Id, parent: String, name: String },
    /// The mailbox `from`, which local folder `folder` follows, is to be called `name` and lie
    /// inside `parent` (empty: the top level; `None`: where it lies now). The mailboxes inside it
    /// go with it. Deleting a folder is this too: it goes into Deleted Items, as Outlook has it.
    Rename {
        folder: Id,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        name: String,
    },
    /// The mailbox `remote` is deleted for good, with its messages. Only ever one inside the
    /// server's Deleted Items: anything else stays on the server.
    Delete { remote: String },
}

/// What became of a [`FolderOp`] that was tried. One that isn't in the answer wasn't tried, and
/// is asked for again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderPushed {
    /// The local folder it was for; 0 for a deletion.
    pub folder: Id,
    /// The mailbox's path before (empty for one that was made).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// Its path now (empty for one that was deleted, or when it was refused).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// What separates the levels of the server's paths (empty when it has no levels): the
    /// mailboxes inside a renamed one are renamed with it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delimiter: String,
    /// The server wouldn't, or couldn't (see the errors): it is as it was there, and the change
    /// made here stays here only.
    #[serde(skip_serializing_if = "is_false")]
    pub refused: bool,
}

/// `path` once the mailbox `from` is called `to`: itself, or one inside it (`delimiter` parts
/// the levels); `None` when it is neither.
pub fn renamed_path(path: &str, from: &str, to: &str, delimiter: &str) -> Option<String> {
    if from.is_empty() {
        return None;
    }
    if path == from {
        return Some(to.to_string());
    }
    if delimiter.is_empty() {
        return None;
    }
    let rest = path.strip_prefix(from)?.strip_prefix(delimiter)?;
    Some(format!("{to}{delimiter}{rest}"))
}

impl RemoteOp {
    /// The server mailbox the change is made in.
    pub fn mailbox(&self) -> &str {
        match self {
            RemoteOp::Flags { folder, .. } | RemoteOp::Categories { folder, .. } => folder,
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
    /// Most messages to download per folder in one run (the newest win); 0 means no limit.
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
    /// Folders made, renamed, moved or deleted here, in the order it was done: done on the server
    /// before anything else, so that the rest of the run finds the mailboxes where they are now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folder_ops: Vec<FolderOp>,
    /// For a shared mailbox ([`Account::shared_of`]): the name the outgoing server is signed in
    /// to with, which is that of the owner whose token goes along (the incoming server is signed
    /// in to with the shared mailbox's address). Mail then goes out from the shared mailbox's
    /// address, which the server allows to an owner with the Send As right. Empty for any other
    /// account: both servers are signed in to with [`Account::login`]. A name, not a secret.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub send_login: String,
    /// Messages here that the server mailboxes of their folders don't have yet ([`Upload`]), to
    /// put there before the `ops` are pushed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uploads: Vec<Upload>,
}

impl SyncRequest {
    /// The account as the outgoing server is signed in to with it: as it is, or, for a shared
    /// mailbox, under the name of the owner whose token goes along ([`SyncRequest::send_login`]).
    pub fn sending_account(&self) -> std::borrow::Cow<'_, Account> {
        let login = self.send_login.trim();
        if login.is_empty() {
            std::borrow::Cow::Borrowed(&self.account)
        } else {
            std::borrow::Cow::Owned(Account { username: login.to_string(), ..self.account.clone() })
        }
    }

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
            .field("uploads", &self.uploads.len())
            .finish()
    }
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

impl Tokens {
    /// Whether the access token is still good at `now` (Unix seconds), with a minute to spare.
    pub fn fresh(&self, now: i64) -> bool {
        !self.access.is_empty() && self.expires_at.saturating_sub(60) > now
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

/// A value from a mailbox file, or `None` when what the file holds in its place isn't one (a
/// file from elsewhere, or from a later version): the mailbox still opens.
pub(crate) fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Maybe::<T>::deserialize(deserializer)? {
        Maybe::Is(value) => Some(value),
        Maybe::Not(_) => None,
    })
}

/// [`lenient`] for a value that has a "nothing was said" of its own: that, when what the file
/// holds in its place isn't one.
pub(crate) fn lenient_or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    lenient(deserializer).map(Option::unwrap_or_default)
}

/// A list from a mailbox file without the entries that aren't what they should be.
pub(crate) fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum List<T> {
        Is(Vec<Maybe<T>>),
        Not(serde::de::IgnoredAny),
    }
    Ok(match List::<T>::deserialize(deserializer)? {
        List::Is(entries) => entries
            .into_iter()
            .filter_map(|entry| match entry {
                Maybe::Is(value) => Some(value),
                Maybe::Not(_) => None,
            })
            .collect(),
        List::Not(_) => Vec::new(),
    })
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Maybe<T> {
    Is(T),
    Not(serde::de::IgnoredAny),
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
    /// Newly downloaded messages (`uid` set; `folder` and `account` are filled in when applied).
    pub new: Vec<Message>,
    /// Current flags of messages the mailbox already had.
    pub flags: Vec<RemoteFlags>,
    /// UIDs the mailbox has that the server no longer does.
    pub removed: Vec<u32>,
    /// The categories the server keeps as keywords on messages of `flags` ([`RemoteCategories`]):
    /// one of those not named here has none there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<RemoteCategories>,
    /// How the mailbox stands now ([`FolderMark`]): the last piece that speaks of it says.
    #[serde(skip_serializing_if = "FolderMark::is_empty")]
    pub mark: FolderMark,
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
    /// `listed` is every folder of mail the server has: the list was read whole, it wasn't cut
    /// short, and the Inbox is in it. A folder here that follows a mailbox not in such a list is
    /// gone from the server. Without it, nothing is taken for gone for not being listed.
    #[serde(skip_serializing_if = "is_false")]
    pub listed_whole: bool,
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
    /// What became of [`SyncRequest::folder_ops`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folders_pushed: Vec<FolderPushed>,
    /// What became of [`SyncRequest::uploads`]: the ones that are on the server now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uploaded: Vec<Uploaded>,
    /// Changes of categories the server took on ([`RemoteOp::Categories`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categorized: Vec<CategoriesPushed>,
    /// Server mailboxes that keep no keywords of the owner's choosing (Exchange): categories
    /// given to their messages stay here, and are not asked of the server again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords_refused: Vec<String>,
    /// Problems worth telling the user (one per failed step); empty when all went well. In a
    /// piece of an answer: the problems since the piece before.
    pub errors: Vec<String>,
    /// This is a piece of an answer, and more follows. The last piece doesn't have it.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// One short line on how far the run has got (`Inbox: 150 of 500`), for a status bar. Empty
    /// in the last piece, whose summary says how it went.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub progress: String,
    /// The run was called off before it had done all there was to do (in the last piece).
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
}

impl SyncResult {
    pub fn new_count(&self) -> usize {
        self.folders.iter().map(|f| f.new.len()).sum()
    }

    /// How many changes made here the server took on.
    pub fn pushed_count(&self) -> usize {
        let folders = self.folders_pushed.iter().filter(|f| !f.refused).count();
        self.flagged.len() + self.moved.len() + self.deleted.len() + self.created.len() + folders + self.uploaded.len() + self.categorized.len()
    }

    /// Whether there is anything in it for a mailbox to take in. A piece with only a line of
    /// progress, or only problems to tell, has nothing.
    pub fn has_news(&self) -> bool {
        !(self.folders.is_empty()
            && self.listed.is_empty()
            && self.sent.is_empty()
            && self.filed.is_empty()
            && self.unfiled.is_empty()
            && self.pushed_count() == 0
            && self.folders_pushed.is_empty()
            && self.keywords_refused.is_empty())
    }

    /// Add the piece that came after this one, so that the two are one answer: taking it in is
    /// the same as taking in the one and then the other. What a mailbox's pieces say about it
    /// comes together under its name, its new messages in the order they came.
    pub fn absorb(&mut self, later: SyncResult) {
        (self.account, self.email) = (later.account, later.email);
        (self.partial, self.progress, self.stopped) = (later.partial, later.progress, later.stopped);
        if !later.listed.is_empty() {
            self.listed = later.listed;
            self.listed_whole = later.listed_whole;
        }
        self.folders_pushed.extend(later.folders_pushed);
        self.sent.extend(later.sent);
        // A copy that didn't get there after all is one that was never said to.
        self.filed.retain(|id| !later.unfiled.contains(id));
        self.filed.extend(later.filed);
        self.unfiled.extend(later.unfiled);
        self.flagged.extend(later.flagged);
        self.moved.extend(later.moved);
        self.deleted.extend(later.deleted);
        self.created.extend(later.created);
        self.uploaded.extend(later.uploaded);
        self.categorized.extend(later.categorized);
        self.keywords_refused.extend(later.keywords_refused);
        self.errors.extend(later.errors);
        for delta in later.folders {
            match self.folders.iter_mut().find(|f| f.remote == delta.remote && f.uid_validity == delta.uid_validity) {
                Some(folder) => {
                    folder.id = folder.id.or(delta.id);
                    // The mailbox is read as far as the piece that got furthest says.
                    folder.uid_seen = folder.uid_seen.max(delta.uid_seen);
                    folder.mark = delta.mark;
                    folder.new.extend(delta.new);
                    folder.flags.extend(delta.flags);
                    folder.removed.extend(delta.removed);
                    folder.categories.extend(delta.categories);
                }
                None => self.folders.push(delta),
            }
        }
    }

    /// Whether `later`, the piece that came after this one, can be [absorbed](Self::absorb) into it
    /// without changing the order in which things are taken in: it says only what came in
    /// (mailboxes' news, problems, how far the run has got), and this piece isn't the last. A host
    /// that has fallen behind takes several such pieces in at once, which costs as much as one.
    pub fn can_absorb(&self, later: &SyncResult) -> bool {
        self.partial
            && self.account == later.account
            && self.email == later.email
            && later.listed.is_empty()
            && later.sent.is_empty()
            && later.filed.is_empty()
            && later.unfiled.is_empty()
            && later.pushed_count() == 0
            && later.keywords_refused.is_empty()
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
    if let Some(e) = error {
        return e.to_string();
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match (new, sent) {
        (0, 0) => "Up to date".into(),
        (n, 0) => format!("{n} new message{}", plural(n)),
        (0, s) => format!("{s} message{} sent", plural(s)),
        (n, s) => format!("{n} new message{}, {s} sent", plural(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(tokens.fresh(900) && !tokens.fresh(950) && !Tokens::default().fresh(0));
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
    fn pieces_that_only_bring_news_are_taken_in_together() {
        let news = |uid_seen| SyncResult {
            account: 3,
            partial: true,
            folders: vec![FolderDelta {
                remote: "INBOX".into(),
                uid_validity: 7,
                uid_seen,
                mark: FolderMark { backfill: uid_seen, ..Default::default() },
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut first = news(4);
        assert!(first.can_absorb(&news(8)) && first.can_absorb(&SyncResult { errors: vec!["x".into()], ..news(1) }));
        // What was done here or sent waits its turn, and so does another account's answer.
        assert!(!first.can_absorb(&SyncResult { flagged: vec![FlagsPushed::default()], ..news(1) }));
        assert!(!first.can_absorb(&SyncResult { sent: vec![1], ..news(1) }));
        assert!(!first.can_absorb(&SyncResult { listed: vec![ListedFolder::default()], ..news(1) }));
        assert!(!first.can_absorb(&SyncResult { account: 4, ..news(1) }));
        assert!(!SyncResult { partial: false, ..news(1) }.can_absorb(&news(2)));
        // The last word on how the mailbox stands is the later piece's.
        first.absorb(news(8));
        assert_eq!((first.folders.len(), first.folders[0].uid_seen, first.folders[0].mark.backfill), (1, 8, 8));
        // A folder's mark reads and writes as JSON, and an old file without it reads as none.
        let state: FolderState = serde_json::from_str(r#"{"id":1,"remote":"INBOX","uidSeen":3}"#).unwrap();
        assert!(state.mark.is_empty());
        let marked = FolderState { mark: FolderMark { uid_next: 9, exists: 4, quick_runs: 1, backfill: 2 }, ..state };
        let json = serde_json::to_string(&marked).unwrap();
        assert!(json.contains(r#""mark":{"uidNext":9,"exists":4,"quickRuns":1,"backfill":2}"#), "{json}");
        assert_eq!(serde_json::from_str::<FolderState>(&json).unwrap(), marked);
        assert!(!serde_json::to_string(&FolderDelta::default()).unwrap().contains("mark"));
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
    fn categories_and_keywords_go_both_ways() {
        assert_eq!(category_keyword("Red category").as_deref(), Some("Red_category"));
        assert_eq!(keyword_category("Red_category").as_deref(), Some("Red category"));
        assert_eq!(category_keyword(" Work ").as_deref(), Some("Work"));
        for name in
            ["", "  ", "$label1", "Junk", "nonjunk", "to_do", "a(b", "a\\b", "50%", "a]b", "Größe", "tab\there", "two  spaces", &"x".repeat(65)]
        {
            assert_eq!(category_keyword(name), None, "{name:?}");
        }
        // What a server says is only a category when it reads back to itself.
        for keyword in ["\\Seen", "$Forwarded", "Junk", "NonJunk", "_lead", "trail_", "a__b", "", "é", "x\r\ny"] {
            assert_eq!(keyword_category(keyword), None, "{keyword:?}");
        }
        assert_eq!(keyword_category("Project-X").as_deref(), Some("Project-X"));
        // Results with categories read and write as JSON, and old ones read as before.
        let result = SyncResult {
            uploaded: vec![Uploaded { message: 3, folder: 4, to: "Drafts".into(), uid: 9, uid_validity: 7, ..Default::default() }],
            categorized: vec![CategoriesPushed { folder: "INBOX".into(), uid: 2, add: vec!["Work".into()], remove: vec![] }],
            keywords_refused: vec!["INBOX".into()],
            ..Default::default()
        };
        assert_eq!((result.pushed_count(), result.has_news()), (2, true));
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(serde_json::from_str::<SyncResult>(&json).unwrap(), result);
        let refused = SyncResult { keywords_refused: vec!["INBOX".into()], ..Default::default() };
        assert!(refused.has_news() && !SyncResult { partial: true, ..Default::default() }.can_absorb(&refused));
        let op = RemoteOp::Categories { folder: "INBOX".into(), uid: 2, add: vec!["Work".into()], remove: vec![] };
        assert_eq!(serde_json::to_string(&op).unwrap(), r#"{"categories":{"folder":"INBOX","uid":2,"add":["Work"]}}"#);
        assert_eq!(op.mailbox(), "INBOX");
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
