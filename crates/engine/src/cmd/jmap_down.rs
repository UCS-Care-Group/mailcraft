// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a JMAP run asks about and brings (M6): what the engine says it has of each mailbox, the
//! messages the background fetches whole, a run's pieces held to sizes, and its windows taken in.
//!
//! A result is handed over by a host and may say anything, so before anything of it is taken in:
//!
//! - **Ids** are held to the JMAP alphabet, `A-Z a-z 0-9 - _`, 1 to 255 octets (RFC 8620 §1.2):
//!   a folder or a message with any other id is passed over.
//! - **Counts and lengths** are held to what a server may send: a window to [`MAX_JMAP_WINDOW`]
//!   messages, a listing to [`MAX_LISTED_FOLDERS`] mailboxes, a subject and a `Message-ID` to a
//!   line of RFC 5322 (998 characters, §2.1.1), a preview to 256 characters (RFC 8621 §4.1.4), an
//!   address to 320 (a local part of 64 octets and a domain of 255, RFC 5321 §4.5.3.1), a message
//!   fetched whole to 50 MB. Text is put on one line.
//! - **What only a Graph delta says** (where a folder's mail goes on from, what left it, what was
//!   read there, categories, importance, a follow-up's dates, a change key, a folder's own Outbox)
//!   is none of a JMAP server's: it is taken out. A JMAP server's marks are its keywords, `$seen`,
//!   `$flagged`, `$answered` and `$forwarded` (RFC 8621 §4.1.1).
//! - **A cut removes nothing.** A piece that had to be cut holds no complete listing of a mailbox
//!   any more, and neither it nor a piece that tells of trouble is the end of a run that went well:
//!   nothing is removed for what it lacks.
//!
//! What a run removes, it removes at its end, and only of the mailboxes whose whole window it took
//! in ([`sweep`]): a mark of a message gone that an earlier run left (one that went wrong after
//! reading a mailbox) is acted on only once a window of that mailbox says so again.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mailcraft_model::sync::{
    FolderListing, GraphFetchResult, GraphFolder, GraphMessage, JmapKnown, JmapMailResult, MAX_JMAP_WINDOW, MAX_LISTED_FOLDERS, MessagePage,
};
use mailcraft_model::{Address, FlagStatus, FolderKind, Id, Importance, Mailbox, Message};

use super::graph_down::{self, Down, ServerIndex};

/// Longest an id may be, in octets (RFC 8620 §1.2).
const MAX_ID: usize = 255;
/// Most windows one piece brings: a run reads a thousand mailboxes at the most, a window each.
const MAX_PAGES: usize = 1_000;
/// Lines of trouble kept of one piece, and the longest, in characters.
const MAX_LINES: usize = 32;
const MAX_LINE: usize = 400;
/// Longest subject and `Message-ID` kept, in characters.
pub(crate) const MAX_TEXT: usize = 998;
/// Longest preview kept, in characters.
pub(crate) const MAX_PREVIEW: usize = 256;
/// Longest name and address kept, in characters, and most addresses of a `To` or a `Cc`.
const MAX_NAME: usize = 256;
const MAX_ADDRESS: usize = 320;
const MAX_PEOPLE: usize = 500;
/// Longest a busy server is waited for, in seconds: a day.
pub(crate) const MAX_BUSY: u32 = 86_400;
/// Most messages of a batch fetched whole, the largest taken (Download messages in full goes up to
/// 50 MB), and the most a batch brings in all, the one message past it taken still.
pub(crate) const MAX_FETCHED: usize = 100;
pub(crate) const MAX_MESSAGE: usize = 50 << 20;
pub(crate) const MAX_BATCH: usize = 64 << 20;
/// How many of the Inbox's newest messages the background fetches whole; any other is fetched when
/// it is opened, so that what a browser tab holds stays small.
pub(crate) const BACKGROUND_NEWEST: usize = 40;

