// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which parts a body shows in place (AU3-15): only what an `<img>` asks for by `cid:`, and only a
//! picture by its bytes.

use proptest::prelude::*;

use crate::Attachment;
use crate::shown::{MAX_SHOWN_IDS, is_picture, is_picture_part, shown_ids, shown_in_place};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn part(mime: &str, data: &[u8]) -> Attachment {
    Attachment { content_id: "logo@a.example".into(), inline: true, ..Attachment::new("logo.png", mime, data.to_vec()) }
}

#[test]
fn only_an_img_asks_for_a_part() {
    for asks in [
        "<img src=\"cid:logo@a.example\">",
        "<IMG\nalt='x' SRC = 'CID:<logo@a.example>' />",
        "<img src=cid:logo%40a.example>",
        "<p><img width=1 src=\"cid:LOGO@a.example\"></p>",
    ] {
        assert!(shown_ids(asks).shows("logo@a.example"), "{asks}");
    }
    for says in [
        "<p>cid:logo@a.example</p>",
        "<!-- <img src=\"cid:logo@a.example\"> -->",
        "<a href=\"cid:logo@a.example\">x</a>",
        "<img alt=\"cid:logo@a.example\" src=\"https://a.example/x.png\">",
        "<image src=\"cid:logo@a.example\">",
        "<img src=\"cid:logo@a.example.evil\">",
        "<img src=\"cid:\">",
        "<img data-src=\"cid:logo@a.example\">",
    ] {
        assert!(!shown_ids(says).shows("logo@a.example"), "{says}");
    }
    // Only the first `src` of an element counts.
    assert!(!shown_ids("<img src=\"https://a.example/x\" src=\"cid:logo@a.example\">").shows("logo@a.example"));
    // An unclosed comment hides the rest.
    assert!(!shown_ids("<!-- <img src=\"cid:logo@a.example\">").shows("logo@a.example"));
}

#[test]
fn a_picture_is_told_by_its_bytes() {
    assert!(is_picture(PNG) && is_picture(&[0xFF, 0xD8, 0xFF, 0xE0]) && is_picture(b"GIF89a..") && is_picture(b"RIFF\0\0\0\0WEBPVP8 "));
    assert!(!is_picture(b"PK\x03\x04") && !is_picture(b"<svg") && !is_picture(b"") && !is_picture(b"\x89PNG"));
    let ids = shown_ids("<img src=\"cid:logo@a.example\">");
    assert!(shown_in_place(&ids, &part("image/png", PNG)));
    // Declared a picture, but not one.
    assert!(!shown_in_place(&ids, &part("image/png", b"PK\x03\x04")));
    // A picture declared otherwise is one.
    assert!(shown_in_place(&ids, &part("application/octet-stream", PNG)));
    // Kept outside memory, it is what it was said to be.
    let kept = |mime: &str| Attachment { kept: Some(crate::ContentRef { sha256: [0; 32], len: 9 }), ..part(mime, b"") };
    assert!(is_picture_part(&kept("image/jpeg")) && !is_picture_part(&kept("application/zip")));
}

#[test]
fn the_ids_kept_are_capped() {
    let html = "<img src=\"cid:x\">".repeat(MAX_SHOWN_IDS * 2);
    assert!(shown_ids(&html).shows("x"));
    let many: String = (0..MAX_SHOWN_IDS * 2).map(|i| format!("<img src=\"cid:{i}\">")).collect();
    let ids = shown_ids(&many);
    assert!(ids.shows("0") && !ids.shows(&format!("{}", MAX_SHOWN_IDS * 2 - 1)));
}

proptest! {
    #[test]
    fn any_text_is_read_without_a_panic(html in "(<|>|img|src|=|\"|'|cid:|%4|-->|<!--| |\\PC){0,200}") {
        let ids = shown_ids(&html);
        let _ = ids.shows("x");
    }
}
