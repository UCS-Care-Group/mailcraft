// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The folder pane: what it lists, its counts and its chevrons.

use mailcraft_engine::Session;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn names(folders: &[&Folder]) -> Vec<String> {
    folders.iter().map(|f| f.name.clone()).collect()
}

/// A standard folder still called by its default name, or the server's folder of its kind,
/// is named in the interface language; one the owner named keeps its name; English shows
/// every folder by its own name. What is kept never changes.
#[test]
fn folders_are_named_in_the_interface_language_by_their_kind() {
    let folder = |name: &str, kind: FolderKind, remote: &str| Folder { name: name.into(), kind, remote: remote.into(), ..Default::default() };
    assert_eq!(named_by(&folder("Inbox", FolderKind::Inbox, "")), Some(Named::Kind(FolderKind::Inbox)));
    assert_eq!(named_by(&folder("Sent Items", FolderKind::Sent, "")), Some(Named::Kind(FolderKind::Sent)));
    assert_eq!(named_by(&folder("Old mail", FolderKind::Archive, "")), None);
    assert_eq!(named_by(&folder("Sent Mail", FolderKind::Sent, "[Gmail]/Sent Mail")), Some(Named::Kind(FolderKind::Sent)));
    assert_eq!(named_by(&folder(mailcraft_model::IMPORTED, FolderKind::Custom, "")), Some(Named::Imported));
    assert_eq!(named_by(&folder("Imported", FolderKind::Custom, "Imported")), None);
    assert_eq!(named_by(&folder("Projects", FolderKind::Custom, "")), None);
    assert_eq!(mailcraft_model::IMPORTED, "Imported", "the text `folder_name` translates");
    // (The tests' window is in English: every folder by its own name.)
    assert_eq!(folder_name(&folder("Sent Mail", FolderKind::Sent, "[Gmail]/Sent Mail")), "Sent Mail");
    // A tangled tree still ends.
    let mut mb = Mailbox::default();
    mb.folders.push(Folder { id: 1, parent: Some(2), ..folder("a", FolderKind::Custom, "") });
    mb.folders.push(Folder { id: 2, parent: Some(1), ..folder("b", FolderKind::Custom, "") });
    assert!(folder_path(&mb, 1).split('/').count() <= 33);
}

/// The folders of `account` under `parent` (`None`: its top level), in the pane's order.
fn ordered(mailbox: &Mailbox, account: Id, parent: Option<Id>) -> Vec<&Folder> {
    let mut list: Vec<&Folder> = mailbox.folders.iter().filter(|f| f.account == account && f.parent == parent).collect();
    sort_as_listed(&mut list, parent.is_none());
    list
}

/// Inbox, Drafts, Sent Items and Deleted Items lead; everything else, the other standard
/// folders among it, follows by name whatever its case; subfolders are by name alone.
#[test]
fn folders_are_listed_leading_four_then_by_name() {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen.example");
    let account = mb.accounts.first().unwrap().id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for name in ["Zebra", "apple", "Banana", "outings", "Projects", "receipts"] {
        mb.add_folder(account, None, name).unwrap();
    }
    for name in ["beta", "Alpha", "delta", "Charlie"] {
        mb.add_folder(account, Some(inbox), name).unwrap();
    }
    assert_eq!(
        names(&ordered(&mb, account, None)),
        [
            "Inbox",
            "Drafts",
            "Sent Items",
            "Deleted Items",
            "apple",
            "Archive",
            "Banana",
            "Junk Email",
            "Outbox",
            "outings",
            "Projects",
            "receipts",
            "Zebra"
        ]
    );
    assert_eq!(names(&ordered(&mb, account, Some(inbox))), ["Alpha", "beta", "Charlie", "delta"]);

    // Depth first, each level in that order, and nothing from under a folded folder.
    let charlie = mb.folders.iter().find(|f| f.name == "Charlie").unwrap().id;
    mb.add_folder(account, Some(charlie), "two").unwrap();
    mb.add_folder(account, Some(charlie), "One").unwrap();
    let listed = |folded: &[Id]| {
        visible(&mb, folded, account).iter().map(|(f, depth, has_children)| (f.name.clone(), *depth, *has_children)).collect::<Vec<_>>()
    };
    let open = listed(&[]);
    let head: Vec<(&str, usize, bool)> = open.iter().take(8).map(|(n, d, c)| (n.as_str(), *d, *c)).collect();
    assert_eq!(
        head,
        [
            ("Inbox", 0, true),
            ("Alpha", 1, false),
            ("beta", 1, false),
            ("Charlie", 1, true),
            ("One", 2, false),
            ("two", 2, false),
            ("delta", 1, false),
            ("Drafts", 0, false)
        ]
    );
    assert_eq!(listed(&[charlie]).len(), open.len() - 2);
    assert_eq!(listed(&[inbox]).iter().filter(|(_, depth, _)| *depth > 0).count(), 0);

    // Another account's folders are its own, in the same order.
    let other = mb.add_account(mailcraft_model::Account { name: "studio".into(), ..Default::default() });
    mb.add_folder(other, None, "aardvark").unwrap();
    assert_eq!(names(&ordered(&mb, other, None)).iter().take(5).collect::<Vec<_>>(), ["Inbox", "Drafts", "Sent Items", "Deleted Items", "aardvark"]);
    assert_eq!(ordered(&mb, account, None).len(), 13);
}

