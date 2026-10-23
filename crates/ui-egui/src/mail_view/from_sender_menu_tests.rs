// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #301: Messages from Sender in a message's context menu is that message's sender's mail.

use egui::PointerButton;
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use crate::people_view::testing;

/// Two messages of two senders selected, the other one in focus: a right click on the first and
/// Messages from Sender look for the first's sender, not the focused one's.
#[test]
fn the_menu_looks_for_the_sender_of_the_message_clicked() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.run_steps(3);
    let rows = w.state_mut().session.rows();
    let sender = |w: &testing::Window, id| w.state().session.mailbox.message(id).map(|m| m.from.email.trim().to_string()).unwrap();
    let clicked = rows.first().map(|r| r.id).unwrap();
    let focused = rows.iter().map(|r| r.id).find(|id| !sender(&w, *id).eq_ignore_ascii_case(&sender(&w, clicked))).unwrap();
    w.state_mut().execute("mail.select", json!({"id": clicked})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": focused, "extend": true})).unwrap();
    w.run_steps(3);
    assert_eq!(w.state().session.sel.focus(), Some(focused));
    assert!(w.state().session.sel.messages.contains(&clicked));

    let pos = w.state().hit.rect("message", clicked).unwrap().center();
    testing::click(&mut w, pos, PointerButton::Secondary, 1);
    w.get_by_label_contains("Messages from Sender").click();
    w.run_steps(3);
    let want = format!("from:{}", mailcraft_model::search::keyword_value(&sender(&w, clicked)));
    assert_eq!(w.state().session.view.search, want);
}
