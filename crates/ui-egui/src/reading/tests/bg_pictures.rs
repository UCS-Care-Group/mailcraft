// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #601: pictures behind a box: the message's own (attached or written into it) always, and none
//! from the web ever: only an `<img>`'s address is asked for.
//!
//! The people, addresses and pictures are invented.

use egui::Color32;
use mailcraft_model::Attachment;

use super::{html_message, in_frame, png};
use crate::reading::backdrop::DARK_TINT;
use crate::reading::images::Pictures;
use crate::reading::layout::{BackdropItem, ItemKind, Laid};
use crate::reading::view::Doc;

/// `m` laid out to the end at 820 points, showing the pictures from the web that have come when
/// `remote` (a picture behind a box is none of them).
fn laid(ctx: &egui::Context, m: &mailcraft_model::Message, remote: bool) -> Laid {
    let doc = Doc::of(m);
    let pictures = Pictures::of(ctx);
    let mut pictures = pictures.lock().unwrap();
    let mut laid = Laid::new(820.0).showing_remote(remote);
    laid.advance(ctx, &doc.blocks, Some(m), &mut pictures, usize::MAX);
    assert!(laid.done);
    laid
}

fn behind(laid: &Laid) -> Vec<(egui::Rect, &BackdropItem)> {
    laid.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Backdrop(b) => Some((i.rect, b)),
            _ => None,
        })
        .collect()
}

/// Where in the painting order the first text that starts with `start` is.
fn text_at(laid: &Laid, start: &str) -> usize {
    laid.items.iter().position(|i| matches!(&i.kind, ItemKind::Text(t) if t.job.text.starts_with(start))).unwrap()
}

fn hero(html_cell: &str) -> mailcraft_model::Message {
    let html = format!(
        "<table width=600 cellpadding=0 cellspacing=0><tr>{html_cell}<p style=\"color:#ffffff\">Autumn sale</p></td></tr>\
         <tr><td>Everything else</td></tr></table>"
    );
    html_message(0, ("Rowan Ash", "rowan@example.org"), "Autumn", &html)
}

#[test]
fn an_attached_picture_is_behind_its_cell_under_the_text() {
    let mut m = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#f4efe6\" style=\"background-repeat:no-repeat\">");
    m.attachments =
        vec![Attachment { content_id: "hero@example.org".into(), inline: true, ..Attachment::new("hero.png", "image/png", png(300, 100)) }];
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = laid(ctx, &m, false);
            let behind = behind(&laid);
            assert_eq!(behind.len(), 1, "{:?}", laid.stats);
            let (rect, item) = behind[0];
            // It covers the cell (which is as tall as what it holds: a picture behind a box gives
            // it no height, as in CSS), is painted before the text in it, and once.
            assert!((rect.width() - 600.0).abs() < 1.0 && rect.height() >= 20.0, "{rect:?}");
            let at = laid.items.iter().position(|i| matches!(i.kind, ItemKind::Backdrop(_))).unwrap();
            assert!(at < text_at(&laid, "Autumn sale"));
            assert_eq!((item.tile, item.repeat), (egui::vec2(300.0, 100.0), [false, false]));
            // In the dark theme the picture is dimmed, as the light colour under it is turned dark.
            assert_eq!((item.tint, item.veil), (if dark { DARK_TINT } else { Color32::WHITE }, None));
        });
    }
}

#[test]
fn a_dark_colour_leaves_its_picture_as_it_is_and_no_colour_veils_it() {
    let attach = |m: &mut mailcraft_model::Message| {
        m.attachments =
            vec![Attachment { content_id: "hero@example.org".into(), inline: true, ..Attachment::new("hero.png", "image/png", png(300, 100)) }];
    };
    let mut dark_cell = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#203040\">");
    attach(&mut dark_cell);
    // White text on a photograph with no colour given: what the text is read against is the
    // paper round it, so the picture is veiled with that paper and the text stays readable.
    let mut bare = hero("<td background=\"cid:hero@example.org\">");
    attach(&mut bare);
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid_dark = laid(ctx, &dark_cell, false);
            assert_eq!((behind(&laid_dark)[0].1.tint, behind(&laid_dark)[0].1.veil), (Color32::WHITE, None));
            let laid_bare = laid(ctx, &bare, false);
            let item = behind(&laid_bare)[0].1.clone();
            let ItemKind::Text(text) = &laid_bare.items[text_at(&laid_bare, "Autumn sale")].kind else { panic!("no text") };
            assert_eq!(item.tint, if dark { DARK_TINT } else { Color32::WHITE });
            let veil = item.veil.unwrap();
            assert!(veil.a() >= 170 && veil.a() < 255, "{veil:?}");
            // The veil is the paper the text was made readable on.
            assert_eq!(veil, text.paper.gamma_multiply(crate::reading::backdrop::VEIL));
        });
    }
}

