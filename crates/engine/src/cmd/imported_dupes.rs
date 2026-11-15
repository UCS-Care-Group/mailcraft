// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail brought in twice (#1401): what makes two messages the same mail, so that an import skips
//! a message already in the folder it goes to (or the folder it was meant for), and the clean-up
//! that removes the copies an import made, before imports did that, beside the mail a server has.
//!
//! **The same mail.** The same Message-ID, and the same date, sender, subject and To/Cc people
//! ([`MailIdentity`], the identity a renumbered IMAP mailbox's changes are matched by), compared
//! whole. A message with no Message-ID, or no date, is never the same as another: the sender's
//! other fields alone could make two different messages one (a notification sent twice in a
//! second), and an import would leave one out, so such a message is always imported, and never
//! removed as a copy. Only a copy here whole stands for the mail ([`whole`]): one that is only its
//! headers, couldn't be read back, or had a part taken out doesn't keep a backup's full copy out.
//!
//! **Where an import looks.** Only in the folder the mail lands in and, when it was sent to
//! Imported instead, the folder it was meant for: never anywhere else, so that a folder exported
//! and imported into an archive of this computer's (before the server's copies are deleted) comes
//! in whole.
//!
//! **The clean-up.** `mail.removeImportedDuplicates` removes a message that only this computer has
//! when the same folder holds the same mail as its server has it. It never touches the server's
//! copy, and tells the server nothing. Only a message that nothing waits on is a copy to remove: a
//! draft, a message in an Outbox or being sent, one being moved into or out of an account
//! ([`Pending::Move`], [`Queued`]), one waiting for its server's copy to be matched (`synced` with
//! no UID), and one that is the server's under any kind of id (an IMAP UID, the id a Microsoft,
//! JMAP or Exchange server gave it) are left as they are. So is a copy the owner changed: its
//! flag, follow-up date, categories, pin, snooze or read mark differ from the server's, or a to-do
//! is made from it. Those are counted as kept, and nothing is carried over to the server's copy.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use mailcraft_model::{FolderKind, Id, MailIdentity, Mailbox, Message, Pending, Queued};
use serde_json::{Value, json};

use super::fail;
use crate::{CmdResult, CommandSpec, Session, p};

/// Most messages one clean-up names in its answer; the rest are counted.
pub const MAX_LISTED: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("mail.removeImportedDuplicates", "Remove Imported Duplicates", "Folder › Clean Up", remove).params(PARAMS)]
}

const PARAMS: &str = r#"{"folder"?: number (one folder; default every folder), "preview"?: bool (only count, change nothing)} — removes messages that only this computer has (imported from a file, nothing waiting to go to a server) when the same folder holds the same mail from its server (same Message-ID, date, sender, subject and To/Cc). A copy whose flag, follow-up date, categories, pin, snooze or read mark differ from the server's, or that a to-do was made from, is kept and counted (`kept`). The server's copy is never touched and the server is told nothing; undo puts them back until the next Send/Receive → {"removed": number, "ids": [number] (at most 500), "kept": number, "preview": bool}"#;

/// What makes a message the same mail as another: its Message-ID, and a digest of the rest of its
/// [`MailIdentity`] (a quick look-up; a hit is compared whole, [`Twins`]).
pub type Key = (String, u64);

/// What makes `m` the same mail as another ([`Key`]); `None` for a message with no Message-ID or
/// no date, which is never taken for another.
pub fn identity(m: &Message) -> Option<Key> {
    if m.message_id.is_empty() || m.date == 0 {
        return None;
    }
    Some((m.message_id.clone(), digest(&MailIdentity::of(m))))
}

fn digest(id: &MailIdentity) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (id.date, &id.from, &id.subject, &id.people).hash(&mut h);
    h.finish()
}

/// Whether `m` is here whole, as a backup's copy would be: not only its headers and preview
/// (`unfetched`, too large to download, its server's copy gone before it came), its text read back,
/// and no part taken out of it. The same test a move onto a server makes of what it puts there.
pub fn whole(m: &Message) -> bool {
    !m.unfetched && !m.incomplete && m.not_downloaded == 0 && !m.kept.as_deref().is_some_and(|k| k.failed) && !m.parts_removed
}

/// The mail an import leaves out, by what it is ([`identity`]), compared whole on a hit.
#[derive(Default)]
pub struct Twins {
    by_key: HashMap<Key, Vec<MailIdentity>>,
}

impl Twins {
    /// The whole messages ([`whole`]) of `folders`.
    pub fn of(mb: &Mailbox, folders: &[Id]) -> Twins {
        let mut t = Twins::default();
        for f in folders {
            for m in mb.messages_in(*f).filter(|m| whole(m)) {
                t.add(m);
            }
        }
        t
    }

    /// Whether the same mail as `m` is here.
    pub fn holds(&self, m: &Message) -> bool {
        let Some(key) = identity(m) else { return false };
        self.by_key.get(&key).is_some_and(|all| all.contains(&MailIdentity::of(m)))
    }

    /// Whether mail with the key `key` may be here (no message to compare it whole with: a count
    /// for a preview alone).
    pub fn may_hold(&self, key: &Key) -> bool {
        self.by_key.contains_key(key)
    }

    /// `m` is here from now on. A message with no identity is never added.
    pub fn add(&mut self, m: &Message) {
        if let Some(key) = identity(m) {
            let all = self.by_key.entry(key).or_default();
            let id = MailIdentity::of(m);
            if !all.contains(&id) {
                all.push(id);
            }
        }
    }

    /// Only the key of a message (a preview, which keeps no messages).
    pub fn add_key(&mut self, key: Key) {
        self.by_key.entry(key).or_default();
    }
}

