// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #603: a Hebrew or Arabic paragraph stands at the right of the reading pane, and selecting and
//! copying it gives its text as it was written, in both themes. The people and text are made up.

use egui::Align as HAlign;
use serde_json::json;

use super::super::layout::ItemKind;
use super::{app_with, copy_key, copy_of, drag, find, gap, html_message, in_frame, lay_out, message, select, whole, window};

const HTML: &str = "<p>שלום דנה, הפגישה ב-10:30 עם ACME נקבעה.</p>\
                    <p>مرحبا، رقم الطلب 4521 جاهز.</p>\
                    <p>Thanks, Dana</p>";

fn rtl_message() -> mailcraft_model::Message {
    html_message(0, ("Dana Levi", "dana@example.org"), "פגישה", HTML)
}

/// The side a laid-out piece of text that starts with `start` stands at, and where its origin is.
fn side(laid: &super::super::layout::Laid, start: &str) -> (HAlign, f32, f32) {
    let item = find(laid, start);
    let ItemKind::Text(text) = &item.kind else { panic!("{start:?} is not text") };
    (text.job.halign, text.anchor, item.rect.width())
}

#[test]
fn hebrew_and_arabic_paragraphs_stand_at_the_right_and_latin_at_the_left() {
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &rtl_message(), 600.0);
            for start in ["שלום", "مرحبا"] {
                let (halign, anchor, width) = side(&laid, start);
                assert_eq!(halign, HAlign::RIGHT, "{start}");
                assert!((anchor - width).abs() < 0.5, "{start}: the origin is at the right, {anchor} of {width}");
            }
            assert_eq!(side(&laid, "Thanks").0, HAlign::LEFT);
            // A copy of all of it is the text in the order it was written.
            assert_eq!(copy_of(&laid), "שלום דנה, הפגישה ב-10:30 עם ACME נקבעה.\n\nمرحبا، رقم الطلب 4521 جاهز.\n\nThanks, Dana");
        });
    }
}

#[test]
fn plain_text_hebrew_stands_at_the_right_too() {
    in_frame(false, |ctx| {
        let m = message(0, ("Dana Levi", "dana@example.org"), "פגישה", None, "שלום דנה,\nהמסמך מצורף.\n\nThanks,\nDana\n".into());
        let laid = lay_out(ctx, &m, 600.0);
        assert_eq!(side(&laid, "שלום").0, HAlign::RIGHT);
        assert_eq!(side(&laid, "Thanks").0, HAlign::LEFT);
    });
}

#[test]
fn a_drag_across_a_right_aligned_paragraph_copies_it_in_written_order() {
    for theme in ["light", "dark"] {
        let (mut app, ids) = app_with(vec![rtl_message()]);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let id = ids[0];
        let mut harness = window(app, (1280.0, 900.0));
        select(&mut harness, id);
        // From the margin beside the Hebrew paragraph to after "Thanks" in the last one. (A
        // point inside a run drawn from the right is not aimed at: which character sits there
        // depends on the fonts the computer has, until the toolkit lays such runs out itself.)
        let (_, origin) = whole(&harness, id);
        let to = gap(&harness, id, "Thanks", 6);
        drag(&mut harness, origin + egui::vec2(-12.0, 4.0), to);
        assert_eq!(
            copy_key(&mut harness).as_deref(),
            Some("שלום דנה, הפגישה ב-10:30 עם ACME נקבעה.\n\nمرحبا، رقم الطلب 4521 جاهز.\n\nThanks"),
            "{theme}"
        );
    }
}
