//! The mailbox: everything MailCraft keeps, with the primitives that edit it.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::sync::RemoteFlags;
use crate::{Action, Calendar, Contact, Event, Id, Message, ModelError, Rgb, Rule, Task, clean_name};

/// Deepest a folder tree may nest (a hostile file could otherwise chain folders without end).
const MAX_DEPTH: usize = 32;
/// Largest id a file may hold: 2^53, the largest integer every JSON reader keeps exactly.
const MAX_ID: Id = 1 << 53;
/// Most changes waiting for the mail server that a file may hold ([`Mailbox::pending`]).
const MAX_PENDING: usize = 100_000;
/// Most changes to folders waiting for the mail server that a file may hold ([`Mailbox::folder_changes`]).
const MAX_FOLDER_CHANGES: usize = 10_000;
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
    /// How the server's mailbox stood when it was last read, for the next synchronisation to
    /// do no more than it has to. A file that holds something else in its place reads as nothing
    /// known, which costs one full read.
    #[serde(skip_serializing_if = "crate::sync::FolderMark::is_empty", deserialize_with = "crate::sync::lenient_or_default")]
    pub mark: crate::sync::FolderMark,
    /// The server's mailbox keeps no keywords of the owner's choosing (Exchange Online): the
    /// categories of its messages stay here, and the server isn't asked to keep them.
    #[serde(skip_serializing_if = "is_false")]
    pub no_keywords: bool,
    /// For an account whose mail is read through Microsoft Graph ([`MailSource::Graph`]): the
    /// address Graph gave for asking what changed in the folder since it was last read through
    /// (its `@odata.deltaLink`); empty while it hasn't been, which makes the next run read it
    /// whole. Graph's own state, not a secret.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delta: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
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
    /// The signature new messages start with (a [`Signature`] name).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
    /// Set when the owner signs in on the provider's own page (OAuth 2.0) instead of with a
    /// password. No token is ever stored here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuth>,
    /// Where the Favorites of the folder pane come from, for an account that signs in with
    /// Microsoft, once its owner has said ([`FavoritesSource`]). Nothing is written of it until
    /// then, and a file that says something else in its place reads as not said.
    #[serde(skip_serializing_if = "FavoritesSource::is_unset", deserialize_with = "crate::sync::lenient_or_default")]
    pub favorites_source: FavoritesSource,
    /// How the account's mail is read and changed on its server ([`MailSource`]): over IMAP, or,
    /// for an account that signs in with Microsoft and whose owner chose it, through Microsoft
    /// Graph. Nothing is written of it while it is IMAP, and a file that says something else in
    /// its place reads as IMAP.
    #[serde(skip_serializing_if = "MailSource::is_imap", deserialize_with = "crate::sync::lenient_or_default")]
    pub mail_source: MailSource,
    /// For a shared mailbox (support@, a team's mailbox) that its owner opens with the Full
    /// Access an administrator gave them: the owner's own account, which signs in with Microsoft.
    /// The shared mailbox has its own folders and mail here, and no sign-in of its own: it is
    /// read over IMAP with the owner's access token in the owner's name (SASL XOAUTH2 with the
    /// shared mailbox's address as the user), and mail from it goes out through SMTP signed in as
    /// the owner, which takes the Send As right ([`Account::is_shared`]). Its servers and sign-in
    /// are the owner's account's, whatever is written here.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub shared_of: Option<Id>,
}

/// How the folders an account's owner keeps under Favorites in Outlook are read from its server,
/// when they are: the owner's choice, for an account of an organisation that signs in with
/// Microsoft. Whichever it is, the list is only ever read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FavoritesSource {
    /// The owner hasn't said. It is then as it was before there was a choice: Microsoft Graph,
    /// once the owner has agreed to that.
    #[default]
    Unset,
    /// Not from the server: the Favorites marked here are the account's.
    None,
    /// Through Microsoft Graph: lasting; in an organisation an administrator may have to approve
    /// it once.
    Graph,
    /// Through Exchange Web Services: needs no administrator where the organisation still allows
    /// it, and Microsoft switches it off for everybody on 1 April 2027 ([`FavoritesSource::EWS_ENDS`]).
    Ews,
}

impl FavoritesSource {
    /// The first day on which Exchange Web Services is no longer there (Microsoft's own date for
    /// switching it off in Exchange Online): from then on it is not offered, and an account set
    /// to it is told so.
    pub const EWS_ENDS: (i32, u8, u8) = (2027, 4, 1);

    pub fn is_unset(&self) -> bool {
        *self == FavoritesSource::Unset
    }

