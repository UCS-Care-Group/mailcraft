//! Folders and the mail server from end to end: the engine's `sync_request` and `apply_sync`
//! with `mailcraft_net::sync` in between, against the in-process fake IMAP server. A folder the
//! server no longer has goes here too; folders made, renamed, moved and deleted here are made,
//! renamed, moved and deleted there, and come back as one folder each.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request, unpushed};
use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::{Value, json};

const PASSWORD: &str = "hunter2";

fn message(uid: u32, subject: &str) -> FakeMessage {
    let body = format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n",
        subject.to_lowercase().replace(' ', "-")
    );
    FakeMessage { uid, flags: String::new(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: body.into_bytes(), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// A mailbox as Exchange lists it over IMAP: Deleted Items, and folders of the owner's.
fn world() -> ImapWorld {
    ImapWorld {
        user: "ada@example.org".into(),
        password: PASSWORD.into(),
        can_move: true,
        uidplus: true,
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "One"), message(2, "Two")]),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("Deleted Items", "\\HasNoChildren \\Trash", 41, Vec::new()),
            mailbox("Projects", "\\HasChildren", 31, vec![message(1, "Project one")]),
            mailbox("Projects/Apollo", "\\HasNoChildren", 32, vec![message(1, "Launch")]),
            mailbox("Old", "\\HasNoChildren", 51, vec![message(1, "Old news")]),
            mailbox("Keep", "\\HasNoChildren", 61, vec![message(1, "Kept on the server")]),
        ],
        ..Default::default()
    }
}

struct Rig {
    imap: FakeImap,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let imap = Fake::imap(world());
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let servers = json!({
            "email": "ada@example.org", "displayName": "Ada",
            "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
        });
        let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        let mut rig = Rig { imap, s, account };
        assert_eq!(rig.sync()["new"], 6);
        rig
    }

    fn run(&mut self, id: &str, params: Value) -> Value {
        self.s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
    }

    /// One Send/Receive, as the app does it; returns what applying it said, problems and all.
    fn sync_any(&mut self) -> Value {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        let applied = apply_sync(&mut self.s, result).unwrap();
        // Whatever came back, the mailbox hangs together.
        assert!(self.s.mailbox.clone().repair().is_empty(), "{:?}", self.s.mailbox.clone().repair());
        applied
    }

    /// One Send/Receive that goes without a problem.
    fn sync(&mut self) -> Value {
        let applied = self.sync_any();
        assert_eq!(applied["errors"], json!([]));
        applied
    }

    /// The account's folders by their path here, each with the mailbox it follows and how many messages it has.
    fn tree(&self) -> Vec<(String, String, usize)> {
        let mut tree: Vec<(String, String, usize)> = self
            .s
            .mailbox
            .folders
            .iter()
            .filter(|f| f.account == self.account && f.kind != FolderKind::Outbox)
            .map(|f| (self.s.mailbox.folder_path(f.id), f.remote.clone(), self.s.mailbox.count(f.id)))
            .collect();
        tree.sort();
        tree
    }

    fn by_path(&self, path: &str) -> Option<Id> {
        self.s.mailbox.folders.iter().find(|f| f.account == self.account && self.s.mailbox.folder_path(f.id) == path).map(|f| f.id)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.imap.world().mailboxes.iter().map(|m| m.raw.clone()).collect();
        names.sort();
        names
    }

    fn sent(&self, text: &str) -> usize {
        self.imap.world().sent(text)
    }

    fn waiting(&self) -> usize {
        unpushed(&self.s, self.account)
    }
}

fn row(path: &str, remote: &str, count: usize) -> (String, String, usize) {
    (path.into(), remote.into(), count)
}

/// The tree a first run leaves, with the standard folders that don't follow a mailbox.
fn first_tree() -> Vec<(String, String, usize)> {
    vec![
        row("Archive", "", 0),
        row("Deleted Items", "Deleted Items", 0),
        row("Drafts", "", 0),
        row("Inbox", "INBOX", 2),
        row("Junk Email", "", 0),
        row("Keep", "Keep", 1),
        row("Old", "Old", 1),
        row("Projects", "Projects", 1),
        row("Projects/Apollo", "Projects/Apollo", 1),
        row("Sent Items", "Sent Items", 0),
    ]
}

