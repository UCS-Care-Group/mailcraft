// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive from end to end: the engine's `sync_request` and `apply_sync` with
//! `mailcraft_net::sync` in between, against the in-process fake servers; and the same with the
//! answer taken in piece by piece (`sync_with`, `apply_sync_part`), as the desktop app does it.
//! This is the only place the three meet (the network crate can't depend on the engine), so it
//! lives with the command line program, which runs the same loop.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use std::sync::atomic::{AtomicBool, Ordering};

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, FakeSmtp, ImapWorld, SmtpWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{SyncTotals, apply_sync, apply_sync_part, sync_request, unpushed};
use mailcraft_model::{Account, FlagStatus, FolderKind, Id, Mailbox};
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
            mailbox(
                "INBOX",
                "\\HasNoChildren",
                11,
                vec![message(1, "\\Seen", "One"), message(2, "", "Two"), message(3, "", "Three"), message(4, "", "Four")],
            ),
            mailbox("Archive", "\\HasNoChildren \\Archive", 21, vec![message(1, "\\Seen", "Filed")]),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 31, Vec::new()),
            mailbox("Deleted Items", "\\HasNoChildren \\Trash", 41, Vec::new()),
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
    fn new(can_move: bool, uidplus: bool) -> Rig {
        Rig::with(world(can_move, uidplus))
    }

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

    /// One Send/Receive, as the app does it.
    fn sync(&mut self) -> Value {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors, Vec::<String>::new());
        let applied = apply_sync(&mut self.s, result).unwrap();
        assert!(self.s.mailbox.clone().repair().is_empty(), "{:?}", self.s.mailbox.clone().repair());
        applied
    }

    /// One Send/Receive as a host that shows mail while it arrives does it. The pieces of the
    /// answer are counted from 1: the first `take` are taken in as they come, and nobody is
    /// there for the others; once piece `stop_at` has come, the run is called off. Returns what
    /// the pieces that were taken in came to, and every piece's line for the status bar.
    fn sync_in_pieces(&mut self, take: usize, stop_at: usize) -> (SyncTotals, Vec<String>) {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let cancel = AtomicBool::new(false);
        let (mut totals, mut lines) = (SyncTotals::default(), Vec::new());
        mailcraft_net::sync_with(&request, &cancel, &mut |piece| {
            assert_eq!(piece.errors, Vec::<String>::new());
            lines.push(piece.progress.clone());
            if lines.len() <= take {
                apply_sync_part(&mut self.s, &mut totals, piece).unwrap();
                assert!(self.s.mailbox.clone().repair().is_empty(), "{:?}", self.s.mailbox.clone().repair());
            }
            if lines.len() >= stop_at {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        (totals, lines)
    }

    /// Run until nothing waits to go up and nothing comes down, then hold the mailbox against
    /// the server: every message there is here once, under its UID, in the one folder that
    /// follows its mailbox, and the account has no other mail.
    fn settled(&mut self) {
        for _ in 0..3 {
            self.sync();
        }
        let quiet = self.sync();
        assert_eq!(
            (quiet["new"].as_u64(), quiet["removed"].as_u64(), quiet["updated"].as_u64(), quiet["pushed"].as_u64()),
            (Some(0), Some(0), Some(0), Some(0))
        );
        assert!(self.waiting() == 0 && self.s.mailbox.pending.is_empty(), "{:?}", self.s.mailbox.pending);
        let world = self.imap.world();
        let mut there_in_all = 0;
        for mailbox in &world.mailboxes {
            let following: Vec<Id> =
                self.s.mailbox.folders.iter().filter(|f| f.account == self.account && f.remote == mailbox.raw).map(|f| f.id).collect();
            assert_eq!(following.len(), 1, "folders for {}", mailbox.raw);
            let subject =
                |m: &FakeMessage| String::from_utf8_lossy(&m.body).lines().find_map(|line| line.strip_prefix("Subject: ").map(str::to_string));
            let mut there: Vec<(u32, String)> =
                mailbox.messages.iter().filter(|m| !m.has("\\Deleted")).map(|m| (m.uid, subject(m).unwrap())).collect();
            let mut here: Vec<(u32, String)> = self.s.mailbox.messages_in(following[0]).map(|m| (m.uid, m.subject.clone())).collect();
            there.sort();
            here.sort();
            assert_eq!(here, there, "{}", mailbox.raw);
            there_in_all += there.len();
        }
        assert_eq!(self.s.mailbox.messages.values().filter(|m| m.account == self.account).count(), there_in_all);
    }

    fn folder(&self, kind: FolderKind) -> Id {
        self.s.mailbox.folder_of_kind(self.account, kind).unwrap()
    }

    fn titled(&self, subject: &str) -> Id {
        let found: Vec<Id> = self.s.mailbox.messages.values().filter(|m| m.subject == subject).map(|m| m.id).collect();
        assert_eq!(found.len(), 1, "{subject}: {found:?}");
        found[0]
    }

    fn server(&self, raw: &str) -> Vec<(u32, String)> {
        self.imap.world().mailbox(raw).messages.iter().map(|m| (m.uid, m.flags.clone())).collect()
    }

    fn total(&self) -> usize {
        self.s.mailbox.messages.len()
    }

    fn waiting(&self) -> usize {
        unpushed(&self.s, self.account)
    }
}

fn check(can_move: bool, uidplus: bool) {
    let mut r = Rig::new(can_move, uidplus);
    let (inbox, archive, deleted, sent) =
        (r.folder(FolderKind::Inbox), r.folder(FolderKind::Archive), r.folder(FolderKind::Deleted), r.folder(FolderKind::Sent));
    assert_eq!(r.sync()["new"], 5);
    assert_eq!((r.s.mailbox.count(inbox), r.s.mailbox.count(archive), r.waiting()), (4, 1, 0));

    // Mark read and flag here: pushed, and it stays that way.
    let (one, two, three, four) = (r.titled("One"), r.titled("Two"), r.titled("Three"), r.titled("Four"));
    r.run("mail.markRead", json!({"id": two}));
    r.run("mail.flag", json!({"id": three}));
    assert_eq!(r.waiting(), 2);
    let applied = r.sync();
    assert_eq!((applied["pushed"].as_u64(), applied["updated"].as_u64(), r.waiting()), (Some(2), Some(0), 0));
    assert_eq!(r.server("INBOX"), [(1, "\\Seen".into()), (2, "\\Seen".into()), (3, "\\Flagged".into()), (4, String::new())]);
    for _ in 0..2 {
        assert_eq!(r.sync()["updated"], 0);
        let m = |id| r.s.mailbox.message(id).map(|m| (m.read, m.flag)).unwrap();
        assert_eq!((m(two), m(three)), ((true, FlagStatus::None), (false, FlagStatus::Flagged)));
    }

    // A change on the server comes down; changes on both sides to different flags both hold;
    // the same flag changed on both sides: the change made here wins.
    r.imap.world().mailbox("INBOX").messages[3].flags = "\\Seen \\Answered".into();
    assert_eq!(r.sync()["updated"], 1);
    assert!(r.s.mailbox.message(four).is_some_and(|m| m.read && m.answered));
    r.run("mail.markUnread", json!({"id": four}));
    r.imap.world().mailbox("INBOX").messages[3].flags = "\\Seen \\Answered \\Flagged".into();
    r.run("mail.markUnread", json!({"id": one}));
    r.imap.world().mailbox("INBOX").messages[0].flags = "\\Seen \\Flagged".into();
    r.sync();
    assert_eq!(r.server("INBOX")[3], (4, "\\Answered \\Flagged".into()));
    assert_eq!(r.server("INBOX")[0], (1, "\\Flagged".into()));
    let m = r.s.mailbox.message(four).unwrap();
    assert_eq!((m.read, m.flag, m.answered), (false, FlagStatus::Flagged, true));
    assert_eq!((r.s.mailbox.message(one).map(|m| (m.read, m.flag)), r.waiting()), (Some((false, FlagStatus::Flagged)), 0));

    // A flag changed while the run is under way is not lost.
    r.run("mail.markRead", json!({"id": four}));
    let mut request = sync_request(&r.s, r.account).unwrap();
    request.password = PASSWORD.into();
    let result = mailcraft_net::sync(&request);
    r.run("mail.clearFlag", json!({"id": four}));
    r.run("mail.markUnread", json!({"id": four}));
    apply_sync(&mut r.s, result).unwrap();
    let m = r.s.mailbox.message(four).unwrap();
    assert_eq!((m.read, m.flag, r.waiting()), (false, FlagStatus::None, 1));
    r.sync();
    assert_eq!((r.server("INBOX")[3].clone(), r.waiting()), ((4, "\\Answered".into()), 0));

    // Read, then archive: one message on both sides, read, and no duplicate after the next runs.
    r.run("mail.markRead", json!({"id": two}));
    r.run("mail.markUnread", json!({"id": two}));
    r.run("mail.markRead", json!({"id": four}));
    r.run("mail.archive", json!({"ids": [two, four]}));
    let total = r.total();
    for round in 0..3 {
        r.sync();
        assert_eq!((r.total(), r.s.mailbox.count(archive), r.s.mailbox.count(inbox), r.waiting()), (total, 3, 2, 0), "round {round}");
        let m = |id| r.s.mailbox.message(id).map(|m| (m.folder, m.uid != 0, m.read)).unwrap();
        assert_eq!((m(two), m(four)), ((archive, true, false), (archive, true, true)), "round {round}");
    }
    let on_server: Vec<(u32, String)> = r.server("Archive");
    assert_eq!(on_server.iter().map(|(_, flags)| flags.as_str()).collect::<Vec<_>>(), ["\\Seen", "", "\\Answered \\Seen"]);
    let left: Vec<u32> = r.server("INBOX").iter().filter(|(_, flags)| !flags.contains("\\Deleted")).map(|(uid, _)| *uid).collect();
    assert_eq!(left, [1, 3]);
    // The local UIDs are the server's.
    let uid = |id| r.s.mailbox.message(id).map(|m| m.uid).unwrap();
    assert_eq!((uid(two), uid(four)), (on_server[1].0, on_server[2].0));

    // Into a folder that only exists here: made on the server, and it is one folder afterwards.
    let receipts = r.run("folder.new", json!({"name": "Receipts", "parent": null, "account": r.account}))["id"].as_u64().unwrap();
    r.run("mail.move", json!({"id": three, "folder": receipts}));
    let folders = r.s.mailbox.folders.len();
    for round in 0..3 {
        r.sync();
        assert_eq!((r.s.mailbox.folders.len(), r.s.mailbox.count(receipts), r.total(), r.waiting()), (folders, 1, total, 0), "round {round}");
    }
    assert_eq!(r.s.mailbox.folder(receipts).unwrap().remote, "Receipts");
    assert_eq!(r.server("Receipts").len(), 1);
    assert_eq!(r.imap.world().subscribed, ["Receipts"]);
    assert!(r.s.mailbox.message(three).is_some_and(|m| m.uid != 0 && m.flag == FlagStatus::Flagged));

    // Delete: to the server's trash. Delete again: for good.
    r.run("mail.delete", json!({"id": one}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(deleted), r.total(), r.waiting()), (1, total, 0));
    }
    assert_eq!(r.server("Deleted Items").len(), 1);
    r.run("mail.delete", json!({"id": one}));
    // There and back before a run: nothing.
    r.run("mail.move", json!({"id": two, "folder": inbox}));
    r.run("mail.move", json!({"id": two, "folder": archive}));
    assert_eq!(r.waiting(), 1);
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(deleted), r.total(), r.waiting()), (0, total - 1, 0));
    }
    let trash = r.server("Deleted Items");
    assert!(trash.iter().all(|(_, flags)| flags.contains("\\Deleted")) && (uidplus == trash.is_empty()), "{trash:?}");
    // A move, then for good, before any run.
    r.run("mail.delete", json!({"id": two}));
    r.run("mail.delete", json!({"id": two}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(archive), r.s.mailbox.count(deleted), r.total(), r.waiting()), (2, 0, total - 2, 0));
    }
    assert_eq!(r.server("Archive").iter().filter(|(_, flags)| !flags.contains("\\Deleted")).count(), 2);

    // Sent mail: one copy on the server, one here.
    r.run("mail.new", json!({"to": "bob@example.org", "subject": "Hello", "body": "Hi Bob", "account": r.account}));
    r.run("compose.send", json!({}));
    let applied = r.sync();
    assert_eq!(
        (applied["sent"].as_u64(), applied["new"].as_u64(), applied["matched"].as_u64(), applied["summary"].as_str()),
        (Some(1), Some(0), Some(1), Some("1 message sent"))
    );
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(sent), r.total(), r.waiting()), (1, total - 1, 0));
    }
    assert_eq!((r.smtp.world().delivered.len(), r.server("Sent Items")), (1, vec![(1, "\\Seen".to_string())]));
    assert!(r.s.mailbox.messages_in(sent).all(|m| m.uid == 1));
    // An undo while the run is under way: the server is asked to put the message back (when it
    // said where it put it), and nothing is lost either way.
    let filed = r.titled("Filed");
    r.run("mail.move", json!({"id": filed, "folder": inbox}));
    let mut request = sync_request(&r.s, r.account).unwrap();
    request.password = PASSWORD.into();
    let result = mailcraft_net::sync(&request);
    assert_eq!((result.errors.len(), result.moved.len()), (0, 1));
    r.run("edit.undo", json!({}));
    apply_sync(&mut r.s, result).unwrap();
    for _ in 0..3 {
        r.sync();
    }
    let here = r.s.mailbox.message(filed).map(|m| m.folder);
    let inbox_has = r.server("INBOX").iter().filter(|(_, flags)| !flags.contains("\\Deleted")).count();
    let archive_has = r.server("Archive").iter().filter(|(_, flags)| !flags.contains("\\Deleted")).count();
    if uidplus {
        // Back in Archive, here and there.
        assert_eq!((here, r.s.mailbox.count(archive), r.s.mailbox.count(inbox), inbox_has, archive_has, r.waiting()), (Some(archive), 2, 0, 0, 2, 0));
    } else {
        // The server couldn't say where it put it: it is in the Inbox there, and here as the server has it.
        assert_eq!((r.s.mailbox.count(archive), r.s.mailbox.count(inbox), inbox_has, archive_has, r.waiting()), (1, 1, 1, 1, 0));
    }
    assert_eq!(r.s.mailbox.messages.values().filter(|m| m.subject == "Filed").count(), 1);
    // Never a sweep of what other programs marked.
    assert!(!r.imap.world().log.iter().any(|line| line.ends_with(" EXPUNGE") || line.ends_with(" CLOSE")));
}

