// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail down through Microsoft Graph (R3.2; R3 spec S5, S9–S11, S15).
//!
//! A folder's mail arrives as pages of its message delta, each taken in by itself with the link
//! that goes on from it, so that the mailbox file never holds a page without its link, nor a link
//! past what it has (S10). Everything is matched by server id (S5): a page that comes twice
//! changes nothing.
//!
//! - **A new message** comes as its headers and preview, not **fetched**. Once its folder's first
//!   round has ended, it is sorted by the Safe and Blocked Senders lists, and mail arriving in the
//!   Inbox then goes through the rules (#567), on every page of a later round.
//! - **A message the server has changed** takes the server's read, flag, categories and
//!   importance, but for a property changed here since the server last said (ADR 0003): that
//!   goes up with R3.3, and nothing changed here is overwritten by coming down.
//! - **A message whose read state alone the server says changed** (its id and `isRead`, as
//!   Exchange Online says a message read or unread elsewhere, #148) takes it, as a message changed
//!   there does: but for a message whose read state was changed here since.
//! - **A message the server says left a folder** (`@removed`: deleted, or moved out) is marked
//!   gone and kept. Seen in another folder, it has moved there, with what was fetched of it. Only
//!   once every folder's delta reached its end in a run is what is still gone removed here.
//! - **A folder listed afresh** (its link was no longer taken, S11) marks all its messages gone
//!   first: what the complete new listing brings is kept, and the rest goes at the end of the run.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{GraphMessage, GraphProgress, MessagePage, ServerMessage, merge_categories};
use mailcraft_model::time::Timestamp;
use mailcraft_model::{Body, FolderKind, Id, Mailbox, Message, OfflineWindow};

/// Where each message a server knows is, by its server id: kept between pages, as finding one
/// otherwise means a walk through every message of the mailbox.
#[derive(Debug, Default)]
pub struct ServerIndex {
    /// By account and server id: each mailbox names its messages by itself, so two mailboxes
    /// (an account and a shared mailbox opened through it) may use the same id for two messages.
    ids: HashMap<(Id, String), Id>,
    /// How many messages the mailbox had when this was last right: anything else that added or
    /// removed one makes it walk again.
    count: usize,
}

impl ServerIndex {
    fn rebuild(&mut self, mb: &Mailbox) {
        self.ids = mb.messages.values().filter_map(|m| m.server.as_ref().map(|s| ((m.account, s.id.clone()), m.id))).collect();
        self.count = mb.messages.len();
    }

    /// The message of `account` the server calls `id`.
    pub(crate) fn find(&mut self, mb: &Mailbox, account: Id, id: &str) -> Option<Id> {
        let is = |m: Id| mb.message(m).is_some_and(|m| m.account == account && m.server.as_ref().is_some_and(|s| s.id == id));
        if self.count != mb.messages.len() {
            self.rebuild(mb);
        }
        let key = (account, id.to_string());
        match self.ids.get(&key).copied() {
            Some(m) if is(m) => Some(m),
            // Out of date (a message was changed by other means): look again, once.
            Some(_) => {
                self.rebuild(mb);
                self.ids.get(&key).copied().filter(|m| is(*m))
            }
            None => None,
        }
    }

    fn note(&mut self, mb: &Mailbox, account: Id, id: &str, m: Id) {
        self.ids.insert((account, id.to_string()), m);
        self.count = mb.messages.len();
    }
}

/// What taking in pages did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Down {
    pub new: usize,
    pub changed: usize,
    pub gone: usize,
    pub removed: usize,
    /// New messages a rule moved out of the account as they arrived (#567).
    pub left: usize,
}

