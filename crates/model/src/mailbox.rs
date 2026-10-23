// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailbox: everything MailCraft keeps, with the primitives that edit it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::sync::RemoteFlags;
use crate::{Action, Calendar, Contact, Event, Id, JmapServer, KeepOffline, Message, ModelError, Rgb, Rule, Signature, Task, clean_name};

/// Deepest a folder tree may nest (a hostile file could otherwise chain folders without end).
const MAX_DEPTH: usize = 32;
/// Largest id a file may hold: 2^53, the largest integer every JSON reader keeps exactly.
const MAX_ID: Id = 1 << 53;
/// Most changes waiting for the mail server that a file may hold ([`Mailbox::pending`]).
const MAX_PENDING: usize = 100_000;
/// The most rules a mailbox keeps; a file with more is cut to this when it is repaired.
pub const MAX_RULES: usize = 500;

/// Most accounts, folders and colour categories a mailbox file is read with; more are dropped on
/// load ([`Mailbox::repair`]), so that a hostile file can't make the work on it grow without end.
pub const MAX_ACCOUNTS: usize = 1_000;
pub const MAX_FOLDERS: usize = 50_000;
pub const MAX_CATEGORIES: usize = 1_000;
/// Most shared mailboxes one account opens ([`Mailbox::add_shared_mailbox`]).
pub const MAX_SHARED: usize = 20;
/// Most folders one account's Favorites from its server hold ([`Mailbox::server_favorites`]).
const MAX_SERVER_FAVORITES: usize = crate::remote::MAX_FAVORITES;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderKind {
    Inbox,
    Drafts,
    Sent,
    Deleted,
    Junk,
    Archive,
    Outbox,
    #[default]
    Custom,
}

impl FolderKind {
    /// The folders every account has, in the order the folder pane lists them.
    pub const STANDARD: [FolderKind; 7] =
        [FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted, FolderKind::Archive, FolderKind::Junk, FolderKind::Outbox];

    pub fn default_name(self) -> &'static str {
        match self {
            FolderKind::Inbox => "Inbox",
            FolderKind::Drafts => "Drafts",
            FolderKind::Sent => "Sent Items",
            FolderKind::Deleted => "Deleted Items",
            FolderKind::Junk => "Junk Email",
            FolderKind::Archive => "Archive",
            FolderKind::Outbox => "Outbox",
            FolderKind::Custom => "Folder",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            FolderKind::Inbox => "inbox",
            FolderKind::Drafts => "drafts",
            FolderKind::Sent => "sent",
            FolderKind::Deleted => "deleted",
            FolderKind::Junk => "junk",
            FolderKind::Archive => "archive",
            FolderKind::Outbox => "outbox",
            FolderKind::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<FolderKind> {
        let s = s.trim().to_ascii_lowercase();
        Self::STANDARD.into_iter().chain([FolderKind::Custom]).find(|k| k.code() == s)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Folder {
    pub id: Id,
    pub account: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Id>,
    pub name: String,
    pub kind: FolderKind,
    pub favorite: bool,
    /// Its path on the server (IMAP), when it has one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// IMAP `UIDVALIDITY` and the highest UID already fetched.
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_validity: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_seen: u32,
    /// How far down the folder was last read ([`crate::sync::FolderState::uid_from`]).
    #[serde(skip_serializing_if = "is_zero")]
    pub uid_from: u32,
    /// For a folder of a [`AccountKind::Microsoft`] account that the server has: what the
    /// server calls it and what it last said of it ([`ServerFolder`]). `None` for a folder that
    /// only exists here (one made here that the server is yet to hear of, or a folder of any
    /// other account). A file written before this was kept has none.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub server: Option<ServerFolder>,
    /// A folder of a Microsoft account that was on its IMAP server before the account switched
    /// to Microsoft Graph (R3.7): it is the server's folder of its kind, or of its name in the
    /// same place, in Graph's first complete listing, and takes that folder's server id there
    /// (so what is kept of it here, its favourite mark and the rules that file into it, stays).
    /// It is never made on the server: one that complete listing doesn't have goes.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "crate::sync::lenient_or_default")]
    pub awaiting_listing: bool,
}

/// A folder as a Microsoft account's server knows it (ADR 0003): its server id, which stays the
/// same when the folder is renamed or moved, and what the server last said of its name and of
/// the folder it lies in. A change made here is the difference between those and the folder as
/// it is now; nothing else is kept of it.
///
/// The id is one mailbox's own: a folder of a shared mailbox (after beta) will also need that
/// mailbox's address, which goes beside the id here, not into it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerFolder {
    /// The server's id of the folder (Graph's immutable id).
    pub id: String,
    /// Its name, as the server last said.
    pub name: String,
    /// The server id of the folder it lies in, as the server last said; empty for one at the top
    /// of the mailbox.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent: String,
    /// Where the folder's mail goes on from next time (Graph's delta link, or the next page of a
    /// listing that was cut short); empty until its mail has been read once (R3.2).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delta: String,
    /// `delta` is a delta link: the folder's mail has been read to the end once, and the link
    /// brings what changed since. Otherwise it is the next page of a round not yet ended.
    #[serde(skip_serializing_if = "is_false")]
    pub caught_up: bool,
    /// How many messages the folder had, and how many unread, as the server's listing counted
    /// them at the start of the round that `delta` ended: a folder whose counts are the same
    /// again may be passed over by a Send/Receive (#276). `None` when not known for that link
    /// (a round not ended, one begun in an earlier run, a mailbox from before): never passed over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<FolderCounts>,
}

/// A folder's message counts as its server's listing gives them (#276).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FolderCounts {
    pub total: u32,
    pub unread: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Security {
    /// No encryption: only for a server on this machine.
    None,
    /// Upgrade a plain connection with STARTTLS.
    StartTls,
    /// TLS from the first byte.
    #[default]
    Tls,
}

impl Security {
    pub fn parse(s: &str) -> Option<Security> {
        match s.trim().to_ascii_lowercase().replace(['/', ' ', '-'], "").as_str() {
            "none" | "plain" => Some(Security::None),
            "starttls" => Some(Security::StartTls),
            "tls" | "ssl" | "ssltls" => Some(Security::Tls),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Security::None => "none",
            Security::StartTls => "starttls",
            Security::Tls => "tls",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountKind {
    /// Lives in this mailbox only.
    #[default]
    Local,
    /// Synchronised with an IMAP server; sends through SMTP.
    Imap,
    /// A Microsoft account (Outlook.com, Microsoft 365) whose folders and mail are kept in step
    /// through Microsoft Graph alone, never by IMAP or SMTP (ADR 0002). It has no server
    /// settings and signs in with Microsoft ([`Account::oauth`]). One that was kept in step by
    /// IMAP becomes one when its owner switches it ([`GraphSwitch`]).
    Microsoft,
    /// A mailbox on a JMAP server (RFC 8620 and 8621), kept in step over HTTPS alone, never by
    /// IMAP or SMTP. It has no mail server settings and no OAuth sign-in: its server is
    /// [`Account::jmap`], and the secret it is opened with (a password, or a bearer token) is
    /// given by whoever holds the connection and is never written anywhere.
    Jmap,
    /// A kind this version doesn't know: an account that a newer MailCraft added, in a file that
    /// one wrote (or one whose kind can't be read at all). Only what says which account it is
    /// is read of it; the whole of it is kept as the file had it and written back so
    /// ([`Account::as_written`]), so that the newer MailCraft finds it as it left it. It is never
    /// kept in step, sent from or changed here: whatever it is, this version doesn't know how.
    #[serde(other)]
    Unknown,
}

impl AccountKind {
    /// Whether the account is kept in step with a server (by IMAP, by Microsoft Graph or by JMAP).
    /// Not an account of a kind this version doesn't know: whatever server it has, this version
    /// can't use it.
    pub fn has_server(self) -> bool {
        matches!(self, AccountKind::Imap | AccountKind::Microsoft | AccountKind::Jmap)
    }

    /// Whether a Send/Receive sends the mail that waits in the account's Outbox (through SMTP, or
    /// through Microsoft Graph). Not yet for a JMAP account: its sending comes after its reading,
    /// and until then no mail is put in its Outbox, where it would wait for ever.
    pub fn sends_mail(self) -> bool {
        matches!(self, AccountKind::Imap | AccountKind::Microsoft)
    }

    /// Whether the account's folders and messages are known to its server by the ids the server
    /// hands out (Microsoft Graph, JMAP), rather than by IMAP's paths and UIDs.
    pub fn server_ids(self) -> bool {
        matches!(self, AccountKind::Microsoft | AccountKind::Jmap)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub id: Id,
    /// What the folder pane calls it (usually the address).
    pub name: String,
    pub email: String,
    /// The owner's name, as recipients see it.
    pub display_name: String,
    pub kind: AccountKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incoming: Option<Server>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outgoing: Option<Server>,
    /// Login name; the password is never stored in the mailbox.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The signature new messages start with (a [`Signature`] name; empty for none).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
    /// The signature replies and forwards start with (empty for none); `None` for an account from
    /// before there were two, which has [`Account::signature`] for both ([`Account::reply_signature`]).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub reply_signature: Option<String>,
    /// Set when the owner signs in on the provider's own page (OAuth 2.0) instead of with a
    /// password. No token is ever stored here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuth>,
    /// A mailbox written while the owner of an account could choose how its Favorites in
    /// Outlook were read (`favoritesSource`, 0.2.0 to 0.2.1) and had chosen anything but
    /// Microsoft Graph: Exchange Web Services, which is gone, or not now. That choice is no
    /// more: they are read through Graph, as for every account that signs in with Microsoft,
    /// and the owner is told so once ([`Mailbox::take_retired_favorites_choices`]). Read from a
    /// file, never written to one: the next save drops it.
    #[serde(rename = "favoritesSource", skip_serializing, deserialize_with = "retired_favorites_choice")]
    pub retired_favorites_choice: bool,
    /// For a Microsoft account: which messages are fetched in the background (R3 spec S12).
    #[serde(skip_serializing_if = "DownloadInFull::is_default", deserialize_with = "crate::sync::lenient_or_default")]
    pub download_in_full: DownloadInFull,
    /// **Mail to keep offline** (#143): how far back, by received date, this account's mail is
    /// kept in full on this computer; older mail keeps its headers and preview. A file from
    /// before there was one says nothing, which is [`KeepOffline::All`]: nothing is removed
    /// without the owner's choosing. A new account starts at 12 months (`account.add`). A shared
    /// mailbox follows its owner's ([`Mailbox::keep_offline_of`]).
    #[serde(skip_serializing_if = "KeepOffline::is_default", deserialize_with = "crate::sync::lenient_or_default")]
    pub keep_offline: KeepOffline,
    /// For a Microsoft account still kept in step by IMAP: what its owner said to switching it
    /// to Microsoft Graph (R3.7). Nothing for every other account.
    #[serde(skip_serializing_if = "GraphSwitch::is_default", deserialize_with = "crate::sync::lenient_or_default")]
    pub graph_switch: GraphSwitch,
    /// For a **shared mailbox** (a mailbox of the owner's organisation that the owner was given
    /// Full Access to, such as `support@`): the Microsoft account it is opened through, whose
    /// sign-in reads it. Its address is [`Account::email`]; it has no sign-in of its own, and its
    /// folders and messages are its own, so that the server ids of two mailboxes never meet
    /// (ADR 0003: the mailbox's address goes beside each id, here by the account the folder is
    /// in). `None` for every other account, and in a file from before there were any.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub shared_of: Option<Id>,
    /// For a [`AccountKind::Jmap`] account: where its server's session resource is and how the
    /// secret is sent ([`JmapServer`]). Plain settings: the secret is never kept here. `None` for
    /// every other account, and in a file from before there were any.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub jmap: Option<JmapServer>,
    /// For an account of a kind this version doesn't know ([`AccountKind::Unknown`]): the account
    /// as its file had it, every field but its id, which is what a mailbox file gets in its place
    /// ([`Mailbox::accounts`]). `None` for every other account.
    #[serde(skip)]
    pub as_written: Option<AsWritten>,
}

/// An account of a kind this version doesn't know, as a mailbox file had it: every field but its
/// id (the mailbox's to give: a repair may give it another), those this version doesn't know among
/// them, value for value ([`Account::as_written`]). What it holds is the account's settings, which
/// its debug output leaves out: it says how many fields there are.
#[derive(Clone, PartialEq, Eq)]
pub struct AsWritten(serde_json::Map<String, serde_json::Value>);

impl std::fmt::Debug for AsWritten {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsWritten").field("fields", &self.0.len()).finish()
    }
}

/// What is read of an account of a kind this version doesn't know: what says which account it is,
/// each field as it is when it can be read, and as nothing when it can't.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Identity {
    #[serde(deserialize_with = "crate::sync::lenient_or_default")]
    id: Id,
    #[serde(deserialize_with = "crate::sync::lenient_or_default")]
    name: String,
    #[serde(deserialize_with = "crate::sync::lenient_or_default")]
    email: String,
    #[serde(deserialize_with = "crate::sync::lenient_or_default")]
    display_name: String,
    #[serde(deserialize_with = "crate::sync::lenient")]
    shared_of: Option<Id>,
}

/// The accounts of a mailbox file. One of a kind this version doesn't know (its `kind` is a word
/// it doesn't know, or can't be read) is read for what says which account it is, and no more: not
/// its servers or its sign-in, which nothing here could use for it, and every other field is kept
/// as it was ([`AsWritten`]). Every other account is read as it always was.
fn read_accounts<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Account>, D::Error> {
    let written = crate::load_limit::capped_vec::<_, serde_json::Value>(deserializer)?;
    let mut accounts = Vec::with_capacity(written.len());
    for value in written {
        // (Only an object has a `kind`; one without says nothing, which is a local account.)
        let unknown = value.get("kind").is_some_and(|kind| AccountKind::deserialize(kind).unwrap_or(AccountKind::Unknown) == AccountKind::Unknown);
        if !unknown {
            accounts.push(Account::deserialize(&value).map_err(serde::de::Error::custom)?);
            continue;
        }
        let who = Identity::deserialize(&value).unwrap_or_default();
        let mut fields = match value {
            serde_json::Value::Object(fields) => fields,
            _ => serde_json::Map::new(),
        };
        fields.remove("id");
        accounts.push(Account {
            id: who.id,
            name: who.name,
            email: who.email,
            display_name: who.display_name,
            kind: AccountKind::Unknown,
            shared_of: who.shared_of,
            as_written: Some(AsWritten(fields)),
            ..Default::default()
        });
    }
    Ok(accounts)
}

/// The accounts as a mailbox file has them: each as the model writes it, but one of a kind this
/// version doesn't know, which is written as its file had it, with the id it has here.
fn write_accounts<S: serde::Serializer>(accounts: &[Account], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq as _;
    let mut list = serializer.serialize_seq(Some(accounts.len()))?;
    for account in accounts {
        match &account.as_written {
            Some(AsWritten(fields)) => {
                let mut fields = fields.clone();
                fields.insert("id".into(), account.id.into());
                list.serialize_element(&fields)?;
            }
            None => list.serialize_element(account)?,
        }
    }
    list.end()
}

/// What the owner of a Microsoft account kept in step by IMAP said to switching it to Microsoft
/// Graph (R3.7). Asked once; a file that says anything else is read as not asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GraphSwitch {
    /// Not asked yet.
    #[default]
    NotAsked,
    /// Later: the account stays on IMAP until IMAP is turned off for Microsoft accounts.
    Later,
    /// Switch: once what waits for the IMAP server has reached it, the account switches.
    Switching,
}

impl GraphSwitch {
    pub fn is_default(&self) -> bool {
        *self == GraphSwitch::NotAsked
    }
}

/// Which messages of a Microsoft account are **fetched** in the background ("Download messages in
/// full", per account; R3 spec S12). A larger one is fetched when it is opened, replied to,
/// forwarded or saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadInFull {
    #[default]
    UpTo10Mb,
    UpTo25Mb,
    UpTo50Mb,
    Everything,
}

impl DownloadInFull {
    pub fn is_default(&self) -> bool {
        *self == DownloadInFull::default()
    }

    /// The largest message fetched in the background, in bytes; `None` for no limit.
    pub fn limit(self) -> Option<u64> {
        match self {
            DownloadInFull::UpTo10Mb => Some(10 << 20),
            DownloadInFull::UpTo25Mb => Some(25 << 20),
            DownloadInFull::UpTo50Mb => Some(50 << 20),
            DownloadInFull::Everything => None,
        }
    }
}

/// Whether what a mailbox file says where `favoritesSource` was is a choice that no longer
/// stands ([`Account::retired_favorites_choice`]): Exchange Web Services, or not now, in the
/// words those versions wrote and read. Whatever else is there, the file opens, and nothing is
/// said of it.
fn retired_favorites_choice<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let said: Option<String> = crate::sync::lenient(deserializer)?;
    Ok(said.is_some_and(|way| matches!(way.trim().to_ascii_lowercase().as_str(), "ews" | "none" | "notnow" | "own")))
}

/// Which kind of Microsoft account signs in: one of an organisation (work or school), or a
/// personal one (Outlook.com). The sign-in itself says which, by the directory it names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Directory {
    /// Nobody has said yet.
    #[default]
    Unknown,
    /// A work or school account: its organisation's own directory.
    Organisation,
    /// A personal Microsoft account.
    Personal,
}

