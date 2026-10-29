// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changes of messages up through Microsoft Graph (R3.3.2), from end to end: the engine's
//! `graph_mail_request` and `apply_graph_mail` with `mailcraft_net::graph::sync_mail` in between,
//! against the strict Graph stand-in. One test per kind of change, refusals put back and said,
//! changes on both sides, ten thousand marked read in batches, a batch answered three ways, a
//! copy whose answer was lost, and a move into a local folder that waits for the save. Nothing
//! here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request, unpushed};
use mailcraft_model::{FlagStatus, FolderKind, Id, Importance, Mailbox};
use mailcraft_net::graph::standin::{Fault, StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

/// A session with one Microsoft account kept in step through Graph, and a stand-in for its server.
/// Each test has an address of its own: the gate that counts a mailbox's requests is the process's.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new(address: &str) -> Rig {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        Rig { server, s, account }
    }

    /// One whole run (changes up, the folders, mail down), each piece taken in as it comes: what
    /// taking in each piece answered.
    fn run(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    /// Every message the account's server should fetch whole, fetched and taken in.
    fn fetch(&mut self) -> Value {
        let request = graph_fetch_request(&self.s, self.account, None, 100).unwrap();
        let _routed = self.server.route();
        let result = mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false));
        apply_graph_fetch(&mut self.s, result).unwrap()
    }

    /// The message here with this subject.
    fn here(&self, subject: &str) -> Id {
        self.s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap_or_else(|| panic!("no message {subject} here"))
    }

    /// Run `command` on the message with this subject, with `more` params.
    fn act(&mut self, command: &str, subject: &str, mut more: Value) -> Value {
        more["ids"] = json!([self.here(subject)]);
        self.s.run(command, &more).unwrap()
    }

    fn has(&self, subject: &str) -> bool {
        self.s.mailbox.messages.values().any(|m| m.subject == subject)
    }

    fn folder(&self, kind: FolderKind) -> Id {
        self.s.mailbox.folder_of_kind(self.account, kind).unwrap()
    }

    /// The stand-in's message with this subject (the first, when it has copies).
    fn there(&self, subject: &str) -> Option<mailcraft_net::graph::standin::Message> {
        self.server.state().mailbox.messages.values().find(|m| m.subject == subject).cloned()
    }

    /// How many requests the stand-in has had so far.
    fn asked(&self) -> usize {
        self.server.state().requests.len()
    }

    /// The changes of messages among the requests since `from`: `PATCH`, `DELETE`, moves, copies.
    fn changes_since(&self, from: usize) -> Vec<(String, String)> {
        let state = self.server.state();
        state.requests[from..]
            .iter()
            .filter(|r| r.target.starts_with("/v1.0/me/messages/"))
            .filter(|r| matches!(r.method.as_str(), "PATCH" | "DELETE") || r.target.ends_with("/move") || r.target.ends_with("/copy"))
            .map(|r| (r.method.clone(), r.target.clone()))
            .collect()
    }

    fn errors(answers: &[Value]) -> Vec<Value> {
        answers.iter().flat_map(|a| a["errors"].as_array().cloned().unwrap_or_default()).collect()
    }
}

