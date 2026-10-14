//! Folders and the mail server: telling the server what the owner did to folders here
//! ([`FolderChange`] → [`FolderOp`]), taking in what it made of that, and letting go of the
//! folders the server no longer has.
//!
//! The server's list of folders is the truth about which folders it has. A folder here that
//! follows a mailbox missing from a whole list ([`SyncResult::listed_whole`]) is gone from the
//! server: deleted there, emptied out of Deleted Items, or renamed into another one that comes
//! down as a folder of its own. What came from the server goes with it; what only exists here
//! (messages moved in and not pushed yet, a change still to push, folders inside it that stay)
//! is kept, in a folder that only exists here. A list that may be short removes nothing.

use std::collections::HashSet;

use mailcraft_model::sync::{FolderOp, FolderPushed, ListedFolder};
use mailcraft_model::{FolderChange, FolderKind, Id, Mailbox};

/// What the server is to be told of the folders of `account`: one op for each change waiting
/// whose folder is where the server can follow it. A folder inside one that has no mailbox on
/// the server yet waits until that one has.
pub(crate) fn folder_ops(mb: &Mailbox, account: Id) -> Vec<FolderOp> {
    // The server path of the folder `parent` is in (empty: the top level); `None` when that
    // folder has no mailbox there yet.
    let place = |parent: Option<Id>| match parent {
        None => Some(String::new()),
        Some(p) => mb.folder(p).filter(|p| p.account == account && !p.remote.is_empty()).map(|p| p.remote.clone()),
    };
    let mut ops = Vec::new();
    for change in mb.folder_changes.iter().filter(|c| c.account() == account) {
        match change {
            FolderChange::Create { folder, .. } => {
                let Some(f) = mb.folder(*folder).filter(|f| f.account == account && f.remote.is_empty() && f.kind == FolderKind::Custom) else {
                    continue;
                };
                let Some(parent) = place(f.parent) else { continue };
                ops.push(FolderOp::Create { folder: f.id, parent, name: f.name.clone() });
            }
            FolderChange::Rename { folder, from, parent: was_in, .. } => {
                let Some(f) = mb.folder(*folder).filter(|f| f.account == account && f.remote == *from && f.kind == FolderKind::Custom) else {
                    continue;
                };
                // Still in the folder it was in: it keeps its place on the server, whatever that is.
                let parent = if f.parent == *was_in {
                    None
                } else {
                    match place(f.parent) {
                        Some(parent) => Some(parent),
                        None => continue,
                    }
                };
                ops.push(FolderOp::Rename { folder: f.id, from: from.clone(), parent, name: f.name.clone() });
            }
            FolderChange::Delete { remote, .. } => ops.push(FolderOp::Delete { remote: remote.clone() }),
        }
    }
    ops
}

/// Take in what the server made of the folder changes: each answered change leaves the list,
/// done or refused (a refused one stays as it is here only, and the owner was told why). A
/// folder made there follows its mailbox now; a rename renames the mailboxes inside it too.
/// Returns whether the server took on any of them.
pub(crate) fn folders_pushed(mb: &mut Mailbox, account: Id, pushed: &[FolderPushed]) -> bool {
    let mut took = false;
    for done in pushed {
        let answers = |c: &FolderChange| {
            c.account() == account
                && match c {
                    FolderChange::Create { folder, .. } => done.folder != 0 && done.from.is_empty() && *folder == done.folder,
                    FolderChange::Rename { folder, from, .. } => done.folder != 0 && *folder == done.folder && *from == done.from,
                    FolderChange::Delete { remote, .. } => done.folder == 0 && *remote == done.from,
                }
        };
        let Some(at) = mb.folder_changes.iter().position(answers) else { continue };
        let change = mb.folder_changes.remove(at);
        if done.refused || done.to.is_empty() {
            took |= !done.refused;
            continue;
        }
        match change {
            FolderChange::Create { folder, .. } => {
                // A mailbox no other folder here follows.
                gone_from_server(mb, account, followers(mb, account, &done.to, "", folder));
                if let Ok(f) = mb.folder_mut(folder)
                    && f.remote.is_empty()
                {
                    f.remote = done.to.clone();
                    took = true;
                }
            }
            FolderChange::Rename { folder, from, .. } if from != done.to => {
                // Folders here that followed what is now its name were stale: the server had no
                // mailbox by that name, or it would have refused.
                gone_from_server(mb, account, followers(mb, account, &done.to, &done.delimiter, folder));
                mb.remote_renamed(account, &from, &done.to, &done.delimiter);
                took = true;
            }
            // Its name and place were already the server's: nothing changed there.
            FolderChange::Rename { .. } => {}
            FolderChange::Delete { .. } => took = true,
        }
    }
    took
}

