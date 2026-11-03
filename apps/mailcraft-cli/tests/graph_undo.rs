// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo for a Microsoft account kept in step through Microsoft Graph (R3.5), from end to end
//! against the strict Graph stand-in: an undo after a change reached the server goes up as the
//! reverse change (a move back, a flag cleared, categories as they were, out of Deleted Items);
//! one before it did sends nothing; a delete for good says it can't be undone; sending is never
//! undone, so nothing is unsent or sent twice; a draft written back goes up as it was; mail that
//! came down meanwhile stays. Nothing here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request, unpushed};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    /// A Microsoft account whose server has these messages in its Inbox, all of them down here.
    fn new(address: &str, subjects: &[&str]) -> Rig {
        let server = StandIn::start().unwrap();
        {
            let mut state = server.state();
            let inbox = state.well_known("inbox");
            for (n, subject) in subjects.iter().enumerate() {
                state.mailbox.add(&inbox, subject, NOW - 3_600 - n as i64 * 60);
            }
        }
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let mut rig = Rig { server, s, account };
        rig.run();
        rig
    }

    fn run(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    fn here(&self, subject: &str) -> Id {
        self.s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap_or_else(|| panic!("no message {subject} here"))
    }

    fn has(&self, subject: &str) -> bool {
        self.s.mailbox.messages.values().any(|m| m.subject == subject)
    }

    fn act(&mut self, command: &str, subject: &str, mut more: Value) -> Value {
        more["ids"] = json!([self.here(subject)]);
        self.s.run(command, &more).unwrap()
    }

    fn undo(&mut self) -> Value {
        self.s.run("edit.undo", &json!({})).unwrap()
    }

    fn folder(&self, kind: FolderKind) -> Id {
        self.s.mailbox.folder_of_kind(self.account, kind).unwrap()
    }

    fn there(&self, subject: &str) -> Option<mailcraft_net::graph::standin::Message> {
        self.server.state().mailbox.messages.values().find(|m| m.subject == subject).cloned()
    }

    fn all_there(&self, subject: &str) -> Vec<mailcraft_net::graph::standin::Message> {
        self.server.state().mailbox.messages.values().filter(|m| m.subject == subject).cloned().collect()
    }

    fn asked(&self) -> usize {
        self.server.state().requests.len()
    }

    /// The changes of messages among the requests since `from`.
    fn changes_since(&self, from: usize) -> Vec<(String, String)> {
        let state = self.server.state();
        state.requests[from..]
            .iter()
            .filter(|r| r.target.starts_with("/v1.0/me/messages/") || r.target.starts_with("/v1.0/me/sendMail") || r.target.ends_with("/send"))
            .filter(|r| matches!(r.method.as_str(), "PATCH" | "DELETE" | "POST"))
            .map(|r| (r.method.clone(), r.target.clone()))
            .collect()
    }

    fn errors(answers: &[Value]) -> Vec<Value> {
        answers.iter().flat_map(|a| a["errors"].as_array().cloned().unwrap_or_default()).collect()
    }
}

