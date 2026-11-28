// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts folders, and the changes to their contacts that a server is yet to be told (#1917).
//!
//! Every contact is kept in exactly one contacts folder: one of an account's
//! ([`ContactFolder`], in [`crate::Mailbox::contact_folders`]), or On this computer
//! ([`Contact::folder`] 0). A change made here to a contact of a folder that a server keeps is
//! made in the mailbox at once and noted in [`crate::Mailbox::contact_changes`]
//! ([`ContactChange`]): the server's copy as it was (`base`) and which fields the owner changed.
//! Only those fields are sent ([`ContactOp::fields`]), so what a server keeps and MailCraft
//! doesn't show is never written over.
//!
//! Plain data, like [`crate::remote`]: the engine says what is to go ([`ContactsUpload`],
//! `sync.contactChanges`), a host with sockets sends it and brings back what the server said
//! ([`ContactsUploaded`], `sync.applyContactChanges`). All of it is hostile: it comes from a
//! server, a mailbox file or a command's parameters, and is cut to size before it is kept.

use std::collections::BTreeSet;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::remote::{RemoteContact, key, line, mailbox};
use crate::{Account, Contact, Id, Labeled, MAX_NAME, PostalAddress};

/// Most contacts folders kept of one account.
pub const MAX_CONTACT_FOLDERS: usize = 250;
/// Most changes to contacts that may wait for servers at once. One more is refused, with a
/// reason, never dropped.
pub const MAX_CONTACT_CHANGES: usize = 5_000;
/// Most changes one [`ContactsUpload`] carries: the rest go with the next.
pub const MAX_CONTACT_OPS: usize = 50;
/// Longest delta position kept of a folder (a server's link or token).
pub const MAX_DELTA: usize = 4_096;
/// The wait after the first failure in a row, and the longest (seconds).
pub const FIRST_WAIT: i64 = 30;
pub const LONGEST_WAIT: i64 = 900;
/// Longest text kept of why a server didn't take a change.
const MAX_WHY: usize = 300;

/// Whose a contacts folder is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderOwner {
    /// The account's own.
    #[default]
    Own,
    /// A shared mailbox's that the account opens, by the mailbox's address.
    Mailbox(String),
    /// A folder a person shared with the account's owner, by that person's address.
    Person(String),
}

impl FolderOwner {
    pub fn is_own(&self) -> bool {
        *self == FolderOwner::Own
    }

    /// The address of the mailbox the folder is in; `None` for the account's own.
    pub fn address(&self) -> Option<&str> {
        match self {
            FolderOwner::Own => None,
            FolderOwner::Mailbox(a) | FolderOwner::Person(a) => Some(a),
        }
    }

    /// The same, with an address from outside that is an address; `None` when it isn't one.
    pub fn tidied(&self) -> Option<FolderOwner> {
        let clean = |a: &str| Some(mailbox(a)).filter(|a| !a.is_empty());
        match self {
            FolderOwner::Own => Some(FolderOwner::Own),
            FolderOwner::Mailbox(a) => clean(a).map(FolderOwner::Mailbox),
            FolderOwner::Person(a) => clean(a).map(FolderOwner::Person),
        }
    }
}

/// A contacts folder of an account: its own contacts (and their subfolders), a shared mailbox's,
/// or one another person shared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactFolder {
    pub id: Id,
    /// The account whose sign-in reads and writes it.
    pub account: Id,
    /// What the server calls it; empty for the main contacts folder of its owner (the account's
    /// own, or the mailbox's or person's of [`ContactFolder::owner`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    pub name: String,
    /// The folder above it here; 0 for one at the top.
    #[serde(skip_serializing_if = "is_no_id")]
    pub parent: Id,
    #[serde(skip_serializing_if = "FolderOwner::is_own")]
    pub owner: FolderOwner,
    /// The account may change its contacts. A folder it may only read refuses changes here.
    pub writable: bool,
    /// Where the server's list of changes was last read up to; empty until it was.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delta: String,
}

