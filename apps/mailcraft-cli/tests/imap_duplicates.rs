// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Repro attempts for a report of IMAP mail shown three times (same Message-ID): one server
//! message must never become more than one message here. Fake servers and invented mail only.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

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
    smtp: FakeSmtp,
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
        Rig { imap, smtp, s, account }
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

// ---- the server numbers its mailboxes anew, or gives its messages other UIDs ----

fn bulk(n: u32) -> ImapWorld {
    let inbox: Vec<FakeMessage> = (1..=n).map(|uid| message(uid, if uid % 3 == 0 { "\\Seen" } else { "" }, &format!("Bulk {uid}"))).collect();
    let archive: Vec<FakeMessage> = (1..=n).map(|uid| message(uid, "\\Seen", &format!("Old {uid}"))).collect();
    ImapWorld {
        user: "ada@example.org".into(),
        password: PASSWORD.into(),
        can_move: true,
        uidplus: true,
        mailboxes: vec![mailbox("INBOX", "\\HasNoChildren", 11, inbox), mailbox("Archive 2019", "\\HasNoChildren", 21, archive)],
        ..Default::default()
    }
}

/// Every Message-ID of the account that is here more than once, with how many times.
fn twice(r: &Rig) -> Vec<(String, usize)> {
    let mut count: std::collections::BTreeMap<String, usize> = Default::default();
    for m in r.s.mailbox.messages.values().filter(|m| m.account == r.account) {
        *count.entry(format!("{}|{}", m.folder, m.message_id)).or_default() += 1;
    }
    count.into_iter().filter(|(_, n)| *n > 1).collect()
}

/// The server gives every message a new UID: with a new UIDVALIDITY (as it must), or, as a
/// broken or rebuilt index may, under the same one. Runs taken in whole or called off anywhere.
#[test]
fn mail_numbered_anew_is_here_once() {
    for new_validity in [true, false] {
        for limit in [0u32, 40] {
            let probe = {
                let mut r = Rig::with(bulk(120));
                r.run("file.options", json!({"syncLimit": limit}));
                r.sync();
                r.sync_called_off(|_| false).len()
            };
            for stop_at in 1..=probe.max(30) {
                let mut r = Rig::with(bulk(120));
                r.run("file.options", json!({"syncLimit": limit}));
                r.sync();
                // Read here, not yet told to the server.
                let some: Vec<u64> = r.s.mailbox.messages.values().filter(|m| m.account == r.account && !m.read).take(5).map(|m| m.id).collect();
                for id in some {
                    r.run("mail.markRead", json!({"id": id}));
                }
                {
                    let mut world = r.imap.world();
                    for mb in &mut world.mailboxes {
                        if new_validity {
                            mb.uid_validity += 100;
                        }
                        for m in &mut mb.messages {
                            m.uid += 1000;
                        }
                        mb.next_uid = 0;
                    }
                }
                let n = std::cell::Cell::new(0);
                r.sync_called_off(|_| {
                    n.set(n.get() + 1);
                    n.get() >= stop_at
                });
                for _ in 0..3 {
                    r.sync();
                }
                let what = format!("new UIDVALIDITY {new_validity}, limit {limit}, called off at {stop_at}");
                assert_eq!(twice(&r), Vec::<(String, usize)>::new(), "{what}");
            }
        }
    }
}