/// Whatever the server can do (MOVE or not, UIDPLUS or not), what is changed here reaches it,
/// what changes there comes down, and no message ends up twice or lost.
#[test]
fn changes_made_here_and_on_the_server_meet() {
    for (can_move, uidplus) in [(true, true), (true, false), (false, true), (false, false)] {
        check(can_move, uidplus);
    }
}

// ---- the answer taken in piece by piece ----

/// More mail than one piece holds, waiting in the server's Inbox.
fn deliver(r: &Rig, uids: std::ops::RangeInclusive<u32>) {
    r.imap.world().mailbox("INBOX").messages.extend(uids.map(|uid| message(uid, "", &format!("Bulk {uid}"))));
}

#[test]
fn mail_shows_while_it_arrives() {
    let mut r = Rig::new(true, true);
    deliver(&r, 5..=124);
    let (inbox, archive) = (r.folder(FolderKind::Inbox), r.folder(FolderKind::Archive));
    let mut request = sync_request(&r.s, r.account).unwrap();
    request.password = PASSWORD.into();
    let mut totals = SyncTotals::default();
    // What the status bar said, and how much there was to see, after each piece.
    let mut seen: Vec<(String, usize, usize)> = Vec::new();
    let mut newest_first = false;
    mailcraft_net::sync_with(&request, &AtomicBool::new(false), &mut |piece| {
        assert_eq!(piece.errors, Vec::<String>::new());
        apply_sync_part(&mut r.s, &mut totals, piece).unwrap();
        let shown = r.s.mailbox.count(inbox);
        if shown > 0 && seen.last().is_some_and(|(_, before, _)| *before == 0) {
            // The first of it to show is the latest mail, not the oldest.
            let has = |subject: &str| r.s.mailbox.messages_in(inbox).any(|m| m.subject == subject);
            newest_first = has("Bulk 124") && !has("One");
        }
        seen.push((r.s.sync_status.clone(), shown, r.s.mailbox.count(archive)));
    });
    let lines: Vec<&str> = seen.iter().map(|(line, _, _)| line.as_str()).collect();
    // The folders are all there to see before the first of their mail.
    assert_eq!(&lines[..3], ["Connecting to the mail server", "Found 4 folders", "Checking Inbox (1 of 4)"]);
    // While the Inbox comes in, the status bar says how far it is, and that much is there to see.
    let counted: Vec<&(String, usize, usize)> = seen.iter().filter(|(line, _, _)| line.starts_with("Inbox: ")).collect();
    assert!(!counted.is_empty() && newest_first, "{lines:?}");
    for (line, shown, filed) in counted {
        assert_eq!((line.clone(), *filed), (format!("Inbox: {shown} of 124"), 0));
    }
    assert!(seen.windows(2).all(|pair| pair[0].1 <= pair[1].1 && pair[0].2 <= pair[1].2), "{seen:?}");
    // Other folders follow once the Inbox is whole.
    assert!(seen.iter().all(|(_, shown, filed)| *filed == 0 || *shown == 124), "{seen:?}");
    assert!(
        lines.contains(&"Checking Sent Items (2 of 4)")
            && lines.contains(&"Checking Archive (3 of 4)")
            && lines.contains(&"Checking Deleted Items (4 of 4)")
    );
    // At the end it says how the whole run went.
    assert!(lines.last().unwrap().starts_with("125 new messages — "), "{lines:?}");
    assert_eq!((totals.new, totals.finished, totals.stopped, totals.errors.len()), (125, true, false, 0));
    assert_eq!((r.s.mailbox.count(inbox), r.s.mailbox.count(archive), r.waiting()), (124, 1, 0));
    // The next run has nothing to bring, in pieces or whole.
    let (totals, lines) = r.sync_in_pieces(usize::MAX, usize::MAX);
    assert_eq!((totals.summary().as_str(), totals.new, lines.last().map(String::as_str)), ("Up to date", 0, Some("")));
    assert!(r.s.sync_status.starts_with("Up to date — "));
    r.settled();
}

