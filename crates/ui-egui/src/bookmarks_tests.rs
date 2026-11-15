// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests for the non-modal All Bookmarks window and the short Bookmarks menu (#951).

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_engine::Session;
use mailcraft_model::{Account, Address, Body, FolderKind, MAX_BOOKMARK_QUOTE, Mailbox, Message};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app_harness(mut session: Session) -> Harness<'static, MailApp> {
    session.view.reading_pane = mailcraft_engine::Pane::Right;
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn key(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    for pressed in [true, false] {
        h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

#[test]
fn opens_via_ui_dialog_and_closes_with_escape_or_button() {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut h = app_harness(session);

    h.state_mut().execute("ui.dialog", json!({"name": "bookmarks"})).unwrap();
    h.run_steps(3);
    assert!(h.state().bookmarks.open);
    assert!(h.state().dialog.is_none(), "All Bookmarks window must be non-modal");
    assert!(h.query_by_label("No bookmarks yet.").is_some());

    key(&mut h, egui::Key::Escape);
    assert!(!h.state().bookmarks.open);

    h.state_mut().execute("ui.bookmarks", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.state().bookmarks.open);

    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().bookmarks.open);
}

#[test]
fn ribbon_menu_is_short_and_disables_add_without_selection() {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut h = app_harness(session);

    // Clear selection: Add Bookmark should be disabled, All Bookmarks… opens the window.
    h.state_mut().session.sel.messages.clear();
    h.run_steps(2);

    h.get_by_label_contains("Bookmarks").click();
    h.run_steps(2);

    let add_btn = h.get_by_label("Add Bookmark");
    assert!(add_btn.accesskit_node().is_disabled());
    let all_btn = h.get_by_label("All Bookmarks\u{2026}");
    assert!(!all_btn.accesskit_node().is_disabled());
    all_btn.click();
    h.run_steps(3);
    assert!(h.state().bookmarks.open);
}

#[test]
fn ordering_filter_by_name_quote_subject_and_empty_states() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "me@example.org".into(), ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();

    let m_alpha = mb
        .add_message(Message {
            folder: inbox,
            account,
            from: Address::new("Alice", "alice@example.org"),
            subject: "Quarterly Budget Review".into(),
            body: Body { text: "Details about revenue targets.".into(), html: None },
            received: NOW - 300,
            ..Default::default()
        })
        .unwrap();
    let long_quote = "Z".repeat(MAX_BOOKMARK_QUOTE + 20);
    let m_beta = mb
        .add_message(Message {
            folder: sent,
            account,
            from: Address::new("Bob", "bob@example.org"),
            subject: "Contract\t\u{202E}Addendum".into(),
            body: Body { text: long_quote.clone(), html: None },
            received: NOW - 100,
            ..Default::default()
        })
        .unwrap();

    let mut s = Session::at(mb, NOW, 0);
    s.run("bookmark.add", &json!({"id": m_alpha, "name": "Alpha Mark", "quote": "revenue targets", "offset": 0})).unwrap();
    s.clock.now = Some(NOW + 60);
    s.run("bookmark.add", &json!({"id": m_beta, "name": "Beta Mark", "quote": long_quote, "offset": 0})).unwrap();

    let mut h = app_harness(s);
    h.state_mut().execute("ui.dialog", json!({"name": "bookmarks"})).unwrap();
    h.run_steps(3);

    // Newest bookmark ("Beta Mark") is selected first, showing its capped quote (MAX_BOOKMARK_QUOTE chars).
    assert_eq!(h.state().bookmarks.cached_rows.len(), 2);
    assert_eq!(h.state().bookmarks.cached_rows[0].name, "Beta Mark");
    assert_eq!(h.state().bookmarks.cached_rows[1].name, "Alpha Mark");
    assert_eq!(h.state().bookmarks.cached_rows[0].subject, "Contract Addendum");
    let expected_quote: String = "Z".repeat(MAX_BOOKMARK_QUOTE);
    assert!(h.query_by_label(&expected_quote).is_some(), "full saved quote must appear in detail pane");

    // Filter case-insensitively by subject -> finds Alpha Mark only.
    h.state_mut().bookmarks.filter = "BUDGET".into();
    h.run_steps(2);
    assert_eq!(h.state().bookmarks.cached_rows.len(), 1);
    assert_eq!(h.state().bookmarks.cached_rows[0].name, "Alpha Mark");

    // Filter case-insensitively by quote -> finds Alpha Mark.
    h.state_mut().bookmarks.filter = "REVENUE TARGETS".into();
    h.run_steps(2);
    assert_eq!(h.state().bookmarks.cached_rows.len(), 1);
    assert_eq!(h.state().bookmarks.cached_rows[0].name, "Alpha Mark");

    // No match -> empty state message.
    h.state_mut().bookmarks.filter = "nonexistent-needle".into();
    h.run_steps(2);
    assert_eq!(h.state().bookmarks.cached_rows.len(), 0);
    assert!(h.query_by_label("No bookmarks match.").is_some());
}

