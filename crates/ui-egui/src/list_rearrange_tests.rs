// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1511 in the window: changing how the message list is arranged (Arrange By, the arrow that
//! turns it over, conversations, groups, single line, preview lines, Reset View) scrolls the list
//! to the selected message, as the keys that move in the list do, instead of leaving it where it
//! was; with no message selected nothing scrolls. The control channel and MCP run the same
//! commands through `execute`, so that is what the first test holds to it.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// A window short enough for the sample's Inbox to scroll, in every layout the tests use.
const HEIGHT: f32 = 300.0;

/// Every way the list is arranged or laid out, with what the command is given.
fn changes() -> Vec<(&'static str, Value)> {
    vec![
        ("view.sort", json!({"by": "from"})),
        ("view.sort", json!({"by": "subject"})),
        ("view.sort", json!({"by": "size"})),
        ("view.sort", json!({"by": "importance"})),
        ("view.sort", json!({"by": "categories"})),
        ("view.sort", json!({"by": "date", "descending": false})),
        ("view.sort", json!({"by": "from", "descending": true})),
        ("view.reverseSort", json!({})),
        ("view.conversations", json!({})),
        ("view.groups", json!({"value": false})),
        ("view.oneLine", json!({"value": true})),
        ("view.previewLines", json!({"value": 3})),
        ("view.reset", json!({})),
    ]
}

/// The sample mailbox, in the Inbox, nothing selected.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.run("folder.select", json!({"kind": "inbox"})).unwrap();
    app
}

fn window() -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, HEIGHT)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app(),
    );
    h.run_steps(4);
    h
}

/// The rows' messages in the order the list has after `id` ran, found in a window of its own.
fn order_after(id: &str, params: &Value) -> Vec<Id> {
    let mut a = app();
    a.execute(id, params.clone()).unwrap();
    a.session.rows().iter().map(|r| r.id).collect()
}

fn press_end(h: &mut Harness<'static, MailApp>) {
    h.event(egui::Event::Key { key: Key::End, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::Key { key: Key::End, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// The row with the focus is drawn, under the list's header and inside the window.
fn focus_in_view(h: &Harness<'static, MailApp>) -> bool {
    let app = h.state();
    let Some(focus) = app.session.sel.focus() else { return false };
    let (Some(row), Some(header)) = (app.hit.rect("message", focus), app.hit.rect("listSort", 0)) else { return false };
    row.min.y >= header.max.y - 0.5 && row.max.y <= HEIGHT
}

/// Whether a row is drawn at all, and so in view.
fn drawn(h: &Harness<'static, MailApp>, id: Id) -> bool {
    h.state().hit.rect("message", id).is_some_and(|r| r.max.y <= HEIGHT)
}

/// The list is scrolled to its end with the last message selected.
fn at_the_end(h: &mut Harness<'static, MailApp>) -> Vec<Id> {
    let ids: Vec<Id> = h.state_mut().session.rows().iter().map(|r| r.id).collect();
    assert!(ids.len() >= 8, "the sample's Inbox has rows enough: {}", ids.len());
    press_end(h);
    assert!(focus_in_view(h), "End scrolls to the last row");
    assert!(!drawn(h, ids[0]), "the first row is out of view, the list is scrolled down");
    ids
}

/// The command reaches the list through `execute` as through the window's menus: whoever runs
/// it, a message is selected and the list asks to scroll to it. One that is refused doesn't.
#[test]
fn every_arrangement_asks_the_list_to_scroll_to_the_selection_and_a_refused_one_does_not() {
    for (id, params) in changes() {
        let mut a = app();
        a.run("mail.first", json!({})).unwrap();
        a.reveal_selection = false;
        a.execute(id, params.clone()).unwrap();
        assert!(a.reveal_selection, "{id} {params}");
    }
    let mut a = app();
    a.run("mail.first", json!({})).unwrap();
    a.reveal_selection = false;
    assert!(a.execute("view.sort", json!({"by": "nonsense"})).is_err());
    assert!(!a.reveal_selection, "a sort that was refused moved nothing");
    assert!(a.execute("view.filter", json!({"value": "all"})).is_ok());
    assert!(!a.reveal_selection, "a command that does not arrange the list leaves it where it is");
}

/// The message that comes first in the new arrangement is selected while the list is scrolled to
/// its end: it is out of view there, and the list goes to it.
#[test]
fn each_arrangement_scrolls_to_the_selected_message() {
    for (id, params) in changes() {
        let mut h = window();
        let ids = at_the_end(&mut h);
        let first = *order_after(id, &params).first().unwrap();
        assert!(ids.contains(&first), "{id}: the message is in the Inbox");
        // (Selecting it from outside the list's keys doesn't scroll to it.)
        h.state_mut().execute("mail.select", json!({"id": first})).unwrap();
        h.run_steps(3);
        h.state_mut().execute(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), Some(first), "{id} {params}: the selection stays");
        assert!(focus_in_view(&h), "{id} {params}: the selected message is in view");
    }
}

/// What the menus do: the arrangement's menu and the arrow (`run`, as a click does).
#[test]
fn the_menu_and_the_arrow_scroll_to_the_selected_message() {
    for (id, params) in [("view.sort", json!({"by": "from"})), ("view.reverseSort", json!({})), ("view.sort", json!({"by": "size"}))] {
        let mut h = window();
        let ids = at_the_end(&mut h);
        let first = *order_after(id, &params).first().unwrap();
        assert!(ids.contains(&first));
        h.state_mut().execute("mail.select", json!({"id": first})).unwrap();
        h.run_steps(3);
        h.state_mut().run(id, params.clone()).unwrap();
        h.run_steps(4);
        assert!(focus_in_view(&h), "{id} {params}");
    }
}

/// The list follows the selection through one change after another, in both directions.
#[test]
fn a_second_change_keeps_the_selection_in_view_again() {
    let mut h = window();
    at_the_end(&mut h);
    let target = h.state().session.sel.focus().unwrap();
    for (id, params) in changes().into_iter().filter(|(id, _)| id.starts_with("view.sort") || *id == "view.reverseSort") {
        h.state_mut().run(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), Some(target), "{id} {params}");
        assert!(focus_in_view(&h), "{id} {params}: in view after this change too");
    }
    // From the first row to the end of the list: the message that went there is in view.
    h.state_mut().run("view.sort", json!({"by": "date", "descending": true})).unwrap();
    h.run_steps(4);
    h.state_mut().run("mail.first", json!({})).unwrap();
    h.run_steps(4);
    let first = h.state().session.sel.focus().unwrap();
    h.state_mut().run("view.reverseSort", json!({})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().session.sel.focus(), Some(first));
    assert!(focus_in_view(&h), "the first message went to the end of the list, and the list with it");
}

/// With no message selected the list stays at its top.
#[test]
fn with_nothing_selected_the_list_stays_at_the_top() {
    for (id, params) in changes() {
        let mut h = window();
        assert_eq!(h.state().session.sel.focus(), None, "nothing is selected");
        h.state_mut().execute(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), None, "{id}: still nothing selected");
        let top = h.state_mut().session.rows().first().map(|r| r.id).unwrap();
        assert!(drawn(&h, top), "{id} {params}: the list is at its top");
    }
}
