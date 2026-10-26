// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail moved between accounts from end to end (#451): a Gmail account against the in-process
//! fake IMAP server and a Microsoft account against the Graph stand-in, with the engine's
//! requests and `mailcraft-net` in between, both ways, and into a second IMAP account and from
//! this computer. The original stays on its server until the copy is made on the other one, or
//! until the message is whole in the mailbox file on this computer; a copy that is refused leaves
//! it there, and one whose answer was lost is found again rather than made twice. Nothing here
//! leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use std::sync::atomic::AtomicBool;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request};
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

const PASSWORD: &str = "hunter2";
/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

fn message(uid: u32, flags: &str, subject: &str) -> FakeMessage {
    let body = format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n",
        subject.to_lowercase()
    );
    FakeMessage { uid, flags: flags.into(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: body.into_bytes(), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// A Gmail account, as Google describes it for IMAP: labels as mailboxes, and every message that
/// is not in the trash also in all mail.
fn gmail() -> ImapWorld {
    let inbox = vec![message(1, "\\Seen", "One"), message(2, "\\Seen", "Two"), message(3, "", "Three"), message(4, "", "Four")];
    ImapWorld {
        user: "ada@example.org".into(),
        password: PASSWORD.into(),
        can_move: true,
        uidplus: true,
        gmail: true,
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, inbox.clone()),
            mailbox("[Gmail]", "\\Noselect \\HasChildren", 0, Vec::new()),
            mailbox("[Gmail]/All Mail", "\\HasNoChildren \\All", 51, inbox),
            mailbox("[Gmail]/Sent Mail", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("[Gmail]/Trash", "\\HasNoChildren \\Trash", 61, Vec::new()),
        ],
        ..Default::default()
    }
}

fn subject(m: &FakeMessage) -> Option<String> {
    String::from_utf8_lossy(&m.body).lines().find_map(|line| line.strip_prefix("Subject: ").map(str::to_string))
}

/// The subjects in a mailbox of a fake IMAP server (what isn't marked deleted).
fn on(imap: &FakeImap, raw: &str) -> Vec<String> {
    let mut world = imap.world();
    let mut there: Vec<String> = world.mailbox(raw).messages.iter().filter(|m| !m.has("\\Deleted")).filter_map(subject).collect();
    there.sort();
    there
}

/// The servers of an IMAP account at `imap`, for `account.add`.
fn servers(email: &str, imap: &FakeImap) -> Value {
    json!({
        "email": email, "displayName": "Ada",
        "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
        "outgoing": {"host": "127.0.0.1", "port": 1, "security": "none"},
    })
}

/// One session with the Gmail account and a Microsoft account, each read once.
struct Rig {
    imap: FakeImap,
    graph: StandIn,
    s: Session,
    gmail: Id,
    ms: Id,
}

impl Rig {
    fn new(address: &str) -> Rig {
        Rig::with(address, gmail())
    }

    fn with(address: &str, world: ImapWorld) -> Rig {
        let imap = Fake::imap(world);
        let graph = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let gmail = s.run("account.add", &servers("ada@example.org", &imap)).unwrap()["id"].as_u64().unwrap();
        let ms = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let mut rig = Rig { imap, graph, s, gmail, ms };
        assert_eq!(rig.sync()["new"], 4);
        rig.run();
        rig
    }

    /// One Send/Receive of the Gmail account.
    fn sync(&mut self) -> Value {
        self.sync_of(self.gmail)
    }

    /// One Send/Receive of an IMAP account, which goes without a problem.
    fn sync_of(&mut self, account: Id) -> Value {
        let (applied, errors) = self.sync_said(account);
        assert_eq!(errors, Vec::<String>::new());
        applied
    }

    /// One Send/Receive of an IMAP account, and the problems it said.
    fn sync_said(&mut self, account: Id) -> (Value, Vec<String>) {
        let mut request = sync_request(&self.s, account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        let errors = result.errors.clone();
        let applied = apply_sync(&mut self.s, result).unwrap();
        assert!(self.s.mailbox.clone().repair().is_empty(), "{:?}", self.s.mailbox.clone().repair());
        (applied, errors)
    }

    /// One whole run of the Microsoft account: what taking in each piece answered.
    fn run(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, self.ms).unwrap();
        let _routed = self.graph.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    /// Every message the Microsoft server should fetch whole, fetched and taken in.
    fn fetch(&mut self) -> Value {
        let request = graph_fetch_request(&self.s, self.ms, None, 100).unwrap();
        let _routed = self.graph.route();
        let result = mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false));
        apply_graph_fetch(&mut self.s, result).unwrap()
    }

    /// A message in the Microsoft account's Inbox, there and here, fetched whole.
    fn arrived_on_microsoft(&mut self, subject: &str) -> Id {
        {
            let mut state = self.graph.state();
            let inbox = state.well_known("inbox");
            state.mailbox.add(&inbox, subject, NOW - 3_600);
        }
        self.run();
        self.fetch();
        self.titled(subject)
    }

    /// The subjects in a mailbox of the Gmail server (what isn't marked deleted).
    fn on_gmail(&self, raw: &str) -> Vec<String> {
        on(&self.imap, raw)
    }

    /// The flags the Gmail server has of a message in a mailbox, and the time it arrived there.
    fn gmail_has(&self, raw: &str, subject: &str) -> Option<(String, String)> {
        let mut world = self.imap.world();
        let found = world.mailbox(raw).messages.iter().find(|m| self::subject(m).as_deref() == Some(subject));
        found.map(|m| (m.flags.clone(), m.internal_date.clone()))
    }

    /// How many copies of a message the Microsoft server has, and whether each is received mail
    /// (not a draft) with the read state given.
    fn on_microsoft(&self, subject: &str) -> Vec<(bool, bool)> {
        self.graph.state().mailbox.messages.values().filter(|m| m.subject == subject).map(|m| (m.draft, m.read)).collect()
    }

    /// How many messages here have this subject.
    fn here(&self, subject: &str) -> usize {
        self.s.mailbox.messages.values().filter(|m| m.subject == subject).count()
    }

    fn titled(&self, subject: &str) -> Id {
        let found: Vec<Id> = self.s.mailbox.messages.values().filter(|m| m.subject == subject).map(|m| m.id).collect();
        assert_eq!(found.len(), 1, "{subject}: {found:?}");
        found[0]
    }

    fn inbox(&self, account: Id) -> Id {
        self.s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap()
    }

    fn errors(answers: &[Value]) -> Vec<Value> {
        answers.iter().flat_map(|a| a["errors"].as_array().cloned().unwrap_or_default()).collect()
    }
}