#[test]
fn every_kind_of_change_made_here_reaches_the_server_once() {
    let mut rig = Rig::new("each-kind@example.org");
    let (inbox, archive, deleted) = {
        let mut state = rig.server.state();
        let (inbox, archive, deleted) = (state.well_known("inbox"), state.well_known("archive"), state.well_known("deleteditems"));
        for (n, subject) in ["Read me", "Flag me", "Tag me", "Raise me", "File me", "Bin me", "Gone for good", "Copy me"].iter().enumerate() {
            state.mailbox.add(&inbox, subject, NOW - 3_600 - n as i64 * 60);
        }
        (inbox, archive, deleted)
    };
    rig.run();
    assert_eq!(rig.s.mailbox.messages.len(), 8);
    let archive_here = rig.folder(FolderKind::Archive);
    let id = |rig: &Rig, subject: &str| rig.here(subject);
    rig.act("mail.markRead", "Read me", json!({}));
    rig.act("mail.flag", "Flag me", json!({"due": "2027-01-20"}));
    rig.act("mail.categorize", "Tag me", json!({"category": "Blue category"}));
    let raise = id(&rig, "Raise me");
    rig.s.mailbox.message_mut(raise).unwrap().importance = Importance::High;
    rig.act("mail.move", "File me", json!({"folder": archive_here}));
    rig.act("mail.delete", "Bin me", json!({}));
    rig.act("mail.deletePermanently", "Gone for good", json!({}));
    let copy = rig.act("mail.copy", "Copy me", json!({"folder": archive_here}))["copies"][0].as_u64().unwrap();
    assert_eq!(unpushed(&rig.s, rig.account), 8);

    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    {
        let there = |subject: &str| rig.there(subject).unwrap();
        assert!(there("Read me").read);
        let flagged = there("Flag me");
        assert_eq!(flagged.flag, "flagged");
        // Due at the end of the working day it was given; started the day it was flagged.
        let due = rig.s.mailbox.message(id(&rig, "Flag me")).unwrap().flag_due.unwrap();
        assert_eq!((flagged.flag_due, flagged.flag_start), (Some(due), Some(NOW - NOW % 86_400)));
        assert_eq!(there("Tag me").categories, ["Blue category"]);
        assert_eq!(there("Raise me").importance, "high");
        assert_eq!(there("File me").folder, archive);
        assert_eq!(there("Bin me").folder, deleted, "Delete: into Deleted Items");
        assert!(rig.there("Gone for good").is_none(), "deleted for good");
        let state = rig.server.state();
        let copies: Vec<_> = state.mailbox.messages.values().filter(|m| m.subject == "Copy me").collect();
        assert_eq!(copies.len(), 2);
        let made = copies.iter().find(|m| m.folder == archive).unwrap();
        assert!(copies.iter().any(|m| m.folder == inbox));
        // The copy here is the server's copy: it doesn't come down a second time.
        assert_eq!(rig.s.mailbox.message(copy).unwrap().server.as_ref().map(|s| s.id.as_str()), Some(made.id.as_str()));
    }
    // Nothing goes again, and nothing came twice.
    let from = rig.asked();
    rig.run();
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Copy me").count(), 2);
    assert_eq!(rig.s.mailbox.messages.len(), 8);
    assert_eq!(rig.s.mailbox.message(id(&rig, "Bin me")).unwrap().folder, rig.folder(FolderKind::Deleted));
    // A flag marked complete goes with when it was.
    rig.act("mail.complete", "Flag me", json!({}));
    rig.run();
    let flagged = rig.there("Flag me").unwrap();
    assert_eq!((flagged.flag.as_str(), flagged.flag_completed), ("complete", Some(NOW)));
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

#[test]
fn a_refused_change_is_put_back_and_said_once() {
    let mut rig = Rig::new("refused@example.org");
    let ids = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        ["Invoice 42", "Gone 1", "Gone 2", "Gone 3", "Not yours", "Odd flag"].map(|subject| state.mailbox.add(&inbox, subject, NOW - 3_600))
    };
    rig.run();
    {
        let mut state = rig.server.state();
        for id in &ids[..4] {
            state.mailbox.faults.insert(id.clone(), Fault::Gone);
        }
        state.mailbox.faults.insert(ids[4].clone(), Fault::Denied);
        state.mailbox.faults.insert(ids[5].clone(), Fault::Invalid);
    }
    let archive = rig.folder(FolderKind::Archive);
    rig.s.run("mail.move", &json!({"ids": [rig.here("Invoice 42")], "folder": archive})).unwrap();
    rig.s.run("mail.markRead", &json!({"ids": [rig.here("Gone 1"), rig.here("Gone 2"), rig.here("Gone 3")]})).unwrap();
    let denied = rig.here("Not yours");
    rig.s.mailbox.message_mut(denied).unwrap().importance = Importance::High;
    let odd = rig.here("Odd flag");
    rig.s.run("mail.flag", &json!({"ids": [odd]})).unwrap();

    let answers = rig.run();
    let status = rig.s.sync_status.clone();
    for said in [
        "Couldn't move 'Invoice 42' to Archive: the server says it no longer exists.",
        "Couldn't mark 3 messages as read: the server says they no longer exist.",
        "Couldn't change the importance of 'Not yours': the server says this account may not change it.",
        "Couldn't change the flag of 'Odd flag': the server says it can't make that change.",
    ] {
        assert!(status.contains(said), "{said} in {status}");
        assert_eq!(Rig::errors(&answers).iter().filter(|e| e.as_str() == Some(said)).count(), 1, "said once: {said}");
    }
    // Put back to how the server has it; what the server no longer has leaves here too.
    assert_eq!(rig.s.mailbox.message(denied).unwrap().importance, Importance::Normal);
    assert_eq!(rig.s.mailbox.message(odd).unwrap().flag, FlagStatus::None);
    assert!(!rig.has("Invoice 42") && !rig.has("Gone 1"), "gone from the server, gone here");
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    // The next run says nothing of it and sends nothing.
    let from = rig.asked();
    rig.run();
    assert!(!rig.s.sync_status.contains("Couldn't"), "{}", rig.s.sync_status);
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());
}

