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
    let _language = crate::i18n::RestoreLanguage::hold();
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

#[test]
fn the_scope_button_is_capped_at_two_fifths_of_a_narrow_box() {
    let _language = crate::i18n::RestoreLanguage::hold();
    crate::i18n::set_current(crate::i18n::Lang::English);
    let widths = std::cell::RefCell::new(Vec::new());
    let mut h = egui_kittest::Harness::builder().with_accessibility_check(false).build_ui(|ui| {
        // (The interface's fonts have the arrow; they take hold from the pass after they are set.)
        ui.ctx().set_fonts(crate::theme::font_definitions_with(&crate::theme::SystemFont::none()));
        if ui.ctx().cumulative_pass_nr() < 2 {
            return;
        }
        widths.borrow_mut().extend([
            crate::chrome::scope_width(ui, 1000.0),
            crate::chrome::scope_width(ui, 100.0),
            crate::chrome::scope_width(ui, 400.0),
        ]);
    });
    h.run_steps(4);
    let [wide, narrow, mid] = widths.borrow()[..3] else { panic!("measured") };
    assert!((104.0..=400.0).contains(&wide), "{wide}");
    assert_eq!(narrow, 104.0, "the least wins in a tiny box");
    assert!(mid <= 160.0 + f32::EPSILON, "two fifths of 400 is the most: {mid}");
}