impl Directory {
    /// The directory every personal Microsoft account is in, as an identity token names it
    /// (its `tid` claim; Microsoft identity platform, "ID token claims reference").
    pub const PERSONAL_TENANT: &'static str = "9188040d-6c67-4c5b-b112-36a304b66dad";

    pub fn is_unknown(&self) -> bool {
        *self == Directory::Unknown
    }

    pub fn parse(s: &str) -> Option<Directory> {
        match s.trim().to_ascii_lowercase().as_str() {
            "organisation" | "organization" | "work" => Some(Directory::Organisation),
            "personal" | "consumer" => Some(Directory::Personal),
            _ => None,
        }
    }

    /// What a command calls it; `None` when it isn't known.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Directory::Unknown => None,
            Directory::Organisation => Some("organisation"),
            Directory::Personal => Some("personal"),
        }
    }

    /// The kind of account that signed in to the directory `tenant`: what an identity token
    /// names (a GUID), or what the sign-in settings name (`consumers`, `organizations`, a
    /// directory's id or domain). `common` and nothing say nothing.
    pub fn of_tenant(tenant: &str) -> Directory {
        match tenant.trim().to_ascii_lowercase().as_str() {
            "" | "common" => Directory::Unknown,
            "consumers" | Directory::PERSONAL_TENANT => Directory::Personal,
            _ => Directory::Organisation,
        }
    }
}

impl Account {
    /// The name of the signature replies and forwards start with (empty for none).
    pub fn reply_signature(&self) -> &str {
        self.reply_signature.as_deref().unwrap_or(&self.signature)
    }

    pub fn address(&self) -> crate::Address {
        crate::Address::new(self.display_name.clone(), self.email.clone())
    }

    /// Whether it is a shared mailbox opened through another account ([`Account::shared_of`]).
    pub fn is_shared(&self) -> bool {
        self.shared_of.is_some()
    }

    /// The name the servers know the owner by: the login name, or else the address.
    pub fn login(&self) -> &str {
        let user = self.username.trim();
        if user.is_empty() { self.email.trim() } else { user }
    }

    /// A Microsoft account (it signs in with Microsoft) that is still kept in step by IMAP and
    /// SMTP: one that can switch to Microsoft Graph (R3.7).
    pub fn on_imap_with_microsoft(&self) -> bool {
        self.kind == AccountKind::Imap && self.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Microsoft)
    }
}

/// Who signs the owner in when it isn't a password.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuthProvider {
    /// Outlook.com and Microsoft 365.
    #[default]
    Microsoft,
}

impl OAuthProvider {
    pub fn parse(s: &str) -> Option<OAuthProvider> {
        match s.trim().to_ascii_lowercase().as_str() {
            "microsoft" | "outlook" | "office365" | "microsoft365" => Some(OAuthProvider::Microsoft),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            OAuthProvider::Microsoft => "microsoft",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OAuthProvider::Microsoft => "Microsoft",
        }
    }
}

/// OAuth 2.0 sign-in: the mail servers take a short-lived token (SASL XOAUTH2) instead of a
/// password. Plain settings only; the tokens live in memory with whoever holds the sockets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OAuth {
    pub provider: OAuthProvider,
    /// The application (client) id presented to the provider; empty means the build's own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    /// Microsoft: `common`, `consumers`, `organizations` or a directory id; empty means `common`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant: String,
    /// Microsoft: whether the account that signs in is an organisation's or a personal one, when
    /// that is known ([`Directory`]). Written when the account is set up, and not otherwise;
    /// never the directory's id.
    #[serde(skip_serializing_if = "Directory::is_unknown", deserialize_with = "crate::sync::lenient_or_default")]
    pub directory: Directory,
}

/// A change made here to what a Microsoft account's server has that comparing with what the
/// server last said can't show (ADR 0003): it waits here until a run has told the server. Each
/// names what it is about by its server id. A file written before a kind was added opens with
/// none of it; an entry of a kind this version doesn't know is left out ([`Mailbox::queued`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Queued {
    /// A folder was deleted for good here (from Deleted Items, or by emptying it): the server
    /// deletes it too, which keeps it recoverable for the server's retention period.
    DeleteFolder { account: Id, id: String },
    /// A message the server has was copied here into another of the account's server folders
    /// (R3 spec S16, S21). `id` is the source's server id, `to` the destination folder's;
    /// `copy` is the copy made here, which takes the server's copy's id once the server has
    /// made it (so that the copy coming down by delta isn't a second one). `marker` is the value,
    /// used once, of MailCraft's invisible extended property the source is given before the
    /// copy is asked for: a copy whose answer was lost is looked for by it in `to` before the
    /// copy is asked for again.
    #[serde(rename_all = "camelCase")]
    CopyMessage { account: Id, id: String, to: String, copy: Id, marker: String },
    /// A message was deleted for good here (Shift+Del, or emptying Deleted Items): the server
    /// deletes it with Graph's delete, which keeps it recoverable for the server's retention
    /// period (ADR 0003, R3 spec S19). `kept` is the message as it was here, kept until the
    /// server has answered: one the server refuses to delete is put back from it, with all that
    /// was fetched of it (S20). An entry without one (from an earlier version, or for a copy the
    /// server made of a message gone here) is put back from what the server says of it.
    DeleteMessage {
        account: Id,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kept: Option<Box<Message>>,
    },
    /// The message `message` was moved or copied here into a folder of `account` from another
    /// mailbox of the same owner's (#70): the server of `account` is to make it, from the whole
    /// message, carrying MailCraft's marker of the value `marker` (by which a make whose answer
    /// was lost is found). Of a move, the original goes from its own mailbox once this copy is
    /// there ([`Message::moving_out`]). `copy`: it was copied, and the message it was copied from
    /// is still where it was (a copy refused is taken away here again).
    #[serde(rename_all = "camelCase")]
    PlaceMessage {
        account: Id,
        message: Id,
        marker: String,
        #[serde(default, skip_serializing_if = "is_false")]
        copy: bool,
    },
}