/// Take in one page of a folder's delta, with its link. A page for a folder this mailbox doesn't
/// have (yet) is passed over, and so is its link: it comes again.
pub fn take_page(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &MessagePage, down: &mut Down) {
    let Some(folder) = mb.folder_by_server(account, &page.folder) else { return };
    // The Safe and Blocked Senders lists sort what arrives once the folder's first round has
    // ended (#504), on every page of a later round (#567): never its first download, nor a round
    // read afresh. (A folder of a file from before #567 has only `caught_up` to say so.)
    let sorting = !page.afresh && mb.folder(folder).and_then(|f| f.server.as_ref()).is_some_and(|s| s.round_ended || s.caught_up);
    // Then the rules (#567), on what arrives in the Inbox and only when the lists sort. Never in a
    // shared mailbox: the rules, like the lists, are the owner's.
    let inbox = mb.folder(folder).is_some_and(|f| f.kind == FolderKind::Inbox);
    let ruling = sorting && mb.account(account).is_some_and(|a| !a.is_shared());
    // The server's copies of messages being moved out of the account are not new mail.
    let mut moving_out: HashSet<String> =
        mb.messages.values().filter_map(|m| m.moving_out.as_ref().filter(|out| out.account == account).map(|out| out.id.clone())).collect();
    if page.afresh {
        for m in mb.messages.values_mut() {
            if m.account == account
                && m.server.as_ref().is_some_and(|s| s.folder == page.folder && !s.gone)
                && let Some(s) = std::sync::Arc::make_mut(m).server.as_mut()
            {
                s.gone = true;
            }
            // Being moved out of the account: still in that folder only if the listing has it (#451).
            if m.moving_out.as_ref().is_some_and(|out| out.account == account && out.folder == page.folder && !out.left)
                && let Some(out) = std::sync::Arc::make_mut(m).moving_out.as_mut()
            {
                out.left = true;
            }
        }
    }
    for id in &page.removed {
        let Some(m) = index.find(mb, account, id) else {
            // Being moved out of the account, and gone from where it was: a refused move has
            // nothing to go back to there, unless it turns up in another folder (#451).
            if moving_out.contains(id)
                && let Some(m) = self::moving_out(mb, account, id)
                && let Ok(m) = mb.message_mut(m)
                && let Some(out) = m.moving_out.as_mut().filter(|out| out.folder == page.folder)
            {
                out.left = true;
            }
            continue;
        };
        if let Ok(m) = mb.message_mut(m)
            && let Some(s) = m.server.as_mut().filter(|s| s.folder == page.folder && !s.gone)
        {
            s.gone = true;
            down.gone += 1;
        }
    }
    for there in &page.read {
        let Some(m) = index.find(mb, account, &there.id) else { continue };
        if let Ok(m) = mb.message_mut(m)
            && let Some(s) = m.server.as_mut().filter(|s| s.folder == page.folder)
        {
            let was = s.read;
            s.read = there.read;
            if m.read == was && m.read != there.read {
                m.read = there.read;
                down.changed += 1;
            }
        }
    }
    for there in &page.messages {
        if there.server.id.is_empty() {
            continue;
        }
        match index.find(mb, account, &there.server.id) {
            Some(m) => {
                if update(mb, account, m, folder, &page.folder, there) {
                    down.changed += 1;
                }
            }
            None if moving_out.contains(&there.server.id) => {
                // Moved on there by another program while being moved out: it is in this folder now.
                if let Some(m) = self::moving_out(mb, account, &there.server.id)
                    && let Ok(m) = mb.message_mut(m)
                    && let Some(out) = m.moving_out.as_mut()
                {
                    (out.folder, out.left) = (page.folder.clone(), false);
                }
            }
            None => {
                if let Ok(m) = mb.add_message(new_message(folder, &page.folder, there)) {
                    index.note(mb, account, &there.server.id, m);
                    // By the Safe and Blocked Senders lists first (#504), then the rules: mail a
                    // list sends to Junk E-mail isn't the rules' to file, and a safe sender's taken
                    // out of Junk E-mail goes through them as any mail arriving in the Inbox does.
                    // A move goes up as the owner's own does (R3.3).
                    let sorted = sorting && mb.sort_junk(m);
                    if ruling && inbox != sorted {
                        mb.apply_rules(m);
                        // Out of the account: should the page list it again, that is the same
                        // message (#606 review).
                        if mb.message(m).is_some_and(|m| m.account != account) {
                            moving_out.insert(there.server.id.clone());
                            down.left += 1;
                        }
                    }
                    down.new += 1;
                }
            }
        }
    }
    if !page.link.is_empty()
        && let Ok(f) = mb.folder_mut(folder)
        && let Some(s) = f.server.as_mut()
    {
        s.delta = page.link.clone();
        s.caught_up = page.caught_up;
        // Ended before this page, or by it; a page read afresh begins again.
        s.round_ended = sorting || page.caught_up;
        // The counts the link was got with, or none (#276): a new link never keeps the old one's.
        s.counts = page.counts;
    }
}

/// A message the server has and this mailbox hasn't: its headers and preview.
pub(crate) fn new_message(folder: Id, server_folder: &str, there: &GraphMessage) -> Message {
    let server = ServerMessage { folder: server_folder.to_string(), gone: false, ..there.server.clone() };
    Message {
        folder,
        message_id: there.message_id.trim().trim_start_matches('<').trim_end_matches('>').to_string(),
        from: there.from.clone(),
        to: there.to.clone(),
        cc: there.cc.clone(),
        subject: there.subject.clone(),
        date: there.sent,
        received: there.received,
        body: Body { text: there.preview.clone(), html: None },
        read: server.read,
        answered: server.answered,
        forwarded: server.forwarded,
        flag: server.flag,
        flag_due: server.flag_due,
        importance: server.importance,
        categories: server.categories.clone(),
        draft: there.draft.unwrap_or(false),
        server: Some(server),
        unfetched: true,
        attachments_on_server: there.has_attachments,
        ..Default::default()
    }
}