#[test]
fn a_picture_written_into_the_message_is_behind_the_body() {
    let tile = png(8, 8);
    let b64 = super::written_pictures::b64(&tile);
    let html = format!("<body background=\"data:image/png;base64,{b64}\"><p>Dear Avery,</p><p>Tiled.</p></body>");
    let m = html_message(0, ("Rowan Ash", "rowan@example.org"), "Paper", &html);
    in_frame(false, |ctx| {
        let laid = laid(ctx, &m, false);
        let behind = behind(&laid);
        assert_eq!(behind.len(), 1);
        assert_eq!((behind[0].1.tile, behind[0].1.repeat), (egui::vec2(8.0, 8.0), [true, true]));
    });
}

/// A message with a picture from the web behind a table, a cell, a block and the body (each a
/// different address, by an attribute, a style and a style sheet), and one `<img>` from there.
fn web_behind(n: usize, host: &str) -> mailcraft_model::Message {
    let at = |what: &str| format!("https://{host}/{what}.png");
    let html = format!(
        "<style>.band{{background:#203040 url('{}') repeat-x}}</style>         <body background=\"{}\"><table width=600 background=\"{}\"><tr><td style=\"background-image:url('{}')\">One</td>         <td background=\"{}\" bgcolor=\"#f4efe6\">Two</td></tr></table>         <div class=\"band\" style=\"padding:8px\">Three</div>         <div style=\"background-image:url('{}');padding:8px\">Four</div>         <p>Five</p><img src=\"{}\" width=40 height=30 alt=\"A picture\"></body>",
        at("sheet"),
        at("body"),
        at("table"),
        at("cell-style"),
        at("cell-attr"),
        at("block"),
        at("img")
    );
    html_message(n, ("Rowan Ash", "rowan@example.org"), "Backgrounds", &html)
}

#[test]
fn a_picture_on_the_web_behind_a_box_is_never_asked_for() {
    use serde_json::json;
    let (inbox, junk) = (web_behind(0, "a.img.example"), web_behind(1, "b.img.example"));
    // Nothing of it is one of the message's pictures on the web but the `<img>`.
    for m in [&inbox, &junk] {
        let doc = Doc::of(m);
        assert_eq!((doc.remote.len(), doc.remote_pictures), (1, 1), "{:?}", doc.remote);
        assert!(doc.remote[0].ends_with("/img.png"));
    }
    let (mut app, ids) = super::app_with(vec![inbox, junk]);
    app.execute("mail.junk", json!({"ids": [ids[1]]})).unwrap();
    let host = super::Host::answering(&mut app, |_| Ok(png(32, 32)));
    // Pictures are downloaded for all mail at once, and the owner asks for the one in Junk E-mail.
    app.execute("ui.downloadPicturesAutomatically", json!({"value": true})).unwrap();
    let mut harness = super::window(app, (1280.0, 1000.0));
    super::select(&mut harness, ids[0]);
    harness.run_steps(8);
    super::select(&mut harness, ids[1]);
    let download = harness.state().hit.rect("downloadPictures", ids[1]).expect("no way to ask for the pictures");
    super::click_at(&mut harness, download.center());
    harness.run_steps(8);
    // Both asked for their `<img>`, and neither for a picture behind a box.
    let mut asked = host.asked();
    asked.sort();
    assert_eq!(asked, ["https://a.img.example/img.png", "https://b.img.example/img.png"]);
    // Nor is one painted: the boxes have their colours alone.
    for id in [ids[0], ids[1]] {
        let backdrops = crate::reading::view::kept(&harness.ctx, id, |laid, _| behind(laid).len()).unwrap();
        assert_eq!(backdrops, 0);
    }
}

#[test]
fn a_picture_that_is_missing_or_not_a_picture_leaves_the_colour() {
    let mut m = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#203040\">");
    in_frame(false, |ctx| {
        assert!(behind(&laid(ctx, &m, true)).is_empty());
        m.attachments =
            vec![Attachment { content_id: "hero@example.org".into(), inline: true, ..Attachment::new("hero.png", "image/png", b"<svg/>".to_vec()) }];
        let laid = laid(ctx, &m, true);
        assert!(behind(&laid).is_empty());
        // The cell's colour is there still.
        assert!(laid.items.iter().any(|i| matches!(i.kind, ItemKind::Fill { color, .. } if color == Color32::from_rgb(0x20, 0x30, 0x40))));
    });
}

#[test]
fn high_contrast_takes_the_pictures_behind_boxes_away_at_once() {
    use serde_json::json;
    let mut m = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#f4efe6\">");
    m.attachments =
        vec![Attachment { content_id: "hero@example.org".into(), inline: true, ..Attachment::new("hero.png", "image/png", png(300, 100)) }];
    let (app, ids) = super::app_with(vec![m]);
    let mut harness = super::window(app, (1280.0, 1000.0));
    super::select(&mut harness, ids[0]);
    let backdrops = |harness: &super::Window| crate::reading::view::kept(&harness.ctx, ids[0], |laid, _| behind(laid).len()).unwrap();
    assert_eq!(backdrops(&harness), 1);
    // Nothing else changes, not the width, the zoom or the message: the picture goes all the same,
    // and comes back when high contrast is turned off.
    harness.state_mut().execute("ui.highContrast", json!({"on": true})).unwrap();
    harness.run_steps(4);
    assert_eq!(backdrops(&harness), 0);
    harness.state_mut().execute("ui.highContrast", json!({"on": false})).unwrap();
    harness.run_steps(4);
    assert_eq!(backdrops(&harness), 1);
}

