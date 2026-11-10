// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture wider than the body has room for (#792): it starts at the body's left margin, shown
//! smaller to fit, never pushed out to the left; and one that wraps to a row of its own after
//! text starts there too, with what follows it after it.

use std::sync::Arc;

use egui::Event;
use mailcraft_model::rich::{Paragraph, Run};
use mailcraft_model::signature::Picture;
use serde_json::json;

use super::*;
use crate::compose::tests::{Window, app, window};

/// A context with its fonts ready, as after a frame.
fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    ctx
}

/// A picture `width` wide (no pixels to decode: shown 3 by 1).
fn picture(id: u32, width: u16) -> Picture {
    Picture { id, width: Some(width), data: Arc::new(b"\x89PNG\r\n\x1a\n".to_vec()), mime: "image/png".into(), ..Default::default() }
}

fn para(runs: Vec<Run>) -> Paragraph {
    Paragraph { runs, ..Default::default() }
}

fn pic(id: u32) -> Run {
    Run::picture(id, Style::default())
}

/// `paragraph` laid out `width` wide with `pictures`.
fn laid(ctx: &egui::Context, paragraph: &Paragraph, pictures: &[Picture], width: f32) -> Para {
    laid_counted(ctx, paragraph, pictures, width).0
}

/// `paragraph` laid out `width` wide with `pictures`, and how many times egui laid out its text
/// for that.
fn laid_counted(ctx: &egui::Context, paragraph: &Paragraph, pictures: &[Picture], width: f32) -> (Para, usize) {
    let mut layout = Layout::default();
    let mut out = None;
    ctx.run_ui(egui::RawInput::default(), |ui| out = Some(layout.lay(ui.ctx(), paragraph, 0, false, pictures, &Tokens::light(), width)))
        .textures_delta
        .clear();
    (out.expect("laid out in the frame"), layout.layouts)
}

/// How wide `text` is laid out on its own.
fn text_width(ctx: &egui::Context, text: &str) -> f32 {
    let p = laid(ctx, &para(vec![Run::plain(text)]), &[], 1000.0);
    char_at(&p, text.chars().count()).min.x
}

/// The left edge of the character at `local` (on the row it starts, after a wrap), and its row's
/// top.
fn char_at(p: &Para, local: usize) -> Rect {
    p.galley.pos_from_cursor(CCursor { index: local.into(), prefer_next_row: true })
}

/// A picture shown within `room`, from its left margin on, at most `room` wide, in proportion.
fn assert_inside(b: &PictureBox, room: f32) {
    assert!(b.rect.min.x >= -0.01, "left of the margin: {:?}", b.rect);
    assert!(b.rect.max.x <= room + 0.5, "past the right edge of {room}: {:?}", b.rect);
    assert!((b.rect.width() / b.rect.height() - 3.0).abs() < 0.05, "out of proportion: {:?}", b.rect);
}

#[test]
fn a_picture_wider_than_the_body_starts_at_its_left_margin_and_fits() {
    let ctx = ctx();
    for (width, own) in [(400.0, 1200), (400.0, 400), (400.0, 399), (1000.0, 1900)] {
        let p = laid(&ctx, &para(vec![pic(1)]), &[picture(1, own)], width);
        let b = p.pictures.first().expect("laid out");
        assert_inside(b, width);
        assert!(b.rect.width() > width - 8.0, "nearly as wide as the body: {own} in {width}: {:?}", b.rect);
        assert!(b.rect.min.x < 0.5, "{:?}", b.rect);
        // One row, as tall as the picture: no empty row above it.
        assert_eq!(p.galley.rows.len(), 1, "{own} in {width}");
        assert!((p.height() - b.rect.height()).abs() < 1.0, "{} {:?}", p.height(), b.rect);
    }
}

