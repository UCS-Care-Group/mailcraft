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

/// A folder of the archive on screen is read even when the archive's heading is folded (opened
/// from Favorites, or with the folder pane hidden); with none on screen, offline, or after a read
/// that failed, nothing is read (#1834 review).
#[test]
fn a_folder_of_the_archive_on_screen_is_read_with_its_heading_folded() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    let year = app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.ui.collapsed_folders.push(archive);
    // No folder of it on screen: nothing is read.
    let inbox = app.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).unwrap();
    app.session.view.folder = inbox;
    let _ = texts(&mut app);
    app.archive_tick();
    assert!(app.archive_reads.is_empty() && !app.archive_failed.contains(&archive), "nothing read");
    // Offline: nothing either.
    app.session.view.folder = year;
    app.session.prefs.offline = true;
    app.archive_tick();
    assert!(app.archive_reads.is_empty() && !app.archive_failed.contains(&archive), "nothing read offline");
    app.session.prefs.offline = false;
    // Its folder on screen with the heading folded: the pane reads nothing, the frame's tick does.
    let _ = texts(&mut app);
    assert!(app.archive_reads.is_empty(), "the folded heading reads nothing");
    app.archive_tick();
    // (No host here: the read can't start, which says so rather than trying at every frame.)
    assert_eq!(app.archive_reads.get(&archive), Some(&year));
    assert!(app.archive_failed.contains(&archive), "a read was tried");
    // A read that failed isn't tried again by itself.
    app.archive_reads.clear();
    app.archive_tick();
    assert!(app.archive_reads.is_empty(), "not tried again after it failed");
}

/// Leaving the archive's folders is noted with its heading folded too: coming back reads it again
/// (#1834 review).
#[test]
fn leaving_the_archive_with_its_heading_folded_is_noted() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    let year = app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.ui.collapsed_folders.push(archive);
    app.archive_reads.insert(archive, year);
    let inbox = app.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).unwrap();
    app.session.view.folder = inbox;
    app.archive_tick();
    assert_eq!(app.archive_reads.get(&archive), Some(&0), "leaving its folders reads nothing");
    assert!(!app.archive_failed.contains(&archive));
    app.session.view.folder = year;
    app.archive_tick();
    assert!(app.archive_failed.contains(&archive), "coming back reads it again");
}

/// Another module or File in front shows no folder of the archive: leaving it that way is noted,
/// nothing is read meanwhile, and Mail back in front reads it again; with the folder pane hidden,
/// Mail still reads it (#1834 review).
#[test]
fn another_module_or_file_in_front_leaves_the_archive() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    let year = app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.session.view.folder = year;
    app.archive_reads.insert(archive, year);
    for module in [mailcraft_engine::Module::Calendar, mailcraft_engine::Module::People, mailcraft_engine::Module::Tasks] {
        app.session.view.module = module;
        app.archive_tick();
        assert_eq!(app.archive_reads.get(&archive), Some(&0), "{module:?} in front leaves it");
        assert!(!app.archive_failed.contains(&archive), "nothing read with {module:?} in front");
        app.archive_reads.insert(archive, year);
    }
    app.session.view.module = mailcraft_engine::Module::Mail;
    app.ui.backstage = true;
    app.archive_tick();
    assert_eq!(app.archive_reads.get(&archive), Some(&0), "File in front leaves it");
    assert!(!app.archive_failed.contains(&archive), "nothing read with File in front");
    // (Read straight, as the folder pane would: File in front still shows none of it.)
    app.read_archive(archive);
    assert!(!app.archive_failed.contains(&archive), "nothing read with File in front");
    // Back to Mail with the folder pane hidden: the same folder is read again.
    app.ui.backstage = false;
    app.session.view.folder_pane = false;
    app.archive_tick();
    // (No host here: the read can't start, which says so rather than trying at every frame.)
    assert_eq!(app.archive_reads.get(&archive), Some(&year));
    assert!(app.archive_failed.contains(&archive), "coming back reads it again");
}

/// A Graph job under way without a stream of pieces holds the run state the archive's read would
/// take: the read waits, as Send/Receive does, and a later frame starts it (#1834 review).
#[test]
fn the_archive_waits_for_a_graph_job_without_pieces() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    let year = app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.session.view.folder = year;
    app.graph_rx = Some((owner, true, false, app.session.openings(), std::sync::mpsc::channel().1));
    assert!(!app.syncing(), "a Graph job without pieces isn't a Send/Receive");
    assert!(app.start_sync(&serde_json::json!({"quiet": true})).is_err(), "Send/Receive waits");
    app.archive_tick();
    assert!(app.archive_reads.is_empty() && !app.archive_failed.contains(&archive), "the read waits");
    assert!(app.graph_rx.is_some() && app.sync_rx.is_none(), "the run state is left alone");
    // The job over: the next frame reads it.
    app.graph_rx = None;
    app.archive_tick();
    assert_eq!(app.archive_reads.get(&archive), Some(&year));
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

