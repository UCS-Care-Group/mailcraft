// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Switching a Microsoft account kept in step by IMAP over to Microsoft Graph (R3.7), from end to
//! end: a mailbox written by 0.2.1, with a flag and a move made offline, a local folder, a draft
//! never sent and a favourite folder, against the fake IMAP server (the last push, which waits
//! while the server can't be reached) and then the Graph stand-in (the fresh download, seeded with
//! what the IMAP server has once the push is in). All five are kept. Nothing here leaves this
//! machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use std::sync::atomic::AtomicBool;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request};
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request};
use mailcraft_model::{AccountKind, FlagStatus, FolderKind, Id, Security, Server};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// The Microsoft account of the 0.2.1 mailbox (`crates/engine/testdata/mailbox-0.2.1.mailcraft`).
const ACCOUNT: Id = 18;
const OWNER: &str = "owner@contoso.example";
/// What the fake IMAP server takes for a sign-in with Microsoft (XOAUTH2).
const IMAP_TOKEN: &str = "imap-access-token";

fn eml(subject: &str, id: &str) -> Vec<u8> {
    format!(
        "From: Noor Haddad <noor@example.org>\r\nTo: Owner <owner@contoso.example>\r\nSubject: {subject}\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
         Message-ID: <{id}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe body of {subject}.\r\n"
    )
    .into_bytes()
}

fn message(uid: u32, flags: &str, subject: &str, id: &str) -> FakeMessage {
    FakeMessage { uid, flags: flags.into(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body: eml(subject, id), claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, uid_validity: u32, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages, refuse_open: false, next_uid: 0 }
}

/// The account's IMAP server: the 0.2.1 mailbox follows its INBOX under `UIDVALIDITY` 41.
fn imap_server() -> FakeImap {
    Fake::imap(ImapWorld {
        user: OWNER.into(),
        token: IMAP_TOKEN.into(),
        can_move: true,
        uidplus: true,
        mailboxes: vec![
            mailbox(
                "INBOX",
                "\\HasNoChildren",
                41,
                vec![message(1, "\\Seen", "Quarterly figures", "q2"), message(2, "", "Invoice 42", "invoice-42"), message(3, "", "Lunch", "lunch")],
            ),
            mailbox("Projects", "\\HasNoChildren", 51, Vec::new()),
            mailbox("Drafts", "\\HasNoChildren \\Drafts", 61, Vec::new()),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 71, Vec::new()),
            mailbox("Deleted Items", "\\HasNoChildren \\Trash", 81, Vec::new()),
        ],
        ..Default::default()
    })
}

struct Rig {
    imap: FakeImap,
    graph: StandIn,
    s: Session,
    dir: std::path::PathBuf,
}