#[test]
fn a_refused_delete_for_good_is_put_back_at_once_with_what_was_fetched() {
    let mut rig = Rig::new("refused-delete@example.org");
    let (invoice, gone) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        (state.mailbox.add(&inbox, "Invoice 42", NOW - 3_600), state.mailbox.add(&inbox, "Gone already", NOW - 7_200))
    };
    rig.run();
    rig.fetch();
    let m = rig.here("Invoice 42");
    let whole = rig.s.mailbox.message(m).unwrap().clone();
    assert!(!whole.unfetched);
    {
        let mut state = rig.server.state();
        state.mailbox.faults.insert(invoice.clone(), Fault::Denied);
        state.mailbox.faults.insert(gone.clone(), Fault::Gone);
    }
    rig.act("mail.markRead", "Invoice 42", json!({}));
    rig.s.run("mail.deletePermanently", &json!({"ids": [m, rig.here("Gone already")]})).unwrap();
    assert!(!rig.has("Invoice 42"));
    let from = rig.asked();
    let answers = rig.run();
    let said = "Couldn't delete 'Invoice 42' for good: the server doesn't allow it.";
    assert_eq!(Rig::errors(&answers).iter().filter(|e| e.as_str() == Some(said)).count(), 1, "said once: {answers:?}");
    assert!(rig.s.sync_status.contains(said), "{}", rig.s.sync_status);
    // Back at once, as the server has it (unread, in the Inbox), with all that was fetched.
    let back = rig.s.mailbox.message(rig.here("Invoice 42")).unwrap();
    assert_eq!((back.folder, back.read, back.unfetched), (rig.folder(FolderKind::Inbox), false, false));
    assert_eq!((&back.body, &back.attachments), (&whole.body, &whole.attachments));
    // The one the server no longer has stays gone, unsaid.
    assert!(!rig.has("Gone already"));
    assert!(!rig.s.sync_status.contains("Gone already"));
    let deletes = rig.changes_since(from).iter().filter(|(method, _)| method == "DELETE").count();
    assert_eq!(deletes, 2);
    assert!(rig.s.mailbox.queued.is_empty());
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    // The next run sends nothing, says nothing, and brings nothing twice.
    let from = rig.asked();
    rig.run();
    assert_eq!(rig.changes_since(from), Vec::<(String, String)>::new());
    assert!(!rig.s.sync_status.contains("Couldn't"), "{}", rig.s.sync_status);
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Invoice 42").count(), 1);
}