impl Queued {
    pub fn account(&self) -> Id {
        match self {
            Queued::DeleteFolder { account, .. }
            | Queued::CopyMessage { account, .. }
            | Queued::DeleteMessage { account, .. }
            | Queued::PlaceMessage { account, .. } => *account,
        }
    }
}

/// A change made here to a message the mail server has, which the server is yet to be told. The
/// next synchronisation pushes it, and it stays listed until the server has taken it on
/// ([`crate::sync::RemoteOp`]). The server's copy is named by the path of its mailbox (`from`,
/// the [`Folder::remote`] of a folder of `account`) and its UID there.
///
/// Flag changes aren't listed: they show as a message whose flags differ from its
/// [`Message::synced`]. Nor is anything done to a folder: removing or renaming one is not
/// pushed to the server, and removing a folder or an account drops the changes that mention it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pending {
    /// A message that was `uid` in server mailbox `from` now belongs in local folder `to`.
    /// `synced` is what its [`Message::synced`] was when it left, for when it has a UID again.
    Move {
        account: Id,
        message: Id,
        from: String,
        uid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
        synced: Option<RemoteFlags>,
        to: Id,
    },
    /// The message that was `uid` in server mailbox `from` was deleted for good. `message_id`
    /// is its Message-ID, by which it is found again when the server numbers the mailbox anew
    /// (`uid` is 0 until then).
    Delete {
        account: Id,
        from: String,
        uid: u32,
        #[serde(default, rename = "messageId", skip_serializing_if = "String::is_empty")]
        message_id: String,
    },
}

impl Pending {
    pub fn account(&self) -> Id {
        match self {
            Pending::Move { account, .. } | Pending::Delete { account, .. } => *account,
        }
    }

    /// The server's copy the change is about: its mailbox's path and its UID there.
    pub fn source(&self) -> (&str, u32) {
        match self {
            Pending::Move { from, uid, .. } | Pending::Delete { from, uid, .. } => (from, *uid),
        }
    }
}

/// The Favorites of an account as its server lists them (the folders Outlook shows under
/// Favorites): the mailbox's own folders for them, in the server's order. While an account has
/// such a list, the folder pane can show it in place of the favourites that are marked here
/// ([`Folder::favorite`]), which stay as they are. Only ever read from the server, never sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerFavorites {
    pub account: Id,
    /// Folders of that account, in the server's order. May be empty: the server has a list, and
    /// nothing in it is a folder of this mailbox.
    pub folders: Vec<Id>,
}

/// A colour category (`Red category`), applied to messages, events, contacts and tasks by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Category {
    pub name: String,
    pub color: Rgb,
}

