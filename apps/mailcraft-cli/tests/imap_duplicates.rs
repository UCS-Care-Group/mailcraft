// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message moved on, or deleted, before the server's copy of its first move came down (a server
//! without UIDPLUS says no new UID) must be moved or deleted there too, and be here once. Fake
//! servers and invented mail only.
#![cfg(not(target_arch = "wasm32"))]

#[allow(dead_code)]
#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use std::sync::atomic::{AtomicBool, Ordering};

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, FakeSmtp, ImapWorld, SmtpWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{SyncTotals, apply_sync, apply_sync_part, sync_request};
use mailcraft_model::{Id, Mailbox};
use serde_json::{Value, json};

const PASSWORD: &str = "hunter2";

fn eml(subject: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n",
        subject.to_lowercase().replace(' ', "-")
    )
    .into_bytes()
}

fn message(uid: u32, flags: &str, subject: &str) -> FakeMessage {
    FakeMessage { uid, flags: flags.into(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: eml(subject), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

fn world(can_move: bool, uidplus: bool) -> ImapWorld {
    ImapWorld {
        user: "ada@example.org".into(),
        password: PASSWORD.into(),
        can_move,
        uidplus,
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "", "One"), message(2, "", "Two"), message(3, "", "Three")]),
            mailbox("Projects", "\\HasNoChildren", 21, Vec::new()),
            mailbox("Later", "\\HasNoChildren", 31, Vec::new()),
        ],
        ..Default::default()
    }
}

struct Rig {
    imap: FakeImap,
    _smtp: FakeSmtp,
    s: Session,
    account: Id,
}

impl Rig {
    fn with(world: ImapWorld) -> Rig {
        let imap = Fake::imap(world);
        let smtp = Fake::smtp(SmtpWorld { user: "ada@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let servers = json!({
            "email": "ada@example.org", "displayName": "Ada",
            "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
            "outgoing": {"host": "127.0.0.1", "port": smtp.port, "security": "none"},
        });
        let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        Rig { imap, _smtp: smtp, s, account }
    }

    fn run(&mut self, id: &str, params: Value) -> Value {
        self.s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
    }

    fn sync(&mut self) -> Value {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors, Vec::<String>::new());
        apply_sync(&mut self.s, result).unwrap()
    }

    /// A run whose request is made, which goes to the server, and then `act` is done here before
    /// the run's answer is taken in (the owner changes something while a run is under way).
    fn sync_acting(&mut self, act: impl FnOnce(&mut Rig)) {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors, Vec::<String>::new());
        act(self);
        apply_sync(&mut self.s, result).unwrap();
    }

    /// A run called off once piece `stop_at` has come (as the window does when the owner
    /// changes something during a long run); every piece is taken in. Returns the pieces' lines.
    fn sync_called_off(&mut self, stop: impl Fn(&mailcraft_model::sync::SyncResult) -> bool) -> Vec<String> {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let cancel = AtomicBool::new(false);
        let (mut totals, mut lines) = (SyncTotals::default(), Vec::new());
        mailcraft_net::sync_with(&request, &cancel, &mut |piece| {
            lines.push(piece.progress.clone());
            if stop(&piece) {
                cancel.store(true, Ordering::Relaxed);
            }
            apply_sync_part(&mut self.s, &mut totals, piece).unwrap();
        });
        lines
    }

    fn folder_named(&self, name: &str) -> Id {
        self.s.mailbox.folders.iter().find(|f| f.account == self.account && f.name == name).map(|f| f.id).unwrap()
    }

    fn titled(&self, subject: &str) -> Vec<Id> {
        self.s.mailbox.messages.values().filter(|m| m.account == self.account && m.subject == subject).map(|m| m.id).collect()
    }

    fn server_count(&self, subject: &str) -> usize {
        let needle = format!("Subject: {subject}\r\n");
        self.imap
            .world()
            .mailboxes
            .iter()
            .flat_map(|m| m.messages.iter())
            .filter(|m| !m.has("\\Deleted") && String::from_utf8_lossy(&m.body).contains(&needle))
            .count()
    }
}

