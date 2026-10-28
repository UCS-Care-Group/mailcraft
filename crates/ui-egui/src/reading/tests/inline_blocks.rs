// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #597: boxes on a line (`display: inline-block`) stand side by side and wrap when the line is
//! full, each keeping its width, padding, frame and background; a picture in a line of text stays
//! in it and wraps with it as a word does.
//!
//! The people, addresses and pictures are invented.

use super::{check, find_exact, html_message, in_frame, lay_out, shown_pictures};
use crate::reading::layout::{Item, ItemKind, Laid};
use crate::theme::Tokens;

fn message(html: &str) -> mailcraft_model::Message {
    html_message(60, ("Marrowby Trust", "garden@marrowby.example"), "Open day", html)
}

const BUTTONS: &str = "<div style='text-align:center'>\
    <a href='https://example.org/a' style='display:inline-block;padding:10px 22px;background:#2f5d50;color:#ffffff;text-decoration:none'>Register</a> \
    <a href='https://example.org/b' style='display:inline-block;padding:10px 22px;border:2px solid #2f5d50;color:#2f5d50;text-decoration:none'>Map</a> \
    <a href='https://example.org/c' style='display:inline-block;padding:10px 22px;background:#e8e4d9;color:#2b2b2b;text-decoration:none'>Lift</a></div>";

const COLUMNS: &str = "<div>\
    <div style='display:inline-block;width:260px;vertical-align:top;background:#eef3fb;padding:12px 14px'>Morning planting</div>\
    <div style='display:inline-block;width:260px;vertical-align:top;background:#fbf4dc;padding:12px 14px'>Afternoon tea</div></div>";

fn fills(laid: &Laid) -> Vec<&Item> {
    laid.items.iter().filter(|i| matches!(i.kind, ItemKind::Fill { .. }) && i.rect.width() > 1.0 && i.rect.height() > 1.0).collect()
}

#[test]
fn buttons_stand_side_by_side_and_wrap_when_the_line_is_full() {
    let m = message(BUTTONS);
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let wide = lay_out(ctx, &m, 800.0);
            check(&wide, &Tokens::get(ctx), "buttons");
            let [a, b, c] = ["Register", "Map", "Lift"].map(|t| find_exact(&wide, t));
            // One line: the same height, one after the other, with room between.
            assert!((a.rect.center().y - b.rect.center().y).abs() < 1.0 && (b.rect.center().y - c.rect.center().y).abs() < 1.0, "{wide:?}");
            assert!(a.rect.max.x < b.rect.min.x && b.rect.max.x < c.rect.min.x);
            // Each has its look: padding round the words and a colour or a frame of its own.
            assert!(fills(&wide).iter().any(|f| f.rect.contains_rect(a.rect) && f.rect.width() >= a.rect.width() + 40.0), "{:?}", fills(&wide));
            // Centred, as the text-align of what holds them asks.
            let (left, right) = (a.rect.min.x, 800.0 - c.rect.max.x);
            assert!((left - right).abs() < 60.0, "{left} {right}");
            // In a narrow pane the line is full: the last wraps to the next one.
            let narrow = lay_out(ctx, &m, 240.0);
            check(&narrow, &Tokens::get(ctx), "buttons, narrow");
            let [a, c] = ["Register", "Lift"].map(|t| find_exact(&narrow, t));
            assert!(c.rect.min.y > a.rect.max.y - 1.0, "{:?} {:?}", a.rect, c.rect);
        });
    }
}