/// Whether `id` may be a JMAP id: 1 to 255 octets of `A-Z a-z 0-9 - _` (RFC 8620 §1.2).
pub(crate) fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What `account` has of each mailbox: the server ids of the messages the server last said lie in
/// it, the newest first and `most` at the most, mailbox by mailbox in the order of their ids. Only
/// the mailboxes `only` names, when it names some (a run that reads a few).
pub(crate) fn known(mb: &Mailbox, account: Id, only: Option<&[String]>, most: usize) -> Vec<JmapKnown> {
    let wanted = |mailbox: &str| only.is_none_or(|only| only.iter().any(|id| id == mailbox));
    let mut by_mailbox: HashMap<&str, Vec<(i64, Id, &str)>> = HashMap::new();
    for m in mb.messages.values().filter(|m| m.account == account) {
        let Some(server) = m.server.as_ref() else { continue };
        if is_id(&server.id) && is_id(&server.folder) && wanted(server.folder.as_str()) {
            by_mailbox.entry(server.folder.as_str()).or_default().push((m.received, m.id, server.id.as_str()));
        }
    }
    let mut known: Vec<JmapKnown> = by_mailbox
        .into_iter()
        .map(|(mailbox, mut ids)| {
            ids.sort_unstable_by_key(|&(received, id, _)| Reverse((received, id)));
            ids.truncate(most);
            JmapKnown { mailbox: mailbox.to_string(), ids: ids.into_iter().map(|(_, _, id)| id.to_string()).collect() }
        })
        .collect();
    known.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));
    known
}

/// The next messages of `account` to fetch whole in the background, by server id: of the
/// [`BACKGROUND_NEWEST`] newest in its Inbox that the server has, those that aren't here whole,
/// that the server didn't refuse this session (`passed_over`) and that are no larger than the
/// account's Download messages in full; `most` at the most, newest first.
pub(crate) fn fetch_queue(mb: &Mailbox, account: Id, most: usize, passed_over: &HashSet<(Id, String)>) -> Vec<String> {
    let Some(inbox) = mb.folder_of_kind(account, FolderKind::Inbox) else { return Vec::new() };
    let limit = mb.account(account).and_then(|a| a.download_in_full.limit());
    let mut newest: Vec<&Message> = mb
        .messages
        .values()
        .map(Arc::as_ref)
        .filter(|m| m.account == account && m.folder == inbox && m.server.as_ref().is_some_and(|s| !s.gone && is_id(&s.id)))
        .collect();
    newest.sort_unstable_by_key(|m| Reverse((m.received, m.id)));
    newest
        .into_iter()
        .take(BACKGROUND_NEWEST)
        .filter(|m| m.unfetched && !m.incomplete)
        .filter_map(|m| m.server.as_ref())
        .filter(|s| limit.is_none_or(|limit| s.size <= limit) && !passed_over.contains(&(account, s.id.clone())))
        .take(most)
        .map(|s| s.id.clone())
        .collect()
}

/// `text` on one line: words parted by one space (a control character parts them too), `most`
/// characters at the most.
pub(crate) fn line(text: &str, most: usize) -> String {
    let mut out = String::new();
    let mut length = 0usize;
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|word| !word.is_empty()) {
        if length > 0 {
            out.push(' ');
            length = length.saturating_add(1);
        }
        out.push_str(word);
        length = length.saturating_add(word.chars().count());
        if length >= most {
            break;
        }
    }
    out.chars().take(most).collect::<String>().trim_end().to_string()
}

/// An address as a server said it, on one line, or none: one whose address part holds what parts
/// addresses in a list or encloses one (`,` `;` `<` `>` `"`) would be two people, or another one,
/// once it is written into a reply.
pub(crate) fn person(a: &Address) -> Option<Address> {
    let (name, email) = (line(&a.name, MAX_NAME), line(&a.email, MAX_ADDRESS));
    if email.contains([',', ';', '<', '>', '"']) {
        return None;
    }
    (!name.is_empty() || !email.is_empty()).then_some(Address { name, email })
}

pub(crate) fn people(list: &[Address]) -> Vec<Address> {
    list.iter().take(MAX_PEOPLE).filter_map(person).collect()
}

/// The lines of trouble a piece tells of, held to their number and length.
pub(crate) fn lines(said: &[String]) -> Vec<String> {
    said.iter().take(MAX_LINES).map(|said| line(said, MAX_LINE)).filter(|said| !said.is_empty()).collect()
}