#[test]
fn moved_from_gmail_into_microsoft_the_original_goes_only_once_the_copy_is_made() {
    let mut r = Rig::new("into-ms@example.org");
    let (two, three) = (r.titled("Two"), r.titled("Three"));
    let theirs = r.inbox(r.ms);
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    r.s.run("mail.copy", &json!({"ids": [three], "folder": theirs})).unwrap();
    // Gmail first: nothing is made yet, so nothing goes.
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three", "Two"]);
    // Made in the Microsoft account as received mail, read as it was.
    let answers = r.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    assert_eq!((r.on_microsoft("Two"), r.on_microsoft("Three")), (vec![(false, true)], vec![(false, false)]));
    // Then the original of the move goes from Gmail, out of all mail too; the copied one stays.
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three"]);
    assert_eq!(r.on_gmail("[Gmail]/All Mail"), ["Four", "One", "Three"]);
    assert!(r.s.mailbox.pending.is_empty(), "{:?}", r.s.mailbox.pending);
    // Settled: once here and once there, whatever runs after.
    for _ in 0..2 {
        r.sync();
        r.run();
    }
    assert_eq!((r.here("Two"), r.on_microsoft("Two").len()), (1, 1));
    assert_eq!((r.here("Three"), r.on_microsoft("Three").len()), (2, 1));
    assert_eq!(r.s.mailbox.message(two).map(|m| m.account), Some(r.ms));
}

#[test]
fn a_copy_microsoft_wont_take_leaves_the_original_on_gmail() {
    let mut r = Rig::new("refused@example.org");
    let two = r.titled("Two");
    let theirs = r.inbox(r.ms);
    // A server that keeps what is made as a draft: the copy is taken away again, and refused.
    r.graph.state().drafts_only = true;
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    r.run();
    assert!(r.on_microsoft("Two").is_empty());
    let m = r.s.mailbox.message(two).unwrap();
    assert_eq!((m.account, m.folder, m.uid), (r.gmail, r.inbox(r.gmail), 2));
    for _ in 0..2 {
        r.sync();
    }
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three", "Two"]);
    assert_eq!(r.here("Two"), 1);
    assert!(r.s.mailbox.pending.is_empty(), "{:?}", r.s.mailbox.pending);
}

#[test]
fn a_run_cut_off_after_the_copy_never_makes_it_twice_and_the_original_waits_for_it() {
    let mut r = Rig::new("cut-off@example.org");
    let two = r.titled("Two");
    let theirs = r.inbox(r.ms);
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    // Made, and the answer lost: not known here yet, so Gmail keeps the original.
    r.graph.state().lose_next.push("POST /v1.0/me/mailFolders/*/messages".into());
    r.run();
    assert_eq!(r.on_microsoft("Two").len(), 1);
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three", "Two"]);

    // The program stops here: the mailbox file as it was is all that is left.
    let saved = serde_json::to_value(&r.s.mailbox).unwrap();
    r.s = Session::at(serde_json::from_value::<Mailbox>(saved).unwrap(), NOW, 0);
    r.run();
    assert_eq!(r.on_microsoft("Two").len(), 1, "found by its marker, not made twice");
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three"]);
    for _ in 0..2 {
        r.sync();
        r.run();
    }
    assert_eq!((r.here("Two"), r.on_microsoft("Two").len()), (1, 1));
}