/// A mailbox with a first run behind it, things changed here since, mail waiting to go, and new
/// mail waiting on the server: a run with every kind of work to do.
fn busy(can_move: bool, uidplus: bool) -> Rig {
    let mut r = Rig::new(can_move, uidplus);
    r.sync();
    let (one, two, three, four) = (r.titled("One"), r.titled("Two"), r.titled("Three"), r.titled("Four"));
    // Deleted, which the server hears of, and then deleted for good.
    r.run("mail.delete", json!({"id": one}));
    r.sync();
    r.run("mail.delete", json!({"id": one}));
    r.run("mail.markRead", json!({"id": two}));
    // Flagged and moved into a folder that only exists here; filed; and a message to send.
    r.run("mail.flag", json!({"id": three}));
    let receipts = r.run("folder.new", json!({"name": "Receipts", "parent": null, "account": r.account}))["id"].as_u64().unwrap();
    r.run("mail.move", json!({"id": three, "folder": receipts}));
    r.run("mail.archive", json!({"id": four}));
    r.run("mail.new", json!({"to": "bob@example.org", "subject": "Hello", "body": "Hi Bob", "account": r.account}));
    r.run("compose.send", json!({}));
    deliver(&r, 5..=64);
    assert!(r.waiting() >= 4);
    r
}