#[test]
fn undoing_a_move_a_flag_categories_and_a_delete_sends_their_reverses() {
    let mut rig = Rig::new("undo-each@example.org", &["File me", "Flag me", "Tag me", "Bin me"]);
    let inbox = rig.server.state().well_known("inbox");
    let (archive, deleted) = {
        let state = rig.server.state();
        (state.well_known("archive"), state.well_known("deleteditems"))
    };
    rig.act("mail.move", "File me", json!({"folder": rig.folder(FolderKind::Archive)}));
    rig.act("mail.flag", "Flag me", json!({}));
    rig.act("mail.categorize", "Tag me", json!({"category": "Blue category"}));
    rig.act("mail.delete", "Bin me", json!({}));
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new());
    assert_eq!(rig.there("File me").unwrap().folder, archive);
    assert_eq!(rig.there("Flag me").unwrap().flag, "flagged");
    assert_eq!(rig.there("Tag me").unwrap().categories, ["Blue category"]);
    assert_eq!(rig.there("Bin me").unwrap().folder, deleted);

    // The history outlived the run: each change is undone here, and goes up as its reverse.
    assert_eq!(rig.s.undo_label(), Some("Delete"));
    for _ in 0..4 {
        let undone = rig.undo();
        assert_eq!(undone["notUndone"], json!([]), "{undone}");
    }
    assert_eq!(rig.s.undo_label(), Some("Add Account"));
    let s = &rig.s.mailbox;
    let inbox_here = rig.folder(FolderKind::Inbox);
    for subject in ["File me", "Bin me"] {
        assert_eq!(s.message(rig.here(subject)).unwrap().folder, inbox_here, "{subject} back here");
    }
    assert_eq!(unpushed(&rig.s, rig.account), 4);
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new());
    assert_eq!(rig.there("File me").unwrap().folder, inbox, "moved back");
    assert_eq!(rig.there("Flag me").unwrap().flag, "notFlagged", "the flag cleared");
    assert!(rig.there("Tag me").unwrap().categories.is_empty(), "the category taken off");
    assert_eq!(rig.there("Bin me").unwrap().folder, inbox, "out of Deleted Items");
    assert_eq!(unpushed(&rig.s, rig.account), 0);

    // Redo is a change like any other: it goes up again.
    rig.s.run("edit.redo", &json!({})).unwrap();
    rig.run();
    assert_eq!(rig.there("File me").unwrap().folder, archive);
    // Nothing goes twice, and nothing came twice.
    let from = rig.asked();
    rig.run();
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());
    assert_eq!(rig.s.mailbox.messages.len(), 4);
}

#[test]
fn an_undo_before_the_change_went_up_sends_nothing() {
    let mut rig = Rig::new("undo-early@example.org", &["Flag me", "File me"]);
    rig.act("mail.flag", "Flag me", json!({}));
    rig.act("mail.move", "File me", json!({"folder": rig.folder(FolderKind::Archive)}));
    rig.undo();
    rig.undo();
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    let from = rig.asked();
    rig.run();
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());
}

#[test]
fn an_undo_after_the_server_changed_the_message_keeps_what_came_down() {
    let mut rig = Rig::new("undo-merge@example.org", &["Flag me"]);
    rig.act("mail.flag", "Flag me", json!({}));
    rig.run();
    // Read in Outlook meanwhile, and new mail came.
    {
        let mut state = rig.server.state();
        let id = state.mailbox.messages.values().find(|m| m.subject == "Flag me").unwrap().id.clone();
        assert!(state.mailbox.update(&id, |m| m.read = true));
        let inbox = state.well_known("inbox");
        state.mailbox.add(&inbox, "Arrived since", NOW - 60);
    }
    rig.run();
    assert!(rig.has("Arrived since"));
    rig.undo();
    assert!(rig.has("Arrived since"), "mail that came down after the change stays");
    let m = rig.s.mailbox.message(rig.here("Flag me")).unwrap();
    assert!(m.read, "what the server changed since stays");
    let from = rig.asked();
    rig.run();
    // Only the flag goes, not the read state of before.
    let there = rig.there("Flag me").unwrap();
    assert_eq!((there.flag.as_str(), there.read), ("notFlagged", true));
    assert_eq!(rig.changes_since(from).len(), 1, "{:?}", rig.changes_since(from));
    assert_eq!(rig.all_there("Arrived since").len(), 1);
}

#[test]
fn undoing_a_delete_for_good_says_it_cant_be_undone_here() {
    let mut rig = Rig::new("undo-gone@example.org", &["Gone for good", "Kept"]);
    rig.act("mail.deletePermanently", "Gone for good", json!({}));
    rig.run();
    assert!(rig.there("Gone for good").is_none());
    let undone = rig.undo();
    assert_eq!(undone["notUndone"], json!(["'Gone for good' was permanently deleted: that can't be undone here."]));
    assert!(rig.s.sync_status.contains("can't be undone here"), "{}", rig.s.sync_status);
    assert!(!rig.has("Gone for good"));
    let from = rig.asked();
    rig.run();
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());

    // Not yet gone up: it isn't brought back either, and the server's copy still goes.
    rig.act("mail.deletePermanently", "Kept", json!({}));
    let undone = rig.undo();
    assert_eq!(undone["notUndone"].as_array().map(Vec::len), Some(1));
    assert!(!rig.has("Kept"));
    rig.run();
    assert!(rig.there("Kept").is_none());
}