/// The folders of `account` other than `except` (and those inside it) that follow `remote` or a
/// mailbox inside it.
fn followers(mb: &Mailbox, account: Id, remote: &str, delimiter: &str, except: Id) -> Vec<Id> {
    let mine = mb.subtree(except);
    mb.folders
        .iter()
        .filter(|f| f.account == account && !mine.contains(&f.id) && !f.remote.is_empty())
        .filter(|f| mailcraft_model::sync::renamed_path(&f.remote, remote, remote, delimiter).is_some())
        .map(|f| f.id)
        .collect()
}

/// Let go of the folders of `account` that follow a mailbox the server's whole list doesn't
/// have ([`gone_from_server`]). Returns the folders that were let go of.
pub(crate) fn forget_unlisted(mb: &mut Mailbox, account: Id, listed: &[ListedFolder]) -> Vec<Id> {
    let listed: HashSet<&str> = listed.iter().map(|f| f.remote.as_str()).collect();
    let gone: Vec<Id> =
        mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty() && !listed.contains(f.remote.as_str())).map(|f| f.id).collect();
    gone_from_server(mb, account, gone.clone());
    gone
}

/// The mailboxes the folders `gone` followed are no longer on the server. What came from there
/// goes: its messages, and the changes still to push about them. A folder the owner made that is
/// left with nothing goes too; a standard folder, or one that still holds what only exists here
/// (messages moved in and not pushed yet, folders inside it that stay, a change of its own still
/// to push), stays and follows no mailbox: the next synchronisation gives a standard folder the
/// server's mailbox of its kind again, and makes one for a folder whose messages wait to go up.
pub(crate) fn gone_from_server(mb: &mut Mailbox, account: Id, mut gone: Vec<Id>) {
    if gone.is_empty() {
        return;
    }
    // The innermost first: a folder is left with nothing once the folders inside it are gone.
    gone.sort_by_cached_key(|id| std::cmp::Reverse(mb.folder_path(*id).split('/').count()));
    let changing: HashSet<Id> = mb.folder_changes.iter().filter(|c| c.account() == account).filter_map(FolderChange::folder).collect();
    for id in gone {
        let Some(f) = mb.folder(id).filter(|f| f.account == account) else { continue };
        let (remote, custom) = (f.remote.clone(), f.kind == FolderKind::Custom);
        // The server's copies went with the mailbox. In a standard folder, a message waiting for
        // its copy there (one that was sent) waits no more: the one here is the one there is.
        let theirs: Vec<Id> = mb.messages_in(id).filter(|m| m.uid != 0 || (custom && m.synced.is_some())).map(|m| m.id).collect();
        for message in theirs {
            let _ = mb.forget_message(message);
        }
        let waiting: Vec<Id> = mb.messages_in(id).filter(|m| m.synced.is_some()).map(|m| m.id).collect();
        for message in waiting {
            if let Ok(m) = mb.message_mut(message) {
                m.synced = None;
            }
        }
        mb.pending.retain(|p| !(p.account() == account && p.source().0 == remote));
        let empty = mb.messages_in(id).next().is_none() && !mb.folders.iter().any(|c| c.parent == Some(id));
        if custom && empty && !changing.contains(&id) && mb.forget_folder(id).is_ok() {
            continue;
        }
        if let Ok(f) = mb.folder_mut(id) {
            f.remote.clear();
            f.uid_validity = 0;
            f.uid_seen = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::sync::{FolderOp, FolderPushed, ListedFolder, SyncResult};
    use mailcraft_model::{FolderKind, Id, Mailbox, Message};
    use serde_json::json;

    use crate::Session;
    use crate::cmd::account::{apply_sync, sync_request};

    /// An account with a server: its Inbox and Deleted Items follow mailboxes there, and so does
    /// "Projects", with a message from the server in it.
    fn session() -> (Session, Id, Id, Id) {
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
        let a = added["id"].as_u64().unwrap();
        for (kind, remote) in [(FolderKind::Inbox, "INBOX"), (FolderKind::Deleted, "Deleted Items")] {
            let id = s.mailbox.folder_of_kind(a, kind).unwrap();
            let f = s.mailbox.folder_mut(id).unwrap();
            (f.remote, f.uid_validity) = (remote.into(), 7);
        }
        let projects = s.mailbox.add_folder(a, None, "Projects").unwrap();
        let f = s.mailbox.folder_mut(projects).unwrap();
        (f.remote, f.uid_validity, f.uid_seen) = ("Projects".into(), 31, 1);
        let m = s.mailbox.add_message(Message { folder: projects, uid: 1, subject: "Project one".into(), ..Default::default() }).unwrap();
        (s, a, projects, m)
    }

    fn listed(remotes: &[&str]) -> Vec<ListedFolder> {
        remotes.iter().map(|remote| ListedFolder { remote: remote.to_string(), name: remote.to_string(), ..Default::default() }).collect()
    }

    #[test]
    fn folder_commands_ask_the_server_for_the_same() {
        let (mut s, a, projects, _) = session();
        s.run("folder.rename", &json!({"id": projects, "name": "Plans"})).unwrap();
        assert_eq!(
            sync_request(&s, a).unwrap().folder_ops,
            [FolderOp::Rename { folder: projects, from: "Projects".into(), parent: None, name: "Plans".into() }]
        );
        s.run("folder.delete", &json!({"id": projects})).unwrap();
        assert_eq!(
            sync_request(&s, a).unwrap().folder_ops,
            [FolderOp::Rename { folder: projects, from: "Projects".into(), parent: Some("Deleted Items".into()), name: "Plans".into() }]
        );
        let made = s.run("folder.new", &json!({"name": "Q3", "parent": null, "account": a})).unwrap()["id"].as_u64().unwrap();
        let ops = sync_request(&s, a).unwrap().folder_ops;
        assert_eq!(ops.get(1), Some(&FolderOp::Create { folder: made, parent: String::new(), name: "Q3".into() }));
        // Undone before the server heard: nothing to tell it.
        for _ in 0..3 {
            s.run("edit.undo", &json!({})).unwrap();
        }
        assert!(sync_request(&s, a).unwrap().folder_ops.is_empty());
    }

    #[test]
    fn only_a_whole_list_takes_folders_for_gone() {
        let (mut s, a, projects, m) = session();
        let without = |whole: bool, remotes: &[&str]| SyncResult { account: a, listed: listed(remotes), listed_whole: whole, ..Default::default() };
        apply_sync(&mut s, without(false, &["INBOX", "Deleted Items"])).unwrap();
        apply_sync(&mut s, without(true, &[])).unwrap();
        assert!(s.mailbox.folder(projects).is_some() && s.mailbox.message(m).is_some());
        apply_sync(&mut s, without(true, &["INBOX", "Deleted Items"])).unwrap();
        assert!(s.mailbox.folder(projects).is_none() && s.mailbox.message(m).is_none());
        assert!(s.mailbox.clone().repair().is_empty());
    }

    #[test]
    fn whatever_a_server_says_of_folder_changes_nothing_breaks() {
        let (mut s, a, projects, _) = session();
        s.run("folder.rename", &json!({"id": projects, "name": "Plans"})).unwrap();
        let junk = [
            FolderPushed { folder: 999, from: "Projects".into(), to: "X".into(), ..Default::default() },
            FolderPushed { folder: projects, from: "Elsewhere".into(), to: "Y".into(), delimiter: "/".into(), refused: false },
            FolderPushed { folder: 0, from: "INBOX".into(), ..Default::default() },
            FolderPushed { folder: projects, from: String::new(), to: "INBOX".into(), ..Default::default() },
        ];
        apply_sync(&mut s, SyncResult { account: a, folders_pushed: junk.to_vec(), ..Default::default() }).unwrap();
        assert_eq!(s.mailbox.folder(projects).map(|f| f.remote.as_str()), Some("Projects"));
        assert_eq!(s.mailbox.folder_changes.len(), 1);
        // The answer that fits: the folder follows the new name, and nothing is left to tell.
        let done = FolderPushed { folder: projects, from: "Projects".into(), to: "Plans".into(), delimiter: "/".into(), refused: false };
        apply_sync(&mut s, SyncResult { account: a, folders_pushed: vec![done], ..Default::default() }).unwrap();
        assert_eq!(s.mailbox.folder(projects).map(|f| f.remote.as_str()), Some("Plans"));
        assert!(s.mailbox.folder_changes.is_empty() && !s.can_undo());
        assert!(s.mailbox.clone().repair().is_empty());
    }
}
