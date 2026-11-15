// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-16 from end to end, against the strict Graph stand-in: the background asks for a draft
//! whole while the owner opens it (fetched at once), edits it and sends it; the background's
//! answer comes after. It is dropped, and what the server sends is what the owner wrote, to whom
//! the owner wrote it. Nothing here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::json;

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
const ME: &str = "stale-fetch@example.org";

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        Rig { server, s, account }
    }

    fn run(&mut self) {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let s = &mut self.s;
        mailcraft_net::graph::sync_mail(TOKEN, &request, &AtomicBool::new(false), &mut |piece| {
            apply_graph_mail(s, piece).unwrap();
        });
    }

    /// What the server answers to a fetch of `message`, or of the background's batch.
    fn fetched(&self, message: Option<Id>) -> mailcraft_model::sync::GraphFetchResult {
        let request = graph_fetch_request(&self.s, self.account, message, 10).unwrap();
        let _routed = self.server.route();
        mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false))
    }
}

#[test]
fn a_late_background_fetch_never_sends_what_the_owner_took_out() {
    let mut rig = Rig::new();
    // The folders first, then a draft written here and saved: it goes up with the next run.
    rig.run();
    rig.s
        .run(
            "mail.new",
            &json!({"account": rig.account, "to": "alice@example.org, bob@example.org", "subject": "Plan", "body": "Old text for Bob too."}),
        )
        .unwrap();
    let d = rig.s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    rig.s.run("compose.close", &json!({})).ok();
    rig.run();
    // Touched on another device: to be fetched again.
    {
        let mut state = rig.server.state();
        let drafts = state.well_known("drafts");
        let id = state.mailbox.messages.values().find(|m| m.folder == drafts).map(|m| m.id.clone()).unwrap();
        assert!(state.mailbox.update(&id, |m| m.body = "Old text for Bob too, seen on the phone.".into()));
    }
    rig.run();
    assert!(rig.s.mailbox.message(d).unwrap().unfetched);
    // The background's batch: asked for, its answer on its way.
    let late = rig.fetched(None);
    assert_eq!(late.fetched.len(), 1);
    // Opened: fetched at once, and taken in first.
    let now = rig.fetched(Some(d));
    assert_eq!(apply_graph_fetch(&mut rig.s, now).unwrap()["fetched"], 1);
    // Edited and sent.
    rig.s.run("mail.editDraft", &json!({"id": d})).unwrap();
    rig.s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
    rig.s.run("compose.send", &json!({})).unwrap();
    let outbox = rig.s.mailbox.folder_of_kind(rig.account, FolderKind::Outbox).unwrap();
    assert_eq!(rig.s.mailbox.messages_in(outbox).count(), 1);
    // The background's answer: dropped.
    assert_eq!(apply_graph_fetch(&mut rig.s, late).unwrap()["fetched"], 0);
    let queued = rig.s.mailbox.messages_in(outbox).next().cloned().unwrap();
    assert_eq!(queued.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["alice@example.org"]);
    // Sent as the owner left it.
    rig.run();
    let state = rig.server.state();
    let sent: Vec<_> = state.sent.iter().filter(|m| m.subject == "Plan").collect();
    assert_eq!(sent.len(), 1, "{:?}", state.sent);
    assert_eq!(sent[0].to, ["alice@example.org".to_string()]);
    let copy = state.mailbox.messages.values().find(|m| m.subject == "Plan" && m.folder == state.well_known("sentitems")).unwrap();
    assert!(copy.body.contains("Only for Alice.") && !copy.body.contains("Bob"), "{}", copy.body);
}
