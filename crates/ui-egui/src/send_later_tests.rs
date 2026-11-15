// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send later and Undo Send in the window (#950): Delay Delivery and Send Later… ask for a date and
//! a time, a message waiting in the Outbox says when it goes and can be changed or cancelled, the
//! wait of Undo Send shows "Sending in N s" with Undo, and Ctrl+Z while it shows takes the message
//! back.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox with an IMAP account to send from, as of [`NOW`]; the account's id.
fn window() -> (Harness<'static, MailApp>, u64) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    let account = app
        .execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example", "port": 587, "security": "starttls"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 700.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    (h, account)
}

fn write(h: &mut Harness<'static, MailApp>, account: u64) {
    h.state_mut().run("mail.new", json!({"to": "maya@lumen.example", "subject": "Quarterly plan", "body": "Hi", "account": account})).unwrap();
    h.run_steps(2);
}

#[test]
fn delay_delivery_without_a_time_asks_for_one_and_sets_it() {
    let (mut h, account) = window();
    write(&mut h, account);
    h.state_mut().run("compose.delay", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("delay"));
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "the dialog closes once the time is set");
    let at = h.state().session.compose.as_ref().unwrap().send_at.expect("a time was set");
    assert!(at > NOW, "the next morning");
}

#[test]
fn a_message_waiting_in_the_outbox_says_when_it_goes_and_can_be_cancelled() {
    let (mut h, account) = window();
    write(&mut h, account);
    let id = h.state_mut().run("compose.sendLater", json!({"at": NOW + 3 * 3600})).unwrap()["id"].as_u64().unwrap();
    h.state_mut().run("folder.select", json!({"kind": "outbox", "account": account})).unwrap();
    h.state_mut().run("mail.select", json!({"id": id})).unwrap();
    h.run_steps(4);
    assert!(h.state().hit.rect("queuedBar", 0).is_some(), "the line over the message says it is delayed");
    h.get_by_label("Cancel Send").click();
    h.run_steps(3);
    let m = h.state().session.mailbox.message(id).unwrap();
    assert!(m.draft && m.send_at.is_some(), "back in Drafts, as a draft");
}

#[test]
fn the_undo_bar_shows_during_the_wait_and_undo_takes_the_message_back() {
    let (mut h, account) = window();
    h.state_mut().run("sync.waitBeforeSending", json!({"seconds": 10})).unwrap();
    write(&mut h, account);
    let id = h.state_mut().run("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    h.run_steps(3);
    assert!(h.state().hit.rect("undoSend", 0).is_some(), "the bar is across the window");
    h.get_by_label("Undo").click();
    h.run_steps(4);
    assert!(h.state().hit.rect("undoSend", 0).is_none(), "and gone once it is used");
    assert_eq!(h.state().session.compose.as_ref().and_then(|c| c.draft), Some(id), "the message is open to write again");
}

#[test]
fn ctrl_z_while_the_undo_bar_shows_takes_the_message_back() {
    let (mut h, account) = window();
    h.state_mut().run("sync.waitBeforeSending", json!({"seconds": 10})).unwrap();
    write(&mut h, account);
    let id = h.state_mut().run("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    h.run_steps(2);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().session.compose.as_ref().and_then(|c| c.draft), Some(id));
}

#[test]
fn the_status_bar_names_the_next_delayed_message() {
    let (mut h, account) = window();
    write(&mut h, account);
    h.state_mut().run("compose.sendLater", json!({"at": NOW + 3 * 3600})).unwrap();
    h.run_steps(3);
    let text = crate::send_later::status_text(h.state(), &egui::Context::default()).expect("a line for the delayed message");
    assert!(text.contains("2026"), "it names the time: {text}");
}
