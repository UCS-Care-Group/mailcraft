// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #601: pictures behind a box: the message's own always, the ones on the web under the rules of
//! every other picture on the web.
//!
//! The people, addresses and pictures are invented.

use std::sync::Arc;

use egui::Color32;
use mailcraft_model::Attachment;

use super::{html_message, in_frame, png};
use crate::reading::backdrop::DARK_TINT;
use crate::reading::images::{Pictures, Remote};
use crate::reading::layout::{BackdropItem, ItemKind, Laid};
use crate::reading::view::Doc;

/// `m` laid out to the end at 820 points, showing the pictures from the web that have come when
/// `remote`.
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

#[test]
fn a_picture_on_the_web_behind_a_box_waits_for_the_owner() {
    let url = "https://img.example.org/2026/hero.png";
    let m = hero(&format!("<td style=\"background:#203040 url('{url}') center / cover no-repeat\">"));
    // It is one of the message's pictures on the web, asked for with the others.
    let doc = Doc::of(&m);
    assert_eq!((doc.remote.len(), doc.remote_pictures), (1, 1));
    assert_eq!(&*doc.remote[0], url);
    in_frame(false, |ctx| {
        // The bytes are at hand (another message's, say): without the owner's say so they are not shown.
        Pictures::of(ctx).lock().unwrap().set_remote(Arc::from(url), Remote::Ready(Arc::new(png(60, 20))), 0);
        assert!(behind(&laid(ctx, &m, false)).is_empty());
        let shown = laid(ctx, &m, true);
        let behind = behind(&shown);
        assert_eq!(behind.len(), 1);
        // `cover`: as small as fills the cell, its shape kept.
        let (rect, item) = behind[0];
        assert!(item.tile.x >= rect.width() - 0.5 && item.tile.y >= rect.height() - 0.5, "{item:?} in {rect:?}");
        assert!((item.tile.x / item.tile.y - 3.0).abs() < 0.01);
    });
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
