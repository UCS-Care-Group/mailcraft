// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility checker for signature HTML, slice 1: three picture rules found by
//! [`mailcraft_html::compat::check`] with exact byte ranges into the checked text, a severity, and
//! at most one exact edit as a fix. Markup that only looks like a picture (in a comment or a style
//! sheet) is not checked; a document that is cut off or larger than is read is refused, so the
//! caller can treat a refusal as blocking.

use mailcraft_html::compat::{CheckError, Finding, Fix, Rule, Severity, check};
use mailcraft_html::{MAX_INPUT, MAX_INPUT_WITH_PICTURES};

fn finding(rule: Rule, range: std::ops::Range<usize>, fix: Option<Fix>) -> Finding {
    Finding { rule, severity: Severity::Error, range, fix }
}

fn insert(at: usize, text: &str) -> Option<Fix> {
    Some(Fix { range: at..at, replacement: text.to_string() })
}

fn apply(html: &str, fix: &Fix) -> String {
    let mut out = String::new();
    out.push_str(&html[..fix.range.start]);
    out.push_str(&fix.replacement);
    out.push_str(&html[fix.range.end..]);
    out
}

#[test]
fn rules_have_stable_ids_and_are_errors() {
    assert_eq!(Rule::SvgImage.id(), "OL-IMG-001");
    assert_eq!(Rule::UnsupportedImageFormat.id(), "OL-IMG-003");
    assert_eq!(Rule::CssOnlyImageSize.id(), "OL-IMG-005");
    for rule in [Rule::SvgImage, Rule::UnsupportedImageFormat, Rule::CssOnlyImageSize] {
        assert_eq!(rule.severity(), Severity::Error, "{}", rule.id());
    }
}

