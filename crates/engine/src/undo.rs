// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo and redo for the Microsoft accounts kept in step through Microsoft Graph (R3.5).
//!
//! Undo puts back what a change touched, as it was before it (#605): the messages and folders it
//! changed, and the waiting changes it added or took away. What it left alone stays as it is now:
//! mail that arrived since, what the server took away, how far an IMAP folder has been read
//! ([`leave_alone`]). For a Microsoft
//! account, what its server last said stays as it is now: each message's and folder's last synced
//! state, the queue of what can't be worked out by comparing, the mail that came down since and
//! what the server took away. The next run then sends the difference, which is the reverse of the
//! change undone: a move back, a flag cleared, categories as they were, out of Deleted Items.
//! So the history is no longer forgotten when a change reaches the server.
//!
//! Undo merges three mailboxes: the one it goes back to, the one right after the change (only
//! what the change touched is kept of it, [`Touched`]) and the one there is now. What the change
//! didn't touch is taken as it is now (mail that came down since, a message read by opening it); of
//! what it touched, only the parts it changed go back.
//!
//! What can't be reversed on the server isn't brought back here, and the undo says so:
//! - **Deleted for good** (a message or a folder the server has): it stays deleted.
//! - **Sending**: nothing in a Microsoft account's Outbox is undone or brought back, so an undo
//!   never unsends, holds back or sends again; nor is the draft that became the message sent.
//! - **Moved out of the account** once the server's copy is removed: it stays where it was moved.
//!
//! What the change made that the server has already (a copy, a draft, a folder) goes into Deleted
//! Items, here and there, rather than being deleted for good; what the server hasn't got yet is
//! taken away here and never made there.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use mailcraft_model::sync::{DraftState, MovingOut, ServerMessage, same_category};
use mailcraft_model::{AccountKind, Folder, FolderKind, Id, Mailbox, Message, Pending, Queued, RETURNED_LISTINGS};

/// What a change did to the mail of the Microsoft accounts: the messages and folders it changed,
/// as they were right after it (`None`: it took them away). Nothing for a mailbox without one.
///
/// Besides, for every account (#605): which messages and folders the change touched, and which
/// changes waiting to go to a mail server it added or took away. Undo puts back only those.
#[derive(Clone, Debug, Default)]
pub(crate) struct Touched {
    messages: BTreeMap<Id, Option<Arc<Message>>>,
    folders: BTreeMap<Id, Option<Folder>>,
    changed_messages: BTreeSet<Id>,
    changed_folders: BTreeSet<Id>,
    pending_added: HashSet<String>,
    pending_removed: HashSet<String>,
}

/// The accounts of `mb` kept in step through Graph.
fn graph_accounts(mb: &Mailbox) -> BTreeSet<Id> {
    mb.accounts.iter().filter(|a| matches!(a.kind, AccountKind::Microsoft | AccountKind::Exchange)).map(|a| a.id).collect()
}

/// Whether `m` is one of those accounts' (or is being moved out of one).
fn relevant(m: &Message, g: &BTreeSet<Id>) -> bool {
    g.contains(&m.account) || m.moving_out.as_ref().is_some_and(|out| g.contains(&out.account))
}

/// A change waiting to go to a server, as a key to compare it by.
fn key(p: &Pending) -> String {
    format!("{p:?}")
}

/// What going from `before` to `after` changed: of the Microsoft accounts' mail, what it is now;
/// of every account's, which messages and folders it was, and the waiting changes it added or took away.
pub(crate) fn touched(before: &Mailbox, after: &Mailbox) -> Touched {
    let mut g = graph_accounts(before);
    g.extend(graph_accounts(after));
    let mut t = Touched::default();
    for (id, b) in &before.messages {
        match after.messages.get(id) {
            Some(a) if Arc::ptr_eq(a, b) || a == b => {}
            Some(a) => {
                t.changed_messages.insert(*id);
                if relevant(b, &g) || relevant(a, &g) {
                    t.messages.insert(*id, Some(a.clone()));
                }
            }
            None => {
                t.changed_messages.insert(*id);
                if relevant(b, &g) {
                    t.messages.insert(*id, None);
                }
            }
        }
    }
    for (id, a) in &after.messages {
        if !before.messages.contains_key(id) {
            t.changed_messages.insert(*id);
            if relevant(a, &g) {
                t.messages.insert(*id, Some(a.clone()));
            }
        }
    }
    for b in &before.folders {
        match after.folder(b.id) {
            Some(a) if a == b => {}
            a => {
                t.changed_folders.insert(b.id);
                if g.contains(&b.account) {
                    t.folders.insert(b.id, a.cloned());
                }
            }
        }
    }
    for a in after.folders.iter().filter(|f| before.folder(f.id).is_none()) {
        t.changed_folders.insert(a.id);
        if g.contains(&a.account) {
            t.folders.insert(a.id, Some(a.clone()));
        }
    }
    if before.pending != after.pending {
        let (was, is): (HashSet<String>, HashSet<String>) = (before.pending.iter().map(key).collect(), after.pending.iter().map(key).collect());
        t.pending_added = is.difference(&was).cloned().collect();
        t.pending_removed = was.difference(&is).cloned().collect();
    }
    t
}

