// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a document's pictures name, as the owner wrote it: [`mailcraft_html::image_sources`] gives
//! each `<img>`'s `src` value in source order, decoded, with the byte range of the value in the
//! document. It reads no address and writes nothing; a caller that rebinds pictures replaces
//! exactly those ranges and keeps every other byte. Markup that only looks like a picture (in a
//! comment, a script, a style sheet, raw text, text, or another tag's attribute) names nothing.
//!
//! The refusals are the contract's own bounds: the input read, the pictures named, the attributes
//! kept, a start tag that does not end, a source that is not where its element says, and a
//! `<base href>` the addresses would resolve against. Each has its own test.

use mailcraft_html::{ImageSourceError, MAX_IMAGE_SOURCES, MAX_INPUT_WITH_PICTURES, image_sources};

/// The document in pieces, cut where a picture's source value sits: the odd parts are values as
/// written, quotes and all kept outside them.
const PARTS: [&str; 5] = [
    "<!DOCTYPE html>\n<HTML lang=\"en\"><head><title><img src=\"decoy-title.png\"></title>\n<style>p::before { content: '<img src=\"decoy-style.png\">' }</style>\n<script>var s = '<img src=\"decoy-script.png\">';</script></head>\n<body class=\"café\">\n  <p data-note='<img src=\"decoy-attr.png\">'>before <img src='",
    "pictures/été&amp;1.png",
    "' alt=\"summer\"> and <img src=",
    "pics/local%20name.png",
    " width=4>\n  <!-- <img src=\"decoy-comment.png\"> -->\n  <span>text says &lt;img src=\"decoy-text.png\"&gt; here</span></p>\n</body></HTML>",
];

/// The whole document, with the two source values replaced by the two given.
fn spliced(values: [&str; 2]) -> String {
    let mut out = String::new();
    out.push_str(PARTS[0]);
    out.push_str(values[0]);
    out.push_str(PARTS[2]);
    out.push_str(values[1]);
    out.push_str(PARTS[4]);
    out
}

#[test]
fn two_pictures_give_their_value_spans_and_decoded_sources() {
    let html = spliced([PARTS[1], PARTS[3]]);
    let found = image_sources(&html).expect("the document is read");
    assert_eq!(found.len(), 2, "only the two real img nodes name a picture");

    // The byte ranges are absolute, in source order, and cover the values without their quotes.
    let start1 = PARTS[0].len();
    let start2 = start1 + PARTS[1].len() + PARTS[2].len();
    let ranges: Vec<std::ops::Range<usize>> = found.iter().map(|s| s.value_range.clone()).collect();
    assert_eq!(ranges, [start1..start1 + PARTS[1].len(), start2..start2 + PARTS[3].len()]);

    // What the ranges cover is the value as written.
    let raw: Vec<&str> = found.iter().map(|s| &html[s.value_range.clone()]).collect();
    assert_eq!(raw, [PARTS[1], PARTS[3]]);

    // The sources are decoded for character references and nothing else.
    assert_eq!(found[0].src, "pictures/été&1.png");
    assert_eq!(found[1].src, "pics/local%20name.png");
}

#[test]
fn replacing_the_value_ranges_changes_only_the_values() {
    let html = spliced([PARTS[1], PARTS[3]]);
    let found = image_sources(&html).expect("the document is read");
    let mut out = String::new();
    let mut at = 0;
    for (i, source) in found.iter().enumerate() {
        out.push_str(&html[at..source.value_range.start]);
        out.push_str(&format!("cid:image-{i}"));
        at = source.value_range.end;
    }
    out.push_str(&html[at..]);
    assert_eq!(out, spliced(["cid:image-0", "cid:image-1"]));
    // Nothing else moved: document case, owner attributes, white space and every decoy stay.
    assert!(out.contains("<!DOCTYPE html>") && out.contains("<HTML lang=\"en\">"));
    assert!(out.contains("data-note='<img src=\"decoy-attr.png\">'") && out.contains("width=4>"));
    for decoy in ["decoy-title.png", "decoy-style.png", "decoy-script.png", "decoy-comment.png", "&lt;img src=\"decoy-text.png\"&gt;"] {
        assert!(out.contains(decoy), "untouched: {decoy}");
    }
}

#[test]
fn a_source_written_as_a_character_reference_keeps_the_range_of_what_was_written() {
    // `&eacute;` is six bytes written and two bytes decoded: the range is the written value's.
    let html = "<p>x <img src=\"caf&eacute;.png\"> y</p>";
    let found = image_sources(html).expect("the document is read");
    assert_eq!(found.len(), 1);
    let source = &found[0];
    assert_eq!(source.src, "café.png");
    assert_eq!(html.get(source.value_range.clone()), Some("caf&eacute;.png"));
    assert_eq!(source.value_range.len(), "caf&eacute;.png".len());
    assert_ne!(source.value_range.len(), source.src.len(), "the written value is longer than its text");
    assert_eq!(&html[source.value_range.start..source.value_range.end], "caf&eacute;.png");
}

#[test]
fn the_first_of_two_src_attributes_wins() {
    let html = "<img src=\"first.png\" src=\"second.png\">";
    let found = image_sources(html).expect("the document is read");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].src, "first.png");
    assert_eq!(html.get(found[0].value_range.clone()), Some("first.png"));
}

#[test]
fn a_picture_with_no_source_or_an_empty_one_names_nothing() {
    let html = "<img alt=\"x\"><img src=\"\"><img src=''><img src><p>no picture</p>";
    let found = image_sources(html).expect("the document is read");
    assert!(found.is_empty(), "absent and empty sources name no picture");
    assert!(image_sources("").expect("empty input is read").is_empty());
    assert!(image_sources("<p>text only</p>").expect("text is read").is_empty());
}