/// Bring a message this mailbox has in step with what the server says of it now. Whether
/// anything changed.
fn update(mb: &mut Mailbox, account: Id, id: Id, folder: Id, server_folder: &str, there: &GraphMessage) -> bool {
    let Some(m) = mb.message(id) else { return false };
    let before = m.clone();
    let last = m.server.clone().unwrap_or_default();
    // A draft the server has changed (its change key differs from what was last seen of it).
    let changed_there = m.draft && !last.change_key.is_empty() && !there.server.change_key.is_empty() && last.change_key != there.server.change_key;
    if changed_there && m.server_draft.as_ref().is_some_and(|d| d.pending) {
        // Changed here too: both are kept (S26).
        let copy = new_message(folder, server_folder, there);
        return super::graph_drafts::keep_twice(mb, id, Some(copy)).is_some();
    }
    // Where the server last had it, as this mailbox has that folder: a message still there here
    // follows the server; one moved here since stays where it was put (that move goes up, R3.3).
    let was_here = mb.folder_by_server(account, &last.folder);
    let follows = last.gone || was_here.is_none_or(|was| m.folder == was);
    let Ok(m) = mb.message_mut(id) else { return false };
    let now = &there.server;
    if m.read == last.read {
        m.read = now.read;
    }
    // The marks a JMAP server keeps as keywords, as the read state (a Graph server says neither:
    // they stay false on both sides, and what is set here stays).
    if m.answered == last.answered {
        m.answered = now.answered;
    }
    if m.forwarded == last.forwarded {
        m.forwarded = now.forwarded;
    }
    // The flag is its status and its due date: changed here, both stay as they are.
    if m.flag == last.flag && m.flag_due == last.flag_due {
        m.flag = now.flag;
        m.flag_due = now.flag_due;
    }
    // Changed on both sides: what was added and taken away here goes onto the server's list (S17).
    if m.categories == last.categories {
        m.categories = now.categories.clone();
    } else if now.categories != last.categories {
        m.categories = merge_categories(&last.categories, &m.categories, &now.categories);
    }
    if m.importance == last.importance {
        m.importance = now.importance;
    }
    if follows {
        m.folder = folder;
    }
    if m.unfetched {
        m.attachments_on_server = there.has_attachments;
    }
    // Sent, the draft is filed in Sent Items under its id: no longer a draft (#611), when the
    // server says so and not merely leaves it out. One saved here that waits to go up stays one.
    // What was sent may not be what was last seen of the draft: it is fetched afresh.
    let sent = m.draft && there.draft == Some(false) && !m.server_draft.as_ref().is_some_and(|d| d.pending);
    if sent {
        m.draft = false;
        m.server_draft = None;
    }
    if changed_there || sent {
        // Changed elsewhere only: the server's version, fetched afresh.
        (m.subject, m.to, m.cc, m.unfetched) = (there.subject.clone(), there.to.clone(), there.cc.clone(), true);
        m.attachments_on_server = there.has_attachments;
        if let Some(sd) = m.server_draft.as_mut() {
            sd.synced = None;
        }
    }
    // A delta never says a size (#300): the last one known, of the whole message when it was
    // fetched, stays; but not for a draft changed elsewhere, whose new version is still to be
    // fetched and may be smaller than the background fetch's limit, which its old size is held
    // against. Until then the list says what is here, its old version.
    let size = if now.size > 0 || changed_there { now.size } else { last.size };
    m.server = Some(ServerMessage { folder: server_folder.to_string(), gone: false, size, ..now.clone() });
    *m != before
}