#[test]
fn emptying_deleted_items_puts_back_what_the_server_refuses_and_a_busy_server_waits() {
    let mut rig = Rig::new("empty-deleted@example.org");
    let ids = {
        let mut state = rig.server.state();
        let deleted = state.well_known("deleteditems");
        ["Old 1", "Not allowed", "Odd one", "Busy", "Old 2"].map(|subject| state.mailbox.add(&deleted, subject, NOW - 3_600))
    };
    rig.run();
    {
        let mut state = rig.server.state();
        state.retry_after = Some(1);
        state.mailbox.faults.insert(ids[1].clone(), Fault::Denied);
        state.mailbox.faults.insert(ids[2].clone(), Fault::Invalid);
        // Busy for every try of this run; taken in the next.
        state.mailbox.faults.insert(ids[3].clone(), Fault::Throttled(4));
    }
    let deleted = rig.folder(FolderKind::Deleted);
    rig.s.run("folder.empty", &json!({"id": deleted})).unwrap();
    assert_eq!(rig.s.mailbox.messages_in(deleted).count(), 0);
    let from = rig.asked();
    let answers = rig.run();
    let errors = Rig::errors(&answers);
    for said in [
        "Couldn't delete 'Not allowed' for good: the server doesn't allow it.",
        "Couldn't delete 'Odd one' for good: the server says it can't make that change.",
    ] {
        assert_eq!(errors.iter().filter(|e| e.as_str() == Some(said)).count(), 1, "said once: {said} in {errors:?}");
    }
    // The refused are back in Deleted Items; the busy one waits, deleted here.
    let mut back: Vec<String> = rig.s.mailbox.messages_in(deleted).map(|m| m.subject.clone()).collect();
    back.sort();
    assert_eq!(back, ["Not allowed", "Odd one"]);
    assert!(rig.there("Busy").is_some() && rig.there("Old 1").is_none() && rig.there("Old 2").is_none());
    let deletes = |rig: &Rig, from: usize, id: &str| {
        rig.server.state().requests[from..].iter().filter(|r| r.method == "DELETE" && r.path.last().is_some_and(|p| p == id)).count()
    };
    assert_eq!(deletes(&rig, from, &ids[1]), 1);
    assert_eq!(unpushed(&rig.s, rig.account), 1, "the busy one is still to go");
    // The next run: the busy one goes, the refused ones aren't asked again, nothing comes back twice.
    let from = rig.asked();
    rig.run();
    assert!(rig.there("Busy").is_none());
    assert_eq!(deletes(&rig, from, &ids[3]), 1);
    assert_eq!(deletes(&rig, from, &ids[1]) + deletes(&rig, from, &ids[2]), 0);
    assert!(!rig.has("Busy"));
    assert_eq!(rig.s.mailbox.messages_in(deleted).count(), 2);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

#[test]
fn a_change_here_and_one_on_the_server_end_as_s17_says() {
    let mut rig = Rig::new("both-sides@example.org");
    let (both, twice, projects) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        let projects = state.add_folder("Projects", None);
        let both = state.mailbox.add(&inbox, "Both", NOW - 3_600);
        state.mailbox.update(&both, |m| m.categories = vec!["Red category".into()]);
        (both, state.mailbox.add(&inbox, "Moved twice", NOW - 7_200), projects)
    };
    rig.run();
    // Here: read, a category added; moved to the Archive.
    let here = rig.here("Both");
    rig.s.run("mail.markRead", &json!({"ids": [here]})).unwrap();
    rig.s.run("mail.categorize", &json!({"ids": [here], "category": "Blue category"})).unwrap();
    let archive = rig.folder(FolderKind::Archive);
    rig.s.run("mail.move", &json!({"ids": [rig.here("Moved twice")], "folder": archive})).unwrap();
    // Meanwhile in Outlook: flagged, made important, another category; moved to Projects.
    {
        let mut state = rig.server.state();
        state.mailbox.update(&both, |m| {
            m.flag = "flagged".into();
            (m.flag_start, m.flag_due) = (Some(NOW), Some(NOW + 86_400));
            m.importance = "high".into();
            m.categories.push("Green category".into());
        });
        state.mailbox.move_to(&twice, &projects);
    }
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new());
    let there = rig.there("Both").unwrap();
    // What was changed here wins for what it touched; the server's for the rest; categories merge.
    assert!(there.read);
    assert_eq!((there.flag.as_str(), there.importance.as_str()), ("flagged", "high"));
    assert_eq!(there.categories, ["Red category", "Green category", "Blue category"]);
    let m = rig.s.mailbox.message(here).unwrap();
    assert!(m.read && m.flag == FlagStatus::Flagged && m.importance == Importance::High);
    // The flag comes down with its dates.
    assert_eq!(m.flag_due, Some(NOW + 86_400));
    assert_eq!(m.server.as_ref().map(|s| (s.flag_start, s.flag_due)), Some((Some(NOW), Some(NOW + 86_400))));

    assert_eq!(m.categories, ["Red category", "Green category", "Blue category"]);
    // Moved on both sides: where it was moved here.
    let archive_there = rig.server.state().well_known("archive");
    assert_eq!(rig.there("Moved twice").unwrap().folder, archive_there);
    assert_eq!(rig.s.mailbox.message(rig.here("Moved twice")).unwrap().folder, archive);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