    pub fn parse(s: &str) -> Option<FavoritesSource> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "notnow" | "own" => Some(FavoritesSource::None),
            "graph" => Some(FavoritesSource::Graph),
            "ews" => Some(FavoritesSource::Ews),
            _ => None,
        }
    }

    /// What a command calls it; `None` for a choice that was never made.
    pub fn code(self) -> Option<&'static str> {
        match self {
            FavoritesSource::Unset => None,
            FavoritesSource::None => Some("none"),
            FavoritesSource::Graph => Some("graph"),
            FavoritesSource::Ews => Some("ews"),
        }
    }
}

/// How an account's mail is read from its server and how what is done to it here goes back
/// there: the owner's choice, for an account that signs in with Microsoft
/// (`account.mailSource`). Sending goes the same way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MailSource {
    /// IMAP for reading, SMTP (or Graph, where SMTP is switched off) for sending.
    #[default]
    Imap,
    /// Microsoft Graph for both (`Mail.ReadWrite` and `Mail.Send`): for an organisation that
    /// switched IMAP off, and for what Graph keeps that IMAP doesn't (Outlook's categories).
    Graph,
}

impl MailSource {
    pub fn is_imap(&self) -> bool {
        *self == MailSource::Imap
    }

    pub fn parse(s: &str) -> Option<MailSource> {
        match s.trim().to_ascii_lowercase().as_str() {
            "imap" => Some(MailSource::Imap),
            "graph" => Some(MailSource::Graph),
            _ => None,
        }
    }

    /// What a command calls it.
    pub fn code(self) -> &'static str {
        match self {
            MailSource::Imap => "imap",
            MailSource::Graph => "graph",
        }
    }
}

