// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box's scope button grows to its longest name, so a language with a long one doesn't
//! have it drawn over the text of the box.

use egui::{Rect, Shape};
use serde_json::json;

use crate::people_view::testing;

/// Where the text ending in the arrow is drawn.
fn arrow_text(shapes: &[Shape], out: &mut Vec<Rect>) {
    for s in shapes {
        match s {
            Shape::Text(t) if t.galley.text().ends_with('▾') => out.push(t.galley.rect.translate(t.pos.to_vec2())),
            Shape::Vec(v) => arrow_text(v, out),
            _ => {}
        }
    }
}

#[test]
fn a_long_scope_name_stays_in_its_button_beside_the_edit_area() {
    let mut w = testing::window("view.mail");
    // Georgian's "Current Folder" is the longest of the four, and wider than the old 104 points.
    w.state_mut().execute("ui.language", json!({"language": "ka"})).unwrap();
    w.run_steps(4);
    let shapes: Vec<Shape> = w.output().shapes.iter().map(|c| c.shape.clone()).collect();
    let mut found = Vec::new();
    arrow_text(&shapes, &mut found);
    let [label] = found[..] else { panic!("the scope button's text is drawn once: {found:?}") };
    let advanced = w.state().hit.rect("advancedSearchButton", 0).unwrap();
    let search = w.state().hit.rect("searchBox", 0).unwrap();
    assert!(label.min.x >= advanced.max.x, "the label {label:?} starts over Advanced Search's button {advanced:?}");
    assert!(label.max.x <= search.max.x, "the label {label:?} ends outside the box {search:?}");
}

/// The (laid-out) texts equal to `text`: where each is drawn, and in how many lines.
fn drawn(shapes: &[Shape], text: &str, out: &mut Vec<(Rect, usize)>) {
    for s in shapes {
        match s {
            Shape::Text(t) if t.galley.text() == text => out.push((t.galley.rect.translate(t.pos.to_vec2()), t.galley.rows.len())),
            Shape::Vec(v) => drawn(v, text, out),
            _ => {}
        }
    }
}

#[test]
fn an_account_cards_buttons_fit_their_labels() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.language", json!({"language": "ka"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    w.run_steps(4);
    let shapes: Vec<Shape> = w.output().shapes.iter().map(|c| c.shape.clone()).collect();
    let (mut settings, mut remove) = (Vec::new(), Vec::new());
    drawn(&shapes, crate::tl!("Settings"), &mut settings);
    drawn(&shapes, crate::tl!("Remove"), &mut remove);
    assert!(!settings.is_empty() && !remove.is_empty(), "an account card is drawn: {settings:?} {remove:?}");
    assert!(settings.iter().chain(&remove).all(|&(_, lines)| lines == 1), "a button's label wraps: {settings:?} {remove:?}");
    let (s, r) = (settings[settings.len() - 1].0, remove[remove.len() - 1].0);
    assert!(s.max.x < r.min.x, "Settings {s:?} reaches Remove {r:?}");
}

#[test]
fn a_card_button_is_as_wide_as_its_label_needs() {
    let widths = std::cell::RefCell::new(Vec::new());
    let mut h = egui_kittest::Harness::builder().with_accessibility_check(false).build_ui(|ui| {
        let long = "A label much longer than any button on a card has ever been";
        let text = ui.painter().layout_no_wrap(long.into(), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE).size().x;
        widths.borrow_mut().extend([crate::backstage::card_button_w(ui, "OK"), crate::backstage::card_button_w(ui, long), text]);
    });
    h.run();
    let [short, long, text] = widths.borrow()[..3] else { panic!("measured") };
    assert_eq!(short, 74.0, "the least a button is");
    assert!(long >= text + 2.0 * 4.0, "the long label {text} doesn't fit in {long}");
}
