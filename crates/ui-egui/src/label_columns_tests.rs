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