#[test]
fn moved_from_gmail_to_this_computer_it_leaves_gmail_once_it_is_saved_here() {
    let mut r = Rig::new("local@example.org");
    let four = r.titled("Four");
    let local = r.s.mailbox.local_account();
    let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
    r.s.run("mail.move", &json!({"ids": [four], "folder": kept})).unwrap();
    // Not saved yet: Gmail keeps it, and it doesn't come down again.
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
    }
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three", "Two"]);
    let dir = std::env::temp_dir().join(format!("mailcraft-451-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kept.mailcraft");
    r.s.run("file.save", &json!({"path": path})).unwrap();
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["One", "Three", "Two"]);
    assert_eq!(r.on_gmail("[Gmail]/All Mail"), ["One", "Three", "Two"]);
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
    }
    // Whole in the file: opened again, it is there with its body.
    let mut opened = Session::at(Mailbox::default(), NOW, 0);
    opened.run("file.open", &json!({"path": path})).unwrap();
    let m = opened.mailbox.messages.values().find(|m| m.subject == "Four").map(|m| m.id).unwrap();
    assert_eq!(opened.mailbox.message(m).map(|m| m.folder), Some(kept));
    assert!(opened.loaded_message(m).unwrap().body.text.contains("The body of Four."));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn moved_from_microsoft_into_gmail_it_leaves_microsoft_once_gmail_has_it() {
    let mut r = Rig::new("out-to-gmail@example.org");
    let invoice = r.arrived_on_microsoft("Invoice 7");
    r.s.run("mail.flag", &json!({"ids": [invoice]})).unwrap();
    r.run();
    let inbox = r.inbox(r.gmail);
    r.s.run("mail.move", &json!({"ids": [invoice], "folder": inbox})).unwrap();
    // Microsoft first: nothing is on Gmail yet, so nothing goes, saved here or not.
    let dir = std::env::temp_dir().join(format!("mailcraft-451-to-gmail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    r.s.run("file.save", &json!({"path": dir.join("moved.mailcraft")})).unwrap();
    r.run();
    assert_eq!(r.on_microsoft("Invoice 7").len(), 1);
    // Gmail takes it whole, flagged and unread as it is, at the time it came; it isn't brought
    // down again.
    let mut request = sync_request(&r.s, r.gmail).unwrap();
    request.password = PASSWORD.into();
    let result = mailcraft_net::sync(&request);
    assert!(result.errors.is_empty() && result.folders.iter().all(|f| f.new.is_empty()), "{:?} {:?}", result.errors, result.folders);
    apply_sync(&mut r.s, result).unwrap();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "Invoice 7", "One", "Three", "Two"]);
    let (flags, arrived) = r.gmail_has("INBOX", "Invoice 7").unwrap();
    assert_eq!((flags.as_str(), arrived.as_str()), ("\\Flagged", "15-Jan-2027 07:00:00 +0000"));
    let m = r.s.mailbox.message(invoice).unwrap();
    assert!(m.account == r.gmail && m.uid != 0 && m.server.is_none(), "{m:?}");
    // Then it leaves Microsoft.
    r.run();
    assert!(r.on_microsoft("Invoice 7").is_empty());
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
        r.run();
    }
    assert_eq!(r.here("Invoice 7"), 1);
    assert_eq!(r.on_gmail("INBOX").iter().filter(|s| *s == "Invoice 7").count(), 1);
    assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_server_that_doesnt_say_where_the_message_went_has_it_known_when_it_comes_down() {
    let mut world = gmail();
    world.uidplus = false;
    let mut r = Rig::with("no-uidplus@example.org", world);
    let receipt = r.arrived_on_microsoft("Receipt");
    let inbox = r.inbox(r.gmail);
    r.s.run("mail.move", &json!({"ids": [receipt], "folder": inbox})).unwrap();
    // Put there, and come down in the same run as the message here: not a second one.
    assert_eq!(r.sync()["new"], 0);
    assert_eq!(r.here("Receipt"), 1);
    assert!(r.s.mailbox.message(receipt).is_some_and(|m| m.uid != 0), "known by its copy");
    r.run();
    assert!(r.on_microsoft("Receipt").is_empty());
    assert_eq!(r.on_gmail("INBOX").iter().filter(|s| *s == "Receipt").count(), 1);
}

