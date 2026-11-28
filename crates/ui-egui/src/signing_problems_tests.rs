// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Review of #1036: a message Microsoft's leg of a Send/Receive couldn't Digitally sign is held,
//! and said among the account's problems until a later run; the IMAP leg of the same run, gone
//! through cleanly, doesn't clear it. Hosts are fakes; every address is invented.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::signing::Unsigned;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;

use crate::{GraphResult, MailApp, Services};

const NOW: i64 = 1_791_993_600;

#[test]
fn a_signing_failure_of_the_graph_leg_outlives_a_clean_imap_leg() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let pipe: Arc<Mutex<Option<mpsc::Sender<SyncResult>>>> = Default::default();
    let feed = pipe.clone();
    a.services.sync = Some(Box::new(move |_: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        *feed.lock().unwrap() = Some(tx);
        rx
    }));
    let added = a.execute(
        "account.add",
        json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();
    a.passwords.insert(id, "invented-password".into());
    a.execute("mail.new", json!({"to": "bob@example.org", "subject": "Rota", "account": id})).unwrap();
    let message = a.execute("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    // Microsoft's leg comes back: the message couldn't be signed.
    let (tx, rx) = mpsc::channel();
    a.graph_rx = Some((id, false, false, a.session.openings(), rx));
    let unsigned = vec![Unsigned { message, why: "the smartcard isn't there".into() }];
    tx.send(GraphResult { account: id, unsigned, ..Default::default() }).unwrap();
    a.poll_graph();
    assert!(!a.session.mailbox.message(message).unwrap().held.is_empty(), "held");
    // The IMAP leg goes through without a problem.
    let leg = pipe.lock().unwrap().take().expect("the IMAP leg started");
    leg.send(SyncResult { account: id, partial: false, ..Default::default() }).unwrap();
    a.poll_sync();
    let lines = a.sync_problem_lines();
    assert!(lines.iter().any(|l| l.contains("Rota") && l.contains("smartcard")), "{lines:?}");
}
