// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #602: the pictures written into a message (`data:`) under a reply or forward. Each one the
//! reading pane would show is handed over to be sent as a part of its own and is referred to by
//! its Content-ID; anything else is left out, as before. The pictures are invented.

use super::*;
use crate::{MAX_DATA_PICTURE_BYTES, MAX_DATA_PICTURES, MAX_INPUT, Picture};

// The test PNG and base64 are the ones the `data:` tests use.
use crate::data_tests::{PNG, b64};

fn png() -> String {
    format!("data:image/png;base64,{}", b64(&PNG))
}

/// A PNG of `len` bytes or a little more: the real header, then zeros.
fn png_of(len: usize) -> String {
    let zeros = len.saturating_sub(PNG.len()).div_ceil(3) * 4;
    format!("data:image/png;base64,{}{}", b64(&PNG), "A".repeat(zeros))
}

/// `html` quoted, each picture handed over named `p<n>`; the pictures as they were handed over.
fn quoted(html: &str) -> (String, Vec<(String, Picture)>) {
    let mut handed = Vec::new();
    let out = quotable_with_pictures(html, &mut |mime: &str, picture: &Picture| {
        handed.push((mime.to_string(), picture.clone()));
        Some(format!("p{}@mailcraft.invalid", handed.len()))
    });
    (out, handed)
}

#[test]
fn a_picture_written_into_the_message_is_carried_as_a_part_of_its_own() {
    let (out, handed) = quoted(&format!("<p>Before</p><img src=\"{}\" alt=\"Chart\" width=\"40\"><p>After</p>", png()));
    assert_eq!(out, "<p>Before</p><img src=\"cid:p1@mailcraft.invalid\" alt=\"Chart\" width=\"40\"><p>After</p>");
    assert_eq!(handed.len(), 1);
    assert_eq!((handed[0].0.as_str(), handed[0].1.to_vec()), ("image/png", PNG.to_vec()));
    // Its type is read from the address in any letter case, with the breaks a program puts in it.
    let (out, handed) = quoted(&format!("<img src=\"DATA:Image/PNG;Base64,\r\n{}\">", b64(&PNG)));
    assert_eq!((out.as_str(), handed.len()), ("<img src=\"cid:p1@mailcraft.invalid\">", 1));
    // Each of the four kinds, by its own bytes.
    for (kind, bytes) in [
        ("image/jpeg", vec![0xff, 0xd8, 0xff, 0xe0, 0, 0]),
        ("image/gif", b"GIF89a\x01\x00".to_vec()),
        ("image/webp", b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec()),
    ] {
        let (out, handed) = quoted(&format!("<img src=\"data:{kind};base64,{}\">", b64(&bytes)));
        assert_eq!(out, "<img src=\"cid:p1@mailcraft.invalid\">", "{kind}");
        assert_eq!((handed[0].0.as_str(), handed[0].1.to_vec()), (kind, bytes), "{kind}");
    }
}

#[test]
fn what_is_not_a_picture_or_was_hidden_is_left_out_and_never_handed_over() {
    let not_png = format!("data:image/png;base64,{}", b64(b"<html><script>x()</script></html>"));
    let svg = format!("data:image/svg+xml;base64,{}", b64(b"<svg onload=\"x()\"/>"));
    for gone in [
        format!("<img src=\"{not_png}\">"),
        format!("<img src=\"{svg}\">"),
        "<img src=\"data:text/html,<script>x()</script>\">".to_string(),
        "<img src=\"data:image/png,%89PNG\">".to_string(),
        "<img src=\"data:image/png;base64,***\">".to_string(),
        "<img src=\"data:\">".to_string(),
        format!("<img src=\"{}\" style=\"display:none\">", png()),
        format!("<img src=\"{}\" hidden>", png()),
        format!("<div style=\"display:none\"><img src=\"{}\"></div>", png()),
        format!("<style>.x{{display:none}}</style><img class=\"x\" src=\"{}\">", png()),
        format!("<script><img src=\"{}\"></script>", png()),
    ] {
        let (out, handed) = quoted(&gone);
        assert!(!out.contains("<img") && !out.to_ascii_lowercase().contains("data:"), "{gone}: {out}");
        assert!(handed.is_empty(), "{gone}");
    }
    // Nor anywhere but a picture's address: a link or a background is no picture.
    let (out, handed) = quoted(&format!("<a href=\"{0}\">x</a><td background=\"{0}\">y</td><p style=\"background:url({0})\">z</p>", png()));
    assert!(!out.contains("data:") && handed.is_empty(), "{out}");
}