/// A mailbox may come from anywhere: folders nested without end, in a ring, or by the thousand
/// are listed up to a point and no further.
#[test]
fn a_hostile_folder_tree_is_listed_up_to_a_point() {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen.example");
    let account = mb.accounts.first().unwrap().id;
    let mut parent = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for i in 0..3 * MAX_DEPTH {
        let id = mb.add_folder(account, None, &format!("level {i}")).unwrap();
        mb.folder_mut(id).unwrap().parent = Some(parent);
        parent = id;
    }
    let rows = visible(&mb, &[], account);
    assert_eq!(rows.iter().map(|(_, depth, _)| *depth).max(), Some(MAX_DEPTH));

    // Two folders that are each other's parent hang from nothing: neither is listed.
    let a = mb.add_folder(account, None, "ring a").unwrap();
    let b = mb.add_folder(account, None, "ring b").unwrap();
    mb.folder_mut(a).unwrap().parent = Some(b);
    mb.folder_mut(b).unwrap().parent = Some(a);
    let me = mb.add_folder(account, None, "ring of one").unwrap();
    mb.folder_mut(me).unwrap().parent = Some(me);
    assert!(!visible(&mb, &[], account).iter().any(|(f, _, _)| f.name.starts_with("ring")));

    for i in 0..MAX_ROWS + 50 {
        mb.folders.push(Folder { id: 1_000_000 + i as Id, account, name: format!("f{i}"), ..Default::default() });
    }
    assert_eq!(visible(&mb, &[], account).len(), MAX_ROWS);
}

/// Unread mail is a bold number; Drafts, Outbox and Junk Email show all they hold, in square
/// brackets; sent and deleted mail shows nothing.
#[test]
fn counts_are_unread_or_a_total_in_brackets() {
    assert_eq!(Count::of(FolderKind::Inbox, 9, 4), Count::Unread(4));
    assert_eq!(Count::of(FolderKind::Custom, 9, 4).text(), "4");
    assert_eq!(Count::of(FolderKind::Archive, 9, 0), Count::Nothing);
    assert_eq!(Count::of(FolderKind::Drafts, 3, 0), Count::Total(3));
    assert_eq!(Count::of(FolderKind::Junk, 28, 5).text(), "[28]");
    assert_eq!(Count::of(FolderKind::Outbox, 1, 1).text(), "[1]");
    assert_eq!(Count::of(FolderKind::Drafts, 0, 0).text(), "");
    assert_eq!(Count::of(FolderKind::Sent, 5, 5), Count::Nothing);
    assert_eq!(Count::of(FolderKind::Deleted, 5, 5), Count::Nothing);

    // The counts behind them, against the mailbox's own counting.
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let counts = s.folder_counts();
    let mb = &s.mailbox;
    assert!(mb.folders.iter().any(|f| mb.unread_count(f.id) > 0));
    for f in &mb.folders {
        let held = counts.folder(f.id);
        assert_eq!((held.total, held.unread), (mb.count(f.id), mb.unread_count(f.id)), "{}", f.name);
    }
}

