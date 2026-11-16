// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1511 in the window: changing how the message list is arranged (Arrange By, the arrow that
//! turns it over, conversations, groups, single line, preview lines, the ready-made views, tighter
//! spacing, Reset View) scrolls the list to the selected message and puts it in the middle,
//! instead of leaving it where it was; with no message selected nothing scrolls. The control channel and MCP run the same
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
const HEIGHT: f32 = 560.0;

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
        ("view.changeView", json!({"value": "preview"})),
        ("view.density", json!({"value": true})),
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

/// The message list's own top and bottom edge: under its header, above the status bar.
fn list_edges(h: &Harness<'static, MailApp>) -> (f32, f32) {
    let top = h.state().hit.rect("listSort", 0).map_or(0.0, |r| r.max.y);
    (top, HEIGHT - crate::chrome::STATUS_BAR_H)
}

/// The row of `id` is drawn whole inside the list.
fn in_list(h: &Harness<'static, MailApp>, id: Id) -> bool {
    let (top, bottom) = list_edges(h);
    h.state().hit.rect("message", id).is_some_and(|r| r.min.y >= top - 0.5 && r.max.y <= bottom + 0.5)
}

/// The row with the focus is drawn, inside the list.
fn focus_in_view(h: &Harness<'static, MailApp>) -> bool {
    h.state().session.sel.focus().is_some_and(|f| in_list(h, f))
}

/// The row with the focus is in the middle of the list, or as near as the scroll range allows:
/// the list's first row at its top, or its last row at its foot, with the row on that side.
fn centred(h: &mut Harness<'static, MailApp>) -> bool {
    let rows: Vec<Id> = h.state_mut().session.rows().iter().map(|r| r.id).collect();
    let app = h.state();
    let (top, bottom) = list_edges(h);
    let Some(row) = app.session.sel.focus().and_then(|f| app.hit.rect("message", f)) else { return false };
    if !focus_in_view(h) {
        return false;
    }
    let (mid, slack) = ((top + bottom) / 2.0, row.height() * 1.5);
    if (row.center().y - mid).abs() <= slack {
        return true;
    }
    let at_start = rows.first().and_then(|id| app.hit.rect("message", *id)).is_some_and(|r| r.min.y <= top + 40.0);
    let at_end = rows.last().and_then(|id| app.hit.rect("message", *id)).is_some_and(|r| r.max.y >= bottom - 40.0 && r.max.y <= bottom + 0.5);
    (at_start && row.center().y < mid) || (at_end && row.center().y > mid)
}

/// Whether a row is drawn at all, and so in view.
fn drawn(h: &Harness<'static, MailApp>, id: Id) -> bool {
    in_list(h, id)
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
        a.reveal_centered = false;
        a.execute(id, params.clone()).unwrap();
        assert!(a.reveal_selection && a.reveal_centered, "{id} {params}");
    }
    let mut a = app();
    a.run("mail.first", json!({})).unwrap();
    a.reveal_selection = false;
    a.reveal_centered = false;
    assert!(a.execute("view.sort", json!({"by": "nonsense"})).is_err());
    assert!(!a.reveal_selection && !a.reveal_centered, "a sort that was refused moved nothing");
    assert!(a.execute("view.filter", json!({"value": "all"})).is_ok());
    assert!(!a.reveal_selection && !a.reveal_centered, "a command that does not arrange the list leaves it where it is");
}

/// A message from the middle of the new arrangement is selected while the list is scrolled to its
/// end: the list goes to it and puts it in the middle (or as near as it can scroll).
#[test]
fn each_arrangement_scrolls_to_the_selected_message_and_centres_it() {
    for (id, params) in changes() {
        let mut h = window();
        let ids = at_the_end(&mut h);
        let order = order_after(id, &params);
        let pick = *order.get(order.len() / 2).unwrap();
        assert!(ids.contains(&pick), "{id}: the message is in the Inbox");
        // (Selecting it from outside the list's keys doesn't scroll to it.)
        h.state_mut().execute("mail.select", json!({"id": pick})).unwrap();
        h.run_steps(3);
        h.state_mut().execute(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), Some(pick), "{id} {params}: the selection stays");
        assert!(centred(&mut h), "{id} {params}: the selected message is in the middle of the list");
    }
}

/// What the menus do: the arrangement's menu and the arrow (`run`, as a click does).
#[test]
fn the_menu_and_the_arrow_centre_the_selected_message() {
    for (id, params) in [("view.sort", json!({"by": "from"})), ("view.reverseSort", json!({})), ("view.sort", json!({"by": "size"}))] {
        let mut h = window();
        let ids = at_the_end(&mut h);
        let order = order_after(id, &params);
        let pick = *order.get(order.len() / 2).unwrap();
        assert!(ids.contains(&pick));
        h.state_mut().execute("mail.select", json!({"id": pick})).unwrap();
        h.run_steps(3);
        h.state_mut().run(id, params.clone()).unwrap();
        h.run_steps(4);
        assert!(centred(&mut h), "{id} {params}");
    }
}

/// The list follows the selection through one change after another, in both directions.
#[test]
fn a_second_change_keeps_the_selection_in_the_middle_again() {
    let mut h = window();
    at_the_end(&mut h);
    let target = h.state().session.sel.focus().unwrap();
    for (id, params) in changes().into_iter().filter(|(id, _)| id.starts_with("view.sort") || *id == "view.reverseSort") {
        h.state_mut().run(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), Some(target), "{id} {params}");
        assert!(centred(&mut h), "{id} {params}: in the middle after this change too");
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
    assert!(centred(&mut h), "the first message went to the end of the list, and the list with it");
}

/// A move from the keyboard still scrolls only as far as it must: the row comes in at the edge,
/// not in the middle (the rearranging's centring is not the keys').
#[test]
fn a_move_from_the_keyboard_still_scrolls_to_the_edge() {
    let mut h = window();
    h.state_mut().run("mail.first", json!({})).unwrap();
    h.run_steps(3);
    press_end(&mut h);
    let focus = h.state().session.sel.focus().unwrap();
    let (top, bottom) = list_edges(&h);
    let row = h.state().hit.rect("message", focus).unwrap();
    assert!(row.max.y >= bottom - row.height() - 1.0, "the last row sits at the foot of the list, not in its middle: {row:?} {top} {bottom}");
}

/// With no message selected nothing is scrolled to: a list scrolled down stays scrolled down
/// (sorting only; the other changes move the rows too little to tell), and nothing panics.
#[test]
fn with_nothing_selected_the_list_stays_where_it_is() {
    for (id, params) in changes() {
        let mut h = window();
        at_the_end(&mut h);
        h.state_mut().session.sel.messages.clear();
        h.run_steps(3);
        assert_eq!(h.state().session.sel.focus(), None, "nothing is selected");
        h.state_mut().execute(id, params.clone()).unwrap();
        h.run_steps(4);
        assert_eq!(h.state().session.sel.focus(), None, "{id}: still nothing selected");
        if id == "view.sort" || id == "view.reverseSort" {
            let top = h.state_mut().session.rows().first().map(|r| r.id).unwrap();
            assert!(!drawn(&h, top), "{id} {params}: the list did not jump to its top");
        }
    }
}