/// A message moved here into a folder (the server hears of it, but says no new UID: no UIDPLUS),
/// then moved on here into another folder before the server's copy in the first came down. The
/// server has it once; here it must be once too.
#[test]
fn moved_on_before_its_copy_came_down_is_here_once() {
    for can_move in [true, false] {
        let mut r = Rig::with(world(can_move, false));
        r.sync();
        let (projects, later) = (r.folder_named("Projects"), r.folder_named("Later"));
        let two = r.titled("Two")[0];
        r.run("mail.move", json!({"ids": [two], "folder": projects}));
        // The run pushes the move and is called off with the piece that says so, before any
        // folder is read (the window calls a long run off when the owner changes something).
        let lines = r.sync_called_off(|piece| !piece.moved.is_empty());
        assert!(r.s.mailbox.pending.is_empty(), "the move went: {:?} {lines:?}", r.s.mailbox.pending);
        // Moved on here, before the server's copy in Projects came down.
        r.run("mail.move", json!({"ids": [two], "folder": later}));
        for _ in 0..4 {
            r.sync();
        }
        let here: Vec<(String, u32)> = r
            .titled("Two")
            .iter()
            .filter_map(|id| r.s.mailbox.message(*id))
            .map(|m| (r.s.mailbox.folder(m.folder).map(|f| f.name.clone()).unwrap_or_default(), m.uid))
            .collect();
        assert_eq!(r.server_count("Two"), 1, "MOVE {can_move}: on the server");
        assert_eq!(here.len(), 1, "MOVE {can_move}: one server message is here {} times: {here:?}", here.len());
    }
}

/// Where the message `subject` is here: its folder's name and UID, one entry for each copy.
fn here(r: &Rig, subject: &str) -> Vec<(String, u32)> {
    r.titled(subject)
        .iter()
        .filter_map(|id| r.s.mailbox.message(*id))
        .map(|m| (r.s.mailbox.folder(m.folder).map(|f| f.name.clone()).unwrap_or_default(), m.uid))
        .collect()
}

/// Which mailboxes of the server hold `subject`.
fn on_server(r: &Rig, subject: &str) -> Vec<String> {
    let needle = format!("Subject: {subject}\r\n");
    let world = r.imap.world();
    world
        .mailboxes
        .iter()
        .filter(|m| m.messages.iter().any(|x| !x.has("\\Deleted") && String::from_utf8_lossy(&x.body).contains(&needle)))
        .map(|m| m.raw.clone())
        .collect()
}

/// `subject` moved into the first folder named and the run called off once the server has it
/// there (it says no UID: no UIDPLUS), before the copy came down.
fn moved_up(r: &mut Rig, subject: &str, first: &str) -> Id {
    let (id, folder) = (r.titled(subject)[0], r.folder_named(first));
    r.run("mail.move", json!({"ids": [id], "folder": folder}));
    r.sync_called_off(|piece| !piece.moved.is_empty());
    assert!(r.s.mailbox.pending.is_empty(), "the move went: {:?}", r.s.mailbox.pending);
    id
}

fn settle(r: &mut Rig) {
    for _ in 0..4 {
        r.sync();
    }
}

#[test]
fn moved_on_twice_before_its_copy_came_down_ends_up_where_it_was_put() {
    for can_move in [true, false] {
        let mut r = Rig::with(world(can_move, false));
        r.sync();
        let id = moved_up(&mut r, "Two", "Projects");
        let (later, inbox) = (r.folder_named("Later"), r.folder_named("Inbox"));
        r.run("mail.move", json!({"ids": [id], "folder": later}));
        r.run("mail.move", json!({"ids": [id], "folder": inbox}));
        settle(&mut r);
        assert_eq!(on_server(&r, "Two"), ["INBOX"], "MOVE {can_move}");
        let found = here(&r, "Two");
        assert_eq!(found.len(), 1, "MOVE {can_move}: {found:?}");
        assert!(found[0].0 == "Inbox" && found[0].1 != 0, "MOVE {can_move}: {found:?}");
    }
}