/// What an undo couldn't do, for saying it once.
#[derive(Debug, Default)]
struct Said {
    deleted: Vec<String>,
    folders: Vec<String>,
    sent: Vec<String>,
    moved_out: Vec<String>,
    /// What the server is to be told besides (#70): a copy made in another mailbox of the
    /// owner's that goes again, a move back into the mailbox a message came from.
    queued: Vec<Queued>,
}

/// A subject or a name as a line about it shows it.
fn shown(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control()).take(60).collect();
    if text.trim().is_empty() { "(no subject)".to_string() } else { text }
}

impl Said {
    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.deleted.as_slice() {
            [] => {}
            [one] => lines.push(format!("'{}' was deleted for good: that can't be undone here.", shown(one))),
            many => lines.push(format!("{} messages were deleted for good: that can't be undone here.", many.len())),
        }
        match self.folders.as_slice() {
            [] => {}
            [one] => lines.push(format!("The folder '{}' was deleted for good: that can't be undone here.", shown(one))),
            many => lines.push(format!("{} folders were deleted for good: that can't be undone here.", many.len())),
        }
        match self.sent.as_slice() {
            [] => {}
            [one] => lines.push(format!("Sending '{}' can't be undone.", shown(one))),
            many => lines.push(format!("Sending {} messages can't be undone.", many.len())),
        }
        match self.moved_out.as_slice() {
            [] => {}
            [one] => lines.push(format!("'{}' was moved out of its account and the server's copy removed: that can't be undone here.", shown(one))),
            many => lines
                .push(format!("{} messages were moved out of their account and the server's copies removed: that can't be undone here.", many.len())),
        }
        lines
    }
}

/// `target`, the mailbox an undo (or redo) goes back to, with the Microsoft accounts' mail kept in
/// step with `now`; `touched` is what the change being undone did. Answers the mailbox and what
/// couldn't be undone, in words.
pub(crate) fn keep_in_step(mut out: Mailbox, touched: &Touched, now: &Mailbox) -> (Mailbox, Vec<String>) {
    // Ids are never given out twice, whatever the undo brings back.
    out.ids_after(now);
    returned(&mut out, now);
    let g: BTreeSet<Id> = graph_accounts(now).intersection(&graph_accounts(&out)).copied().collect();
    leave_alone(&mut out, touched, now, &g);
    if g.is_empty() {
        return (out, Vec::new());
    }
    let mut said = Said::default();
    let dropped = folders(&mut out, touched, now, &g, &mut said);
    messages(&mut out, touched, now, &g, &mut said);
    // A folder the change made, not on the server, that something still lies in stays.
    for f in dropped {
        if out.messages.values().any(|m| m.folder == f.id) || out.folders.iter().any(|o| o.parent == Some(f.id)) {
            out.folders.push(f);
        }
    }
    tidy(&mut out, now, &g);
    queue(&mut out, now, &g);
    for q in std::mem::take(&mut said.queued) {
        if let Queued::PlaceMessage { message, .. } = &q {
            let message = *message;
            out.queued.retain(|o| !matches!(o, Queued::PlaceMessage { message: other, .. } if *other == message));
        }
        out.queued.push(q);
    }
    // What the server last gave of these is the server's, not the owner's to undo.
    out.outlook_categories = now.outlook_categories.clone();
    out.favorites_from_server = now.favorites_from_server.clone();
    (out, said.lines())
}

/// A message the undo puts back into the IMAP account it was being moved out of, while the run
/// that deletes its original there may be on its way: should that run's answer be lost, it is kept
/// here rather than forgotten when the server no longer lists it, as when it is moved back (#451).
fn returned(out: &mut Mailbox, now: &Mailbox) {
    for p in &now.pending {
        let Pending::Move { account, message, uid, .. } = p else { continue };
        let back = out.message(*message).is_some_and(|m| m.account == *account && m.uid == *uid && *uid != 0);
        if back
            && now.message(*message).is_some_and(|m| m.account != *account)
            && let Ok(m) = out.message_mut(*message)
        {
            m.returned = RETURNED_LISTINGS;
        }
    }
}