/// Every folder's delta reached its end: what is still gone from every folder is gone here too.
pub fn sweep(mb: &mut Mailbox, account: Id, down: &mut Down) {
    // But for a draft saved here that waits to go up: it is made on the server anew (S26); and
    // for what waits in the Outbox, or is held there (R3.4.2): it stays, for the owner to see.
    let outbox = mb.folder_of_kind(account, mailcraft_model::FolderKind::Outbox);
    for m in mb.messages.values_mut() {
        let keeps = (m.draft && m.server_draft.as_ref().is_some_and(|d| d.pending)) || Some(m.folder) == outbox;
        let waits = m.account == account && keeps && m.server.as_ref().is_some_and(|s| s.gone);
        if waits {
            let m = std::sync::Arc::make_mut(m);
            m.server = None;
            if let Some(sd) = m.server_draft.as_mut() {
                sd.synced = None;
            }
        }
    }
    // Put back after a refused copy, and gone from the server since: kept here instead (#451).
    let returned: Vec<Id> =
        mb.messages.values().filter(|m| m.account == account && m.returned > 0 && m.server.as_ref().is_some_and(|s| s.gone)).map(|m| m.id).collect();
    for id in returned {
        super::graph_place::kept_instead(mb, id);
    }
    let before = mb.messages.len();
    mb.messages.retain(|_, m| !(m.account == account && m.server.as_ref().is_some_and(|s| s.gone)));
    down.removed += before - mb.messages.len();
    // What was put back after a refused copy and is still there once every folder was read (or
    // passed over, its counts the same): one run nearer to being the server's message as any other.
    for m in mb.messages.values_mut() {
        if m.account == account && m.returned > 0 {
            std::sync::Arc::make_mut(m).returned -= 1;
        }
    }
}

/// The message being moved out of `account` whose server copy is `id`.
pub fn moving_out(mb: &Mailbox, account: Id, id: &str) -> Option<Id> {
    mb.messages.values().find(|m| m.moving_out.as_ref().is_some_and(|out| out.account == account && out.id == id)).map(|m| m.id)
}

/// What the status bar says while a run is under way.
pub fn progress_words(mb: &Mailbox, account: Id, progress: &GraphProgress, lang: mailcraft_i18n::Lang) -> String {
    let name = mb.folder_by_server(account, &progress.folder).and_then(|f| mb.folder(f)).map(|f| f.name.clone()).unwrap_or_default();
    let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    let (read, total) = (progress.read.min(progress.total).to_string(), progress.total.to_string());
    let (number, folders) = (progress.number.to_string(), progress.folders.to_string());
    let template = if progress.total > 0 {
        lang.t("Receiving {name}: {read} of {total} · folder {number} of {folders}")
    } else {
        lang.t("Receiving {name} · folder {number} of {folders}")
    };
    mailcraft_i18n::fmt(template, &[("name", &name), ("read", &read), ("total", &total), ("number", &number), ("folders", &folders)])
}

/// The next messages of `account` to fetch whole in the background (R3 spec S12): not fetched,
/// not gone, no larger than the account's "Download messages in full", within its mail to keep
/// offline at `now` (#143: older mail is fetched when it is opened), newest first; `most` of
/// them at most, by server id.
pub fn fetch_queue(mb: &Mailbox, account: Id, most: usize, now: Timestamp) -> Vec<String> {
    fetch_queue_passing_over(mb, account, most, now, &HashSet::new())
}

/// [`fetch_queue`] without the messages in `passed_over` (by account and server id): those the
/// server wouldn't give whole this session.
pub fn fetch_queue_passing_over(mb: &Mailbox, account: Id, most: usize, now: Timestamp, passed_over: &HashSet<(Id, String)>) -> Vec<String> {
    let skip = |id: &str| passed_over.contains(&(account, id.to_string()));
    // Messages being moved out of the account first, whatever their size: the move ends only
    // once they are whole here (S19).
    let mut out: Vec<String> = mb
        .messages
        .values()
        .filter(|m| m.unfetched && !m.incomplete)
        .filter_map(|m| m.moving_out.as_ref().filter(|out| out.account == account && !skip(&out.id)).map(|out| out.id.clone()))
        .take(most)
        .collect();
    out.extend(fetch_queue_of_account(mb, account, most.saturating_sub(out.len()), now, &skip));
    out
}

fn fetch_queue_of_account(mb: &Mailbox, account: Id, most: usize, now: Timestamp, skip: &dyn Fn(&str) -> bool) -> Vec<String> {
    let limit = keeps_whole(mb, account);
    let window = OfflineWindow::of(mb, now);
    let mut waiting: Vec<(i64, &str)> = mb
        .messages
        .values()
        .filter(|m| m.account == account && m.unfetched && window.within(m))
        .filter_map(|m| {
            m.server.as_ref().filter(|s| !s.gone && limit.is_none_or(|limit| s.size <= limit) && !skip(&s.id)).map(|s| (m.received, s.id.as_str()))
        })
        .collect();
    let most = most.min(waiting.len());
    if most == 0 {
        return Vec::new();
    }
    // The newest `most`, without sorting them all.
    if most < waiting.len() {
        waiting.select_nth_unstable_by(most - 1, |a, b| b.0.cmp(&a.0));
        waiting.truncate(most);
    }
    waiting.sort_by_key(|w| std::cmp::Reverse(w.0));
    waiting.into_iter().map(|(_, id)| id.to_string()).collect()
}