#[test]
fn moved_on_and_back_before_its_copy_came_down_stays_in_the_first_folder() {
    let mut r = Rig::with(world(true, false));
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    let (later, projects) = (r.folder_named("Later"), r.folder_named("Projects"));
    r.run("mail.move", json!({"ids": [id], "folder": later}));
    r.run("mail.move", json!({"ids": [id], "folder": projects}));
    settle(&mut r);
    assert_eq!(on_server(&r, "Two"), ["Projects"]);
    let found = here(&r, "Two");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].0 == "Projects" && found[0].1 != 0, "{found:?}");
}

#[test]
fn deleted_before_its_copy_came_down_is_deleted_on_the_server_too() {
    for can_move in [true, false] {
        let mut r = Rig::with(world(can_move, false));
        r.sync();
        let id = moved_up(&mut r, "Two", "Projects");
        r.run("mail.deletePermanently", json!({"ids": [id]}));
        settle(&mut r);
        assert_eq!(on_server(&r, "Two"), Vec::<String>::new(), "MOVE {can_move}");
        assert_eq!(here(&r, "Two"), Vec::<(String, u32)>::new(), "MOVE {can_move}");
        for other in ["One", "Three"] {
            assert_eq!(r.server_count(other), 1, "MOVE {can_move}: {other} is untouched");
            assert_eq!(here(&r, other).len(), 1, "MOVE {can_move}: {other} is here once");
        }
    }
}

#[test]
fn deleted_to_deleted_items_before_its_copy_came_down_is_there_once() {
    let mut w = world(true, false);
    w.mailboxes.push(mailbox("Trash", "\\HasNoChildren \\Trash", 41, Vec::new()));
    let mut r = Rig::with(w);
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    r.run("mail.delete", json!({"ids": [id]}));
    settle(&mut r);
    assert_eq!(on_server(&r, "Two"), ["Trash"]);
    for other in ["One", "Three"] {
        assert_eq!(r.server_count(other), 1, "{other} is untouched");
    }
    let found = here(&r, "Two");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1 != 0, "{found:?}");
}

#[test]
fn a_copy_that_can_not_be_told_apart_is_never_given_a_change_meant_for_another() {
    // The server's copy of "Two" has a Message-ID that two messages of Projects share: the
    // change meant for one of them must not go to either.
    let mut r = Rig::with(world(true, false));
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    r.imap.world().mailboxes.iter_mut().find(|m| m.raw == "Projects").unwrap().messages.push(message(900, "", "Two"));
    let later = r.folder_named("Later");
    r.run("mail.move", json!({"ids": [id], "folder": later}));
    settle(&mut r);
    // Both copies stay where they are on the server, the change meant for one is dropped, and
    // nothing is left waiting.
    assert!(r.s.mailbox.pending.is_empty(), "{:?}", r.s.mailbox.pending);
    assert_eq!(on_server(&r, "Two"), ["Projects"]);
    assert_eq!(r.server_count("Two"), 2, "both copies are on the server");
    assert_eq!(r.imap.world().mailboxes.iter().find(|m| m.raw == "Later").map_or(9, |m| m.messages.len()), 0, "nothing went to Later");
    // Here: the message in Later, and the copies in Projects as they are on the server.
    let found = here(&r, "Two");
    assert_eq!(found.iter().filter(|(folder, _)| folder == "Projects").count(), 2, "{found:?}");
    assert_eq!(found.iter().filter(|(folder, _)| folder == "Later").count(), 1, "{found:?}");
}

#[test]
fn undo_of_a_delete_after_the_copy_was_matched_keeps_the_server_copy() {
    let mut r = Rig::with(world(true, false));
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    r.run("mail.deletePermanently", json!({"ids": [id]}));
    // The run matches the copy to the deletion.
    r.sync();
    r.s.run("edit.undo", &json!({})).unwrap();
    settle(&mut r);
    assert_eq!(on_server(&r, "Two"), ["Projects"], "the copy stays on the server");
    let found = here(&r, "Two");
    assert!(matches!(&found[..], [(folder, uid)] if folder == "Projects" && *uid != 0), "{found:?}");
}