/// The name a main contacts folder is given until its server says another.
pub const MAIN_FOLDER_NAME: &str = "Contacts";

impl ContactFolder {
    /// The account's own main contacts folder, before a server has named it.
    pub fn main(account: Id) -> ContactFolder {
        ContactFolder { account, name: MAIN_FOLDER_NAME.into(), writable: true, ..Default::default() }
    }

    /// It is the main contacts folder of its owner.
    pub fn is_main(&self) -> bool {
        self.remote.is_empty()
    }

    /// Cut to size, as everything from outside is: `None` for one that can't be a folder (its
    /// owner's address is none, or it has an id that can't be read, which must not be taken for
    /// the main folder's none).
    pub fn tidied(mut self) -> Option<ContactFolder> {
        let had = !self.remote.is_empty();
        self.remote = key(&self.remote);
        if had && self.remote.is_empty() {
            return None;
        }
        self.name = line(&self.name, MAX_NAME);
        if self.name.is_empty() {
            self.name = MAIN_FOLDER_NAME.into();
        }
        self.owner = self.owner.tidied()?;
        self.delta = delta(&self.delta);
        Some(self)
    }
}

/// A delta position from outside: as it came, or nothing when it couldn't be one.
pub fn delta(text: &str) -> String {
    let text = text.trim();
    if text.len() > MAX_DELTA || text.chars().any(|c| c.is_control() || c.is_whitespace()) { String::new() } else { text.to_string() }
}

/// A contacts folder as a server lists it ([`crate::remote::ContactsDownload::folder`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteContactFolder {
    /// What the server calls it; empty for the main contacts folder of its owner.
    pub id: String,
    pub name: String,
    /// What the server calls the folder above it; empty for one at the top.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent: String,
    #[serde(skip_serializing_if = "FolderOwner::is_own")]
    pub owner: FolderOwner,
    pub writable: bool,
}

/// A field of a contact that a server keeps and a change may carry. What is kept here alone
/// (whether it is a favourite, its keys, its vCard UID, where it was imported from) is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContactField {
    DisplayName,
    First,
    Middle,
    Last,
    Nickname,
    Company,
    /// The job title.
    Title,
    Department,
    Emails,
    Phones,
    Addresses,
    Websites,
    Birthday,
    Notes,
    Categories,
}

impl ContactField {
    pub const ALL: [ContactField; 15] = [
        ContactField::DisplayName,
        ContactField::First,
        ContactField::Middle,
        ContactField::Last,
        ContactField::Nickname,
        ContactField::Company,
        ContactField::Title,
        ContactField::Department,
        ContactField::Emails,
        ContactField::Phones,
        ContactField::Addresses,
        ContactField::Websites,
        ContactField::Birthday,
        ContactField::Notes,
        ContactField::Categories,
    ];

    /// Whether `a` and `b` differ in this field.
    pub fn differs(self, a: &Contact, b: &Contact) -> bool {
        match self {
            ContactField::DisplayName => a.display_name != b.display_name,
            ContactField::First => a.first != b.first,
            ContactField::Middle => a.middle != b.middle,
            ContactField::Last => a.last != b.last,
            ContactField::Nickname => a.nickname != b.nickname,
            ContactField::Company => a.company != b.company,
            ContactField::Title => a.title != b.title,
            ContactField::Department => a.department != b.department,
            ContactField::Emails => a.emails != b.emails,
            ContactField::Phones => a.phones != b.phones,
            ContactField::Addresses => a.addresses != b.addresses,
            ContactField::Websites => a.websites != b.websites,
            ContactField::Birthday => a.birthday != b.birthday,
            ContactField::Notes => a.notes != b.notes,
            ContactField::Categories => a.categories != b.categories,
        }
    }

