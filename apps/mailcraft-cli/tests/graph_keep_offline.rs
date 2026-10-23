// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline (#143), from end to end against the strict Graph stand-in: a Microsoft
//! account's mail within its window is fetched whole in the background, older mail keeps its
//! headers and preview and is fetched when it is opened. Nothing here leaves this machine; every
//! message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};
use mailcraft_model::{Id, Mailbox, Message};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    /// A Microsoft account keeping `keep` offline, and a server whose Inbox has a message
    /// received each of `days_ago`; their ids there.
    fn new(keep: &str, days_ago: &[i64]) -> (Rig, Vec<String>) {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": "offline@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        s.run("account.edit", &json!({"id": account, "keepOffline": keep})).unwrap();
        let ids = {
            let mut state = server.state();
            let inbox = state.well_known("inbox");
            days_ago.iter().map(|d| state.mailbox.add(&inbox, &format!("{d} days ago"), NOW - d * DAY)).collect()
        };
        let mut rig = Rig { server, s, account };
        rig.run();
        (rig, ids)
    }

    fn run(&mut self) {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let s = &mut self.s;
        mailcraft_net::graph::sync_mail(TOKEN, &request, &AtomicBool::new(false), &mut |piece| {
            apply_graph_mail(s, piece).unwrap();
        });
    }

    /// The background's next batch, or the message the owner opened: what was asked for, and
    /// what taking it in said.
    fn fetch(&mut self, message: Option<Id>) -> (Vec<String>, Value) {
        let request = graph_fetch_request(&self.s, self.account, message, 100).unwrap();
        let asked = request.ids.clone();
        let _routed = self.server.route();
        let result = mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false));
        (asked, apply_graph_fetch(&mut self.s, result).unwrap())
    }

    fn here(&self, id: &str) -> &Message {
        self.s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).unwrap()
    }
}

#[test]
fn the_background_fetches_mail_within_the_window_and_opening_older_mail_fetches_it() {
    let (mut rig, ids) = Rig::new("1m", &[1, 10, 40, 400]);
    assert!(ids.iter().all(|id| rig.here(id).unfetched), "headers and preview first");
    let (asked, said) = rig.fetch(None);
    assert_eq!(asked, [ids[0].clone(), ids[1].clone()], "within a month, newest first");
    assert_eq!((said["waiting"].as_u64(), said["all"].as_u64()), (Some(0), Some(2)), "{said}");
    assert!(!rig.here(&ids[0]).unfetched && !rig.here(&ids[1]).unfetched);
    assert!(rig.here(&ids[2]).unfetched && rig.here(&ids[3]).unfetched);
    assert!(!rig.here(&ids[3]).body.text.is_empty(), "its preview");
    let (asked, _) = rig.fetch(None);
    assert!(asked.is_empty(), "nothing more in the background: {asked:?}");

    // Opened: an older message is fetched whole all the same.
    let old = rig.here(&ids[3]).id;
    let (asked, _) = rig.fetch(Some(old));
    assert_eq!(asked, [ids[3].clone()]);
    assert!(!rig.here(&ids[3]).unfetched);

    // Longer: the newly covered mail comes in the background, newest first.
    rig.s.run("account.edit", &json!({"id": rig.account, "keepOffline": "2y"})).unwrap();
    let (asked, _) = rig.fetch(None);
    assert_eq!(asked, [ids[2].clone()]);
}

/// A message with a few kilobytes of text and an attachment of its own, so that both are kept
/// in content files beside the mailbox file.
fn mime(n: usize) -> String {
    let text = format!("Message {n} says this. ").repeat(200);
    let file = mailcraft_charset::base64::STANDARD.encode(format!("attachment {n} ").repeat(300));
    format!(
        "From: Kit <kit@example.org>\r\nTo: offline@example.org\r\nSubject: Report {n}\r\nMessage-ID: <r{n}@example.org>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--b\r\n\
Content-Type: application/pdf; name=\"report{n}.pdf\"\r\nContent-Disposition: attachment; filename=\"report{n}.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{file}\r\n--b--\r\n"
    )
}

/// The content files beside the mailbox file at `path`.
fn content_files(path: &std::path::Path) -> usize {
    let dir = mailcraft_engine::store::blob_dir(path);
    std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|sub| std::fs::read_dir(sub.path()).into_iter().flatten().flatten()).count()
}

fn temp(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mailcraft-143-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("offline.mailcraft");
    (dir, path)
}

