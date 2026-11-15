// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder › Clean Up › Senders (#1207, part 3): the list of a folder's senders, and what its
//! buttons hand on once it has closed.
//!
//! The people, lists and hosts are invented.

use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::{Address, FolderKind, Mailbox, Message};
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

fn window(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(2);
    harness
}

const NOW: i64 = 1_791_993_600;

/// A window on Avery's mailbox whose Inbox has mail from three senders: two from the list (one
/// read), one from a shop, one unvouched from a "bank". The ids, in that order.
fn app() -> (MailApp, Vec<u64>) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    for account in &mut mailbox.accounts {
        account.kind = mailcraft_model::AccountKind::Imap;
        account.incoming = Some(mailcraft_model::Server { host: "imap.example.org".into(), port: 993, security: mailcraft_model::Security::Tls });
    }
    let inbox = mailbox.inbox().unwrap();
    let mail = [
        ("letter@news.example", "<https://news.example/leave>", true, true, 7_200),
        ("letter@news.example", "<https://news.example/leave>", true, false, 3_600),
        ("deals@shop.example", "<mailto:leave@shop.example>", true, false, 5_000),
        ("alerts@bank.example", "<https://bank.example/u>", false, false, 100),
    ];
    let mut ids = Vec::new();
    for (n, (from, list, vouched, read, ago)) in mail.into_iter().enumerate() {
        let domain = from.rsplit_once('@').map_or("", |(_, d)| d);
        let mut headers = vec![("List-Unsubscribe".to_string(), list.to_string())];
        if vouched {
            headers.push(("Authentication-Results".into(), format!("mx.example.org; dmarc=pass header.from={domain}")));
        }
        let m = Message {
            folder: inbox,
            from: Address::new("", from),
            subject: format!("issue {n}"),
            uid: n as u32 + 1,
            date: NOW - ago,
            read,
            headers,
            ..Message::default()
        };
        ids.push(mailbox.add_message(m).unwrap());
    }
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.session.view.folder = inbox;
    (app, ids)
}

fn senders(app: &mut MailApp) -> &mut crate::senders::Senders {
    match &mut app.dialog {
        Some(Dialog::Senders(d)) => d,
        _ => panic!("Senders isn't open"),
    }
}

#[test]
fn the_ribbon_button_opens_the_senders_of_the_folder_in_view() {
    let (mut app, _) = app();
    // Run by the owner, the command is a dialog; scripts and agents get the list itself.
    app.run("mail.senders", json!({})).unwrap();
    let d = senders(&mut app);
    let listed: Vec<(&str, u64, u64)> = d.rows.iter().map(|r| (r.sender.as_str(), r.messages, r.unread)).collect();
    assert_eq!(listed, [("letter@news.example", 2, 1), ("alerts@bank.example", 1, 1), ("deals@shop.example", 1, 1)]);
    // As many messages: the newest first. The bank's mail offers nothing, and says why.
    assert!(d.rows[1].way.is_none() && !d.rows[1].why.is_empty());
    let said = app.execute("mail.senders", json!({})).unwrap();
    assert_eq!(said["senders"].as_array().map(Vec::len), Some(3));
    let mut harness = window(app);
    harness.run_steps(2);
    assert!(harness.query_all_by_label_contains("Senders in").next().is_some());
    assert!(harness.query_all_by_label("deals@shop.example").next().is_some());
}

#[test]
fn unsubscribe_asks_about_each_ticked_senders_newest_message() {
    let (mut app, ids) = app();
    app.run("mail.senders", json!({})).unwrap();
    let d = senders(&mut app);
    d.ticked.extend(["letter@news.example".to_string(), "alerts@bank.example".to_string()]);
    // The bank's mail offers nothing: only the list's newest message goes ahead.
    if let Some(Dialog::Senders(d)) = &app.dialog {
        assert_eq!(d.to_unsubscribe(&app), [ids[1]]);
    }
    let mut harness = window(app);
    harness.run_steps(2);
    harness.get_by_label("Unsubscribe").click();
    harness.run_steps(3);
    // Senders has closed, and the question of `ui.unsubscribeMany` is the one shown.
    match &harness.state().dialog {
        Some(Dialog::Confirm { command, params, .. }) => {
            assert_eq!(command, "ui.unsubscribeMany");
            assert_eq!(params["ids"], json!([ids[1]]));
        }
        _ => panic!("no question was asked"),
    }
}

#[test]
fn delete_all_takes_every_message_of_the_ticked_senders_to_deleted_items() {
    let (mut app, ids) = app();
    app.run("mail.senders", json!({})).unwrap();
    senders(&mut app).ticked.insert("letter@news.example".to_string());
    let mut harness = window(app);
    harness.run_steps(2);
    harness.get_by_label("Delete All").click();
    harness.run_steps(3);
    let app = harness.state();
    let deleted = app.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Deleted).map(|f| f.id).unwrap();
    let folder_of = |id: u64| app.session.mailbox.message(id).map(|m| m.folder);
    assert_eq!((folder_of(ids[0]), folder_of(ids[1])), (Some(deleted), Some(deleted)));
    assert_ne!(folder_of(ids[2]), Some(deleted), "the other senders' mail stays");
    assert!(app.dialog.is_none());
}

#[test]
fn move_all_selects_their_messages_and_asks_where() {
    let (mut app, ids) = app();
    app.run("mail.senders", json!({})).unwrap();
    senders(&mut app).ticked.insert("letter@news.example".to_string());
    let mut harness = window(app);
    harness.run_steps(2);
    harness.get_by_label("Move All…").click();
    harness.run_steps(3);
    let app = harness.state();
    let mut selected = app.session.sel.messages.clone();
    selected.sort_unstable();
    assert_eq!(selected, [ids[0], ids[1]]);
    assert!(matches!(&app.dialog, Some(d) if d.name() == "moveItems"));
}
