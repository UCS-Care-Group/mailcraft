// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail brought in twice (#1401): an import leaves out what the folder holds already, mail meant
//! for a server's folder stays on this computer, and the clean-up removes only the local copies of
//! mail its server has.

use mailcraft_model::sync::{FolderDelta, ServerMessage, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Mailbox, Message, Pending, Queued};
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::account::sync_request;

fn mail(n: u32) -> Message {
    Message {
        message_id: format!("m{n}@example.org"),
        subject: format!("Plans {n}"),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Cy", "cy@example.org")],
        date: 1_700_000_000 + i64::from(n) * 60,
        ..Default::default()
    }
}

/// An IMAP account whose Inbox holds three messages from its server (UIDs 1 to 3).
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let new: Vec<Message> = (1..=3).map(|n| Message { uid: n, ..mail(n) }).collect();
    let delta = FolderDelta { id: None, remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 3, new, ..Default::default() };
    s.run("sync.apply", &json!({"result": SyncResult { account: a, folders: vec![delta], ..Default::default() }})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    (s, a, inbox)
}

fn export(s: &mut Session, folder: Id) -> String {
    let r = s.run("file.exportFolder", &json!({"id": folder})).unwrap();
    r["data"].as_str().unwrap().to_string()
}

fn import(s: &mut Session, data: &str, folder: Id) -> Value {
    s.run("file.import", &json!({"name": "folder.mbox", "data": data, "folder": folder})).unwrap()
}

#[test]
fn a_folder_exported_and_imported_twice_is_not_doubled() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let inbox = s.mailbox.inbox().unwrap();
    for n in 1..=4 {
        s.mailbox.add_message(Message { folder: inbox, ..mail(n) }).unwrap();
    }
    let data = export(&mut s, inbox);
    let before = s.mailbox.messages.len();
    for _ in 0..2 {
        let r = import(&mut s, &data, inbox);
        assert_eq!(r["messages"], 0);
        assert_eq!(r["already"], 4);
        assert_eq!(r["local"], false);
    }
    assert_eq!(s.mailbox.messages.len(), before);
}

#[test]
fn a_message_without_a_message_id_is_always_imported() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let inbox = s.mailbox.inbox().unwrap();
    let eml = "From: Bo <bo@example.org>\nTo: cy@example.org\nSubject: No id\nDate: Tue, 14 Nov 2023 22:13:20 +0000\n\nHello\n";
    let data = crate::cmd::base64_encode(eml.as_bytes());
    for _ in 0..2 {
        let r = s.run("file.import", &json!({"name": "a.eml", "data": data, "folder": inbox})).unwrap();
        assert_eq!(r["messages"], 1);
        assert_eq!(r["already"], 0);
    }
    assert_eq!(s.mailbox.messages_in(inbox).count(), 2);
}

#[test]
fn the_same_message_id_with_another_subject_is_another_message() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let inbox = s.mailbox.inbox().unwrap();
    s.mailbox.add_message(Message { folder: inbox, ..mail(1) }).unwrap();
    let mut other = mail(1);
    other.subject = "Something else".into();
    let other = mailcraft_mime_model::write(&other, &mailcraft_mime_model::WriteOptions::default());
    let r = s.run("file.import", &json!({"name": "a.eml", "data": crate::cmd::base64_encode(&other), "folder": inbox})).unwrap();
    assert_eq!(r["messages"], 1);
}

#[test]
fn an_import_into_an_imap_folder_lands_in_imported_and_nothing_goes_to_the_server() {
    let (mut s, a, inbox) = imap();
    // The folder's own export, handed back: the server's mail is in the mailbox already, so not
    // even the first import copies it (the tester's case).
    let own = export(&mut s, inbox);
    let first = import(&mut s, &own, inbox);
    assert_eq!((first["messages"].as_u64(), first["already"].as_u64()), (Some(0), Some(3)), "{first}");
    assert_eq!(s.mailbox.messages.len(), 3);
    // Mail the mailbox hasn't got.
    let others: Vec<Message> = (4..=6).map(mail).collect();
    let refs: Vec<&Message> = others.iter().collect();
    let data = crate::cmd::base64_encode(&mailcraft_mime_model::write_mbox(&refs, &Default::default()));
    let r = import(&mut s, &data, inbox);
    assert_eq!(r["local"], true);
    assert_eq!(r["account"], "imap");
    assert_eq!(r["messages"], 3);
    let imported = r["folder"].as_u64().unwrap();
    assert_ne!(imported, inbox);
    let local = s.mailbox.local_account();
    assert_eq!(s.mailbox.folder(imported).unwrap().account, local, "a folder on this computer");
    assert_eq!(s.mailbox.messages_in(inbox).count(), 3, "the server's folder is as it was");
    // Again: nothing more.
    let again = import(&mut s, &data, inbox);
    assert_eq!(again["messages"], 0);
    assert_eq!(again["already"], 3);
    assert_eq!(s.mailbox.messages_in(imported).count(), 3);
    // Nothing for the next Send/Receive to tell the server.
    let req = sync_request(&s, a).unwrap();
    assert!(req.ops.is_empty() && req.places.is_empty() && req.outbox.is_empty(), "nothing is sent to the IMAP server");
    assert!(s.mailbox.pending.is_empty() && s.mailbox.queued.is_empty());
}