#[test]
fn arrow_keys_and_double_click_jump_across_folders_and_highlight() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "me@example.org".into(), ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();

    let msg_a = mb
        .add_message(Message {
            folder: sent,
            account,
            from: Address::new("Carol", "carol@example.org"),
            subject: "Archived Clause".into(),
            body: Body { text: String::new(), html: Some("<p>Intro paragraph.</p><p>Clause Forty-Two of the final contract.</p>".into()) },
            received: NOW - 200,
            ..Default::default()
        })
        .unwrap();
    let msg_b = mb
        .add_message(Message {
            folder: inbox,
            account,
            from: Address::new("Dan", "dan@example.org"),
            subject: "Inbox Distraction".into(),
            body: Body { text: "Just a note in Inbox.".into(), html: None },
            received: NOW - 50,
            ..Default::default()
        })
        .unwrap();

    let mut s = Session::at(mb, NOW, 0);
    let b_a =
        s.run("bookmark.add", &json!({"id": msg_a, "name": "Clause 42", "quote": "Clause Forty-Two of the final contract.", "offset": 16})).unwrap();
    s.clock.now = Some(NOW + 30);
    let b_b = s.run("bookmark.add", &json!({"id": msg_b, "name": "Inbox Note", "quote": "Just a note", "offset": 0})).unwrap();

    // Select msg_b in Inbox so msg_a (in Sent) is not in the active folder view.
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    s.run("mail.select", &json!({"id": msg_b})).unwrap();

    let mut h = app_harness(s);
    h.state_mut().execute("ui.dialog", json!({"name": "bookmarks"})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().bookmarks.selected, Some(b_b["id"].as_u64().unwrap()));

    // ArrowDown moves selection to "Clause 42", ArrowUp back, then double-click "Clause 42".
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(h.state().bookmarks.selected, Some(b_a["id"].as_u64().unwrap()));
    key(&mut h, egui::Key::ArrowUp);
    assert_eq!(h.state().bookmarks.selected, Some(b_b["id"].as_u64().unwrap()));

    let row_center = h.get_by_label("Clause 42").rect().center();
    h.ctx.options_mut(|o| o.input_options.max_double_click_delay = 1.0);
    for _ in 0..2 {
        h.event(egui::Event::PointerMoved(row_center));
        h.event(egui::Event::PointerButton {
            pos: row_center,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        h.event(egui::Event::PointerButton {
            pos: row_center,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    h.run_steps(4);

    assert_eq!(h.state().session.sel.focus(), Some(msg_a));
    assert_eq!(h.state().session.view.folder, sent);
    assert!(h.state().bookmarks.open, "non-modal window stays open while reading");

    // Verify reading pane highlighted the quote of msg_a.
    assert!(h.state().text_selection.at(crate::reading::Place::Pane).is_some(), "expected highlighted quote selection in reading pane");
}

#[test]
fn two_step_remove_and_search_commands_integration() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let m1 = s.mailbox.messages.values().find(|m| m.folder == inbox).unwrap().id;
    let added = s.run("bookmark.add", &json!({"id": m1, "name": "Temp Mark", "quote": "snippet", "offset": 0})).unwrap();
    let bid = added["id"].as_u64().unwrap();

    let mut h = app_harness(s);
    // Search Commands lists "All Bookmarks" and running it opens the window.
    let all = crate::dialogs::all_commands(h.state());
    assert!(all.iter().any(|(id, label, _, _, on)| id == "ui.bookmarks" && label == "All Bookmarks" && *on));

    h.state_mut().execute("ui.dialog", json!({"name": "commands"})).unwrap();
    h.run_steps(2);
    h.event(egui::Event::Text("All Bookmarks".into()));
    h.run_steps(2);
    key(&mut h, egui::Key::Enter);
    assert!(h.state().bookmarks.open);

    // Two-step remove: first Remove, then Cancel keeps it; Remove + Confirm Remove deletes it.
    h.get_by_label("Remove").click();
    h.run_steps(2);
    assert!(h.state().session.mailbox.bookmark(bid).is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().session.mailbox.bookmark(bid).is_some());

    h.get_by_label("Remove").click();
    h.run_steps(2);
    h.get_by_label("Confirm Remove").click();
    h.run_steps(2);
    assert!(h.state().session.mailbox.bookmark(bid).is_none());
    assert!(h.query_by_label("No bookmarks yet.").is_some());
}
