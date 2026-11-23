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
    app.passwords.insert(owner, "an invented password".into());
    let order: Vec<Id> = app.session.mailbox.pane_order().iter().map(|a| a.id).collect();
    let at = |id: Id| order.iter().position(|a| *a == id).unwrap();
    assert_eq!(at(archive), at(owner) + 1);
    // (Not among the accounts mail is sent from or set up: the From list, File › Info…)
    assert!(!app.session.mailbox.accounts_in_order().iter().any(|a| a.id == archive));
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

/// Without a password at hand nothing is read by itself, and it says what would read it rather
/// than Loading… (#1818 review).
#[test]
fn without_a_password_it_says_how_to_read_it() {
    let (mut app, _, _) = app();
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t.contains("Send/Receive the mailbox")), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "Loading…"));
}

/// A read that failed isn't started again at every frame, nor forever shown as Loading…: the
/// pane says so, and the owner's Refresh (a Send/Receive they ask for) tries again (#1818 review).
#[test]
fn a_failed_read_says_so_and_waits_for_the_owner() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    app.archive_failed.insert(archive);
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t.contains("couldn't be read")), "{shown:?}");
    assert!(!app.syncing(), "nothing started by itself");
    assert!(app.archive_reads.is_empty());
    // The owner asks: the failure is forgotten, and what comes of the run decides again.
    let _ = app.start_ews_mail(archive, false, &serde_json::json!({}));
    assert!(!app.archive_failed.contains(&archive));
}

/// Coming back to a folder of the archive reads it again; leaving it reads nothing (#1818 review).
#[test]
fn a_folder_of_the_archive_is_read_each_time_it_comes_on_screen() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    let year = app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.archive_reads.insert(archive, year);
    let inbox = app.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).unwrap();
    app.session.view.folder = inbox;
    app.read_archive(archive);
    assert_eq!(app.archive_reads.get(&archive), Some(&0), "leaving its folders reads nothing");
    assert!(!app.archive_failed.contains(&archive), "no read was tried");
    app.session.view.folder = year;
    app.read_archive(archive);
    // (No host here: the read can't start, which says so rather than trying at every frame.)
    assert!(app.archive_failed.contains(&archive), "a read was tried");
}

/// Wherever the archive is named away from its heading (a folder's account in a list, the
/// problems of a run, the window's title), it is named in the interface's words, never by the
/// name it is kept under (#1834 review).
#[test]
fn the_archive_is_never_named_by_the_name_it_is_kept_under() {
    let (mut app, _, archive) = app();
    let kept = "Kept under this name".to_owned();
    app.session.mailbox.accounts.iter_mut().find(|a| a.id == archive).unwrap().name.clone_from(&kept);
    let a = app.session.mailbox.account(archive).unwrap().clone();
    for shown in [account_label(&a), account_name(&a), account_title(&a), heading_name(&app, &a)] {
        assert_eq!(shown, "Online Archive – ada@corp.example");
    }
    let folder = app.session.mailbox.add_folder(archive, None, "Projects").unwrap();
    app.session.view.folder = folder;
    assert!(!app.title_stem().contains(&kept), "{}", app.title_stem());
}
