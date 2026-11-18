// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail down from a Zimbra account (Z3): a run's pieces held to sizes, and its folders and pages
//! taken in.
//!
//! A result is handed over by a host and may say anything, so before anything of it is taken in
//! ([`bounded`], after [`mailcraft_model::sync::ZimbraMailResult::check`] has refused what is larger
//! than anything real):
//!
//! - **Ids** (a folder's, an item's) are opaque: held to the length the model allows and to no
//!   control character. A folder with any other id is passed over; a message or a removal with one
//!   makes its piece a cut one.
//! - **Text** is put on one line and cut to what is shown, as a JMAP or an Exchange run's is.
//! - **A cut removes nothing**, and keeps no token: the next run reads again from where this one
//!   began.
//!
//! **Taking in** goes the way of a Graph run, by server id and with the same pieces
//! ([`super::graph_down::take_page`]). A folder read from its start is one folder's page, read
//! afresh. A page of changes is the whole mailbox's: its messages are taken in folder by folder
//! (a message that moved is found by its id, which Zimbra keeps across folders), and a message it
//! says was deleted is marked gone, to be removed when the run ends well
//! ([`super::graph_down::sweep`]).

use std::collections::BTreeMap;

use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMessage, MessagePage, ZimbraMailResult, ZimbraMessagePage, zimbra_id_ok};
use mailcraft_model::{FolderKind, Id, Mailbox};

use super::graph_down::{self, Down, ServerIndex};
use super::jmap_down::{MAX_BUSY, MAX_PREVIEW, MAX_TEXT, line, lines, people, person};

/// Longest name of a folder kept, in characters.
const MAX_NAME: usize = 255;
/// Most pages one piece brings.
const MAX_PAGES: usize = 1_000;

/// A message of a page: whether it can be taken in at all (its ids are ids), and if so with its
/// text held to its sizes.
fn bounded_message(m: &mut GraphMessage) -> bool {
    if !zimbra_id_ok(&m.server.id) || !zimbra_id_ok(&m.server.folder) {
        return false;
    }
    let server = &mut m.server;
    server.gone = false;
    server.thread = mailcraft_model::sync::thread_id(&server.thread);
    (server.thread_asked, server.thread_tries) = (false, 0);
    server.change_key.clear();
    server.categories.truncate(50);
    server.categories = server.categories.iter().map(|c| line(c, 255)).filter(|c| !c.is_empty()).collect();
    m.message_id = line(&m.message_id, MAX_TEXT);
    m.subject = line(&m.subject, MAX_TEXT);
    m.preview = line(&m.preview, MAX_PREVIEW);
    m.from = person(&m.from).unwrap_or_default();
    m.to = people(&m.to);
    m.cc = people(&m.cc);
    true
}

/// A page held to its sizes: whether it came whole. A folder's page says its messages are in that
/// folder, whatever else they say.
fn bounded_page(page: &mut ZimbraMessagePage) -> bool {
    let before = page.messages.len() + page.removed.len();
    if !page.folder.is_empty() {
        let folder = page.folder.clone();
        page.messages.iter_mut().for_each(|m| m.server.folder.clone_from(&folder));
    }
    page.messages.retain_mut(bounded_message);
    page.removed.retain(|id| zimbra_id_ok(id));
    before == page.messages.len() + page.removed.len()
}

/// A folder of the listing: whether it can be taken in, and if so as a Zimbra server can say it.
/// A folder that lies in itself, or in a folder that is no id, is at the top; there is no Outbox
/// on a Zimbra server, and its counts pass no folder over.
fn bounded_folder(f: &mut GraphFolder) -> bool {
    if !zimbra_id_ok(&f.id) {
        return false;
    }
    if f.parent == f.id || (!f.parent.is_empty() && !zimbra_id_ok(&f.parent)) {
        f.parent.clear();
    }
    if f.kind == FolderKind::Outbox {
        f.kind = FolderKind::Custom;
    }
    f.name = line(&f.name, MAX_NAME);
    f.counted = false;
    !f.name.is_empty()
}