fn window() -> egui_kittest::Harness<'static, MailApp> {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    // The sample has reminders due, and their window would lie over the top of the pane.
    let _ = harness.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
    harness.run_steps(3);
    harness
}

fn click(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
}

/// Two accounts' Inboxes among the Favorites say which account each is, as in a search of every
/// mailbox; under each account's own heading they stay plain Inbox.
#[test]
fn two_accounts_inboxes_among_the_favorites_are_told_apart() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let mb = &mut app.session.mailbox;
    let first = mb.inbox().unwrap();
    let work = mb.add_account(mailcraft_model::Account { name: "Work".into(), email: "me@work.example".into(), ..Default::default() });
    let second = mb.folder_of_kind(work, FolderKind::Inbox).unwrap();
    for id in [first, second] {
        app.execute("folder.favorite", serde_json::json!({"id": id, "value": true})).unwrap();
    }
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| folder_pane(&mut app, ui));
    out.textures_delta.clear();
    let texts: Vec<String> = out
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "Inbox - Work"), "{texts:?}");
    let mine = app.session.mailbox.accounts.first().unwrap();
    let mine = if mine.name.trim().is_empty() { mine.email.trim() } else { mine.name.trim() };
    assert!(texts.iter().any(|t| *t == format!("Inbox - {mine}")), "{texts:?}");
    assert!(texts.iter().any(|t| t == "Inbox"), "each account's own tree keeps the plain name: {texts:?}");
}

/// The pane lists the folders in order down the screen, one row each, further in at each level.
#[test]
fn rows_go_down_the_pane_in_order() {
    let harness = window();
    let app = harness.state();
    let account = app.session.mailbox.accounts.first().unwrap().id;
    let rows = visible(&app.session.mailbox, &[], account);
    assert!(rows.iter().any(|(_, depth, _)| *depth == 2), "the sample has folders two levels down");
    // A favourite's first row is the one under Favorites: look at the others.
    let listed: Vec<Rect> = rows.iter().filter(|(f, _, _)| !f.favorite).map(|(f, _, _)| app.hit.rect("folder", f.id).unwrap()).collect();
    assert!(listed.len() > 5);
    for pair in listed.windows(2) {
        assert!(pair[1].min.y >= pair[0].max.y - 0.5, "{pair:?}");
        assert!((pair[0].height() - ROW_H).abs() < 0.01, "{pair:?}");
    }
    // Favourites are above the account's heading, the account's own rows under it.
    let heading = app.hit.rect("folderHeading", account).unwrap();
    let favorites = app.hit.rect("folderHeading", FAVORITES).unwrap();
    assert!(favorites.max.y <= heading.min.y);
    assert!(listed.iter().all(|r| r.min.y >= heading.max.y - 0.5));
    let favorite = app.session.mailbox.folders.iter().find(|f| f.favorite).unwrap().id;
    let row = app.hit.rect("folder", favorite).unwrap();
    assert!(row.min.y >= favorites.max.y - 0.5 && row.max.y <= heading.min.y + 0.5);
}