#[test]
fn columns_keep_their_widths_backgrounds_and_wrap_under_when_there_is_no_room() {
    let m = message(COLUMNS);
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let wide = lay_out(ctx, &m, 800.0);
            check(&wide, &Tokens::get(ctx), "columns");
            let (left, right) = (find_exact(&wide, "Morning planting"), find_exact(&wide, "Afternoon tea"));
            assert!((left.rect.min.y - right.rect.min.y).abs() < 1.0, "side by side: {:?} {:?}", left.rect, right.rect);
            assert!(right.rect.min.x > left.rect.max.x);
            // Each box is the width it asks for, with its own colour behind it.
            let widths: Vec<f32> = fills(&wide).iter().map(|f| f.rect.width().round()).collect();
            assert_eq!(widths.iter().filter(|w| (**w - 260.0).abs() <= 1.0).count(), 2, "{widths:?}");
            // Too narrow for both: the second goes under the first, whole.
            let narrow = lay_out(ctx, &m, 400.0);
            check(&narrow, &Tokens::get(ctx), "columns, narrow");
            let (left, right) = (find_exact(&narrow, "Morning planting"), find_exact(&narrow, "Afternoon tea"));
            assert!(right.rect.min.y > left.rect.max.y, "{:?} {:?}", left.rect, right.rect);
            assert!((right.rect.min.x - left.rect.min.x).abs() < 1.0);
        });
    }
}

#[test]
fn a_picture_in_a_line_of_text_stays_in_it_and_the_text_goes_on_under_it() {
    let html = "<p>Click <img src='cid:tick-16@quillon.example' width='24' height='24' alt='tick'> to carry on reading this sentence, which is long enough \
                that it cannot stay on one line of the pane and has to go on to the next one and then the one after it.</p>";
    let m = message(html);
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &m, 360.0);
            check(&laid, &Tokens::get(ctx), "picture in text");
            let pictures = shown_pictures(&laid);
            assert_eq!(pictures.len(), 1);
            let (click, tick) = (find_exact(&laid, "Click"), pictures[0]);
            let tail = laid
                .items
                .iter()
                .find(|i| matches!(&i.kind, ItemKind::Text(_)) && i.rect.height() > 2.0 * click.rect.height())
                .expect("the sentence");
            // The word, the picture and the start of the sentence are in one line...
            assert!((click.rect.center().y - tick.rect.center().y).abs() < 4.0, "{:?} {:?}", click.rect, tick.rect);
            assert!(tail.rect.min.y < tick.rect.max.y, "the sentence starts under the picture: {:?} {:?}", tail.rect, tick.rect);
            assert!(tick.rect.min.x > click.rect.max.x);
            // ...and the sentence is as wide as the line, going on under them.
            assert!(tail.rect.width() > 300.0, "{:?}", tail.rect);
        });
    }
}

#[test]
fn small_pieces_of_a_line_wrap_as_words_do() {
    let html = "<p><img src='cid:tick-16@quillon.example' width='30' height='30' alt='one'> alpha <img src='cid:cross-16@quillon.example' width='30' height='30' alt='two'> beta \
                <img src='cid:tick-16@quillon.example' width='30' height='30' alt='three'> gamma</p>";
    let m = message(html);
    in_frame(false, |ctx| {
        // Wide: one line.
        let wide = lay_out(ctx, &m, 800.0);
        let rows: Vec<f32> = shown_pictures(&wide).iter().map(|p| p.rect.center().y.round()).collect();
        assert!(rows.windows(2).all(|w| w[0] == w[1]), "{rows:?}");
        // Narrow: more lines, in reading order, none outside the pane.
        let narrow = lay_out(ctx, &m, 120.0);
        check(&narrow, &Tokens::get(ctx), "wrapped");
        let ys: Vec<f32> = shown_pictures(&narrow).iter().map(|p| p.rect.min.y).collect();
        assert!(ys.windows(2).all(|w| w[0] <= w[1]) && ys.last() > ys.first(), "{ys:?}");
        assert!(narrow.items.iter().all(|i| i.rect.max.x <= 120.5), "{:?}", narrow.items.iter().map(|i| i.rect).collect::<Vec<_>>());
    });
}

#[test]
fn the_open_day_notice_lays_out_in_both_themes_and_hostile_boxes_stay_bounded() {
    for (name, m) in super::corpus_messages().into_iter().filter(|(n, _)| ["style inline", "many inline boxes", "deep inline boxes"].contains(n)) {
        for dark in [false, true] {
            for width in [320.0, 700.0] {
                in_frame(dark, |ctx| {
                    let laid = lay_out(ctx, &m, width);
                    check(&laid, &Tokens::get(ctx), name);
                });
            }
        }
    }
}
