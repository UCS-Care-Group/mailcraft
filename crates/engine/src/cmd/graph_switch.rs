// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Switching a Microsoft account kept in step by IMAP over to Microsoft Graph (R3.7).
//!
//! Each such account is asked once (the window shows a page; `account.graphSwitch` takes the
//! answer). Later keeps it on IMAP. Switch:
//!
//! 1. **What waits for the IMAP server goes first.** While anything does (mail in the Outbox, a
//!    flag, a move, a deletion), the account only notes that it is switching, and the host runs
//!    an ordinary Send/Receive; the switch waits, offline included, and happens with the first
//!    answer after one that left nothing waiting. Nothing is ever pushed by the switch itself.
//! 2. **A backup** of the whole mailbox, as it is at that moment, is written beside the mailbox
//!    file, with the date in its name, as one file that holds everything (`store::save_single`).
//!    Nothing is changed unless that worked.
//! 3. **The account becomes a Microsoft account** (`AccountKind::Microsoft`): no mail servers,
//!    its sign-in kept. Every message the IMAP server has is dropped here, to come down afresh
//!    through Graph; nothing is told to any server.
//! 4. **What is only here stays.** A draft that never went to the server stays in Drafts and goes
//!    up as a server draft with the next run (R3.4.1); the Outbox stays the Outbox, sent through
//!    Graph (R3.4.2). A folder that only ever was here, and any other message the server hasn't
//!    got, go to a local folder named after the account in the first account on this computer
//!    (one is made when there is none), since Graph can't put received mail on the server (S30).
//! 5. **The folders keep what was kept of them.** Each folder that was on the IMAP server stays,
//!    emptied and without its IMAP state, marked [`Folder::awaiting_listing`]: Graph's first
//!    complete listing matches it by path (a special folder by its kind, any other by its name
//!    under the folder it is in, as `graph_mail::adopt` does), so its favourite mark and the
//!    rules that file into it stay. It is never made on the server; one that the listing doesn't
//!    have goes ([`sweep_unlisted`]).

use std::collections::HashSet;

use mailcraft_model::{Account, AccountKind, Folder, FolderKind, GraphSwitch, Id, Mailbox, Message, clean_name};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.graphSwitch", "Switch to Microsoft Graph", "File › Info", graph_switch)
            .pure()
            .params(
                r#"{"account"?: number (default: the first Microsoft account still on IMAP), "answer": "switch"|"switchWithout"|"later"} — for a Microsoft account kept in step by IMAP. later: it stays on IMAP. switch: once nothing waits for the IMAP server (else answers how much does, and a Send/Receive goes first); switchWithout: at once, leaving what waits for the IMAP server in the backup (answers how many changes as `leftBehind`). Either backs the mailbox file up beside itself with the date in its name, and makes the account one kept in step through Microsoft Graph; its mail comes down afresh, what is only on this computer stays"#,
            )
            .when(has_imap_microsoft),
    ]
}

fn has_imap_microsoft(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(Account::on_imap_with_microsoft) { None } else { Some("no Microsoft account is kept in step by IMAP") }
}

/// How much of `account` waits to go to its IMAP server: what the switch waits for (R3.7). A
/// message held in the Outbox waits for its owner, not for a run.
pub fn waiting(s: &Session, account: Id) -> usize {
    let mb = &s.mailbox;
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox).map_or(0, |f| mb.messages_in(f).filter(|m| m.held.is_empty()).count());
    outbox.saturating_add(super::account::unpushed(s, account))
}