#[test]
fn a_picture_narrower_than_the_body_keeps_its_own_width() {
    let ctx = ctx();
    let p = laid(&ctx, &para(vec![pic(1)]), &[picture(1, 300)], 400.0);
    let b = p.pictures.first().expect("laid out");
    assert_inside(b, 400.0);
    assert!((b.rect.width() - 300.0).abs() < 0.01 && b.rect.min.x.abs() < 0.5, "{:?}", b.rect);
}

#[test]
fn a_picture_after_text_that_wraps_starts_its_own_row_at_the_left_margin() {
    let ctx = ctx();
    let runs = vec![Run::plain("Hello there "), pic(1), Run::plain(" after")];
    // Just too wide to go after the text, whatever the font.
    let just = (400.0 - text_width(&ctx, "Hello there ") + 20.0) as u16;
    for own in [just, 380, 400, 1200] {
        let p = laid(&ctx, &para(runs.clone()), &[picture(1, own)], 400.0);
        let b = p.pictures.first().expect("laid out");
        assert_inside(b, 400.0);
        assert!(b.rect.min.x < 0.5, "{own}: {:?}", b.rect);
        // Below the text before it, and what follows it is after it or on a later row.
        assert!(b.rect.min.y >= char_at(&p, 0).max.y - 0.5, "{own}: {:?}", b.rect);
        let after = char_at(&p, 13);
        assert!(after.min.x >= b.rect.max.x - 0.5 || after.min.y >= b.rect.max.y - 0.5, "{own}: {after:?} over {:?}", b.rect);
    }
}

#[test]
fn a_picture_right_after_a_word_that_wraps_starts_its_own_row_too() {
    let ctx = ctx();
    // Narrow enough to go after "there" on a row of their own, or just too wide for that, whatever
    // the font.
    let there = text_width(&ctx, "there");
    let (with_word, alone) = ((400.0 - there - 20.0) as u16, (400.0 - there + 20.0) as u16);
    for own in [with_word, alone, 400, 1200] {
        for after in ["", "after", " after"] {
            let p = laid(&ctx, &para(vec![Run::plain("Hello there"), pic(1), Run::plain(after)]), &[picture(1, own)], 400.0);
            let b = p.pictures.first().expect("laid out");
            assert_inside(b, 400.0);
            assert!(b.rect.min.y >= char_at(&p, 0).max.y - 0.5, "{own} {after:?}: {:?}", b.rect);
            // The line breaks before the picture, as by a picture in Word, so even the narrow one
            // goes down by itself and "there" stays on the first row.
            assert!(b.rect.min.x < 0.5, "{own} {after:?}: {:?}", b.rect);
            assert!(char_at(&p, 6).max.y <= b.rect.min.y + 0.5, "{own} {after:?}: {:?}", char_at(&p, 6));
            if !after.is_empty() {
                let next = char_at(&p, 12);
                assert!(next.min.x >= b.rect.max.x - 0.5 || next.min.y >= b.rect.max.y - 0.5, "{own} {after:?}: {next:?} over {:?}", b.rect);
            }
        }
    }
    // A small one stays with its word.
    let p = laid(&ctx, &para(vec![Run::plain("Hello"), pic(1)]), &[picture(1, 60)], 400.0);
    assert_eq!(p.galley.rows.len(), 1);
    assert!(p.pictures[0].rect.min.x >= char_at(&p, 4).max.x - 0.5, "{:?}", p.pictures[0].rect);
}

#[test]
fn the_caret_after_a_picture_is_at_its_right_edge() {
    let ctx = ctx();
    let mut layout = Layout::default();
    let paragraphs = [para(vec![pic(1)]), para(vec![Run::plain("Hello "), pic(2)]), para(vec![Run::plain("ab"), pic(3), Run::plain("cd")])];
    let pictures = [picture(1, 300), picture(2, 1200), picture(3, 60)];
    synced(&ctx, &mut layout, &paragraphs, &pictures, 400.0);
    // Pictures at 0, 8 and 12; after each, the caret is at its right edge, on its row.
    for (at, id) in [(0, 1), (8, 2), (12, 3)] {
        let (pid, r) = layout.selected_picture(Selection { anchor: at, head: at + 1 }).expect("a picture");
        assert_eq!(pid, id);
        let caret = layout.caret(at + 1);
        assert!((caret.min.x - r.max.x).abs() < 0.5, "{id}: {caret:?} {r:?}");
        assert!(caret.min.y >= r.min.y - 0.5 && caret.max.y <= r.max.y + 0.5, "{id}: {caret:?} {r:?}");
        let before = layout.caret(at);
        assert!((before.min.x - r.min.x).abs() < 0.5, "{id}: {before:?} {r:?}");
    }
}