#[test]
fn a_folder_the_server_no_longer_has_goes_here_too_but_what_only_exists_here_stays() {
    let mut r = Rig::new();
    assert_eq!(r.tree(), first_tree());
    // A copy made here, in a folder that goes from the server before the copy can go up.
    let one = r.s.mailbox.messages.values().find(|m| m.subject == "One").map(|m| m.id).unwrap();
    let keep = r.by_path("Keep").unwrap();
    r.run("mail.copy", json!({"id": one, "folder": keep}));

    // Deleted in another program, or emptied out of Deleted Items there.
    r.imap.world().mailboxes.retain(|m| !["Old", "Keep", "Projects/Apollo"].contains(&m.raw.as_str()));
    // A list that may be short takes nothing for gone.
    r.imap.world().refuse.push("LIST".into());
    let applied = r.sync_any();
    assert_ne!(applied["errors"], json!([]));
    r.imap.world().refuse.clear();
    r.imap.world().mailboxes.retain(|m| m.raw != "INBOX");
    r.sync_any();
    let mut expected = first_tree();
    expected.iter_mut().find(|row| row.0 == "Keep").unwrap().2 = 2;
    assert_eq!(r.tree(), expected);

    // The whole list: what came from the server goes with the mailbox, the folder holding what
    // only exists here stays and follows no mailbox, and its sibling is untouched.
    r.imap.world().mailboxes.insert(0, mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "One"), message(2, "Two")]));
    let applied = r.sync();
    assert_eq!(applied["new"], 0);
    let mut expected = first_tree();
    expected.retain(|row| !["Old", "Projects/Apollo"].contains(&row.0.as_str()));
    *expected.iter_mut().find(|row| row.0 == "Keep").unwrap() = row("Keep", "", 1);
    assert_eq!(r.tree(), expected);
    assert!(r.s.mailbox.messages.values().all(|m| m.subject != "Old news" && m.subject != "Launch" && m.subject != "Kept on the server"));
    // And it stays that way.
    r.sync();
    assert_eq!(r.tree(), expected);
    assert_eq!(r.waiting(), 0);
}

#[test]
fn a_deleted_folder_goes_into_deleted_items_on_the_server_and_is_there_once() {
    let mut r = Rig::new();
    let projects = r.by_path("Projects").unwrap();
    // Undone before the server heard: nothing to tell it.
    r.run("folder.delete", json!({"id": projects}));
    assert_eq!(r.waiting(), 1);
    r.run("edit.undo", json!({}));
    assert_eq!((r.waiting(), r.by_path("Projects")), (0, Some(projects)));

    r.run("folder.delete", json!({"id": projects}));
    assert_eq!(r.s.mailbox.folder_path(projects), "Deleted Items/Projects");
    assert_eq!(r.waiting(), 1);
    let applied = r.sync();
    assert_eq!((applied["new"].as_u64(), applied["pushed"].as_u64()), (Some(0), Some(1)));
    assert_eq!(r.sent("RENAME \"Projects\" \"Deleted Items/Projects\""), 1);
    assert_eq!(r.names(), ["Deleted Items", "Deleted Items/Projects", "Deleted Items/Projects/Apollo", "INBOX", "Keep", "Old", "Sent Items"]);
    // What reached the server can't be undone here.
    assert!(!r.s.can_undo());
    let mut expected = first_tree();
    for row in &mut expected {
        if row.0.starts_with("Projects") {
            row.0 = format!("Deleted Items/{}", row.0);
            row.1 = format!("Deleted Items/{}", row.1);
        }
    }
    expected.sort();
    for round in 0..2 {
        assert_eq!(r.tree(), expected, "round {round}");
        assert_eq!(r.sync()["new"], 0);
    }
    assert_eq!(r.waiting(), 0);

    // Deleted from Deleted Items: deleted for good there too, the innermost first.
    r.run("folder.delete", json!({"id": projects}));
    assert_eq!(r.waiting(), 2);
    r.sync();
    assert_eq!(r.sent("DELETE \"Deleted Items/Projects/Apollo\""), 1);
    assert_eq!(r.sent("DELETE \"Deleted Items/Projects\""), 1);
    assert_eq!(r.names(), ["Deleted Items", "INBOX", "Keep", "Old", "Sent Items"]);
    r.sync();
    expected.retain(|row| !row.0.starts_with("Deleted Items/"));
    assert_eq!((r.tree(), r.waiting()), (expected, 0));
}