/// The categories a new mailbox starts with (our own colours).
fn default_categories() -> Vec<Category> {
    [
        ("Red category", Rgb(0xD1, 0x3B, 0x3B)),
        ("Orange category", Rgb(0xE0, 0x7B, 0x1F)),
        ("Yellow category", Rgb(0xD9, 0xA4, 0x0C)),
        ("Green category", Rgb(0x2E, 0x8B, 0x57)),
        ("Blue category", Rgb(0x2F, 0x6F, 0xD0)),
        ("Purple category", Rgb(0x7E, 0x57, 0xC2)),
    ]
    .into_iter()
    .map(|(name, color)| Category { name: name.into(), color })
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mailbox {
    /// File format version.
    pub version: u32,
    /// An account of a kind this version doesn't know goes back into the file as it came
    /// ([`Account::as_written`]).
    #[serde(serialize_with = "write_accounts", deserialize_with = "read_accounts")]
    pub accounts: Vec<Account>,
    #[serde(deserialize_with = "crate::load_limit::capped_vec")]
    pub folders: Vec<Folder>,
    #[serde(deserialize_with = "crate::load_limit::capped_map")]
    pub messages: BTreeMap<Id, Arc<Message>>,
    #[serde(deserialize_with = "crate::load_limit::capped_map")]
    pub contacts: BTreeMap<Id, Arc<Contact>>,
    pub calendars: Vec<Calendar>,
    #[serde(deserialize_with = "crate::load_limit::capped_map")]
    pub events: BTreeMap<Id, Arc<Event>>,
    #[serde(deserialize_with = "crate::load_limit::capped_map")]
    pub tasks: BTreeMap<Id, Arc<Task>>,
    #[serde(deserialize_with = "crate::load_limit::capped_vec")]
    pub categories: Vec<Category>,
    pub rules: Vec<Rule>,
    pub signatures: Vec<Signature>,
    /// Moves and deletions the mail server is yet to be told, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub pending: Vec<Pending>,
    /// Changes made here to events of calendars that a server keeps, which that server is yet
    /// to be told, oldest first ([`crate::remote::EventChange`]).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub event_changes: Vec<crate::remote::EventChange>,
    /// Changes to events that a server said it took and that no download has shown yet
    /// ([`crate::remote::EventCheck`]).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub event_checks: Vec<crate::remote::EventCheck>,
    /// The Favorites of the accounts whose server has a list of them ([`ServerFavorites`]), one
    /// entry an account at most ([`Mailbox::server_favorites`]). Nothing is written of it until
    /// a server's list has been taken in.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub favorites_from_server: Vec<ServerFavorites>,
    /// Changes to what a Microsoft account's server has that its next run is to tell it and that
    /// can't be worked out by comparing ([`Queued`]), oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub queued: Vec<Queued>,
    /// The names on Outlook's list of categories, as a Microsoft account's server last gave it
    /// (R3 spec S18): their colours are Outlook's. Empty until such a list was read.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub outlook_categories: Vec<String>,
    /// The time zone the wall-clock times of the events are written in ([`Event::start`]): the
    /// zone the calendar was shown in when they were written, by its name in the IANA database
    /// (`Europe/Berlin`) or as an offset (`UTC+05:30`). Empty in a mailbox written before zones
    /// were kept, and when the system couldn't say which zone it is in: the times are then
    /// taken as this computer's, as they always were. Whoever shows the calendar in another
    /// zone has the times rewritten first ([`Mailbox::rebase`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// The messages of Microsoft accounts that held a character that couldn't be read (U+FFFD)
    /// were looked for once, and marked to be fetched again (#185): mail fetched before character
    /// sets beyond UTF-8 and Windows-1252 were read was kept with U+FFFD in their place. Looked
    /// for once only, so that a message whose own bytes are broken isn't fetched at every start.
    #[serde(skip_serializing_if = "is_false")]
    pub redecoded: bool,
    next_id: Id,
}

impl Default for Mailbox {
    fn default() -> Self {
        Mailbox {
            version: 1,
            accounts: Vec::new(),
            folders: Vec::new(),
            messages: BTreeMap::new(),
            contacts: BTreeMap::new(),
            calendars: Vec::new(),
            events: BTreeMap::new(),
            tasks: BTreeMap::new(),
            categories: default_categories(),
            rules: Vec::new(),
            signatures: Vec::new(),
            pending: Vec::new(),
            event_changes: Vec::new(),
            event_checks: Vec::new(),
            favorites_from_server: Vec::new(),
            queued: Vec::new(),
            outlook_categories: Vec::new(),
            zone: String::new(),
            redecoded: false,
            next_id: 1,
        }
    }
}

/// Why nothing from elsewhere goes into a Microsoft account's folders yet (R3 spec S19).
pub const INTO_GRAPH: &str = "Mail can't be moved or copied into a Microsoft account from another account or a local folder yet: this version can't put a message on Microsoft's server";

/// Why mail doesn't move or copy between a shared mailbox and a mailbox of another account's
/// (#70): it goes between a shared mailbox and its owner's mailbox, or another shared mailbox of
/// the same owner's, whose sign-in reaches both.
pub const BETWEEN_MAILBOXES: &str =
    "Mail moves and copies between a shared mailbox and its own account's mailboxes only: move it there, or into a folder on this computer";

/// Why received mail in a local folder (imported mail, say) doesn't go into a Microsoft
/// account's folders (R3 spec S30): what Graph makes there is always a draft.
pub const STAYS_LOCAL: &str = "Microsoft doesn't let a mail program put received mail into your mailbox; it stays in a local folder.";

/// The local folder that mail imported for a Microsoft account goes to (R3 spec S30).
pub const IMPORTED: &str = "Imported";

/// Why nothing is sent from a JMAP account ([`AccountKind::sends_mail`]): its sending comes after
/// its reading. Until then nothing goes into its Outbox, where it would wait for ever, and nothing
/// is filed as sent that wasn't.
pub const NOT_SENDING_YET: &str = "MailCraft doesn't send mail from a JMAP account yet";

fn not_found(what: &'static str, id: Id) -> ModelError {
    ModelError::NotFound(what, id)
}

fn invalid(msg: impl Into<String>) -> ModelError {
    ModelError::Invalid(msg.into())
}

impl Mailbox {
    /// An empty mailbox with one local account, its standard folders and a calendar.
    pub fn with_local_account(display_name: &str, email: &str) -> Mailbox {
        let mut mb = Mailbox::default();
        mb.add_account(Account { name: email.to_string(), email: email.to_string(), display_name: display_name.to_string(), ..Default::default() });
        mb
    }

    /// Go on giving ids from where `other` has got to, so that none it gave is given again (an
    /// older mailbox brought back by Undo, beside mail that came since).
    pub fn ids_after(&mut self, other: &Mailbox) {
        self.next_id = self.next_id.max(other.next_id);
    }

    pub fn alloc_id(&mut self) -> Id {
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1);
        id
    }

    // ---- accounts ----

    /// Add an account with its standard folders; the first account also gets the default calendar.
    pub fn add_account(&mut self, mut account: Account) -> Id {
        account.id = self.alloc_id();
        let id = account.id;
        if account.name.trim().is_empty() {
            account.name = if account.email.trim().is_empty() { "Mailbox".into() } else { account.email.clone() };
        }
        self.accounts.push(account);
        for kind in FolderKind::STANDARD {
            let fid = self.alloc_id();
            self.folders.push(Folder {
                id: fid,
                account: id,
                name: kind.default_name().into(),
                kind,
                favorite: kind == FolderKind::Inbox,
                ..Default::default()
            });
        }
        if self.calendars.is_empty() {
            let cid = self.alloc_id();
            self.calendars.push(Calendar { id: cid, name: "Calendar".into(), color: Rgb(0x2F, 0x6F, 0xD0), visible: true, ..Default::default() });
        }
        id
    }

    pub fn account(&self, id: Id) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn account_mut(&mut self, id: Id) -> Result<&mut Account, ModelError> {
        self.accounts.iter_mut().find(|a| a.id == id).ok_or(not_found("account", id))
    }

    /// The placeholder of an empty mailbox, when that is what it has: its only account, with no
    /// address and no mail. A first start makes one; the first account added takes its place, and
    /// removing the only account leaves a fresh one (#313).
    pub fn placeholder(&self) -> Option<Id> {
        match self.accounts.as_slice() {
            [only] if only.email.is_empty() && self.messages.is_empty() => Some(only.id),
            _ => None,
        }
    }

    /// The account new mail is written from: the first one.
    pub fn default_account(&self) -> Option<&Account> {
        self.accounts.first()
    }

    /// Remove an account with its folders and messages, and the shared mailboxes opened through
    /// it with theirs. Its server isn't told anything: what was still waiting to be pushed there
    /// is dropped.
    pub fn remove_account(&mut self, id: Id) -> Result<(), ModelError> {
        if self.account(id).is_none() {
            return Err(not_found("account", id));
        }
        let going: HashSet<Id> = self.accounts.iter().filter(|a| a.id == id || a.shared_of == Some(id)).map(|a| a.id).collect();
        self.accounts.retain(|a| !going.contains(&a.id));
        self.folders.retain(|f| !going.contains(&f.account));
        self.messages.retain(|_, m| !going.contains(&m.account));
        self.pending.retain(|p| !going.contains(&p.account()));
        self.event_changes.retain(|change| !going.contains(&change.account));
        self.event_checks.retain(|check| !going.contains(&check.account));
        self.queued.retain(|queued| !going.contains(&queued.account()));
        self.favorites_from_server.retain(|list| !going.contains(&list.account));
        Ok(())
    }

    // ---- shared mailboxes ----

    /// The shared mailboxes opened through `account` ([`Account::shared_of`]), in the order they
    /// were added.
    pub fn shared_mailboxes(&self, account: Id) -> Vec<&Account> {
        self.accounts.iter().filter(|a| a.shared_of == Some(account) && a.id != account).collect()
    }

    /// The account whose sign-in reads the shared mailbox `account`: a Microsoft account that is
    /// no shared mailbox itself. `None` for an account that isn't a shared mailbox, and for one
    /// whose owner is gone or can't be one (a file may say anything).
    /// The **mail to keep offline** of `account`: its own, and for a shared mailbox its owner's
    /// (#143: it has none of its own). All for an account that isn't here.
    pub fn keep_offline_of(&self, account: Id) -> KeepOffline {
        self.owner_of(account).or(self.account(account)).map(|a| a.keep_offline).unwrap_or_default()
    }

    pub fn owner_of(&self, account: Id) -> Option<&Account> {
        let owner = self.account(account)?.shared_of.filter(|owner| *owner != account)?;
        self.account(owner).filter(|o| o.kind == AccountKind::Microsoft && !o.is_shared())
    }

    /// The accounts in the order the folder pane lists them: each account of its own, followed
    /// by the shared mailboxes opened through it. A shared mailbox whose owner is gone comes
    /// last, as it is (it can't be read, and says so when it is).
    pub fn accounts_in_order(&self) -> Vec<&Account> {
        let mut shared: HashMap<Id, Vec<&Account>> = HashMap::new();
        for a in self.accounts.iter().filter(|a| a.shared_of.is_some_and(|o| o != a.id)) {
            if let Some(owner) = a.shared_of {
                shared.entry(owner).or_default().push(a);
            }
        }
        let mut out: Vec<&Account> = Vec::with_capacity(self.accounts.len());
        for a in self.accounts.iter().filter(|a| !a.is_shared()) {
            out.push(a);
            out.extend(shared.get(&a.id).into_iter().flatten().filter(|s| self.owner_of(s.id).is_some()));
        }
        // (A shared mailbox whose owner is missing.)
        let placed: HashSet<Id> = out.iter().map(|a| a.id).collect();
        out.extend(self.accounts.iter().filter(|a| !placed.contains(&a.id)));
        out
    }

    /// Open the shared mailbox at `address` under the Microsoft account `owner`: an account of
    /// its own kind ([`AccountKind::Microsoft`]) with its standard folders, read with the owner's
    /// sign-in ([`Account::shared_of`]). Nothing is asked of any server here: whether the owner
    /// may open it is found at its first Send/Receive. Its id.
    pub fn add_shared_mailbox(&mut self, owner: Id, address: &str) -> Result<Id, ModelError> {
        let address = crate::mailbox_address(address).ok_or_else(|| invalid("that isn't an e-mail address"))?;
        let o = self.account(owner).ok_or(not_found("account", owner))?;
        if o.is_shared() {
            return Err(invalid("a shared mailbox can't open another one: name the account it is opened through"));
        }
        if o.kind != AccountKind::Microsoft {
            return Err(invalid("shared mailboxes open under an account that is kept in step through Microsoft Graph"));
        }
        if o.oauth.as_ref().is_some_and(|oauth| oauth.directory == Directory::Personal) {
            return Err(invalid("a personal Microsoft account (Outlook.com) has no shared mailboxes: they are Microsoft 365's"));
        }
        if o.email.trim().eq_ignore_ascii_case(&address) {
            return Err(invalid("that is the account's own address: name the shared mailbox's"));
        }
        if self.accounts.iter().any(|a| a.email.trim().eq_ignore_ascii_case(&address)) {
            return Err(invalid(format!("{address} is already here")));
        }
        if self.shared_mailboxes(owner).len() >= MAX_SHARED {
            return Err(invalid(format!("an account opens {MAX_SHARED} shared mailboxes at most")));
        }
        let download_in_full = o.download_in_full;
        let shared = Account {
            name: address.clone(),
            email: address,
            kind: AccountKind::Microsoft,
            download_in_full,
            shared_of: Some(owner),
            ..Default::default()
        };
        let id = self.add_account(shared);
        // Outlook doesn't put a shared mailbox's Inbox among the Favorites by itself.
        for f in self.folders.iter_mut().filter(|f| f.account == id) {
            f.favorite = false;
        }
        Ok(id)
    }

    /// The name of the shared mailbox `account` as its server writes it, for its From line
    /// (#70): when it has none yet, the name on its own address in the mail its Sent Items
    /// brought down (Exchange writes that, not a sender from outside). Whether it was named.
    pub fn name_shared_from_sent(&mut self, account: Id) -> bool {
        let Some(a) = self.account(account).filter(|a| a.is_shared() && a.display_name.trim().is_empty()) else { return false };
        let address = a.email.trim().to_string();
        let Some(sent) = self.folder_of_kind(account, FolderKind::Sent) else { return false };
        let named = self
            .messages_in(sent)
            .filter(|m| m.from.email.trim().eq_ignore_ascii_case(&address))
            .map(|m| m.from.name.chars().map(|c| if c.is_control() { ' ' } else { c }).take(254).collect::<String>().trim().to_string())
            .find(|name| !name.is_empty() && !name.eq_ignore_ascii_case(&address));
        match (named, self.account_mut(account)) {
            (Some(name), Ok(a)) => {
                a.display_name = name;
                true
            }
            _ => false,
        }
    }

    // ---- favourites ----

    /// The folders of `account` that its server lists as Favorites, in the server's order;
    /// `None` when the mailbox has no list from that account's server. Only folders that the
    /// account has are named, each once (a file may say anything).
    pub fn server_favorites(&self, account: Id) -> Option<Vec<Id>> {
        let list = self.favorites_from_server.iter().find(|list| list.account == account)?;
        let mut folders: Vec<Id> = Vec::new();
        for id in list.folders.iter().take(MAX_SERVER_FAVORITES) {
            if !folders.contains(id) && self.folder(*id).is_some_and(|f| f.account == account) {
                folders.push(*id);
            }
        }
        Some(folders)
    }

    /// Keep the Favorites of `account` as its server lists them (`None`: its server has no list,
    /// and what was kept of one goes). The favourites marked here are not touched. Whether
    /// anything is different for it.
    pub fn set_server_favorites(&mut self, account: Id, folders: Option<Vec<Id>>) -> bool {
        let at = self.favorites_from_server.iter().position(|list| list.account == account);
        match (folders, at) {
            (None, None) => false,
            (None, Some(_)) => {
                self.favorites_from_server.retain(|list| list.account != account);
                true
            }
            (Some(mut folders), at) => {
                if self.account(account).is_none() {
                    return false;
                }
                let mut seen: Vec<Id> = Vec::new();
                folders.retain(|id| {
                    let fits = seen.len() < MAX_SERVER_FAVORITES && !seen.contains(id) && self.folder(*id).is_some_and(|f| f.account == account);
                    if fits {
                        seen.push(*id);
                    }
                    fits
                });
                match at.and_then(|at| self.favorites_from_server.get_mut(at)) {
                    Some(list) if list.folders == folders => false,
                    Some(list) => {
                        list.folders = folders;
                        true
                    }
                    None => {
                        // (One list an account: what a file had twice goes with the first.)
                        self.favorites_from_server.push(ServerFavorites { account, folders });
                        true
                    }
                }
            }
        }
    }

    // ---- folders ----

    pub fn folder(&self, id: Id) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    pub fn folder_mut(&mut self, id: Id) -> Result<&mut Folder, ModelError> {
        self.folders.iter_mut().find(|f| f.id == id).ok_or(not_found("folder", id))
    }

    /// An account's standard folder of `kind`.
    pub fn folder_of_kind(&self, account: Id, kind: FolderKind) -> Option<Id> {
        self.folders.iter().find(|f| f.account == account && f.kind == kind && f.parent.is_none()).map(|f| f.id)
    }

    /// The default account's Inbox (or any folder when the mailbox has no Inbox).
    pub fn inbox(&self) -> Option<Id> {
        self.default_account().and_then(|a| self.folder_of_kind(a.id, FolderKind::Inbox)).or_else(|| self.folders.first().map(|f| f.id))
    }

    /// Top-level folders of an account (`parent` `None`) or a folder's subfolders, standard
    /// folders first in their usual order, then the others by name.
    pub fn children(&self, account: Id, parent: Option<Id>) -> Vec<&Folder> {
        let mut v: Vec<&Folder> = self.folders.iter().filter(|f| f.account == account && f.parent == parent).collect();
        let rank = |f: &Folder| FolderKind::STANDARD.iter().position(|k| *k == f.kind).unwrap_or(usize::MAX);
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }

    /// The folder is an Outbox or lies under one: mail put there is sent by the next Send/Receive.
    pub fn is_send_queue(&self, id: Id) -> bool {
        let mut cur = Some(id);
        for _ in 0..=self.folders.len() {
            let Some(f) = cur.and_then(|c| self.folder(c)) else { return false };
            if f.kind == FolderKind::Outbox {
                return true;
            }
            cur = f.parent;
        }
        false
    }

    /// A folder and everything under it.
    pub fn subtree(&self, id: Id) -> Vec<Id> {
        let mut out = vec![id];
        let mut i = 0;
        while let Some(cur) = out.get(i).copied() {
            i += 1;
            if out.len() > self.folders.len() {
                break;
            }
            out.extend(self.folders.iter().filter(|f| f.parent == Some(cur) && !out.contains(&f.id)).map(|f| f.id).collect::<Vec<_>>());
        }
        out
    }

    /// Whether `id` is a Junk E-mail folder or lies anywhere under one.
    pub fn in_junk(&self, id: Id) -> bool {
        let mut cur = Some(id);
        let mut steps = 0;
        while let Some(f) = cur.and_then(|c| self.folder(c)) {
            if f.kind == FolderKind::Junk {
                return true;
            }
            cur = f.parent;
            steps += 1;
            if steps > MAX_DEPTH {
                break;
            }
        }
        false
    }

    /// `Inbox/Projects/Apollo`.
    pub fn folder_path(&self, id: Id) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(f) = cur.and_then(|c| self.folder(c)) {
            parts.push(f.name.as_str());
            cur = f.parent;
            if parts.len() > MAX_DEPTH {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    }

    /// How many folders down `id` is (1 at the top), counted by its parents: a name may hold a
    /// `/` ([`Mailbox::names_may_hold_slashes`]).
    fn depth(&self, id: Id) -> usize {
        let mut depth = 0;
        let mut cur = Some(id);
        while let Some(f) = cur.and_then(|c| self.folder(c)) {
            depth += 1;
            cur = f.parent;
            if depth > MAX_DEPTH {
                break;
            }
        }
        depth
    }

    /// Whether the names of `account`'s folders may hold `/` (and `\`): a Microsoft account's
    /// server allows them, and its folders are known by their server ids, never by a path. Any
    /// other account's folders are named by their path on an IMAP server, where `/` parts them.
    pub fn names_may_hold_slashes(&self, account: Id) -> bool {
        self.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft)
    }

    /// The folder of `account` that its server calls `id` ([`Folder::server`]).
    pub fn folder_by_server(&self, account: Id, id: &str) -> Option<Id> {
        if id.is_empty() {
            return None;
        }
        self.folders.iter().find(|f| f.account == account && f.server.as_ref().is_some_and(|s| s.id == id)).map(|f| f.id)
    }

    /// Remove exactly the folders `gone` with the mail in them: what a Microsoft account's server
    /// no longer has (R3 spec S8). A folder under one of them that isn't itself among them goes up
    /// to the top of its account rather than with it. Nothing is told to any server.
    pub fn remove_folders(&mut self, gone: &[Id]) {
        if gone.is_empty() {
            return;
        }
        let gone: HashSet<Id> = gone.iter().copied().collect();
        self.folders.retain(|f| !gone.contains(&f.id));
        for f in &mut self.folders {
            if f.parent.is_some_and(|p| gone.contains(&p)) {
                f.parent = None;
            }
        }
        self.messages.retain(|_, m| !gone.contains(&m.folder));
        for list in &mut self.favorites_from_server {
            list.folders.retain(|folder| !gone.contains(folder));
        }
        self.pending.retain(|p| !matches!(p, Pending::Move { to, .. } if gone.contains(to)));
    }

    fn name_taken(&self, account: Id, parent: Option<Id>, name: &str, except: Option<Id>) -> bool {
        self.folders
            .iter()
            .any(|f| f.account == account && f.parent == parent && Some(f.id) != except && f.name.to_lowercase() == name.to_lowercase())
    }

    /// Create a folder under `parent` (or at the account's top level).
    pub fn add_folder(&mut self, account: Id, parent: Option<Id>, name: &str) -> Result<Id, ModelError> {
        let name = clean_name(name).ok_or_else(|| invalid("a folder needs a name"))?;
        if name.contains('/') && !self.names_may_hold_slashes(account) {
            return Err(invalid("a folder name can't contain /"));
        }
        if self.account(account).is_none() {
            return Err(not_found("account", account));
        }
        if let Some(p) = parent {
            let pf = self.folder(p).ok_or(not_found("folder", p))?;
            if pf.account != account {
                return Err(invalid("the parent folder belongs to another account"));
            }
            if self.depth(p) >= MAX_DEPTH {
                return Err(invalid("folders are nested too deeply"));
            }
        }
        if self.name_taken(account, parent, &name, None) {
            return Err(invalid(format!("a folder named \"{name}\" already exists here")));
        }
        let id = self.alloc_id();
        self.folders.push(Folder { id, account, parent, name, ..Default::default() });
        Ok(id)
    }

    /// Rename a folder the user made (standard folders keep their names).
    pub fn rename_folder(&mut self, id: Id, name: &str) -> Result<(), ModelError> {
        let name = clean_name(name).ok_or_else(|| invalid("a folder needs a name"))?;
        let f = self.folder(id).ok_or(not_found("folder", id))?;
        if name.contains('/') && !self.names_may_hold_slashes(f.account) {
            return Err(invalid("a folder name can't contain /"));
        }
        if f.kind != FolderKind::Custom {
            return Err(invalid(format!("{} can't be renamed", f.name)));
        }
        if self.name_taken(f.account, f.parent, &name, Some(id)) {
            return Err(invalid(format!("a folder named \"{name}\" already exists here")));
        }
        self.folder_mut(id)?.name = name;
        Ok(())
    }

    /// Move a folder the user made under `parent` (or to the top level).
    pub fn move_folder(&mut self, id: Id, parent: Option<Id>) -> Result<(), ModelError> {
        let f = self.folder(id).ok_or(not_found("folder", id))?.clone();
        if f.kind != FolderKind::Custom {
            return Err(invalid(format!("{} can't be moved", f.name)));
        }
        if let Some(p) = parent {
            let pf = self.folder(p).ok_or(not_found("folder", p))?;
            if pf.account != f.account {
                return Err(invalid("a folder can't move to another account"));
            }
            if self.subtree(id).contains(&p) {
                return Err(invalid("a folder can't move into itself"));
            }
            if self.depth(p) + self.subtree(id).len() > MAX_DEPTH {
                return Err(invalid("folders are nested too deeply"));
            }
        }
        if self.name_taken(f.account, parent, &f.name, Some(id)) {
            return Err(invalid(format!("a folder named \"{}\" already exists there", f.name)));
        }
        self.folder_mut(id)?.parent = parent;
        Ok(())
    }

    /// Delete a folder the user made: it moves into Deleted Items with its messages; a folder
    /// already there is removed for good. Returns `true` when it was removed permanently.
    ///
    /// For an IMAP account none of this is pushed to the mail server (a mailbox there stays, and
    /// comes back as a folder with the next synchronisation); the changes still waiting to be
    /// pushed that mention a removed folder are dropped with it. For a Microsoft account the move
    /// into Deleted Items is the change its next run sends (the folder's parent differs from what
    /// the server last said), and a folder removed for good is queued for the server to delete
    /// ([`Queued::DeleteFolder`]), as Outlook does (ADR 0003).
    pub fn delete_folder(&mut self, id: Id) -> Result<bool, ModelError> {
        let f = self.folder(id).ok_or(not_found("folder", id))?.clone();
        if f.kind != FolderKind::Custom {
            return Err(invalid(format!("{} can't be deleted", f.name)));
        }
        let deleted = self.folder_of_kind(f.account, FolderKind::Deleted);
        let in_deleted = deleted.is_some_and(|d| self.subtree(d).contains(&id));
        match deleted {
            Some(d) if !in_deleted => {
                // A name already used in Deleted Items gets a number.
                let mut name = f.name.clone();
                let mut n = 1;
                while self.name_taken(f.account, Some(d), &name, Some(id)) && n < 1000 {
                    n += 1;
                    name = format!("{} ({n})", f.name);
                }
                let folder = self.folder_mut(id)?;
                folder.parent = Some(d);
                folder.name = name;
                folder.favorite = false;
                Ok(false)
            }
            _ => {
                let gone = self.subtree(id);
                let remotes: Vec<String> =
                    self.folders.iter().filter(|f| gone.contains(&f.id) && !f.remote.is_empty()).map(|f| f.remote.clone()).collect();
                // The server deletes what it has of them: each folder it has whose parent it
                // doesn't have among them (deleting that takes what is under it along).
                if self.names_may_hold_slashes(f.account) {
                    let on_server = |p: Id| gone.contains(&p) && self.folder(p).is_some_and(|parent| parent.server.is_some());
                    let deletes: Vec<Queued> = self
                        .folders
                        .iter()
                        .filter(|g| gone.contains(&g.id) && (g.id == id || !g.parent.is_some_and(on_server)))
                        .filter_map(|g| g.server.as_ref().map(|server| Queued::DeleteFolder { account: f.account, id: server.id.clone() }))
                        .collect();
                    self.queued.extend(deletes);
                }
                self.folders.retain(|f| !gone.contains(&f.id));
                self.messages.retain(|_, m| !gone.contains(&m.folder));
                for list in &mut self.favorites_from_server {
                    list.folders.retain(|folder| !gone.contains(folder));
                }
                self.pending.retain(|p| {
                    let into = matches!(p, Pending::Move { to, .. } if gone.contains(to));
                    let out_of = p.account() == f.account && remotes.iter().any(|remote| remote == p.source().0);
                    !into && !out_of
                });
                Ok(true)
            }
        }
    }

    // ---- messages ----

    /// Add a message to its folder (`folder` must exist); gives it an id when it has none.
    pub fn add_message(&mut self, mut m: Message) -> Result<Id, ModelError> {
        let folder = self.folder(m.folder).ok_or(not_found("folder", m.folder))?;
        m.account = folder.account;
        if m.id == 0 || self.messages.contains_key(&m.id) {
            m.id = self.alloc_id();
        }
        self.next_id = self.next_id.max(m.id.saturating_add(1));
        let id = m.id;
        self.messages.insert(id, Arc::new(m));
        Ok(id)
    }

    pub fn message(&self, id: Id) -> Option<&Message> {
        self.messages.get(&id).map(Arc::as_ref)
    }

    pub fn message_mut(&mut self, id: Id) -> Result<&mut Message, ModelError> {
        self.messages.get_mut(&id).map(Arc::make_mut).ok_or(not_found("message", id))
    }

    /// Message `id` has been sent: the original it answers or forwards ([`Message::answers`]) is
    /// marked replied or forwarded now, and `id` no longer waits to do so (#154). Nothing for a
    /// message that answers nothing, or whose original is gone. Whether anything changed.
    pub fn mark_answered(&mut self, id: Id) -> bool {
        let Some(answers) = self.message(id).and_then(|m| m.answers.clone()) else { return false };
        if let Ok(m) = self.message_mut(id) {
            m.answers = None;
        }
        let marked = |m: &Message| if answers.forward { m.forwarded } else { m.answered };
        let same = |m: &Message| answers.message_id.is_empty() || m.message_id == answers.message_id;
        if answers.message != id
            && self.message(answers.message).is_some_and(|orig| same(orig) && !marked(orig))
            && let Ok(orig) = self.message_mut(answers.message)
        {
            if answers.forward {
                orig.forwarded = true;
            } else {
                orig.answered = true;
            }
        }
        true
    }

    pub fn messages_in(&self, folder: Id) -> impl Iterator<Item = &Arc<Message>> {
        self.messages.values().filter(move |m| m.folder == folder)
    }

    pub fn count(&self, folder: Id) -> usize {
        self.messages_in(folder).count()
    }

    pub fn unread_count(&self, folder: Id) -> usize {
        self.messages_in(folder).filter(|m| !m.read).count()
    }

    /// The number a folder shows beside its name: unread messages, or everything for Drafts and
    /// Outbox (whatever waits there needs attention).
    pub fn badge(&self, folder: Id) -> usize {
        match self.folder(folder).map(|f| f.kind) {
            Some(FolderKind::Drafts | FolderKind::Outbox) => self.count(folder),
            Some(FolderKind::Deleted | FolderKind::Sent) | None => 0,
            Some(_) => self.unread_count(folder),
        }
    }

    /// Where `id` has a move waiting to be pushed to the server.
    fn waiting_move(&self, id: Id) -> Option<usize> {
        self.pending.iter().position(|p| matches!(p, Pending::Move { message, .. } if *message == id))
    }

    /// Move a message to another folder.
    ///
    /// A message the server has (it has a UID in a folder that follows a server mailbox) gets a
    /// [`Pending::Move`] for the next synchronisation to push; until then it has no UID in its
    /// new folder. One that already has a move waiting keeps it with the new destination, and
    /// moving it back to where the server still has it calls the move off. A message moved to
    /// another account's folder stays where it is on its own server: nothing is pushed.
    pub fn move_message(&mut self, id: Id, folder: Id) -> Result<(), ModelError> {
        let (account, remote) = {
            let f = self.folder(folder).ok_or(not_found("folder", folder))?;
            (f.account, f.remote.clone())
        };
        let m = self.message(id).ok_or(not_found("message", id))?;
        if m.folder == folder {
            return Ok(());
        }
        if self.move_across_graph(id, folder, account)? {
            return Ok(());
        }
        let m = self.message(id).ok_or(not_found("message", id))?;
        let (was_in, uid, synced) = (m.folder, m.uid, m.synced);
        let waiting = self.waiting_move(id);
        // What the message is to the server in its new folder: nothing, unless it is back.
        let (mut uid_there, mut synced_there, mut called_off) = (0, None, None);
        match waiting.and_then(|at| self.pending.get_mut(at)) {
            Some(Pending::Move { account: owner, from, uid, synced, to, .. }) => {
                if *owner == account && !remote.is_empty() && *from == remote {
                    // Back where the server has it all along.
                    (uid_there, synced_there, called_off) = (*uid, *synced, waiting);
                } else if *owner == account {
                    *to = folder;
                } else {
                    called_off = waiting;
                }
            }
            _ => {
                let from = self.folder(was_in).filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.clone());
                if let (Some(from), true) = (from, uid != 0) {
                    self.pending.push(Pending::Move { account, message: id, from, uid, synced, to: folder });
                }
            }
        }
        if let Some(at) = called_off.filter(|at| *at < self.pending.len()) {
            self.pending.remove(at);
        }
        let m = self.message_mut(id)?;
        m.folder = folder;
        m.account = account;
        // The UID belonged to the old folder.
        m.uid = uid_there;
        m.synced = synced_there;
        Ok(())
    }

    /// Whether `account` is kept in step through Microsoft Graph.
    pub fn is_graph(&self, account: Id) -> bool {
        self.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft)
    }

    /// The mailbox a sign-in reaches `account` through: its own for a Microsoft account, its
    /// owner's for a shared mailbox (#70). `None` for any other account.
    fn reached_through(&self, account: Id) -> Option<Id> {
        let a = self.account(account).filter(|a| a.kind == AccountKind::Microsoft)?;
        if a.is_shared() { self.owner_of(account).map(|o| o.id) } else { Some(a.id) }
    }

    /// Whether mail goes between the Microsoft accounts `from` and `to` as between two mailboxes
    /// of one owner's (#70): a shared mailbox and its owner's mailbox, or two shared mailboxes of
    /// the same owner's. It is made in `to` by `to`'s own run, and a move's original goes from
    /// `from` once that copy is there ([`Queued::PlaceMessage`]).
    pub fn between_mailboxes(&self, from: Id, to: Id) -> bool {
        let shared = |id: Id| self.account(id).is_some_and(Account::is_shared);
        from != to && (shared(from) || shared(to)) && self.reached_through(from).is_some_and(|owner| self.reached_through(to) == Some(owner))
    }

    /// Why `m` can't go to another mailbox (#70), when it can't: what goes there is made of the
    /// whole message, and a draft stays in its own mailbox.
    fn not_whole(m: &Message, doing: &str) -> Option<String> {
        let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(80).collect();
        if m.incomplete {
            Some(format!("Only the headers of \"{subject}\" are here, so it can't be {doing} to another mailbox"))
        } else if m.unfetched {
            Some(format!("\"{subject}\" isn't downloaded yet: open it, then try again"))
        } else if m.draft || m.server_draft.is_some() || !m.held.is_empty() {
            Some(format!("The draft \"{subject}\" stays in its own mailbox: send it from there"))
        } else {
            None
        }
    }

    /// A marker for a copy made in another mailbox: a value used once in this mailbox's life, and
    /// of the source's server id, so that copies of different messages never share one.
    fn place_marker(&mut self, id: Id, source: &str) -> String {
        let n = self.alloc_id();
        let source = crate::sync::fingerprint(source.as_bytes());
        format!("mailcraft-place-{id:x}-{n:x}-{source:016x}")
    }

    /// `id` is no longer to be made on any server for where it lay: its wait is called off, and a
    /// copy of it made already in a mailbox it is being moved on from is deleted there (the
    /// original still is where it came from).
    fn place_called_off(&mut self, id: Id) {
        self.queued.retain(|q| !matches!(q, Queued::PlaceMessage { message, .. } if *message == id));
        let Some(m) = self.message(id) else { return };
        if m.moving_out.is_none() || !self.is_graph(m.account) {
            return;
        }
        if let Some(server) = m.server.as_ref().filter(|s| !s.gone && !s.id.is_empty()) {
            let delete = Queued::DeleteMessage { account: m.account, id: server.id.clone(), kept: None };
            self.queued.push(delete);
        }
    }

    /// A move between accounts where one of them is kept in step through Microsoft Graph (R3
    /// spec S19). Whether it was one (and is done).
    ///
    /// - Out of a Microsoft account (into a local folder or another account): the message is
    ///   there at once, **being moved** ([`Message::moving_out`]); the server's copy is removed
    ///   once it is fetched whole and saved here.
    /// - Between two mailboxes of one owner's (a shared mailbox and its owner's, or two of his
    ///   shared mailboxes, #70): there at once, being moved, and waiting to be made on the
    ///   destination's server ([`Queued::PlaceMessage`]); the original is removed once that copy
    ///   is there. Refused for a message that isn't whole here, and for a draft.
    /// - Back into the account it is being moved out of: it is that server message again.
    /// - Anything else into a Microsoft account is refused, with the reason, until a whole
    ///   message can be put on its server (R3.4).
    fn move_across_graph(&mut self, id: Id, folder: Id, to_account: Id) -> Result<bool, ModelError> {
        let m = self.message(id).ok_or(not_found("message", id))?;
        let from_account = m.account;
        if from_account == to_account {
            return Ok(false);
        }
        if self.is_graph(to_account) {
            if let Some(out) = m.moving_out.clone().filter(|out| out.account == to_account) {
                // Where the server has it isn't known here when it wasn't said: the next run
                // moves it where it is now.
                let server = crate::sync::ServerMessage {
                    id: out.id,
                    folder: out.folder,
                    read: m.read,
                    flag: m.flag,
                    flag_due: m.flag_due,
                    categories: m.categories.clone(),
                    importance: m.importance,
                    ..Default::default()
                };
                self.place_called_off(id);
                let m = self.message_mut(id)?;
                (m.server, m.moving_out) = (Some(server), None);
                (m.folder, m.account, m.uid, m.synced) = (folder, to_account, 0, None);
                return Ok(true);
            }
            if !self.between_mailboxes(from_account, to_account) {
                return Err(invalid(self.refusal_into_graph(m, to_account)));
            }
            if let Some(why) = Mailbox::not_whole(m, "moved") {
                return Err(invalid(why));
            }
            let source = m.server.clone().filter(|s| !s.gone && !s.id.is_empty());
            let first = m.moving_out.is_some();
            self.place_called_off(id);
            let marker = self.place_marker(id, source.as_ref().map_or("", |s| s.id.as_str()));
            let m = self.message_mut(id)?;
            // Being moved out of where it lay first, it goes on being moved out of there.
            if let (Some(server), false) = (source, first) {
                m.moving_out = Some(crate::sync::MovingOut { account: from_account, id: server.id, folder: server.folder });
            }
            m.server = None;
            (m.folder, m.account, m.uid, m.synced) = (folder, to_account, 0, None);
            self.queued.push(Queued::PlaceMessage { account: to_account, message: id, marker, copy: false });
            return Ok(true);
        }
        if !self.is_graph(from_account) {
            return Ok(false);
        }
        let first = m.moving_out.is_some();
        let Some(server) = m.server.clone() else {
            self.place_called_off(id);
            return Ok(false);
        };
        self.place_called_off(id);
        let m = self.message_mut(id)?;
        if !server.gone && !first {
            m.moving_out = Some(crate::sync::MovingOut { account: from_account, id: server.id, folder: server.folder });
        }
        m.server = None;
        (m.folder, m.account, m.uid, m.synced) = (folder, to_account, 0, None);
        Ok(true)
    }

    /// Why `m` can't go into the Microsoft account `to` from elsewhere: received mail in a local
    /// folder never can (S30); between a shared mailbox and another account's mailbox, Microsoft
    /// reaches only the mailboxes of the one sign-in (#70); anything else not yet (S19).
    fn refusal_into_graph(&self, m: &Message, to: Id) -> &'static str {
        let local = self.account(m.account).is_some_and(|a| a.kind == AccountKind::Local);
        let shared = |id: Id| self.account(id).is_some_and(Account::is_shared);
        if local && !m.draft {
            STAYS_LOCAL
        } else if self.is_graph(m.account) && (shared(m.account) || shared(to)) {
            BETWEEN_MAILBOXES
        } else {
            INTO_GRAPH
        }
    }

    /// The first local account, made ("On this computer") when the mailbox has none.
    pub fn local_account(&mut self) -> Id {
        match self.accounts.iter().find(|a| a.kind == AccountKind::Local) {
            Some(a) => a.id,
            None => self.add_account(Account { name: "On this computer".into(), ..Default::default() }),
        }
    }

    /// Where mail imported for a Microsoft account goes, as Graph can't put received mail on
    /// its server (R3 spec S30): the folder [`IMPORTED`] at the top of the first local account
    /// (that account's Drafts, for `drafts`), made when it isn't there yet; and the local
    /// account itself ("On this computer") when the mailbox has none.
    pub fn imported_folder(&mut self, drafts: bool) -> Result<Id, ModelError> {
        let local = self.local_account();
        if drafts && let Some(folder) = self.folder_of_kind(local, FolderKind::Drafts) {
            return Ok(folder);
        }
        let there = self.folders.iter().find(|f| f.account == local && f.parent.is_none() && f.name.eq_ignore_ascii_case(IMPORTED));
        match there {
            Some(f) => Ok(f.id),
            None => self.add_folder(local, None, IMPORTED),
        }
    }

    /// Copy a message into `folder`; returns the copy's id. The copy is not on a server.
    pub fn copy_message(&mut self, id: Id, folder: Id) -> Result<Id, ModelError> {
        self.copy_message_marked(id, folder, 0)
    }

    /// [`Mailbox::copy_message`]. A copy of a message a Microsoft account's server has, into
    /// another folder of that account, waits in the queue for the server to make it
    /// ([`Queued::CopyMessage`]), with a marker that `nonce` (the time, say) helps make one that
    /// is used once. Copying from elsewhere into a Microsoft account is refused until R3.4
    /// (S19), and so is copying a message that isn't fetched yet out of one: its copy would
    /// never be whole.
    pub fn copy_message_marked(&mut self, id: Id, folder: Id, nonce: u64) -> Result<Id, ModelError> {
        let to_account = self.folder(folder).ok_or(not_found("folder", folder))?.account;
        let m = self.message(id).ok_or(not_found("message", id))?;
        let from_account = m.account;
        if self.between_mailboxes(from_account, to_account) {
            // Made on the destination's server by its own run, from the whole message (#70).
            if let Some(why) = Mailbox::not_whole(m, "copied") {
                return Err(invalid(why));
            }
            let source = m.server.as_ref().map(|s| s.id.clone()).unwrap_or_default();
            let mut copy = m.clone();
            (copy.id, copy.folder, copy.account, copy.uid, copy.synced) = (0, folder, to_account, 0, None);
            (copy.server, copy.moving_out) = (None, None);
            let new = self.add_message(copy)?;
            let marker = self.place_marker(new, &source);
            self.queued.push(Queued::PlaceMessage { account: to_account, message: new, marker, copy: true });
            return Ok(new);
        }
        if from_account != to_account && self.is_graph(to_account) {
            return Err(invalid(self.refusal_into_graph(m, to_account)));
        }
        if from_account != to_account && self.is_graph(from_account) && m.unfetched {
            let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(80).collect();
            return Err(invalid(format!("\"{subject}\" isn't downloaded yet: open it, then copy it")));
        }
        let server = m.server.clone().filter(|s| from_account == to_account && !s.gone && self.is_graph(from_account));
        let mut copy = m.clone();
        copy.id = 0;
        copy.folder = folder;
        copy.uid = 0;
        copy.synced = None;
        copy.server = None;
        copy.moving_out = None;
        let new = self.add_message(copy)?;
        if let Some(server) = server {
            let to = self.folder(folder).and_then(|f| f.server.as_ref()).map(|s| s.id.clone()).unwrap_or_default();
            // FNV-1a of the source's id: copies of different messages never share a marker.
            let source = server.id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
            let marker = format!("mailcraft-{nonce:x}-{new:x}-{source:016x}");
            self.queued.push(Queued::CopyMessage { account: from_account, id: server.id, to, copy: new, marker });
        }
        Ok(new)
    }

    /// What the server of a Microsoft account is to be told when `id` goes for good here: its
    /// copy is deleted there (and so is the server's copy of one being moved out of the
    /// account), and a copy of it not yet made there is not made.
    fn queue_graph_delete(&mut self, id: Id) {
        let Some(m) = self.message(id) else { return };
        let mut deletes = Vec::new();
        if let Some(server) = m.server.as_ref().filter(|s| !s.gone && self.is_graph(m.account)) {
            deletes.push(Queued::DeleteMessage { account: m.account, id: server.id.clone(), kept: Some(Box::new(m.clone())) });
        }
        if let Some(out) = &m.moving_out {
            deletes.push(Queued::DeleteMessage { account: out.account, id: out.id.clone(), kept: Some(Box::new(m.clone())) });
        }
        if !self.queued.is_empty() {
            self.queued.retain(|q| match q {
                Queued::CopyMessage { copy, .. } => *copy != id,
                Queued::PlaceMessage { message, .. } => *message != id,
                _ => true,
            });
        }
        for delete in deletes {
            let Queued::DeleteMessage { account, id, .. } = &delete else { continue };
            if !self.queued.iter().any(|q| matches!(q, Queued::DeleteMessage { account: a, id: i, .. } if a == account && i == id)) {
                self.queued.push(delete);
            }
        }
    }

    /// Remove a message for good, as its owner asks. When the server has it, a
    /// [`Pending::Delete`] tells the next synchronisation to delete it there too; a message with
    /// a move still waiting is deleted where the server has it.
    pub fn remove_message(&mut self, id: Id) -> Result<Message, ModelError> {
        let (was_in, account, uid, message_id) = {
            let m = self.message(id).ok_or(not_found("message", id))?;
            (m.folder, m.account, m.uid, m.message_id.clone())
        };
        self.queue_graph_delete(id);
        match self.waiting_move(id).and_then(|at| self.pending.get_mut(at)) {
            Some(op) => {
                if let Pending::Move { account, from, uid, .. } = &mut *op {
                    let delete = Pending::Delete { account: *account, from: std::mem::take(from), uid: *uid, message_id };
                    *op = delete;
                }
            }
            None => {
                let from = self.folder(was_in).filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.clone());
                if let (Some(from), true) = (from, uid != 0) {
                    self.pending.push(Pending::Delete { account, from, uid, message_id });
                }
            }
        }
        self.forget_message(id)
    }

    /// Remove a message without a word to the server: for a synchronisation taking out what the
    /// server no longer has. A move still waiting for the message is forgotten with it.
    pub fn forget_message(&mut self, id: Id) -> Result<Message, ModelError> {
        let m = self.messages.remove(&id).ok_or(not_found("message", id))?;
        for t in self.tasks.values_mut().filter(|t| t.message == Some(id)) {
            Arc::make_mut(t).message = None;
        }
        if !self.pending.is_empty() {
            self.pending.retain(|p| !matches!(p, Pending::Move { message, .. } if *message == id));
        }
        Ok(Arc::unwrap_or_clone(m))
    }

    /// Delete a message: it moves to Deleted Items, or is removed for good when it already is
    /// there (or the account has no such folder). Returns `true` when removed for good.
    pub fn delete_message(&mut self, id: Id) -> Result<bool, ModelError> {
        let m = self.message(id).ok_or(not_found("message", id))?;
        let deleted = self.folder_of_kind(m.account, FolderKind::Deleted);
        match deleted {
            Some(d) if !self.subtree(d).contains(&m.folder) => {
                self.move_message(id, d)?;
                Ok(false)
            }
            _ => {
                self.remove_message(id)?;
                Ok(true)
            }
        }
    }

    /// The messages of the conversation `id` is part of (same account, same topic), oldest first.
    pub fn conversation(&self, id: Id) -> Vec<Id> {
        let Some(m) = self.message(id) else { return Vec::new() };
        let key = m.thread_key();
        let deleted = self.folder_of_kind(m.account, FolderKind::Deleted);
        let junk = self.folder_of_kind(m.account, FolderKind::Junk);
        let mut v: Vec<&Arc<Message>> = self
            .messages
            .values()
            .filter(|x| x.account == m.account && x.thread_key() == key)
            // Deleted and junk mail stays out, unless that is where the conversation is read.
            .filter(|x| x.folder == m.folder || (Some(x.folder) != deleted && Some(x.folder) != junk))
            .collect();
        v.sort_by_key(|x| (x.date, x.id));
        v.into_iter().map(|x| x.id).collect()
    }

    /// Run the enabled rules on a message (new mail). Returns the names of the rules that ran.
    pub fn apply_rules(&mut self, id: Id) -> Vec<String> {
        let mut ran = Vec::new();
        for rule in self.rules.clone() {
            let Some(m) = self.message(id) else { break };
            if !rule.matches(m) {
                continue;
            }
            for action in &rule.actions {
                match action {
                    Action::MoveTo(folder) => {
                        // A rule pointing at a folder that is gone does nothing, and none puts
                        // mail into a send queue (the next Send/Receive would send it).
                        if !self.is_send_queue(*folder) {
                            let _ = self.move_message(id, *folder);
                        }
                    }
                    Action::Delete => {
                        let _ = self.delete_message(id);
                    }
                    change => {
                        let Ok(m) = self.message_mut(id) else { break };
                        match change {
                            Action::Categorize(c) if !m.has_category(c) => m.categories.push(c.clone()),
                            Action::Flag => m.flag = crate::FlagStatus::Flagged,
                            Action::MarkRead => m.read = true,
                            Action::SetImportance(i) => m.importance = *i,
                            _ => {}
                        }
                    }
                }
            }
            ran.push(rule.name.clone());
            if rule.stop {
                break;
            }
        }
        ran
    }

    // ---- contacts, events, tasks ----

    pub fn add_contact(&mut self, mut c: Contact) -> Id {
        if c.id == 0 || self.contacts.contains_key(&c.id) {
            c.id = self.alloc_id();
        }
        self.next_id = self.next_id.max(c.id.saturating_add(1));
        let id = c.id;
        self.contacts.insert(id, Arc::new(c));
        id
    }

    pub fn contact(&self, id: Id) -> Option<&Contact> {
        self.contacts.get(&id).map(Arc::as_ref)
    }

    pub fn contact_mut(&mut self, id: Id) -> Result<&mut Contact, ModelError> {
        self.contacts.get_mut(&id).map(Arc::make_mut).ok_or(not_found("contact", id))
    }

    pub fn remove_contact(&mut self, id: Id) -> Result<(), ModelError> {
        self.contacts.remove(&id).map(|_| ()).ok_or(not_found("contact", id))
    }

    /// The contact with this address, if any.
    pub fn contact_by_email(&self, email: &str) -> Option<&Contact> {
        self.contacts.values().map(Arc::as_ref).find(|c| c.has_email(email))
    }

    pub fn calendar(&self, id: Id) -> Option<&Calendar> {
        self.calendars.iter().find(|c| c.id == id)
    }

    pub fn default_calendar(&self) -> Option<Id> {
        self.calendars.first().map(|c| c.id)
    }

    /// Add an event; without a (known) calendar it goes to the default one.
    pub fn add_event(&mut self, mut e: Event) -> Result<Id, ModelError> {
        if self.calendar(e.calendar).is_none() {
            e.calendar = self.default_calendar().ok_or_else(|| invalid("the mailbox has no calendar"))?;
        }
        if e.id == 0 || self.events.contains_key(&e.id) {
            e.id = self.alloc_id();
        }
        self.next_id = self.next_id.max(e.id.saturating_add(1));
        let id = e.id;
        self.events.insert(id, Arc::new(e));
        Ok(id)
    }

    pub fn event(&self, id: Id) -> Option<&Event> {
        self.events.get(&id).map(Arc::as_ref)
    }

    pub fn event_mut(&mut self, id: Id) -> Result<&mut Event, ModelError> {
        self.events.get_mut(&id).map(Arc::make_mut).ok_or(not_found("event", id))
    }

    pub fn remove_event(&mut self, id: Id) -> Result<(), ModelError> {
        self.events.remove(&id).map(|_| ()).ok_or(not_found("event", id))
    }

    pub fn add_task(&mut self, mut t: Task) -> Id {
        if t.id == 0 || self.tasks.contains_key(&t.id) {
            t.id = self.alloc_id();
        }
        self.next_id = self.next_id.max(t.id.saturating_add(1));
        let id = t.id;
        self.tasks.insert(id, Arc::new(t));
        id
    }

    pub fn task(&self, id: Id) -> Option<&Task> {
        self.tasks.get(&id).map(Arc::as_ref)
    }

    pub fn task_mut(&mut self, id: Id) -> Result<&mut Task, ModelError> {
        self.tasks.get_mut(&id).map(Arc::make_mut).ok_or(not_found("task", id))
    }

    pub fn remove_task(&mut self, id: Id) -> Result<(), ModelError> {
        self.tasks.remove(&id).map(|_| ()).ok_or(not_found("task", id))
    }

    // ---- categories ----

    pub fn category(&self, name: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.name.eq_ignore_ascii_case(name.trim()))
    }

    /// A category's colour; grey for a name the list doesn't have (a category from elsewhere).
    pub fn category_color(&self, name: &str) -> Rgb {
        self.category(name).map_or(Rgb(0x8A, 0x8A, 0x8A), |c| c.color)
    }

    /// Add a category, or recolour the one with that name.
    pub fn set_category(&mut self, name: &str, color: Rgb) -> Result<(), ModelError> {
        let name = clean_name(name).ok_or_else(|| invalid("a category needs a name"))?;
        match self.categories.iter_mut().find(|c| c.name.eq_ignore_ascii_case(&name)) {
            Some(c) => c.color = color,
            None => self.categories.push(Category { name, color }),
        }
        Ok(())
    }

    /// Rename a category everywhere it is used.
    pub fn rename_category(&mut self, from: &str, to: &str) -> Result<(), ModelError> {
        let to = clean_name(to).ok_or_else(|| invalid("a category needs a name"))?;
        if !from.eq_ignore_ascii_case(&to) && self.category(&to).is_some() {
            return Err(invalid(format!("a category named \"{to}\" already exists")));
        }
        let cat = self
            .categories
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(from.trim()))
            .ok_or_else(|| invalid(format!("no category named \"{from}\"")))?;
        cat.name = to.clone();
        self.each_category_list(|list| {
            for c in list.iter_mut().filter(|c| c.eq_ignore_ascii_case(from.trim())) {
                *c = to.clone();
            }
        });
        Ok(())
    }

    /// Remove a category from the list and from every item.
    pub fn remove_category(&mut self, name: &str) -> Result<(), ModelError> {
        let before = self.categories.len();
        self.categories.retain(|c| !c.name.eq_ignore_ascii_case(name.trim()));
        if self.categories.len() == before {
            return Err(invalid(format!("no category named \"{name}\"")));
        }
        self.each_category_list(|list| list.retain(|c| !c.eq_ignore_ascii_case(name.trim())));
        Ok(())
    }

    /// Apply `f` to the category list of every item that has one (items without stay shared).
    fn each_category_list(&mut self, f: impl Fn(&mut Vec<String>)) {
        for m in self.messages.values_mut().filter(|m| !m.categories.is_empty()) {
            f(&mut Arc::make_mut(m).categories);
        }
        for c in self.contacts.values_mut().filter(|c| !c.categories.is_empty()) {
            f(&mut Arc::make_mut(c).categories);
        }
        for e in self.events.values_mut().filter(|e| !e.categories.is_empty()) {
            f(&mut Arc::make_mut(e).categories);
        }
        for t in self.tasks.values_mut().filter(|t| !t.categories.is_empty()) {
            f(&mut Arc::make_mut(t).categories);
        }
    }

    pub fn signature(&self, name: &str) -> Option<&Signature> {
        self.signatures.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()))
    }

    // ---- loading ----

    /// How many accounts the file said had their Favorites in Outlook read another way than
    /// through Microsoft Graph ([`Account::retired_favorites_choice`]), which is forgotten here:
    /// the owner is told once, and the next save writes nothing of it.
    pub fn take_retired_favorites_choices(&mut self) -> usize {
        let mut told = 0usize;
        for account in &mut self.accounts {
            told = told.saturating_add(usize::from(std::mem::take(&mut account.retired_favorites_choice)));
        }
        told
    }

    /// Make a mailbox read from a file consistent, whatever the file held: ids are unique and
    /// below `next_id`, every account has its standard folders, folder parents exist and don't
    /// loop, every message is in a folder, every event in a calendar. Returns what was fixed.
    pub fn repair(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
        if self.rules.len() > MAX_RULES {
            self.rules.truncate(MAX_RULES);
            fixed.push(format!("rules cut to {MAX_RULES}"));
        }
        // Ids stay within what JSON numbers hold exactly; anything above gets a new id, so the
        // counter can't run into the end of u64.
        let sane = |id: &Id| *id <= MAX_ID;
        let top = self
            .messages
            .keys()
            .chain(self.contacts.keys())
            .chain(self.events.keys())
            .chain(self.tasks.keys())
            .copied()
            .chain(self.accounts.iter().map(|a| a.id))
            .chain(self.folders.iter().map(|f| f.id))
            .chain(self.calendars.iter().map(|c| c.id))
            .chain(self.rules.iter().map(|r| r.id))
            .filter(sane)
            .max()
            .unwrap_or(0);
        self.next_id = if sane(&self.next_id) { self.next_id.max(top + 1) } else { top + 1 };
        /// Give items whose key is out of range a new one.
        fn rehome<T: Clone>(map: &mut BTreeMap<Id, Arc<T>>, next: &mut Id) -> usize {
            let big: Vec<Id> = map.range(MAX_ID + 1..).map(|(k, _)| *k).collect();
            for k in &big {
                if let Some(v) = map.remove(k) {
                    map.insert(*next, v);
                    *next += 1;
                }
            }
            big.len()
        }
        /// Map keys are the ids.
        fn rekey<T: Clone>(map: &mut BTreeMap<Id, Arc<T>>, id: impl Fn(&mut T) -> &mut Id) -> usize {
            let mut n = 0;
            for (k, v) in map.iter_mut() {
                if *id(Arc::make_mut(v)) != *k {
                    *id(Arc::make_mut(v)) = *k;
                    n += 1;
                }
            }
            n
        }
        let next = &mut self.next_id;
        let rehomed =
            rehome(&mut self.messages, next) + rehome(&mut self.contacts, next) + rehome(&mut self.events, next) + rehome(&mut self.tasks, next);
        let rekeyed = rekey(&mut self.messages, |m| &mut m.id)
            + rekey(&mut self.contacts, |c| &mut c.id)
            + rekey(&mut self.events, |e| &mut e.id)
            + rekey(&mut self.tasks, |t| &mut t.id);
        if rehomed + rekeyed > 0 {
            fixed.push(format!("{} item id(s) corrected", rehomed.max(rekeyed)));
        }
        // A file can't hold more accounts, folders or colour categories than a person has: the
        // rest are dropped (their messages go to the Inbox below), so the work on a hostile file
        // stays bounded.
        if self.accounts.len() > MAX_ACCOUNTS || self.folders.len() > MAX_FOLDERS || self.categories.len() > MAX_CATEGORIES {
            self.accounts.truncate(MAX_ACCOUNTS);
            self.folders.truncate(MAX_FOLDERS);
            self.categories.truncate(MAX_CATEGORIES);
            fixed.push("accounts, folders or categories beyond the limits dropped".into());
        }
        for c in &mut self.categories {
            if let Some(clean) = clean_name(&c.name)
                && clean != c.name
            {
                c.name = clean;
            }
        }
        // Accounts and folders with duplicate, missing or out-of-range ids get new ones.
        let mut seen = std::collections::BTreeSet::new();
        for c in &mut self.calendars {
            if c.id == 0 || !sane(&c.id) {
                c.id = self.next_id;
                self.next_id += 1;
            }
        }
        for a in &mut self.accounts {
            if a.id == 0 || !sane(&a.id) || !seen.insert(a.id) {
                a.id = self.next_id;
                self.next_id = self.next_id.saturating_add(1);
                seen.insert(a.id);
                fixed.push("an account id corrected".into());
            }
        }
        let accounts: Vec<Id> = self.accounts.iter().map(|a| a.id).collect();
        let known_accounts: HashSet<Id> = accounts.iter().copied().collect();
        let before = self.folders.len();
        self.folders.retain(|f| known_accounts.contains(&f.account));
        if self.folders.len() != before {
            fixed.push(format!("{} folder(s) of unknown accounts removed", before - self.folders.len()));
        }
        for f in &mut self.folders {
            if f.id == 0 || !sane(&f.id) || !seen.insert(f.id) {
                f.id = self.next_id;
                self.next_id = self.next_id.saturating_add(1);
                seen.insert(f.id);
                fixed.push("a folder id corrected".into());
            }
            match clean_name(&f.name) {
                Some(clean) => f.name = clean,
                None => f.name = f.kind.default_name().into(),
            }
        }
        // Parents must exist, be in the same account and not loop.
        let mut snapshot: HashMap<Id, (Id, Option<Id>)> = HashMap::with_capacity(self.folders.len());
        for f in &self.folders {
            snapshot.entry(f.id).or_insert((f.account, f.parent));
        }
        for f in &mut self.folders {
            let mut cur = f.parent;
            let mut steps = 0;
            let ok = loop {
                let Some(p) = cur else { break true };
                let Some(&(parent_account, grandparent)) = snapshot.get(&p) else { break false };
                steps += 1;
                if parent_account != f.account || p == f.id || steps > MAX_DEPTH {
                    break false;
                }
                cur = grandparent;
            };
            if !ok {
                f.parent = None;
                fixed.push(format!("folder \"{}\" moved to the top level", f.name));
            }
            // A nested or duplicate standard folder is an ordinary one.
            if f.kind != FolderKind::Custom && f.parent.is_some() {
                f.kind = FolderKind::Custom;
            }
        }
        // Only one folder of an account is its Inbox (and so on); a second one is an ordinary
        // folder. One pass over the folders, one lookup per account and kind.
        let mut has: HashSet<(Id, FolderKind)> = HashSet::new();
        for f in &mut self.folders {
            if f.kind != FolderKind::Custom && !has.insert((f.account, f.kind)) {
                f.kind = FolderKind::Custom;
            }
        }
        for account in accounts {
            for kind in FolderKind::STANDARD {
                if !has.contains(&(account, kind)) {
                    let id = self.alloc_id();
                    self.folders.push(Folder { id, account, name: kind.default_name().into(), kind, ..Default::default() });
                    fixed.push(format!("{} recreated", kind.default_name()));
                }
            }
        }
        // Messages without a folder go to their account's Inbox, or the first one.
        let folder_account: BTreeMap<Id, Id> = self.folders.iter().map(|f| (f.id, f.account)).collect();
        let fallback = self.inbox();
        let mut homeless = Vec::new();
        for (id, m) in &mut self.messages {
            match folder_account.get(&m.folder) {
                Some(a) if *a == m.account => {}
                Some(a) => Arc::make_mut(m).account = *a,
                None => homeless.push(*id),
            }
        }
        if !homeless.is_empty() {
            fixed.push(format!("{} message(s) without a folder moved to the Inbox", homeless.len()));
        }
        for id in homeless {
            match fallback {
                Some(f) => {
                    let _ = self.move_message(id, f);
                }
                None => {
                    self.messages.remove(&id);
                }
            }
        }
        // What is kept about the server's copies must hang together: a UID names one message of
        // its folder, and the flags the server is said to have are those of that UID.
        let mut on_server: HashSet<(Id, u32)> = HashSet::new();
        let mut untied = 0;
        for m in self.messages.values_mut() {
            let twice = m.uid != 0 && !on_server.insert((m.folder, m.uid));
            if twice || m.synced.is_some_and(|s| s.uid != m.uid) {
                let m = Arc::make_mut(m);
                if twice {
                    m.uid = 0;
                }
                m.synced = None;
                untied += 1;
            }
        }
        if untied > 0 {
            fixed.push(format!("{untied} message(s) untied from the mail server's copy"));
        }
        let dropped = self.repair_pending(&on_server);
        if dropped > 0 {
            fixed.push(format!("{dropped} change(s) waiting for the mail server dropped"));
        }
        if self.calendars.is_empty() && !self.events.is_empty() {
            let id = self.alloc_id();
            self.calendars.push(Calendar { id, name: "Calendar".into(), color: Rgb(0x2F, 0x6F, 0xD0), visible: true, ..Default::default() });
        }
        if let Some(default) = self.default_calendar() {
            let known: HashSet<Id> = self.calendars.iter().map(|c| c.id).collect();
            for e in self.events.values_mut().filter(|e| !known.contains(&e.calendar)) {
                Arc::make_mut(e).calendar = default;
            }
        }
        // The Favorites a server listed: one list an account, of folders that account has.
        let stated = std::mem::take(&mut self.favorites_from_server);
        let before: usize = stated.iter().map(|list| list.folders.len().saturating_add(1)).sum();
        for list in stated.into_iter().take(self.accounts.len().saturating_mul(2)) {
            if self.favorites_from_server.iter().all(|kept| kept.account != list.account) {
                self.set_server_favorites(list.account, Some(list.folders));
            }
        }
        let after: usize = self.favorites_from_server.iter().map(|list| list.folders.len().saturating_add(1)).sum();
        if after != before {
            fixed.push("the Favorites kept from a mail server corrected".into());
        }
        self.next_id = self.next_id.max(1);
        fixed
    }

    /// Keep of [`Mailbox::pending`] only what could have been noted here, and no more than
    /// [`MAX_PENDING`] of it: a file could otherwise have the server told to move or delete
    /// anything. `on_server` is every (folder, UID) a message has. Returns how many changes went.
    ///
    /// A change stays when it is about a mailbox one of its account's folders follows and a
    /// UID no message here still has, and no other change is about the same message; a move
    /// also needs its message to be in the folder it names, in that account, without a UID.
    fn repair_pending(&mut self, on_server: &HashSet<(Id, u32)>) -> usize {
        let before = self.pending.len();
        let mut kept: Vec<Pending> = Vec::new();
        let mut sources: HashSet<(Id, u32)> = HashSet::new();
        let mut moved: HashSet<Id> = HashSet::new();
        let mut mended = 0;
        for mut op in std::mem::take(&mut self.pending) {
            if kept.len() >= MAX_PENDING {
                break;
            }
            let account = op.account();
            let (from, uid) = op.source();
            let source = self.folders.iter().find(|f| f.account == account && !from.is_empty() && f.remote == from).map(|f| f.id);
            // UID 0: waiting for the copy it is about in a mailbox the server numbered anew, to
            // be found by its Message-ID.
            let waiting = uid == 0
                && match &op {
                    Pending::Move { message, .. } => self.messages.get(message).is_some_and(|m| !m.message_id.is_empty()),
                    Pending::Delete { message_id, .. } => !message_id.is_empty(),
                };
            let Some(source) = source.filter(|source| (uid != 0 || waiting) && !on_server.contains(&(*source, uid))) else { continue };
            if let Pending::Move { message, synced, to, .. } = &mut op {
                let fits = self.messages.get(message).is_some_and(|m| m.account == account && m.folder == *to && m.uid == 0);
                if !fits || *to == source || moved.contains(message) {
                    continue;
                }
                if synced.is_some_and(|s| s.uid != uid) {
                    *synced = None;
                    mended += 1;
                }
                moved.insert(*message);
            }
            if waiting || sources.insert((source, uid)) {
                kept.push(op);
            }
        }
        self.pending = kept;
        (before - self.pending.len()) + mended
    }
}