#[test]
fn undo_of_a_move_on_after_the_copy_was_matched_ties_the_message_to_the_copy() {
    let mut r = Rig::with(world(true, false));
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    let later = r.folder_named("Later");
    r.run("mail.move", json!({"ids": [id], "folder": later}));
    // The run matches the copy to the move, which goes up the next time.
    r.sync();
    r.s.run("edit.undo", &json!({})).unwrap();
    settle(&mut r);
    // Back in Projects on the server too, once, and tied to it: a later deletion reaches it.
    let found = here(&r, "Two");
    assert!(matches!(&found[..], [(folder, uid)] if folder == "Projects" && *uid != 0), "{found:?}");
    r.run("mail.deletePermanently", json!({"ids": [r.titled("Two")[0]]}));
    settle(&mut r);
    assert_eq!(r.server_count("Two"), 0, "the deletion reached the server");
}

#[test]
fn a_move_on_made_while_the_run_that_pushed_the_first_is_under_way_goes_up_too() {
    for delete in [false, true] {
        let mut r = Rig::with(world(true, false));
        r.sync();
        let (id, projects, later) = (r.titled("Two")[0], r.folder_named("Projects"), r.folder_named("Later"));
        r.run("mail.move", json!({"ids": [id], "folder": projects}));
        r.sync_acting(|r| {
            if delete {
                r.run("mail.deletePermanently", json!({"ids": [id]}));
            } else {
                r.run("mail.move", json!({"ids": [id], "folder": later}));
            }
        });
        settle(&mut r);
        if delete {
            assert_eq!(r.server_count("Two"), 0, "deleted on the server");
            assert!(here(&r, "Two").is_empty());
        } else {
            assert_eq!(on_server(&r, "Two"), ["Later"]);
            let found = here(&r, "Two");
            assert!(matches!(&found[..], [(folder, uid)] if folder == "Later" && *uid != 0), "{found:?}");
        }
    }
}