    /// Give `to` this field as `from` has it.
    pub fn copy(self, from: &Contact, to: &mut Contact) {
        match self {
            ContactField::DisplayName => to.display_name.clone_from(&from.display_name),
            ContactField::First => to.first.clone_from(&from.first),
            ContactField::Middle => to.middle.clone_from(&from.middle),
            ContactField::Last => to.last.clone_from(&from.last),
            ContactField::Nickname => to.nickname.clone_from(&from.nickname),
            ContactField::Company => to.company.clone_from(&from.company),
            ContactField::Title => to.title.clone_from(&from.title),
            ContactField::Department => to.department.clone_from(&from.department),
            ContactField::Emails => to.emails.clone_from(&from.emails),
            ContactField::Phones => to.phones.clone_from(&from.phones),
            ContactField::Addresses => to.addresses.clone_from(&from.addresses),
            ContactField::Websites => to.websites.clone_from(&from.websites),
            ContactField::Birthday => to.birthday = from.birthday,
            ContactField::Notes => to.notes.clone_from(&from.notes),
            ContactField::Categories => to.categories.clone_from(&from.categories),
        }
    }

    /// The fields `a` and `b` differ in.
    pub fn changed(a: &Contact, b: &Contact) -> BTreeSet<ContactField> {
        ContactField::ALL.into_iter().filter(|f| f.differs(a, b)).collect()
    }

    /// Changed in both places: the server's version `theirs` with the fields the owner changed
    /// here (`mine`'s `changed`). Everything else is the server's (ADR: field by field).
    pub fn merged(mine: &Contact, theirs: &Contact, changed: &BTreeSet<ContactField>) -> Contact {
        let mut out = theirs.clone();
        for field in changed {
            field.copy(mine, &mut out);
        }
        out
    }
}

/// What a waiting change does to a contact on its server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContactChangeKind {
    /// A contact made here (or put into a server's folder) that the server doesn't have yet.
    #[default]
    Create,
    /// One the server has that was changed here: [`ContactChange::fields`].
    Change,
    /// One the server has that was deleted here.
    Delete,
}

/// A change made here to a contact of a folder a server keeps, which that server is yet to be
/// told. The change is in the mailbox already; this is the note that it has to go up. One note a
/// contact at most: a second change to the same contact before the first went is the same note,
/// worked out again against the server's copy, and one taken back is none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactChange {
    /// The account whose server is to be told.
    pub account: Id,
    pub kind: ContactChangeKind,
    /// The contact here; for a deletion, the id it had.
    pub contact: Id,
    /// The contacts folder here it is in (for a deletion: was in).
    pub folder: Id,
    /// What the server calls it; empty for a new one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// For a change: the fields the owner changed (those that differ from `base`).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub fields: BTreeSet<ContactField>,
    /// For a change or a deletion: the contact as the server has it. Put back when the server
    /// refuses for good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Box<Contact>>,
    /// How many times in a row the server couldn't be told.
    #[serde(skip_serializing_if = "is_zero")]
    pub attempts: u32,
    /// Not to be tried again before this (Unix seconds); 0 for at once.
    #[serde(skip_serializing_if = "is_zero_time")]
    pub next_try: Timestamp,
    /// The server said once that the account may not do this: tried once more after the owner
    /// was asked for the permission, and given up when refused again.
    #[serde(skip_serializing_if = "is_false")]
    pub denied: bool,
    /// Why it didn't go the last time, for the owner to read.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Tried at least once (an answer said so, other than that the server took nothing): it may
    /// have reached the server whatever that answer was. Only a new contact that was sent (`base`
    /// then holds what was sent) is taken for one the server lists under a name not heard yet.
    #[serde(skip_serializing_if = "is_false")]
    pub sent: bool,
    /// For a new contact: when the server said it made it but not what it calls it (Unix
    /// seconds); 0 for not. Not sent again (that could make it twice) until a download shows it,
    /// or a whole list of its folder read after that doesn't
    /// ([`crate::remote::ContactsDownload::read_at`]).
    #[serde(skip_serializing_if = "is_zero_time")]
    pub unconfirmed: Timestamp,
    /// For a new contact that was sent: when an answer last said it was tried (Unix seconds).
    /// Only a list read after that can say the server doesn't have it.
    #[serde(skip_serializing_if = "is_zero_time")]
    pub tried: Timestamp,
}