/// What a busy run must leave, however much of its answer was taken in.
fn nothing_lost_nothing_twice(r: &mut Rig, what: &str) {
    r.settled();
    let count = |subject: &str| r.s.mailbox.messages.values().filter(|m| m.account == r.account && m.subject == subject).count();
    assert_eq!((count("One"), count("Two"), count("Three"), count("Four"), count("Bulk 5"), count("Bulk 64")), (0, 1, 1, 1, 1, 1), "{what}");
    // The folder that only existed here is one folder, and follows the mailbox made for it.
    let receipts: Vec<_> = r.s.mailbox.folders.iter().filter(|f| f.account == r.account && f.name.starts_with("Receipts")).collect();
    assert_eq!(receipts.iter().map(|f| (f.name.as_str(), f.remote.as_str())).collect::<Vec<_>>(), [("Receipts", "Receipts")], "{what}");
    let (three, two) = (r.titled("Three"), r.titled("Two"));
    let m = r.s.mailbox.message(three).unwrap();
    assert_eq!((m.folder, m.flag, m.uid != 0), (receipts[0].id, FlagStatus::Flagged, true), "{what}");
    assert!(r.s.mailbox.message(two).is_some_and(|m| m.read), "{what}");
    assert_eq!(r.server("Receipts").len(), 1, "{what}");
    assert!(!r.imap.world().log.iter().any(|line| line.ends_with(" EXPUNGE") || line.ends_with(" CLOSE")));
}

