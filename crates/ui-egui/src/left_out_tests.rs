// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1581: File › Info says, for as long as it is so, how many messages of an account were left
//! out for failing to download, and offers to try them again.

use mailcraft_engine::Session;
use mailcraft_model::sync::MAX_TRIES;
use mailcraft_model::{Folder, FolderKind, Id};
use serde_json::json;

use crate::{MailApp, Services};

fn window() -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), 1_791_993_600, 0), Services::default());
    app.autosave = false;
    let incoming = json!({"host": "imap.mail.example", "security": "tls"});
    let account = app.execute("account.add", json!({"email": "noor@example.org", "incoming": incoming})).unwrap()["id"].as_u64().unwrap();
    let failing = [(4, MAX_TRIES), (5, 1), (6, MAX_TRIES)].into();
    app.session.mailbox.folders.push(Folder { id: 9000, account, name: "INBOX".into(), kind: FolderKind::Inbox, failing, ..Default::default() });
    (app, account)
}

#[test]
fn the_account_says_how_many_messages_were_left_out_and_offers_to_try_again() {
    let (mut app, account) = window();
    assert_eq!(app.left_out_line(account).as_deref(), Some("2 messages were left out because they failed to download."));
    // One of them only: said in the singular.
    app.session.mailbox.folder_mut(9000).unwrap().failing.remove(&6);
    assert_eq!(app.left_out_line(account).as_deref(), Some("1 message was left out because it failed to download."));
    // Another account has none of it.
    assert_eq!(app.left_out_line(account + 1), None);
    app.retry_left_out(account);
    assert_eq!(app.left_out_line(account), None);
    assert!(app.session.mailbox.folder_mut(9000).unwrap().failing.values().all(|tries| *tries == 0));
}