impl Rig {
    /// The 0.2.1 mailbox, its Microsoft account pointed at the fake IMAP server, kept in a file.
    fn new() -> Rig {
        let imap = imap_server();
        let graph = StandIn::start().unwrap();
        let dir = std::env::temp_dir().join(format!("mailcraft-r37-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mailbox.mailcraft");
        std::fs::write(&path, include_bytes!("../../../crates/engine/testdata/mailbox-0.2.1.mailcraft")).unwrap();
        let mut mb = mailcraft_engine::io::open_path(&path).unwrap();
        let a = mb.account_mut(ACCOUNT).unwrap();
        assert_eq!((a.email.as_str(), a.kind), (OWNER, AccountKind::Imap));
        a.incoming = Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None });
        let mut s = Session::at(mb, 1_800_000_000, 0);
        s.path = Some(path);
        Rig { imap, graph, s, dir }
    }

    fn run(&mut self, id: &str, params: Value) -> Value {
        self.s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
    }

    /// One IMAP Send/Receive of the account, as the app does it; `offline`: the server can't be
    /// reached.
    fn imap_sync(&mut self, offline: bool) -> Value {
        let mut request = sync_request(&self.s, ACCOUNT).unwrap();
        request.access_token = IMAP_TOKEN.into();
        if offline {
            // A port nobody listens on any more.
            let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            request.account.incoming = Some(Server { host: "127.0.0.1".into(), port: closed, security: Security::None });
        }
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors.is_empty(), !offline, "{:?}", result.errors);
        apply_sync(&mut self.s, result).unwrap()
    }

    /// One Graph Send/Receive of the account, against the stand-in.
    fn graph_sync(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, ACCOUNT).unwrap();
        let _routed = self.graph.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    fn folder(&self, name: &str) -> Id {
        self.s.mailbox.folders.iter().find(|f| f.account == ACCOUNT && f.name == name).map(|f| f.id).unwrap_or_else(|| panic!("no folder {name}"))
    }

    /// The account's messages called `subject` (the 0.2.1 mailbox's other account has some too).
    fn by_subject(&self, subject: &str) -> Vec<&mailcraft_model::Message> {
        self.s.mailbox.messages.values().filter(|m| m.subject == subject && m.account == ACCOUNT).map(AsRef::as_ref).collect()
    }

    fn switch(&mut self) -> Value {
        self.run("account.graphSwitch", json!({"account": ACCOUNT, "answer": "switch"}))
    }

    /// The stand-in, seeded with what the IMAP server has (the mailbox both protocols show).
    fn seed_graph_from_imap(&self) {
        let world = self.imap.world();
        let mut state = self.graph.state();
        for m in &world.mailboxes {
            let folder = match m.raw.as_str() {
                "INBOX" => state.well_known("inbox"),
                "Drafts" => state.well_known("drafts"),
                "Sent Items" => state.well_known("sentitems"),
                "Deleted Items" => state.well_known("deleteditems"),
                other => state.add_folder(other, None),
            };
            for message in &m.messages {
                let text = String::from_utf8_lossy(&message.body).into_owned();
                let header = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)).unwrap_or_default().trim().to_string();
                let (subject, internet_id) = (header("Subject:"), header("Message-ID:"));
                let id = state.mailbox.add(&folder, &subject, 1_790_000_000);
                state.mailbox.update(&id, |there| {
                    there.read = message.flags.contains("\\Seen");
                    there.flag = if message.flags.contains("\\Flagged") { "flagged".into() } else { "notFlagged".into() };
                    there.internet_id = internet_id.clone();
                    there.mime = Some(text.clone());
                });
            }
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_mailbox_of_0_2_1_switches_and_keeps_its_offline_changes_local_folder_draft_and_favourite() {
    let mut rig = Rig::new();
    // As 0.2.1 had it: the account's mail down by IMAP.
    rig.imap_sync(false);
    let (inbox, projects) = (rig.folder("Inbox"), rig.folder("Projects"));
    let figures = rig.by_subject("Quarterly figures")[0].id;
    let invoice = rig.by_subject("Invoice 42")[0].id;
    // A favourite of MailCraft's own.
    rig.s.prefs.own_favorites = true;
    rig.run("folder.favorite", json!({"id": projects, "value": true}));
    // A local folder, with a message in it.
    let local = rig.run("account.add", json!({"email": "me@example.org", "displayName": "Me"}))["id"].as_u64().unwrap();
    let recipes = rig.run("folder.new", json!({"name": "Recipes", "account": local, "parent": null}))["id"].as_u64().unwrap();
    let data = mailcraft_charset::base64::STANDARD.encode(eml("Lentil soup", "lentil-soup"));
    rig.run("file.import", json!({"name": "soup.eml", "data": data, "folder": recipes}));
    let soup = rig.s.mailbox.messages_in(recipes).find(|m| m.subject == "Lentil soup").map(|m| m.as_ref().clone()).unwrap();
    // A draft never sent.
    rig.run("mail.new", json!({"account": ACCOUNT, "to": "kit@example.org", "subject": "Plans for Friday", "body": "Shall we meet at ten?"}));
    let draft = rig.run("compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    rig.run("compose.close", json!({}));
    // Offline: a flag and a move, which the IMAP server hasn't heard of.
    rig.run("mail.flag", json!({"ids": [figures]}));
    rig.run("mail.move", json!({"ids": [invoice], "folder": projects}));

    // Switch: what waits goes first, and while the server can't be reached the switch waits.
    let said = rig.switch();
    let waiting = said["waiting"].as_u64().unwrap_or(0);
    assert!(said["switched"] == json!(false) && waiting > 0, "{said}");
    rig.imap_sync(true);
    assert_eq!(rig.switch()["waiting"], json!(waiting));
    assert_eq!(rig.s.mailbox.account(ACCOUNT).unwrap().kind, AccountKind::Imap);
    assert_eq!(std::fs::read_dir(&rig.dir).unwrap().count(), 1, "no backup while it waits: only the mailbox file");
    // Online: the push goes, and then the switch.
    rig.imap_sync(false);
    {
        let world = rig.imap.world();
        let inbox_there = world.mailboxes.iter().find(|m| m.raw == "INBOX").unwrap();
        assert!(inbox_there.messages.iter().any(|m| m.flags.contains("\\Flagged")), "the flag reached the IMAP server");
        assert_eq!(world.mailboxes.iter().find(|m| m.raw == "Projects").unwrap().messages.len(), 1, "so did the move");
    }
    let before = rig.s.mailbox.clone();
    let said = rig.switch();
    assert_eq!(said["switched"], json!(true), "{said}");
    // The backup, beside the file with the date in its name, is the mailbox as it was.
    let backup = rig.dir.join("mailbox (backup 2027-01-15).mailcraft");
    assert_eq!(said["backup"], json!(backup.to_string_lossy()));
    let mut was = before.clone();
    was.account_mut(ACCOUNT).unwrap().graph_switch = mailcraft_model::GraphSwitch::NotAsked;
    // (0.2.1's choice of how Favorites are read is read and never written again.)
    was.take_retired_favorites_choices();
    assert_eq!(mailcraft_engine::io::open_path(&backup).unwrap(), was);
    rig.run("file.save", json!({}));
    let connections = rig.imap.world().connections;

    // The fresh download, through Graph alone.
    rig.seed_graph_from_imap();
    for _ in 0..3 {
        let answers = rig.graph_sync();
        let errors: Vec<&Value> = answers.iter().flat_map(|a| a["errors"].as_array().into_iter().flatten()).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }
    assert_eq!(rig.imap.world().connections, connections, "no IMAP after the switch");
    let a = rig.s.mailbox.account(ACCOUNT).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none()), (AccountKind::Microsoft, true, true));

    // 1. The flag made offline.
    let figures = rig.by_subject("Quarterly figures");
    assert_eq!(figures.len(), 1);
    assert_eq!((figures[0].folder, figures[0].flag, figures[0].server.is_some()), (inbox, FlagStatus::Flagged, true));
    // 2. The move made offline.
    let invoice = rig.by_subject("Invoice 42");
    assert_eq!(invoice.len(), 1);
    assert_eq!((invoice[0].folder, invoice[0].server.is_some()), (projects, true));
    // 3. The local folder, with its message, as it was.
    assert_eq!(rig.s.mailbox.folder(recipes).map(|f| (f.account, f.name.as_str())), Some((local, "Recipes")));
    assert_eq!(rig.s.mailbox.message(soup.id), Some(&soup));
    // 4. The draft never sent: on the server's Drafts now, once, and here once, as that.
    let drafts_there: Vec<String> = {
        let state = rig.graph.state();
        let drafts = state.well_known("drafts");
        state.mailbox.messages.values().filter(|m| m.folder == drafts).map(|m| m.subject.clone()).collect()
    };
    assert_eq!(drafts_there, ["Plans for Friday"]);
    let draft_here = rig.by_subject("Plans for Friday");
    assert_eq!(draft_here.len(), 1);
    assert_eq!((draft_here[0].id, draft_here[0].folder, draft_here[0].server.is_some()), (draft, rig.folder("Drafts"), true));
    // 5. The favourite: the same folder, the server's Projects now.
    let projects_there = rig.graph.state().folder_named("Projects").unwrap().id.clone();
    let f = rig.s.mailbox.folder(projects).unwrap();
    assert!(f.favorite && !f.awaiting_listing);
    assert_eq!(f.server.as_ref().map(|s| s.id.as_str()), Some(projects_there.as_str()));
    // And nothing else came twice: the Inbox has what the server's has.
    let mut inbox_here: Vec<String> = rig.s.mailbox.messages_in(inbox).map(|m| m.subject.clone()).collect();
    inbox_here.sort();
    assert_eq!(inbox_here, ["Lunch", "Quarterly figures"]);
    assert!(rig.s.mailbox.folders.iter().all(|f| f.account != ACCOUNT || f.kind != FolderKind::Custom || f.server.is_some()));
}