/// Whatever the server can do, and however many pieces of a run's answer are taken in before
/// nobody is there for the rest (the program was closed, say), the runs that follow put
/// everything in its place: no message is lost, none shows twice, and nothing that was changed
/// here is forgotten.
#[test]
fn whatever_of_a_run_is_taken_in_nothing_is_lost_and_nothing_shows_twice() {
    for (can_move, uidplus) in [(true, true), (false, false)] {
        // How many pieces such a run has, and which of them says the message went.
        let (_, lines) = busy(can_move, uidplus).sync_in_pieces(usize::MAX, usize::MAX);
        let said_sent = lines.iter().position(|line| line == "Sent 1 of 1").unwrap() + 1;
        assert!(
            lines.len() >= 10 && lines.iter().any(|line| line.starts_with("Updated ")) && lines.iter().any(|line| line.starts_with("Inbox: ")),
            "{lines:?}"
        );
        for taken in 0..=lines.len() {
            let what = format!("{taken} of {} pieces taken in (MOVE {can_move}, UIDPLUS {uidplus})", lines.len());
            let mut r = busy(can_move, uidplus);
            // The run goes through on the servers; here, only its first pieces arrive.
            r.sync_in_pieces(taken, usize::MAX);
            nothing_lost_nothing_twice(&mut r, &what);
            // A message goes out a second time only when the mailbox never heard that it went:
            // that is the piece that must not be lost, and it is the first to be said.
            let went = r.smtp.world().delivered.len();
            assert_eq!(went, if taken >= said_sent { 1 } else { 2 }, "{what}");
        }
    }
}