fn graph_switch(s: &mut Session, v: &Value) -> CmdResult {
    let account = match p::id(v, "account") {
        Some(id) => id,
        None => s
            .mailbox
            .accounts
            .iter()
            .find(|a| a.on_imap_with_microsoft())
            .map(|a| a.id)
            .ok_or_else(|| fail("no Microsoft account is kept in step by IMAP"))?,
    };
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !a.on_imap_with_microsoft() {
        return Err(failed!("{name} isn't a Microsoft account kept in step by IMAP", name = a.name));
    }
    // `switchWithout`: the owner chose to switch without what still waits for an IMAP server
    // that can't be reached (it is in the backup).
    let (answer, without) = match p::str(v, "answer").map(|a| a.trim().to_ascii_lowercase()) {
        Some(a) if a == "switch" => (GraphSwitch::Switching, false),
        Some(a) if a == "switchwithout" => (GraphSwitch::Switching, true),
        Some(a) if a == "later" => (GraphSwitch::Later, false),
        _ => return Err(bad("`answer` is switch, switchWithout or later")),
    };
    if s.mailbox.account(account).map(|a| a.graph_switch) != Some(answer) {
        s.mailbox.account_mut(account)?.graph_switch = answer;
        s.mailbox_changed();
    }
    if answer == GraphSwitch::Later {
        return Ok(json!({"account": account, "switched": false, "later": true}));
    }
    let waiting = waiting(s, account);
    if waiting > 0 && !without {
        return Ok(json!({"account": account, "switched": false, "waiting": waiting}));
    }
    // What won't reach the IMAP server now: the changes not pushed. (The Outbox stays, and is sent
    // through Graph.) All of it is in the backup, which is written first.
    let left_behind = left_behind(s, account);
    let backup = back_up(s, account)?;
    let done = switch(s, account)?;
    Ok(json!({
        "account": account,
        "switched": true,
        "backup": backup,
        "dropped": done.dropped,
        "drafts": done.drafts,
        "kept": done.kept,
        "keptIn": done.kept_in,
        "leftBehind": left_behind,
    }))
}

/// How many changes made in `account` would never reach its IMAP server if it switched now
/// (flags, moves and deletions not pushed); the Outbox isn't among them, it goes through Graph.
pub fn left_behind(s: &Session, account: Id) -> usize {
    super::account::unpushed(s, account)
}

/// Write the whole mailbox, as it is now, as one file beside the mailbox file, with today's date
/// in its name; answers where. In the copy, `account` is as it was before its owner was asked:
/// opened again, it is asked again rather than switched by itself. A mailbox that isn't kept in a
/// file isn't switched: there would be nowhere to keep the copy.
///
/// A copy the window already had written off its own thread for `account`
/// ([`Session::graph_switch_backup`]) is taken instead, while it is still there.
#[cfg(not(target_arch = "wasm32"))]
fn back_up(s: &mut Session, account: Id) -> Result<String, CmdError> {
    if let Some((_, path)) = s.graph_switch_backup.take().filter(|(of, path)| *of == account && path.is_file()) {
        return Ok(path.to_string_lossy().into_owned());
    }
    let (target, copy) = prepare_backup(s, account)?;
    crate::store::save_single(&target, &copy).map_err(|e| fail(copy_failed(&e)))?;
    Ok(target.to_string_lossy().into_owned())
}

#[cfg(target_arch = "wasm32")]
fn back_up(_: &mut Session, _: Id) -> Result<String, CmdError> {
    Err(fail("the browser can't keep a copy of the mailbox beside it: switch in the desktop app"))
}

/// Where the copy of the mailbox goes before `account` switches, and what goes in it, for a host
/// that writes it off the window's thread (then [`Session::graph_switch_backup`]). Nothing is
/// written here.
#[cfg(not(target_arch = "wasm32"))]
pub fn prepare_backup(s: &Session, account: Id) -> Result<(std::path::PathBuf, Mailbox), CmdError> {
    let path = s.path.clone().ok_or_else(|| fail("save the mailbox to a file first: the switch keeps a copy of it beside that file"))?;
    let target = backup_path(&path, &s.today().iso()).ok_or_else(|| fail("there is no free name for the copy of the mailbox beside it"))?;
    let mut copy = s.mailbox.clone();
    if let Ok(a) = copy.account_mut(account) {
        a.graph_switch = GraphSwitch::NotAsked;
    }
    Ok((target, copy))
}

/// Why nothing changed, when the copy couldn't be written.
pub fn copy_failed(why: &str) -> String {
    format!("the copy of the mailbox couldn't be written, so nothing was changed ({why})")
}

/// `mailbox (backup 2026-10-16).mailcraft` beside `mailbox.mailcraft`; with a number when that is
/// taken (`mailbox (backup 2026-10-16, 2).mailcraft`). `None` when every name tried is taken.
#[cfg(not(target_arch = "wasm32"))]
pub fn backup_path(path: &std::path::Path, date: &str) -> Option<std::path::PathBuf> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "mailbox".into());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).filter(|e| !e.is_empty()).unwrap_or_else(|| crate::io::MAILBOX_EXT.into());
    let dir = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    (1..=100)
        .map(|n| if n == 1 { format!("{stem} (backup {date}).{ext}") } else { format!("{stem} (backup {date}, {n}).{ext}") })
        .map(|name| dir.join(name))
        .find(|p| {
            // (Its content folder too: a copy that is one file must not be read with another's.)
            !p.exists() && !crate::store::blob_dir(p).exists()
        })
}

