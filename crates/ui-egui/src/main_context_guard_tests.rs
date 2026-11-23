// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Copy behind an appointment's own window: while one of the Calendar's items is open in an
//! embedded appointment form, that form's window is the active one (`keys::active` is not the
//! main layer), so `ui.calendarCopy` must be disabled and must refuse even though no dialog, no
//! agent question, no File page and no menu is open — and the clipboard the Calendar made, the
//! mailbox and the selection must be exactly as they were afterwards.
//!
//! One module of `calendar_selection_tests`, which is itself a child of `calendar_view`:
//!
//! `super::*` is that file's fixtures (`calendar`, `event`, `select`, `selected`); `super::super`
//! is `calendar_view`'s own `Clip` and `CLIP_ID`. The window is the real one, the appointment is
//! the sample's invented local `Lunch with Dev`, the clock is fixed, and the only acts are the two
//! commands, frames, and reads of what the window holds.

use std::sync::Arc;

use serde_json::json;

use super::super::{CLIP_ID, Clip};
use super::*;

/// The Calendar's clipboard, as `calendar_view::clip`/`take_for_paste` stored it.
fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

#[test]
fn copy_is_refused_while_an_appointment_window_is_the_active_one() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    let dentist = event(&w, "Dentist");
    // Fixture: two distinct items of this computer's own, and the one opened below is not a
    // series (so `ui.openWindow` opens its form directly, with no recurrence question).
    assert_ne!(lunch.id, dentist.id, "fixture: two distinct items");
    assert!(lunch.recurrence.is_none(), "fixture: a local appointment, not a series");
    select(&mut w, &[(lunch.id, None), (dentist.id, None)]);
    assert_eq!(selected(&w), [(lunch.id, None), (dentist.id, None)], "fixture: the typed set is selected");

    // In the Calendar in front Copy works: the functional precondition this regression needs.
    let answer = w.state_mut().execute("ui.calendarCopy", json!({})).expect("ui.calendarCopy is a Calendar command");
    assert_eq!(answer, json!({ "items": 2, "cut": false }), "fixture: Copy took the selected set");
    w.run_steps(2);

    // The clipboard it made, kept for the refusal below to leave untouched.
    let prior = clipboard(&w).expect("Copy left the Calendar's clipboard");
    let prior_items: Vec<_> = prior.items.iter().map(|item| (item.id, item.day)).collect();
    assert_eq!(prior_items, selected(&w), "fixture: the clipboard is the selected set");
    assert!(!prior.cut, "fixture: Copy made a copy");
    assert_eq!(prior.opening, w.state().session.openings(), "fixture: the clipboard is this mailbox's");
    let prior_sources = prior.sources.clone();

    // The owner opens one of them: a real embedded appointment window over the Calendar.
    let opened = w.state_mut().execute("ui.openWindow", json!({ "event": lunch.id })).expect("an appointment opens in a window of its own");
    let window = opened["window"].as_u64().expect("a window number");
    w.run_steps(5);

    // Fixture: the window is still the Calendar's, with the appointment's form active over it,
    // and none of the guards the command already has is the one refusing — so the missing guard
    // is the active layer, not a dialog, a question, a File page or a menu.
    let ctx = w.ctx.clone();
    assert_eq!(w.state().session.view.module, mailcraft_engine::Module::Calendar, "fixture: the Calendar is still the module");
    assert!(w.state().dialog.is_none(), "fixture: no dialog");
    assert!(w.state().asking.is_empty(), "fixture: no agent question");
    assert!(!w.state().ui.backstage, "fixture: no File page");
    assert!(!egui::Popup::is_any_open(&ctx), "fixture: no menu or list");
    let active = crate::keys::active(&ctx);
    assert_ne!(active, crate::keys::main_layer(), "fixture: the appointment's window is the active one");
    assert_eq!(active, crate::windows::embedded_layer(window), "fixture: the active layer is that very window");
    assert_eq!(w.state().windows.len(), 1, "fixture: one appointment window is open");

    // What must survive the refusal, and the set it would take if it were wrongly let through.
    let mailbox = w.state().session.mailbox.clone();
    let selection = selected(&w);
    assert_eq!(selection, [(lunch.id, None), (dentist.id, None)], "fixture: the set is still selected behind the window");

    // THE GUARD: the Calendar's Copy is not offered, and run directly it refuses.
    assert!(!w.state().enabled("ui.calendarCopy"), "the Calendar's Copy is disabled while an appointment window is the active one");
    let refused = w.state_mut().execute("ui.calendarCopy", json!({}));
    assert!(refused.is_err(), "ui.calendarCopy refuses from a Calendar behind an appointment window: {refused:?}");
    w.run_steps(2);

    // A refusal clobbers nothing: the same clipboard (the same shared source events), the same
    // mailbox and the same selection.
    let after = clipboard(&w).expect("the refused Copy left the prior clipboard in place");
    assert_eq!(after.items, prior.items, "the prior clipboard's items are untouched");
    let (before, now) = ((prior.cut, prior.text.as_str(), prior.opening), (after.cut, after.text.as_str(), after.opening));
    assert_eq!(now, before, "the prior clipboard's cut flag, text and mailbox are untouched");
    assert_eq!(after.sources.len(), prior_sources.len(), "the prior clipboard keeps one source per item");
    assert!(
        after.sources.iter().zip(&prior_sources).all(|(now, was)| Arc::ptr_eq(now, was)),
        "the prior clipboard still points at the same source events"
    );
    assert!(w.state().session.mailbox == mailbox, "the mailbox is untouched");
    assert_eq!(selected(&w), selection, "the selection is untouched");
}