/// A piece of a run held to its sizes, as the module documentation says, and whether it had to be
/// cut. A piece is the end of a run that went well (`whole`) only if it is the last, came whole,
/// and tells of no trouble; a cut one keeps no token.
pub(crate) fn bounded(mut result: ZimbraMailResult) -> (ZimbraMailResult, bool) {
    let reported_errors = !result.errors.is_empty();
    let mut cut = reported_errors;
    if let Some(listing) = result.listing.as_mut() {
        let before = listing.folders.len();
        listing.folders.retain_mut(bounded_folder);
        let whole = listing.folders.len() == before;
        // (A listing with no folder is not one: an account always has an Inbox.)
        listing.complete &= whole && !listing.folders.is_empty();
        cut |= !listing.complete;
    }
    result.well_known.retain(|id| zimbra_id_ok(id));
    cut |= result.pages.len() > MAX_PAGES;
    result.pages.truncate(MAX_PAGES);
    for page in &mut result.pages {
        cut |= !bounded_page(page);
    }
    // A page that came cut is no part of a fresh read that ends: nothing is removed for it.
    if cut {
        result.pages.iter_mut().for_each(|page| (page.afresh, page.last) = (false, false));
        result.token.clear();
        result.resume = None;
    }
    result.errors = lines(&result.errors);
    result.busy_for = result.busy_for.map(|seconds| seconds.min(MAX_BUSY));
    let trouble = reported_errors
        || result.stopped
        || result.auth_refused
        || result.code_needed
        || result.busy_for.is_some()
        || !result.untrusted.is_empty();
    result.whole &= !cut && !trouble && !result.partial;
    if !result.whole {
        result.token.clear();
        result.resume = None;
    }
    (result, cut)
}

/// Take in the listing of a run: every folder of mail the server has, by server id, the server's
/// own folders marked as such. How many folders went with it.
pub(crate) fn take_folders(
    mb: &mut Mailbox,
    account: Id,
    listing: &FolderListing,
    well_known: &[String],
    taken: &mut super::graph_mail::Taken,
) -> usize {
    let mut listing = listing.clone();
    for f in &mut listing.folders {
        f.well_known |= well_known.contains(&f.id);
    }
    let removed = if listing.complete { super::jmap_down::keep_what_is_only_here(mb, account, &listing) } else { 0 };
    super::graph_mail::take_listing(mb, account, &listing, taken);
    removed
}

fn take_folder_page(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &MessagePage, down: &mut Down, folder_read: bool) -> bool {
    let Some(id) = mb.folder_by_server(account, &page.folder) else { return false };
    graph_down::take_page(mb, index, account, page, down);
    if folder_read
        && let Ok(folder) = mb.folder_mut(id)
        && let Some(server) = folder.server.as_mut()
    {
        let ended = server.round_ended || server.caught_up;
        server.caught_up = page.caught_up;
        server.round_ended = page.caught_up || (!page.afresh && ended);
    }
    true
}

/// Take in one page of a run. A folder's page goes as a Graph page of that folder; a page of
/// changes is cut into one page per folder its messages are in, and what it says was deleted is
/// marked gone wherever it is.
pub(crate) fn take_page(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &ZimbraMessagePage, down: &mut Down) -> bool {
    if !page.folder.is_empty() {
        let as_page = MessagePage {
            folder: page.folder.clone(),
            messages: page.messages.clone(),
            removed: page.removed.clone(),
            afresh: page.afresh,
            caught_up: page.last,
            ..MessagePage::default()
        };
        return take_folder_page(mb, index, account, &as_page, down, true);
    }
    let mut by_folder: BTreeMap<&str, Vec<GraphMessage>> = BTreeMap::new();
    for m in &page.messages {
        by_folder.entry(m.server.folder.as_str()).or_default().push(m.clone());
    }
    let mut whole = true;
    for (folder, messages) in by_folder {
        let as_page = MessagePage { folder: folder.to_string(), messages, ..MessagePage::default() };
        whole &= take_folder_page(mb, index, account, &as_page, down, false);
    }
    for id in &page.removed {
        let Some(m) = index.find(mb, account, id) else { continue };
        if let Ok(m) = mb.message_mut(m)
            && let Some(s) = m.server.as_mut().filter(|s| !s.gone)
        {
            s.gone = true;
            down.gone += 1;
        }
    }
    whole
}