#[test]
fn renames_moves_and_new_folders_round_trip() {
    let mut r = Rig::new();
    let (projects, inbox) = (r.by_path("Projects").unwrap(), r.by_path("Inbox").unwrap());
    r.run("folder.rename", json!({"id": projects, "name": "Plans"}));
    r.sync();
    assert_eq!(r.sent("RENAME \"Projects\" \"Plans\""), 1);
    r.run("folder.move", json!({"id": projects, "parent": inbox}));
    let q3 = r.run("folder.new", json!({"name": "Q3", "parent": projects}))["id"].as_u64().unwrap();
    // The folder made inside it is made where its parent is once that has moved.
    assert_eq!(r.waiting(), 2);
    r.sync();
    assert_eq!(r.sent("RENAME \"Plans\" \"INBOX/Plans\""), 1);
    assert_eq!(r.sent("CREATE \"INBOX/Plans/Q3\""), 1);
    assert_eq!(r.names(), ["Deleted Items", "INBOX", "INBOX/Plans", "INBOX/Plans/Apollo", "INBOX/Plans/Q3", "Keep", "Old", "Sent Items"]);
    for _ in 0..2 {
        let applied = r.sync();
        assert_eq!((applied["new"].as_u64(), applied["pushed"].as_u64()), (Some(0), Some(0)));
    }
    let mut expected = first_tree();
    expected.retain(|row| !row.0.starts_with("Projects"));
    expected.extend([
        row("Inbox/Plans", "INBOX/Plans", 1),
        row("Inbox/Plans/Apollo", "INBOX/Plans/Apollo", 1),
        row("Inbox/Plans/Q3", "INBOX/Plans/Q3", 0),
    ]);
    expected.sort();
    assert_eq!((r.tree(), r.waiting()), (expected, 0));
    assert_eq!(r.s.mailbox.folder(q3).map(|f| f.remote.as_str()), Some("INBOX/Plans/Q3"));

    // Moved back out in another program: the folder here follows, once.
    {
        let mut world = r.imap.world();
        for m in &mut world.mailboxes {
            if let Some(rest) = m.raw.strip_prefix("INBOX/") {
                m.raw = rest.to_string();
            }
        }
    }
    r.sync();
    let paths: Vec<String> = r.tree().into_iter().map(|row| row.0).filter(|path| path.contains("Plans")).collect();
    assert_eq!(paths, ["Plans", "Plans/Apollo", "Plans/Q3"]);
}

#[test]
fn a_folder_change_the_server_refuses_stays_here_and_is_not_asked_for_again() {
    let mut r = Rig::new();
    let projects = r.by_path("Projects").unwrap();
    r.imap.world().refuse.push("RENAME".into());
    r.run("folder.rename", json!({"id": projects, "name": "Plans"}));
    let applied = r.sync_any();
    assert!(applied["errors"][0].as_str().unwrap().contains("Couldn't move or rename the folder \"Projects\""), "{applied}");
    assert_eq!((r.s.mailbox.folder_path(projects), r.waiting()), ("Plans".to_string(), 0));
    assert_eq!(r.s.mailbox.folder(projects).map(|f| f.remote.as_str()), Some("Projects"));
    // The folder still follows its mailbox, and gets its news.
    r.imap.world().mailbox("Projects").messages.push(message(2, "Project two"));
    assert_eq!(r.sync()["new"], 1);
    assert_eq!(r.s.mailbox.count(projects), 2);
}
