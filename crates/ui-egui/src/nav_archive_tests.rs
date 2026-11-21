// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An online archive in the folder pane (#1818): named as Outlook names it, right after its
//! owner's mailbox and before its shared mailboxes, saying it is loading (or needs a connection)
//! until its folders come, and never dragged by itself.

use mailcraft_engine::Session;
use mailcraft_model::{Account, AccountKind, EwsServer};

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A window with an Exchange server account (after the sample's) that has an online archive.
fn app() -> (MailApp, Id, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let mb = &mut app.session.mailbox;
    let server = EwsServer { url: "https://mail.example.org/EWS/Exchange.asmx".into(), ..Default::default() };
    let owner = mb.add_account(Account {
        name: "Work".into(),
        email: "ada@corp.example".into(),
        kind: AccountKind::Exchange,
        ews: Some(server),
        ..Default::default()
    });
    let archive = mb.add_archive(owner).unwrap();
    (app, owner, archive)
}

fn texts(app: &mut MailApp) -> Vec<String> {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| folder_pane(app, ui));
    out.textures_delta.clear();
    out.shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_archive_is_named_as_outlook_names_it_right_after_its_owner() {
    let (mut app, owner, archive) = app();
    let order: Vec<Id> = app.session.mailbox.accounts_in_order().iter().map(|a| a.id).collect();
    let at = |id: Id| order.iter().position(|a| *a == id).unwrap();
    assert_eq!(at(archive), at(owner) + 1);
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t == "Online Archive – ada@corp.example"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "Loading…"), "nothing of it here yet: {shown:?}");
    // Only the owner's own accounts are dragged; the archive goes with its owner.
    assert!(!owners(&app.session.mailbox).contains(&archive));
}

#[test]
fn offline_it_says_it_needs_a_connection() {
    let (mut app, _, _) = app();
    app.session.prefs.offline = true;
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t.contains("needs a connection")), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "Loading…"));
}

#[test]
fn with_its_folders_here_it_lists_them_and_says_nothing_more() {
    let (mut app, _, archive) = app();
    app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t == "2019") && !shown.iter().any(|t| t == "Loading…"), "{shown:?}");
}
