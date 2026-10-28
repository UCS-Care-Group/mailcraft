// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Switching a Microsoft account kept in step by IMAP over to Microsoft Graph (R3.7), as the
//! engine does it. (From end to end, against the fake IMAP server and the Graph stand-in:
//! `apps/mailcraft-cli/tests/graph_switch.rs`.)

use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, RemoteFlags};
use mailcraft_model::{FlagStatus, Pending, Server};
use serde_json::json;

use super::*;
use crate::cmd::graph_mail::{apply_graph_mail, graph_mail_request};

const OWNER: &str = "owner@contoso.example";

/// A directory of the test's own, made empty.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-r37-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Ids {
    account: Id,
    local: Id,
    inbox: Id,
    drafts: Id,
    projects: Id,
    old: Id,
    notes: Id,
    on_server: Id,
    server_draft: Id,
    unsent: Id,
    loose: Id,
    in_notes: Id,
    local_message: Id,
}

fn message(folder: Id, subject: &str, uid: u32) -> Message {
    Message {
        folder,
        subject: subject.into(),
        message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
        uid,
        synced: (uid != 0).then_some(RemoteFlags { uid, ..Default::default() }),
        ..Default::default()
    }
}

/// A mailbox as 0.2.1 kept it: a local account with a folder of its own, and a Microsoft account
/// on IMAP whose folders the server has (Inbox, Drafts, Projects marked a favourite, Old) beside
/// one that only is here (Notes), with mail the server has, a draft it has, a draft it hasn't and
/// a message it hasn't.
fn mailbox() -> (Session, Ids) {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let local = mb.accounts[0].id;
    let local_folder = mb.add_folder(local, None, "Recipes").unwrap();
    let local_message = mb.add_message(message(local_folder, "Lentil soup", 0)).unwrap();
    let server = |host: &str| Some(Server { host: host.into(), port: 993, ..Default::default() });
    let account = mb.add_account(Account {
        email: OWNER.into(),
        kind: AccountKind::Imap,
        incoming: server("outlook.office365.com"),
        outgoing: server("smtp.office365.com"),
        oauth: Some(mailcraft_model::OAuth::default()),
        ..Default::default()
    });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let drafts = mb.folder_of_kind(account, FolderKind::Drafts).unwrap();
    for (id, remote) in [(inbox, "INBOX"), (drafts, "Drafts")] {
        let f = mb.folder_mut(id).unwrap();
        (f.remote, f.uid_validity, f.uid_seen) = (remote.into(), 7, 9);
    }
    let projects = mb.add_folder(account, Some(inbox), "Projects").unwrap();
    let old = mb.add_folder(account, None, "Old").unwrap();
    for (id, remote) in [(projects, "INBOX/Projects"), (old, "Old")] {
        let f = mb.folder_mut(id).unwrap();
        (f.remote, f.uid_validity) = (remote.into(), 3);
    }
    mb.folder_mut(projects).unwrap().favorite = true;
    let notes = mb.add_folder(account, Some(inbox), "Notes").unwrap();
    let flagged = Some(RemoteFlags { uid: 5, flagged: true, ..Default::default() });
    let on_server = mb.add_message(Message { flag: FlagStatus::Flagged, synced: flagged, ..message(inbox, "Quarterly figures", 5) }).unwrap();
    let server_draft = mb.add_message(Message { draft: true, ..message(drafts, "Written elsewhere", 7) }).unwrap();
    let unsent = mb.add_message(Message { draft: true, ..message(drafts, "Not sent yet", 0) }).unwrap();
    let loose = mb.add_message(message(inbox, "Brought in by hand", 0)).unwrap();
    let in_notes = mb.add_message(message(notes, "A note", 0)).unwrap();
    let mut s = Session::at(mb, 1_800_000_000, 0);
    s.view.folder = projects;
    (s, Ids { account, local, inbox, drafts, projects, old, notes, on_server, server_draft, unsent, loose, in_notes, local_message })
}

fn answer(s: &mut Session, account: Id, answer: &str) -> Result<Value, CmdError> {
    s.run("account.graphSwitch", &json!({"account": account, "answer": answer}))
}

fn graph_folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

fn listed(s: &mut Session, account: Id, folders: Vec<GraphFolder>, complete: bool) {
    let result = GraphMailResult { account, email: OWNER.into(), listing: Some(FolderListing { folders, complete }), ..Default::default() };
    apply_graph_mail(s, result).unwrap();
}