#[test]
fn an_import_into_a_jmap_or_exchange_folder_lands_in_imported_too() {
    for kind in [mailcraft_model::AccountKind::Jmap, mailcraft_model::AccountKind::Exchange] {
        let (mut s, a, inbox) = imap();
        s.mailbox.accounts.iter_mut().find(|x| x.id == a).unwrap().kind = kind;
        let data = export(&mut s, inbox);
        let r = import(&mut s, &data, inbox);
        assert_eq!(r["local"], true, "{kind:?}");
        assert_ne!(r["folder"].as_u64().unwrap(), inbox, "{kind:?}");
    }
}

/// A local copy of mail `n`, in `folder`, as an import before #1401 made it.
fn local_copy(s: &mut Session, folder: Id, n: u32) -> Id {
    s.mailbox.add_message(Message { folder, ..mail(n) }).unwrap()
}

#[test]
fn the_clean_up_removes_only_the_local_twins_and_undo_brings_them_back() {
    let (mut s, a, inbox) = imap();
    // A message a JMAP or Microsoft server keeps by its own id, with no UID.
    let by_id = s
        .mailbox
        .add_message(Message { folder: inbox, server: Some(ServerMessage { id: "E4".into(), folder: "M1".into(), ..Default::default() }), ..mail(4) })
        .unwrap();
    let twin1 = local_copy(&mut s, inbox, 1);
    let twin2 = local_copy(&mut s, inbox, 2);
    let twin4 = local_copy(&mut s, inbox, 4);
    // A message only here with no twin.
    let lone = local_copy(&mut s, inbox, 9);
    // A local copy with a move waiting on it, and one waiting to be put on the server.
    let moving = local_copy(&mut s, inbox, 3);
    s.mailbox.pending.push(Pending::Move { account: a, message: moving, from: "INBOX".into(), uid: 3, synced: None, to: inbox });
    let placing = local_copy(&mut s, inbox, 3);
    s.mailbox.queued.push(Queued::PlaceMessage { account: a, message: placing, marker: "mailcraft-1".into(), copy: false, from: None });
    // A draft, and a copy waiting for its server's UID.
    let draft = s.mailbox.add_message(Message { folder: inbox, draft: true, ..mail(1) }).unwrap();
    let awaiting = s.mailbox.add_message(Message { folder: inbox, synced: Some(mailcraft_model::sync::RemoteFlags::default()), ..mail(2) }).unwrap();
    let server_side: Vec<Id> = s.mailbox.messages_in(inbox).filter(|m| m.uid != 0).map(|m| m.id).collect();
    let before = s.mailbox.messages.len();

    let preview = s.run("mail.removeImportedDuplicates", &json!({"preview": true})).unwrap();
    assert_eq!(preview["removed"], 3);
    assert_eq!(s.mailbox.messages.len(), before, "a preview changes nothing");

    let r = s.run("mail.removeImportedDuplicates", &json!({})).unwrap();
    let mut gone: Vec<Id> = r["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    gone.sort_unstable();
    assert_eq!(gone, vec![twin1, twin2, twin4]);
    for id in [lone, moving, placing, draft, awaiting, by_id].iter().chain(&server_side) {
        assert!(s.mailbox.message(*id).is_some(), "{id} stays");
    }
    assert_eq!(s.mailbox.messages.len(), before - 3);
    let req = sync_request(&s, a).unwrap();
    assert!(req.ops.iter().all(|op| !matches!(op, mailcraft_model::sync::RemoteOp::Delete { .. })), "nothing is deleted on the server");
    assert!(!s.mailbox.pending.iter().any(|p| matches!(p, Pending::Delete { .. })));

    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.len(), before);
    for id in [twin1, twin2, twin4] {
        assert!(s.mailbox.message(id).is_some(), "{id} is back");
    }
}