/// A message of the window of mailbox `folder` as a JMAP server says it: whether it can be taken
/// in at all (its id is one), and if so, with what isn't a JMAP server's taken out and its text
/// held to its sizes.
fn bounded_message(m: &mut GraphMessage, folder: &str) -> bool {
    if !is_id(&m.server.id) {
        return false;
    }
    let server = &mut m.server;
    server.folder = folder.to_string();
    if server.flag != FlagStatus::Flagged {
        server.flag = FlagStatus::None;
    }
    server.categories.clear();
    server.importance = Importance::Normal;
    (server.flag_start, server.flag_due, server.flag_completed) = (None, None, None);
    server.change_key.clear();
    server.gone = false;
    m.message_id = line(&m.message_id, MAX_TEXT);
    m.subject = line(&m.subject, MAX_TEXT);
    m.preview = line(&m.preview, MAX_PREVIEW);
    m.from = person(&m.from).unwrap_or_default();
    m.to = people(&m.to);
    m.cc = people(&m.cc);
    true
}

/// A window held to its sizes: whether it came whole (if not, it is no complete listing of its
/// mailbox any more).
fn bounded_page(page: &mut MessagePage) -> bool {
    // What only a Graph delta says: where the folder's mail goes on from, what left it, what was
    // read there. A window says what is in it, nothing else.
    page.link.clear();
    page.caught_up = false;
    page.counts = None;
    page.removed.clear();
    page.read.clear();
    let most = usize::try_from(MAX_JMAP_WINDOW).unwrap_or(usize::MAX);
    let mut whole = page.messages.len() <= most;
    page.messages.truncate(most);
    let (folder, before) = (page.folder.clone(), page.messages.len());
    page.messages.retain_mut(|m| bounded_message(m, &folder));
    whole &= page.messages.len() == before;
    page.afresh &= whole;
    whole
}

/// A mailbox of the listing: whether it can be taken in (its id is one), and if so as a JMAP
/// server can say it. A parent that is no id, or the mailbox itself, puts it at the top; a JMAP
/// server has no Outbox (the Outbox is this program's own), and counts pass no folder over.
fn bounded_folder(f: &mut GraphFolder) -> bool {
    if !is_id(&f.id) {
        return false;
    }
    if f.parent == f.id || !is_id(&f.parent) {
        f.parent.clear();
    }
    if f.kind == FolderKind::Outbox {
        f.kind = FolderKind::Custom;
    }
    f.counted = false;
    true
}

/// A listing held to its size: one that had to be cut, or that lists no mailbox (an account always
/// has an Inbox), is no complete listing.
fn bounded_listing(listing: &mut FolderListing) {
    let mut whole = listing.folders.len() <= MAX_LISTED_FOLDERS;
    listing.folders.truncate(MAX_LISTED_FOLDERS);
    let before = listing.folders.len();
    listing.folders.retain_mut(bounded_folder);
    whole &= listing.folders.len() == before && !listing.folders.is_empty();
    listing.complete &= whole;
}

/// A piece of a JMAP run held to its sizes, as the module documentation says. What became of
/// changes and of mail sent is taken out: nothing goes up yet, so it answers nothing that was
/// asked. A piece is the end of a run that went well (`whole`) only if it is the last, came whole,
/// and tells of no trouble.
pub(crate) fn bounded(mut result: JmapMailResult) -> JmapMailResult {
    result.changes.clear();
    result.sent.clear();
    if let Some(listing) = result.listing.as_mut() {
        bounded_listing(listing);
    }
    let mut cut = result.pages.len() > MAX_PAGES;
    result.pages.truncate(MAX_PAGES);
    let before = result.pages.len();
    result.pages.retain(|page| is_id(&page.folder));
    cut |= result.pages.len() != before;
    // Two windows of one mailbox in a piece: neither is all of it.
    let mut seen: HashSet<&str> = HashSet::new();
    cut |= !result.pages.iter().all(|page| seen.insert(page.folder.as_str()));
    for page in &mut result.pages {
        if !bounded_page(page) {
            cut = true;
        }
    }
    // (Nor does any of its windows say what its mailbox lacks.)
    if cut {
        result.pages.iter_mut().for_each(|page| page.afresh = false);
    }
    result.errors = lines(&result.errors);
    result.busy_for = result.busy_for.map(|seconds| seconds.min(MAX_BUSY));
    if let Some(progress) = result.progress.as_mut().filter(|progress| !is_id(&progress.folder)) {
        progress.folder.clear();
    }
    let trouble = !result.errors.is_empty() || result.stopped || result.auth_refused || result.busy_for.is_some();
    result.whole &= !cut && !trouble && !result.partial;
    result
}