#[test]
fn ten_thousand_marked_read_take_no_more_than_500_batches() {
    let mut rig = Rig::new("ten-thousand@example.org");
    {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        state.mailbox.generate(&inbox, 10_000, NOW);
    }
    rig.run();
    assert_eq!(rig.s.mailbox.messages.len(), 10_000);
    let inbox = rig.folder(FolderKind::Inbox);
    rig.s.run("mail.markAllRead", &json!({"folder": inbox})).unwrap();
    assert_eq!(unpushed(&rig.s, rig.account), 10_000);
    let from = rig.asked();
    rig.run();
    let state = rig.server.state();
    let since = &state.requests[from..];
    let batches = since.iter().filter(|r| r.method == "POST" && r.target == "/v1.0/$batch").count();
    let patches = since.iter().filter(|r| r.method == "PATCH").count();
    // One batch is the run's question of which folder is which; the rest are the changes.
    assert!(batches <= 501, "{batches} batches");
    assert_eq!(patches, 10_000);
    assert!(state.mailbox.messages.values().all(|m| m.read));
    drop(state);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

#[test]
fn a_batch_answered_partly_done_refused_and_throttled() {
    let mut rig = Rig::new("three-ways@example.org");
    let ids = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        state.mailbox.generate(&inbox, 6, NOW)
    };
    rig.run();
    {
        let mut state = rig.server.state();
        state.retry_after = Some(1);
        state.mailbox.faults.insert(ids[1].clone(), Fault::Gone);
        state.mailbox.faults.insert(ids[2].clone(), Fault::Throttled(1));
        state.mailbox.faults.insert(ids[3].clone(), Fault::Denied);
    }
    let all: Vec<Id> = rig.s.mailbox.messages.keys().copied().collect();
    rig.s.run("mail.markRead", &json!({"ids": all})).unwrap();
    let from = rig.asked();
    let started = std::time::Instant::now();
    rig.run();
    assert!(started.elapsed() >= std::time::Duration::from_millis(950), "the server's second was waited out");
    let state = rig.server.state();
    let read: Vec<bool> = ids.iter().map(|id| state.mailbox.messages[id].read).collect();
    assert_eq!(read, [true, false, true, false, true, true], "the throttled one was asked again and made");
    // Nothing was sent twice but the one the server didn't take the first time.
    for (n, id) in ids.iter().enumerate() {
        let sent = state.requests[from..].iter().filter(|r| r.method == "PATCH" && r.path.last() == Some(id)).count();
        assert_eq!(sent, if n == 2 { 2 } else { 1 }, "message {n}");
    }
    drop(state);
    // The refused are put back (the one the server no longer has is gone here too).
    let here = |rig: &Rig, id: &str| rig.s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).map(|m| m.read);
    assert_eq!((here(&rig, &ids[1]), here(&rig, &ids[3]), here(&rig, &ids[2])), (None, Some(false), Some(true)));
    assert!(
        rig.s.sync_status.contains("Couldn't mark 'Message 3' as read: the server says this account may not change it."),
        "{}",
        rig.s.sync_status
    );
}