/// A read that failed says so under the heading even when the archive's folders are here (a
/// folder of it opened later couldn't be read), and the folders stay listed (#1834 review).
#[test]
fn a_failed_read_says_so_with_its_folders_here_too() {
    let (mut app, owner, archive) = app();
    app.passwords.insert(owner, "an invented password".into());
    app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    app.archive_failed.insert(archive);
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t.contains("couldn't be read")), "{shown:?}");
    assert!(shown.iter().any(|t| t == "2019"), "{shown:?}");
    assert!(!app.syncing(), "nothing started by itself");
    // Offline it is the connection that is wanted: no line over the folders.
    app.session.prefs.offline = true;
    let shown = texts(&mut app);
    assert!(!shown.iter().any(|t| t.contains("couldn't be read")), "{shown:?}");
}

/// A run of the archive that read well but said something (a change the server refused) keeps
/// what it said, and is no failed read: the archive goes on being read by itself (#1834 review).
#[test]
fn a_run_that_read_well_keeps_what_it_said() {
    let (mut app, _, archive) = app();
    app.archive_failed.insert(archive);
    app.note_sync_problems(archive, &["“2019” couldn't be renamed: a name that is taken".into()], false);
    app.archive_run_ended(archive, true, true);
    assert!(!app.archive_read_failed(archive));
    assert!(app.sync_problems.iter().any(|(a, _, _, lines)| *a == archive && !lines.is_empty()), "what it said stays");
    // One that said nothing clears what earlier runs said.
    app.archive_run_ended(archive, true, false);
    assert!(!app.sync_problems.iter().any(|(a, _, _, lines)| *a == archive && !lines.is_empty()));
    // A failed one waits for the owner.
    app.archive_run_ended(archive, false, false);
    assert!(app.archive_read_failed(archive));
}

/// Hidden, the archive's heading is gone: its owner's heading menu offers Show Online Archive,
/// which runs the command and starts a run of the owner's, whose yes brings it back (#1834
/// review).
#[test]
fn hidden_its_owners_menu_brings_it_back() {
    let (mut app, owner, archive) = app();
    assert!(!archive_hidden(&app.session.mailbox, owner), "nothing to show while it is shown");
    app.run("account.archive.hide", serde_json::json!({"account": archive})).unwrap();
    assert!(app.session.mailbox.archive_of(owner).is_none());
    assert!(archive_hidden(&app.session.mailbox, owner), "the owner's menu offers the way back");
    assert!(!archive_hidden(&app.session.mailbox, 9_999));
    // Its heading's menu, as the window draws it, right-clicked.
    use egui_kittest::kittest::Queryable as _;
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 1600.0)).build_ui_state(
            // (The window without its logic: no run starts by itself here, only what is clicked.)
            |ui, app: &mut MailApp| app.ui(ui),
            app,
        );
    h.run_steps(4);
    // A host for the runs from here on that says what each was asked (and never answers).
    h.state_mut().passwords.insert(owner, "an invented password".into());
    let asked: std::sync::Arc<std::sync::Mutex<Vec<(Id, bool)>>> = Default::default();
    let seen = asked.clone();
    let held: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<mailcraft_model::sync::EwsMailResult>>>> = Default::default();
    let keep = held.clone();
    h.state_mut().services.ews = Some(Box::new(move |job: crate::ews_mail::EwsJob| {
        seen.lock().unwrap().push((job.request.account, job.request.probe_archive));
        let (tx, rx) = std::sync::mpsc::channel();
        keep.lock().unwrap().push(tx);
        rx
    }));
    let pos = h.state().hit.rect("folderHeading", owner).expect("the owner's heading is on screen").center();
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert!(asked.lock().unwrap().is_empty(), "nothing started before the click: {:?}", asked.lock().unwrap());
    h.get_by_label("Show Online Archive").click();
    h.run_steps(3);
    let app = h.state_mut();
    assert!(!archive_hidden(&app.session.mailbox, owner));
    let request = mailcraft_engine::cmd::ews_mail::ews_mail_request(&app.session, owner, false).unwrap();
    assert!(request.probe_archive, "the server is asked again");
    // And a run of the owner's starts at once, as Refresh starts one, asking the server about
    // the archive (#1834 review).
    assert_eq!(asked.lock().unwrap().as_slice(), [(owner, true)], "one run of the owner's, asking about the archive");
}

