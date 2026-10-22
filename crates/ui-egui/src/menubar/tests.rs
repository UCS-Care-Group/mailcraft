// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menu bar's table: what each menu holds, which keys it shows, and what an item runs when.

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;

use super::*;
use crate::Services;

fn app() -> MailApp {
    MailApp::new(Session::new(Mailbox::default()), Services::default())
}

fn items(block: &MenuBlock) -> impl Iterator<Item = &Item> {
    block.entries.iter().filter_map(|e| if let Entry::Item(i) = e { Some(i) } else { None })
}

#[test]
fn every_listed_command_exists_and_none_is_left_out() {
    let app = app();
    let tables: Vec<&[Row]> = TABLES.iter().map(|t| t.1).chain([HELP]).collect();
    for rows in &tables {
        for row in *rows {
            if let Row::Cmd(id) = row {
                assert!(app.session.registry.get(id).is_some(), "{id} is not a command");
            }
        }
    }
    let wanted: usize = tables.iter().flat_map(|r| r.iter()).filter(|r| !matches!(r, Row::Sep)).count();
    let built: usize = menus(&app).iter().map(|m| items(m).count()).sum();
    assert_eq!(wanted, built);
    // The `ui.*` commands the own items run exist too, with params that read.
    for own in [Own::Info, Own::Options, Own::About, Own::HelpTab, Own::Settings] {
        let Action::Command { id, params } = own.action() else { panic!("{own:?} is not a command") };
        assert!(app.enabled(id), "{id} is not a command");
        assert!(serde_json::from_str::<Value>(params).is_ok_and(|v| v.is_object()), "{params}");
    }
}

#[test]
fn no_menu_is_empty_and_separators_only_divide() {
    let app = app();
    let menus = menus(&app);
    assert_eq!(menus.len(), TABLES.len() + 1);
    for m in &menus {
        assert!(!m.title.is_empty());
        assert!(items(m).count() > 0, "{} is empty", m.title);
        assert!(!matches!(m.entries.first(), Some(Entry::Separator)));
        assert!(!matches!(m.entries.last(), Some(Entry::Separator)));
        assert!(m.entries.windows(2).all(|w| !matches!(w, [Entry::Separator, Entry::Separator])), "{}", m.title);
        assert!(items(m).all(|i| !i.label.is_empty()));
    }
    assert!(!window_title().is_empty());
    assert!(!app_items().is_empty());
    let t = app_text();
    assert!([t.about, t.hide, t.quit, t.minimize, t.zoom].iter().all(|s| !s.is_empty()));
}

#[test]
fn a_key_goes_to_one_item_and_none_is_one_a_text_field_needs() {
    let app = app();
    let mut seen = HashSet::new();
    for i in menus(&app).iter().flat_map(items) {
        if let Some(c) = &i.chord {
            assert!(seen.insert(c.clone()), "{c:?} twice");
            assert!(c.key == "Enter" || c.key.chars().count() == 1);
        }
    }
    assert!(seen.contains(&Chord::new(false, false, "Z")) && seen.contains(&Chord::new(false, false, "V")));
    assert!(chord("Delete").is_none() && chord("Escape").is_none() && chord("F9").is_none());
    assert!(chord("Mod+Q").is_none() && chord("Mod+Y").is_none() && chord("Mod+A").is_none());
    assert_eq!(chord("Delete / Mod+D"), Some(Chord::new(false, false, "D")));
    assert_eq!(chord("Mod+Shift+R"), Some(Chord::new(true, false, "R")));
    assert_eq!(chord("Mod+Alt+1"), Some(Chord::new(false, true, "1")));
    assert_eq!(chord("Mod+Enter / Alt+S"), Some(Chord::new(false, false, "Enter")));
    assert!(chord("").is_none() && chord("Mod+").is_none() && chord("Mod+Foo+R").is_none());
}

#[test]
fn commands_are_enabled_from_the_registry_and_unknown_ones_are_refused() {
    let mut app = app();
    // Nothing is selected in an empty mailbox, so Reply can't run; New Email can.
    assert!(!enabled(&app, &Action::Command { id: "mail.reply", params: "{}" }));
    assert!(enabled(&app, &Action::Command { id: "mail.new", params: "{}" }));
    assert!(!enabled(&app, &Action::Command { id: "no.such.command", params: "{}" }));
    assert!(enabled(&app, &Action::Edit(EditKey::Copy)));
    // Running something unknown, with junk params, does nothing and does not panic.
    run(&mut app, &Action::Command { id: "no.such.command", params: "{" }, String::new);
}

#[test]
fn edit_items_become_the_input_a_text_field_takes() {
    let mut app = app();
    run(&mut app, &Action::Edit(EditKey::Paste), || "pasted".to_string());
    run(&mut app, &Action::Edit(EditKey::SelectAll), String::new);
    assert!(app.synthetic.iter().any(|e| matches!(e, Event::Paste(t) if t == "pasted")));
    assert!(app.synthetic.iter().any(|e| matches!(e, Event::Key { key: Key::A, pressed: true, modifiers, .. } if modifiers.command)));
    for _ in 0..200 {
        run(&mut app, &Action::Edit(EditKey::Copy), String::new);
    }
    assert!(app.synthetic.len() <= 70);
}

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox, with a message selected in Mail.
fn sample() -> (MailApp, u64) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let id = app.session.rows().first().map(|r| r.id).unwrap();
    app.execute("mail.select", serde_json::json!({"id": id})).unwrap();
    (app, id)
}