/// Everything but the Microsoft accounts' mail (the rest of `keep_in_step`'s work) stays as it is
/// now, but for what the change touched (#605): mail that came down after it, a message the
/// server took away since, and how far each IMAP folder has been read are not the change's to
/// undo. A message the change touched goes back to how it was before it; a folder the change
/// touched too, but with the server's side of it (its path, how far it has been read) as now.
/// The changes waiting to go to the server are as now, less those the change added and plus
/// those it took away.
fn leave_alone(out: &mut Mailbox, touched: &Touched, now: &Mailbox, g: &BTreeSet<Id>) {
    // Accounts that the undo takes away again (one the change added): nothing of theirs stays.
    let gone: BTreeSet<Id> = now.accounts.iter().map(|a| a.id).filter(|id| !out.accounts.iter().any(|a| a.id == *id)).collect();
    let ids: BTreeSet<Id> = out.messages.keys().chain(now.messages.keys()).copied().collect();
    for id in ids {
        let (t, c) = (out.messages.get(&id), now.messages.get(&id));
        if t.is_some_and(|m| relevant(m, g)) || c.is_some_and(|m| relevant(m, g)) || touched.changed_messages.contains(&id) {
            continue;
        }
        match c {
            Some(c) => {
                out.messages.insert(id, c.clone());
            }
            None => {
                out.messages.remove(&id);
            }
        }
    }
    let then = std::mem::take(&mut out.folders);
    let mut kept = Vec::with_capacity(then.len());
    let mut seen = BTreeSet::new();
    for t in then {
        seen.insert(t.id);
        if g.contains(&t.account) {
            kept.push(t);
            continue;
        }
        match (touched.changed_folders.contains(&t.id), now.folder(t.id)) {
            (true, Some(c)) => {
                let mut f = t;
                (f.remote, f.uid_validity, f.uid_seen, f.uid_from) = (c.remote.clone(), c.uid_validity, c.uid_seen, c.uid_from);
                (f.whole, f.server, f.awaiting_listing) = (c.whole, c.server.clone(), c.awaiting_listing);
                kept.push(f);
            }
            (true, None) => kept.push(t),
            (false, Some(c)) => kept.push(c.clone()),
            // Gone since (the server's, or the account's).
            (false, None) => {}
        }
    }
    let mut made = Vec::new();
    for c in now.folders.iter().filter(|f| !g.contains(&f.account) && !seen.contains(&f.id)) {
        if touched.changed_folders.contains(&c.id) { made.push(c.clone()) } else { kept.push(c.clone()) }
    }
    out.folders = kept;
    out.folders.retain(|f| !gone.contains(&f.account));
    out.messages.retain(|_, m| !gone.contains(&m.account));
    // A folder the change made that something still lies in stays.
    for f in made.into_iter().filter(|f| !gone.contains(&f.account)) {
        if out.messages.values().any(|m| m.folder == f.id) || out.folders.iter().any(|o| o.parent == Some(f.id)) {
            out.folders.push(f);
        }
    }
    // The waiting changes of the Microsoft accounts are as the mailbox goes back to (`queue`'s
    // business); the others are as now, but for the change's own.
    let owned = |p: &Pending| {
        let k = key(p);
        touched.pending_added.contains(&k)
            || touched.pending_removed.contains(&k)
            || matches!(p, Pending::Move { message, .. } if touched.changed_messages.contains(message))
    };
    let then = std::mem::take(&mut out.pending);
    let mut pending: Vec<Pending> = then.iter().filter(|p| g.contains(&p.account())).cloned().collect();
    pending.extend(now.pending.iter().filter(|p| !g.contains(&p.account()) && !owned(p)).cloned());
    pending.extend(then.iter().filter(|p| !g.contains(&p.account()) && owned(p)).cloned());
    pending.retain(|p| !gone.contains(&p.account()));
    out.pending = pending;
}