/// What a switch did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Switched {
    /// Messages dropped here, to come down afresh through Graph.
    pub dropped: usize,
    /// Drafts that never went to the IMAP server, now waiting to go up as server drafts.
    pub drafts: usize,
    /// Messages that only were here and went to a local folder, with those in the folders that
    /// only were here.
    pub kept: usize,
    /// The local folder they went to, when anything went.
    pub kept_in: Option<Id>,
}

/// Whether the IMAP server has `m` (it has a UID there, or is known to be there without one).
fn on_imap(m: &Message) -> bool {
    m.uid != 0 || m.synced.is_some()
}

/// Make `account`, a Microsoft account kept in step by IMAP, one kept in step through Graph
/// (the module's steps 3 to 5). Not an undo step: what the server has comes down afresh, and
/// what was here before is in the backup.
pub fn switch(s: &mut Session, account: Id) -> Result<Switched, CmdError> {
    let a = s.mailbox.account(account).cloned().ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !a.on_imap_with_microsoft() {
        return Err(failed!("{name} isn't a Microsoft account kept in step by IMAP", name = a.name));
    }
    let now = u64::try_from(s.now()).unwrap_or_default();
    let mb = &mut s.mailbox;
    let mut done = Switched::default();
    let ours: Vec<Folder> = mb.folders.iter().filter(|f| f.account == account).cloned().collect();
    let ids: HashSet<Id> = ours.iter().map(|f| f.id).collect();
    // The folders that only ever were here: what the IMAP server has no path for and that isn't
    // a special folder. The topmost of them go, with what is under them.
    let only_here = |f: &Folder| f.kind == FolderKind::Custom && f.remote.is_empty();
    let tops: Vec<Id> = ours
        .iter()
        .filter(|f| only_here(f) && !f.parent.and_then(|p| ours.iter().find(|g| g.id == p)).is_some_and(only_here))
        .map(|f| f.id)
        .collect();
    let mut leaving: HashSet<Id> = HashSet::new();
    for top in &tops {
        leaving.extend(mb.subtree(*top).into_iter().filter(|id| ids.contains(id)));
    }
    let drafts = mb.folder_of_kind(account, FolderKind::Drafts);
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    // Each message of the account: dropped (the server has it), kept as it is (the Outbox, a
    // draft never sent up), or taken to the local folder (anything else that only is here).
    let mut loose: Vec<Id> = Vec::new();
    let mut unsent: Vec<Id> = Vec::new();
    let mut drop: Vec<Id> = Vec::new();
    for m in mb.messages.values().filter(|m| m.account == account) {
        if on_imap(m) {
            drop.push(m.id);
        } else if leaving.contains(&m.folder) || Some(m.folder) == outbox {
            // Goes with its folder, or stays in the Outbox.
        } else if m.draft && drafts.is_some() {
            unsent.push(m.id);
        } else {
            loose.push(m.id);
        }
    }
    for id in &drop {
        mb.forget_message(*id).map_err(|e| fail(e.to_string()))?;
    }
    done.dropped = drop.len();
    // The drafts: in Drafts, waiting to go up as the server's (made there with a marker of their own).
    done.drafts = unsent.len();
    for id in &unsent {
        let m = mb.message_mut(*id).map_err(|e| fail(e.to_string()))?;
        if let Some(d) = drafts {
            m.folder = d;
        }
        m.server_draft =
            Some(mailcraft_model::sync::ServerDraft { marker: super::graph_drafts::marker(*id, now), pending: true, saves: 1, ..Default::default() });
    }
    // What only is here, in a local folder.
    let in_leaving = mb.messages.values().filter(|m| leaving.contains(&m.folder)).count();
    if !loose.is_empty() || !tops.is_empty() {
        let kept = kept_folder(mb, &a)?;
        for id in &loose {
            let m = mb.message_mut(*id).map_err(|e| fail(e.to_string()))?;
            m.folder = kept.folder;
        }
        for folder in mb.folders.iter_mut().filter(|f| leaving.contains(&f.id)) {
            folder.account = kept.account;
            if tops.contains(&folder.id) {
                folder.parent = Some(kept.folder);
            }
        }
        for m in mb.messages.values_mut().filter(|m| m.folder == kept.folder || leaving.contains(&m.folder)) {
            let m = std::sync::Arc::make_mut(m);
            m.account = kept.account;
        }
        done.kept_in = Some(kept.folder);
    }
    done.kept = loose.len() + in_leaving;
    // The folders that stay: emptied, without IMAP's state, waiting for Graph's listing.
    for f in mb.folders.iter_mut().filter(|f| f.account == account) {
        let was_on_server = !f.remote.is_empty();
        f.remote.clear();
        f.uid_validity = 0;
        f.uid_seen = 0;
        f.uid_from = 0;
        f.server = None;
        f.awaiting_listing = f.kind == FolderKind::Custom && was_on_server;
    }
    // Nothing the IMAP server was still to be told is told anybody now.
    mb.pending.retain(|p| p.account() != account);
    let a = mb.account_mut(account).map_err(|e| fail(e.to_string()))?;
    a.kind = AccountKind::Microsoft;
    (a.incoming, a.outgoing) = (None, None);
    a.graph_switch = GraphSwitch::NotAsked;
    // What the session kept of the messages that went.
    let mb = &s.mailbox;
    s.sel.messages.retain(|id| mb.messages.contains_key(id));
    if let Some(c) = s.compose.as_mut()
        && c.draft.is_some_and(|d| !mb.messages.contains_key(&d))
    {
        c.draft = None;
    }
    if mb.folder(s.view.folder).is_none() {
        s.view.folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap_or(0);
    }
    s.graph_index = Default::default();
    s.mailbox_changed();
    s.forget_history();
    Ok(done)
}