/// Whether the server has `m` under an id of its own: an IMAP UID, or the id a Microsoft, JMAP
/// or Exchange server gave it (and hasn't said is gone).
fn on_server(m: &Message) -> bool {
    m.uid != 0 || m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty())
}

/// Whether `m` is only this computer's and nothing waits on it: no id of a server's, no copy on
/// a server waiting to be matched to it, nothing to be sent or made of it, not a draft.
fn only_here(m: &Message) -> bool {
    m.uid == 0
        && m.server.is_none()
        && m.synced.is_none()
        && m.placed.is_empty()
        && m.moving_out.is_none()
        && m.server_draft.is_none()
        && m.answers.is_none()
        && !m.draft
        && !m.sending
        && !m.sent_here
        && m.held.is_empty()
        && m.returned == 0
        && m.original_placed == 0
}

/// What the owner may have changed of a message, which the clean-up must not lose.
fn marks(m: &Message) -> (bool, &mailcraft_model::FlagStatus, Option<i64>, Vec<&str>, bool, Option<mailcraft_model::Snooze>) {
    let mut categories: Vec<&str> = m.categories.iter().map(String::as_str).collect();
    categories.sort_unstable();
    (m.read, &m.flag, m.flag_due, categories, m.pinned, m.snooze)
}

/// What the clean-up finds: the copies it removes, sorted by id, and how many it keeps because
/// the owner changed them.
#[derive(Debug, Default)]
pub struct Found {
    pub remove: Vec<Id>,
    pub kept: usize,
}

/// The messages the clean-up removes: in `folder` (or every folder), only this computer's
/// ([`only_here`]), with nothing waiting on them in any of the mailbox's queues, and the same mail
/// ([`identity`]) as a message of the same folder that its server has, with the same marks
/// ([`marks`]) and no to-do made from it. Never in an Outbox or a Drafts folder.
pub fn twins(mb: &Mailbox, folder: Option<Id>) -> Found {
    let in_scope = |f: Id| folder.is_none_or(|want| want == f);
    // What waits on a message, in every queue that names one, and the messages to-dos are made from.
    let mut waited: HashSet<Id> = HashSet::new();
    for p in &mb.pending {
        if let Pending::Move { message, .. } = p {
            waited.insert(*message);
        }
    }
    for q in &mb.queued {
        match q {
            Queued::PlaceMessage { message, .. } => {
                waited.insert(*message);
            }
            Queued::CopyMessage { copy, .. } => {
                waited.insert(*copy);
            }
            Queued::DeleteFolder { .. } | Queued::DeleteMessage { .. } => {}
        }
    }
    let tasked: HashSet<Id> = mb.tasks.values().filter_map(|t| t.message).collect();
    // The server's mail of each folder, by Message-ID, compared whole (what is removed must be the
    // same mail, not one whose digest happens to agree). Not a copy the server lost before it came
    // whole here, nor one whose text couldn't be read back: the copy here may be all there is.
    let mut theirs: HashMap<(Id, &str), Vec<&Message>> = HashMap::new();
    for m in mb.messages.values().filter(|m| in_scope(m.folder) && on_server(m) && identity(m).is_some()) {
        if m.incomplete || m.parts_removed || m.kept.as_deref().is_some_and(|k| k.failed) {
            continue;
        }
        theirs.entry((m.folder, m.message_id.as_str())).or_default().push(m.as_ref());
    }
    let mut found = Found::default();
    if theirs.is_empty() {
        return found;
    }
    let kept_out = |f: Id| mb.is_send_queue(f) || mb.folder(f).is_none_or(|f| f.kind == FolderKind::Drafts);
    for m in mb.messages.values().filter(|m| only_here(m) && !waited.contains(&m.id) && identity(m).is_some() && !kept_out(m.folder)) {
        let Some(all) = theirs.get(&(m.folder, m.message_id.as_str())) else { continue };
        let me = MailIdentity::of(m);
        let same: Vec<&&Message> = all.iter().filter(|t| MailIdentity::of(t) == me).collect();
        if same.is_empty() {
            continue;
        }
        if !tasked.contains(&m.id) && same.iter().any(|t| marks(t) == marks(m)) {
            found.remove.push(m.id);
        } else {
            found.kept = found.kept.saturating_add(1);
        }
    }
    found.remove.sort_unstable();
    found
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["folder", "preview"])?;
    let preview = p::opt_bool(v, "preview")?.unwrap_or(false);
    let folder = p::id_strict(v, "folder")?;
    if let Some(f) = folder
        && s.mailbox.folder(f).is_none()
    {
        return Err(failed!("no folder with id {id}", id = f));
    }
    let Found { remove: ids, kept } = twins(&s.mailbox, folder);
    if !preview {
        // Taken out without a word to any server: none of them is a server's.
        for id in &ids {
            s.mailbox.forget_message(*id).map_err(|e| fail(e.to_string()))?;
        }
        if !ids.is_empty() {
            let gone: HashSet<Id> = ids.iter().copied().collect();
            s.sel.messages.retain(|m| !gone.contains(m));
            s.sel.whole.retain(|m| !gone.contains(m));
            if s.sel.focus_row.is_some_and(|(m, _)| gone.contains(&m)) {
                s.sel.focus_row = None;
            }
            if s.sel.anchor_row.is_some_and(|(m, _)| gone.contains(&m)) {
                s.sel.anchor_row = None;
            }
            s.list_due();
        }
        log::info!("imported duplicates: {} removed, {kept} kept because they differ", ids.len());
    }
    let listed: Vec<Id> = ids.iter().copied().take(MAX_LISTED).collect();
    Ok(json!({"removed": ids.len(), "ids": listed, "kept": kept, "preview": preview}))
}