/// How many messages of `account` are kept in full here, and how many of them are still to be
/// fetched ([`full_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FullCounts {
    /// Messages from its server kept in full here, fetched or waiting to be: those within its
    /// mail to keep offline (#143).
    pub all: usize,
    /// Of those, the ones not fetched yet.
    pub waiting: usize,
    /// Of those waiting, the ones larger than its "Download messages in full" (they wait until
    /// they are opened).
    pub large: usize,
}

/// [`FullCounts`] of `account` at `now`, in one pass over the mailbox. Older mail isn't counted:
/// it stays as its headers and preview (#143).
pub fn full_counts(mb: &Mailbox, account: Id, now: Timestamp) -> FullCounts {
    let limit = keeps_whole(mb, account);
    let window = OfflineWindow::of(mb, now);
    let mut counts = FullCounts::default();
    for m in mb.messages.values().filter(|m| m.account == account && (m.server.is_some() || m.unfetched) && window.within(m)) {
        counts.all = counts.all.saturating_add(1);
        if m.unfetched {
            let size = m.server.as_ref().map_or(0, |s| s.size);
            counts.waiting = counts.waiting.saturating_add(1);
            counts.large = counts.large.saturating_add(usize::from(limit.is_some_and(|limit| size > limit)));
        }
    }
    counts
}

/// Take in a message fetched whole: its text, attachments and headers from its MIME, with what
/// the mailbox keeps of it (where it is, its marks, its server state) as it was. Whether there
/// was such a message.
pub fn take_fetched(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, id: &str, mime: &[u8]) -> bool {
    take_fetched_as(mb, index, account, id, mime).is_some()
}

/// [`take_fetched`], answering which message it was.
pub fn take_fetched_as(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, id: &str, mime: &[u8]) -> Option<Id> {
    let here = index.find(mb, account, id).or_else(|| moving_out(mb, account, id))?;
    let read = crate::mime::parse(mime);
    let m = mb.message_mut(here).ok()?;
    m.message_id = read.message_id;
    m.in_reply_to = read.in_reply_to;
    m.references = read.references;
    m.reply_to = read.reply_to;
    m.bcc = read.bcc;
    if !read.from.email.is_empty() {
        m.from = read.from;
    }
    if !read.to.is_empty() {
        m.to = read.to;
    }
    if !read.cc.is_empty() {
        m.cc = read.cc;
    }
    if !read.subject.is_empty() {
        m.subject = read.subject;
    }
    if read.date != 0 {
        m.date = read.date;
    }
    m.body = read.body;
    m.attachments = read.attachments;
    m.headers = read.headers;
    // What it is now is all of it: nothing the mailbox kept on disk of its preview is its body.
    m.kept = None;
    m.unfetched = false;
    m.attachments_on_server = false;
    // Its size is that of the MIME brought (#300), which the fetch caps and which is measured
    // here: kept for when it is back to its preview, and taken anew at every fetch, so that a
    // draft changed elsewhere and fetched again doesn't keep the size it had.
    if let Some(server) = m.server.as_mut() {
        server.size = mime.len() as u64;
    }
    // A draft as the server has it now: what a change made here is worked out from.
    if m.draft {
        let state = mailcraft_model::sync::DraftState::of(m);
        let sd = m.server_draft.get_or_insert_with(Default::default);
        if !sd.pending {
            sd.synced = Some(state);
        }
    }
    Some(here)
}

/// Back to its headers and `preview` (#143): what [`take_fetched`] brought in goes, and the
/// message is not fetched again, as it was when it came down. Everything else stays as it is:
/// where it is, its marks, the server's state of it, what waits to go up, its headers. It wears
/// the paperclip still when it had attachments; opening it fetches it whole again.
pub fn thin_message(m: &mut Message, preview: String) {
    m.attachments_on_server = m.has_attachments();
    m.body = Body { text: preview, html: None };
    m.attachments.clear();
    m.kept = None;
    m.unfetched = true;
}

