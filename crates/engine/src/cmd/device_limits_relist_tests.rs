// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Device limits (MB3.16): what listing afresh does to folders and to messages outside the window.

use mailcraft_model::device_limits::PHONE_STORAGE;
use serde_json::json;

use super::device_limits_tests::{add, graph_session};

#[test]
fn taking_the_limits_away_lists_the_folders_afresh_and_never_before_they_were_set() {
    let (mut s, _, inbox) = graph_session();
    let relisting = |s: &crate::Session| s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().relist;
    s.run("account.deviceLimits", &json!({"off": true})).unwrap();
    assert!(!relisting(&s), "there was no window to widen");
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    assert!(!relisting(&s));
    s.run("account.deviceLimits", &json!({"off": true})).unwrap();
    assert!(relisting(&s), "the saved delta link keeps the window's filter");
}

#[test]
fn a_message_outside_the_window_with_a_change_not_sent_stays_when_a_listing_afresh_ends() {
    let (mut s, account, inbox) = graph_session();
    let changed = add(&mut s, account, inbox, 60, 10);
    let plain = add(&mut s, account, inbox, 60, 10);
    s.mailbox.message_mut(changed).unwrap().read = true; // the server has it unread
    for id in [changed, plain] {
        let m = std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&id).unwrap());
        m.server.as_mut().unwrap().gone = true; // the listing afresh did not return it
    }
    crate::cmd::graph_down::sweep(&mut s.mailbox, account, &mut Default::default());
    assert!(s.mailbox.message(plain).is_none());
    let kept = s.mailbox.message(changed).expect("the change is not lost");
    assert!(kept.read && kept.server.as_ref().is_some_and(|server| !server.gone));
    assert_eq!(crate::cmd::graph_up::message_changes(&s, account, 100).0.len(), 1, "and it still goes up");
}