/// The same, for a run that is called off after any of its pieces: everything it hands over is
/// taken in, the next runs do the rest, and mail goes out exactly once.
#[test]
fn a_run_called_off_at_any_point_is_finished_by_the_next() {
    for (can_move, uidplus) in [(true, true), (false, false)] {
        let (_, lines) = busy(can_move, uidplus).sync_in_pieces(usize::MAX, usize::MAX);
        for stop_at in 1..=lines.len() {
            let what = format!("called off after piece {stop_at} of {} (MOVE {can_move}, UIDPLUS {uidplus})", lines.len());
            let mut r = busy(can_move, uidplus);
            let (totals, came) = r.sync_in_pieces(usize::MAX, stop_at);
            // What follows a call-off is short: at most connecting, the list of folders that had
            // to be read anyway, the copy of a message that went, and the last piece.
            assert!(totals.finished && came.len() <= stop_at + 4, "{what}: {came:?}");
            if totals.stopped {
                assert!(r.s.sync_status.starts_with("Stopped"), "{what}: {}", r.s.sync_status);
            }
            nothing_lost_nothing_twice(&mut r, &what);
            assert_eq!(r.smtp.world().delivered.len(), 1, "{what}");
        }
        // Called off in the middle of the Inbox: it says so, and what came is there to see.
        let mut r = busy(can_move, uidplus);
        let stop_at = lines.iter().position(|line| line.starts_with("Inbox: ")).unwrap() + 1;
        let (totals, _) = r.sync_in_pieces(usize::MAX, stop_at);
        assert!(totals.stopped && totals.new > 0 && totals.new < 60, "{totals:?}");
        assert!(r.s.sync_status.starts_with(&format!("Stopped ({} new messages, 1 sent) — ", totals.new)), "{}", r.s.sync_status);
    }
}

// ---- Gmail ----

/// A Gmail account, as Google describes it for IMAP: labels as mailboxes, and every message that
/// is not in the trash also in all mail.
fn gmail() -> ImapWorld {
    let inbox = vec![message(1, "\\Seen", "One"), message(2, "", "Two"), message(3, "", "Three"), message(4, "", "Four")];
    let mut all = inbox.clone();
    all.push(message(5, "\\Seen", "Archived before"));
    ImapWorld {
        user: "ada@example.org".into(),
        password: PASSWORD.into(),
        can_move: true,
        uidplus: true,
        gmail: true,
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, inbox),
            mailbox("[Gmail]", "\\Noselect \\HasChildren", 0, Vec::new()),
            mailbox("[Gmail]/All Mail", "\\HasNoChildren \\All", 51, all),
            mailbox("[Gmail]/Sent Mail", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("[Gmail]/Trash", "\\HasNoChildren \\Trash", 61, Vec::new()),
        ],
        ..Default::default()
    }
}