#[test]
fn a_message_gmail_wont_take_goes_back_to_microsoft_and_stays_there() {
    let mut r = Rig::new("gmail-refuses@example.org");
    let receipt = r.arrived_on_microsoft("Receipt");
    let inbox = r.inbox(r.gmail);
    r.s.run("mail.move", &json!({"ids": [receipt], "folder": inbox})).unwrap();
    r.imap.world().refuse.push("APPEND".into());
    let (_, errors) = r.sync_said(r.gmail);
    assert!(errors.iter().any(|e| e.contains("Couldn't put \"Receipt\" in \"INBOX\"")), "{errors:?}");
    // Back where its original is, as that server message, and nothing went there.
    let m = r.s.mailbox.message(receipt).unwrap();
    assert!(m.account == r.ms && m.folder == r.inbox(r.ms) && m.server.is_some() && m.moving_out.is_none(), "{m:?}");
    r.imap.world().refuse.clear();
    for _ in 0..2 {
        r.sync();
        r.run();
    }
    assert_eq!(r.on_microsoft("Receipt").len(), 1);
    assert!(!r.on_gmail("INBOX").contains(&"Receipt".to_string()));
    assert_eq!(r.here("Receipt"), 1);
}

#[test]
fn mail_on_this_computer_moved_and_copied_into_gmail_is_put_there() {
    let mut r = Rig::new("local-to-gmail@example.org");
    let local = r.s.mailbox.local_account();
    let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
    let add = |r: &mut Rig, subject: &str| {
        let m = mailcraft_model::Message {
            folder: kept,
            subject: subject.into(),
            message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
            from: mailcraft_model::Address::new("Bob", "bob@example.org"),
            read: true,
            date: NOW - 7_200,
            received: NOW - 7_000,
            body: mailcraft_model::Body { text: format!("All of {subject}."), html: None },
            ..Default::default()
        };
        r.s.mailbox.add_message(m).unwrap()
    };
    let (moved, copied) = (add(&mut r, "Moved in"), add(&mut r, "Copied in"));
    // Into a folder Gmail doesn't have yet: it is made there.
    let inbox = r.inbox(r.gmail);
    let projects = r.s.run("folder.new", &json!({"parent": inbox, "name": "Projects"})).unwrap()["id"].as_u64().unwrap();
    r.s.run("mail.move", &json!({"ids": [moved], "folder": projects})).unwrap();
    r.s.run("mail.copy", &json!({"ids": [copied], "folder": projects})).unwrap();
    r.sync();
    assert_eq!(r.on_gmail("INBOX/Projects"), ["Copied in", "Moved in"]);
    assert_eq!(r.gmail_has("INBOX/Projects", "Moved in").map(|(flags, _)| flags), Some("\\Seen".to_string()));
    // The copy's source stays where it is; nothing is here twice.
    assert_eq!(r.s.mailbox.message(copied).map(|m| m.folder), Some(kept));
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
    }
    assert_eq!((r.here("Moved in"), r.here("Copied in")), (1, 2));
}

#[test]
fn moved_from_gmail_into_another_imap_account_it_leaves_gmail_once_the_other_has_it() {
    let mut r = Rig::new("imap-to-imap@example.org");
    let mut world = gmail();
    world.gmail = false;
    world.user = "ada@work.example".into();
    world.mailboxes = vec![mailbox("INBOX", "\\HasNoChildren", 77, Vec::new()), mailbox("Trash", "\\HasNoChildren \\Trash", 78, Vec::new())];
    let other = Fake::imap(world);
    let work = r.s.run("account.add", &servers("ada@work.example", &other)).unwrap()["id"].as_u64().unwrap();
    r.sync_of(work);
    let two = r.titled("Two");
    let theirs = r.inbox(work);
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    // Gmail first: the other server doesn't have it yet, so it stays, also once the mailbox is
    // opened again.
    r.sync();
    assert!(r.on_gmail("INBOX").contains(&"Two".to_string()));
    let saved = serde_json::to_value(&r.s.mailbox).unwrap();
    r.s = Session::at(serde_json::from_value::<Mailbox>(saved).unwrap(), NOW, 0);
    assert_eq!(r.s.mailbox.repair(), Vec::<String>::new());
    r.sync_of(work);
    assert_eq!(on(&other, "INBOX"), ["Two"]);
    r.sync();
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three"]);
    assert_eq!(r.on_gmail("[Gmail]/All Mail"), ["Four", "One", "Three"]);
    // Settled, and once each.
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
        assert_eq!(r.sync_of(work)["new"], 0);
    }
    assert_eq!(r.here("Two"), 1);
    assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
    // Moved on in the other account and back out of it: an ordinary move there.
    let m = r.s.mailbox.message(two).unwrap();
    assert!(m.account == work && m.uid != 0, "{m:?}");
}
