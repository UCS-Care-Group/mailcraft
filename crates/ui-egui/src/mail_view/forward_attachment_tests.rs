// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forward as Attachment in the window (#315): in a message's right-click menu after Forward,
//! under Forward's `▾` in Home › Respond, and on Ctrl+Alt+F; each opens the new message with the
//! message attached as an `.eml`, and with several selected, every one of them (#360).

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const MOCKUPS: &str = "Homepage mockups, round three";

type Window = egui_kittest::Harness<'static, MailApp>;

fn window() -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

fn mockups(harness: &Window) -> Id {
    harness.state().session.mailbox.messages.values().find(|m| m.subject == MOCKUPS).unwrap().id
}

fn press(harness: &mut Window, pos: egui::Pos2, button: egui::PointerButton) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

/// The buttons whose names hold `label` (a menu item's name ends with its key).
fn items<'h>(harness: &'h Window, label: &'h str) -> impl Iterator<Item = egui_kittest::Node<'h>> {
    harness.query_all(egui_kittest::kittest::By::new().role(Role::Button).label_contains(label))
}

fn item<'h>(harness: &'h Window, label: &'h str) -> egui_kittest::Node<'h> {
    let mut found = items(harness, label);
    let item = found.next().unwrap_or_else(|| panic!("no {label}"));
    assert!(found.next().is_none(), "one {label}");
    item
}

/// The message being written forwards `id` as an attachment.
fn forwards_as_attachment(harness: &Window, id: Id) -> bool {
    harness.state().session.compose.as_ref().is_some_and(|c| {
        c.source == Some(id)
            && c.subject == format!("FW: {MOCKUPS}")
            && matches!(c.attachments.as_slice(), [eml] if eml.name == format!("{MOCKUPS}.eml") && eml.mime == "message/rfc822")
    })
}

#[test]
fn the_right_click_menu_has_it_after_forward() {
    let mut harness = window();
    let id = mockups(&harness);
    let row = harness.state().hit.rect("message", id).unwrap_or_else(|| panic!("no row for {id}")).center();
    press(&mut harness, row, egui::PointerButton::Secondary);
    // The menu's items, top to bottom (the ribbon's and the reading pane's buttons are icons).
    let tops = |h: &Window, label: &str| {
        let mut tops: Vec<f32> = items(h, label).map(|n| n.rect().top()).collect();
        tops.sort_by(f32::total_cmp);
        tops
    };
    let (forward, unread) = (tops(&harness, "Forward"), tops(&harness, "Mark as Unread"));
    assert_eq!((forward.len(), unread.len()), (2, 1), "Forward and Forward as Attachment, once each");
    assert_eq!(tops(&harness, "Forward as Attachment"), [forward[1]]);
    assert!(forward[0] < forward[1] && forward[1] < unread[0], "{forward:?} {unread:?}");
    item(&harness, "Forward as Attachment").click();
    harness.run_steps(4);
    assert!(forwards_as_attachment(&harness, id));
}