/// How long a change waits after failing `attempts` times in a row (seconds): 30 s, doubling,
/// 15 minutes at most; longer when the server asked for that, a day at most.
pub fn wait_after(attempts: u32, asked: Option<u32>) -> i64 {
    let own = FIRST_WAIT.saturating_mul(1i64 << attempts.saturating_sub(1).min(10)).min(LONGEST_WAIT);
    own.max(i64::from(asked.unwrap_or(0)).min(86_400))
}

/// One thing a server is asked to do to a contact ([`ContactsUpload`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactOp {
    pub kind: ContactChangeKind,
    /// The contact here (for the answer to find it again).
    pub contact: Id,
    /// What the server calls it; empty for a new one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// The server's folder: what it calls it (empty: its owner's main contacts folder) and whose
    /// it is (the account's own, or the mailbox at that address).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    #[serde(skip_serializing_if = "FolderOwner::is_own")]
    pub owner: FolderOwner,
    /// The fields to write: all of them for a new contact, those the owner changed for a
    /// change, none for a deletion. A field not named is never written.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub fields: BTreeSet<ContactField>,
    /// The contact's values (its `change_key` the server's mark of the version changed).
    pub values: RemoteContact,
}

/// The changes to an account's contacts that its server is to be told now, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactsUpload {
    pub account: Id,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub ops: Vec<ContactOp>,
}

impl ContactsUpload {
    pub fn for_account(mut self, account: &Account) -> ContactsUpload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// What a server made of one [`ContactOp`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContactResult {
    /// Done.
    Done,
    /// The server no longer has the contact: a deletion is done, a change makes it again.
    Gone,
    /// Refused for good: what the server has is put back here, and the owner told.
    Refused,
    /// The account may not do this: the owner is asked for the permission once.
    Denied,
    /// Not now (offline, busy, an error of the server's): tried again later.
    #[default]
    Failed,
}

impl ContactResult {
    /// What an answer with HTTP status `status` (0: no answer) to an op of `kind` comes to.
    /// `None` for 401: the sign-in was refused, the upload stops there
    /// ([`ContactsUploaded::token_refused`]) and the op waits as it was.
    pub fn of_status(kind: ContactChangeKind, status: u16) -> Option<ContactResult> {
        Some(match status {
            200..=299 => ContactResult::Done,
            401 => return None,
            403 => ContactResult::Denied,
            404 | 410 => ContactResult::Gone,
            409 if kind == ContactChangeKind::Change => ContactResult::Refused,
            400 | 412 | 413 => ContactResult::Refused,
            _ => ContactResult::Failed,
        })
    }
}

/// What became of one [`ContactOp`], with the op itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactOutcome {
    pub op: ContactOp,
    pub result: ContactResult,
    /// What the server calls the contact now (a new one) and its mark of the version it has.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub change_key: String,
    /// The HTTP status of the answer; 0 when there was none.
    #[serde(skip_serializing_if = "is_no_status")]
    pub status: u16,
    /// Why not, in words for the owner; never anything that was sent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// How many seconds the server asked to be left alone, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

impl ContactOutcome {
    /// Cut to size, as everything from outside is.
    pub fn tidy(&mut self) {
        self.op.remote = key(&self.op.remote);
        self.op.folder = key(&self.op.folder);
        self.op.values.tidy();
        self.remote = key(&self.remote);
        self.change_key = key(&self.change_key);
        self.error = line(&self.error, MAX_WHY);
    }
}

/// What a server made of a [`ContactsUpload`]: an outcome for every op that was tried, in order.
/// An op without one wasn't tried, and waits as before.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactsUploaded {
    pub account: Id,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub outcomes: Vec<ContactOutcome>,
    /// The server stopped taking the sign-in part-way (401): a new one is needed for the rest.
    #[serde(skip_serializing_if = "is_false")]
    pub token_refused: bool,
}