#[test]
fn a_picture_after_text_that_fits_stays_on_its_line() {
    let ctx = ctx();
    let p = laid(&ctx, &para(vec![Run::plain("ab"), pic(1), Run::plain("cd")]), &[picture(1, 100)], 400.0);
    let b = p.pictures.first().expect("laid out");
    assert_eq!(p.galley.rows.len(), 1);
    // After "ab", and "cd" after it.
    assert!(b.rect.min.x >= char_at(&p, 1).max.x - 0.5, "{:?}", b.rect);
    assert!(char_at(&p, 3).min.x >= b.rect.max.x - 0.5, "{:?} {:?}", char_at(&p, 3), b.rect);
}

#[test]
fn two_pictures_in_one_paragraph_each_start_at_the_margin_when_they_wrap() {
    let ctx = ctx();
    for own in [300, 1200] {
        let p = laid(&ctx, &para(vec![pic(1), pic(2), Run::plain("end")]), &[picture(1, own), picture(2, own)], 400.0);
        let [a, b] = [&p.pictures[0], &p.pictures[1]];
        assert_inside(a, 400.0);
        assert_inside(b, 400.0);
        assert!(a.rect.min.x < 0.5 && b.rect.min.x < 0.5, "{own}: {:?} {:?}", a.rect, b.rect);
        assert!(b.rect.min.y >= a.rect.max.y - 0.5, "the second below the first: {:?} {:?}", a.rect, b.rect);
        let end = char_at(&p, 2);
        assert!(end.min.x >= b.rect.max.x - 0.5 || end.min.y >= b.rect.max.y - 0.5, "{own}: {end:?} over {:?}", b.rect);
    }
    // Two that fit side by side stay side by side.
    let p = laid(&ctx, &para(vec![pic(1), pic(2)]), &[picture(1, 150), picture(2, 150)], 400.0);
    let [a, b] = [&p.pictures[0], &p.pictures[1]];
    assert!(
        a.rect.min.x < 0.5 && (b.rect.min.x - a.rect.max.x).abs() < 4.0 && (a.rect.min.y - b.rect.min.y).abs() < 0.5,
        "{:?} {:?}",
        a.rect,
        b.rect
    );
}

#[test]
fn a_picture_in_a_list_item_fits_the_room_after_its_indent() {
    let ctx = ctx();
    for level in [0, 2] {
        let mut item = para(vec![pic(1)]);
        item.list = ListKind::Bullet;
        item.level = level;
        let room = 400.0 - indent_of(&item);
        let p = laid(&ctx, &item, &[picture(1, 1200)], 400.0);
        let b = p.pictures.first().expect("laid out");
        assert_inside(b, room);
        assert!(b.rect.width() > room - 8.0, "{:?}", b.rect);
    }
}

/// The body's layout, kept between frames, synchronised with `paragraphs` at `width`.
fn synced(ctx: &egui::Context, layout: &mut Layout, paragraphs: &[Paragraph], pictures: &[Picture], width: f32) {
    let key = LayoutKey { width: width as u32, dark: false, scale: 1f32.to_bits(), plain: false, pictures: pictures_key(pictures), families: 0 };
    ctx.run_ui(egui::RawInput::default(), |ui| {
        layout.sync(ui.ctx(), paragraphs, pictures, key, &Tokens::light(), width);
    })
    .textures_delta
    .clear();
}