/// The program stops without warning (a crash, a kill) part way through writing a save, and is
/// opened again: whatever of the file is read back, the next runs leave each message here once.
#[test]
fn a_save_cut_short_and_reopened_leaves_mail_here_once() {
    let dir = std::env::temp_dir().join(format!("mc-imap-dup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    for cut in [0u64, 1, 7, 64, 300, 2_000, 10_000] {
        let _ = std::fs::remove_file(&path);
        let mut r = Rig::with(bulk(120));
        r.run("file.options", json!({"syncLimit": 0}));
        // Half of it, then a save, then the rest and more mail, then a second save.
        r.sync_called_off({
            let n = std::cell::Cell::new(0);
            move |_| {
                n.set(n.get() + 1);
                n.get() >= 4
            }
        });
        mailcraft_engine::store::save(&path, &r.s.mailbox).unwrap();
        let first = std::fs::metadata(&path).unwrap().len();
        r.imap.world().mailbox("INBOX").messages.extend((121..=160).map(|uid| message(uid, "", &format!("Bulk {uid}"))));
        r.sync();
        mailcraft_engine::store::save(&path, &r.s.mailbox).unwrap();
        let whole = std::fs::metadata(&path).unwrap().len();
        // Cut the second save short.
        let keep = whole.saturating_sub(cut).max(first);
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(keep).unwrap();
        drop(f);
        let opened = mailcraft_engine::store::open(&path).unwrap();
        r.s = Session::at(opened.mailbox, 1_800_000_000, 0);
        for _ in 0..3 {
            r.sync();
        }
        assert_eq!(twice(&r), Vec::<(String, usize)>::new(), "cut {cut} of {whole} bytes");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

impl Rig {
    /// A run whose pieces are taken in `together` at a time, as the window takes in what waits
    /// (`apply_sync_parts`, #991).
    fn sync_together(&mut self, together: usize) {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let mut pieces = Vec::new();
        mailcraft_net::sync_with(&request, &AtomicBool::new(false), &mut |piece| pieces.push(piece));
        let mut totals = SyncTotals::default();
        while !pieces.is_empty() {
            let rest = pieces.split_off(together.min(pieces.len()));
            mailcraft_engine::cmd::account::apply_sync_parts(&mut self.s, &mut totals, std::mem::replace(&mut pieces, rest)).unwrap();
        }
    }
}

fn many_folders(n: usize, per: u32) -> ImapWorld {
    let mut w = bulk(per);
    for f in 0..n {
        let msgs = (1..=per).map(|uid| message(uid, "\\Seen", &format!("F{f} {uid}"))).collect();
        w.mailboxes.push(mailbox(&format!("Projects/P{f}"), "\\HasNoChildren", 1000 + f as u32, msgs));
    }
    w
}

#[test]
fn pieces_taken_in_together_leave_mail_here_once() {
    for together in [1usize, 2, 3, 7, 256] {
        for renumber in [false, true] {
            let mut r = Rig::with(many_folders(20, 60));
            r.run("file.options", json!({"syncLimit": 0}));
            r.sync_together(together);
            r.imap.world().mailbox("INBOX").messages.extend((121..=200).map(|uid| message(uid, "", &format!("Bulk {uid}"))));
            if renumber {
                let mut world = r.imap.world();
                for mb in &mut world.mailboxes {
                    mb.uid_validity += 7;
                    for m in &mut mb.messages {
                        m.uid += 500;
                    }
                }
            }
            for _ in 0..3 {
                r.sync_together(together);
            }
            assert_eq!(twice(&r), Vec::<(String, usize)>::new(), "together {together}, renumbered {renumber}");
        }
    }
}

/// Undo after a Send/Receive took mail in (or numbered a mailbox anew): the mail stays here once.
#[test]
fn undo_after_a_run_leaves_mail_here_once() {
    for (action, renumber) in [("mail.markRead", false), ("mail.markRead", true), ("mail.delete", false), ("mail.delete", true), ("mail.flag", true)]
    {
        let mut r = Rig::with(bulk(30));
        r.run("file.options", json!({"syncLimit": 0}));
        r.sync();
        let ids: Vec<u64> =
            r.s.mailbox.messages.values().filter(|m| m.account == r.account && m.subject.starts_with("Bulk")).take(3).map(|m| m.id).collect();
        r.run(action, json!({"ids": ids}));
        if renumber {
            let mut world = r.imap.world();
            for mb in &mut world.mailboxes {
                mb.uid_validity += 7;
                for m in &mut mb.messages {
                    m.uid += 500;
                }
            }
        }
        r.imap.world().mailbox("INBOX").messages.extend((31..=40).map(|uid| message(uid, "", &format!("Bulk {uid}"))));
        r.sync();
        let _ = r.s.run("edit.undo", &json!({}));
        for _ in 0..3 {
            r.sync();
        }
        assert_eq!(twice(&r), Vec::<(String, usize)>::new(), "{action}, renumbered {renumber}");
    }
}

/// Two programs with one mailbox file (two computers on a synced or shared folder, where the
/// lock beside it doesn't reach): each takes in the same new mail and saves; the file opened
/// after holds what?
#[test]
fn two_programs_saving_one_file_leave_mail_here_once() {
    let dir = std::env::temp_dir().join(format!("mc-imap-dup-two-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    let _ = std::fs::remove_file(&path);
    let mut a = Rig::with(bulk(40));
    a.run("file.options", json!({"syncLimit": 0}));
    a.sync();
    mailcraft_engine::store::save(&path, &a.s.mailbox).unwrap();
    // The second program opens the same file.
    let mut b_session = Session::at(mailcraft_engine::store::open(&path).unwrap().mailbox, 1_800_000_000, 0);
    std::mem::swap(&mut a.s, &mut b_session);
    a.imap.world().mailbox("INBOX").messages.extend((41..=60).map(|uid| message(uid, "", &format!("Bulk {uid}"))));
    // B takes in the new mail and saves; then A does the same.
    a.sync();
    let b_saved = mailcraft_engine::store::save(&path, &a.s.mailbox);
    std::mem::swap(&mut a.s, &mut b_session);
    a.sync();
    let a_saved = mailcraft_engine::store::save(&path, &a.s.mailbox);
    let opened = mailcraft_engine::store::open(&path).unwrap();
    let repaired = opened.repaired;
    a.s = Session::at(opened.mailbox, 1_800_000_000, 0);
    let after_open = twice(&a).len();
    for _ in 0..2 {
        a.sync();
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        (twice(&a), after_open),
        (Vec::<(String, usize)>::new(), 0),
        "saves {:?} {:?}, repaired {repaired}",
        b_saved.map(|_| ()),
        a_saved.map(|_| ())
    );
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
    // Whatever was decided, no message of the server went anywhere but where the owner asked,
    // and nothing is left waiting.
    assert!(r.s.mailbox.pending.is_empty(), "{:?}", r.s.mailbox.pending);
    assert!(on_server(&r, "Two").iter().all(|name| name == "Projects" || name == "Later"));
}