#[test]
fn an_imap_folder_exported_into_a_local_archive_comes_in_whole() {
    // Kept on this computer before the server's copies are deleted: none of it is left out for
    // being in the IMAP folder too.
    let (mut s, _, inbox) = imap();
    let data = export(&mut s, inbox);
    let local = s.mailbox.local_account();
    let archive = s.mailbox.add_folder(local, None, "Archive 2025").unwrap();
    let r = import(&mut s, &data, archive);
    assert_eq!((r["messages"].as_u64(), r["already"].as_u64(), r["local"].as_bool()), (Some(3), Some(0), Some(false)), "{r}");
    assert_eq!(s.mailbox.messages_in(archive).count(), 3);
    // Imported again into the archive: nothing more.
    let again = import(&mut s, &data, archive);
    assert_eq!(again["already"], 3);
}

#[test]
fn a_copy_that_is_not_whole_does_not_keep_the_backup_out() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let inbox = s.mailbox.inbox().unwrap();
    let damaged: [fn(&mut Message); 4] = [|m| m.incomplete = true, |m| m.not_downloaded = 50_000, |m| m.parts_removed = true, |m| m.unfetched = true];
    for (n, damage) in (1..).zip(damaged) {
        let mut m = Message { folder: inbox, ..mail(n) };
        damage(&mut m);
        s.mailbox.add_message(m).unwrap();
    }
    let backup: Vec<Message> = (1..=4).map(mail).collect();
    let refs: Vec<&Message> = backup.iter().collect();
    let data = crate::cmd::base64_encode(&mailcraft_mime_model::write_mbox(&refs, &Default::default()));
    let r = import(&mut s, &data, inbox);
    assert_eq!((r["messages"].as_u64(), r["already"].as_u64()), (Some(4), Some(0)), "{r}");
}

#[test]
fn the_clean_up_keeps_a_copy_the_owner_changed() {
    let (mut s, _, inbox) = imap();
    let flagged = s.mailbox.add_message(Message { folder: inbox, flag: mailcraft_model::FlagStatus::Flagged, ..mail(1) }).unwrap();
    let read = s.mailbox.add_message(Message { folder: inbox, read: true, ..mail(2) }).unwrap();
    let categorised = s.mailbox.add_message(Message { folder: inbox, categories: vec!["Red category".into()], ..mail(3) }).unwrap();
    let tasked = s.mailbox.add_message(Message { folder: inbox, ..mail(3) }).unwrap();
    s.mailbox.add_task(mailcraft_model::Task { title: "Follow up".into(), message: Some(tasked), ..Default::default() });
    let same = s.mailbox.add_message(Message { folder: inbox, ..mail(3) }).unwrap();
    let r = s.run("mail.removeImportedDuplicates", &json!({})).unwrap();
    assert_eq!((r["removed"].as_u64(), r["kept"].as_u64()), (Some(1), Some(4)), "{r}");
    assert!(s.mailbox.message(same).is_none());
    for id in [flagged, read, categorised, tasked] {
        assert!(s.mailbox.message(id).is_some(), "{id} stays");
    }
    // The server's copies are as they were: nothing carried over.
    assert!(s.mailbox.messages_in(inbox).filter(|m| m.uid != 0).all(|m| !m.read && m.categories.is_empty()));
}

#[test]
fn the_clean_up_of_one_folder_leaves_the_others() {
    let (mut s, _, inbox) = imap();
    let twin = local_copy(&mut s, inbox, 1);
    let local = s.mailbox.local_account();
    let elsewhere = s.mailbox.add_folder(local, None, "Kept").unwrap();
    let other = local_copy(&mut s, elsewhere, 1);
    let r = s.run("mail.removeImportedDuplicates", &json!({"folder": elsewhere})).unwrap();
    assert_eq!(r["removed"], 0);
    let r = s.run("mail.removeImportedDuplicates", &json!({"folder": inbox})).unwrap();
    assert_eq!(r["removed"], 1);
    assert!(s.mailbox.message(twin).is_none());
    assert!(s.mailbox.message(other).is_some(), "a copy in another folder is no twin");
}

#[test]
fn an_agent_previews_freely_and_is_asked_before_removing() {
    use crate::agent::{Access, Class, classify};
    let (s, _, _) = imap();
    assert!(matches!(classify(&s, "mail.removeImportedDuplicates", &json!({"preview": true})), Access::Safe));
    assert!(matches!(classify(&s, "mail.removeImportedDuplicates", &json!({})), Access::Owner(Class::Delete)));
}

#[test]
fn the_clean_up_refuses_what_it_does_not_take() {
    let (mut s, _, _) = imap();
    for params in [json!({"folder": 999_999}), json!({"preview": "yes"}), json!({"folder": null}), json!({"other": 1}), json!([1])] {
        assert!(s.run("mail.removeImportedDuplicates", &params).is_err(), "{params}");
    }
}