#[test]
fn forwards_drop_down_and_its_key_have_it() {
    let mut harness = window();
    let id = mockups(&harness);
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(3);
    let drop = harness.state().hit.rect("ribbonMenu", crate::ribbon::hit_id("mail.forward")).expect("Forward has a ▾").center();
    press(&mut harness, drop, egui::PointerButton::Primary);
    assert!(harness.state().session.compose.is_none(), "the ▾ doesn't forward");
    item(&harness, "Forward as Attachment").click();
    harness.run_steps(4);
    assert!(forwards_as_attachment(&harness, id));

    harness.state_mut().execute("compose.close", json!({})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(3);
    let ctrl_alt = egui::Modifiers { alt: true, ctrl: !cfg!(target_os = "macos"), mac_cmd: cfg!(target_os = "macos"), command: true, shift: false };
    harness.key_press_modifiers(ctrl_alt, egui::Key::F);
    harness.run_steps(4);
    assert!(forwards_as_attachment(&harness, id), "Ctrl+Alt+F");
}

/// The right-click menu and Forward's `▾` have a name for each item, for a screen reader (what the
/// window had without them is left as it was).
#[test]
fn what_forward_as_attachment_adds_has_names() {
    let mut harness = window();
    let id = mockups(&harness);
    let before = harness.unnamed_widgets();
    let added = |h: &Window| h.unnamed_widgets().into_iter().filter(|w| !before.contains(w)).collect::<Vec<_>>();
    let row = harness.state().hit.rect("message", id).unwrap().center();
    press(&mut harness, row, egui::PointerButton::Secondary);
    item(&harness, "Forward as Attachment");
    assert_eq!(added(&harness), Vec::<String>::new(), "the right-click menu");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(3);
    let drop = harness.state().hit.rect("ribbonMenu", crate::ribbon::hit_id("mail.forward")).unwrap().center();
    press(&mut harness, drop, egui::PointerButton::Primary);
    item(&harness, "Forward as Attachment");
    assert_eq!(added(&harness), Vec::<String>::new(), "Forward's drop-down");
}

/// A message whose server's copy went before it could be fetched never will be: the command
/// says so, instead of the window waiting for a download that can't come.
#[test]
fn a_message_that_will_never_download_is_refused_not_waited_for() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let id = app.session.mailbox.messages.values().find(|m| m.subject == MOCKUPS).unwrap().id;
    let m = app.session.mailbox.message_mut(id).unwrap();
    m.server = Some(mailcraft_model::sync::ServerMessage { id: "AAMk1".into(), ..Default::default() });
    (m.unfetched, m.incomplete) = (true, true);
    let e = app.run("mail.forwardAsAttachment", json!({"id": id})).unwrap_err();
    assert!(e.contains("only the headers"), "{e}");
    // The other commands that need it whole have no word of their own for it: they wait as before,
    // never running on what stands in for it.
    for command in ["mail.forward", "file.exportMessage"] {
        let e = app.run(command, json!({"id": id})).unwrap_err();
        assert!(e.contains("It will download when you're online"), "{command}: {e}");
    }
    assert!(app.session.compose.is_none());
    // Still to be fetched, it is waited for (here, with nothing to fetch it, offline).
    app.session.mailbox.message_mut(id).unwrap().incomplete = false;
    let e = app.run("mail.forwardAsAttachment", json!({"id": id})).unwrap_err();
    assert!(e.contains("It will download when you're online"), "{e}");
}

/// With several selected (#360), the right-click menu's Forward and Forward as Attachment, Forward's
/// `▾` and Ctrl+Alt+F each open one new message with every one of them attached.
#[test]
fn with_several_selected_each_way_attaches_them_all() {
    let mut harness = window();
    let subject = |h: &Window, s: &str| h.state().session.mailbox.messages.values().find(|m| m.subject == s).unwrap().id;
    let ids = [MOCKUPS, "Q4 roadmap review moved to Thursday", "Your October invoice is ready"].map(|s| subject(&harness, s));
    let all_attached = |h: &Window| {
        h.state().session.compose.as_ref().is_some_and(|c| {
            c.as_attachment && c.attachments.len() == 3 && c.attachments.iter().all(|a| a.mime == "message/rfc822") && c.source == Some(ids[2])
        })
    };
    let select = |h: &mut Window| {
        h.state_mut().execute("compose.close", json!({"discard": true})).ok();
        h.state_mut().execute("mail.select", json!({"ids": ids})).unwrap();
        h.run_steps(3);
    };
    for label in ["Forward as Attachment", "Forward"] {
        select(&mut harness);
        // A right click on one of the selected keeps them all selected.
        let row = harness.state().hit.rect("message", ids[0]).unwrap().center();
        press(&mut harness, row, egui::PointerButton::Secondary);
        assert_eq!(harness.state().session.sel.messages, ids, "{label}");
        // (Forward is the first of the two whose names hold "Forward".)
        let node = items(&harness, label).min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()));
        node.unwrap_or_else(|| panic!("no {label}")).click();
        harness.run_steps(4);
        assert!(all_attached(&harness), "the right-click menu's {label}");
    }
    select(&mut harness);
    let drop = harness.state().hit.rect("ribbonMenu", crate::ribbon::hit_id("mail.forward")).expect("Forward has a ▾").center();
    press(&mut harness, drop, egui::PointerButton::Primary);
    item(&harness, "Forward as Attachment").click();
    harness.run_steps(4);
    assert!(all_attached(&harness), "Forward's ▾");
    select(&mut harness);
    let ctrl_alt = egui::Modifiers { alt: true, ctrl: !cfg!(target_os = "macos"), mac_cmd: cfg!(target_os = "macos"), command: true, shift: false };
    harness.key_press_modifiers(ctrl_alt, egui::Key::F);
    harness.run_steps(4);
    assert!(all_attached(&harness), "Ctrl+Alt+F");
}
