// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The label columns of the contact form (and the dialogs laid out like it) and of the task details
//! are as wide as their widest label in the interface language, so a translated label isn't cut.

use egui::Shape;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// Whether `label` is drawn on the page, and whole (not cut with an ellipsis).
fn drawn_whole(h: &Window, label: &str) -> Option<bool> {
    h.output().shapes.iter().find_map(|s| match &s.shape {
        Shape::Text(t) if t.galley.text() == label => Some(!t.galley.elided),
        _ => None,
    })
}

/// Steps frames until `label` is drawn (up to 60: a fixed count is flaky under load).
fn whole_after_stepping(h: &mut Window, label: &str) -> bool {
    for _ in 0..60 {
        h.step();
        if let Some(whole) = drawn_whole(h, label) {
            // Another few frames: the column learns the widest label a pass late.
            h.run_steps(3);
            return drawn_whole(h, label).unwrap_or(whole);
        }
    }
    panic!("{label:?} is not drawn");
}

fn georgian(module: &str) -> Window {
    let mut h = testing::window(module);
    h.state_mut().execute("ui.language", json!({"language": "ka"})).unwrap();
    h.run_steps(3);
    h
}

fn english(module: &str) -> Window {
    testing::window(module)
}

fn select_task(h: &mut Window) {
    let id = *h.state().session.mailbox.tasks.keys().next().unwrap();
    h.state_mut().run("tasks.select", json!({"id": id})).unwrap();
}

#[test]
fn the_contact_forms_display_name_label_is_whole_in_georgian() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = georgian("view.people");
    h.state_mut().run("people.new", json!({})).unwrap();
    let label = crate::i18n::t("Display name").to_owned();
    assert_ne!(label, "Display name", "the catalogue has Georgian for it");
    assert!(whole_after_stepping(&mut h, &label));
}

#[test]
fn the_task_details_categories_label_is_whole_in_georgian() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = georgian("view.tasks");
    select_task(&mut h);
    let label = crate::i18n::t("Categories").to_owned();
    assert_ne!(label, "Categories", "the catalogue has Georgian for it");
    assert!(whole_after_stepping(&mut h, &label));
}

#[test]
fn english_keeps_the_old_column_width() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = english("view.tasks");
    select_task(&mut h);
    assert!(whole_after_stepping(&mut h, "Categories"));
    let value_x = |h: &Window, text: &str| {
        h.output().shapes.iter().find_map(|s| match &s.shape {
            Shape::Text(t) if t.galley.text() == text => Some(t.pos.x),
            _ => None,
        })
    };
    let (label, due) = (value_x(&h, "Categories").unwrap(), value_x(&h, "Due").unwrap());
    assert_eq!(label, due, "the labels start on one line");
    // The first of the value column: "None" after Categories, as far in as the label's 112 points.
    let none = value_x(&h, "None").unwrap();
    assert!((none - due - (112.0 - 26.0)).abs() < 1.0, "label at {due}, value at {none}");
}

/// Where the contact form's first field starts: the leftmost text drawn right of "Display name" on its row.
fn first_field_x(h: &Window, label: &str) -> f32 {
    let texts: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some((t.galley.text().to_owned(), t.pos)),
            _ => None,
        })
        .collect();
    let at = texts.iter().find(|(text, _)| text == label).expect("the label is drawn").1;
    texts.iter().filter(|(_, pos)| (pos.y - at.y).abs() < 8.0 && pos.x > at.x + 20.0).map(|(_, pos)| pos.x).fold(f32::INFINITY, f32::min)
}

#[test]
fn the_form_column_shrinks_back_after_georgian() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut fresh = english("view.people");
    fresh.state_mut().run("people.new", json!({})).unwrap();
    assert!(whole_after_stepping(&mut fresh, "Display name"));
    let want = first_field_x(&fresh, "Display name");

    let mut h = georgian("view.people");
    h.state_mut().run("people.new", json!({})).unwrap();
    assert!(whole_after_stepping(&mut h, crate::i18n::t("Display name")));
    // No focused field: its blinking cursor would repaint the window and hide a column that never settles.
    h.ctx.memory_mut(|m| m.focused().map(|id| m.surrender_focus(id)));
    h.state_mut().execute("ui.language", json!({"language": "en"})).unwrap();
    // Only the passes the window asks for: an idle window must not keep the Georgian width.
    h.run();
    let got = first_field_x(&h, "Display name");
    assert!((got - want).abs() < 1.0, "fields at {got} after Georgian, {want} in a fresh English window");
}