/// What a batch of messages fetched whole brought, held to its sizes: a hundred messages, each
/// 50 MB at the most (a larger one, or an empty one, is one the server wouldn't give), 64 MB in all
/// but for the first (from the one that would take it past that, the rest waits for the next batch,
/// which names it again), and ids that are ids.
pub(crate) fn bounded_fetch(mut result: GraphFetchResult) -> GraphFetchResult {
    let mut refused = Vec::new();
    let (mut brought, mut full) = (0usize, false);
    result.fetched.truncate(MAX_FETCHED);
    result.fetched.retain(|f| {
        if !is_id(&f.id) || full {
            return false;
        }
        if f.mime.is_empty() || f.mime.len() > MAX_MESSAGE {
            refused.push(f.id.clone());
            return false;
        }
        full = brought > 0 && brought.saturating_add(f.mime.len()) > MAX_BATCH;
        brought = brought.saturating_add(f.mime.len());
        !full
    });
    for ids in [&mut result.missing, &mut result.failed] {
        ids.retain(|id| is_id(id));
        ids.truncate(MAX_FETCHED);
    }
    result.failed.extend(refused);
    result.errors = lines(&result.errors);
    result.busy_for = result.busy_for.map(|seconds| seconds.min(MAX_BUSY));
    result
}

/// Whether a window's entry says more of a message than its marks. The entry for a message the run
/// was told is here carries its marks alone; one for a message that isn't, carries what a list
/// shows of it, a size and an arrival date at least (RFC 8621 §4.1.1 gives every email both).
fn more_than_marks(there: &GraphMessage) -> bool {
    there.server.size > 0
        || there.received != 0
        || there.sent != 0
        || !there.message_id.is_empty()
        || !there.subject.is_empty()
        || !there.from.email.is_empty()
        || !there.to.is_empty()
        || !there.preview.is_empty()
}

/// Take in one window (a piece held to its sizes already, [`bounded`]) as a page of its folder's
/// messages ([`graph_down::take_page`]). An entry with marks alone says nothing of what else the
/// server said of the message: what the message has of that stays (whether it has attachments, for
/// one not fetched yet). Such an entry for a message that isn't here (removed here while the run was
/// under way) is passed over, so that no empty message is made of it; the next run, which no longer
/// says it is here, brings it whole. A window that isn't the first of its mailbox in the run
/// (`first` false) is taken as one read in part, whatever it says.
pub(crate) fn take_window(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &MessagePage, first: bool, down: &mut Down) {
    let mut window = page.clone();
    window.afresh &= first;
    window.messages.retain_mut(|there| {
        if more_than_marks(there) {
            return true;
        }
        match index.find(mb, account, &there.server.id).and_then(|id| mb.message(id)) {
            Some(here) => {
                there.has_attachments = here.attachments_on_server;
                true
            }
            None => false,
        }
    });
    graph_down::take_page(mb, index, account, &window, down);
}

/// Whether `m` is a message of `account` whose server said it is gone from a mailbox of `read`.
fn swept(m: &Message, account: Id, read: &HashSet<String>) -> bool {
    m.account == account && m.server.as_ref().is_some_and(|server| server.gone && read.contains(&server.folder))
}

/// A draft that is here whole: it may have been changed here, and nothing goes up yet, so what the
/// server does with its copy doesn't take it away.
fn draft_here(m: &Message) -> bool {
    m.draft && !m.unfetched
}