#[test]
fn a_picture_too_small_to_see_is_left_out_and_takes_nothing_of_the_caps() {
    for tiny in [
        format!("<img src=\"{}\" width=\"1\" height=\"1\">", png()),
        format!("<img src=\"{}\" width=\"1\">", png()),
        format!("<img src=\"{}\" style=\"width:1px\">", png()),
        format!("<img src=\"{}\" style=\"height:2px\">", png()),
        format!("<style>.x{{width:1px}}</style><img class=\"x\" src=\"{}\">", png()),
    ] {
        let (out, handed) = quoted(&tiny);
        assert!(!out.contains("<img") && handed.is_empty(), "{tiny}: {out}");
    }
    // As many as the reading pane shows at most, then a picture to see: that one still goes.
    let tiny: String = (0..MAX_DATA_PICTURES).map(|_| format!("<img src=\"{}\" width=\"1\" height=\"1\">", png())).collect();
    let (out, handed) = quoted(&format!("{tiny}<img src=\"{}\" alt=\"Chart\">", png()));
    assert_eq!(out, "<img src=\"cid:p1@mailcraft.invalid\" alt=\"Chart\">");
    assert_eq!(handed.len(), 1);
}

#[test]
fn a_picture_refused_by_the_caller_is_left_out() {
    let out = quotable_with_pictures(&format!("<p>a</p><img src=\"{}\" alt=\"Chart\">", png()), &mut |_: &str, _: &Picture| None);
    assert_eq!(out, "<p>a</p>");
}

#[test]
fn without_a_place_for_its_pictures_the_quote_is_as_before() {
    let html = format!("<p>a</p><img src=\"{}\"><img src=\"cid:logo\">", png());
    assert_eq!(quotable(&html), "<p>a</p><img src=\"cid:logo\">");
}

#[test]
fn no_more_pictures_than_the_reading_pane_shows_are_handed_over() {
    let many: String = (0..MAX_DATA_PICTURES + 3).map(|_| format!("<img src=\"{}\">", png())).collect();
    let (out, handed) = quoted(&many);
    assert_eq!(handed.len(), MAX_DATA_PICTURES);
    assert_eq!(out.matches("<img").count(), MAX_DATA_PICTURES);
    // One over the size of one picture is left out; one at it is taken.
    let (out, handed) = quoted(&format!("<img src=\"{}\"><img src=\"{}\">", png_of(MAX_DATA_PICTURE_BYTES + 3), png_of(MAX_DATA_PICTURE_BYTES - 3)));
    assert_eq!((out.matches("<img").count(), handed.len()), (1, 1));
    assert!(handed[0].1.len() <= MAX_DATA_PICTURE_BYTES);
}

#[test]
fn the_text_after_large_pictures_is_still_quoted() {
    // Three pictures of 2 MB come to far more text than is read of a message without them.
    let big: String = (0..3).map(|_| format!("<p><img src=\"{}\"></p>", png_of(2 * 1024 * 1024))).collect();
    let (out, handed) = quoted(&format!("<p>Hello</p>{big}<p>Regards, Rowan</p>"));
    assert_eq!(handed.len(), 3);
    assert!(out.ends_with("<p>Regards, Rowan</p>"), "{}", out.get(out.len().saturating_sub(200)..).unwrap_or(""));
    // The rest of the message is still read only as far as it would be without them.
    let long = "x".repeat(MAX_INPUT + 1000);
    let (out, _) = quoted(&format!("<p>{}</p><img src=\"{}\"><p>{long}</p><p>END</p>", "y".repeat(10), png()));
    assert!(!out.contains("END") && out.len() <= MAX_INPUT + 200, "{}", out.len());
    // A picture past that point is not handed over either.
    let (out, handed) = quoted(&format!("<p>{long}</p><img src=\"{}\">", png()));
    assert!(handed.is_empty() && !out.contains("<img"));
}

#[test]
fn hostile_input_never_panics() {
    for html in [
        "<img src=\"data:image/png;base64,",
        "<img src=data:image/png;base64,iVBORw0KGgo",
        "<img src=\"data:image/png;base64,iVBORw0KGgo=====AAAA\">",
        "<img src=\"data:image/png;;;;base64,\u{0}\">",
        "<img src=\"data:image/pngé;base64,é\">",
        "<img\u{0} src=\"data:image/gif;base64,R0lGODlh\">",
        "<body><img src=\"data:image/gif;base64,R0lGODlh\"></body><img src=\"data:image/gif;base64,R0lGODlh\">",
    ] {
        let _ = quoted(html);
    }
}

proptest::proptest! {
    #[test]
    fn any_input_is_quoted_without_panicking(html in "(<img src=\"data:image/(png|gif);base64,[A-Za-z0-9+/=\r\n]{0,40}\">|<[a-z]{1,4}>|[a-z <>\"=]{0,10}){0,12}") {
        let (out, handed) = quoted(&html);
        proptest::prop_assert!(!out.to_ascii_lowercase().contains("src=\"data:"));
        proptest::prop_assert!(handed.len() <= MAX_DATA_PICTURES);
    }
}