#[test]
fn sending_is_never_undone_nor_sent_twice() {
    let mut rig = Rig::new("undo-send@example.org", &[]);
    rig.s.run("mail.new", &json!({"account": rig.account, "to": "kit@example.org", "subject": "Lunch?", "body": "Hello."})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    rig.run();
    rig.s.run("compose.send", &json!({})).unwrap();
    // Undone before it went: it still goes, once, and the draft doesn't come back.
    let undone = rig.undo();
    assert_eq!(undone["notUndone"], json!(["Sending 'Lunch?' can't be undone."]));
    let outbox = rig.folder(FolderKind::Outbox);
    assert_eq!(rig.s.mailbox.messages_in(outbox).count(), 1);
    assert_eq!(rig.s.mailbox.messages_in(rig.folder(FolderKind::Drafts)).count(), 0);
    rig.run();
    assert_eq!(rig.server.state().sent.len(), 1);
    // Undone again after it went: nothing comes back, nothing goes again.
    // (The draft's save: the draft became the message sent.)
    assert_eq!(rig.s.undo_label(), Some("Save Draft"));
    rig.undo();
    rig.run();
    rig.run();
    assert_eq!(rig.server.state().sent.len(), 1);
    assert_eq!(rig.s.mailbox.messages_in(outbox).count(), 0);
    assert_eq!(rig.s.mailbox.messages_in(rig.folder(FolderKind::Drafts)).count(), 0);
}

#[test]
fn a_draft_written_back_goes_up_as_it_was() {
    let mut rig = Rig::new("undo-draft@example.org", &[]);
    rig.s.run("mail.new", &json!({"account": rig.account, "to": "kit@example.org", "subject": "Plan", "body": "First."})).unwrap();
    let draft = rig.s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    rig.s.run("compose.close", &json!({})).unwrap();
    rig.run();
    rig.s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    rig.s.run("compose.set", &json!({"subject": "Plan, second"})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    rig.s.run("compose.close", &json!({})).unwrap();
    rig.run();
    assert!(rig.there("Plan, second").is_some());
    rig.undo();
    assert_eq!(rig.s.mailbox.message(draft).unwrap().subject, "Plan");
    rig.run();
    assert!(rig.there("Plan").is_some() && rig.there("Plan, second").is_none());
    assert_eq!(rig.s.mailbox.messages_in(rig.folder(FolderKind::Drafts)).count(), 1, "one draft, not two");

    // Undoing the draft's delete brings it out of Deleted Items, there too.
    rig.s.run("mail.delete", &json!({"ids": [draft]})).unwrap();
    rig.run();
    rig.undo();
    rig.run();
    let drafts = rig.server.state().well_known("drafts");
    assert_eq!(rig.there("Plan").unwrap().folder, drafts);
}

#[test]
fn undoing_a_copy_takes_it_away_or_puts_it_in_deleted_items() {
    let mut rig = Rig::new("undo-copy@example.org", &["Copy me"]);
    let archive = rig.folder(FolderKind::Archive);
    // Not made on the server yet: never made.
    rig.act("mail.copy", "Copy me", json!({"folder": archive}));
    rig.undo();
    rig.run();
    assert_eq!(rig.all_there("Copy me").len(), 1);
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Copy me").count(), 1);
    // Made already: into Deleted Items there and here, nothing deleted for good.
    let copy = rig.act("mail.copy", "Copy me", json!({"folder": archive}))["copies"][0].as_u64().unwrap();
    rig.run();
    assert_eq!(rig.all_there("Copy me").len(), 2);
    rig.undo();
    assert_eq!(rig.s.mailbox.message(copy).unwrap().folder, rig.folder(FolderKind::Deleted));
    rig.run();
    let deleted = rig.server.state().well_known("deleteditems");
    let copies = rig.all_there("Copy me");
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().any(|m| m.folder == deleted));
    assert!(!rig.server.state().requests.iter().any(|r| r.method == "DELETE"));
}