/// Where the local folder for what only was here of `of` is.
struct Kept {
    account: Id,
    folder: Id,
}

/// The local folder `Kept from <address>` at the top of the first account on this computer (one
/// called "On this computer" is made when the mailbox has none), made for the switch.
fn kept_folder(mb: &mut Mailbox, of: &Account) -> Result<Kept, CmdError> {
    let local = mb.local_account();
    let base = clean_name(&format!("Kept from {}", if of.email.trim().is_empty() { of.name.as_str() } else { of.email.trim() }))
        .unwrap_or_else(|| "Kept from the switch".into());
    // (A slash parts names of a folder on this computer; an address has none, a name may.)
    let base = base.replace('/', "-");
    let taken = |mb: &Mailbox, name: &str| mb.folders.iter().any(|f| f.account == local && f.parent.is_none() && f.name.eq_ignore_ascii_case(name));
    let name = (1..=100)
        .map(|n| if n == 1 { base.clone() } else { format!("{base} ({n})") })
        .find(|name| !taken(mb, name))
        .ok_or_else(|| fail("there is no free name for a local folder"))?;
    let folder = mb.add_folder(local, None, &name).map_err(|e| fail(e.to_string()))?;
    Ok(Kept { account: local, folder })
}

/// After a complete listing of `account`: a folder that was on its IMAP server and that the
/// listing didn't have is no folder of the server's (it was deleted there since, or Graph lists
/// it differently), and goes, with what is under it that wasn't found either. One that holds mail
/// here (nothing should) isn't removed: it becomes a folder made here. How many went.
pub fn sweep_unlisted(mb: &mut Mailbox, account: Id) -> usize {
    let unlisted: Vec<Id> = mb.folders.iter().filter(|f| f.account == account && f.awaiting_listing && f.server.is_none()).map(|f| f.id).collect();
    if unlisted.is_empty() {
        return 0;
    }
    let mut gone: Vec<Id> = Vec::new();
    for id in &unlisted {
        let subtree = mb.subtree(*id);
        let empty = subtree.iter().all(|f| mb.count(*f) == 0);
        let all_unlisted = subtree.iter().all(|f| mb.folder(*f).is_some_and(|f| f.awaiting_listing && f.server.is_none()));
        if empty && all_unlisted {
            gone.push(*id);
        }
    }
    for id in &unlisted {
        if !gone.contains(id)
            && let Ok(f) = mb.folder_mut(*id)
        {
            f.awaiting_listing = false;
        }
    }
    mb.remove_folders(&gone);
    gone.len()
}

#[cfg(test)]
#[path = "graph_switch_tests.rs"]
mod tests;