/// The end of a run that went well: what the complete windows of the mailboxes `read` left out is
/// gone from the server (deleted, or moved to a mailbox the run didn't read), and goes here too. A
/// message put in the Outbox here stays there, the owner's, as for Graph, and so does a draft that
/// is here whole ([`draft_here`]): both become this computer's own.
pub(crate) fn sweep(mb: &mut Mailbox, account: Id, read: &HashSet<String>, down: &mut Down) {
    if read.is_empty() {
        return;
    }
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    for m in mb.messages.values_mut() {
        if (Some(m.folder) == outbox || draft_here(m)) && swept(m, account, read) {
            Arc::make_mut(m).server = None;
        }
    }
    let before = mb.messages.len();
    mb.messages.retain(|_, m| !swept(m, account, read));
    down.removed = down.removed.saturating_add(before.saturating_sub(mb.messages.len()));
}

/// Most folders followed upwards from one that stays, to the ones above it that stay with it.
const MAX_DEPTH: usize = 64;

/// Before a complete listing is taken in ([`super::graph_mail::take_listing`], which removes a
/// folder whose mailbox it doesn't have, with the mail in it): such a folder stays when it is one
/// of the account's standard folders, or holds mail that only this computer has (moved or
/// imported here, or a draft that is here whole: nothing goes up yet), and so do the folders above
/// it whose mailboxes are gone too. It stays as a folder of this computer alone, without what the
/// server had in that mailbox; and what of such a mailbox lies anywhere else here (moved there
/// here), or is a draft here whole, is this computer's own from now on. How many messages went.
pub(crate) fn keep_what_is_only_here(mb: &mut Mailbox, account: Id, listing: &FolderListing) -> usize {
    if !listing.complete {
        return 0;
    }
    let listed: HashSet<&str> = listing.folders.iter().map(|f| f.id.as_str()).collect();
    let only_here: HashSet<Id> =
        mb.messages.values().filter(|m| m.account == account && (m.server.is_none() || draft_here(m))).map(|m| m.folder).collect();
    // The folders whose mailbox is gone, by the id the server gave the mailbox; and which of them stay.
    let mut dropped: HashMap<String, Id> = HashMap::new();
    let mut kept: HashSet<Id> = HashSet::new();
    for f in mb.folders.iter().filter(|f| f.account == account) {
        let Some(server) = f.server.as_ref().filter(|server| !listed.contains(server.id.as_str())) else { continue };
        dropped.insert(server.id.clone(), f.id);
        if f.kind != FolderKind::Custom || only_here.contains(&f.id) {
            kept.insert(f.id);
        }
    }
    if dropped.is_empty() {
        return 0;
    }
    let gone_too: HashSet<Id> = dropped.values().copied().collect();
    for folder in kept.clone() {
        let mut at = mb.folder(folder).and_then(|f| f.parent);
        for _ in 0..MAX_DEPTH {
            let Some(parent) = at.filter(|parent| gone_too.contains(parent)) else { break };
            kept.insert(parent);
            at = mb.folder(parent).and_then(|f| f.parent);
        }
    }
    let before = mb.messages.len();
    // In a folder that stays: what lay in its mailbox goes with it, but a draft here whole.
    mb.messages.retain(|_, m| {
        let lay_there = m.server.as_ref().is_some_and(|server| dropped.get(&server.folder) == Some(&m.folder));
        !(kept.contains(&m.folder) && m.account == account && lay_there && !draft_here(m))
    });
    let gone = before.saturating_sub(mb.messages.len());
    // What else of those mailboxes is here, and not in a folder that goes (whose mail goes with it),
    // is this computer's now.
    let doomed: HashSet<Id> = gone_too.difference(&kept).copied().collect();
    for m in mb.messages.values_mut() {
        let of_one = m.account == account && m.server.as_ref().is_some_and(|server| dropped.contains_key(&server.folder));
        if of_one && !doomed.contains(&m.folder) {
            Arc::make_mut(m).server = None;
        }
    }
    for f in mb.folders.iter_mut().filter(|f| kept.contains(&f.id)) {
        f.server = None;
    }
    gone
}