/// What follows the Inbox's name in a server path whose first part is the Inbox, whatever case
/// it is written in (`Inbox/Projects` → `/Projects`); `None` for any other path.
fn inbox_rest(remote: &str) -> Option<&str> {
    let end = remote.find('/').unwrap_or(remote.len());
    let (first, rest) = remote.split_at(end);
    first.eq_ignore_ascii_case("inbox").then_some(rest)
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
    /// that is known ([`Directory`]). Written when the account is set up or its owner chooses
    /// where its Favorites come from, and not otherwise; never the directory's id.
    #[serde(skip_serializing_if = "Directory::is_unknown", deserialize_with = "crate::sync::lenient_or_default")]
    pub directory: Directory,
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
    /// The message that was `uid` in server mailbox `from` was deleted for good. `remote_id` is
    /// the id Microsoft Graph knows it by, for an account whose mail is read that way (the
    /// message is no longer here to say); empty otherwise.
    Delete {
        account: Id,
        from: String,
        uid: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        remote_id: String,
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

/// A change made here to a folder of an account with a mail server, which the server is yet to
/// be told ([`crate::sync::FolderOp`]). What the folder is to become is read off the folder
/// itself when the server is told, so that a folder renamed twice is renamed there once.
///
/// Only what the owner did is listed: what a synchronisation brings (a folder the server made,
/// renamed or no longer has) never is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderChange {
    /// The folder was made here, and is to have a mailbox on the server.
    Create { account: Id, folder: Id },
    /// The folder, which follows the server's mailbox `from`, was renamed or moved here (deleted
    /// into Deleted Items too). `parent` is the folder it lay in when that was first done: while
    /// it still lies there, the mailbox stays where it is on the server and only its name changes.
    Rename {
        account: Id,
        folder: Id,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<Id>,
    },
    /// The folder that followed the server's mailbox `remote` was deleted for good here.
    Delete { account: Id, remote: String },
}

impl FolderChange {
    pub fn account(&self) -> Id {
        match self {
            FolderChange::Create { account, .. } | FolderChange::Rename { account, .. } | FolderChange::Delete { account, .. } => *account,
        }
    }

    /// The local folder it is about; `None` for a deletion (the folder is gone).
    pub fn folder(&self) -> Option<Id> {
        match self {
            FolderChange::Create { folder, .. } | FolderChange::Rename { folder, .. } => Some(*folder),
            FolderChange::Delete { .. } => None,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Signature {
    pub name: String,
    pub text: String,
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
    pub accounts: Vec<Account>,
    pub folders: Vec<Folder>,
    pub messages: BTreeMap<Id, Arc<Message>>,
    pub contacts: BTreeMap<Id, Arc<Contact>>,
    pub calendars: Vec<Calendar>,
    pub events: BTreeMap<Id, Arc<Event>>,
    pub tasks: BTreeMap<Id, Arc<Task>>,
    pub categories: Vec<Category>,
    pub rules: Vec<Rule>,
    pub signatures: Vec<Signature>,
    /// Moves and deletions the mail server is yet to be told, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub pending: Vec<Pending>,
    /// Folders made, renamed, moved and deleted here that the mail server is yet to be told,
    /// oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub folder_changes: Vec<FolderChange>,
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
    /// The time zone the wall-clock times of the events are written in ([`Event::start`]): the
    /// zone the calendar was shown in when they were written, by its name in the IANA database
    /// (`Europe/Berlin`) or as an offset (`UTC+05:30`). Empty in a mailbox written before zones
    /// were kept, and when the system couldn't say which zone it is in: the times are then
    /// taken as this computer's, as they always were. Whoever shows the calendar in another
    /// zone has the times rewritten first ([`Mailbox::rebase`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// Saved searches the folder pane shows under Search Folders ([`crate::SearchFolder`]).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub search_folders: Vec<crate::SearchFolder>,
    /// The Quick Steps of the Home tab, in their order ([`crate::QuickStep`]).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub quick_steps: Vec<crate::QuickStep>,
    /// Contact groups ([`crate::ContactGroup`]).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub contact_groups: Vec<crate::ContactGroup>,
    /// Certificates and OpenPGP public keys: the owner's own, the people they write to, the
    /// roots they trust ([`crate::security::StoredKey`]). Never a private key.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub keys: Vec<crate::security::StoredKey>,
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
            folder_changes: Vec::new(),
            event_changes: Vec::new(),
            event_checks: Vec::new(),
            favorites_from_server: Vec::new(),
            zone: String::new(),
            search_folders: Vec::new(),
            quick_steps: Vec::new(),
            contact_groups: Vec::new(),
            keys: Vec::new(),
            next_id: 1,
        }
    }
}

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

    /// The account new mail is written from: the first one.
    pub fn default_account(&self) -> Option<&Account> {
        self.accounts.first()
    }

    /// Remove an account with its folders and messages. Its server isn't told anything: what
    /// was still waiting to be pushed there is dropped.
    ///
    /// The shared mailboxes opened through it ([`Account::shared_of`]) go with it: without it
    /// nothing signs them in.
    pub fn remove_account(&mut self, id: Id) -> Result<(), ModelError> {
        if self.account(id).is_none() {
            return Err(not_found("account", id));
        }
        let going: HashSet<Id> = self.accounts.iter().filter(|a| a.id == id || a.shared_of == Some(id)).map(|a| a.id).collect();
        self.accounts.retain(|a| !going.contains(&a.id));
        self.folders.retain(|f| !going.contains(&f.account));
        self.messages.retain(|_, m| !going.contains(&m.account));
        self.pending.retain(|p| !going.contains(&p.account()));
        self.folder_changes.retain(|c| !going.contains(&c.account()));
        self.event_changes.retain(|change| !going.contains(&change.account));
        self.event_checks.retain(|check| !going.contains(&check.account));
        self.favorites_from_server.retain(|list| !going.contains(&list.account));
        Ok(())
    }

    /// The shared mailboxes opened through `account` ([`Account::shared_of`]), in the order
    /// they were added.
    pub fn shared_mailboxes(&self, account: Id) -> Vec<&Account> {
        self.accounts.iter().filter(|a| a.shared_of == Some(account) && a.id != account).collect()
    }

    /// How the mail of `account` is read from its server: its own choice, or, for a shared
    /// mailbox, its owner's ([`MailSource`]; a shared mailbox is read with its owner's sign-in,
    /// and the way the owner chose).
    pub fn mail_source_of(&self, account: Id) -> MailSource {
        let Some(a) = self.account(account) else { return MailSource::Imap };
        match a.shared_of.filter(|owner| *owner != a.id).and_then(|owner| self.account(owner)) {
            Some(owner) => owner.mail_source,
            None => a.mail_source,
        }
    }

    /// Read the mail of `account` another way ([`MailSource`]), untying it from the server's
    /// copies: a message's number in its folder means something else each way, so every message
    /// known to be on the server keeps its place here but forgets its number there (marked as
    /// expected there, so the next download matches it by its Message-ID instead of bringing it
    /// in twice), every folder forgets how far it was read, and the changes waiting to be sent,
    /// which name messages the old way, are dropped. Folder changes still to reach the server
    /// are by name, and stay.
    pub fn switch_mail_source(&mut self, account: Id, source: MailSource) {
        if let Ok(a) = self.account_mut(account) {
            a.mail_source = source;
        }
        let mut followed = HashSet::new();
        for f in self.folders.iter_mut().filter(|f| f.account == account) {
            if !f.remote.is_empty() {
                followed.insert(f.id);
                // Both ways call the Inbox `INBOX`, and what lies in it by that name.
                if let Some(rest) = inbox_rest(&f.remote) {
                    f.remote = format!("INBOX{rest}");
                }
            }
            f.uid_validity = 0;
            f.uid_seen = 0;
            f.mark = crate::sync::FolderMark::default();
            f.no_keywords = false;
            f.delta.clear();
        }
        let ids: Vec<Id> = self.messages.values().filter(|m| m.account == account && followed.contains(&m.folder)).map(|m| m.id).collect();
        for id in ids {
            let Ok(m) = self.message_mut(id) else { continue };
            if m.uid != 0 || !m.remote_id.is_empty() {
                // Known to be on the server, its number there not known: matched when it comes down.
                let flags = m.synced.map_or_else(|| RemoteFlags::of(m, 0), |flags| flags.at(0));
                m.synced = Some(flags);
                m.uid = 0;
                m.remote_id.clear();
            }
        }
        self.pending.retain(|p| p.account() != account);
    }

    /// The accounts in the order the folder pane lists them: each one, then the shared
    /// mailboxes opened through it, as Outlook lists them. A shared mailbox whose account is not
    /// in the mailbox, or is itself a shared one (a file from elsewhere), comes in its own place.
    pub fn accounts_in_pane_order(&self) -> Vec<&Account> {
        let opens = |id: Id| self.accounts.iter().any(|a| a.id == id && !a.is_shared());
        let mut out: Vec<&Account> = Vec::with_capacity(self.accounts.len());
        for a in &self.accounts {
            match a.shared_of {
                Some(parent) if parent != a.id && opens(parent) => {}
                Some(_) => out.push(a),
                None => {
                    out.push(a);
                    out.extend(self.shared_mailboxes(a.id));
                }
            }
        }
        out
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

    fn depth(&self, id: Id) -> usize {
        self.folder_path(id).split('/').count()
    }

    fn name_taken(&self, account: Id, parent: Option<Id>, name: &str, except: Option<Id>) -> bool {
        self.folders
            .iter()
            .any(|f| f.account == account && f.parent == parent && Some(f.id) != except && f.name.to_lowercase() == name.to_lowercase())
    }

    /// Create a folder under `parent` (or at the account's top level).
    pub fn add_folder(&mut self, account: Id, parent: Option<Id>, name: &str) -> Result<Id, ModelError> {
        let name = clean_name(name).ok_or_else(|| invalid("a folder needs a name"))?;
        if name.contains('/') {
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
        if name.contains('/') {
            return Err(invalid("a folder name can't contain /"));
        }
        let f = self.folder(id).ok_or(not_found("folder", id))?;
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
    /// A move into Deleted Items reaches the server once the caller notes it
    /// ([`Mailbox::note_folder_moved`]). A folder removed for good that had mailboxes on the
    /// server has their deletion noted here ([`FolderChange::Delete`], the innermost first); the
    /// changes still waiting to be pushed that mention a removed folder are dropped with it.
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
                let mut remotes = self.remove_subtree(id);
                if self.account(f.account).is_some_and(|a| a.kind == AccountKind::Imap) {
                    // The innermost first: a server may keep the name of a mailbox that still
                    // has mailboxes inside it.
                    remotes.sort_by_key(|remote| std::cmp::Reverse(remote.chars().count()));
                    for remote in remotes {
                        if self.folder_changes.len() < MAX_FOLDER_CHANGES {
                            self.folder_changes.push(FolderChange::Delete { account: f.account, remote });
                        }
                    }
                }
                Ok(true)
            }
        }
    }

    /// Remove a folder and everything in it without a word to the server: for a synchronisation
    /// taking out what the server no longer has. Standard folders stay. The changes still
    /// waiting to be pushed that mention it are dropped with it.
    pub fn forget_folder(&mut self, id: Id) -> Result<(), ModelError> {
        let f = self.folder(id).ok_or(not_found("folder", id))?;
        if f.kind != FolderKind::Custom {
            return Err(invalid(format!("{} can't be removed", f.name)));
        }
        self.remove_subtree(id);
        Ok(())
    }

    /// Take out a folder, the folders inside it and their messages, with whatever waits to be
    /// pushed about them. Returns the server paths they followed.
    fn remove_subtree(&mut self, id: Id) -> Vec<String> {
        let account = self.folder(id).map_or(0, |f| f.account);
        let gone = self.subtree(id);
        let remotes: Vec<String> = self.folders.iter().filter(|f| gone.contains(&f.id) && !f.remote.is_empty()).map(|f| f.remote.clone()).collect();
        self.folders.retain(|f| !gone.contains(&f.id));
        let messages: Vec<Id> = self.messages.values().filter(|m| gone.contains(&m.folder)).map(|m| m.id).collect();
        for message in messages {
            let _ = self.forget_message(message);
        }
        for list in &mut self.favorites_from_server {
            list.folders.retain(|folder| !gone.contains(folder));
        }
        self.pending.retain(|p| {
            let into = matches!(p, Pending::Move { to, .. } if gone.contains(to));
            let out_of = p.account() == account && remotes.iter().any(|remote| remote == p.source().0);
            !into && !out_of
        });
        self.folder_changes.retain(|c| c.folder().is_none_or(|folder| !gone.contains(&folder)));
        remotes
    }

    /// Note that the owner made folder `id` here: an account with a mail server is to have a
    /// mailbox for it there.
    pub fn note_folder_made(&mut self, id: Id) {
        let Some(f) = self.folder(id) else { return };
        let account = f.account;
        let served = self.account(account).is_some_and(|a| a.kind == AccountKind::Imap);
        if served
            && f.remote.is_empty()
            && self.folder_changes.len() < MAX_FOLDER_CHANGES
            && !self.folder_changes.iter().any(|c| c.folder() == Some(id))
        {
            self.folder_changes.push(FolderChange::Create { account, folder: id });
        }
    }

    /// Note that the owner renamed or moved folder `id` here (into Deleted Items too), and that
    /// its mailbox on the server is to follow. `parent` is the folder it lay in before. A folder
    /// whose mailbox is still to be made is made where it is now; one that has a change waiting
    /// keeps it, and goes where it is when the server is told.
    pub fn note_folder_moved(&mut self, id: Id, parent: Option<Id>) {
        let Some(f) = self.folder(id) else { return };
        let (account, from) = (f.account, f.remote.clone());
        if from.is_empty() || f.kind != FolderKind::Custom || self.folder_changes.iter().any(|c| c.folder() == Some(id)) {
            return;
        }
        if self.account(account).is_some_and(|a| a.kind == AccountKind::Imap) && self.folder_changes.len() < MAX_FOLDER_CHANGES {
            self.folder_changes.push(FolderChange::Rename { account, folder: id, from, parent });
        }
    }

    /// The server took on a rename: what `account` knew as `from` is `to` now, and so are the
    /// mailboxes inside it (`delimiter` parts the levels). Folders, and the changes still to be
    /// pushed, name the mailboxes by their new paths.
    pub fn remote_renamed(&mut self, account: Id, from: &str, to: &str, delimiter: &str) {
        let moved = |path: &str| crate::sync::renamed_path(path, from, to, delimiter);
        if to.is_empty() {
            return;
        }
        for f in self.folders.iter_mut().filter(|f| f.account == account) {
            if let Some(path) = moved(&f.remote) {
                f.remote = path;
            }
        }
        for p in self.pending.iter_mut().filter(|p| p.account() == account) {
            let (Pending::Move { from, .. } | Pending::Delete { from, .. }) = p;
            if let Some(path) = moved(from) {
                *from = path;
            }
        }
        for c in self.folder_changes.iter_mut().filter(|c| c.account() == account) {
            match c {
                FolderChange::Rename { from, .. } | FolderChange::Delete { remote: from, .. } => {
                    if let Some(path) = moved(from) {
                        *from = path;
                    }
                }
                FolderChange::Create { .. } => {}
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
        let (was_in, uid, synced) = (m.folder, m.uid, m.synced);
        let (other_account, remote_id) = (m.account != account, m.remote_id.clone());
        // (Into an account on this device only, the server keeps its copy where it is.)
        let to_a_server = self.account(account).is_some_and(|a| a.kind == AccountKind::Imap);
        let waiting = self.waiting_move(id);
        // What the message is to the server in its new folder: nothing, unless it is back.
        let (mut uid_there, mut synced_there, mut called_off) = (0, None, None);
        // Moved to a folder of another account with a server (a shared mailbox and the account it
        // is opened with): the server it leaves deletes its copy there, and the one it goes to is given it
        // as a message of its own by the next synchronisation (an upload).
        let mut leaving = None;
        match waiting.and_then(|at| self.pending.get_mut(at)) {
            Some(Pending::Move { account: owner, from, uid, synced, to, .. }) => {
                if *owner == account && !remote.is_empty() && *from == remote {
                    // Back where the server has it all along.
                    (uid_there, synced_there, called_off) = (*uid, *synced, waiting);
                } else if *owner == account {
                    *to = folder;
                } else {
                    called_off = waiting;
                    if to_a_server {
                        leaving = Some(Pending::Delete { account: *owner, from: from.clone(), uid: *uid, remote_id: remote_id.clone() });
                    }
                }
            }
            _ => match self.folder(was_in).filter(|f| !f.remote.is_empty()).map(|f| (f.account, f.remote.clone())) {
                Some((owner, from)) if uid != 0 && owner == account => {
                    self.pending.push(Pending::Move { account, message: id, from, uid, synced, to: folder });
                }
                Some((owner, from)) if uid != 0 && to_a_server => {
                    leaving = Some(Pending::Delete { account: owner, from, uid, remote_id: remote_id.clone() });
                }
                _ => {}
            },
        }
        if let Some(at) = called_off.filter(|at| *at < self.pending.len()) {
            self.pending.remove(at);
        }
        self.pending.extend(leaving);
        let m = self.message_mut(id)?;
        m.folder = folder;
        m.account = account;
        // The UID belonged to the old folder.
        m.uid = uid_there;
        m.synced = synced_there;
        if other_account {
            // (Graph's id is the other mailbox's.)
            m.remote_id.clear();
        }
        Ok(())
    }

    /// Copy a message into `folder`; returns the copy's id. The copy is not on a server.
    pub fn copy_message(&mut self, id: Id, folder: Id) -> Result<Id, ModelError> {
        let mut copy = self.message(id).ok_or(not_found("message", id))?.clone();
        copy.id = 0;
        copy.folder = folder;
        copy.uid = 0;
        copy.synced = None;
        copy.remote_id.clear();
        self.add_message(copy)
    }

    /// Remove a message for good, as its owner asks. When the server has it, a
    /// [`Pending::Delete`] tells the next synchronisation to delete it there too; a message with
    /// a move still waiting is deleted where the server has it.
    pub fn remove_message(&mut self, id: Id) -> Result<Message, ModelError> {
        let (was_in, account, uid, remote_id) = {
            let m = self.message(id).ok_or(not_found("message", id))?;
            (m.folder, m.account, m.uid, m.remote_id.clone())
        };
        match self.waiting_move(id).and_then(|at| self.pending.get_mut(at)) {
            Some(op) => {
                if let Pending::Move { account, from, uid, .. } = &mut *op {
                    let delete = Pending::Delete { account: *account, from: std::mem::take(from), uid: *uid, remote_id: remote_id.clone() };
                    *op = delete;
                }
            }
            None => {
                let from = self.folder(was_in).filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.clone());
                if let (Some(from), true) = (from, uid != 0) {
                    self.pending.push(Pending::Delete { account, from, uid, remote_id });
                }
            }
        }
        self.forget_message(id)
    }

    /// Put `m` in the place of message `id` (a draft saved again): it keeps the id, and is a
    /// message the server doesn't have yet. The server's copy of the one it replaces, when there
    /// is one, is deleted there ([`Pending::Delete`]) once the new one has gone up: the next
    /// synchronisation puts the new one there first, then takes the old one away by its UID.
    pub fn replace_message(&mut self, id: Id, mut m: Message) -> Result<(), ModelError> {
        let (was_in, account, uid, remote_id) = {
            let old = self.message(id).ok_or(not_found("message", id))?;
            (old.folder, old.account, old.uid, old.remote_id.clone())
        };
        if self.folder(m.folder).is_none() {
            return Err(not_found("folder", m.folder));
        }
        match self.waiting_move(id).and_then(|at| self.pending.get_mut(at)) {
            // On its way somewhere on the server: the copy there goes, wherever it is now.
            Some(op) => {
                if let Pending::Move { account, from, uid, .. } = &mut *op {
                    let delete = Pending::Delete { account: *account, from: std::mem::take(from), uid: *uid, remote_id: remote_id.clone() };
                    *op = delete;
                }
            }
            None => {
                let from = self.folder(was_in).filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.clone());
                if let (Some(from), true) = (from, uid != 0) {
                    self.pending.push(Pending::Delete { account, from, uid, remote_id });
                }
            }
        }
        m.id = id;
        m.account = self.folder(m.folder).map_or(account, |f| f.account);
        (m.uid, m.synced, m.synced_categories) = (0, None, Vec::new());
        *self.message_mut(id)? = m;
        Ok(())
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
                        // A rule pointing at a folder that is gone does nothing.
                        let _ = self.move_message(id, *folder);
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

    /// Make a mailbox read from a file consistent, whatever the file held: ids are unique and
    /// below `next_id`, every account has its standard folders, folder parents exist and don't
    /// loop, every message is in a folder, every event in a calendar. Returns what was fixed.
    pub fn repair(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
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
            .chain(self.search_folders.iter().map(|f| f.id))
            .chain(self.quick_steps.iter().map(|q| q.id))
            .chain(self.contact_groups.iter().map(|g| g.id))
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
        let before = self.folders.len();
        self.folders.retain(|f| accounts.contains(&f.account));
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
            if clean_name(&f.name).is_none() {
                f.name = f.kind.default_name().into();
            }
        }
        // Parents must exist, be in the same account and not loop.
        let snapshot = self.folders.clone();
        for f in &mut self.folders {
            let mut cur = f.parent;
            let mut steps = 0;
            let ok = loop {
                let Some(p) = cur else { break true };
                let Some(pf) = snapshot.iter().find(|x| x.id == p) else { break false };
                steps += 1;
                if pf.account != f.account || p == f.id || steps > MAX_DEPTH {
                    break false;
                }
                cur = pf.parent;
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
        for account in accounts {
            for kind in FolderKind::STANDARD {
                if !self.folders.iter().any(|f| f.account == account && f.kind == kind) {
                    let id = self.alloc_id();
                    self.folders.push(Folder { id, account, name: kind.default_name().into(), kind, ..Default::default() });
                    fixed.push(format!("{} recreated", kind.default_name()));
                }
                // Only one folder is the Inbox; a second one is an ordinary folder.
                for extra in self.folders.iter_mut().filter(|f| f.account == account && f.kind == kind).skip(1) {
                    extra.kind = FolderKind::Custom;
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
        let dropped = self.repair_pending(&on_server) + self.repair_folder_changes();
        if dropped > 0 {
            fixed.push(format!("{dropped} change(s) waiting for the mail server dropped"));
        }
        if self.calendars.is_empty() && !self.events.is_empty() {
            let id = self.alloc_id();
            self.calendars.push(Calendar { id, name: "Calendar".into(), color: Rgb(0x2F, 0x6F, 0xD0), visible: true, ..Default::default() });
        }
        if let Some(default) = self.default_calendar() {
            let known: Vec<Id> = self.calendars.iter().map(|c| c.id).collect();
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
        if self.repair_organizers() > 0 {
            fixed.push("search folders, Quick Steps or contact groups corrected".into());
        }
        // A shared mailbox's mail is read the way its owner's is (a file from before the rule,
        // or written by hand): it follows, untied from the server's copies as a switch unties.
        let following: Vec<(Id, MailSource)> = self
            .accounts
            .iter()
            .filter(|a| a.shared_of.is_some_and(|owner| owner != a.id))
            .map(|a| (a.id, self.mail_source_of(a.id)))
            .filter(|(id, source)| self.account(*id).is_some_and(|a| a.mail_source != *source))
            .collect();
        for (id, source) in &following {
            self.switch_mail_source(*id, *source);
        }
        if !following.is_empty() {
            fixed.push(format!("{} shared mailbox(es) now read the way the account they open under is", following.len()));
        }
        self.next_id = self.next_id.max(1);
        fixed
    }

    /// Search folders, Quick Steps and contact groups as a file may hold them: no more than
    /// [`crate::MAX_SAVED`] of each, named, with ids of their own (a search folder's never one a
    /// folder has, since the folder in view may be either), pointing only at what exists.
    /// Returns how many things were corrected.
    fn repair_organizers(&mut self) -> usize {
        let mut n = 0;
        let mut seen: HashSet<Id> = self.folders.iter().map(|f| f.id).collect();
        let accounts: HashSet<Id> = self.accounts.iter().map(|a| a.id).collect();
        let folders: HashSet<Id> = self.folders.iter().map(|f| f.id).collect();
        for list_len in [self.search_folders.len(), self.quick_steps.len(), self.contact_groups.len()] {
            n += list_len.saturating_sub(crate::MAX_SAVED);
        }
        self.search_folders.truncate(crate::MAX_SAVED);
        self.quick_steps.truncate(crate::MAX_SAVED);
        self.contact_groups.truncate(crate::MAX_SAVED);
        // Keys: no more than so many, and one of each.
        n += self.keys.len().saturating_sub(crate::security::MAX_KEYS);
        self.keys.truncate(crate::security::MAX_KEYS);
        let mut prints = HashSet::new();
        let before = self.keys.len();
        self.keys.retain(|k| !k.fingerprint.is_empty() && prints.insert((k.scheme, k.fingerprint.clone())));
        n += before - self.keys.len();
        let mut fresh = |id: &mut Id, seen: &mut HashSet<Id>, next: &mut Id| {
            if *id == 0 || *id > MAX_ID || !seen.insert(*id) {
                *id = *next;
                *next = next.saturating_add(1);
                seen.insert(*id);
                n += 1;
            }
        };
        let next = &mut self.next_id;
        for f in &mut self.search_folders {
            fresh(&mut f.id, &mut seen, next);
        }
        for q in &mut self.quick_steps {
            fresh(&mut q.id, &mut seen, next);
        }
        for g in &mut self.contact_groups {
            fresh(&mut g.id, &mut seen, next);
        }
        for f in &mut self.search_folders {
            let name = clean_name(&f.name).unwrap_or_else(|| "Search Folder".into());
            let query = crate::SearchFolder::clean_query(&f.query);
            let account = f.account.filter(|a| accounts.contains(a));
            if (&name, &query, account) != (&f.name, &f.query, f.account) {
                (f.name, f.query, f.account) = (name, query, account);
                n += 1;
            }
        }
        for q in &mut self.quick_steps {
            let name = clean_name(&q.name).unwrap_or_else(|| "Quick Step".into());
            let before = q.actions.len();
            q.actions.truncate(crate::MAX_STEP_ACTIONS);
            q.actions.retain(|a| match a {
                crate::StepAction::MoveTo(f) | crate::StepAction::CopyTo(f) => folders.contains(f),
                crate::StepAction::Categorize(c) => clean_name(c).is_some(),
                _ => true,
            });
            if name != q.name || q.actions.len() != before {
                q.name = name;
                n += 1;
            }
        }
        for g in &mut self.contact_groups {
            let name = clean_name(&g.name).unwrap_or_else(|| "Contact Group".into());
            let before = g.members.len();
            let members = std::mem::take(&mut g.members);
            for mut m in members {
                if m.contact.is_some_and(|c| !self.contacts.contains_key(&c)) {
                    m.contact = None;
                }
                g.add(m);
            }
            if name != g.name || g.members.len() != before {
                g.name = name;
                n += 1;
            }
        }
        n
    }

    /// Let go of the folder changes that have nothing left to do: a folder made here that has its
    /// mailbox on the server by now (a message moved into it made it), a renamed one that follows
    /// no mailbox any more (the server no longer had it). Returns how many went.
    pub fn settle_folder_changes(&mut self) -> usize {
        self.repair_folder_changes()
    }

    /// Keep of [`Mailbox::folder_changes`] only what could have been noted here, one change a
    /// folder at most, and no more than [`MAX_FOLDER_CHANGES`]. Returns how many went.
    fn repair_folder_changes(&mut self) -> usize {
        let before = self.folder_changes.len();
        let mut kept: Vec<FolderChange> = Vec::new();
        for change in std::mem::take(&mut self.folder_changes) {
            if kept.len() >= MAX_FOLDER_CHANGES {
                break;
            }
            let account = change.account();
            if !self.account(account).is_some_and(|a| a.kind == AccountKind::Imap) {
                continue;
            }
            let fits = match &change {
                FolderChange::Create { folder, .. } => {
                    self.folder(*folder).is_some_and(|f| f.account == account && f.kind == FolderKind::Custom && f.remote.is_empty())
                }
                FolderChange::Rename { folder, from, parent, .. } => {
                    self.folder(*folder)
                        .is_some_and(|f| f.account == account && f.kind == FolderKind::Custom && !from.is_empty() && f.remote == *from)
                        && parent.is_none_or(|p| self.folder(p).is_some_and(|p| p.account == account))
                }
                // Not one a folder here still follows.
                FolderChange::Delete { remote, .. } => {
                    !remote.is_empty() && !self.folders.iter().any(|f| f.account == account && f.remote == *remote)
                }
            };
            let twice = change.folder().is_some_and(|folder| kept.iter().any(|c| c.folder() == Some(folder)));
            if fits && !twice && !kept.contains(&change) {
                kept.push(change);
            }
        }
        self.folder_changes = kept;
        before - self.folder_changes.len()
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
            let Some(source) = source.filter(|source| uid != 0 && !on_server.contains(&(*source, uid))) else { continue };
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
            if sources.insert((source, uid)) {
                kept.push(op);
            }
        }
        self.pending = kept;
        (before - self.pending.len()) + mended
    }
}
