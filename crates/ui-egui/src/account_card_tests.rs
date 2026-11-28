// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Settings and Remove buttons on File ▸ Info's account cards fit a longer translated label,
//! up to a fifth of the card each.

use egui::{Rect, Shape};
use serde_json::json;

use crate::people_view::testing;

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
    // Step until the page has drawn its card (a busy machine can take more than a few frames).
    let (mut settings, mut remove) = (Vec::new(), Vec::new());
    for _ in 0..60 {
        w.run_steps(1);
        let shapes: Vec<Shape> = w.output().shapes.iter().map(|c| c.shape.clone()).collect();
        (settings, remove) = (Vec::new(), Vec::new());
        drawn(&shapes, crate::tl!("Settings"), &mut settings);
        drawn(&shapes, crate::tl!("Remove"), &mut remove);
        if !settings.is_empty() && !remove.is_empty() {
            break;
        }
    }
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
        widths.borrow_mut().extend([
            crate::backstage::card_button_w(ui, "OK", f32::INFINITY),
            crate::backstage::card_button_w(ui, long, f32::INFINITY),
            crate::backstage::card_button_w(ui, long, 120.0),
            crate::backstage::card_button_w(ui, "OK", 50.0),
            crate::backstage::card_button_w(ui, long, f32::NAN),
            text,
        ]);
    });
    h.run();
    let [short, long, capped, narrow, nan, text] = widths.borrow()[..6] else { panic!("measured") };
    assert_eq!(short, 74.0, "the least a button is");
    assert!(long >= text + 2.0 * 4.0, "the long label {text} doesn't fit in {long}");
    assert_eq!(capped, 120.0, "a label past the cap is cut at it");
    assert_eq!(narrow, 50.0, "the cap wins over the least on a narrow card");
    assert_eq!(nan, 0.0, "a cap that is no number gives no width, not a panic");
}