/// At Gmail, Archive takes the Inbox label away and makes no label of its own; Delete for good
/// takes the message out of all mail too, by way of the trash. The runs after either find
/// everything where it was left: nothing comes down again, and nothing is lost.
#[test]
fn gmail_archives_into_all_mail_and_deletes_for_good_through_the_trash() {
    let mut r = Rig::with(gmail());
    let (inbox, archive, deleted) = (r.folder(FolderKind::Inbox), r.folder(FolderKind::Archive), r.folder(FolderKind::Deleted));
    // All mail is not downloaded: only the Inbox's four come.
    assert_eq!(r.sync()["new"], 4);
    assert_eq!((r.s.mailbox.count(inbox), r.s.mailbox.count(archive)), (4, 0));
    let (one, two, three, four) = (r.titled("One"), r.titled("Two"), r.titled("Three"), r.titled("Four"));
    let all_mail = |r: &Rig| r.server("[Gmail]/All Mail").iter().map(|(uid, _)| *uid).collect::<Vec<_>>();
    let inbox_uids = |r: &Rig| r.server("INBOX").iter().map(|(uid, _)| *uid).collect::<Vec<_>>();

    // Archive: out of the Inbox there, still in all mail, and no "Archive" label.
    r.run("mail.archive", json!({"ids": [one, two]}));
    let total = r.total();
    for round in 0..3 {
        let applied = r.sync();
        assert_eq!((applied["new"].as_u64(), applied["removed"].as_u64()), (Some(0), Some(0)), "round {round}");
        assert_eq!((r.total(), r.s.mailbox.count(archive), r.s.mailbox.count(inbox), r.waiting()), (total, 2, 2, 0), "round {round}");
        assert!(r.s.mailbox.pending.is_empty(), "round {round}");
    }
    assert_eq!(inbox_uids(&r), [3, 4]);
    assert_eq!(all_mail(&r), [1, 2, 3, 4, 5]);
    let world = r.imap.world();
    assert!(world.mailboxes.iter().all(|m| !m.raw.eq_ignore_ascii_case("Archive")) && world.subscribed.is_empty());
    assert_eq!((world.sent("CREATE"), world.sent("SUBSCRIBE")), (0, 0));
    // All mail was put into, never read whole.
    assert!(!world.log.iter().any(|line| line.contains("EXAMINE") && line.contains("All Mail")));
    drop(world);
    // The Archive folder follows all mail, and the archived messages have their UIDs there.
    let folder = r.s.mailbox.folder(archive).unwrap();
    assert_eq!((folder.remote.as_str(), folder.uid_validity), ("[Gmail]/All Mail", 51));
    let uid = |r: &Rig, id| r.s.mailbox.message(id).map(|m| (m.folder, m.uid)).unwrap();
    assert_eq!((uid(&r, one), uid(&r, two)), ((archive, 1), (archive, 2)));
    let downloads = |r: &Rig| r.imap.world().log.iter().filter(|line| line.contains("BODY.PEEK[]")).count();
    assert_eq!(downloads(&r), 4);

    // Back to the Inbox: the label comes back, and the message is here once.
    r.run("mail.move", json!({"id": two, "folder": inbox}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.total(), r.s.mailbox.count(archive), r.s.mailbox.count(inbox), r.waiting()), (total, 1, 3, 0));
    }
    assert_eq!((inbox_uids(&r), all_mail(&r)), (vec![3, 4, 5], vec![1, 2, 3, 4, 5]));
    assert_eq!(downloads(&r), 4);

    // Delete for good, in the Inbox and in Archive: gone from all mail and from the trash.
    r.run("mail.deletePermanently", json!({"ids": [three, one]}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.total(), r.s.mailbox.count(archive), r.s.mailbox.count(inbox), r.waiting()), (total - 2, 0, 2, 0));
    }
    assert_eq!((inbox_uids(&r), all_mail(&r)), (vec![4, 5], vec![2, 4, 5]));
    assert!(r.server("[Gmail]/Trash").is_empty());
    assert_eq!(r.s.mailbox.count(deleted), 0);

    // Delete: to the trash, and out of all mail; then for good from there.
    r.run("mail.delete", json!({"id": four}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(deleted), r.s.mailbox.count(inbox), r.total(), r.waiting()), (1, 1, total - 2, 0));
    }
    assert_eq!((inbox_uids(&r), all_mail(&r), r.server("[Gmail]/Trash").len()), (vec![5], vec![2, 5], 1));
    r.run("mail.delete", json!({"id": four}));
    for _ in 0..2 {
        r.sync();
        assert_eq!((r.s.mailbox.count(deleted), r.total(), r.waiting()), (0, total - 3, 0));
    }
    assert!(r.server("[Gmail]/Trash").is_empty());
    assert_eq!(downloads(&r), 4);
    // Never a sweep of what other programs marked.
    assert!(!r.imap.world().log.iter().any(|line| line.ends_with(" EXPUNGE") || line.ends_with(" CLOSE")));
}

