// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature imported as HTML the owner wrote, plus the picture files beside it: the HTML is
//! kept byte for byte except each local picture's `src`, which becomes the address the signature
//! holds it under; the picture's bytes are kept as one payload however many places name it.
//!
//! The first test is the whole path end to end. The rest are the refusals of this slice, one
//! branch each: a name that is not a relative resource name, an address that is unsafe or does
//! not resolve, a file that is missing, unused or named twice, the model's own quotas (each at the
//! limit and one past it), a document that grows past its cap, and a picture layout the stand-in
//! would not keep.

use mailcraft_html::ImageSourceError;
use mailcraft_mime_model::rich::internal_cid;
use mailcraft_mime_model::signature_import::{Asset, ImportError, import};
use mailcraft_model::Signature;
use mailcraft_model::signature::{Layout, MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES, MAX_PICTURES_BYTES};

/// A one-pixel PNG, whole and valid (67 bytes). Only its kind matters here: the model reads a
/// picture's type from its own first bytes, and this fixture says nothing about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The document in pieces, cut where each of the two picture values sits. The decoys are markup
/// only in appearance: in a style sheet, in a comment, in another tag's attribute and as text.
const PARTS: [&str; 5] = [
    "<!DOCTYPE html>\n<HTML lang=\"en\"><head><meta charset=\"utf-8\">\n<title>Signature</title>\n<style>p::before { content: '<img src=\"cid:sig-1@signature.invalid\" width=\"9\" alt=\"decoy\">' }</style>\n</head>\n<body class=\"café\" id=\"sig-root\">\n<table width=\"520\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:520px;background-color:#fdf6e3;padding:12px\"><tr>\n<td style=\"vertical-align:top;padding-right:16px\"><img src=",
    "pictures/caf&eacute;%20line.png",
    " alt=\"Studio mark\" width=\"120\" height=\"80\" style=\"border:0\"></td>\n<td style=\"font-family:Georgia,serif\">\n<table cellpadding=\"0\" border=\"0\"><tr><td><div>Priya Raman</div><div>Northwind Atelier</div></td></tr></table>\n</td>\n</tr></table>\n<!-- <img src=\"pictures/café line.png\" alt=\"decoy comment\"> -->\n<p data-note='<img src=\"decoy-attr.png\">'>Text says &lt;img src=\"decoy-text.png\"&gt; here.</p>\n<div><img src=",
    "./pictures/caf%C3%A9%20line.png",
    " width=\"64\" style=\"border:0\"></div>\n</body></HTML>",
];

/// The whole document, with the two picture values replaced by the two given.
fn spliced(values: [&str; 2]) -> String {
    let mut out = String::new();
    out.push_str(PARTS[0]);
    out.push_str(values[0]);
    out.push_str(PARTS[2]);
    out.push_str(values[1]);
    out.push_str(PARTS[4]);
    out
}

/// One document with one supplied picture named `name`.
fn one_asset(html: &str, name: &str) -> Result<Signature, ImportError> {
    import("Work", html, &[Asset { name: name.to_string(), data: ONE_PIXEL_PNG.to_vec() }])
}

/// A header-valid file of exactly `len` bytes: the one-pixel picture and zeros past its end. It is
/// a size probe, not a decodable picture: the model reads a picture's kind from its first bytes
/// alone, and nothing here reads pixels.
fn sized_png(len: usize) -> Vec<u8> {
    assert!(len >= ONE_PIXEL_PNG.len());
    let mut data = ONE_PIXEL_PNG.to_vec();
    data.resize(len, 0);
    data
}