#[test]
fn the_pictures_behind_boxes_and_the_others_share_one_cap() {
    use crate::reading::images::MAX_PICTURES_PER_MESSAGE;
    let cells = "<table><tr><td background=\"cid:tile@example.org\">x</td></tr></table>".repeat(MAX_PICTURES_PER_MESSAGE);
    let html = format!("{cells}<p><img src=\"cid:tile@example.org\" alt=\"A picture\"></p>");
    let mut m = html_message(0, ("Rowan Ash", "rowan@example.org"), "Tiles", &html);
    m.attachments = vec![Attachment { content_id: "tile@example.org".into(), inline: true, ..Attachment::new("tile.png", "image/png", png(8, 8)) }];
    in_frame(false, |ctx| {
        let laid = laid(ctx, &m, false);
        // Every one behind a box, and none past them: the `<img>` after them is a stand-in.
        assert_eq!((laid.stats.behind, laid.stats.pictures), (MAX_PICTURES_PER_MESSAGE, 0), "{:?}", laid.stats);
    });
}

/// The colour the first run of the text that starts with `start` is painted in.
fn ink_of(laid: &Laid, start: &str) -> Color32 {
    let ItemKind::Text(text) = &laid.items[text_at(laid, start)].kind else { panic!("no text") };
    text.job.sections[0].format.color
}

fn attached(m: &mut mailcraft_model::Message) {
    m.attachments =
        vec![Attachment { content_id: "hero@example.org".into(), inline: true, ..Attachment::new("hero.png", "image/png", png(300, 100)) }];
}

#[test]
fn text_over_a_picture_shown_as_it_is_keeps_its_colour() {
    // The newsletter case: white text over a dark banner, with a light fallback colour for the
    // programs that show no picture. The text is not darkened for the cream.
    let mut m = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#f4efe6\" style=\"background-repeat:no-repeat\">");
    attached(&mut m);
    // The same with a colour of the cell's own that the dark theme leaves as it is: a dark one.
    let mut dark_fallback = hero("<td background=\"cid:hero@example.org\" bgcolor=\"#203040\">");
    attached(&mut dark_fallback);
    // And the picture behind the whole table, over its colour.
    let html = "<table width=600 bgcolor=\"#f4efe6\" background=\"cid:hero@example.org\"><tr><td><p style=\"color:#ffffff\">Autumn sale</p></td></tr></table>";
    let mut table = html_message(0, ("Rowan Ash", "rowan@example.org"), "Autumn", html);
    attached(&mut table);
    in_frame(false, |ctx| {
        for message in [&m, &dark_fallback, &table] {
            let laid = laid(ctx, message, false);
            assert_eq!(behind(&laid).len(), 1);
            assert_eq!(ink_of(&laid, "Autumn sale"), Color32::WHITE);
        }
    });
    // In the dark theme the cream is turned dark and the picture dimmed with it: white stays.
    in_frame(true, |ctx| {
        for message in [&m, &dark_fallback, &table] {
            assert_eq!(ink_of(&laid(ctx, message, false), "Autumn sale"), Color32::WHITE);
        }
    });
}

#[test]
fn text_with_only_a_colour_behind_it_is_adjusted_as_before() {
    let cell = |attributes: &str| hero(&format!("<td {attributes}>"));
    // A colour and no picture: white on cream is darkened in the light theme.
    let plain = cell("bgcolor=\"#f4efe6\"");
    // A picture that can't be shown (not attached): the colour alone is what is there.
    let missing = cell("background=\"cid:absent@example.org\" bgcolor=\"#f4efe6\"");
    in_frame(false, |ctx| {
        for message in [&plain, &missing] {
            let laid = laid(ctx, message, false);
            assert!(behind(&laid).is_empty());
            let ink = ink_of(&laid, "Autumn sale");
            assert_ne!(ink, Color32::WHITE);
            assert!(crate::reading::colors::contrast(ink, Color32::from_rgb(0xf4, 0xef, 0xe6)) >= crate::reading::colors::MIN_CONTRAST);
        }
    });
    // A picture dimmed with a colour turned dark (the dark theme) is read against that colour, as before:
    // black text on it is turned light.
    let html = "<table width=600><tr><td background=\"cid:hero@example.org\" bgcolor=\"#f4efe6\"><p style=\"color:#000000\">Autumn sale</p></td></tr></table>";
    let mut black = html_message(0, ("Rowan Ash", "rowan@example.org"), "Autumn", html);
    attached(&mut black);
    in_frame(true, |ctx| {
        let laid = laid(ctx, &black, false);
        assert_eq!(behind(&laid)[0].1.tint, DARK_TINT);
        assert!(crate::reading::colors::luminance(ink_of(&laid, "Autumn sale")) > 0.5);
    });
}
