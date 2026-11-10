// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Account Settings › Deleting: whether Delete on a whole conversation asks first, per account
//! (#503).
use mailcraft_engine::Session;

use super::*;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

#[test]
fn the_box_shows_and_saves_whether_deleting_a_conversation_asks() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (a.autosave, a.auto_sync) = (false, false);
    let id = a.session.mailbox.accounts[0].id;
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    assert_eq!(w.ask_conversation, Some((true, true)));
    // Unticked and saved, the account stops asking; the page opens again with it unticked.
    w.ask_conversation = Some((true, false));
    assert!(w.act(&mut a, Action::Go));
    assert!(!a.session.mailbox.asks_before_deleting_conversation(id));
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    assert_eq!(w.ask_conversation, Some((false, false)));
    // Cancel keeps it as it was; ticked and saved, it asks again.
    w.ask_conversation = Some((false, true));
    assert!(w.act(&mut a, Action::Back));
    assert!(!a.session.mailbox.asks_before_deleting_conversation(id));
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    w.ask_conversation = Some((false, true));
    assert!(w.act(&mut a, Action::Go));
    assert!(a.session.mailbox.asks_before_deleting_conversation(id));
}