fn cmd(id: &'static str) -> Action {
    Action::Command { id, params: "{}" }
}

/// #253 review: Edit › Search (⌘E, which the window no longer sees) focuses the search box, as
/// the key did, instead of running `view.search` with no query.
#[test]
fn search_focuses_the_search_box_as_its_key_does() {
    let (mut app, _) = sample();
    app.execute("view.calendar", serde_json::json!({})).unwrap();
    assert_eq!(crate::keys::window_command(&app, "view.search"), Some("ui.focusSearch"));
    assert!(enabled(&app, &cmd("view.search")));
    run(&mut app, &cmd("view.search"), String::new);
    assert!(app.search_focus, "the search box was not given the keyboard");
    assert_eq!(app.session.view.module, mailcraft_engine::Module::Mail);
    assert!(app.session.view.search.is_empty());
}

/// #253 review: Folder › Send/Receive is the host's `ui.sync`, as F9 is: greyed where the window
/// can't send or receive, and starting a run where it can.
#[test]
fn send_receive_is_the_hosts_as_its_key_is() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mut app, _) = sample();
    app.execute("account.add", serde_json::json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    assert!(app.session.enabled("sync.request"));
    assert_eq!(crate::keys::window_command(&app, "sync.request"), Some("ui.sync"));
    // No host to talk to the server: the engine's command would run, the window's can't.
    assert!(!enabled(&app, &cmd("sync.request")));
    let started = Arc::new(AtomicUsize::new(0));
    let seen = started.clone();
    app.services.sync = Some(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        std::sync::mpsc::channel().1
    }));
    assert!(enabled(&app, &cmd("sync.request")));
    run(&mut app, &cmd("sync.request"), String::new);
    // A run starts, or asks for the account's password first, as the button does.
    assert!(started.load(Ordering::SeqCst) > 0 || app.sync_rx.is_some() || app.dialog.is_some(), "no run started");
}

/// #253 review: in Calendar, People or Tasks the Message menu's items are greyed and do nothing,
/// though a message is still selected in Mail (⌘D deleted it, ⌘R answered it).
#[test]
fn mail_commands_are_greyed_and_not_run_outside_mail() {
    let (mut app, id) = sample();
    let folder = app.session.mailbox.message(id).map(|m| m.folder);
    assert!(folder.is_some());
    for c in ["mail.reply", "mail.delete", "mail.toggleFlag"] {
        assert!(enabled(&app, &cmd(c)), "{c} in Mail");
    }
    for module in ["view.calendar", "view.people", "view.tasks"] {
        app.execute(module, serde_json::json!({})).unwrap();
        for c in ["mail.reply", "mail.delete", "mail.forward", "mail.toggleFlag", "folder.new", "compose.send"] {
            assert!(!enabled(&app, &cmd(c)), "{c} in {module}");
            assert_eq!(crate::keys::window_command(&app, c), None);
        }
        run(&mut app, &cmd("mail.delete"), String::new);
        run(&mut app, &cmd("mail.reply"), String::new);
        run(&mut app, &cmd("folder.new"), String::new);
        assert!(app.session.compose.is_none(), "a reply was started from {module}");
        assert!(app.dialog.is_none(), "a dialog opened from {module}");
        assert_eq!(app.session.mailbox.message(id).map(|m| m.folder), folder, "deleted from {module}");
    }
    // Commands of no module still run anywhere.
    assert!(enabled(&app, &cmd("view.mail")));
    run(&mut app, &cmd("view.mail"), String::new);
    assert_eq!(app.session.view.module, mailcraft_engine::Module::Mail);
    assert!(enabled(&app, &cmd("mail.reply")));
}

/// #253 review: while File or a dialog is open the window takes no command keys, and the menu bar
/// runs nothing but the Edit items, which the dialog's text fields need.
#[test]
fn nothing_runs_from_the_menu_while_file_or_a_dialog_is_open() {
    let (mut app, _) = sample();
    assert!(crate::keys::takes_commands(&app));
    app.ui.backstage = true;
    assert!(!crate::keys::takes_commands(&app));
    for c in ["mail.reply", "mail.new", "view.calendar", "view.search"] {
        assert!(!enabled(&app, &cmd(c)), "{c} with File open");
    }
    assert!(!enabled(&app, &Own::Settings.action()));
    assert!(enabled(&app, &Action::Edit(EditKey::Paste)));
    run(&mut app, &cmd("mail.reply"), String::new);
    assert!(app.session.compose.is_none() && app.ui.backstage, "File was closed and a reply started");

    app.ui.backstage = false;
    app.run("folder.new", serde_json::json!({})).unwrap();
    assert!(app.dialog.is_some(), "New Folder asks for a name");
    assert!(!crate::keys::takes_commands(&app));
    assert!(!enabled(&app, &cmd("mail.new")));
    assert!(enabled(&app, &Action::Edit(EditKey::Copy)));
    run(&mut app, &cmd("mail.new"), String::new);
    assert!(app.session.compose.is_none(), "a message was started under the New Folder dialog");
    assert!(app.dialog.is_some());
}