#[test]
fn the_picture_is_hit_selected_and_has_the_caret_where_it_is_drawn() {
    let ctx = ctx();
    let mut layout = Layout::default();
    let paragraphs = [para(vec![Run::plain("Above")]), para(vec![Run::plain("Hello "), pic(1), Run::plain(" after")])];
    let pictures = [picture(1, 1200)];
    synced(&ctx, &mut layout, &paragraphs, &pictures, 400.0);
    // The picture is character 6 of the second paragraph, 12 of the text.
    let (at, id, r) = layout.selected_picture(Selection { anchor: 12, head: 13 }).map(|(id, r)| (12, id, r)).expect("selected");
    assert_eq!(id, 1);
    assert!(r.min.x >= 0.0 && r.max.x <= 400.5 && r.min.y > 0.0, "{r:?}");
    assert_eq!(layout.picture_hit(r.center().to_vec2()).map(|(i, id, _)| (i, id)), Some((at, 1)));
    assert_eq!(layout.char_under(r.center().to_vec2()), Some(at));
    // A click on its left half puts the caret before it, on its right half after it.
    assert_eq!(layout.hit(r.left_center().to_vec2() + vec2(4.0, 0.0)), at);
    assert_eq!(layout.hit(r.right_center().to_vec2() - vec2(4.0, 0.0)), at + 1);
    // The caret before it is at its left edge, at the margin.
    let caret = layout.caret(at);
    assert!((caret.min.x - r.min.x).abs() < 0.5 && caret.min.x >= 0.0, "{caret:?} {r:?}");
}

#[test]
fn making_the_window_wider_and_narrower_again_keeps_the_picture_at_the_margin() {
    let ctx = ctx();
    let mut layout = Layout::default();
    let paragraphs = [para(vec![pic(1)]), para(vec![Run::plain("Text "), pic(2)])];
    let pictures = [picture(1, 900), picture(2, 900)];
    for width in [400.0, 1200.0, 400.0, 899.0, 901.0] {
        synced(&ctx, &mut layout, &paragraphs, &pictures, width);
        for p in &layout.paras {
            for b in &p.pictures {
                assert!(b.rect.min.x >= -0.01 && b.rect.max.x <= width + 0.5, "{width}: {:?}", b.rect);
                assert!(b.rect.width() >= (width - 8.0).min(900.0), "{width}: {:?}", b.rect);
            }
        }
    }
}

/// A screenshot wider than the body, pasted into a new message (as in #792) and into a
/// signature: each is drawn inside its body.
fn screenshot() -> Vec<u8> {
    use image::ImageEncoder as _;
    let (w, h) = (1600u32, 400u32);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w / 40 % 256) as u8, 60, 90]).collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

/// Every picture of the body `id` laid out within its width.
fn assert_drawn_inside(h: &Window, id: egui::Id) -> usize {
    let store = store(&h.ctx, id);
    let layout = store.lock().unwrap();
    let width = layout.key.expect("laid out").width as f32 + 1.0;
    let mut n = 0;
    for p in &layout.paras {
        for b in &p.pictures {
            let r = b.rect.translate(vec2(p.indent(), 0.0));
            assert!(r.min.x >= -0.01 && r.max.x <= width + 0.5, "{width}: {r:?}");
            n += 1;
        }
    }
    n
}