#[test]
fn a_copy_whose_answer_was_lost_is_found_by_its_marker_and_not_made_twice() {
    let mut rig = Rig::new("lost-copy@example.org");
    let (source, archive) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        (state.mailbox.add(&inbox, "Copy me", NOW - 60), state.well_known("archive"))
    };
    rig.run();
    rig.server.state().mailbox.faults.insert(source.clone(), Fault::CopyAnswerLost);
    let archive_here = rig.folder(FolderKind::Archive);
    let copy = rig.s.run("mail.copy", &json!({"ids": [rig.here("Copy me")], "folder": archive_here})).unwrap()["copies"][0].as_u64().unwrap();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new());
    let made: Vec<String> = {
        let state = rig.server.state();
        state.mailbox.messages.values().filter(|m| m.folder == archive).map(|m| m.id.clone()).collect()
    };
    assert_eq!(made.len(), 1, "made once");
    assert!(rig.server.state().mailbox.faults.is_empty(), "the copy's answer was lost");
    let copies = rig.server.state().requests.iter().filter(|r| r.target.ends_with("/copy")).count();
    assert_eq!(copies, 1, "asked for once");
    assert_eq!(rig.s.mailbox.message(copy).unwrap().server.as_ref().map(|s| s.id.clone()), made.first().cloned());
    assert!(rig.s.mailbox.queued.is_empty());
    // Asked again in a later run (as after a crash before the answer was taken in): found, not made.
    {
        let m = rig.s.mailbox.message_mut(copy).unwrap();
        m.server = None;
    }
    let marker = "mailcraft-0-1-2";
    {
        let mut state = rig.server.state();
        let made = made[0].clone();
        state.mailbox.update(&made, |m| m.extended = vec![(mailcraft_net_marker().into(), marker.into())]);
    }
    rig.s.mailbox.queued.push(mailcraft_model::Queued::CopyMessage {
        account: rig.account,
        id: source.clone(),
        to: archive.clone(),
        copy,
        marker: marker.into(),
    });
    rig.run();
    assert_eq!(rig.server.state().mailbox.messages.values().filter(|m| m.folder == archive).count(), 1);
    assert_eq!(rig.s.mailbox.message(copy).unwrap().server.as_ref().map(|s| s.id.clone()), made.first().cloned());
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Copy me").count(), 2);
}

#[test]
fn a_move_whose_answer_was_lost_is_looked_for_before_it_is_asked_again() {
    let mut rig = Rig::new("lost-move@example.org");
    let (moved, archive) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        (state.mailbox.add(&inbox, "File me", NOW - 60), state.well_known("archive"))
    };
    rig.run();
    rig.server.state().mailbox.faults.insert(moved.clone(), Fault::AnswerLost);
    let archive_here = rig.folder(FolderKind::Archive);
    rig.act("mail.move", "File me", json!({"folder": archive_here}));
    let from = rig.asked();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new());
    assert!(rig.server.state().mailbox.faults.is_empty(), "the move's answer was lost");
    assert_eq!(rig.there("File me").unwrap().folder, archive);
    let state = rig.server.state();
    let since = &state.requests[from..];
    assert_eq!(since.iter().filter(|r| r.target.ends_with("/move")).count(), 1, "moved once");
    assert!(since.iter().any(|r| r.method == "GET" && r.path.last() == Some(&moved)), "looked for by its id");
    drop(state);
    assert_eq!(rig.s.mailbox.message(rig.here("File me")).unwrap().server.as_ref().map(|s| s.folder.clone()), Some(archive));
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

/// MailCraft's marker as the stand-in sees it written.
fn mailcraft_net_marker() -> &'static str {
    "String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Name MailCraftMarker"
}