#[test]
fn mail_outside_a_shortened_window_goes_back_to_its_headers_and_its_files_leave_the_disk() {
    let (mut rig, ids) = Rig::new("all", &[1, 40, 41, 400]);
    for (n, id) in ids.iter().enumerate() {
        rig.server.state().mailbox.update(id, |m| m.mime = Some(mime(n)));
    }
    rig.fetch(None);
    assert!(ids.iter().all(|id| !rig.here(id).unfetched), "everything, with All");
    let (dir, path) = temp("thin");
    rig.s.run("file.save", &json!({"path": path.to_string_lossy()})).unwrap();
    let before = content_files(&path);
    assert_eq!(before, 8, "a text and an attachment each");

    // A copy in a local folder has the same files: they stay for it.
    let local = rig.s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let local_inbox = rig.s.mailbox.folder_of_kind(local, mailcraft_model::FolderKind::Inbox).unwrap();
    rig.s.run("mail.copy", &json!({"folder": local_inbox, "ids": [rig.here(&ids[3]).id]})).unwrap();

    // Marks, here and waiting to go up: read, flagged (an undo step), a category.
    let (kept, flagged) = (rig.here(&ids[1]).id, rig.here(&ids[2]).id);
    rig.s.run("mail.markRead", &json!({"ids": [kept]})).unwrap();
    rig.s.run("mail.flag", &json!({"id": flagged})).unwrap();
    let marks = |rig: &Rig, id: &str| {
        let m = rig.here(id);
        (m.read, m.flag, m.categories.clone(), m.folder)
    };
    let (kept_marks, flagged_marks) = (marks(&rig, &ids[1]), marks(&rig, &ids[2]));

    rig.s.run("account.edit", &json!({"id": rig.account, "keepOffline": "1m"})).unwrap();
    let done = mailcraft_engine::cmd::offline::thin(&mut rig.s, Some(rig.account), 1_000);
    assert_eq!((done.messages, done.left), (3, 0), "{done:?}");
    assert!(done.bytes > 3 * 8_000, "{done:?}");
    for id in &ids[1..] {
        let m = rig.here(id);
        assert!(m.unfetched && m.attachments.is_empty() && m.body.html.is_none() && m.has_attachments(), "{id}");
        assert!(m.body.text.starts_with("Message ") && m.body.text.chars().count() <= 256, "a preview: {}", m.body.text);
        assert_eq!(m.subject, format!("Report {}", ids.iter().position(|x| x == id).unwrap()), "its headers stay");
    }
    assert!(!rig.here(&ids[0]).unfetched, "within the window: whole");
    assert_eq!((marks(&rig, &ids[1]), marks(&rig, &ids[2])), (kept_marks, flagged_marks.clone()), "marks stay");

    // Saved: their content files are gone at once, the one within stays.
    rig.s.run("file.save", &json!({})).unwrap();
    assert_eq!(content_files(&path), 4, "the one within, and the local copy's");
    let copy = rig.s.mailbox.messages.values().find(|m| m.folder == local_inbox).unwrap().id;
    let copy = rig.s.loaded_message(copy).unwrap();
    assert!(copy.body.text.starts_with("Message 3 says") && copy.attachments[0].data.len() > 3_000);

    // Undo: the window goes back to All, then the flag goes; the content doesn't come back.
    rig.s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(rig.s.mailbox.account(rig.account).unwrap().keep_offline, mailcraft_model::KeepOffline::All);
    assert!(rig.here(&ids[2]).unfetched && rig.here(&ids[2]).flag == flagged_marks.1);
    rig.s.run("edit.undo", &json!({})).unwrap();
    let m = rig.here(&ids[2]);
    assert!(m.unfetched && m.attachments.is_empty() && m.flag != flagged_marks.1, "{m:?}");
    let id = m.id;
    rig.s.run("account.edit", &json!({"id": rig.account, "keepOffline": "1m"})).unwrap();
    // Opened again, it comes again.
    rig.fetch(Some(id));
    assert!(!rig.here(&ids[2]).unfetched && rig.here(&ids[2]).attachments.len() == 1);
    // Opened outside the window, it stays whole until the mailbox is opened again.
    assert_eq!(mailcraft_engine::cmd::offline::thin(&mut rig.s, None, 1_000).messages, 0, "kept until the next start");
    rig.s.run("file.save", &json!({})).unwrap();
    rig.s.run("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(rig.s.run("account.thinOffline", &json!({})).unwrap()["thinned"], 1);
    assert!(rig.here(&ids[2]).unfetched);
    assert_eq!(mailcraft_engine::cmd::offline::thin(&mut rig.s, None, 1_000).messages, 0, "nothing twice");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shared_mailbox_fetches_as_far_back_as_its_owner_keeps() {
    let (mut rig, _) = Rig::new("1w", &[]);
    let shared =
        rig.s.run("account.addSharedMailbox", &json!({"account": rig.account, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = rig.s.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Inbox).unwrap();
    rig.s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "in".into(), ..Default::default() });
    for (n, days) in [(1, 2), (2, 20)] {
        let server = mailcraft_model::sync::ServerMessage { id: format!("S{n}"), folder: "in".into(), ..Default::default() };
        let m = Message { account: shared, folder: inbox, received: NOW - days * DAY, unfetched: true, server: Some(server), ..Default::default() };
        rig.s.mailbox.add_message(m).unwrap();
    }
    let ids = |s: &Session| graph_fetch_request(s, shared, None, 100).unwrap().ids;
    assert_eq!(ids(&rig.s), ["S1"], "a week, as its owner keeps");
    rig.s.run("account.edit", &json!({"id": rig.account, "keepOffline": "1m"})).unwrap();
    assert_eq!(ids(&rig.s), ["S1", "S2"]);
}
