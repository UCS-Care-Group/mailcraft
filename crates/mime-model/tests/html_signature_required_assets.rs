// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The names a signature's HTML asks its owner for, read with no file behind them: the slots a
//! picker offers one at a time, and the constructor that must take exactly those names.
//!
//! The first test pins what a picker relies on: ordered, distinct, normalized resource names. The
//! refusals are the importer's own, reached through the same classifier it uses. The last test is
//! the producer-consumer trace — the planner's names become the assets, and `import` retains every
//! payload it was asked for.

use mailcraft_html::ImageSourceError;
use mailcraft_mime_model::signature_import::{Asset, ImportError, MAX_ASSET_NAME, import, required_assets};
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURES};

/// A one-pixel PNG, whole and valid (67 bytes). Only its kind matters here: the model reads a
/// picture's type from its own first bytes, and this fixture says nothing about pixels.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The names `src` asks for, as one one-picture document.
fn named(src: &str) -> Result<Vec<String>, ImportError> {
    required_assets(&format!("<img src=\"{src}\">"))
}

/// A whole table of results, so one wrong answer shows every answer rather than hiding the rest.
fn named_all(sources: &[&str]) -> Vec<Result<Vec<String>, ImportError>> {
    sources.iter().map(|src| named(src)).collect()
}

// ---- the contract a picker binds to ----

#[test]
fn the_named_slots_are_ordered_distinct_normalized_names() {
    // Two spellings of one picture (a character reference, an entity and a `./` alias), a second
    // picture under a folder, and a name in another script: each slot is a normalized resource
    // name, once, in the order the document first names it.
    let html = concat!(
        "<p>Priya Raman</p>\n",
        "<div><img src=\"pictures/caf&eacute;%20line.png\" width=\"48\" alt=\"first\"></div>\n",
        "<div><img src=\"./pictures/caf%C3%A9%20line.png\" width=\"48\"></div>\n",
        "<div><img src=\"brand/mark.png\" width=\"32\"></div>\n",
        "<div><img src=\"./brand/mark.png\" width=\"32\"></div>\n",
        "<div><img src=\"логотип.png\" width=\"24\"></div>\n",
    );
    assert_eq!(
        required_assets(html).expect("five references, three slots"),
        vec!["pictures/café line.png".to_string(), "brand/mark.png".to_string(), "логотип.png".to_string()],
    );
}

#[test]
fn a_document_with_no_local_picture_needs_nothing() {
    assert_eq!(required_assets("<p>Questions? Write to me.</p>").expect("no pictures"), Vec::<String>::new());

    // Web addresses, however their letter case and surrounding white space are written, are kept
    // as they are and add no slot: nothing would be fetched or read.
    let web_only = "<p>Hi</p>\n<img src=\" \tHTTPS://example.org/mark.png\r\n \" alt=\"mark\">\n<img src=\"http://Example.org/other.png\">\n";
    assert_eq!(required_assets(web_only).expect("web pictures ask for nothing"), Vec::<String>::new());

    // A local picture among them is still the one slot, in its own place.
    let mixed = "<img src=\"https://example.org/a.png\"><img src=\"local.png\"><img src=\"HTTPS://EXAMPLE.ORG/b.png\">";
    assert_eq!(required_assets(mixed).expect("one local picture"), vec!["local.png".to_string()]);
}

#[test]
fn a_reference_is_percent_decoded_exactly_once() {
    // `%25` is a literal percent, so the slot's name holds `%2e%2e` — text, not a component that
    // climbs. Decoding the name again, as a picker must not, would turn it into `..`.
    assert_eq!(named("pictures/%252e%252e/logo%20one.png").expect("one literal escape"), vec!["pictures/%2e%2e/logo one.png".to_string()]);
}

// ---- the importer's refusals, reached through its own classifier ----

#[test]
fn a_malformed_or_encoded_separator_escape_is_refused() {
    assert_eq!(
        named_all(&["logo%zz.png", "logo%2.png", "logo%2f.png", "pictures%5clogo.png", "logo%ff.png"]),
        vec![Err(ImportError::BadReference); 5],
        "a bad escape, a short escape, an encoded separator and an escape that is not UTF-8",
    );
}

#[test]
fn a_traversing_or_absolute_path_is_refused() {
    assert_eq!(
        named_all(&["../logo.png", "pictures/../../logo.png", "pictures/%2e%2e/logo.png", "/pics/logo.png"]),
        vec![Err(ImportError::BadReference); 4],
        "a component that climbs, in any spelling, and a name that starts at the root",
    );
}

