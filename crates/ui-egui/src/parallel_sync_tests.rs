// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;
use std::{cell::RefCell, rc::Rc, sync::mpsc};

#[test]
fn accounts_start_without_waiting_for_a_large_mailbox_and_cancel_independently() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), crate::Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let runs = Rc::new(RefCell::new(Vec::new()));
    let seen = runs.clone();
    app.services.sync_with_cancel = Some(Box::new(move |request: SyncRequest, cancel| {
        let (tx, rx) = mpsc::channel::<SyncResult>();
        seen.borrow_mut().push((request.account.id, tx, cancel));
        rx
    }));
    let mut ids = Vec::new();
    for name in ["first", "second", "third", "fourth", "fifth"] {
        let id = app
            .execute(
                "account.add",
                json!({"email":format!("{name}@mail.example"),"incoming":{"host":"imap.mail.example"},"outgoing":{"host":"smtp.mail.example"}}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        app.adopt_password(id, "synthetic-password".into());
        ids.push(id);
    }
    app.execute("ui.sync", json!({})).unwrap();
    for _ in 0..10 {
        app.auto_sync();
    }
    assert_eq!(runs.borrow().len(), MAX_MAIL_RUNS, "four accounts start while the first has not answered");
    assert!(app.syncing());
    assert!(app.start_sync(&json!({"account": ids[0]})).is_err(), "no second run for one account");
    // A small account finishes while the first account's channel remains held open.
    runs.borrow()[1].1.send(SyncResult { account: ids[1], ..Default::default() }).unwrap();
    app.poll_parallel_mail();
    assert!(app.parallel_mail.iter().any(|run| run.account == ids[0]));
    assert!(!app.parallel_mail.iter().any(|run| run.account == ids[1]));
    app.auto_sync();
    app.auto_sync();
    assert_eq!(runs.borrow().len(), 5, "the next waiting account uses the freed slot");
    let first_cancel = runs.borrow()[0].2.clone();
    let second_cancel = runs.borrow()[2].2.clone();
    assert!(!Arc::ptr_eq(&first_cancel, &second_cancel));
    first_cancel.store(true, Ordering::Relaxed);
    assert!(!second_cancel.load(Ordering::Relaxed));
    app.execute("ui.cancelSync", json!({})).unwrap();
    assert!(runs.borrow().iter().all(|(_, _, cancel)| cancel.load(Ordering::Relaxed)));
    assert!(app.sync_queue.is_empty());
    app.forget_mail_runs();
    assert!(!app.syncing());
}