impl ContactsUploaded {
    pub fn for_account(mut self, account: &Account) -> ContactsUploaded {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

fn is_no_id(id: &Id) -> bool {
    *id == 0
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_time(t: &Timestamp) -> bool {
    *t == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_no_status(s: &u16) -> bool {
    *s == 0
}

/// Where Microsoft Graph keeps a contact's lists (#1917): three e-mail addresses; one mobile, two
/// home and two business numbers; one business, one home and one other postal address; one web
/// page. Each is the position of the entry in the contact's own list (empty ones have none). What
/// has no place there isn't sent, and stays on this computer ([`keep_graph_overflow`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphPlaces {
    pub emails: Vec<usize>,
    pub mobile: Option<usize>,
    pub home_phones: Vec<usize>,
    pub business_phones: Vec<usize>,
    pub business_address: Option<usize>,
    pub home_address: Option<usize>,
    pub other_address: Option<usize>,
    pub website: Option<usize>,
}

impl GraphPlaces {
    /// Whether the phone number at `at` has a place.
    pub fn has_phone(&self, at: usize) -> bool {
        self.mobile == Some(at) || self.home_phones.contains(&at) || self.business_phones.contains(&at)
    }

    /// Whether the postal address at `at` has a place.
    pub fn has_address(&self, at: usize) -> bool {
        [self.business_address, self.home_address, self.other_address].contains(&Some(at))
    }
}

/// Graph's places for `emails`, `phones`, `addresses` and `websites`, in their order: the first
/// three addresses; the first mobile (or cell) number, the first two home ones, the first two of
/// the rest that aren't mobile; the first work (or business) address, the first home one, and the
/// first of the others; the first web page.
pub fn graph_places(emails: &[Labeled], phones: &[Labeled], addresses: &[PostalAddress], websites: &[String]) -> GraphPlaces {
    let is = |label: &str, kinds: &[&str]| kinds.iter().any(|kind| label.trim().eq_ignore_ascii_case(kind));
    let filled = |list: &[Labeled]| {
        list.iter().enumerate().filter(|(_, e)| !e.value.trim().is_empty()).map(|(at, e)| (at, e.label.clone())).collect::<Vec<_>>()
    };
    let phones = filled(phones);
    let mobile = phones.iter().find(|(_, label)| is(label, &["mobile", "cell"])).map(|(at, _)| *at);
    let home_phones = phones.iter().filter(|(_, label)| is(label, &["home"])).take(2).map(|(at, _)| *at).collect();
    let business_phones = phones.iter().filter(|(_, label)| !is(label, &["home", "mobile", "cell"])).take(2).map(|(at, _)| *at).collect();
    let business_address = addresses.iter().position(|a| is(&a.label, &["work", "business"]));
    let home_address = addresses.iter().position(|a| is(&a.label, &["home"]));
    let other_address = (0..addresses.len()).find(|at| Some(*at) != business_address && Some(*at) != home_address);
    GraphPlaces {
        emails: filled(emails).into_iter().take(3).map(|(at, _)| at).collect(),
        mobile,
        home_phones,
        business_phones,
        business_address,
        home_address,
        other_address,
        website: websites.iter().position(|w| !w.trim().is_empty()),
    }
}

/// `fresh`, a contact as Graph just sent it, given back what `old` (the same contact here) has that
/// Graph has no place for ([`graph_places`]), after the server's own: so a fourth e-mail address,
/// another mobile number or a second web page kept here isn't lost to a server that can't hold
/// it. (Those the server has already are not doubled.)
///
/// Returns the server's copy to compare it with: `fresh` as Graph sent it, with those of the
/// kept entries that still have no place. One that has a place now (the server let one go) is
/// then a change to send, so that the server holds it from now on and it isn't taken for one the
/// server deleted on the next read.
pub fn keep_graph_overflow(fresh: &mut Contact, old: &Contact) -> Contact {
    let server = fresh.clone();
    let places = graph_places(&old.emails, &old.phones, &old.addresses, &old.websites);
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    for (at, email) in old.emails.iter().enumerate() {
        if !email.value.trim().is_empty() && !places.emails.contains(&at) && !fresh.emails.iter().any(|e| same(&e.value, &email.value)) {
            fresh.emails.push(email.clone());
        }
    }
    for (at, phone) in old.phones.iter().enumerate() {
        if !phone.value.trim().is_empty() && !places.has_phone(at) && !fresh.phones.iter().any(|p| same(&p.value, &phone.value)) {
            fresh.phones.push(phone.clone());
        }
    }
    for (at, address) in old.addresses.iter().enumerate() {
        if !places.has_address(at) && !fresh.addresses.contains(address) {
            fresh.addresses.push(address.clone());
        }
    }
    for (at, site) in old.websites.iter().enumerate() {
        if !site.trim().is_empty() && places.website != Some(at) && !fresh.websites.iter().any(|w| same(w, site)) {
            fresh.websites.push(site.clone());
        }
    }
    // The server's copy: what it sent, and what was kept that has no place even now.
    let now = graph_places(&fresh.emails, &fresh.phones, &fresh.addresses, &fresh.websites);
    let mut base = server;
    for at in base.emails.len()..fresh.emails.len() {
        if let Some(e) = fresh.emails.get(at).filter(|_| !now.emails.contains(&at)) {
            base.emails.push(e.clone());
        }
    }
    for at in base.phones.len()..fresh.phones.len() {
        if let Some(p) = fresh.phones.get(at).filter(|_| !now.has_phone(at)) {
            base.phones.push(p.clone());
        }
    }
    for at in base.addresses.len()..fresh.addresses.len() {
        if let Some(a) = fresh.addresses.get(at).filter(|_| !now.has_address(at)) {
            base.addresses.push(a.clone());
        }
    }
    for at in base.websites.len()..fresh.websites.len() {
        if let Some(w) = fresh.websites.get(at).filter(|_| now.website != Some(at)) {
            base.websites.push(w.clone());
        }
    }
    base
}

/// The entries of a contact that its server has no place for, by their position in the
/// contact's own lists (#1917): they are kept, and shown, on this computer only. MailCraft never
/// limits how many a contact has; this only says which of them the server can't hold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LocalOnly {
    pub emails: Vec<usize>,
    pub phones: Vec<usize>,
    pub addresses: Vec<usize>,
    pub websites: Vec<usize>,
}

impl LocalOnly {
    pub fn is_empty(&self) -> bool {
        self.emails.is_empty() && self.phones.is_empty() && self.addresses.is_empty() && self.websites.is_empty()
    }
}

/// What of `c` its server has no place for: nothing On this computer, nor for a server that
/// keeps every entry.
pub fn local_only(mailbox: &crate::Mailbox, c: &Contact) -> LocalOnly {
    match mailbox.contact_folder(c.folder).and_then(|f| mailbox.account(f.account)) {
        // A Microsoft account: its contacts are kept through Graph, with its places.
        Some(a) if a.contacts_in_graph() => graph_local_only(c),
        // Other servers keep every entry, as far as MailCraft knows; one that doesn't adds its
        // places here.
        _ => LocalOnly::default(),
    }
}

/// What of `c`, empty entries aside, has no place among Graph's ([`graph_places`]).
fn graph_local_only(c: &Contact) -> LocalOnly {
    let places = graph_places(&c.emails, &c.phones, &c.addresses, &c.websites);
    let filled = |value: &str| !value.trim().is_empty();
    LocalOnly {
        emails: c.emails.iter().enumerate().filter(|(at, e)| filled(&e.value) && !places.emails.contains(at)).map(|(at, _)| at).collect(),
        phones: c.phones.iter().enumerate().filter(|(at, p)| filled(&p.value) && !places.has_phone(*at)).map(|(at, _)| at).collect(),
        addresses: c.addresses.iter().enumerate().filter(|(at, a)| !a.is_empty() && !places.has_address(*at)).map(|(at, _)| at).collect(),
        websites: c.websites.iter().enumerate().filter(|(at, w)| filled(w) && places.website != Some(*at)).map(|(at, _)| at).collect(),
    }
}
