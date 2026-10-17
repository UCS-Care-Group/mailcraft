//! Mail down through Microsoft Graph (R3.2; R3 spec S5, S9–S11, S15).
//!
//! A folder's mail arrives as pages of its message delta, each taken in by itself with the link
//! that goes on from it, so that the mailbox file never holds a page without its link, nor a link
//! past what it has (S10). Everything is matched by server id (S5): a page that comes twice
//! changes nothing.
//!
//! - **A new message** comes as its headers and preview, not **fetched**.
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
use mailcraft_model::{Body, Id, Mailbox, Message};

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
    fn find(&mut self, mb: &Mailbox, account: Id, id: &str) -> Option<Id> {
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
}

/// Take in one page of a folder's delta, with its link. A page for a folder this mailbox doesn't
/// have (yet) is passed over, and so is its link: it comes again.
pub fn take_page(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &MessagePage, down: &mut Down) {
    let Some(folder) = mb.folder_by_server(account, &page.folder) else { return };
    // The server's copies of messages being moved out of the account are not new mail.
    let moving_out: HashSet<String> =
        mb.messages.values().filter_map(|m| m.moving_out.as_ref().filter(|out| out.account == account).map(|out| out.id.clone())).collect();
    if page.afresh {
        for m in mb.messages.values_mut() {
            if m.account == account
                && m.server.as_ref().is_some_and(|s| s.folder == page.folder && !s.gone)
                && let Some(s) = std::sync::Arc::make_mut(m).server.as_mut()
            {
                s.gone = true;
            }
        }
    }
    for id in &page.removed {
        let Some(m) = index.find(mb, account, id) else { continue };
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
            None if moving_out.contains(&there.server.id) => {}
            None => {
                if let Ok(m) = mb.add_message(new_message(folder, &page.folder, there)) {
                    index.note(mb, account, &there.server.id, m);
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
        flag: server.flag,
        flag_due: server.flag_due,
        importance: server.importance,
        categories: server.categories.clone(),
        draft: there.draft,
        server: Some(server),
        unfetched: true,
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
    if changed_there {
        // Changed elsewhere only: the server's version, fetched afresh.
        (m.subject, m.to, m.cc, m.unfetched) = (there.subject.clone(), there.to.clone(), there.cc.clone(), true);
        if let Some(sd) = m.server_draft.as_mut() {
            sd.synced = None;
        }
    }
    m.server = Some(ServerMessage { folder: server_folder.to_string(), gone: false, ..now.clone() });
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
    let before = mb.messages.len();
    mb.messages.retain(|_, m| !(m.account == account && m.server.as_ref().is_some_and(|s| s.gone)));
    down.removed += before - mb.messages.len();
}

/// The message being moved out of `account` whose server copy is `id`.
pub fn moving_out(mb: &Mailbox, account: Id, id: &str) -> Option<Id> {
    mb.messages.values().find(|m| m.moving_out.as_ref().is_some_and(|out| out.account == account && out.id == id)).map(|m| m.id)
}

/// What the status bar says while a run is under way.
pub fn progress_words(mb: &Mailbox, account: Id, progress: &GraphProgress) -> String {
    let name = mb.folder_by_server(account, &progress.folder).and_then(|f| mb.folder(f)).map(|f| f.name.clone()).unwrap_or_default();
    let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    if progress.total > 0 {
        format!("Receiving {name}: {} of {} · folder {} of {}", progress.read.min(progress.total), progress.total, progress.number, progress.folders)
    } else {
        format!("Receiving {name} · folder {} of {}", progress.number, progress.folders)
    }
}

/// The next messages of `account` to fetch whole in the background (R3 spec S12): not fetched,
/// not gone, no larger than the account's "Download messages in full", newest first; `most` of
/// them at most, by server id.
pub fn fetch_queue(mb: &Mailbox, account: Id, most: usize) -> Vec<String> {
    // Messages being moved out of the account first, whatever their size: the move ends only
    // once they are whole here (S19).
    let mut out: Vec<String> = mb
        .messages
        .values()
        .filter(|m| m.unfetched && !m.incomplete)
        .filter_map(|m| m.moving_out.as_ref().filter(|out| out.account == account).map(|out| out.id.clone()))
        .take(most)
        .collect();
    out.extend(fetch_queue_of_account(mb, account, most.saturating_sub(out.len())));
    out
}

fn fetch_queue_of_account(mb: &Mailbox, account: Id, most: usize) -> Vec<String> {
    let limit = keeps_whole(mb, account);
    let mut waiting: Vec<(i64, &str)> = mb
        .messages
        .values()
        .filter(|m| m.account == account && m.unfetched)
        .filter_map(|m| m.server.as_ref().filter(|s| !s.gone && limit.is_none_or(|limit| s.size <= limit)).map(|s| (m.received, s.id.as_str())))
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

/// How many messages of `account` aren't fetched, and how many of those are larger than its
/// "Download messages in full" (they wait until they are opened).
pub fn not_fetched(mb: &Mailbox, account: Id) -> (usize, usize) {
    let limit = keeps_whole(mb, account);
    mb.messages.values().filter(|m| m.account == account && m.unfetched).fold((0, 0), |(all, large), m| {
        let size = m.server.as_ref().map_or(0, |s| s.size);
        (all + 1, large + usize::from(limit.is_some_and(|limit| size > limit)))
    })
}

/// Take in a message fetched whole: its text, attachments and headers from its MIME, with what
/// the mailbox keeps of it (where it is, its marks, its server state) as it was. Whether there
/// was such a message.
pub fn take_fetched(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, id: &str, mime: &[u8]) -> bool {
    let Some(m) = index.find(mb, account, id).or_else(|| moving_out(mb, account, id)) else { return false };
    let read = crate::mime::parse(mime);
    let Ok(m) = mb.message_mut(m) else { return false };
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
    // A draft as the server has it now: what a change made here is worked out from.
    if m.draft {
        let state = mailcraft_model::sync::DraftState::of(m);
        let sd = m.server_draft.get_or_insert_with(Default::default);
        if !sd.pending {
            sd.synced = Some(state);
        }
    }
    true
}

/// How large a message of `account` is fetched whole in the background: its own Download
/// messages in full, and for a shared mailbox its owner's (#70: it keeps as much mail as its
/// owner's account does; there is no setting of its own).
fn keeps_whole(mb: &Mailbox, account: Id) -> Option<u64> {
    let a = mb.owner_of(account).or(mb.account(account))?;
    a.download_in_full.limit()
}

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
        assert_eq!(fetch_queue(&mb, account, 10), ["AAkA3", "AAkA1"]);
        assert_eq!(fetch_queue(&mb, account, 1), ["AAkA3"]);
        assert_eq!(not_fetched(&mb, account), (3, 1));
        mb.accounts[0].download_in_full = DownloadInFull::Everything;
        assert_eq!(fetch_queue(&mb, account, 10), ["AAkA3", "AAkA2", "AAkA1"]);
        assert_eq!(not_fetched(&mb, account), (3, 0));
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
}