/// The folders of the Microsoft accounts. Answers the folders the change made that weren't on the
/// server, taken away unless something still lies in them.
fn folders(out: &mut Mailbox, touched: &Touched, now: &Mailbox, g: &BTreeSet<Id>, said: &mut Said) -> Vec<Folder> {
    let then = std::mem::take(&mut out.folders);
    let mut kept = Vec::with_capacity(then.len());
    let mut seen = BTreeSet::new();
    let mut dropped = Vec::new();
    for t in then {
        if !g.contains(&t.account) {
            kept.push(t);
            continue;
        }
        seen.insert(t.id);
        let c = now.folder(t.id);
        match (touched.folders.get(&t.id), c) {
            // Not the change's: as it is now (gone from the server, or renamed there).
            (None, c) => kept.extend(c.cloned()),
            (Some(Some(a)), Some(c)) => {
                let mut f = c.clone();
                if t.name != a.name {
                    f.name = t.name.clone();
                }
                if t.parent != a.parent {
                    f.parent = t.parent;
                }
                if t.favorite != a.favorite {
                    f.favorite = t.favorite;
                }
                kept.push(f);
            }
            // Taken away by the change but put back since (the server refused).
            (Some(None), Some(c)) => kept.push(c.clone()),
            // Deleted for good by the change.
            (Some(None), None) if t.server.is_some() => said.folders.push(t.name.clone()),
            (Some(None), None) => kept.push(t),
            // Gone from the server since.
            (Some(Some(_)), None) => {}
        }
    }
    for c in now.folders.iter().filter(|f| g.contains(&f.account) && !seen.contains(&f.id)) {
        match touched.folders.get(&c.id) {
            // Made by the change and on the server already: into Deleted Items, with what is in it.
            Some(Some(_)) if c.server.is_some() => {
                let mut f = c.clone();
                if let Some(deleted) = now.folder_of_kind(c.account, FolderKind::Deleted).filter(|d| *d != c.id) {
                    f.parent = Some(deleted);
                }
                kept.push(f);
            }
            Some(Some(_)) => dropped.push(c.clone()),
            // Come down since.
            _ => kept.push(c.clone()),
        }
    }
    out.folders = kept;
    dropped
}

/// Whether `m` lies in its Microsoft account's Outbox.
fn in_outbox(m: &Message, now: &Mailbox, g: &BTreeSet<Id>) -> bool {
    g.contains(&m.account) && now.folder_of_kind(m.account, FolderKind::Outbox) == Some(m.folder)
}

/// The messages of the Microsoft accounts.
fn messages(out: &mut Mailbox, touched: &Touched, now: &Mailbox, g: &BTreeSet<Id>, said: &mut Said) {
    let mut ids: BTreeSet<Id> = touched.messages.keys().copied().collect();
    ids.extend(out.messages.iter().filter(|(_, m)| relevant(m, g)).map(|(id, _)| *id));
    ids.extend(now.messages.iter().filter(|(_, m)| relevant(m, g)).map(|(id, _)| *id));
    // The messages the change put in an Outbox: the drafts they were are sent, not undone.
    let sent: Vec<&Message> = touched.messages.values().flatten().map(AsRef::as_ref).filter(|m| in_outbox(m, now, g)).collect();
    for id in ids {
        let t = out.messages.get(&id).cloned();
        let c = now.messages.get(&id);
        if !t.as_deref().is_some_and(|m| relevant(m, g)) && !c.is_some_and(|m| relevant(m, g)) {
            continue;
        }
        let result = match touched.messages.get(&id) {
            // Not the change's: as it is now.
            None => c.cloned(),
            Some(a) => merge(t.as_deref(), a.as_deref(), c, now, g, &sent, said),
        };
        match result {
            Some(m) => {
                out.messages.insert(id, m);
            }
            None => {
                out.messages.remove(&id);
            }
        }
    }
}