/// Once a mailbox (#185): the messages of Microsoft accounts fetched whole whose subject, names
/// or text hold U+FFFD are marked not fetched, so that the background fetches them again and
/// reads them with the character sets of today. Mail fetched before Windows-1250 and the rest
/// were read was kept with U+FFFD where a character was (the MIME itself isn't kept). Their
/// headers, preview, attachments and everything done to them here stay as they are until the
/// new copy comes; [`take_fetched`] then writes the text over. Drafts, a message being moved
/// out of its account, and one the server no longer has are left alone. Only what is in
/// memory is looked at: the text of older mail kept on disk isn't read for this. How many were
/// marked; none once the mailbox has been looked through ([`Mailbox::redecoded`]).
pub fn refetch_unreadable(mb: &mut Mailbox) -> usize {
    if mb.redecoded {
        return 0;
    }
    const MARK: char = char::REPLACEMENT_CHARACTER;
    let graph: HashSet<Id> = mb.accounts.iter().filter(|a| a.kind == mailcraft_model::AccountKind::Microsoft).map(|a| a.id).collect();
    // Without a Microsoft account there is nothing to look for, and the mailbox stays as it is.
    if graph.is_empty() {
        return 0;
    }
    mb.redecoded = true;
    let unreadable = |m: &Message| {
        let names = std::iter::once(&m.from).chain(&m.to).chain(&m.cc).chain(&m.reply_to).map(|a| a.name.as_str());
        let texts = [m.subject.as_str(), m.body.text.as_str(), m.body.html.as_deref().unwrap_or("")];
        names.chain(texts).any(|text| text.contains(MARK))
    };
    let mut marked = 0usize;
    for m in mb.messages.values_mut() {
        let whole_from_server = graph.contains(&m.account)
            && !m.unfetched
            && !m.incomplete
            && !m.draft
            && m.server_draft.is_none()
            && m.moving_out.is_none()
            && m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty());
        if whole_from_server && unreadable(m) {
            let m = std::sync::Arc::make_mut(m);
            m.attachments_on_server = m.has_attachments();
            m.unfetched = true;
            marked = marked.saturating_add(1);
        }
    }
    marked
}

/// How large a message of `account` is fetched whole in the background: its own Download
/// messages in full, and for a shared mailbox its owner's (#70: it keeps as much mail as its
/// owner's account does; there is no setting of its own).
fn keeps_whole(mb: &Mailbox, account: Id) -> Option<u64> {
    let a = mb.owner_of(account).or(mb.account(account))?;
    a.download_in_full.limit()
}

#[cfg(test)]
#[path = "graph_down_size_tests.rs"]
mod size_tests;

#[cfg(test)]
mod tests {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{AccountKind, DownloadInFull, FlagStatus, Folder, FolderKind, Mailbox, Message};

    use super::*;

    fn mailbox() -> (Mailbox, Id, Id) {
        let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
        let account = mb.accounts[0].id;
        mb.accounts[0].kind = AccountKind::Microsoft;
        let folder = mb.alloc_id();
        mb.folders.push(Folder { id: folder, account, name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() });
        for (n, size) in [(1, 1_000), (2, 30 << 20), (3, 0), (4, 2_000)] {
            let server = ServerMessage { id: format!("AAkA{n}"), folder: "F".into(), size, ..Default::default() };
            let m =
                Message { folder, received: n * 60, unfetched: n != 4, server: Some(server), subject: format!("Preview {n}"), ..Default::default() };
            mb.add_message(m).unwrap();
        }
        (mb, account, folder)
    }

    #[test]
    fn the_newest_not_fetched_within_the_limit_come_first() {
        let (mut mb, account, _) = mailbox();
        // Number 4 is fetched; number 2 is larger than 10 MB; number 3 didn't say its size.
        assert_eq!(fetch_queue(&mb, account, 10, 0), ["AAkA3", "AAkA1"]);
        assert_eq!(fetch_queue(&mb, account, 1, 0), ["AAkA3"]);
        assert_eq!(full_counts(&mb, account, 0), FullCounts { all: 4, waiting: 3, large: 1 });
        mb.accounts[0].download_in_full = DownloadInFull::Everything;
        assert_eq!(fetch_queue(&mb, account, 10, 0), ["AAkA3", "AAkA2", "AAkA1"]);
        assert_eq!(full_counts(&mb, account, 0), FullCounts { all: 4, waiting: 3, large: 0 });
    }

    #[test]
    fn a_fetched_message_keeps_what_was_done_to_it_here() {
        let (mut mb, account, folder) = mailbox();
        let id = mb.messages.values().find(|m| m.subject == "Preview 1").unwrap().id;
        {
            let m = mb.message_mut(id).unwrap();
            m.read = true;
            m.flag = FlagStatus::Flagged;
        }
        let mime = b"From: Kit <kit@example.org>\r\nTo: avery@mail.example\r\nSubject: The whole of it\r\nMessage-ID: <w1@example.org>\r\nContent-Type: text/plain\r\n\r\nAll of the text.\r\n";
        let mut index = ServerIndex::default();
        assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", mime));
        assert!(!take_fetched(&mut mb, &mut index, account, "AAkA9", mime));
        let m = mb.message(id).unwrap();
        assert_eq!((m.subject.as_str(), m.body.text.trim(), m.from.email.as_str()), ("The whole of it", "All of the text.", "kit@example.org"));
        assert!(!m.unfetched && m.read && m.flag == FlagStatus::Flagged && m.folder == folder && m.server.as_ref().unwrap().id == "AAkA1");
    }

