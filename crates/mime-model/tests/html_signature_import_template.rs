// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The importer asks for the files the signature keeps, and refuses what it can't: a picture in a
//! `<template>` (whose content no message keeps) asks for no file, but one after a template that
//! an element left out whole ended (an `svg`, a `noscript`, a hidden block) does; a picture shown in one place
//! and hidden in another is refused, as one hidden everywhere is; and a document whose pictures'
//! new addresses would make it longer than a signature's HTML may be is refused before any file
//! is asked for.
//!
//! The documents and pictures are invented; nothing is read from disk or fetched.

use mailcraft_mime_model::signature_import::{Asset, ImportError, import, required_assets};
use mailcraft_model::signature::MAX_HTML;

const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn png(name: &str) -> Asset {
    Asset { name: name.to_string(), data: ONE_PIXEL_PNG.to_vec() }
}

#[test]
fn a_picture_in_a_template_asks_for_no_file() {
    let html = "<p>Ana Pereira</p><template><img src=\"draft.png\"></template><img src=\"logo.png\" width=\"120\">";
    assert_eq!(required_assets(html), Ok(vec!["logo.png".to_string()]));
    let sig = import("Bindery", html, &[png("logo.png")]).expect("the files asked for are enough");
    assert_eq!(sig.pictures().len(), 1);
}

#[test]
fn a_picture_after_a_template_an_element_left_out_whole_ended_asks_for_its_file() {
    for shape in ["", "<math><template></math>", "<noscript><template></noscript>", "<select><template></select>", "<p hidden><template></p>"] {
        let html = format!("<p>Ana Pereira</p>{shape}<svg><template></svg><img src=\"a.png\">");
        assert_eq!(required_assets(&html), Ok(vec!["a.png".to_string()]), "{html}");
    }
    assert_eq!(required_assets("<svg><template></svg><img src=\"a.png\">"), Ok(vec!["a.png".to_string()]));
    let html = "<p>Ana Pereira</p><svg><template></svg><img src=\"a.png\" width=\"120\">";
    // The file asked for is the one the picture needs. The reading pane, which keeps its own count
    // of templates, doesn't show that picture, so it doesn't stand where the owner put it, and the
    // import refuses the document rather than make a signature whose picture goes out unseen.
    assert_eq!(import("Bindery", html, &[png("a.png")]).map(|_| ()), Err(ImportError::UnsupportedRepresentation));
}

#[test]
fn a_picture_shown_once_and_hidden_once_is_refused() {
    let html = "<p>Ana Pereira</p><img src=\"logo.png\" width=\"120\"><div style=\"display:none\"><img src=\"logo.png\" width=\"120\"></div>";
    assert_eq!(import("Bindery", html, &[png("logo.png")]), Err(ImportError::UnsupportedRepresentation));
    // Control: shown twice, under two spellings of its name, it is one picture.
    let twice = "<p>Ana Pereira</p><img src=\"logo.png\" width=\"120\"><p>Lisbon</p><img src=\"./logo.png\" width=\"120\">";
    let sig = import("Bindery", twice, &[png("logo.png")]).expect("shown twice");
    assert_eq!(sig.pictures().len(), 1);
    assert_eq!(sig.body().pictures(), [1, 1]);
}

/// A document of exactly `len` bytes that names `logo.png` once, made long by a comment, which adds
/// nothing to the signature's text.
fn sized(len: usize) -> String {
    let head = "<p>Ana Pereira</p><img src=\"logo.png\" width=\"120\"><!--";
    let tail = "-->";
    format!("{head}{}{tail}", "a".repeat(len - head.len() - tail.len()))
}

#[test]
fn a_document_its_picture_addresses_would_make_too_long_asks_for_no_file() {
    // `logo.png` (8 bytes) is written `cid:sig-1@signature.invalid` (27 bytes): 19 bytes more.
    let at_cap = sized(MAX_HTML);
    assert_eq!(import("Bindery", &at_cap, &[png("logo.png")]), Err(ImportError::RewrittenHtmlTooLarge), "fixture guard");
    assert_eq!(required_assets(&at_cap), Err(ImportError::RewrittenHtmlTooLarge));
    // The longest document that still fits is asked for, and imported.
    let fits = sized(MAX_HTML - 19);
    assert_eq!(required_assets(&fits), Ok(vec!["logo.png".to_string()]));
    assert_eq!(import("Bindery", &fits, &[png("logo.png")]).map(|_| ()), Ok(()), "the longest document that fits is imported");
    let over = sized(MAX_HTML - 18);
    assert_eq!(required_assets(&over), Err(ImportError::RewrittenHtmlTooLarge));
}