/// One message the change touched: as it was before it (`t`), right after it (`a`) and now (`c`).
fn merge(
    t: Option<&Message>,
    a: Option<&Message>,
    c: Option<&Arc<Message>>,
    now: &Mailbox,
    g: &BTreeSet<Id>,
    sent: &[&Message],
    said: &mut Said,
) -> Option<Arc<Message>> {
    // Sending is never undone: what is in an Outbox, or went from it, stays as it is now.
    if [t, a, c.map(AsRef::as_ref)].into_iter().flatten().any(|m| in_outbox(m, now, g)) {
        if t.is_none()
            && let Some(m) = a
        {
            said.sent.push(m.subject.clone());
        }
        return c.cloned();
    }
    match (t, a, c) {
        (Some(t), None, None) => {
            // The draft that became a message sent.
            if !t.message_id.is_empty() && sent.iter().any(|m| m.account == t.account && m.message_id == t.message_id) {
                return None;
            }
            // Deleted for good by the change: the server's copy is gone, or going.
            if t.server.as_ref().is_some_and(|s| !s.gone) || t.moving_out.is_some() {
                said.deleted.push(t.subject.clone());
                return None;
            }
            // Never on the server: back, as ever.
            Some(Arc::new(t.clone()))
        }
        // Taken away by the server since, or made by the change and gone since.
        (_, Some(_), None) | (None, None, None) => None,
        // Taken away by the change and put back since (the server refused to delete it).
        (Some(_), None, Some(c)) => Some(c.clone()),
        // Made by the change.
        (None, Some(_), Some(c)) => {
            if now.queued.iter().any(|q| match q {
                Queued::CopyMessage { copy, .. } => *copy == c.id,
                Queued::PlaceMessage { message, .. } => *message == c.id,
                _ => false,
            }) {
                // A copy the server hasn't made: never made there.
                return None;
            }
            if !c.server.as_ref().is_some_and(|s| !s.gone) {
                return None;
            }
            // On the server already: into Deleted Items there and here, nothing deleted for good.
            let mut m = c.as_ref().clone();
            if let Some(deleted) = now.folder_of_kind(c.account, FolderKind::Deleted)
                && !now.subtree(deleted).contains(&c.folder)
            {
                m.folder = deleted;
            }
            Some(Arc::new(m))
        }
        (None, None, Some(c)) => Some(c.clone()),
        (Some(t), Some(a), Some(c)) => fields(t, a, c, now, g, said),
    }
}

/// A message here before the change, after it and now: as it is now, but for what the change
/// changed, which is as it was before.
fn fields(t: &Message, a: &Message, c: &Arc<Message>, now: &Mailbox, g: &BTreeSet<Id>, said: &mut Said) -> Option<Arc<Message>> {
    let mut m = c.as_ref().clone();
    if t.account != c.account {
        let placed = c.server.as_ref().filter(|s| !s.gone && !s.id.is_empty() && g.contains(&c.account));
        if g.contains(&t.account) && c.moving_out.as_ref().is_none_or(|out| out.account != t.account) {
            // Moved into another mailbox of the owner's, and the original removed (#70): moved
            // back as it went, a copy made there from the whole message, and this one gone once it is.
            let Some(placed) = placed.filter(|_| now.between_mailboxes(c.account, t.account)) else {
                said.moved_out.push(t.subject.clone());
                return Some(c.clone());
            };
            let source = mailcraft_model::sync::fingerprint(placed.id.as_bytes());
            said.queued.push(Queued::PlaceMessage {
                account: t.account,
                message: c.id,
                marker: format!("mailcraft-place-{:x}-u-{source:016x}", c.id),
                copy: false,
                from: None,
            });
            m.moving_out = Some(MovingOut { account: c.account, id: placed.id.clone(), folder: placed.folder.clone(), ..Default::default() });
            (m.server, m.uid, m.synced) = (None, 0, None);
        } else if g.contains(&t.account) {
            // Moved out of a Microsoft account: back while the server's copy is still there.
            let Some(out) = c.moving_out.clone().filter(|out| out.account == t.account && !out.left) else {
                said.moved_out.push(t.subject.clone());
                return Some(c.clone());
            };
            // A copy made already in the mailbox it went to goes again there (#70).
            if let Some(placed) = placed {
                said.queued.push(Queued::DeleteMessage { account: c.account, id: placed.id.clone(), kept: None });
            }
            m.server = t.server.clone().filter(|s| s.id == out.id).or_else(|| {
                Some(ServerMessage {
                    id: out.id.clone(),
                    read: t.read,
                    flag: t.flag,
                    flag_due: t.flag_due,
                    categories: t.categories.clone(),
                    importance: t.importance,
                    ..Default::default()
                })
            });
            (m.moving_out, m.uid, m.synced) = (None, 0, None);
            // Its run may be removing the server's copy: should that answer be lost, it is kept
            // here rather than forgotten.
            m.returned = RETURNED_LISTINGS;
        } else {
            // Moved back into the account it was being moved out of: out of it again.
            m.moving_out = c
                .server
                .as_ref()
                .filter(|s| !s.gone)
                .map(|s| MovingOut { account: c.account, id: s.id.clone(), folder: s.folder.clone(), ..Default::default() })
                // What is known now of the same original (where it is, whether it left) stands.
                .or_else(|| match (&t.moving_out, &c.moving_out) {
                    (Some(then), Some(now)) if now.account == then.account && now.id == then.id => Some(now.clone()),
                    (then, _) => then.clone(),
                });
            (m.server, m.uid, m.synced) = (None, t.uid, t.synced);
        }
        m.account = t.account;
    }
    macro_rules! back {
        ($($field:ident),*) => {
            $(if t.$field != a.$field {
                m.$field = t.$field.clone();
            })*
        };
    }
    back!(folder, read, flag, flag_due, importance, pinned, answered, forwarded, draft);
    // Categories as a list that others add to too (R3 spec S17): what the change added is taken
    // off and what it took off is put back; one added in Outlook meanwhile stays.
    if t.categories != a.categories {
        let has = |list: &[String], name: &String| list.iter().any(|c| same_category(c, name));
        m.categories.retain(|c| has(&t.categories, c) || !has(&a.categories, c));
        for c in t.categories.iter().filter(|c| !has(&a.categories, c)) {
            if !has(&m.categories, c) {
                m.categories.push(c.clone());
            }
        }
    }
    back!(subject, from, reply_to, to, cc, bcc, date, body, attachments, in_reply_to, references);
    if t.account != c.account {
        m.folder = t.folder;
    }
    // A draft written back to what it was goes up again as that.
    let differs = m.draft && DraftState::of(&m) != DraftState::of(c);
    if let Some(sd) = m.server_draft.as_mut().filter(|_| differs) {
        sd.pending = true;
        sd.saves = sd.saves.saturating_add(1);
    }
    Some(Arc::new(m))
}

