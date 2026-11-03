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
use mailcraft_model::sync::SyncResult;
use mailcraft_model::{Address, Body, FolderKind, Id, Mailbox, Message};
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

/// A message with a file attached, `report.txt`, whose bytes are [`FIGURES`].
fn with_attachment(uid: u32, subject: &str) -> FakeMessage {
    let mut m = message(uid, "\\Seen", subject);
    let head = String::from_utf8_lossy(&m.body).split("Content-Type:").next().unwrap_or_default().to_string();
    let body = format!(
        "{head}Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n\
         --b1\r\nContent-Type: text/plain; name=\"report.txt\"\r\nContent-Disposition: attachment; filename=\"report.txt\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{FIGURES_BASE64}\r\n--b1--\r\n"
    );
    m.body = body.into_bytes();
    m
}

/// What `report.txt` of [`with_attachment`] holds.
const FIGURES: &[u8] = b"Figures: 1, 2, 3.";
/// The same in base64, as a message carries it.
const FIGURES_BASE64: &str = "RmlndXJlczogMSwgMiwgMy4=";

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

    /// What one Send/Receive of an IMAP account brings back, not taken in yet.
    fn sync_result(&self, account: Id) -> SyncResult {
        let mut request = sync_request(&self.s, account).unwrap();
        request.password = PASSWORD.into();
        mailcraft_net::sync(&request)
    }

    /// The program closed and opened again: the mailbox file as it was saved is all there is.
    fn reopened(&mut self, saved: &Value) {
        self.s = Session::at(serde_json::from_value::<Mailbox>(saved.clone()).unwrap(), NOW, 0);
    }

    /// A second IMAP account, on a server that isn't Gmail's, with an Inbox, an Archive and a
    /// Trash, read once. `uidplus`: the server says where what is put there went.
    fn other_account(&mut self, uidplus: bool) -> (FakeImap, Id) {
        let mut world = gmail();
        world.gmail = false;
        world.uidplus = uidplus;
        world.user = "ada@work.example".into();
        world.mailboxes = vec![
            mailbox("INBOX", "\\HasNoChildren", 77, Vec::new()),
            mailbox("Archive", "\\HasNoChildren \\Archive", 79, Vec::new()),
            mailbox("Trash", "\\HasNoChildren \\Trash", 78, Vec::new()),
        ];
        let other = Fake::imap(world);
        let work = self.s.run("account.add", &servers("ada@work.example", &other)).unwrap()["id"].as_u64().unwrap();
        self.sync_of(work);
        (other, work)
    }

    /// Whether Gmail still has a message in its Inbox.
    fn gmail_keeps(&self, subject: &str) -> bool {
        self.on_gmail("INBOX").iter().any(|s| s == subject)
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
fn a_rule_that_files_new_gmail_mail_into_another_account_puts_it_there_before_gmail_lets_it_go() {
    for (into_microsoft, uidplus) in [(false, true), (false, false), (true, true)] {
        let mut r = Rig::new(&format!("rule-{into_microsoft}-{uidplus}@example.org"));
        let (other, work) = r.other_account(uidplus);
        let to = if into_microsoft { r.ms } else { work };
        let theirs = r.inbox(to);
        let rule =
            json!({"name": "Rota", "conditions": [{"type": "subjectContains", "value": "rota"}], "actions": [{"type": "moveTo", "value": theirs}]});
        r.s.run("rules.add", &rule).unwrap();
        // New mail comes to Gmail, and the rule files it into the other account at once.
        for raw in ["INBOX", "[Gmail]/All Mail"] {
            r.imap.world().mailbox(raw).messages.push(message(5, "", "Rota"));
        }
        assert_eq!(r.sync()["new"], 1, "{into_microsoft} {uidplus}");
        let rota = r.titled("Rota");
        assert_eq!(r.s.mailbox.message(rota).map(|m| (m.account, m.folder)), Some((to, theirs)));
        // Gmail keeps it until the other server has it.
        for _ in 0..2 {
            assert_eq!(r.sync()["new"], 0);
        }
        assert!(r.gmail_keeps("Rota"));
        // The other account's own run puts it there, whole, unread as it came; then Gmail's lets
        // it go, out of all mail too.
        if into_microsoft {
            let answers = r.run();
            assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
            assert_eq!(r.on_microsoft("Rota"), [(false, false)]);
        } else {
            r.sync_of(work);
            assert_eq!(on(&other, "INBOX"), ["Rota"], "{uidplus}");
            let copy = String::from_utf8_lossy(&other.world().mailbox("INBOX").messages[0].body).into_owned();
            assert!(copy.contains("The body of Rota."), "{copy}");
        }
        assert!(r.gmail_keeps("Rota"));
        r.sync();
        assert!(!r.gmail_keeps("Rota"));
        assert!(!r.on_gmail("[Gmail]/All Mail").iter().any(|s| s == "Rota"));
        // Settled: once here and once there, whatever runs after.
        for _ in 0..2 {
            r.sync_of(work);
            r.run();
            r.sync();
        }
        let there = if into_microsoft { r.on_microsoft("Rota").len() } else { on(&other, "INBOX").len() };
        assert_eq!((r.here("Rota"), there), (1, 1), "{into_microsoft} {uidplus}");
        assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
    }
}

#[test]
fn a_message_microsoft_wont_take_stays_on_gmail() {
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
    let again = sync_request(&r.s, r.gmail).unwrap().ops;
    assert!(again.is_empty(), "its flags are on the server as they are here: {again:?}");
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
fn a_copy_gmail_wont_take_is_taken_away_and_the_original_stays_on_microsoft() {
    let mut r = Rig::new("gmail-refuses-a-copy@example.org");
    let receipt = r.arrived_on_microsoft("Receipt");
    let inbox = r.inbox(r.gmail);
    r.s.run("mail.copy", &json!({"ids": [receipt], "folder": inbox})).unwrap();
    assert_eq!(r.here("Receipt"), 2);
    r.imap.world().refuse.push("APPEND".into());
    let (_, errors) = r.sync_said(r.gmail);
    assert!(errors.iter().any(|e| e.contains("Couldn't put \"Receipt\" in \"INBOX\"")), "{errors:?}");
    // The copy is taken away; the original is as it was.
    assert_eq!(r.here("Receipt"), 1);
    let m = r.s.mailbox.message(receipt).unwrap();
    assert!(m.account == r.ms && m.folder == r.inbox(r.ms) && m.server.is_some() && m.moving_out.is_none(), "{m:?}");
    r.imap.world().refuse.clear();
    for _ in 0..2 {
        r.sync();
        r.run();
    }
    assert_eq!(r.on_microsoft("Receipt").len(), 1);
    assert!(!r.gmail_keeps("Receipt"));
    assert_eq!(r.here("Receipt"), 1);
    assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
}

#[test]
fn moved_back_into_microsoft_after_gmail_took_it_it_is_on_microsoft_once_and_gone_from_gmail() {
    let mut r = Rig::new("round-trip@example.org");
    let invoice = r.arrived_on_microsoft("Invoice 9");
    let (inbox, theirs) = (r.inbox(r.gmail), r.inbox(r.ms));
    r.s.run("mail.move", &json!({"ids": [invoice], "folder": inbox})).unwrap();
    // Gmail takes it; Microsoft's original is still there, its deletion not yet on its way.
    r.sync();
    assert!(r.gmail_keeps("Invoice 9"));
    assert_eq!(r.on_microsoft("Invoice 9").len(), 1);
    // Back into Microsoft: the message Microsoft has, and Gmail's copy goes.
    r.s.run("mail.move", &json!({"ids": [invoice], "folder": theirs})).unwrap();
    let m = r.s.mailbox.message(invoice).unwrap();
    assert!(m.account == r.ms && m.folder == theirs && m.server.is_some(), "{m:?}");
    for _ in 0..2 {
        r.sync();
        assert_eq!(Rig::errors(&r.run()), Vec::<Value>::new());
    }
    assert_eq!(r.on_microsoft("Invoice 9").len(), 1);
    assert!(!r.gmail_keeps("Invoice 9"));
    assert_eq!(r.on_gmail("[Gmail]/All Mail").iter().filter(|s| *s == "Invoice 9").count(), 0);
    assert_eq!(r.here("Invoice 9"), 1);
    assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
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
    let mut request = sync_request(&r.s, r.gmail).unwrap();
    request.password = PASSWORD.into();
    let result = mailcraft_net::sync(&request);
    assert_eq!(r.on_gmail("INBOX/Projects"), ["Copied in", "Moved in"]);
    assert_eq!(r.gmail_has("INBOX/Projects", "Moved in").map(|(flags, _)| flags), Some("\\Seen".to_string()));
    // Marked unread here while it went: that goes up next, and stays.
    r.s.run("mail.markUnread", &json!({"ids": [moved]})).unwrap();
    apply_sync(&mut r.s, result).unwrap();
    // The copy's source stays where it is; nothing is here twice.
    assert_eq!(r.s.mailbox.message(copied).map(|m| m.folder), Some(kept));
    for _ in 0..2 {
        assert_eq!(r.sync()["new"], 0);
    }
    assert_eq!((r.here("Moved in"), r.here("Copied in")), (1, 2));
    assert_eq!(r.gmail_has("INBOX/Projects", "Moved in").map(|(flags, _)| flags), Some(String::new()));
    assert!(r.s.mailbox.message(moved).is_some_and(|m| !m.read));
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
    // The other server's message now: moved on there, it is an ordinary move on that server.
    let m = r.s.mailbox.message(two).unwrap();
    assert!(m.account == work && m.uid != 0, "{m:?}");
    let trash = r.s.mailbox.folder_of_kind(work, FolderKind::Deleted).unwrap();
    r.s.run("mail.move", &json!({"ids": [two], "folder": trash})).unwrap();
    assert_eq!(r.sync_of(work)["new"], 0);
    assert_eq!((on(&other, "INBOX"), on(&other, "Trash")), (Vec::<String>::new(), vec!["Two".to_string()]));
    assert_eq!(r.on_gmail("INBOX"), ["Four", "One", "Three"]);
    assert_eq!(r.here("Two"), 1);
}

#[test]
fn called_off_while_its_copy_goes_to_a_server_that_doesnt_say_where_it_went_that_copy_goes_again() {
    for how in ["undo", "back", "delete"] {
        let mut r = Rig::new(&format!("called-off-{how}@example.org"));
        let (other, work) = r.other_account(false);
        let two = r.titled("Two");
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        // The copy is on its way when the owner thinks again.
        let result = r.sync_result(work);
        assert_eq!(on(&other, "INBOX"), ["Two"], "{how}");
        match how {
            "undo" => {
                r.s.run("edit.undo", &json!({})).unwrap();
            }
            "back" => {
                let inbox = r.inbox(r.gmail);
                r.s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
            }
            _ => {
                r.s.run("mail.deletePermanently", &json!({"ids": [two]})).unwrap();
            }
        }
        apply_sync(&mut r.s, result).unwrap();
        // The copy goes again, and nothing of it comes down as new mail.
        for _ in 0..2 {
            assert_eq!(r.sync_of(work)["new"], 0, "{how}");
            r.sync();
        }
        assert_eq!(on(&other, "INBOX"), Vec::<String>::new(), "{how}");
        assert_eq!(r.here("Two"), usize::from(how != "delete"), "{how}");
        if how != "delete" {
            assert!(r.gmail_keeps("Two"), "{how}");
            let m = r.s.mailbox.message(two).unwrap();
            assert!(m.account == r.gmail && m.uid == 2, "{how}: {m:?}");
            // Moved there again: put there anew, not taken for the copy that went (which a server
            // without UIDPLUS keeps, marked as deleted).
            r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
            for _ in 0..2 {
                r.sync_of(work);
                r.sync();
            }
            assert_eq!((on(&other, "INBOX"), r.here("Two")), (vec!["Two".to_string()], 1), "{how}");
            assert!(!r.gmail_keeps("Two"), "{how}");
        }
    }
}

#[test]
fn moved_on_while_its_copy_goes_to_a_server_that_doesnt_say_where_it_went_it_is_there_once() {
    let mut r = Rig::new("moved-on@example.org");
    let (other, work) = r.other_account(false);
    let two = r.titled("Two");
    let (inbox, archive) = (r.inbox(work), r.s.mailbox.folder_of_kind(work, FolderKind::Archive).unwrap());
    r.s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
    let result = r.sync_result(work);
    // Filed in the other account while its copy went to the Inbox there.
    r.s.run("mail.move", &json!({"ids": [two], "folder": archive})).unwrap();
    apply_sync(&mut r.s, result).unwrap();
    for _ in 0..2 {
        assert_eq!(r.sync_of(work)["new"], 0);
        r.sync();
    }
    assert_eq!((on(&other, "INBOX"), on(&other, "Archive")), (Vec::new(), vec!["Two".to_string()]));
    assert_eq!(r.here("Two"), 1);
    assert!(!r.gmail_keeps("Two"));
    let m = r.s.mailbox.message(two).unwrap();
    assert!(m.folder == archive && m.uid != 0, "{m:?}");
    assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
}

#[test]
fn a_copy_whose_answer_was_lost_with_the_connection_is_found_there_and_not_put_there_twice() {
    for uidplus in [false, true] {
        let mut r = Rig::new(&format!("lost-{uidplus}@example.org"));
        let (other, work) = r.other_account(uidplus);
        let two = r.titled("Two");
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        other.world().hang_up_after = Some("APPEND".into());
        let (_, errors) = r.sync_said(work);
        assert!(!errors.is_empty(), "{uidplus}");
        assert_eq!(on(&other, "INBOX"), ["Two"], "{uidplus}");
        // Marked unread here meanwhile: once found there, the copy is told so too.
        r.s.run("mail.markUnread", &json!({"ids": [two]})).unwrap();
        // Nothing known of it here yet: Gmail keeps the original.
        r.sync();
        assert!(r.gmail_keeps("Two"), "{uidplus}");
        for _ in 0..2 {
            assert_eq!(r.sync_of(work)["new"], 0, "{uidplus}");
            r.sync();
        }
        assert_eq!(on(&other, "INBOX"), ["Two"], "{uidplus}");
        assert_eq!(r.here("Two"), 1, "{uidplus}");
        assert!(!r.gmail_keeps("Two"), "{uidplus}");
        let read_there = other.world().mailbox("INBOX").messages.iter().find(|m| subject(m).as_deref() == Some("Two")).map(|m| m.has("\\Seen"));
        assert_eq!(read_there, Some(false), "{uidplus}");
        assert!(r.s.mailbox.message(two).is_some_and(|m| !m.read), "{uidplus}");
        assert!(r.s.mailbox.pending.is_empty() && r.s.mailbox.queued.is_empty(), "{uidplus}: {:?} {:?}", r.s.mailbox.pending, r.s.mailbox.queued);
    }
}

#[test]
fn a_look_for_its_copy_that_the_server_cuts_off_is_said_and_the_next_run_puts_it_there_once() {
    for (n, cut) in ["HEADER Message-ID", "INTERNALDATE BODY.PEEK[HEADER]"].into_iter().enumerate() {
        let mut r = Rig::new(&format!("cut-{n}@example.org"));
        let (other, work) = r.other_account(true);
        let two = r.titled("Two");
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        if n == 1 {
            // A copy is there already, its answer lost: it is looked at before it goes there again.
            other.world().hang_up_after = Some("APPEND".into());
            r.sync_said(work);
        }
        other.world().answers = vec![(cut.into(), "* BYE going away\r\n".into())];
        let (_, errors) = r.sync_said(work);
        assert!(errors.iter().any(|e| e.starts_with("Couldn't look for \"Two\" in \"INBOX\" on the server")), "{cut}: {errors:?}");
        other.world().answers.clear();
        for _ in 0..2 {
            r.sync_of(work);
            r.sync();
        }
        assert_eq!((on(&other, "INBOX"), r.here("Two")), (vec!["Two".to_string()], 1), "{cut}");
        assert!(!r.gmail_keeps("Two"), "{cut}");
    }
}

#[test]
fn a_copy_made_by_a_run_whose_answer_was_never_kept_is_found_there_and_not_put_there_twice() {
    for uidplus in [false, true] {
        let mut r = Rig::new(&format!("never-kept-{uidplus}@example.org"));
        let (other, work) = r.other_account(uidplus);
        let two = r.titled("Two");
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        let saved = serde_json::to_value(&r.s.mailbox).unwrap();
        // The program closes before what the run brought back is kept.
        let _lost = r.sync_result(work);
        assert_eq!(on(&other, "INBOX"), ["Two"], "{uidplus}");
        r.reopened(&saved);
        for _ in 0..2 {
            assert_eq!(r.sync_of(work)["new"], 0, "{uidplus}");
            r.sync();
        }
        assert_eq!(on(&other, "INBOX"), ["Two"], "{uidplus}");
        assert_eq!(r.here("Two"), 1, "{uidplus}");
        assert!(!r.gmail_keeps("Two"), "{uidplus}");
    }
}

#[test]
fn mail_that_merely_carries_the_message_id_of_one_put_on_the_server_is_not_taken_for_its_copy() {
    let mut r = Rig::new("look-alike@example.org");
    let (other, work) = r.other_account(false);
    let two = r.titled("Two");
    // Another message with the same Message-ID came to the other account and isn't here yet.
    let mut look_alike = message(5, "", "Not two");
    look_alike.body = String::from_utf8_lossy(&look_alike.body).replace("<not two@example.org>", "<two@example.org>").into_bytes();
    other.world().mailbox("INBOX").messages.push(look_alike);
    let theirs = r.inbox(work);
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    r.sync_of(work);
    assert_eq!(on(&other, "INBOX"), ["Not two", "Two"]);
    for _ in 0..2 {
        r.sync_of(work);
        r.sync();
    }
    assert_eq!((r.here("Two"), r.here("Not two")), (1, 1));
    assert!(!r.gmail_keeps("Two"));
}

#[test]
fn a_copy_put_on_an_imap_server_doesnt_say_who_got_a_blind_copy() {
    let mut r = Rig::new("blind-copy@example.org");
    let (other, work) = r.other_account(true);
    let local = r.s.mailbox.local_account();
    let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
    let rota =
        r.s.mailbox
            .add_message(Message {
                folder: kept,
                subject: "Rota".into(),
                message_id: "rota@example.org".into(),
                from: Address::new("Ada", "ada@example.org"),
                to: vec![Address::new("Bob", "bob@example.org")],
                bcc: vec![Address::new("Eve", "eve@example.net")],
                // Resent, with a blind copy to somebody else.
                headers: vec![("Resent-To".into(), "carol@example.org".into()), ("Resent-Bcc".into(), "mallory@example.net".into())],
                read: true,
                date: NOW - 7_200,
                body: Body { text: "The rota.".into(), html: None },
                ..Default::default()
            })
            .unwrap();
    let theirs = r.inbox(work);
    r.s.run("mail.move", &json!({"ids": [rota], "folder": theirs})).unwrap();
    r.sync_of(work);
    let copy = String::from_utf8_lossy(&other.world().mailbox("INBOX").messages[0].body).into_owned();
    assert!(copy.contains("Subject: Rota") && copy.contains("bob@example.org") && !copy.contains("eve@example.net"), "{copy}");
    assert!(copy.contains("carol@example.org") && !copy.contains("mallory@example.net"), "{copy}");
    // Here it still says who got one.
    assert_eq!(r.s.mailbox.message(rota).map(|m| m.bcc.len()), Some(1));
}

#[test]
fn a_message_whose_message_id_is_too_long_to_look_for_is_put_there_and_new_mail_still_comes() {
    for uidplus in [true, false] {
        let mut world = gmail();
        world.uidplus = uidplus;
        let mut r = Rig::with("long-id@example.org", world);
        let local = r.s.mailbox.local_account();
        let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
        let long = Message {
            folder: kept,
            subject: "Long".into(),
            message_id: format!("{}@example.org", "x".repeat(1_200)),
            from: Address::new("Ada", "ada@example.org"),
            read: true,
            date: NOW - 7_200,
            body: Body { text: "Long.".into(), html: None },
            ..Default::default()
        };
        let long = r.s.mailbox.add_message(long).unwrap();
        let inbox = r.inbox(r.gmail);
        r.s.run("mail.move", &json!({"ids": [long], "folder": inbox})).unwrap();
        {
            let mut world = r.imap.world();
            let fresh = message(9, "", "Fresh");
            world.mailbox("INBOX").messages.push(fresh.clone());
            world.mailbox("[Gmail]/All Mail").messages.push(fresh);
        }
        // Not looked for there, as the server can't be asked for it: put there, and the run
        // goes on to read new mail.
        assert_eq!(r.sync()["new"], 1, "UIDPLUS {uidplus}");
        assert!(r.gmail_keeps("Long") && r.here("Fresh") == 1, "UIDPLUS {uidplus}");
        assert_eq!(r.on_gmail("INBOX").iter().filter(|s| *s == "Long").count(), 1, "UIDPLUS {uidplus}");
        assert_eq!(r.sync()["new"], 0, "UIDPLUS {uidplus}");
        assert_eq!(r.here("Long"), 1, "UIDPLUS {uidplus}");
    }
}

#[test]
fn a_message_copied_more_than_once_into_a_folder_is_put_there_as_often() {
    let mut r = Rig::new("copied-again@example.org");
    let (other, work) = r.other_account(false);
    let three = r.titled("Three");
    let theirs = r.inbox(work);
    // Twice before a run, and once more after it: each is a copy of its own there.
    for _ in 0..2 {
        r.s.run("mail.copy", &json!({"ids": [three], "folder": theirs})).unwrap();
    }
    r.sync_of(work);
    assert_eq!(on(&other, "INBOX"), ["Three", "Three"]);
    r.s.run("mail.copy", &json!({"ids": [three], "folder": theirs})).unwrap();
    for _ in 0..2 {
        assert_eq!(r.sync_of(work)["new"], 0);
    }
    assert_eq!(on(&other, "INBOX"), ["Three", "Three", "Three"]);
    let mut uids: Vec<u32> = r.s.mailbox.messages.values().filter(|m| m.subject == "Three" && m.account == work).map(|m| m.uid).collect();
    uids.sort();
    uids.dedup();
    assert!(uids.len() == 3 && !uids.contains(&0), "{uids:?}");
    assert!(r.gmail_keeps("Three"));
}

#[test]
fn a_message_put_on_another_server_arrives_whole_with_its_attachment() {
    let mut r = Rig::new("whole@example.org");
    let (other, work) = r.other_account(true);
    {
        let mut world = r.imap.world();
        let report = with_attachment(5, "Report");
        world.mailbox("INBOX").messages.push(report.clone());
        world.mailbox("[Gmail]/All Mail").messages.push(report);
    }
    assert_eq!(r.sync()["new"], 1);
    let report = r.titled("Report");
    assert_eq!(r.s.mailbox.message(report).map(|m| m.attachments.len()), Some(1));
    // Copied into Microsoft, and moved into the other IMAP account.
    let (theirs, work_inbox) = (r.inbox(r.ms), r.inbox(work));
    r.s.run("mail.copy", &json!({"ids": [report], "folder": theirs})).unwrap();
    r.s.run("mail.move", &json!({"ids": [report], "folder": work_inbox})).unwrap();
    assert_eq!(Rig::errors(&r.run()), Vec::<Value>::new());
    r.sync_of(work);
    // On the other IMAP server: its text and its file, byte for byte.
    let copy = String::from_utf8_lossy(&other.world().mailbox("INBOX").messages[0].body).into_owned();
    assert!(copy.contains("The body of Report.") && copy.contains("report.txt"), "{copy}");
    assert!(copy.contains(FIGURES_BASE64), "{copy}");
    // On Microsoft's: the same.
    let state = r.graph.state();
    let made: Vec<_> = state.mailbox.messages.values().filter(|m| m.subject == "Report").collect();
    assert_eq!(made.len(), 1);
    assert!(made[0].body.contains("The body of Report."), "{}", made[0].body);
    assert_eq!(made[0].attachments.iter().map(|a| (a.name.as_str(), a.bytes.as_slice())).collect::<Vec<_>>(), [("report.txt", FIGURES)]);
}

#[test]
fn undo_of_a_move_leaves_the_destinations_own_earlier_copy_alone() {
    for uidplus in [true, false] {
        let mut r = Rig::new(&format!("undo-{uidplus}@example.org"));
        let (other, work) = r.other_account(uidplus);
        other.world().mailbox("INBOX").messages.push(message(50, "\\Seen", "Two"));
        let two = r.titled("Two");
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
        r.sync_of(work);
        let inbox = r.inbox(r.gmail);
        r.s.run("mail.move", &json!({"ids": [two], "folder": inbox})).unwrap();
        for _ in 0..2 {
            r.sync_of(work);
            r.sync();
        }
        assert!(r.gmail_keeps("Two"), "{uidplus}");
        assert_eq!(on(&other, "INBOX"), ["Two"], "{uidplus}");
    }
}

/// The marker a message's copy on the server carries, when it carries one.
fn marker_of(m: &FakeMessage) -> Option<String> {
    let body = String::from_utf8_lossy(&m.body).into_owned();
    let line = body.lines().find(|l| l.starts_with("X-MailCraft-Place: "))?;
    Some(line.trim_start_matches("X-MailCraft-Place: ").to_string())
}

#[test]
fn copies_put_on_a_server_that_doesnt_say_where_they_went_are_each_their_own_and_never_the_servers() {
    let mut r = Rig::new("each-its-own@example.org");
    let (other, work) = r.other_account(false);
    // The same mail came to the other account by itself, and isn't here yet.
    other.world().mailbox("INBOX").messages.push(message(50, "\\Seen", "Three"));
    let three = r.titled("Three");
    let theirs = r.inbox(work);
    for _ in 0..2 {
        r.s.run("mail.copy", &json!({"ids": [three], "folder": theirs})).unwrap();
    }
    let copies: Vec<Id> = r.s.mailbox.messages.values().filter(|m| m.subject == "Three" && m.account == work).map(|m| m.id).collect();
    assert_eq!(copies.len(), 2);
    r.sync_of(work);
    assert_eq!(on(&other, "INBOX"), ["Three", "Three", "Three"]);
    {
        // Each copy carries a marker of its own; the server's own message none.
        let mut world = other.world();
        let markers: Vec<Option<String>> = world.mailbox("INBOX").messages.iter().map(marker_of).collect();
        assert!(markers[0].is_none() && markers[1].is_some() && markers[2].is_some() && markers[1] != markers[2], "{markers:?}");
    }
    let uid = |r: &Rig, id: Id| r.s.mailbox.message(id).map(|m| m.uid);
    let (first, second) = (uid(&r, copies[0]), uid(&r, copies[1]));
    assert!(first.is_some_and(|u| u > 50) && second.is_some_and(|u| u > 50) && first != second, "{first:?} {second:?}");
    // Deleting one copy here takes that one alone off the server.
    r.s.run("mail.deletePermanently", &json!({"ids": [copies[0]]})).unwrap();
    for _ in 0..2 {
        r.sync_of(work);
    }
    let left: Vec<u32> = other.world().mailbox("INBOX").messages.iter().filter(|m| !m.has("\\Deleted")).map(|m| m.uid).collect();
    assert_eq!(left, [50, second.unwrap_or_default()]);
    assert!(r.gmail_keeps("Three"));
}

#[test]
fn a_copy_without_a_message_id_whose_answer_was_lost_is_found_by_its_marker_and_not_put_there_twice() {
    for uidplus in [false, true] {
        let mut r = Rig::new(&format!("no-id-{uidplus}@example.org"));
        let (other, work) = r.other_account(uidplus);
        let local = r.s.mailbox.local_account();
        let kept = r.s.mailbox.add_folder(local, None, "Kept").unwrap();
        let plain = Message {
            folder: kept,
            subject: "No id".into(),
            from: Address::new("Ada", "ada@example.org"),
            read: true,
            date: NOW - 7_200,
            body: Body { text: "No id.".into(), html: None },
            ..Default::default()
        };
        let plain = r.s.mailbox.add_message(plain).unwrap();
        let theirs = r.inbox(work);
        r.s.run("mail.move", &json!({"ids": [plain], "folder": theirs})).unwrap();
        other.world().hang_up_after = Some("APPEND".into());
        let (_, errors) = r.sync_said(work);
        assert!(!errors.is_empty(), "{uidplus}");
        assert_eq!(on(&other, "INBOX"), ["No id"], "{uidplus}");
        for _ in 0..2 {
            assert_eq!(r.sync_of(work)["new"], 0, "{uidplus}");
        }
        assert_eq!(on(&other, "INBOX"), ["No id"], "{uidplus}");
        assert_eq!(r.here("No id"), 1, "{uidplus}");
        let m = r.s.mailbox.message(plain).unwrap();
        assert!(m.account == work && m.uid != 0, "{uidplus}: {m:?}");
        assert!(r.s.mailbox.queued.is_empty(), "{uidplus}: {:?}", r.s.mailbox.queued);
    }
}

#[test]
fn a_copy_whose_answer_was_lost_is_found_among_much_of_the_same_mail_there() {
    let mut r = Rig::new("among-many@example.org");
    let (other, work) = r.other_account(false);
    // The same mail came to the other account four times by itself, and none is here yet.
    for uid in 50..54 {
        other.world().mailbox("INBOX").messages.push(message(uid, "\\Seen", "Two"));
    }
    let two = r.titled("Two");
    let theirs = r.inbox(work);
    r.s.run("mail.move", &json!({"ids": [two], "folder": theirs})).unwrap();
    other.world().hang_up_after = Some("APPEND".into());
    let (_, errors) = r.sync_said(work);
    assert!(!errors.is_empty());
    for _ in 0..2 {
        r.sync_of(work);
        r.sync();
    }
    assert_eq!(on(&other, "INBOX").len(), 5);
    assert!(r.s.mailbox.message(two).is_some_and(|m| m.uid == 54), "{:?}", r.s.mailbox.message(two));
    assert!(!r.gmail_keeps("Two"));
}
