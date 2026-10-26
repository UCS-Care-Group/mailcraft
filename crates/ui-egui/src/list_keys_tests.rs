// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #426 in the window: Up, Down, Home, End, Page Up and Page Down move in the message list, with
//! Shift they select a range, a held key moves a row for each repeat, the row with the focus stays
//! in view, and none of them is the list's while the keyboard is in the search box.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// A window short enough for the sample's Inbox to scroll.
const HEIGHT: f32 = 560.0;

/// The sample mailbox, in the Inbox, with the first message selected.
fn window() -> (Harness<'static, MailApp>, Vec<Id>) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.run("folder.select", json!({"kind": "inbox"})).unwrap();
    app.run("mail.first", json!({})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, HEIGHT)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let ids: Vec<Id> = h.state_mut().session.rows().iter().map(|r| r.id).collect();
    assert!(ids.len() >= 8, "the sample's Inbox has rows enough: {}", ids.len());
    (h, ids)
}

fn press(h: &mut Harness<'static, MailApp>, key: Key, modifiers: Modifiers, repeats: usize) {
    h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    for _ in 0..repeats {
        h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: true, modifiers });
        h.step();
    }
    h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(3);
}

fn key(h: &mut Harness<'static, MailApp>, k: Key) {
    press(h, k, Modifiers::NONE, 0);
}

fn shift(h: &mut Harness<'static, MailApp>, k: Key) {
    press(h, k, Modifiers::SHIFT, 0);
}

fn selected(h: &Harness<'static, MailApp>) -> Vec<Id> {
    h.state().session.sel.messages.clone()
}

fn focus_at(h: &Harness<'static, MailApp>, ids: &[Id]) -> Option<usize> {
    let focus = h.state().session.sel.focus();
    ids.iter().position(|id| Some(*id) == focus)
}

fn sorted(mut v: Vec<Id>) -> Vec<Id> {
    v.sort_unstable();
    v
}

/// The row with the focus is drawn, under the list's header and inside the window.
fn focus_in_view(h: &Harness<'static, MailApp>) -> bool {
    let app = h.state();
    let Some(focus) = app.session.sel.focus() else { return false };
    let (Some(row), Some(header)) = (app.hit.rect("message", focus), app.hit.rect("listSort", 0)) else { return false };
    row.min.y >= header.max.y - 0.5 && row.max.y <= HEIGHT
}

#[test]
fn the_arrows_move_and_with_shift_select_a_range() {
    let (mut h, ids) = window();
    key(&mut h, Key::ArrowDown);
    assert_eq!(selected(&h), vec![ids[1]]);
    shift(&mut h, Key::ArrowDown);
    shift(&mut h, Key::ArrowDown);
    assert_eq!(selected(&h), ids[1..=3].to_vec());
    shift(&mut h, Key::ArrowUp);
    assert_eq!(selected(&h), ids[1..=2].to_vec(), "Shift+Up takes the last one out again");
    key(&mut h, Key::ArrowUp);
    assert_eq!(selected(&h), vec![ids[1]], "Up without Shift is one message");
}

#[test]
fn home_end_and_the_page_keys_move_and_with_shift_select() {
    let (mut h, ids) = window();
    let end = ids.len() - 1;
    let page = h.state().list_page_rows;
    assert!(page > 1 && page < ids.len(), "a page is some rows of the list: {page}");
    key(&mut h, Key::End);
    assert_eq!(selected(&h), vec![ids[end]]);
    assert!(focus_in_view(&h), "End scrolls to the last row");
    key(&mut h, Key::Home);
    assert_eq!(selected(&h), vec![ids[0]]);
    assert!(focus_in_view(&h), "Home scrolls back to the first");
    key(&mut h, Key::PageDown);
    assert_eq!(focus_at(&h, &ids), Some(page.min(end)));
    assert!(focus_in_view(&h));
    key(&mut h, Key::PageUp);
    assert_eq!(focus_at(&h, &ids), Some(0));
    key(&mut h, Key::ArrowDown);
    shift(&mut h, Key::End);
    assert_eq!(selected(&h), ids[1..].to_vec(), "Shift+End: from the anchor to the last");
    assert!(focus_in_view(&h));
    shift(&mut h, Key::Home);
    assert_eq!(sorted(selected(&h)), sorted(ids[..=1].to_vec()), "Shift+Home: from the anchor to the first");
    key(&mut h, Key::ArrowDown);
    shift(&mut h, Key::PageDown);
    assert_eq!(sorted(selected(&h)), sorted(ids[1..=(1 + page).min(end)].to_vec()), "Shift+Page Down");
    assert!(focus_in_view(&h));
    shift(&mut h, Key::PageUp);
    assert_eq!(selected(&h), vec![ids[1]], "Shift+Page Up back to the anchor");
}

#[test]
fn a_held_key_moves_a_row_for_each_repeat_and_the_focus_stays_in_view() {
    let (mut h, ids) = window();
    let n = ids.len() - 2;
    press(&mut h, Key::ArrowDown, Modifiers::NONE, n - 1);
    assert_eq!(focus_at(&h, &ids), Some(n));
    assert_eq!(selected(&h).len(), 1);
    assert!(focus_in_view(&h), "the list follows the focus down");
    press(&mut h, Key::ArrowUp, Modifiers::SHIFT, 2);
    assert_eq!(focus_at(&h, &ids), Some(n - 3));
    assert_eq!(selected(&h).len(), 4, "a held Shift+Up selects a row for each repeat");
    assert!(focus_in_view(&h));
}

#[test]
fn not_while_typing_in_the_search_box() {
    let (mut h, _) = window();
    h.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.ctx.memory(|m| m.has_focus(crate::chrome::search_box_id())));
    let before = selected(&h);
    for k in [Key::ArrowDown, Key::ArrowUp, Key::Home, Key::End, Key::PageDown, Key::PageUp] {
        key(&mut h, k);
        shift(&mut h, k);
        assert_eq!(selected(&h), before, "{k:?} is the search box's");
    }
}
