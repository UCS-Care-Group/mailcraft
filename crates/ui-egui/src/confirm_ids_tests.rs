// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A question about messages keeps the messages it asked about (#620): the selection may change
//! while it is open (an agent shares the window), and Delete for good deletes what was named.

use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

fn some_messages(a: &MailApp, n: usize) -> Vec<u64> {
    let mut ids: Vec<u64> = a.session.mailbox.messages.values().filter(|m| !m.draft && !m.incomplete && !m.unfetched).map(|m| m.id).collect();
    ids.sort_unstable();
    ids.truncate(n);
    ids
}

#[test]
fn delete_for_good_deletes_what_the_question_named_after_the_selection_changes() {
    let mut a = app();
    let ids = some_messages(&a, 5);
    let (asked, others) = ids.split_at(1);
    a.session.sel.messages = asked.to_vec();
    assert_eq!(a.run("mail.deletePermanently", json!({})).unwrap()["pending"], "confirm");
    let Some(Dialog::Confirm { command, params, ask }) = a.dialog.take() else { panic!("the owner is asked first") };
    assert!(ask.text.contains("message"), "{}", ask.text);
    // While the question is up, the selection becomes four other messages.
    a.session.sel.messages = others.to_vec();
    let params = crate::dialogs::answered(&params, &ask.with, None);
    crate::dialogs::go_ahead(&mut a, &command, params);
    assert!(a.session.mailbox.message(asked[0]).is_none(), "the message that was asked about is gone");
    for id in others {
        assert!(a.session.mailbox.message(*id).is_some(), "message {id} was never asked about");
    }
}