#[test]
fn one_picture_named_twice_imports_as_one_payload_with_only_the_two_sources_rewritten() {
    let html = spliced([PARTS[1], PARTS[3]]);
    let mut assets = vec![Asset { name: "pictures/café line.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let assets_before = assets.clone();
    let html_before = html.clone();

    let sig = import("Work", &html, &assets).expect("the HTML and the one picture it names import");

    // Neither input was changed.
    assert_eq!(assets, assets_before, "the supplied assets are read, not changed");
    assert_eq!(html, html_before, "the HTML is read, not changed");
    assets.clear();

    // The owner's document, every byte of it, but for the two source values: the doctype's case,
    // the head, the class and id, the inline styles, the tables, the comment, the attribute decoy
    // and the white space stay exactly as written.
    let one = "cid:sig-1@signature.invalid".to_string();
    let expected = spliced([&one, &one]);
    assert_eq!(sig.html(), Some(expected.as_str()), "only the two picture source values changed");

    // The picture the owner named is one payload, held once.
    let pictures = sig.pictures();
    assert_eq!(pictures.len(), 1, "one file named twice is one picture");
    let picture = &pictures[0];
    assert_eq!(picture.data.as_slice(), ONE_PIXEL_PNG, "the file's bytes, unchanged");
    assert_eq!(picture.mime, "image/png");
    assert_eq!(picture.name, "café line.png", "the attached name is the file's own, without its folders");
    assert_eq!(picture.width, Some(120), "the first real picture's width, not the style sheet's decoy");
    assert_eq!(picture.alt, "Studio mark", "the first real picture's description, not a decoy's");

    // The address the signature holds it under is the module's own, not another prefix.
    assert_eq!(sig.html().map(|own| own.contains(&format!("cid:{}", internal_cid(picture)))), Some(true));

    // Both places the owner put the picture stand in the text: the one text is the stand-in, a
    // message being written shows it, and it holds the picture twice.
    assert_eq!(sig.body().pictures(), vec![picture.id, picture.id], "a repeated picture keeps both places");
    assert_eq!(sig.layout, Layout::Inline, "an arbitrary table layout is in line");

    // The signature as it is kept and read back is the one that was made.
    let json = serde_json::to_string(&sig).expect("a signature is written as JSON");
    let back: Signature = serde_json::from_str(&json).expect("a signature reads back");
    assert_eq!(back, sig, "the model reads back what it wrote");
}

// ---- the signature's name ----

#[test]
fn a_name_of_nothing_but_space_is_refused() {
    assert_eq!(import("   ", "<p>text</p>", &[]), Err(ImportError::BadName));
}

// ---- an asset's name is a relative POSIX resource name ----

#[test]
fn an_absolute_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "/pics/logo.png").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn a_drive_lettered_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "C:/pics/logo.png").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn a_traversing_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "../pics/logo.png").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn a_backslash_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "pics\\logo.png").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn a_query_in_an_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "pics/logo.png?v=2").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn a_control_character_in_an_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "pics/lo\u{7}go.png").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn an_empty_component_in_an_asset_name_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\">", "pics//logo.png").unwrap_err(), ImportError::BadAssetName);
    assert_eq!(one_asset("<img src=\"logo.png\">", "pics/").unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn an_asset_name_over_its_byte_cap_is_refused() {
    let long = format!("{}.png", "a".repeat(4_100));
    assert_eq!(one_asset("<img src=\"logo.png\">", &long).unwrap_err(), ImportError::BadAssetName);
}

#[test]
fn dot_components_in_an_asset_name_normalize_away() {
    let sig = one_asset("<img src=\"pics/logo.png\">", "./pics/./logo.png").expect("a dotted relative name is the same file");
    assert_eq!(sig.pictures()[0].name, "logo.png", "the name a picture is shown by is its last component");
}

// ---- a picture's address is a relative reference ----

#[test]
fn a_traversing_reference_is_refused() {
    assert_eq!(one_asset("<img src=\"../pics/logo.png\">", "pics/logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn an_encoded_separator_in_a_reference_is_refused() {
    assert_eq!(one_asset("<img src=\"pics%2Flogo.png\">", "pics/logo.png").unwrap_err(), ImportError::BadReference);
    assert_eq!(one_asset("<img src=\"pics%5Clogo.png\">", "pics/logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_malformed_percent_escape_is_refused() {
    // The file whose name is literally `logo%2.png` exists, so only the escape's shape can refuse.
    assert_eq!(one_asset("<img src=\"logo%2.png\">", "logo%2.png").unwrap_err(), ImportError::BadReference);
    assert_eq!(one_asset("<img src=\"logo%zz.png\">", "logo%zz.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_percent_escape_that_is_not_utf8_is_refused() {
    assert_eq!(one_asset("<img src=\"logo%FF.png\">", "logo%FF.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_data_picture_is_refused() {
    assert_eq!(one_asset("<img src=\"data:image/png;base64,iVBORw0KGgo=\">", "logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_cid_reference_is_refused() {
    assert_eq!(one_asset("<img src=\"cid:logo@example.invalid\">", "logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_file_address_is_refused() {
    assert_eq!(one_asset("<img src=\"file:///home/someone/logo.png\">", "logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn a_schemeless_host_address_is_refused() {
    assert_eq!(one_asset("<img src=\"//example.org/logo.png\">", "logo.png").unwrap_err(), ImportError::BadReference);
}

#[test]
fn an_unsafe_web_address_is_refused() {
    assert_eq!(one_asset("<img src=\"http://\">", "logo.png").unwrap_err(), ImportError::BadReference);
}

// ---- what is named, and what is supplied ----

#[test]
fn an_address_that_names_no_supplied_file_is_refused() {
    assert_eq!(one_asset("<img src=\"pics/other.png\">", "pics/logo.png").unwrap_err(), ImportError::MissingAsset);
}

#[test]
fn a_supplied_file_no_picture_names_is_refused() {
    let assets = vec![Asset { name: "logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    assert_eq!(import("Work", "<p>no picture here</p>", &assets).unwrap_err(), ImportError::UnusedAsset);
}

#[test]
fn two_names_of_one_resource_are_refused_even_with_the_same_bytes() {
    let assets = vec![
        Asset { name: "./pics/logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() },
        Asset { name: "pics/logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() },
    ];
    assert_eq!(import("Work", "<img src=\"pics/logo.png\">", &assets).unwrap_err(), ImportError::DuplicateAsset);
}

#[test]
fn a_literal_percent_in_a_file_name_is_a_percent_escape_in_its_address() {
    // The asset's name is a filesystem label and is never percent-decoded; the address is, once.
    let assets = vec![Asset { name: "pics/a%20b.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let sig = import("Work", "<img src=\"pics/a%2520b.png\">", &assets).expect("`%25` in the address is the file's own `%`");
    assert_eq!(sig.pictures()[0].name, "a%20b.png");
    assert_eq!(import("Work", "<img src=\"pics/a%20b.png\">", &assets).unwrap_err(), ImportError::MissingAsset);
}

#[test]
fn spaces_an_ampersand_unicode_and_percent_in_a_file_name_are_kept() {
    let assets = vec![Asset { name: "pics/café & 100%.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let sig = import("Work", "<img src=\"pics/caf%C3%A9%20&amp;%20100%25.png\">", &assets).expect("ordinary characters in a file name");
    assert_eq!(sig.pictures()[0].name, "café & 100%.png");
    assert_eq!(sig.layout, Layout::Inline);
}

#[test]
fn a_picture_on_the_web_stays_as_it_is_and_adds_no_payload() {
    let html = "<div><img src=\"https://example.org/logo.png\" alt=\"Site logo\"></div>";
    let sig = import("Work", html, &[]).expect("a web picture is not a local one");
    assert_eq!(sig.html(), Some(html), "byte for byte, as the owner wrote it");
    assert!(sig.pictures().is_empty(), "nothing is fetched or added");
}

// ---- the model's quotas ----

#[test]
fn more_pictures_than_a_signature_holds_are_refused() {
    let assets: Vec<Asset> = (0..=MAX_PICTURES).map(|i| Asset { name: format!("p{i}.png"), data: ONE_PIXEL_PNG.to_vec() }).collect();
    let html: String = (0..=MAX_PICTURES).map(|i| format!("<img src=p{i}.png>")).collect();
    assert_eq!(import("Work", &html, &assets).unwrap_err(), ImportError::TooManyPictures);
}

#[test]
fn a_picture_exactly_at_its_byte_cap_is_accepted() {
    let data = sized_png(MAX_PICTURE_BYTES);
    let assets = vec![Asset { name: "logo.png".to_string(), data }];
    let sig = import("Work", "<img src=\"logo.png\">", &assets).expect("a picture at the cap is a picture");
    assert_eq!(sig.pictures()[0].data.len(), MAX_PICTURE_BYTES);
}

#[test]
fn a_picture_one_byte_past_its_cap_is_refused() {
    let mut assets = vec![Asset { name: "logo.png".to_string(), data: sized_png(MAX_PICTURE_BYTES + 1) }];
    let before = assets.clone();
    assert_eq!(import("Work", "<img src=\"logo.png\">", &assets).unwrap_err(), ImportError::PictureTooLarge);
    assert_eq!(assets, before, "refused before the bytes are taken");
    assets.clear();
}

#[test]
fn pictures_exactly_at_the_total_cap_are_accepted() {
    let assets = vec![
        Asset { name: "one.png".to_string(), data: sized_png(MAX_PICTURE_BYTES) },
        Asset { name: "two.png".to_string(), data: sized_png(MAX_PICTURE_BYTES) },
    ];
    let sig = import("Work", "<img src=\"one.png\"><img src=\"two.png\">", &assets).expect("pictures at the cap together");
    assert_eq!(sig.picture_bytes(), MAX_PICTURES_BYTES);
    assert_eq!(sig.pictures().len(), 2);
}

#[test]
fn pictures_past_the_total_cap_are_refused() {
    // Each file is within its own cap; together, three of them are not. The third is where the
    // total is seen, and the whole import is refused rather than kept in part.
    let assets = vec![
        Asset { name: "one.png".to_string(), data: sized_png(MAX_PICTURE_BYTES) },
        Asset { name: "two.png".to_string(), data: sized_png(MAX_PICTURE_BYTES) },
        Asset { name: "three.png".to_string(), data: sized_png(MAX_PICTURE_BYTES) },
    ];
    let html = "<img src=\"one.png\"><img src=\"two.png\"><img src=\"three.png\">";
    assert_eq!(import("Work", html, &assets).unwrap_err(), ImportError::PicturesTooLarge);
}

#[test]
fn a_file_that_is_not_a_picture_is_refused() {
    let assets = vec![Asset { name: "logo.png".to_string(), data: b"not a picture at all".to_vec() }];
    assert_eq!(import("Work", "<img src=\"logo.png\">", &assets).unwrap_err(), ImportError::UnsupportedPicture);
}

// ---- the document's own caps ----

#[test]
fn html_over_its_cap_is_refused_before_it_is_read() {
    let html = "a".repeat(MAX_HTML + 1);
    assert_eq!(import("Work", &html, &[]).unwrap_err(), ImportError::HtmlTooLarge);
}

#[test]
fn a_document_that_grows_past_its_cap_is_refused() {
    // Exactly at the cap as it is written; the internal address is longer than the file's name.
    let mut html = String::from("<img src=x.png>");
    html.push_str(&"a".repeat(MAX_HTML - html.len()));
    assert_eq!(html.len(), MAX_HTML);
    let assets = vec![Asset { name: "x.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    assert_eq!(import("Work", &html, &assets).unwrap_err(), ImportError::RewrittenHtmlTooLarge);
}

#[test]
fn a_stand_in_over_the_text_cap_is_refused() {
    let html = format!("<p>{}<img src=x.png></p>", "a".repeat(12_000));
    let assets = vec![Asset { name: "x.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    assert_eq!(import("Work", &html, &assets).unwrap_err(), ImportError::TextTooLarge);
}

// ---- a picture layout the stand-in would not keep ----

#[test]
fn a_picture_hidden_by_a_style_is_refused() {
    let assets = vec![Asset { name: "logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let html = "<div style=\"display:none\"><img src=\"logo.png\"></div>";
    assert_eq!(import("Work", html, &assets).unwrap_err(), ImportError::UnsupportedRepresentation);
}

#[test]
fn a_picture_hidden_by_its_attribute_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\" hidden>", "logo.png").unwrap_err(), ImportError::UnsupportedRepresentation);
}

#[test]
fn a_picture_too_small_to_show_is_refused() {
    assert_eq!(one_asset("<img src=\"logo.png\" width=\"1\" height=\"1\">", "logo.png").unwrap_err(), ImportError::UnsupportedRepresentation);
}

// ---- errors say nothing of what was read ----

#[test]
fn a_hostile_reference_is_not_echoed_in_the_error() {
    let err = import("Work", "<img src=\"javascript:alert('secret-token')\">", &[]).unwrap_err();
    assert_eq!(err, ImportError::BadReference);
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains("secret") && !shown.contains("javascript"), "the error says what, not what was read: {shown}");
}

#[test]
fn a_hostile_asset_name_is_not_echoed_in_the_error() {
    let assets = vec![Asset { name: "../secret-token/logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let err = import("Work", "<p>text</p>", &assets).unwrap_err();
    assert_eq!(err, ImportError::BadAssetName);
    assert!(!format!("{err} {err:?}").contains("secret"), "the error says what, not what was read");
}

// ---- the picture reader's own refusals reach the caller typed ----

#[test]
fn a_base_element_is_refused_with_the_reader_s_reason() {
    let assets = vec![Asset { name: "logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    let html = "<base href=\"https://example.org/\"><img src=\"logo.png\">";
    assert_eq!(import("Work", html, &assets).unwrap_err(), ImportError::Source(ImageSourceError::UnsupportedBase));
}

#[test]
fn more_picture_elements_than_the_reader_reads_is_refused() {
    let html: String = (0..300).map(|_| "<img src=logo.png>").collect();
    let assets = vec![Asset { name: "logo.png".to_string(), data: ONE_PIXEL_PNG.to_vec() }];
    assert_eq!(import("Work", &html, &assets).unwrap_err(), ImportError::Source(ImageSourceError::TooManySources));
}

// ---- a web picture's address, however it is wrapped ----

#[test]
fn a_web_address_with_white_space_around_it_is_still_a_web_address_and_stays_as_it_is() {
    // The owner's document, with text, and one picture whose address is absolute and on the web
    // but written in upper case with the ASCII white space a copy out of a mail client leaves
    // around it: a tab before, a line break and a space after. It is the same web address, so it
    // stays exactly as written, no payload is added, and nothing is fetched.
    let html = "<p>Questions? Write to me.</p>\n<div><img src=\" \tHTTPS://example.org/mark.png\r\n \" alt=\"Site mark\"></div>\n";
    let sig = import("Work", html, &[]).expect("a web address is kept as it is, white space and all");

    // Every byte of the document, the white space around the address included: nothing rewritten.
    assert_eq!(sig.html(), Some(html), "the document as the owner wrote it, byte for byte");
    assert!(sig.pictures().is_empty(), "a picture on the web adds no payload");
    assert!(sig.body().pictures().is_empty(), "and no picture stands in the stand-in text");
}