/// Every message of a Microsoft account lies in a folder there is.
fn tidy(out: &mut Mailbox, now: &Mailbox, g: &BTreeSet<Id>) {
    let folders: BTreeSet<Id> = out.folders.iter().map(|f| f.id).collect();
    let parentless: Vec<usize> = out
        .folders
        .iter()
        .enumerate()
        .filter(|(_, f)| g.contains(&f.account) && f.parent.is_some_and(|p| !folders.contains(&p)))
        .map(|(i, _)| i)
        .collect();
    for i in parentless {
        if let Some(f) = out.folders.get_mut(i) {
            f.parent = now.folder(f.id).and_then(|c| c.parent).filter(|p| folders.contains(p));
        }
    }
    let homeless: Vec<Id> = out.messages.values().filter(|m| relevant(m, g) && !folders.contains(&m.folder)).map(|m| m.id).collect();
    for id in homeless {
        let Some(m) = out.messages.get(&id) else { continue };
        let home = now
            .message(id)
            .map(|c| c.folder)
            .filter(|f| folders.contains(f))
            .or_else(|| out.folder_of_kind(m.account, FolderKind::Inbox))
            .filter(|f| folders.contains(f));
        match home {
            Some(home) => {
                if let Some(m) = out.messages.get_mut(&id) {
                    Arc::make_mut(m).folder = home;
                }
            }
            // Nowhere at all (its account is gone): as before the undo.
            None => match now.messages.get(&id) {
                Some(c) => {
                    out.messages.insert(id, c.clone());
                }
                None => {
                    out.messages.remove(&id);
                }
            },
        }
    }
}

/// The queue of what the server is to be told: as it is now, less the copies taken away, with a
/// copy brought back that was never made there.
fn queue(out: &mut Mailbox, now: &Mailbox, g: &BTreeSet<Id>) {
    let then = std::mem::take(&mut out.queued);
    let mut queued: Vec<Queued> = then.iter().filter(|q| !g.contains(&q.account())).cloned().collect();
    queued.extend(now.queued.iter().filter(|q| g.contains(&q.account())).cloned());
    queued.retain(|q| match q {
        Queued::CopyMessage { copy, .. } => out.messages.contains_key(copy),
        // Made from the message where it lies: only while it lies there, not on a server yet (#70).
        Queued::PlaceMessage { account, message, .. } => out.messages.get(message).is_some_and(|m| m.account == *account && m.server.is_none()),
        _ => true,
    });
    for q in then.iter().filter(|q| g.contains(&q.account())) {
        if let Queued::CopyMessage { copy, .. } = q
            && out.messages.get(copy).is_some_and(|m| m.server.is_none())
            && !now.messages.contains_key(copy)
            && !queued.iter().any(|o| matches!(o, Queued::CopyMessage { copy: other, .. } if other == copy))
        {
            queued.push(q.clone());
        }
    }
    out.queued = queued;
}

#[cfg(test)]
#[path = "undo_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "undo_later_mail_tests.rs"]
mod later_mail_tests;