#[test]
fn a_pasted_screenshot_wider_than_the_window_is_drawn_inside_the_body() {
    #[derive(Clone)]
    struct Board(Vec<u8>);
    impl crate::RichClipboard for Board {
        fn put(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn html(&self) -> Option<String> {
            None
        }
        fn image(&self) -> Option<Vec<u8>> {
            Some(self.0.clone())
        }
    }
    let mut a = app();
    a.services.clipboard = Some(Box::new(Board(screenshot())));
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"body": "Look:"})).unwrap();
    a.execute("compose.select", json!({"from": 5})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.run_steps(2);
    h.event(Event::Paste(String::new()));
    h.run_steps(3);
    assert_eq!(h.state().session.compose.as_ref().unwrap().pictures.len(), 1);
    let id = Which::Message.id(h.state());
    assert_eq!(assert_drawn_inside(&h, id), 1);
    // Narrower, wider than the picture and narrower again.
    for size in [vec2(900.0, 700.0), vec2(2400.0, 900.0), vec2(700.0, 700.0)] {
        h.set_size(size);
        h.run_steps(3);
        assert_eq!(assert_drawn_inside(&h, id), 1, "{size:?}");
    }
}

#[test]
fn a_signature_picture_wider_than_its_body_is_drawn_inside_it() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Mine", "text": "Avery"})).unwrap();
    a.execute("mail.new", json!({})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    a.execute("signature.open", json!({"name": "Mine"})).unwrap();
    crate::signatures::add_picture(&mut a, json!({"data": crate::files::base64_lite::encode(&screenshot()), "file": "wide.png"}));
    assert_eq!(a.session.signature_edit.as_ref().unwrap().signature.pictures().len(), 1);
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    // A window narrow enough that the signature's body is narrower than the picture.
    h.set_size(vec2(640.0, 700.0));
    h.run_steps(4);
    let id = Which::Signature.id(h.state());
    assert_eq!(assert_drawn_inside(&h, id), 1);
}

/// Every picture of `p` drawn from the left margin on and within `room` (each is shown at most
/// `room` wide), no two drawn over each other, and none over a letter of the text.
fn assert_all_inside(p: &Para, room: f32, what: &str) {
    for (i, b) in p.pictures.iter().enumerate() {
        assert!(b.rect.min.x >= -0.01, "{what}: picture {i} left of the margin: {:?}", b.rect);
        assert!(b.rect.max.x <= room + 0.5, "{what}: picture {i} past the right edge of {room}: {:?}", b.rect);
        for (j, c) in p.pictures.iter().enumerate().skip(i + 1) {
            assert!(!b.rect.shrink(0.5).intersects(c.rect.shrink(0.5)), "{what}: pictures {i} and {j} overlap: {:?} {:?}", b.rect, c.rect);
        }
    }
    for row in &p.galley.rows {
        // The letters, not the characters pictures are laid out as or the spaces.
        for g in row.glyphs.iter().filter(|g| g.chr != OBJECT && g.chr != BREAKS_BEFORE && !g.chr.is_whitespace()) {
            let letter = g.logical_rect().translate(row.pos.to_vec2());
            for (i, b) in p.pictures.iter().enumerate() {
                assert!(!b.rect.shrink(0.5).intersects(letter.shrink(0.5)), "{what}: picture {i} over {:?}: {:?} {letter:?}", g.chr, b.rect);
            }
        }
    }
}

#[test]
fn two_pictures_side_by_side_with_no_space_between_them_stay_inside_the_body() {
    let ctx = ctx();
    let pictures = [picture(1, 260), picture(2, 90), picture(3, 150), picture(4, 260)];
    let cases = [
        (331.0, vec![Run::plain("word "), pic(1), pic(2), Run::plain("longer ")]),
        (278.0, vec![Run::plain(" "), pic(1), pic(3), Run::plain(" ")]),
        (316.0, vec![Run::plain("x"), Run::plain("word "), pic(1), pic(4)]),
    ];
    for (width, runs) in cases {
        let p = laid(&ctx, &para(runs), &pictures, width);
        assert_eq!(p.pictures.len(), 2);
        assert_all_inside(&p, width, &format!("{width}"));
        // The second picture does not fit after the first, so it starts a row of its own.
        let [a, b] = [&p.pictures[0], &p.pictures[1]];
        assert!(b.rect.min.x < 0.5 && b.rect.min.y >= a.rect.max.y - 0.5, "{width}: {:?} {:?}", a.rect, b.rect);
    }
}

