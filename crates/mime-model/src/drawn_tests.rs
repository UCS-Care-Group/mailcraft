// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which parts an HTML body draws (#1970): the same parts `part_for` finds, found in one pass
//! however many parts and pictures a hostile message has. Invented addresses only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mailcraft_model::Attachment;
use mailcraft_model::shown::part_for;

use crate::drawn_parts;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn part(content_id: &str) -> Attachment {
    Attachment { content_id: content_id.into(), ..Attachment::new("p.png", "image/png", PNG.to_vec()) }
}

#[test]
fn the_parts_drawn_are_the_ones_part_for_finds() {
    let parts = vec![part("Logo@A.example"), part("logo@a.example"), part("x y@a.example"), part("only@a.example"), part("only@a.example")];
    for (src, want) in [
        ("cid:logo@a.example", Some(1)),
        ("cid:Logo@A.example", Some(0)),
        ("cid:LOGO@A.EXAMPLE", Some(0)),
        ("cid:<logo%40a.example>", Some(1)),
        ("cid:X%20Y@A.EXAMPLE", Some(2)),
        ("cid:only@a.example", Some(3)),
        ("cid:none@a.example", None),
    ] {
        assert_eq!(part_for(&parts, src).map(|(i, _)| i), want, "{src}");
        let drawn = drawn_parts(&format!("<p>Hi</p><img src=\"{src}\">"), &parts);
        assert_eq!(drawn.into_iter().collect::<Vec<_>>(), want.into_iter().collect::<Vec<_>>(), "{src}");
    }
}

#[test]
fn a_picture_of_two_pixels_or_less_draws_nothing() {
    let parts = vec![part("logo@a.example")];
    for size in ["width=\"2\" height=\"2\"", "width=\"1\"", "height=\"2\"", "style=\"width:2px;height:2px\""] {
        assert!(drawn_parts(&format!("<p>Hi</p><img src=\"cid:logo@a.example\" {size}>"), &parts).is_empty(), "{size}");
    }
    assert_eq!(drawn_parts("<p>Hi</p><img src=\"cid:logo@a.example\" width=\"3\" height=\"3\">", &parts).len(), 1);
}

#[test]
fn many_parts_and_many_pictures_that_match_none_are_looked_through_quickly() {
    // Ids of one length that differ only at their end: comparing each picture with each part
    // would read them whole, 20,000 x 1,000 times over.
    let id = |kind: &str, n: usize| format!("{}{kind}{n:06}@a.example", "q".repeat(60));
    let parts: Vec<Attachment> = (0..1_000).map(|n| Attachment { data: Arc::new(PNG.to_vec()), ..part(&id("p", n)) }).collect();
    let html: String = (0..20_000).map(|n| format!("<img src=\"cid:{}\">", id("i", n))).collect();
    assert_eq!(mailcraft_html::images(&mailcraft_html::parse_text(&html)).len(), 20_000);
    let started = Instant::now();
    assert!(drawn_parts(&html, &parts).is_empty());
    // (Tens of milliseconds, laying out the HTML included; a part-by-part search takes seconds.)
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}