    /// R6.2 S5: the server's "has attachments" is kept on a message not fetched yet, so that it
    /// wears the paperclip and `has:attachment` finds it; once fetched, its own files say.
    #[test]
    fn a_message_not_fetched_yet_has_the_attachments_the_server_said() {
        let (mut mb, account, folder) = mailbox();
        mb.folder_mut(folder).unwrap().server = Some(mailcraft_model::ServerFolder { id: "F".into(), ..Default::default() });
        let page = |id: &str, has_attachments: bool| MessagePage {
            folder: "F".into(),
            messages: vec![GraphMessage {
                server: ServerMessage { id: id.into(), ..Default::default() },
                subject: "Quarterly figures".into(),
                has_attachments,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut index = ServerIndex::default();
        take_page(&mut mb, &mut index, account, &page("AAkB1", true), &mut Down::default());
        let id = mb.messages.values().find(|m| m.subject == "Quarterly figures").unwrap().id;
        let m = mb.message(id).unwrap();
        assert!(m.unfetched && m.attachments.is_empty() && m.attachments_on_server && m.has_attachments());
        let query = mailcraft_model::search::Query::parse("has:attachment");
        assert!(query.matches(m, 0));
        assert!(!query.matches(mb.messages.values().find(|m| m.subject == "Preview 1").unwrap(), 0));
        // Written to the mailbox file and read back; a file from before has none.
        let json = serde_json::to_string(m).unwrap();
        assert!(json.contains(r#""attachmentsOnServer":true"#), "{json}");
        assert_eq!(&serde_json::from_str::<Message>(&json).unwrap(), m);
        assert!(!serde_json::from_str::<Message>(r#"{"id": 9, "unfetched": true}"#).unwrap().has_attachments());

        // The server says it has none after all (its files were taken off): neither does it here.
        take_page(&mut mb, &mut index, account, &page("AAkB1", false), &mut Down::default());
        assert!(!mb.message(id).unwrap().has_attachments());
        take_page(&mut mb, &mut index, account, &page("AAkB1", true), &mut Down::default());
        assert!(mb.message(id).unwrap().has_attachments());

        // Fetched whole, without files: its own attachments say.
        let mime = b"From: Kit <kit@example.org>\r\nSubject: Quarterly figures\r\nContent-Type: text/plain\r\n\r\nNo files.\r\n";
        assert!(take_fetched(&mut mb, &mut index, account, "AAkB1", mime));
        let m = mb.message(id).unwrap();
        assert!(!m.unfetched && !m.attachments_on_server && !m.has_attachments() && m.folder == folder);
    }

    #[test]
    fn a_message_fetched_whole_is_not_given_back_what_was_kept_on_disk_of_its_preview() {
        let (mut mb, account, _) = mailbox();
        let id = mb.messages.values().find(|m| m.subject == "Preview 1").unwrap().id;
        // Its preview's HTML is on disk, not in memory (R6.1).
        let stale = mailcraft_model::ContentRef { sha256: [7; 32], len: 40 };
        mb.message_mut(id).unwrap().kept = Some(Box::new(mailcraft_model::KeptBody { html: Some(stale), ..Default::default() }));
        let mime = b"From: Kit <kit@example.org>\r\nSubject: Plain\r\nContent-Type: text/plain\r\n\r\nOnly text.\r\n";
        assert!(take_fetched(&mut mb, &mut ServerIndex::default(), account, "AAkA1", mime));
        let m = mb.message(id).unwrap();
        assert!(m.kept.is_none() && !m.body_is_kept() && m.body.html.is_none(), "{:?}", m.kept);
    }

    /// A message in Windows-1250 (#185): the sender's name, the subject and the text with the
    /// characters Windows-1250 has and Windows-1252 hasn't (đ, ž) or has elsewhere (’, –).
    pub(crate) const CENTRAL_EUROPEAN: &[u8] = b"From: =?windows-1250?Q?Tarik_Gvo=9E=F0ar?= <tarik@example.org>\r\n\
To: Avery <avery@mail.example>\r\nSubject: =?windows-1250?Q?Introduction_=96_Josh?=\r\nMessage-ID: <cp1250@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\n\
Content-Type: text/plain; charset=windows-1250\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHi Avery, we=92ve met.\r\n--b\r\n\
Content-Type: text/html; charset=windows-1250\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
<html><head><meta http-equiv=3D\"Content-Type\" content=3D\"text/html; charset=3Dwindows-1250\"></head><body><p>Hi Avery, we=92ve met.</p></body></html>\r\n--b--\r\n";

    #[test]
    fn mail_kept_with_unreadable_characters_is_fetched_again_once() {
        let (mut mb, account, folder) = mailbox();
        // As it was kept before Windows-1250 was read: U+FFFD where đ, ž, ’ and – were.
        let broken = |n: u32, draft: bool| {
            let server = ServerMessage { id: format!("AAkC{n}"), folder: "F".into(), size: 2_000, ..Default::default() };
            Message {
                folder,
                received: 9_000 + i64::from(n),
                server: Some(server),
                draft,
                from: mailcraft_model::Address::new("Tarik Gvo\u{FFFD}\u{FFFD}ar", "tarik@example.org"),
                subject: "Introduction \u{FFFD} Josh".into(),
                body: Body { text: "Hi Avery, we\u{FFFD}ve met.".into(), html: Some("<p>Hi Avery, we\u{FFFD}ve met.</p>".into()) },
                read: true,
                flag: FlagStatus::Flagged,
                categories: vec!["Blue category".into()],
                ..Default::default()
            }
        };
        let id = mb.add_message(broken(1, false)).unwrap();
        // A draft is the owner's own: written over, what was written here would be lost.
        let draft = mb.add_message(broken(2, true)).unwrap();
        // Only its body has one, and the body only in HTML.
        let mut html_only = broken(3, false);
        (html_only.from.name, html_only.subject, html_only.body.text) = ("Tarik".into(), "Plain".into(), "Plain".into());
        let html_only = mb.add_message(html_only).unwrap();
        let count = mb.messages.len();

        assert_eq!(refetch_unreadable(&mut mb), 2);
        assert!(mb.redecoded);
        let m = mb.message(id).unwrap();
        // Nothing disappears meanwhile: its headers and text are as they were until the new copy comes.
        assert!(m.unfetched && m.subject.starts_with("Introduction") && m.body.html.is_some());
        assert!(mb.message(html_only).unwrap().unfetched && !mb.message(draft).unwrap().unfetched);
        assert!(fetch_queue(&mb, account, 10, 0).contains(&"AAkC1".to_string()));
        // Opened again: not looked through again.
        assert_eq!(refetch_unreadable(&mut mb), 0);

        let mut index = ServerIndex::default();
        assert!(take_fetched(&mut mb, &mut index, account, "AAkC1", CENTRAL_EUROPEAN));
        let m = mb.message(id).unwrap();
        assert_eq!((m.from.name.as_str(), m.subject.as_str()), ("Tarik Gvožđar", "Introduction – Josh"));
        assert_eq!(m.body.text.trim(), "Hi Avery, we’ve met.");
        assert!(m.body.html.as_deref().unwrap().contains("we’ve met"));
        assert!(!m.unfetched && m.read && m.flag == FlagStatus::Flagged && m.categories == ["Blue category"] && m.folder == folder);
        assert_eq!(mb.messages.len(), count, "no copy");

        // Its source is broken (bytes that aren't UTF-8 in a message that says it is): fetched
        // again it is as before, and nothing fetches it once more, after a save and an open too.
        let bad = b"From: Kit <kit@example.org>\r\nSubject: Still \xff\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBroken \xff text.\r\n";
        assert!(take_fetched(&mut mb, &mut index, account, "AAkC3", bad));
        assert!(mb.message(html_only).unwrap().body.text.contains('\u{FFFD}'));
        let json = serde_json::to_string(&mb).unwrap();
        assert!(json.contains(r#""redecoded":true"#));
        let mut again: Mailbox = serde_json::from_str(&json).unwrap();
        assert_eq!(refetch_unreadable(&mut again), 0);
        assert!(again.messages.values().all(|m| !m.unfetched || m.subject.starts_with("Preview")));
        // A file from before has not been looked through.
        assert!(!serde_json::from_str::<Mailbox>("{}").unwrap().redecoded);
    }

    #[test]
    fn mail_fetched_again_keeps_its_size_in_the_list() {
        // As a delta brings it: size 0, "not said" (#273). Marked to be fetched again for an
        // unreadable character, it keeps its body until the new copy comes, and the list says
        // that body's size meanwhile, not none.
        let (mut mb, _, folder) = mailbox();
        let server = ServerMessage { id: "AAkC9".into(), folder: "F".into(), size: 0, ..Default::default() };
        let text = format!("Hi Avery, we\u{FFFD}ve met. {}", "x".repeat(4000));
        let whole = Message { folder, server: Some(server), body: Body { text, html: None }, ..Default::default() };
        let id = mb.add_message(whole).unwrap();
        assert_eq!(refetch_unreadable(&mut mb), 1);
        let m = mb.message(id).unwrap();
        assert!(m.unfetched);
        assert_eq!(m.list_size(), Some(m.size()));
        assert!(m.size() > 4000);
    }
}
