// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which `media` the tokenizer hands a `<style>` element's sheet over with, and whether that
//! `media` is known: of an attribute written twice HTML takes the first, whatever the case of its
//! name; a first media past the generic cap is still read, though strict export refuses it; and a value made
//! only of spaces that aren't CSS white space is kept, not trimmed to nothing: a CSS reader reads
//! it as a media type it doesn't know.
//!
//! The documents are invented; nothing here is fetched.

use super::Tokenizer;

/// Each sheet's `media` and whether it is unknown, as the tokenizer hands them over.
fn styles(html: &str) -> Vec<(String, bool)> {
    let mut tokens = Tokenizer::new(html);
    while tokens.next().is_some() {}
    tokens.styles.iter().map(|s| (s.media.clone(), s.overflowed)).collect()
}

/// More attributes than the tokenizer keeps.
fn many() -> String {
    (0..40).map(|i| format!(" data-a{i}=\"\"")).collect()
}

fn known(media: &str) -> Vec<(String, bool)> {
    vec![(media.to_string(), false)]
}

fn overflowed(media: &str) -> Vec<(String, bool)> {
    vec![(media.to_string(), true)]
}

#[test]
fn of_a_media_written_twice_the_first_counts_and_one_past_the_cap_is_a_second() {
    let many = many();
    let first_kept = format!("<style media=\"(min-width: 600px)\"{many} media=\"print\">p{{}}</style>");
    assert_eq!(styles(&first_kept), known("(min-width: 600px)"), "the second, past the cap, is dropped as HTML drops it");
    let both_past = format!("<style{many} media=\"(min-width: 600px)\" media=\"print\">p{{}}</style>");
    assert_eq!(styles(&both_past), overflowed("(min-width: 600px)"), "strict export refuses, but the reader keeps the first condition");
    assert_eq!(styles("<style media=\"screen\" media=\"print\">p{}</style>"), known("screen"), "both kept: the first");
}

#[test]
fn a_media_in_capitals_counts_as_one_in_small_letters() {
    let many = many();
    assert_eq!(
        styles(&format!("<style{many} MEDIA=\"print\">p{{}}</style>")),
        overflowed("print"),
        "past the generic cap, the reader still gets print"
    );
    assert_eq!(styles(&format!("<style MEDIA=\"print\"{many}>p{{}}</style>")), known("print"), "kept");
    assert_eq!(styles(&format!("<style MeDiA=\"print\"{many} media=\"screen\">p{{}}</style>")), known("print"), "kept, and first");
}

#[test]
fn a_kept_empty_media_is_known_though_attributes_were_dropped() {
    let many = many();
    assert_eq!(styles(&format!("<style media=\"\"{many} media=\"print\">p{{}}</style>")), known(""), "the empty one is the one that counts");
    assert_eq!(styles(&format!("<style media=\" &#12;&#9;\"{many}>p{{}}</style>")), known(""), "CSS white space alone is no condition");
    // Control: no `media` among those kept.
    assert_eq!(styles(&format!("<style{many}>p{{}}</style>")), overflowed(""));
}

#[test]
fn a_media_of_spaces_that_arent_css_white_space_alone_is_kept() {
    for (media, read) in [
        ("&nbsp;", "\u{a0}"),
        ("&#160;", "\u{a0}"),
        ("&#11;", "\u{b}"),
        ("&#x2003;", "\u{2003}"),
        ("\u{a0}", "\u{a0}"),
        ("\u{2003}&nbsp;", "\u{2003}\u{a0}"),
        (" &#9;&nbsp;&#10;&#12; ", "\u{a0}"),
    ] {
        assert_eq!(styles(&format!("<style media=\"{media}\">p{{}}</style>")), known(read), "{media:?}");
    }
    // Control: white space round a list that says something goes, as before, and CSS's own alone
    // is no condition.
    for (media, read) in [
        (" &#9;screen&#10;&#12; ", "screen"),
        ("&#13;\nall\t", "all"),
        ("&#x3000;print\u{a0}", "print"),
        (" &#9;\u{2003}screen\u{a0}&#10;&#12; ", "screen"),
        ("&#12;", ""),
    ] {
        assert_eq!(styles(&format!("<style media=\"{media}\">p{{}}</style>")), known(read), "{media:?}");
    }
}
