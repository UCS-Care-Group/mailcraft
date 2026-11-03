// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #533: pictures written into a message's HTML (`data:image/png;base64,…`), in a message as it
//! comes: `multipart/alternative`, plain text and HTML, nothing attached. They are shown as
//! attached pictures are; nothing is fetched for them.
//!
//! The people, addresses and pictures are invented.

use super::{in_frame, lay_out, png, shown_pictures, stand_ins};
use crate::reading::images::Pictures;

/// Base64 (RFC 4648), as a mail program writes it into HTML.
fn b64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let n = group.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= group.len() { char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' });
        }
    }
    out
}

/// Quoted-printable (RFC 2045), with its soft line breaks every 72 characters: they fall inside
/// the base64, as they do in real mail.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    let mut line = 0;
    for c in text.chars() {
        let piece = if c == '=' { "=3D".to_string() } else { c.to_string() };
        if c == '\n' {
            line = 0;
        } else if line + piece.len() > 72 {
            out.push_str("=\r\n");
            line = 0;
        }
        line += piece.len();
        out.push_str(&piece);
    }
    out
}

/// A message as it comes from the server, with `pictures` written into its HTML.
fn raw_message(pictures: &[String]) -> Vec<u8> {
    let images: String = pictures.iter().map(|src| format!("<p><img src=\"{src}\" alt=\"Chart\"></p>\n")).collect();
    let html = format!("<html><body>\n<p>Hello Avery,</p>\n<p>The figures for the quarter:</p>\n{images}<p>Regards,<br>Rowan</p>\n</body></html>\n");
    format!(
        "From: Rowan Ash <rowan@example.org>\r\nTo: Avery Quinn <avery@example.org>\r\nSubject: Quarter figures\r\n\
         Date: Mon, 26 Oct 2026 09:00:00 +0000\r\nMessage-ID: <q3@example.org>\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello Avery,\r\n\r\nThe figures for the quarter.\r\n\r\nRegards,\r\nRowan\r\n\
         --b1\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}\r\n--b1--\r\n",
        quoted_printable(&html)
    )
    .into_bytes()
}

fn data_png(w: u32, h: u32) -> String {
    format!("data:image/png;base64,{}", b64(&png(w, h)))
}

#[test]
fn two_pictures_written_into_a_message_are_shown() {
    let m = mailcraft_mime::parse(&raw_message(&[data_png(64, 40), data_png(32, 32)]));
    assert!(m.attachments.is_empty(), "nothing is attached");
    assert!(m.body.html.as_deref().is_some_and(|h| h.contains("data:image/png;base64,iVBOR")));
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &m, 820.0);
            assert_eq!((laid.stats.pictures, laid.stats.placeholders, laid.stats.remote), (2, 0, 0), "{:?}", laid.stats);
            let sizes: Vec<(f32, f32)> = shown_pictures(&laid).iter().map(|i| (i.rect.width(), i.rect.height())).collect();
            assert_eq!(sizes, [(64.0, 40.0), (32.0, 32.0)]);
            // The text around them is there too.
            assert!(laid.stats.texts >= 3, "{:?}", laid.stats);
        });
    }
}

#[test]
fn what_is_written_in_but_is_no_picture_of_ours_is_a_stand_in() {
    let svg = format!("data:image/svg+xml;base64,{}", b64(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
    let html_page = format!("data:text/html;base64,{}", b64(b"<script>alert(1)</script>"));
    let not_png = format!("data:image/png;base64,{}", b64(b"no picture at all"));
    let broken = "data:image/png;base64,iVBOR*w0K".to_string();
    // A PNG's header on a body that is no PNG: past the first look, the picture checks refuse it.
    let mut damaged = png(16, 16);
    damaged.truncate(20);
    let damaged = format!("data:image/png;base64,{}", b64(&damaged));
    let m = mailcraft_mime::parse(&raw_message(&[svg, html_page, not_png, broken, damaged, data_png(24, 24)]));
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 820.0);
        // Five stand-ins, none of them for a picture on the web; the good one is shown.
        assert_eq!((laid.stats.pictures, laid.stats.placeholders, laid.stats.remote), (1, 5, 0), "{:?}", laid.stats);
        assert!(stand_ins(&laid).iter().all(|(label, remote)| *label == "Chart" && !remote), "{:?}", stand_ins(&laid));
        // Only the good picture's bytes, and the damaged one's that were looked at and refused.
        assert!(Pictures::of(ctx).lock().map(|p| p.counts().0 <= 2).unwrap_or(false));
    });
}

/// A message read again is parsed again, and its pictures come as new bytes: each picture is
/// kept once, and nothing of it is held once no message read holds it.
#[test]
fn a_message_read_again_keeps_its_pictures_once() {
    let m = mailcraft_mime::parse(&raw_message(&[data_png(64, 40), data_png(32, 32)]));
    in_frame(false, |ctx| {
        let store = Pictures::of(ctx);
        for _ in 0..6 {
            let laid = lay_out(ctx, &m, 820.0);
            assert_eq!(laid.stats.pictures, 2);
            assert_eq!(store.lock().unwrap().counts().0, 2, "one entry for each picture, however often it is read");
        }
        // The last reading is gone: so are its pictures, at the end of the next frame.
        store.lock().unwrap().release_after(0.0);
        assert_eq!(store.lock().unwrap().counts().0, 0);
    });
}

/// HTML that is nothing but pictures written into it, with a text part beside it, is shown as
/// HTML: the pictures are what it says.
#[test]
fn a_message_of_pictures_alone_shows_them_rather_than_its_text() {
    let html = format!("<html><body><img src=\"{}\"><img src=\"{}\"></body></html>", data_png(64, 40), data_png(32, 32));
    let m = mailcraft_model::Message {
        body: mailcraft_model::Body { text: "See the pictures in the HTML version.".into(), html: Some(html) },
        ..mailcraft_model::Message::default()
    };
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 820.0);
        assert_eq!((laid.stats.pictures, laid.stats.placeholders), (2, 0), "{:?}", laid.stats);
        assert!(!laid.items.iter().any(|i| super::text_of(i).contains("HTML version")));
    });
}