#[test]
fn moved_to_this_computer_before_its_copy_came_down_leaves_the_server_once_safe() {
    let mut r = Rig::with(world(true, false));
    r.sync();
    let id = moved_up(&mut r, "Two", "Projects");
    let local = r.s.mailbox.local_account();
    let kept_in = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
    r.run("mail.move", json!({"ids": [id], "folder": kept_in}));
    // Not saved here yet: the server keeps its copy.
    settle(&mut r);
    assert_eq!(r.server_count("Two"), 1, "the server's copy stays until the message is saved here");
    let dir = std::env::temp_dir().join(format!("mailcraft-imap-dup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    r.run("file.save", json!({"path": dir.join("kept.mailcraft")}));
    settle(&mut r);
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(r.server_count("Two"), 0, "the server's copy went once the message was safe here");
    let kept: Vec<Id> = r.s.mailbox.messages.values().filter(|m| m.subject == "Two").map(|m| m.id).collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(r.s.mailbox.message(kept[0]).is_some_and(|m| m.account == local));
}

#[test]
fn undo_of_one_of_two_deletions_of_the_same_mail_keeps_the_other_deletion_only() {
    // The same mail delivered twice (INBOX UIDs 7 and 8: one Message-ID, one identity), each
    // deleted for good in a step of its own: an undo gives back the one it undoes, never the
    // other's deletion.
    let mut w = world(true, true);
    w.mailboxes[0].messages = vec![message(7, "", "Dup"), message(8, "", "Dup")];
    let mut r = Rig::with(w);
    r.sync();
    let by_uid = |r: &Rig, uid: u32| r.titled("Dup").into_iter().find(|id| r.s.mailbox.message(*id).is_some_and(|m| m.uid == uid)).unwrap();
    let (seven, eight) = (by_uid(&r, 7), by_uid(&r, 8));
    r.run("mail.deletePermanently", json!({"ids": [seven]}));
    r.run("mail.deletePermanently", json!({"ids": [eight]}));
    let deletes = |r: &Rig| -> Vec<u32> {
        let mut request = sync_request(&r.s, r.account).unwrap();
        request.password = PASSWORD.into();
        request.ops.iter().filter_map(|op| if let mailcraft_model::sync::RemoteOp::Delete { uid, .. } = op { Some(*uid) } else { None }).collect()
    };
    assert_eq!(deletes(&r), [7, 8]);
    r.run("edit.undo", json!({}));
    assert_eq!(deletes(&r), [7], "the second deletion is undone, the first still goes");
    r.run("edit.undo", json!({}));
    assert_eq!(deletes(&r), Vec::<u32>::new());
    r.run("edit.redo", json!({}));
    assert_eq!(deletes(&r), [7]);
    r.run("edit.redo", json!({}));
    assert_eq!(deletes(&r), [7, 8]);
}

#[test]
fn undo_of_a_deletion_that_waited_for_its_copy_leaves_the_deletion_of_a_like_mail_with_its_own_uid() {
    // Projects holds X (UID 5); Y, the same mail, is moved up from the Inbox with no UIDPLUS, so
    // it waits for its copy. X is deleted for good, then Y: undoing Y's deletion must not hand Y
    // X's UID, and redoing it must not give Y's deletion X's.
    let mut w = world(true, false);
    w.mailboxes[1].messages = vec![message(5, "", "Two")];
    let mut r = Rig::with(w);
    r.sync();
    let projects = r.folder_named("Projects");
    let x = r.titled("Two").into_iter().find(|id| r.s.mailbox.message(*id).is_some_and(|m| m.folder == projects)).unwrap();
    let y = r.titled("Two").into_iter().find(|id| *id != x).unwrap();
    r.run("mail.move", json!({"ids": [y], "folder": projects}));
    r.sync_called_off(|piece| !piece.moved.is_empty());
    r.run("mail.deletePermanently", json!({"ids": [x]}));
    r.run("mail.deletePermanently", json!({"ids": [y]}));
    let uids = |r: &Rig| -> Vec<u32> { r.s.mailbox.pending.iter().map(|p| p.source().1).collect() };
    assert_eq!(uids(&r), [5, 0]);
    r.run("edit.undo", json!({}));
    assert_eq!(uids(&r), [5], "X's deletion stays");
    assert!(r.s.mailbox.message(y).is_some_and(|m| m.uid == 0), "Y is not given X's UID");
    r.run("edit.redo", json!({}));
    assert_eq!(uids(&r), [5, 0], "each deletion has its own UID");
}

#[test]
fn moved_to_this_computer_while_the_run_that_pushed_the_first_move_is_under_way_leaves_the_server_once_safe() {
    let mut r = Rig::with(world(true, false));
    r.sync();
    let (id, projects) = (r.titled("Two")[0], r.folder_named("Projects"));
    let local = r.s.mailbox.local_account();
    let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
    r.run("mail.move", json!({"ids": [id], "folder": projects}));
    r.sync_acting(|r| {
        r.run("mail.move", json!({"ids": [id], "folder": kept}));
    });
    settle(&mut r);
    assert_eq!(r.server_count("Two"), 1, "the server's copy stays until the message is saved here");
    let dir = std::env::temp_dir().join(format!("mailcraft-imap-dup-flight-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    r.run("file.save", json!({"path": dir.join("kept.mailcraft")}));
    settle(&mut r);
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(r.server_count("Two"), 0, "the server's copy went once the message was safe here");
    let kept_here: Vec<Id> = r.s.mailbox.messages.values().filter(|m| m.subject == "Two").map(|m| m.id).collect();
    assert!(matches!(&kept_here[..], [one] if r.s.mailbox.message(*one).is_some_and(|m| m.account == local)), "{kept_here:?}");
}