#[test]
fn an_address_that_is_not_a_local_resource_name_is_refused() {
    assert_eq!(
        named_all(&[
            "file:///home/someone/logo.png",
            "cid:logo@example.invalid",
            "data:image/png;base64,iVBORw0KGgo=",
            "//example.org/logo.png",
            "http://"
        ]),
        vec![Err(ImportError::BadReference); 5],
        "a file address, a cid, a data picture, a schemeless host and an unsafe web address",
    );
}

// ---- what a signature holds, refused before a file is asked for ----

#[test]
fn ten_distinct_slots_are_accepted_and_an_eleventh_is_refused() {
    let ten: String = (0..MAX_PICTURES).map(|i| format!("<img src=p{i}.png>")).collect();
    assert_eq!(required_assets(&ten).expect("a signature holds ten"), (0..MAX_PICTURES).map(|i| format!("p{i}.png")).collect::<Vec<_>>(),);

    let eleven: String = (0..=MAX_PICTURES).map(|i| format!("<img src=p{i}.png>")).collect();
    assert_eq!(required_assets(&eleven), Err(ImportError::TooManyPictures), "one slot past the cap, before any file");

    // One name in eleven places is one slot: the cap counts what an owner is asked for.
    let repeats: String = (0..=MAX_PICTURES).map(|_| "<img src=one.png>").collect();
    assert_eq!(required_assets(&repeats).expect("one name, many places"), vec!["one.png".to_string()]);
}

#[test]
fn a_normalized_name_over_the_asset_name_cap_is_refused() {
    let at_cap = "a".repeat(MAX_ASSET_NAME);
    assert_eq!(named(&at_cap).expect("a name at the cap"), vec![at_cap]);

    let over_cap = "a".repeat(MAX_ASSET_NAME + 1);
    assert_eq!(named(&over_cap), Err(ImportError::BadReference), "refused before its file is asked for");
}

// ---- the document's own caps, with the reader's typed reasons ----

#[test]
fn the_document_caps_are_refused_with_their_typed_errors() {
    let over = "a".repeat(MAX_HTML + 1);
    assert_eq!(required_assets(&over), Err(ImportError::HtmlTooLarge), "the HTML cap, before a name is read");

    let base = "<base href=\"https://example.org/\"><img src=\"logo.png\">";
    assert_eq!(required_assets(base), Err(ImportError::Source(ImageSourceError::UnsupportedBase)));
}

// ---- producer to consumer: the names a picker asks for are the ones the constructor takes ----

#[test]
fn the_constructor_retains_every_payload_the_planner_named() {
    // The owner's document: one picture named twice (once through a `./` alias) and one more in a
    // folder. No file exists for any of it here.
    let html = concat!(
        "<p>Priya Raman</p>\n",
        "<div><img src=\"pictures/mark%20one.png\" width=\"48\" alt=\"Mark\"></div>\n",
        "<div><img src=\"./pictures/mark%20one.png\" width=\"48\"></div>\n",
        "<div><img src=\"brand/logo.png\" width=\"32\" alt=\"Logo\"></div>\n",
    );
    let needed = required_assets(html).expect("two slots, one named twice");
    assert_eq!(needed, vec!["pictures/mark one.png".to_string(), "brand/logo.png".to_string()]);

    // The picker asks for exactly one file per name and nothing else; each is named by its slot.
    let assets: Vec<Asset> = needed.iter().map(|name| Asset { name: name.clone(), data: ONE_PIXEL_PNG.to_vec() }).collect();

    // The constructor takes the planner's own names, and keeps every payload it was given.
    let sig = import("Work", html, &assets).expect("the planner named exactly what the constructor takes");
    assert_eq!(sig.pictures().len(), needed.len(), "one payload per named slot, the alias not a second");
    assert!(sig.pictures().iter().all(|picture| picture.data.as_slice() == ONE_PIXEL_PNG), "each payload as supplied");
    let names: Vec<&str> = sig.pictures().iter().map(|picture| picture.name.as_str()).collect();
    assert_eq!(names, vec!["mark one.png", "logo.png"], "each picture by the last component of its slot");

    // Both places the aliased picture stands are that one payload.
    let mark = sig.pictures().iter().find(|picture| picture.name == "mark one.png").expect("the mark").id;
    let logo = sig.pictures().iter().find(|picture| picture.name == "logo.png").expect("the logo").id;
    assert_eq!(sig.body().pictures(), vec![mark, mark, logo]);

    // The document is kept, its local addresses replaced by the signature's own.
    let own = sig.html().expect("the HTML is kept");
    assert!(!own.contains("brand/logo.png") && !own.contains("pictures/mark%20one.png"), "no local address survives");
    assert!(own.contains("cid:"), "the pictures stand under the signature's own addresses");
}