// ---- Messages to download per folder ----

/// The Inbox of an account of this computer alone: a folder the server never hears of.
fn kept_here(r: &mut Rig) -> Id {
    let local = r.s.mailbox.add_account(Account { name: "On this computer".into(), ..Default::default() });
    r.s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap()
}

#[test]
fn mail_moved_to_another_account_does_not_come_down_again() {
    // Without a limit, and with one that leaves the oldest out.
    for (limit, left) in [(0, 3), (3, 2)] {
        let mut r = Rig::new(true, true);
        r.run("file.options", json!({"syncLimit": limit}));
        r.sync();
        let (inbox, local, four) = (r.folder(FolderKind::Inbox), kept_here(&mut r), r.titled("Four"));
        r.run("mail.move", json!({"ids": [four], "folder": local}));
        for _ in 0..2 {
            assert_eq!(r.sync()["new"].as_u64(), Some(0), "limit {limit}");
        }
        // The server still has it, and here it is in the other account's folder alone.
        assert_eq!((r.server("INBOX").len(), r.s.mailbox.count(inbox)), (4, left), "limit {limit}");
        assert_eq!(r.s.mailbox.message(r.titled("Four")).map(|m| m.folder), Some(local));
    }
}

#[test]
fn a_raised_limit_brings_the_older_mail_down_once() {
    let mut r = Rig::new(true, true);
    r.run("file.options", json!({"syncLimit": 2}));
    r.sync();
    let (inbox, local, four) = (r.folder(FolderKind::Inbox), kept_here(&mut r), r.titled("Four"));
    assert_eq!(r.s.mailbox.count(inbox), 2);
    // What a raised limit takes in is the older mail, not what left the folder here meanwhile.
    r.run("mail.move", json!({"ids": [four], "folder": local}));
    r.run("file.options", json!({"syncLimit": 3}));
    assert_eq!(r.sync()["new"].as_u64(), Some(1));
    r.titled("Two");
    r.run("file.options", json!({"syncLimit": 0}));
    assert_eq!(r.sync()["new"].as_u64(), Some(1));
    r.titled("One");
    // And it comes down once.
    assert_eq!(r.sync()["new"].as_u64(), Some(0));
    let mut here: Vec<String> = r.s.mailbox.messages_in(inbox).map(|m| m.subject.clone()).collect();
    here.sort();
    assert_eq!(here, ["One", "Three", "Two"]);
    assert_eq!(r.s.mailbox.message(r.titled("Four")).map(|m| m.folder), Some(local));
}

/// A run called off while the older mail a raised limit lets in comes down: the next runs bring
/// the rest, and nothing comes twice.
#[test]
fn older_mail_called_off_halfway_comes_down_with_the_next_run() {
    let raised = || {
        let mut r = Rig::new(true, true);
        deliver(&r, 5..=124);
        r.run("file.options", json!({"syncLimit": 2}));
        r.sync();
        assert_eq!(r.s.mailbox.count(r.folder(FolderKind::Inbox)), 2);
        r.run("file.options", json!({"syncLimit": 0}));
        r
    };
    let (_, lines) = raised().sync_in_pieces(usize::MAX, usize::MAX);
    assert!(lines.iter().filter(|line| line.starts_with("Inbox: ")).count() >= 2, "{lines:?}");
    for stop_at in 1..=lines.len() {
        let mut r = raised();
        r.sync_in_pieces(usize::MAX, stop_at);
        r.settled();
    }
}
