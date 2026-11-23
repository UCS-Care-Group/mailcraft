// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png
}

#[test]
fn html_is_read_as_utf8_or_utf16() {
    assert_eq!(decode_html("<b>é</b>".as_bytes()), "<b>é</b>");
    assert_eq!(decode_html(b"\xEF\xBB\xBF<i>x</i>"), "<i>x</i>");
    let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain("<p>é".encode_utf16().flat_map(u16::to_le_bytes)).collect();
    assert_eq!(decode_html(&le), "<p>é");
    let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("<p>é".encode_utf16().flat_map(u16::to_be_bytes)).collect();
    assert_eq!(decode_html(&be), "<p>é");
}

#[test]
fn hostile_html_bytes_are_repaired_not_refused() {
    assert_eq!(decode_html(b""), "");
    assert_eq!(decode_html(b"\xFF\xFE\x3C"), "");
    assert_eq!(decode_html(b"\xFF\xFE"), "");
    assert!(decode_html(b"<p>\xFF\xFE</p>").contains('\u{FFFD}'));
}

#[test]
fn a_png_must_be_a_png_of_a_size_that_is_taken() {
    assert!(png_fits(&png_header(1, 1)));
    assert!(png_fits(&png_header(MAX_PICTURE_SIDE as u32, MAX_PICTURE_SIDE as u32)));
    assert!(!png_fits(&png_header(MAX_PICTURE_SIDE as u32 + 1, 1)));
    assert!(!png_fits(&png_header(1, MAX_PICTURE_SIDE as u32 + 1)));
    assert!(!png_fits(&png_header(0, 5)));
    assert!(!png_fits(&png_header(u32::MAX, u32::MAX)));
}

#[test]
fn bytes_that_are_not_a_png_are_refused() {
    assert!(!png_fits(b""));
    assert!(!png_fits(b"\x89PNG\r\n\x1a\n"));
    assert!(!png_fits(b"GIF89a\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"));
    let mut wrong_chunk = png_header(2, 2);
    wrong_chunk[12..16].copy_from_slice(b"IDAT");
    assert!(!png_fits(&wrong_chunk));
    let mut huge = png_header(2, 2);
    huge.resize(MAX_PICTURE_BYTES + 1, 0);
    assert!(!png_fits(&huge));
}

#[test]
fn the_types_asked_for_are_in_order() {
    assert_eq!(HTML_MIMES[0], "text/html;charset=utf-8");
    assert_eq!(IMAGE_MIMES, ["image/png"]);
}