/// A click on the Favorites heading or on an account's folds what is under it away, another
/// brings it back, and the choice is saved with the settings.
#[test]
fn headings_fold_and_the_settings_remember() {
    let mut harness = window();
    let account = harness.state().session.mailbox.accounts.first().unwrap().id;
    let drafts = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
    assert!(at(&harness, "folder", drafts).is_some());
    let before = at(&harness, "folderHeading", account).unwrap();

    // Favorites folded: the account's heading moves up to where the favourites were.
    let favorites = at(&harness, "folderHeading", FAVORITES).unwrap();
    click(&mut harness, favorites.center());
    assert!(harness.state().ui.collapsed_folders.contains(&FAVORITES));
    let after = at(&harness, "folderHeading", account).unwrap();
    assert!(after.min.y < before.min.y - ROW_H, "{before:?} -> {after:?}");
    assert!(at(&harness, "folder", drafts).is_some(), "the account's folders stay");

    // The account folded: none of its folders is listed.
    click(&mut harness, after.center());
    assert!(harness.state().ui.collapsed_folders.contains(&account));
    assert!(harness.state().session.mailbox.folders.iter().all(|f| at(&harness, "folder", f.id).is_none()));

    // Saved, and restored into another window.
    let saved = serde_json::to_string(&harness.state().prefs()).unwrap();
    let restored: crate::UiState = serde_json::from_str(&saved).unwrap();
    assert!(restored.collapsed_folders.contains(&FAVORITES) && restored.collapsed_folders.contains(&account));

    // And back.
    let heading = at(&harness, "folderHeading", account).unwrap();
    click(&mut harness, heading.center());
    assert!(!harness.state().ui.collapsed_folders.contains(&account));
    assert!(at(&harness, "folder", drafts).is_some());
    let favorites = at(&harness, "folderHeading", FAVORITES).unwrap();
    click(&mut harness, favorites.center());
    assert!(harness.state().ui.collapsed_folders.is_empty());
}

/// The button at the top of the pane puts it away; a narrow strip stays, with the button that
/// brings it back.
#[test]
fn the_pane_hides_and_its_strip_brings_it_back() {
    let mut harness = window();
    let inbox = harness.state().session.view.folder;
    let shown = |h: &egui_kittest::Harness<'static, MailApp>| h.state().session.view.folder_pane;
    assert!(shown(&harness) && harness.state().hit.rect("folderPaneShow", 0).is_none());
    let wide = harness.state().hit.rect("folderHeading", FAVORITES).unwrap().width();
    let hide = harness.state().hit.rect("folderPaneHide", 0).unwrap();
    click(&mut harness, hide.center());
    assert!(!shown(&harness));
    assert!(harness.state().hit.rect("folder", inbox).is_none() && harness.state().hit.rect("folderPaneHide", 0).is_none());
    let show = harness.state().hit.rect("folderPaneShow", 0).unwrap();
    assert!(show.max.x - hide.min.x < 0.0 && show.width() < wide, "the strip is at the pane's left edge, and narrow: {show:?}");
    assert!((show.center().y - hide.center().y).abs() < 0.5, "where the other button was");
    click(&mut harness, show.center());
    assert!(shown(&harness) && harness.state().hit.rect("folder", inbox).is_some());
}

/// A click on a folder's row shows that folder; a click on its chevron folds its subfolders
/// away and leaves the folder in view alone.
#[test]
fn a_row_selects_and_its_chevron_folds() {
    let mut harness = window();
    let account = harness.state().session.mailbox.accounts.first().unwrap().id;
    let (inbox, drafts) = {
        let mb = &harness.state().session.mailbox;
        (mb.folder_of_kind(account, FolderKind::Inbox).unwrap(), mb.folder_of_kind(account, FolderKind::Drafts).unwrap())
    };
    let child = harness.state().session.mailbox.folders.iter().find(|f| f.parent == Some(inbox) && !f.favorite).unwrap().id;
    let row = harness.state().hit.rect("folder", drafts).unwrap();
    click(&mut harness, row.center());
    assert_eq!(harness.state().session.view.folder, drafts);

    // The Inbox is a favourite too: its row in the account's list is the one with the chevron.
    let heading = harness.state().hit.rect("folderHeading", account).unwrap();
    let child_row = harness.state().hit.rect("folder", child).unwrap();
    let inbox_row = Rect::from_min_size(pos2(child_row.min.x, heading.max.y), vec2(child_row.width(), ROW_H));
    click(&mut harness, pos2(inbox_row.min.x + CHEVRON_X, inbox_row.center().y));
    assert!(harness.state().ui.collapsed_folders.contains(&inbox));
    assert_eq!(harness.state().session.view.folder, drafts, "the chevron doesn't select");
    assert!(harness.state().hit.rect("folder", child).is_none());
    click(&mut harness, pos2(inbox_row.min.x + CHEVRON_X, inbox_row.center().y));
    assert!(harness.state().hit.rect("folder", child).is_some());
}
