// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A draft begun on another device, opened here, edited and sent, from end to end against the
//! strict Graph stand-in: the next run sends it, to whom the owner sent it, as the owner wrote
//! it. Nothing here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::json;

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
const ME: &str = "draft-elsewhere@example.org";

/// One Send/Receive of `account` against `server`, every piece taken in as it comes.
fn send_receive(server: &StandIn, s: &mut Session, account: Id) {
    let asked = graph_mail_request(s, account).unwrap();
    let _routed = server.route();
    let cancel = AtomicBool::new(false);
    mailcraft_net::graph::sync_mail(TOKEN, &asked, &cancel, &mut |piece| {
        let _taken = apply_graph_mail(s, piece).unwrap();
    });
}

#[test]
fn a_draft_begun_elsewhere_and_sent_here_is_sent() {
    let server = StandIn::start().unwrap();
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    {
        let mut state = server.state();
        let drafts = state.well_known("drafts");
        let id = state.mailbox.add(&drafts, "Plan", NOW - 3_600);
        state.mailbox.update(&id, |m| {
            m.draft = true;
            m.from = ME.into();
            m.to = vec!["alice@example.org".into(), "bob@example.org".into()];
            m.body = "Begun on the phone.".into();
        });
    }
    send_receive(&server, &mut s, account);
    let d = s.mailbox.messages.values().find(|m| m.subject == "Plan" && m.account == account).map(|m| m.id).unwrap();
    let request = graph_fetch_request(&s, account, Some(d), 10).unwrap();
    let fetched = {
        let _routed = server.route();
        mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false))
    };
    assert_eq!(apply_graph_fetch(&mut s, fetched).unwrap()["fetched"], 1);
    s.run("mail.editDraft", &json!({"id": d})).unwrap();
    s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    send_receive(&server, &mut s, account);
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0, "nothing left in the Outbox");
    let state = server.state();
    let sent: Vec<_> = state.sent.iter().filter(|m| m.subject == "Plan").collect();
    assert_eq!(sent.len(), 1, "{:?}", state.sent);
    assert_eq!(sent[0].to, ["alice@example.org".to_string()]);
    let copy = state.mailbox.messages.values().find(|m| m.subject == "Plan" && m.folder == state.well_known("sentitems")).unwrap();
    assert!(copy.body.contains("Only for Alice."), "{}", copy.body);
}
