// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mixed owner/guest Delete question in the smallest window MailCraft opens (760 × 480), in
//! both palettes: every answer it draws — the primary, the one alternate and Cancel — is whole
//! inside the window and inside the question's own available area, and Cancel changes nothing.
//!
//! One module of `calendar_mixed_guest_tests`, which is itself a child of
//! `calendar_review_regression_tests` (`review_regressions`), so `super::mixed_window` is that
//! file's private fixture, `super::super::*` (one level above) is
//! the review file's `open_question`, `press` and `select_set`, and `super::super::super::*` is
//! `calendar_selection_tests`'s `click`, `chord`, `drawn` and `selected`. Nothing here is
//! production code or a test hook: the window is the real one, the two appointments are synthetic
//! (`example.org`), the clock is fixed, and every act is a key or a button the owner would press.
//! No screenshot, pixel, native window or network is used.
//!
//! WHAT IS HELD (source read 2026-11-15): a mixed set (the owner's whole meeting and a
//! guest copy) is asked about once with a primary, exactly one alternate and Cancel; at
//! `more.len() == 1` the footer is the one-row layout whose labels are read from the question, and
//! reach is a rectangle question, not a caption one. This holds the contract; it does not force a
//! failure: if every button is drawn whole, it is a genuine PASS baseline. The current English
//! alternate is 44 characters.

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;

use super::super::super::{chord, click, drawn, selected};
use super::super::{open_question, press, select_set};
use super::mixed_window;
use crate::people_view::testing::Window;

/// The lowest button of the window that says exactly `label`: the question is drawn over the
/// window, so where the ribbon has a button of the same name the question's is the lower one.
fn button_rect(w: &Window, label: &str) -> Option<egui::Rect> {
    let mut found: Vec<egui::Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .filter(|node| node.accesskit_node().label().is_some_and(|said| said.trim() == label))
        .map(|node| node.rect())
        .collect();
    found.sort_by(|a, b| b.min.y.total_cmp(&a.min.y));
    found.first().copied()
}

#[test]
fn a_mixed_delete_question_keeps_every_answer_in_reach_at_the_smallest_window() {
    // The default question says `Cancel` when it names no other way of going nowhere; hold the
    // window's language so that the label read from the question is the one the button draws.
    let _language = crate::i18n::RestoreLanguage::hold();
    crate::i18n::set_current(crate::i18n::Lang::English);
    for theme in ["light", "dark"] {
        // A fresh window for each palette: the sample's `Lunch with Dev` is the owner's own
        // meeting and `Dentist` is somebody else's copy, each kept by the same invented account.
        let (mut w, owned, guest) = mixed_window();
        w.state_mut().execute("ui.theme", json!({ "value": theme })).expect("the window takes the theme");
        w.run_steps(3);
        // Give the calendar the keys before narrowing it: the sample's full title is not drawn
        // at the minimum width. The question itself is still opened and tested at 760 x 480.
        let on_review = drawn(&w, "Design review").center();
        click(&mut w, on_review, egui::Modifiers::NONE, egui::PointerButton::Primary);
        w.set_size(egui::vec2(760.0, 480.0));
        w.run_steps(3);

        // The typed set through the engine's own command, as a Command click of each leaves it.
        select_set(&mut w, &[owned.id, guest.id]);
        assert_eq!(selected(&w), [(owned.id, None), (guest.id, None)], "{theme}: fixture: the owner's meeting and the guest copy are the typed set");

        let before = w.state().session.mailbox.clone();
        let picked_before = selected(&w);
        chord(&mut w, egui::Modifiers::NONE, egui::Key::Delete);
        w.run_steps(3);
        let asked = open_question(&w, &format!("{theme}: a Delete of the owner's meeting and a guest copy must ask before either goes"));
        assert_eq!(asked.command, "calendar.delete", "{theme}: the question is about the Delete");
        assert!(!asked.action.trim().is_empty(), "{theme}: the primary answer has a caption");
        assert_eq!(asked.more.len(), 1, "{theme}: the mixed question offers exactly one alternate answer");
        assert!(asked.more.iter().all(|(label, _)| !label.trim().is_empty()), "{theme}: the alternate answer has a caption");
        assert!(!asked.cancel.trim().is_empty(), "{theme}: the way of going nowhere has a caption");
        assert_ne!(asked.action, asked.more[0].0, "{theme}: the primary and the alternate are different answers");
        assert!(w.state().session.mailbox == before, "{theme}: nothing is deleted while the question waits");
        assert_eq!(selected(&w), picked_before, "{theme}: the typed set is unchanged while the question waits");

        // The window and the question's own available area: the frame is the area the owner sees
        // the dialog painted into, so a button outside it is not a button they can press.
        let viewport = w.ctx.content_rect();
        let dialog = w.ctx.memory(|memory| memory.area_rect(crate::keys::dialog_layer().id)).expect("the question's window is laid out");
        assert!(viewport.contains_rect(dialog), "{theme}: the question itself is inside the window ({dialog:?} in {viewport:?})");

        let mut answers: Vec<(&str, String)> = vec![("primary", asked.action.clone()), ("cancel", asked.cancel.clone())];
        answers.extend(asked.more.iter().map(|(label, _)| ("alternate", label.clone())));
        let mut drawn_answers: Vec<(&str, String, egui::Rect)> = Vec::new();
        for (role, label) in answers {
            let rect = button_rect(&w, &label).unwrap_or_else(|| panic!("{theme}: the {role} answer {label:?} is drawn as a button"));
            assert!(rect.width() > 0.0 && rect.height() > 0.0, "{theme}: the {role} answer {label:?} has area ({rect:?})");
            assert!(viewport.contains_rect(rect), "{theme}: the {role} answer {label:?} is drawn whole inside the window ({rect:?} in {viewport:?})");
            assert!(
                dialog.contains_rect(rect),
                "{theme}: the {role} answer {label:?} is drawn whole inside the question's own area ({rect:?} in {dialog:?})"
            );
            drawn_answers.push((role, label, rect));
        }
        for (i, (role, label, rect)) in drawn_answers.iter().enumerate() {
            for (other_role, other_label, other) in drawn_answers.iter().skip(i + 1) {
                assert!(
                    !rect.intersects(*other),
                    "{theme}: the {role} answer {label:?} and the {other_role} answer {other_label:?} do not overlap ({rect:?} and {other:?})"
                );
            }
        }

        // Cancel, pressed where it is drawn: the question closes and nothing is deleted, queued or
        // unselected.
        press(&mut w, &asked.cancel);
        assert!(w.state().dialog.is_none(), "{theme}: Cancel closes the question");
        assert!(w.state().session.mailbox == before, "{theme}: Cancel deletes nothing and queues nothing");
        assert_eq!(selected(&w), picked_before, "{theme}: Cancel leaves the typed set as it was");
        assert!(w.state().session.mailbox.event_changes.is_empty(), "{theme}: no change is queued");
        assert!(w.state().session.mailbox.event(owned.id).is_some(), "{theme}: the owner's meeting is still there");
        assert!(w.state().session.mailbox.event(guest.id).is_some(), "{theme}: and the guest copy too");
    }
}