/// The folders a move or a copy is offered are those the engine takes it into (#1834 review):
/// a copy never into the archive from elsewhere, nor out of it; a move into it only of mail its
/// owner's server has.
#[test]
fn the_folder_menus_offer_the_archive_only_where_mail_may_go() {
    use crate::ribbon::{FolderScope, folder_rows};
    use std::sync::Arc;
    let (mut app, owner, archive) = app();
    let mb = &mut app.session.mailbox;
    let year = mb.add_folder(archive, None, "2019").unwrap();
    // Mail of the owner's mailbox, as a run brings it.
    let inbox = mb.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "INBOX-ID".into(), ..Default::default() });
    let owners = mb.alloc_id();
    let server = mailcraft_model::sync::ServerMessage { id: "ITEM-1".into(), folder: "INBOX-ID".into(), ..Default::default() };
    mb.messages
        .insert(owners, Arc::new(mailcraft_model::Message { id: owners, account: owner, folder: inbox, server: Some(server), ..Default::default() }));
    // Mail in the archive.
    let archived = mb.alloc_id();
    mb.messages.insert(archived, Arc::new(mailcraft_model::Message { id: archived, account: archive, folder: year, ..Default::default() }));
    // Mail of another account (the sample's).
    let other = mb.messages.values().find(|m| m.account != owner && m.account != archive).unwrap().id;
    let offered = |app: &MailApp, scope: FolderScope, folder: Id| folder_rows(app, scope).0.iter().any(|row| row.id == folder);
    let other_folder = app.session.mailbox.message(other).unwrap().folder;
    for (selected, moved, copied, elsewhere_copied) in [
        (vec![other], false, false, true),
        (vec![owners], true, false, true),
        (vec![owners, other], false, false, true),
        (vec![archived], true, true, false),
    ] {
        app.session.sel.messages = selected.clone();
        assert_eq!(offered(&app, FolderScope::EveryAccount, year), moved, "move {selected:?}");
        assert_eq!(offered(&app, FolderScope::CopyTargets, year), copied, "copy {selected:?}");
        assert_eq!(offered(&app, FolderScope::CopyTargets, other_folder), elsewhere_copied, "copy elsewhere {selected:?}");
        // And the engine says the same of each.
        let mb = &app.session.mailbox;
        for id in &selected {
            let m = mb.message(*id).unwrap();
            assert_eq!(mb.copies_into(m, archive), m.account == archive);
        }
    }
}

/// With its folders here but no password at hand (the mailbox's was turned down, or forgotten),
/// the archive says how to read it rather than nothing (#1834 review).
#[test]
fn without_a_password_it_says_how_to_read_it_with_its_folders_here_too() {
    let (mut app, _, archive) = app();
    app.session.mailbox.add_folder(archive, None, "2019").unwrap();
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t.contains("Send/Receive the mailbox")), "{shown:?}");
    assert!(shown.iter().any(|t| t == "2019"), "{shown:?}");
    assert!(!app.syncing(), "nothing started by itself");
}

/// A folder of the archive named like one of its owner's (both have an Inbox), as a Favorite or a
/// row of a search of every mailbox, is told apart by the archive's name in the interface's
/// words, never by the name it is kept under (#1834 review).
#[test]
fn an_archive_folder_named_like_its_owners_is_told_apart_by_the_archives_name() {
    let (mut app, owner, archive) = app();
    app.session.mailbox.accounts.iter_mut().find(|a| a.id == archive).unwrap().name = "Kept under this name".into();
    let inbox = app.session.mailbox.add_folder(archive, None, "Inbox").unwrap();
    assert!(app.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).is_some());
    let mb = &app.session.mailbox;
    assert_eq!(result_folder_label(mb, mb.folder(inbox).unwrap()), "Inbox - Online Archive – ada@corp.example");
    // As a Favorite, drawn in the pane.
    app.session.mailbox.folder_mut(inbox).unwrap().favorite = true;
    let shown = texts(&mut app);
    assert!(shown.iter().any(|t| t == "Inbox - Online Archive – ada@corp.example"), "{shown:?}");
    assert!(!shown.iter().any(|t| t.contains("Kept under this name")), "{shown:?}");
}