/// A small random number generator with a fixed seed, so that the paragraphs below are the same
/// at every run.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

#[test]
fn pictures_among_words_in_bodies_of_any_width_are_drawn_inside_them() {
    let ctx = ctx();
    let mut dice = Dice(0x5EED_0792_0796);
    let pictures: Vec<Picture> = (1..=6).map(|id| picture(id, 0)).collect();
    for n in 0..1500 {
        let width = 100.0 + dice.below(401) as f32;
        let mut pictures = pictures.clone();
        let mut runs = Vec::new();
        for _ in 0..1 + dice.below(8) {
            match dice.below(5) {
                0 | 1 => {
                    let id = 1 + dice.below(6) as u32;
                    if let Some(x) = pictures.get_mut(id as usize - 1) {
                        x.width = Some(8 + dice.below(600) as u16);
                    }
                    runs.push(pic(id));
                }
                k => {
                    let word: String = (0..1 + dice.below(9)).map(|_| char::from(b'a' + dice.below(26) as u8)).collect();
                    runs.push(Run::plain(if k == 2 { word } else { format!("{word} ") }));
                }
            }
        }
        let (p, layouts) = laid_counted(&ctx, &para(runs.clone()), &pictures, width);
        assert_all_inside(&p, width, &format!("{n}: {width} {runs:?}"));
        // So few pictures are found one by one, never all at once.
        assert!(layouts <= MOST_LAYOUTS, "{n}: {layouts}");
    }
}

#[test]
fn a_picture_repeated_a_thousand_times_in_a_paragraph_is_laid_out_a_bounded_number_of_times() {
    let ctx = ctx();
    let pictures = [picture(1, 240)];
    // Each on a row of its own, alone and among words.
    let alone: Vec<Run> = (0..1000).map(|_| pic(1)).collect();
    let among: Vec<Run> = (0..1000).flat_map(|i| [Run::plain(if i % 2 == 0 { "word " } else { "word" }), pic(1)]).collect();
    for (what, runs) in [("alone", alone), ("among words", among)] {
        let (p, layouts) = laid_counted(&ctx, &para(runs), &pictures, 400.0);
        assert_eq!(p.pictures.len(), 1000, "{what}");
        // A few dozen times, not once for each picture.
        assert!(layouts <= 40, "{what}: laid out {layouts} times");
        assert_all_inside(&p, 400.0, what);
    }
}

#[test]
fn many_pictures_among_words_laid_out_all_at_once_are_drawn_inside_and_over_nothing() {
    let ctx = ctx();
    let mut dice = Dice(0x0796_CA9F_A11B);
    let pictures: Vec<Picture> = (1..=6).map(|id| picture(id, 0)).collect();
    let mut at_once = 0;
    for n in 0..200 {
        let width = 100.0 + dice.below(401) as f32;
        let mut pictures = pictures.clone();
        let mut runs = Vec::new();
        for _ in 0..40 + dice.below(80) {
            match dice.below(5) {
                0..=2 => {
                    let id = 1 + dice.below(6) as u32;
                    if let Some(x) = pictures.get_mut(id as usize - 1) {
                        x.width = Some(8 + dice.below(600) as u16);
                    }
                    runs.push(pic(id));
                }
                k => {
                    let word: String = (0..1 + dice.below(9)).map(|_| char::from(b'a' + dice.below(26) as u8)).collect();
                    runs.push(Run::plain(if k == 3 { word } else { format!("{word} ") }));
                }
            }
        }
        let (p, layouts) = laid_counted(&ctx, &para(runs.clone()), &pictures, width);
        assert!(layouts <= MOST_LAYOUTS + 1, "{n}: {layouts}");
        at_once += usize::from(layouts == MOST_LAYOUTS + 1);
        assert_all_inside(&p, width, &format!("{n}: {width} {runs:?}"));
    }
    // (The pictures left were marked all at once in most of them.)
    assert!(at_once >= 50, "{at_once}");
}