#[test]
fn css_size_without_attributes_gets_an_exact_insert_fix() {
    let html = r#"<img src="logo.png" style="width:120px;height:40px">"#;
    let found = check(html).expect("checked");
    assert_eq!(found, [finding(Rule::CssOnlyImageSize, 27..50, insert(4, r#" width="120" height="40""#))]);
    assert_eq!(&html[27..50], "width:120px;height:40px");
}

#[test]
fn applying_the_size_fix_leaves_nothing_to_find_and_keeps_every_other_byte() {
    let html = r#"<img src="logo.png" style="width:120px;height:40px">"#;
    let found = check(html).expect("checked");
    let fix = found[0].fix.clone().expect("a fix");
    let fixed = apply(html, &fix);
    assert_eq!(fixed, r#"<img width="120" height="40" src="logo.png" style="width:120px;height:40px">"#);
    assert_eq!(&fixed[..fix.range.start], &html[..fix.range.start]);
    assert_eq!(&fixed[fix.range.start + fix.replacement.len()..], &html[fix.range.end..]);
    assert_eq!(check(&fixed).expect("checked"), []);
}

#[test]
fn upper_case_names_and_one_missing_attribute() {
    let html = r#"<IMG SRC='Logo.PNG' Height=40 STYLE="WIDTH: 120PX; HEIGHT: 40px">"#;
    let found = check(html).expect("checked");
    assert_eq!(found, [finding(Rule::CssOnlyImageSize, 37..49, insert(4, r#" width="120""#))]);
    assert_eq!(&html[37..49], "WIDTH: 120PX");
    let fixed = apply(html, found[0].fix.as_ref().expect("a fix"));
    assert_eq!(check(&fixed).expect("checked"), []);
}

#[test]
fn a_size_that_is_not_plain_pixels_is_an_error_without_fix() {
    let html = r#"<img src="a.png" style="width:50%">"#;
    assert_eq!(check(html).expect("checked"), [finding(Rule::CssOnlyImageSize, 24..33, None)]);
}

#[test]
fn svg_sources_by_path_or_data_type() {
    let html = r#"<p>x</p><img src="https://example.org/brand/logo.SVG?v=2#top" width="10">"#;
    assert_eq!(check(html).expect("checked"), [finding(Rule::SvgImage, 18..60, None)]);
    // Written into the HTML too (OL-IMG-004), on the same source.
    let data = r#"<img src="DATA:image/SVG+xml;base64,PHN2Zy8+">"#;
    assert_eq!(check(data).expect("checked"), [finding(Rule::SvgImage, 10..44, None), finding(Rule::DataUriImage, 10..44, None)]);
}

#[test]
fn entity_encoded_and_unquoted_sources_are_read_as_written() {
    let encoded = "<img src='logo&#46;svg'>";
    assert_eq!(check(encoded).expect("checked"), [finding(Rule::SvgImage, 10..22, None)]);
    let unquoted = "<img src=a.svg>";
    assert_eq!(check(unquoted).expect("checked"), [finding(Rule::SvgImage, 9..14, None)]);
}

#[test]
fn modern_formats_by_extension_or_data_type() {
    for src in ["photo.webp", "https://example.org/p.AVIF#x", "img/pic.heic?s=1", "data:image/webp;base64,AAAA", "Data:Image/AVIF,AAAA"] {
        let html = format!(r#"<img src="{src}">"#);
        let mut expected = vec![finding(Rule::UnsupportedImageFormat, 10..10 + src.len(), None)];
        // A source written into the HTML is that error too (OL-IMG-004), on the same range.
        if src.to_ascii_lowercase().starts_with("data:") {
            expected.push(finding(Rule::DataUriImage, 10..10 + src.len(), None));
        }
        assert_eq!(check(&html).expect("checked"), expected, "{src}");
    }
}

#[test]
fn ordinary_and_attached_sources_are_not_flagged() {
    for src in ["logo.png", "a.svg.png", "https://example.org/svg/x.jpg", "https://example.org/a.png?f=.svg", "cid:logo.svg", "CID:pic.webp"] {
        let html = format!(r#"<img src="{src}" width="1" height="1">"#);
        assert_eq!(check(&html).expect("checked"), [], "{src}");
    }
}

#[test]
fn pictures_in_comments_and_style_sheets_are_not_checked() {
    let html = r#"<!-- <img src="a.svg" style="width:1px"> --><style>/* <img src="b.webp" style="width:2px"> */ p { color: red }</style><p>ok</p>"#;
    assert_eq!(check(html).expect("checked"), []);
}

#[test]
fn a_clean_table_signature_has_no_findings() {
    let html = concat!(
        r#"<table cellpadding="0" cellspacing="0" border="0" style="border-collapse:collapse"><tr>"#,
        r#"<td style="padding:0 12px 0 0"><img src="cid:logo@example.org" width="120" height="40" alt="Example" style="display:block;width:120px;height:40px"></td>"#,
        r#"<td style="font-family:Arial,sans-serif;font-size:13px">Ada Example<br><a href="https://example.org">example.org</a></td>"#,
        "</tr></table>"
    );
    assert_eq!(check(html).expect("checked"), []);
}

#[test]
fn findings_are_sorted_by_start_and_ranges_fall_on_char_boundaries() {
    let html = r#"<img style="width:12px" src="a.svg"><img src="b.webp" style="height:3px">"#;
    assert_eq!(
        check(html).expect("checked"),
        [
            finding(Rule::CssOnlyImageSize, 12..22, insert(4, r#" width="12""#)),
            finding(Rule::SvgImage, 29..34, None),
            finding(Rule::UnsupportedImageFormat, 46..52, None),
            finding(Rule::CssOnlyImageSize, 61..71, insert(40, r#" height="3""#)),
        ]
    );
    let accented = r#"<p>é</p><img alt="ü" src="ñ.svg">"#;
    let found = check(accented).expect("checked");
    assert_eq!(found, [finding(Rule::SvgImage, 28..34, None)]);
    assert!(accented.is_char_boundary(28) && accented.is_char_boundary(34));
}

#[test]
fn a_cut_off_document_is_refused() {
    let html = r#"<table><tr><td><img src="logo.png" style="width:120px"#;
    assert_eq!(check(html), Err(CheckError::UnclosedTag));
}

#[test]
fn a_picture_with_more_attributes_than_are_read_is_refused() {
    let attrs: Vec<String> = (0..40).map(|i| format!("data-a{i}=\"1\"")).collect();
    let html = format!(r#"<img {} src="a.svg">"#, attrs.join(" "));
    assert_eq!(check(&html), Err(CheckError::AttributeOverflow));
}

#[test]
fn a_document_over_the_input_cap_is_refused() {
    let html = "x".repeat(MAX_INPUT_WITH_PICTURES + 1);
    assert_eq!(check(&html), Err(CheckError::OverInput));
    assert_eq!(check(&html[..MAX_INPUT_WITH_PICTURES]), Ok(Vec::new()));
}

/// The bytes of the whole `n`th (from 0) `<style>` element of `html`, start tag to end tag, in
/// any letter case.
fn style_element(html: &str, n: usize) -> std::ops::Range<usize> {
    let lower = html.to_ascii_lowercase();
    let start = lower.match_indices("<style").nth(n).expect("the fixture's style element").0;
    let end = start + lower[start..].find("</style>").expect("its end tag") + "</style>".len();
    start..end
}

fn not_sendable(range: std::ops::Range<usize>) -> Finding {
    Finding { rule: Rule::StylesheetNotSendable, severity: Severity::Error, range, fix: None }
}

#[test]
fn the_style_sheet_rule_has_a_stable_id_and_is_an_error() {
    assert_eq!(Rule::StylesheetNotSendable.id(), "MC-CSS-001");
    assert_eq!(Rule::StylesheetNotSendable.severity(), Severity::Error);
    assert!(Rule::ALL.contains(&Rule::StylesheetNotSendable));
}

#[test]
fn an_import_in_a_style_sheet_is_found_on_its_style_element() {
    let html = r#"<p>Ada</p><style>@import "https://example.org/a.css"; p { color: #333 }</style><p>Example</p>"#;
    let range = style_element(html, 0);
    assert_eq!(&html[range.clone()], r#"<style>@import "https://example.org/a.css"; p { color: #333 }</style>"#);
    assert_eq!(check(html).expect("checked"), [not_sendable(range)]);
}

#[test]
fn a_web_address_in_a_declaration_is_found_on_the_style_element_that_has_it() {
    let html = concat!(
        "<style>p { color: #333 }</style><p>Ada</p>",
        r#"<style media="screen">td { background: #fff url(https://example.org/b.png) }</style><p>Example</p>"#
    );
    let range = style_element(html, 1);
    assert!(html[range.clone()].starts_with(r#"<style media="screen">td"#), "{:?}", &html[range.clone()]);
    // The background picture is also a warning (OL-BG-001), on its declaration.
    let declaration = "background: #fff url(https://example.org/b.png)";
    let at = html.find(declaration).expect("the fixture's declaration");
    let picture = Finding { rule: Rule::BackgroundImage, severity: Severity::Warning, range: at..at + declaration.len(), fix: None };
    assert_eq!(check(html).expect("checked"), [not_sendable(range), picture]);
}

#[test]
fn an_is_selector_is_found_on_its_style_element() {
    let html = r#"<p>Ada</p><STYLE type="text/css">:is(p, td) { font-weight: bold }</STYLE>"#;
    let range = style_element(html, 0);
    assert_eq!(range, 10..html.len());
    assert_eq!(check(html).expect("checked"), [not_sendable(range)]);
}

#[test]
fn a_style_sheet_that_can_be_scoped_and_sent_has_no_findings() {
    let html = concat!(
        "<style>body { margin: 0 } p.name { color: #333; font-weight: bold !important } td + td { padding-left: 8px }",
        " @media (max-width: 600px) { td { display: block } }</style>",
        r#"<table><tr><td><p class="name">Ada Example</p></td><td><a href="https://example.org">example.org</a></td></tr></table>"#
    );
    assert_eq!(check(html).expect("checked"), []);
}

#[test]
fn a_refusal_no_style_element_explains_is_given_at_the_first_style_start_tag() {
    // The sheet itself can be sent; the document is longer than the writer reads.
    let html = format!("<style>p {{ color: #333 }}</style><p>{}</p>", "x".repeat(MAX_INPUT));
    assert!(html.len() <= MAX_INPUT_WITH_PICTURES);
    assert_eq!(check(&html).expect("checked"), [not_sendable(0..7)]);
}

#[test]
fn without_a_style_sheet_nothing_is_said_of_style_sheets() {
    // Longer than the writer reads, but no sheet in it.
    assert_eq!(check(&"x".repeat(MAX_INPUT + 1)).expect("checked"), []);
    // Not a style sheet, or one no reader takes: in a template.
    assert_eq!(check(r#"<style type="text/plain">@import "a.css";</style><p>x</p>"#).expect("checked"), []);
    assert_eq!(check(r#"<template><style>@import "a.css";</style></template><p>x</p>"#).expect("checked"), []);
}

/// Review-02: style sheets whose selectors hide brackets in comments, or whose declaration
/// blocks leave a `(` or `[` open, would put a rule outside the signature's scope in a CSS
/// reader: each is MC-CSS-001 on its style element.
#[test]
fn a_scope_escape_through_comments_and_brackets_is_found_on_its_style_element() {
    for html in [
        "<style>p { x: ( }\nq:hover(/*(((*/)) } html:not(.zz /*))*/) { color: red }</style><p>Hi</p>",
        "<style>p { x: [ }\nq::before(/*[[[*/]] } body:not(.zz /*]]*/) { color: red }</style><p>Hi</p>",
        "<style>@media screen { p { x: ( } }\nq:hover(/*(((*/)) } html:not(.zz /*))*/) { color: red }</style><p>Hi</p>",
        "<style>p { x: ( }\nq:hover(/*(((*/)) } blockquote:not(.zz /*))*/) { display: none }</style><p>Hi</p>",
        "<style>.a { x: ( }\n.b:focus(/*(((*/)) } *:not(.zz /*))*/) { visibility: hidden }</style><p class=\"a b\">Hi</p>",
        "<style>p { x: ( }\nq { color: blue }</style><p>Hi</p>",
        "<style>q:hover(x) { color: blue }</style><p>Hi</p>",
    ] {
        assert_eq!(check(html).expect("checked"), [not_sendable(style_element(html, 0))], "{html:?}");
    }
}

/// Control: comments between rules and inside declaration blocks, `:not()`, `:nth-child()` and
/// brackets inside a quoted attribute value can still be sent.
#[test]
fn comments_between_rules_and_bracketed_selectors_can_still_be_sent() {
    let html = concat!(
        "<style>/* card */ p:not(.muted) { color: #333; /* grey */ } td:nth-child(2n+1) { padding: 4px }",
        " a[title=\"Call (24h)\"] { color: #333 } @media (max-width: 600px) { td { display: block } }</style>",
        "<table><tr><td><p>Ada Example</p></td></tr></table>"
    );
    assert_eq!(check(html).expect("checked"), []);
}