#[test]
fn later_keeps_the_account_on_imap() {
    let (mut s, ids) = mailbox();
    let before = s.mailbox.clone();
    let said = answer(&mut s, ids.account, "later").unwrap();
    assert_eq!(said["later"], json!(true));
    let a = s.mailbox.account(ids.account).unwrap();
    assert_eq!((a.kind, a.graph_switch), (AccountKind::Imap, GraphSwitch::Later));
    // Nothing else is any different, and the answer is kept with the account in the file.
    let mut was = before;
    was.account_mut(ids.account).unwrap().graph_switch = GraphSwitch::Later;
    assert_eq!(s.mailbox, was);
    let written = String::from_utf8(crate::io::write_mailbox(&s.mailbox).unwrap()).unwrap();
    assert!(written.contains(r#""graphSwitch":"later""#), "{written}");
    assert_eq!(crate::io::read_mailbox(written.as_bytes()).unwrap().account(ids.account).unwrap().graph_switch, GraphSwitch::Later);
    // A file that says something else there opens as not asked.
    let odd = written.replace(r#""graphSwitch":"later""#, r#""graphSwitch":{"x":1}"#);
    assert_eq!(crate::io::read_mailbox(odd.as_bytes()).unwrap().account(ids.account).unwrap().graph_switch, GraphSwitch::NotAsked);
}

#[test]
fn only_a_microsoft_account_on_imap_is_asked() {
    let (mut s, ids) = mailbox();
    assert!(answer(&mut s, ids.local, "switch").is_err());
    assert!(s.run("account.graphSwitch", &json!({"account": ids.account, "answer": "maybe"})).is_err());
    assert!(s.run("account.graphSwitch", &json!({"account": 999_999, "answer": "switch"})).is_err());
    let mut plain = Session::empty();
    assert!(matches!(plain.run("account.graphSwitch", &json!({"answer": "switch"})), Err(CmdError::Disabled(_))));
}

#[test]
fn switching_waits_while_anything_waits_for_the_imap_server() {
    let (mut s, ids) = mailbox();
    let dir = scratch("waits");
    s.path = Some(dir.join("mailbox.mailcraft"));
    // A flag changed here that the server hasn't been told, and a deletion it hasn't either.
    s.mailbox.message_mut(ids.on_server).unwrap().flag = FlagStatus::None;
    s.mailbox.pending.push(Pending::Delete { account: ids.account, from: "INBOX".into(), uid: 4, message_id: String::new(), seen: None });
    let said = answer(&mut s, ids.account, "switch").unwrap();
    assert_eq!((said["switched"].clone(), said["waiting"].clone()), (json!(false), json!(2)));
    // Noted, so that the switch goes ahead once a Send/Receive has told the server; nothing else.
    let a = s.mailbox.account(ids.account).unwrap();
    assert_eq!((a.kind, a.graph_switch), (AccountKind::Imap, GraphSwitch::Switching));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no backup yet");
    // The server took both: the switch goes ahead.
    s.mailbox.message_mut(ids.on_server).unwrap().synced = Some(RemoteFlags { uid: 5, ..Default::default() });
    s.mailbox.message_mut(ids.on_server).unwrap().flag = FlagStatus::None;
    s.mailbox.pending.clear();
    assert_eq!(answer(&mut s, ids.account, "switch").unwrap()["switched"], json!(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switch_without_them_leaves_what_waits_in_the_backup() {
    let (mut s, ids) = mailbox();
    let dir = scratch("without");
    s.path = Some(dir.join("mailbox.mailcraft"));
    // A flag and a deletion the IMAP server can't be told (it is turned off, say).
    s.mailbox.message_mut(ids.on_server).unwrap().flag = FlagStatus::None;
    s.mailbox.pending.push(Pending::Delete { account: ids.account, from: "INBOX".into(), uid: 4, message_id: String::new(), seen: None });
    assert_eq!(answer(&mut s, ids.account, "switch").unwrap()["waiting"], json!(2));
    let before = s.mailbox.clone();
    let said = answer(&mut s, ids.account, "switchWithout").unwrap();
    assert_eq!((said["switched"].clone(), said["leftBehind"].clone()), (json!(true), json!(2)));
    // They are in the backup, as they were.
    let backup = crate::io::open_path(&dir.join("mailbox (backup 2027-01-15).mailcraft")).unwrap();
    assert_eq!(backup.message(ids.on_server).map(|m| m.flag), Some(FlagStatus::None));
    assert_eq!(backup.pending, before.pending);
    assert_eq!(s.mailbox.account(ids.account).unwrap().kind, AccountKind::Microsoft);
    assert!(s.mailbox.pending.iter().all(|p| p.account() != ids.account));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mailbox_that_isnt_in_a_file_isnt_switched() {
    let (mut s, ids) = mailbox();
    let before = s.mailbox.clone();
    let refused = answer(&mut s, ids.account, "switch").unwrap_err();
    assert!(refused.to_string().contains("save the mailbox to a file first"), "{refused}");
    assert_eq!(s.mailbox, before);
}

#[test]
fn switching_backs_up_first_and_keeps_what_only_is_here() {
    let (mut s, ids) = mailbox();
    let dir = scratch("switch");
    let path = dir.join("mailbox.mailcraft");
    s.path = Some(path.clone());
    let before = s.mailbox.clone();
    let said = answer(&mut s, ids.account, "switch").unwrap();
    assert_eq!(said["switched"], json!(true));
    // The backup is the whole mailbox as it was, beside the file, with the date in its name.
    let backup = dir.join("mailbox (backup 2027-01-15).mailcraft");
    assert_eq!(said["backup"], json!(backup.to_string_lossy()));
    assert_eq!(crate::io::open_path(&backup).unwrap(), before);
    // A Microsoft account kept in step through Graph, with no mail servers and its sign-in.
    let a = s.mailbox.account(ids.account).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none(), a.graph_switch), (AccountKind::Microsoft, true, true, GraphSwitch::NotAsked));
    assert!(a.oauth.is_some());
    // What the server has is gone here, to come down afresh.
    assert!(s.mailbox.message(ids.on_server).is_none() && s.mailbox.message(ids.server_draft).is_none());
    assert_eq!((said["dropped"].clone(), said["drafts"].clone(), said["kept"].clone()), (json!(2), json!(1), json!(2)));
    // The draft that never went up waits to go up as the server's.
    let draft = s.mailbox.message(ids.unsent).unwrap();
    assert_eq!((draft.account, draft.folder), (ids.account, ids.drafts));
    let sd = draft.server_draft.as_ref().unwrap();
    assert!(sd.pending && !sd.marker.is_empty() && sd.synced.is_none());
    assert_eq!(graph_mail_request(&s, ids.account).unwrap().drafts.len(), 1);
    // The folder that only was here, and the message the server hasn't, are in a local folder.
    let kept = said["keptIn"].as_u64().unwrap();
    let kept_folder = s.mailbox.folder(kept).unwrap();
    assert_eq!((kept_folder.account, kept_folder.name.as_str(), kept_folder.parent), (ids.local, "Kept from owner@contoso.example", None));
    let notes = s.mailbox.folder(ids.notes).unwrap();
    assert_eq!((notes.account, notes.parent), (ids.local, Some(kept)));
    let (in_notes, loose) = (s.mailbox.message(ids.in_notes).unwrap(), s.mailbox.message(ids.loose).unwrap());
    assert_eq!((in_notes.account, in_notes.folder), (ids.local, ids.notes));
    assert_eq!((loose.account, loose.folder), (ids.local, kept));
    // The local account is as it was.
    assert_eq!(s.mailbox.message(ids.local_message), before.message(ids.local_message));
    // The server's folders stay, emptied, waiting for Graph's listing; never made there.
    for id in [ids.inbox, ids.drafts, ids.projects, ids.old] {
        let f = s.mailbox.folder(id).unwrap();
        assert_eq!((f.account, f.remote.as_str(), f.uid_validity, f.uid_seen, f.server.is_none()), (ids.account, "", 0, 0, true));
        assert_eq!(f.awaiting_listing, f.kind == FolderKind::Custom, "{}", f.name);
    }
    assert!(s.mailbox.folder(ids.projects).unwrap().favorite);
    assert!(graph_mail_request(&s, ids.account).unwrap().changes.is_empty());
    assert_eq!(s.view.folder, ids.projects);
    // Nothing goes back to before: what was before is the backup.
    assert!(!s.can_undo());
    let _ = s.run("edit.undo", &json!({}));
    assert_eq!(s.mailbox.account(ids.account).unwrap().kind, AccountKind::Microsoft);
    // Switched again the next day (after going back to the backup), the copy has a name of its own.
    let next = backup_path(&path, "2027-01-15").unwrap();
    assert_eq!(next, dir.join("mailbox (backup 2027-01-15, 2).mailcraft"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn graphs_first_complete_listing_finds_the_folders_by_path() {
    let (mut s, ids) = mailbox();
    let dir = scratch("listing");
    s.path = Some(dir.join("mailbox.mailcraft"));
    answer(&mut s, ids.account, "switch").unwrap();
    let server = vec![
        graph_folder("in", "", "Inbox", FolderKind::Inbox),
        graph_folder("dr", "", "Drafts", FolderKind::Drafts),
        graph_folder("pr", "in", "projects", FolderKind::Custom),
        graph_folder("ab", "", "A/B", FolderKind::Custom),
    ];
    // A listing cut short finds what it has, and removes nothing.
    listed(&mut s, ids.account, server.clone(), false);
    assert!(s.mailbox.folder(ids.old).is_some_and(|f| f.awaiting_listing));
    listed(&mut s, ids.account, server, true);
    // The Inbox by its kind, Projects by its name under it: the same folders, favourite and all.
    let id_of = |s: &Session, id: Id| s.mailbox.folder(id).and_then(|f| f.server.as_ref()).map(|server| server.id.clone());
    assert_eq!((id_of(&s, ids.inbox), id_of(&s, ids.drafts), id_of(&s, ids.projects)), (Some("in".into()), Some("dr".into()), Some("pr".into())));
    let projects = s.mailbox.folder(ids.projects).unwrap();
    assert!(projects.favorite && !projects.awaiting_listing);
    assert_eq!(projects.parent, Some(ids.inbox));
    // What Graph has that IMAP never showed is new; what Graph doesn't have of IMAP's is gone.
    assert!(s.mailbox.folder_by_server(ids.account, "ab").is_some());
    assert!(s.mailbox.folder(ids.old).is_none());
    assert!(s.mailbox.folders.iter().all(|f| !f.awaiting_listing));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unlisted_folder_that_holds_mail_is_kept() {
    let (mut s, ids) = mailbox();
    let dir = scratch("unlisted");
    s.path = Some(dir.join("mailbox.mailcraft"));
    answer(&mut s, ids.account, "switch").unwrap();
    // (Nothing should put mail there; if anything does, it isn't lost.)
    s.mailbox.add_message(message(ids.old, "Somehow here", 0)).unwrap();
    listed(&mut s, ids.account, vec![graph_folder("in", "", "Inbox", FolderKind::Inbox)], true);
    let old = s.mailbox.folder(ids.old).unwrap();
    assert!(!old.awaiting_listing && old.server.is_none());
    assert_eq!(s.mailbox.count(ids.old), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mailbox_without_a_local_account_gets_one_for_what_only_is_here() {
    let (mut s, ids) = mailbox();
    s.mailbox.remove_account(ids.local).unwrap();
    let dir = scratch("nolocal");
    s.path = Some(dir.join("mailbox.mailcraft"));
    let said = answer(&mut s, ids.account, "switch").unwrap();
    let kept = s.mailbox.folder(said["keptIn"].as_u64().unwrap()).unwrap();
    let local = s.mailbox.account(kept.account).unwrap();
    assert_eq!((local.kind, local.name.as_str()), (AccountKind::Local, "On this computer"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_message_whose_draft_went_with_the_switch_counts_as_not_saved() {
    let (mut s, ids) = mailbox();
    s.path = Some(scratch("switch-draft").join("mailbox.mailcraft"));
    s.run("mail.editDraft", &json!({"id": ids.server_draft})).unwrap();
    assert!(s.compose.as_ref().is_some_and(|c| c.draft == Some(ids.server_draft) && !c.dirty));
    assert_eq!(answer(&mut s, ids.account, "switch").unwrap()["switched"], json!(true));
    assert!(s.mailbox.message(ids.server_draft).is_none());
    // What is written is saved nowhere now (#573): closing it asks, and a save makes a new draft.
    let c = s.compose.as_ref().unwrap();
    assert!(c.draft.is_none() && c.dirty, "{:?}", (c.draft, c.dirty));
    assert_eq!(c.subject, "Written elsewhere");
}
