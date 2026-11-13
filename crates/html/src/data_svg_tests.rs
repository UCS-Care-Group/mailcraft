// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #565: SVG pictures written into a message (`data:image/svg+xml,…`): taken as the drawing's
//! own bytes, base64 or percent-encoded, within the same caps as the others. Drawing it, with
//! nothing it refers to ever loaded, is the reading pane's (`ui-egui/src/reading/svg.rs`).

use crate::data_tests::{b64, img, pictures};
use crate::{BlockKind, MAX_DATA_PICTURE_BYTES, MAX_DATA_PICTURES, images, is_embedded, looks_like_svg, parse};

const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'><rect width='20' height='10' fill='#2e8b57'/></svg>";

/// `text` with every byte that isn't a letter or a digit written as `%xx`, as mail programs write
/// an SVG into an address.
fn percent(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}

#[test]
fn an_svg_written_into_the_message_is_shown() {
    for src in [
        format!("data:image/svg+xml;base64,{}", b64(SVG.as_bytes())),
        format!("DATA:Image/SVG+XML;charset=utf-8;base64,{}", b64(SVG.as_bytes())),
        // Percent-encoded, as most mail programs and web pages write one.
        format!("data:image/svg+xml;charset=utf-8,{}", percent(SVG)),
        format!("data:image/svg+xml,{}", percent(SVG)),
        // As written, only its quotes changed for the attribute's sake.
        format!("data:image/svg+xml;utf8,{}", SVG.replace('#', "%23")),
    ] {
        let shown = pictures(&img(&src));
        assert_eq!(shown.len(), 1, "{src}");
        let (kept, data) = &shown[0];
        // The address kept says what it is, never its content.
        assert_eq!(kept, "data:image/svg+xml", "{src}");
        assert!(is_embedded(kept));
        assert_eq!(data.as_deref(), Some(SVG.as_bytes()), "{src}");
    }
}

#[test]
fn what_is_said_to_be_an_svg_must_be_one_and_the_other_way_round() {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    for src in [
        // A drawing's type on bytes that are no drawing.
        format!("data:image/svg+xml;base64,{}", b64(png)),
        format!("data:image/svg+xml,{}", percent("<html><body><svg></svg></body></html>")),
        format!("data:image/svg+xml,{}", percent("just some words")),
        format!("data:image/svg+xml,{}", percent("<svgx/>")),
        "data:image/svg+xml,".to_string(),
        // Bad base64 or percent-encoding is no drawing either.
        "data:image/svg+xml;base64,PHN2Zy*+".to_string(),
        // A picture's type on a drawing.
        format!("data:image/png;base64,{}", b64(SVG.as_bytes())),
        format!("data:image/gif,{}", percent(SVG)),
        // Anything else that is text.
        format!("data:image/svg;base64,{}", b64(SVG.as_bytes())),
        format!("data:text/xml;base64,{}", b64(SVG.as_bytes())),
    ] {
        let shown = pictures(&img(&src));
        assert_eq!(shown.len(), 1, "{src}: a placeholder stands for it");
        assert!(shown[0].1.is_none(), "{src}");
    }
}

#[test]
fn an_svg_is_known_by_its_first_element() {
    for yes in [
        SVG.to_string(),
        format!("\u{feff}{SVG}"),
        format!("  \r\n\t{SVG}"),
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Generator: a drawing program -->\n{SVG}"),
        format!("<?xml version='1.0'?><!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">{SVG}"),
        format!("<!DOCTYPE svg [ <!ELEMENT x ANY> ]><?pi x?>{SVG}"),
        "<svg/>".to_string(),
        "<svg>".to_string(),
        "<svg\nviewBox='0 0 1 1'></svg>".to_string(),
    ] {
        assert!(looks_like_svg(yes.as_bytes()), "{yes:?}");
    }
    for no in [
        "",
        "svg",
        "<svgx>",
        "<SVG></SVG>",
        "<html><svg></svg></html>",
        "<!-- <svg> -->",
        "<!-- never closed <svg>",
        "<?xml version='1.0'?>",
        "<!DOCTYPE html><svg></svg>",
        "<!DOCTYPE svg [ <svg/>",
        "\u{89}PNG",
    ] {
        assert!(!looks_like_svg(no.as_bytes()), "{no:?}");
    }
    // However many comments it begins with, it is looked at for no longer than its length.
    let comments = "<!---->".repeat(10_000) + SVG;
    assert!(looks_like_svg(comments.as_bytes()));
}

#[test]
fn an_svg_takes_its_share_of_the_caps() {
    // Over the cap for one picture, percent-encoded or not.
    let big = format!("<svg xmlns='http://www.w3.org/2000/svg'><desc>{}</desc></svg>", "x".repeat(MAX_DATA_PICTURE_BYTES));
    assert!(pictures(&img(&format!("data:image/svg+xml,{big}")))[0].1.is_none());
    assert!(pictures(&img(&format!("data:image/svg+xml;base64,{}", b64(big.as_bytes()))))[0].1.is_none());
    // And one of the pictures a message may have.
    let one = img(&format!("data:image/svg+xml,{}", percent(SVG)));
    let shown = pictures(&one.repeat(MAX_DATA_PICTURES + 3));
    assert_eq!(shown.len(), MAX_DATA_PICTURES + 3);
    assert_eq!(shown.iter().filter(|(_, d)| d.is_some()).count(), MAX_DATA_PICTURES);
}

#[test]
fn a_drawing_in_the_html_itself_is_still_not_drawn() {
    // Only an `<img>` of a drawing is a picture: an `<svg>` element in the message is not drawn
    // (its words are read as they were before #565).
    let blocks = parse(&format!("<p>Before</p>{SVG}<svg><text>Drawn words</text></svg><p>After</p>"));
    assert!(images(&blocks).is_empty());
    assert!(blocks.iter().all(|b| !matches!(b.kind, BlockKind::Image { .. })));
}
