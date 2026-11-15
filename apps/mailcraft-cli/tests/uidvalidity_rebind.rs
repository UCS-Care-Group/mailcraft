// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-17 from end to end, against the in-process fake IMAP server: a permanent deletion or a
//! move made here waits while the server numbers the mailbox anew, and the mailbox comes back
//! in pieces, as the desktop app takes them in. Two messages the server has with the Message-ID
//! of the one changed here (one of them made by its sender to look like it to the letter), both
//! in a later piece: neither is deleted or moved on the server, and the owner is told. One copy
//! alone gets the change. Nothing here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use std::sync::atomic::AtomicBool;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{SyncTotals, apply_sync_part, sync_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::json;

const PASSWORD: &str = "hunter2";

fn eml(subject: &str, id: &str, text: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{id}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n"
    )
    .into_bytes()
}

fn message(uid: u32, body: Vec<u8>) -> FakeMessage {
    FakeMessage { uid, flags: String::new(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body, claimed_size: None }
}

/// Mail that fills the mailbox, so that it comes down in more than one piece (the host hands a
/// mailbox over 50 messages at a time, newest first).
fn filler(from_uid: u32) -> Vec<FakeMessage> {
    (0..60).map(|n| message(from_uid + n, eml(&format!("Note {n}"), &format!("note-{n}"), "A note."))).collect()
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

struct Rig {
    imap: FakeImap,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let mut inbox = vec![message(1, eml("Three", "three", "The real one."))];
        inbox.extend(filler(2));
        let world = ImapWorld {
            user: "ada@example.org".into(),
            password: PASSWORD.into(),
            can_move: true,
            uidplus: true,
            mailboxes: vec![mailbox("INBOX", "\\HasNoChildren", 11, inbox), mailbox("Archive", "\\HasNoChildren \\Archive", 21, Vec::new())],
            ..Default::default()
        };
        let imap = Fake::imap(world);
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let servers = json!({
            "email": "ada@example.org", "displayName": "Ada",
            "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
            "outgoing": {"host": "127.0.0.1", "port": 1, "security": "none"},
        });
        let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        let mut rig = Rig { imap, s, account };
        rig.sync();
        rig
    }

    /// One Send/Receive, its answer taken in piece by piece as the desktop app does it; what was
    /// said to the owner, and in how many pieces of the INBOX it came.
    fn sync(&mut self) -> (Vec<String>, usize) {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let (mut totals, mut pieces) = (SyncTotals::default(), 0usize);
        let s = &mut self.s;
        mailcraft_net::sync_with(&request, &AtomicBool::new(false), &mut |piece| {
            pieces += piece.folders.iter().filter(|f| f.remote == "INBOX").count();
            apply_sync_part(s, &mut totals, piece).unwrap();
            assert!(s.mailbox.clone().repair().is_empty(), "{:?}", s.mailbox.clone().repair());
        });
        (totals.errors, pieces)
    }

    fn titled(&self, subject: &str) -> Id {
        let found: Vec<Id> = self.s.mailbox.messages.values().filter(|m| m.subject == subject).map(|m| m.id).collect();
        assert_eq!(found.len(), 1, "{subject}: {found:?}");
        found[0]
    }

    /// The server numbers the INBOX anew; the real Three is uid 1, and `lookalike` (when there is
    /// one) uid 2, the oldest, so that they come in the last piece.
    fn renumber(&mut self, lookalike: bool) {
        let mut inbox = vec![message(1, eml("Three", "three", "The real one."))];
        if lookalike {
            inbox.push(message(2, eml("Three", "three", "Not the real one.")));
        }
        inbox.extend(filler(3));
        let mut world = self.imap.world();
        let m = world.mailbox("INBOX");
        (m.uid_validity, m.messages, m.next_uid) = (99, inbox, 0);
    }

    /// The INBOX on the server, by UID: the text, and whether it is marked deleted.
    fn threes_on_server(&self) -> Vec<(u32, bool)> {
        let mut world = self.imap.world();
        world
            .mailbox("INBOX")
            .messages
            .iter()
            .filter(|m| String::from_utf8_lossy(&m.body).contains("Subject: Three"))
            .map(|m| (m.uid, m.has("\\Deleted")))
            .collect()
    }

    fn archived_on_server(&self) -> usize {
        self.imap.world().mailbox("Archive").messages.len()
    }
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Delete,
    Move,
}

fn change(r: &mut Rig, change: Change) {
    let three = r.titled("Three");
    match change {
        Change::Delete => r.s.run("mail.deletePermanently", &json!({"id": three})).unwrap(),
        Change::Move => {
            let archive = r.s.mailbox.folder_of_kind(r.account, FolderKind::Archive).unwrap();
            r.s.run("mail.move", &json!({"ids": [three], "folder": archive})).unwrap()
        }
    };
}

#[test]
fn two_copies_in_a_later_piece_are_both_left_on_the_server() {
    for kind in [Change::Delete, Change::Move] {
        let mut r = Rig::new();
        change(&mut r, kind);
        r.renumber(true);
        let (said, pieces) = r.sync();
        assert!(pieces >= 2, "{kind:?}: the INBOX came in {pieces} piece(s)");
        assert!(said.iter().any(|n| n.contains("not made on the server")), "{kind:?}: {said:?}");
        // The next runs change nothing on the server.
        r.sync();
        r.sync();
        assert_eq!(r.threes_on_server(), [(1, false), (2, false)], "{kind:?}");
        assert_eq!(r.archived_on_server(), 0, "{kind:?}");
        assert!(r.s.mailbox.pending.is_empty(), "{kind:?}: {:?}", r.s.mailbox.pending);
    }
}

#[test]
fn one_copy_gets_the_change_once_the_mailbox_is_read_whole() {
    for kind in [Change::Delete, Change::Move] {
        let mut r = Rig::new();
        change(&mut r, kind);
        r.renumber(false);
        r.sync();
        r.sync();
        match kind {
            Change::Delete => {
                assert!(r.threes_on_server().iter().all(|(_, deleted)| *deleted) || r.threes_on_server().is_empty(), "{:?}", r.threes_on_server())
            }
            Change::Move => assert_eq!((r.threes_on_server().iter().filter(|(_, d)| !d).count(), r.archived_on_server()), (0, 1)),
        }
        assert!(r.s.mailbox.pending.is_empty(), "{kind:?}: {:?}", r.s.mailbox.pending);
    }
}