#[test]
fn raw_text_names_no_picture() {
    let html = "<textarea><img src=\"in-textarea.png\"></textarea><xmp><img src=\"in-xmp.png\"></xmp>";
    let found = image_sources(html).expect("the document is read");
    assert!(found.is_empty(), "the content of textarea and xmp is text, not markup");
}

#[test]
fn a_document_at_the_input_limit_is_read() {
    // A valid picture first, then text up to exactly the limit the reader takes.
    let mut html = String::from("<img src=ok.png>");
    html.push_str(&"a".repeat(MAX_INPUT_WITH_PICTURES - html.len()));
    assert_eq!(html.len(), MAX_INPUT_WITH_PICTURES);
    let found = image_sources(&html).expect("a document at the limit is read");
    assert_eq!(found.len(), 1);
}

#[test]
fn a_document_past_the_input_limit_is_refused() {
    // The valid prefix is real, but the tail is past the bound: refused, not cut.
    let mut html = String::from("<img src=ok.png>");
    html.push_str(&"a".repeat(MAX_INPUT_WITH_PICTURES));
    assert!(html.len() > MAX_INPUT_WITH_PICTURES);
    assert_eq!(image_sources(&html), Err(ImageSourceError::OverInput));
}

#[test]
fn the_most_pictures_the_limit_allows_are_read() {
    let mut html = String::new();
    for i in 0..MAX_IMAGE_SOURCES {
        html.push_str(&format!("<img src=p{i}.png>"));
    }
    let found = image_sources(&html).expect("the limit itself is read");
    assert_eq!(found.len(), MAX_IMAGE_SOURCES);
    assert_eq!(found[0].src, "p0.png");
    assert_eq!(found[MAX_IMAGE_SOURCES - 1].src, format!("p{}.png", MAX_IMAGE_SOURCES - 1));
}

#[test]
fn one_picture_past_the_limit_is_refused() {
    let mut html = String::new();
    for i in 0..=MAX_IMAGE_SOURCES {
        html.push_str(&format!("<img src=p{i}.png>"));
    }
    assert_eq!(image_sources(&html), Err(ImageSourceError::TooManySources));
}

#[test]
fn an_img_with_every_attribute_kept_is_read() {
    // `src` and thirty-one more: the tokenizer keeps thirty-two attributes, so nothing is dropped.
    let mut html = String::from("<img src=ok.png");
    for i in 0..31 {
        html.push_str(&format!(" a{i}=1"));
    }
    html.push('>');
    let found = image_sources(&html).expect("thirty-two attributes are all kept");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].src, "ok.png");
}

#[test]
fn an_img_with_more_attributes_than_are_kept_is_refused() {
    let mut html = String::from("<img src=ok.png");
    for i in 0..32 {
        html.push_str(&format!(" a{i}=1"));
    }
    html.push('>');
    assert_eq!(image_sources(&html), Err(ImageSourceError::AttributeOverflow));
}

#[test]
fn an_unclosed_quoted_src_ending_in_a_bracket_is_still_an_unfinished_tag() {
    // The last byte is `>`, but it is inside the unclosed quote: the tag never ended.
    assert_eq!(image_sources("<img src=\"asset.png>"), Err(ImageSourceError::UnclosedTag));
}

#[test]
fn an_unclosed_unquoted_img_tag_is_refused() {
    assert_eq!(image_sources("<p>x</p><img src=asset.png"), Err(ImageSourceError::UnclosedTag));
}

#[test]
fn a_base_element_with_an_address_is_refused() {
    let html = "<base href=\"https://example.org/inbox/\"><img src=\"asset.png\">";
    assert_eq!(image_sources(html), Err(ImageSourceError::UnsupportedBase));
}

#[test]
fn a_base_element_with_no_address_is_not_a_refusal() {
    for html in ["<base target=\"_blank\"><img src=asset.png>", "<base href=\"\"><img src=asset.png>", "<base href=\"   \"><img src=asset.png>"] {
        let found = image_sources(html).expect("a base with no address resolves nothing new");
        assert_eq!(found.len(), 1, "the picture is still read: {html}");
        assert_eq!(found[0].src, "asset.png");
    }
}

#[test]
fn a_base_href_past_the_attribute_limit_is_refused_as_overflow() {
    // A real picture comes first: a partial list would name it, so the refusal must be all or
    // nothing. The base's address is the thirty-third attribute, so the tokenizer drops it.
    let mut html = String::from("<img src=kept.png><base");
    for i in 0..32 {
        html.push_str(&format!(" a{i}=1"));
    }
    html.push_str(" href=\"https://example.org/\">");
    assert_eq!(image_sources(&html), Err(ImageSourceError::AttributeOverflow));
}

#[test]
fn a_base_href_at_the_attribute_limit_is_refused_as_a_base() {
    // Thirty-one attributes, then the address as the last one kept: the address itself is read.
    let mut html = String::from("<img src=kept.png><base");
    for i in 0..31 {
        html.push_str(&format!(" a{i}=1"));
    }
    html.push_str(" href=\"https://example.org/\">");
    assert_eq!(image_sources(&html), Err(ImageSourceError::UnsupportedBase));
}

#[test]
fn a_refusal_says_nothing_of_the_document() {
    let html = "<base href=\"https://secret.example/track?id=QUERYMARK\"><img src=\"PRIVATEPATH.png\">";
    let err = image_sources(html).expect_err("a base address is refused");
    let shown = format!("{err} {err:?}");
    for secret in ["secret.example", "QUERYMARK", "PRIVATEPATH", "http"] {
        assert!(!shown.contains(secret), "the reason says nothing of the document: {shown}");
    }
}