#[test]
fn a_move_into_a_local_folder_leaves_the_servers_copy_until_the_whole_message_is_saved() {
    let mut rig = Rig::new("moving-out@example.org");
    let (keep, lose) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        (state.mailbox.add(&inbox, "Keep me", NOW - 60), state.mailbox.add(&inbox, "Lose me", NOW - 120))
    };
    rig.run();
    let local = rig.s.mailbox.add_account(mailcraft_model::Account { name: "On this computer".into(), ..Default::default() });
    let kept = rig.s.mailbox.add_folder(local, None, "Kept").unwrap();
    let m = rig.here("Keep me");
    assert!(rig.s.mailbox.message(m).unwrap().unfetched);
    // Offline: moved, and shown there at once, being moved.
    rig.s.run("mail.move", &json!({"ids": [m, rig.here("Lose me")], "folder": kept})).unwrap();
    let moved = rig.s.mailbox.message(m).unwrap();
    assert_eq!((moved.folder, moved.moving_out.as_ref().map(|out| out.id.as_str())), (kept, Some(keep.as_str())));
    {
        let request = graph_mail_request(&rig.s, rig.account).unwrap();
        let _offline = mailcraft_net::graph::standin::route("http://127.0.0.1:9");
        let cancel = AtomicBool::new(false);
        let s = &mut rig.s;
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| {
            apply_graph_mail(s, piece).unwrap();
        });
    }
    assert!(rig.there("Keep me").is_some());
    // Online, not fetched: the server keeps it, and it doesn't come down again.
    rig.server.state().mailbox.update(&keep, |m| m.read = true);
    rig.run();
    assert!(rig.there("Keep me").is_some());
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Keep me").count(), 1);
    // Gone from the server before it could be fetched: kept as it is, incomplete.
    rig.server.state().mailbox.delete(&lose);
    // Fetched whole, not saved: the server still keeps it.
    rig.fetch();
    let whole = rig.s.mailbox.message(m).unwrap();
    assert!(!whole.unfetched && whole.moving_out.is_some());
    let lost = rig.s.mailbox.message(rig.here("Lose me")).unwrap();
    assert!(lost.incomplete && lost.moving_out.is_none() && lost.folder == kept);
    rig.run();
    assert!(rig.there("Keep me").is_some(), "not before the mailbox file holding it is saved");
    // Saved: then it goes.
    let dir = std::env::temp_dir().join(format!("mailcraft-r332-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("moving-out.mailcraft");
    rig.s.run("file.save", &json!({"path": path.to_string_lossy()})).unwrap();
    rig.run();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(rig.there("Keep me").is_none(), "removed from the server once saved here whole");
    let done = rig.s.mailbox.message(m).unwrap();
    assert!(done.moving_out.is_none() && done.folder == kept && !done.unfetched);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
}

/// #451: mail in a folder on this computer is moved and copied into a Microsoft account by the
/// account's own run, made there as received mail from the whole message; a draft stays out.
#[test]
fn local_mail_moved_and_copied_into_a_microsoft_account_is_made_on_its_server() {
    let mut rig = Rig::new("into@example.org");
    rig.run();
    let local = rig.s.mailbox.add_account(mailcraft_model::Account { name: "On this computer".into(), ..Default::default() });
    let kept = rig.s.mailbox.add_folder(local, None, "Kept").unwrap();
    let add = |rig: &mut Rig, subject: &str, draft: bool, read: bool| {
        let m = mailcraft_model::Message {
            folder: kept,
            subject: subject.into(),
            message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
            from: mailcraft_model::Address::new("Bob", "bob@example.org"),
            draft,
            read,
            date: NOW - 7_200,
            received: NOW - 7_000,
            ..Default::default()
        };
        rig.s.mailbox.add_message(m).unwrap()
    };
    let draft = add(&mut rig, "Local draft", true, false);
    let moved = add(&mut rig, "Moved in", false, true);
    let copied = add(&mut rig, "Copied in", false, false);
    let inbox = rig.folder(FolderKind::Inbox);
    for command in ["mail.move", "mail.copy"] {
        let refused = rig.s.run(command, &json!({"ids": [draft], "folder": inbox})).unwrap_err().to_string();
        assert!(refused.contains("stays in its own mailbox"), "{refused}");
    }
    assert_eq!(rig.s.mailbox.message(draft).unwrap().folder, kept);
    rig.s.run("mail.move", &json!({"ids": [moved], "folder": inbox})).unwrap();
    let copy = rig.s.run("mail.copy", &json!({"ids": [copied], "folder": inbox})).unwrap()["copies"][0].as_u64().unwrap();
    assert_eq!(unpushed(&rig.s, rig.account), 2);

    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    let well_inbox = rig.server.state().well_known("inbox");
    for (subject, here, read) in [("Moved in", moved, true), ("Copied in", copy, false)] {
        let there = rig.there(subject).unwrap_or_else(|| panic!("{subject} on the server"));
        // Received mail, not a draft, with its read state and its Message-ID.
        assert_eq!((there.folder.as_str(), there.draft, there.read), (well_inbox.as_str(), false, read), "{subject}");
        assert_eq!(there.internet_id, format!("<{}@example.org>", subject.to_lowercase().replace(' ', "-")));
        let m = rig.s.mailbox.message(here).unwrap();
        assert_eq!(m.server.as_ref().map(|s| s.id.as_str()), Some(there.id.as_str()), "{subject}");
    }
    // The original of the copy stays where it was, and the moved one isn't here twice.
    assert_eq!(rig.s.mailbox.message(copied).unwrap().folder, kept);
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Moved in").count(), 1);
    rig.run();
    assert_eq!(rig.s.mailbox.messages.values().filter(|m| m.subject == "Moved in").count(), 1);
    assert_eq!(rig.server.state().mailbox.messages.values().filter(|m| m.subject == "Moved in").count(), 1);
}

/// Graph's delete past the recoverable items is used nowhere (R3 spec S19). The name is spelled in
/// pieces here so that this file isn't a place it is written.
#[test]
fn graphs_delete_past_the_retention_period_is_nowhere_in_the_code() {
    let name = ["perm", "anent", "delete"].concat();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut stack = vec![root.join("crates"), root.join("apps")];
    let mut looked = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default().to_lowercase();
                assert!(!text.contains(&name), "{} names it", path.display());
                looked += 1;
            }
        }
    }
    assert!(looked > 100, "{looked} files looked at");
}

/// Microsoft 365 answers a flag with a start and no due date 400 (and takes the same with the
/// due date at the start): a plain Follow Up goes with both, and stays in step afterwards.
#[test]
fn a_flag_without_a_due_date_goes_up_due_at_its_start() {
    let mut rig = Rig::new("plain-flag@example.org");
    {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        state.mailbox.add(&inbox, "Flag me plainly", NOW - 3_600);
    }
    rig.run();
    rig.act("mail.flag", "Flag me plainly", json!({}));
    let from = rig.asked();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    let there = rig.there("Flag me plainly").unwrap();
    assert_eq!(there.flag, "flagged");
    let start = NOW - NOW % 86_400;
    assert_eq!((there.flag_start, there.flag_due), (Some(start), Some(start)));
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    let flags_since = |rig: &Rig, from: usize| -> Vec<Value> {
        let state = rig.server.state();
        state.requests[from..].iter().filter(|r| r.method == "PATCH").filter_map(|r| r.json().get("flag").cloned()).collect()
    };
    let flags = flags_since(&rig, from);
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert_eq!(flags[0]["startDateTime"], flags[0]["dueDateTime"], "{flags:?}");
    // The due date comes back down, and nothing goes up again.
    let from = rig.asked();
    rig.run();
    rig.run();
    assert_eq!(flags_since(&rig, from), Vec::<Value>::new());
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    assert_eq!(rig.s.mailbox.message(rig.here("Flag me plainly")).unwrap().flag_due, Some(start));
}
