// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The checker says what the writer of a signature would lose, and nothing that it keeps: a
//! `style` attribute it leaves out whole (`MC-CSS-002`), a document cut off inside any start tag,
//! a picture's size as CSS works it out (priority, character references), and declarations and
//! calls read past CSS comments, as a CSS reader reads them. A `var()` whose variable is a
//! CSS-wide keyword gets no fix, since the keyword is no value to write in its place.
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::compat::{CheckError, Finding, Severity, check};
use mailcraft_html::{AuthoredScope, authored_fragment_with_rebound_cids};

fn found(html: &str) -> Vec<Finding> {
    check(html).unwrap_or_else(|e| panic!("{html:?} was not checked: {e}"))
}

fn ids(html: &str) -> Vec<&'static str> {
    found(html).iter().map(|f| f.rule.id()).collect()
}

/// What the writer of an authored signature puts in a message's body for `html`.
fn written(html: &str) -> String {
    let scope = AuthoredScope::new("mc-test").expect("a scope class");
    authored_fragment_with_rebound_cids(html, &scope, &mut |cid: &str| Some(cid.to_string())).expect("the writer takes the document").body
}

/// Each document holds one `style` attribute that the writer leaves out whole: the checker
/// finds it, as an error with no fix, on the attribute's value.
const DROPPED: [(&str, &str); 5] = [
    (r#"<p style="color:red;background-image:url(logo.png)">Priya Raman</p>"#, "color:red;background-image:url(logo.png)"),
    (
        r#"<ul><li style="color:red;list-style-image:url(https://pictures.example.net/dot.png)">Studio</li></ul>"#,
        "color:red;list-style-image:url(https://pictures.example.net/dot.png)",
    ),
    (r#"<p style="color:red;behavior:url(card.htc)">Priya Raman</p>"#, "color:red;behavior:url(card.htc)"),
    (r#"<p style="color:red;background:\75 rl(logo.png)">Priya Raman</p>"#, r"color:red;background:\75 rl(logo.png)"),
    (r#"<p style="color:red;background-image:url&#40;logo.png)">Priya Raman</p>"#, "color:red;background-image:url&#40;logo.png)"),
];

#[test]
fn a_style_the_writer_leaves_out_whole_is_an_error_on_the_attribute() {
    for (html, style) in DROPPED {
        assert!(!written(html).contains("color:red"), "fixture guard: the writer keeps the style of {html:?}");
        let all = found(html);
        let f = all.iter().find(|f| f.rule.id() == "MC-CSS-002").unwrap_or_else(|| panic!("no MC-CSS-002 for {html:?}: {all:?}"));
        assert_eq!(f.severity, Severity::Error, "{html:?}: what would be lost is an error");
        assert_eq!(html.get(f.range.clone()), Some(style), "{html:?}: the finding is the attribute's value");
        assert_eq!(f.fix, None, "{html:?}: no one edit puts it right");
    }
}

#[test]
fn a_style_the_writer_keeps_is_not_that_error() {
    for html in [
        r#"<p style="color:red;background-color:#fdf6e3">Priya Raman</p>"#,
        r#"<p style="color:red;font-family:'Url Sans', sans-serif">Priya Raman</p>"#,
        r#"<p style="color:red /* url(logo.png) */">Priya Raman</p>"#,
    ] {
        assert!(written(html).contains("color:red"), "fixture guard: the writer leaves out the style of {html:?}");
        assert!(!ids(html).contains(&"MC-CSS-002"), "{html:?}: a style the writer keeps is found as one it drops");
    }
}

#[test]
fn a_document_cut_off_inside_any_start_tag_is_refused() {
    for html in ["<p>Priya Raman</p><script", "<p>Priya Raman</p><title lang=\"en\"", "<p>Priya Raman</p><noframes", "<p>Priya Raman</p><style"] {
        assert_eq!(check(html), Err(CheckError::UnclosedTag), "{html:?}");
    }
    // Control: the same elements whole are read.
    assert_eq!(ids("<p>Priya Raman</p><script></script><title>Card</title>"), Vec::<&str>::new());
}

const IMG: &str = r#"<img src="https://pictures.example.net/mark.png" style=""#;

#[test]
fn an_important_size_outranks_a_later_one_of_the_same_dimension() {
    let html = format!(r#"{IMG}width:120px!important;width:auto">"#);
    let all = found(&html);
    assert_eq!(ids(&html), ["OL-IMG-005"], "the picture is 120 pixels wide by its style alone: {all:?}");
    let fix = all.first().and_then(|f| f.fix.clone()).expect("a fix");
    assert_eq!(fix.replacement, r#" width="120""#);
    // A later `!important` wins over an earlier plain size: `auto`, no size.
    assert_eq!(ids(&format!(r#"{IMG}width:120px;width:auto !important">"#)), Vec::<&str>::new());
    // Equal priority: the last wins, as before.
    assert_eq!(ids(&format!(r#"{IMG}width:auto;width:120px">"#)), ["OL-IMG-005"]);
    assert_eq!(ids(&format!(r#"{IMG}width:120px;width:auto">"#)), Vec::<&str>::new());
}

#[test]
fn a_size_written_with_a_character_reference_is_found_on_the_attribute() {
    let html = format!(r#"{IMG}width&#58;120px">"#);
    let all = found(&html);
    assert_eq!(ids(&html), ["OL-IMG-005"], "{all:?}");
    let f = all.first().expect("the finding");
    assert_eq!(html.get(f.range.clone()), Some("width&#58;120px"), "the finding is the whole value");
    let fix = f.fix.clone().expect("a fix");
    assert_eq!((fix.range, fix.replacement.as_str()), (4..4, r#" width="120""#));
}

#[test]
fn a_size_is_decoded_once_as_the_writer_decodes_it() {
    // `&amp;#49;20px` is `&#49;20px` once decoded, which CSS reads as no size at all.
    let twice = format!(r#"{IMG}width:&amp;#49;20px">"#);
    assert!(!ids(&twice).contains(&"OL-IMG-005"), "a size decoded twice: {:?}", found(&twice));
    // Control: `&#49;20px` is `120px`.
    assert_eq!(ids(&format!(r#"{IMG}width:&#49;20px">"#)), ["OL-IMG-005"]);
    // A declaration CSS can't read is dropped, so the size before it stands.
    let html = format!(r#"{IMG}width:120px;width:&amp;#49;20px">"#);
    let all = found(&html);
    assert_eq!(ids(&html), ["OL-IMG-005"], "{all:?}");
    let fix = all.first().and_then(|f| f.fix.clone()).expect("a fix");
    assert_eq!(fix.replacement, r#" width="120""#);
}

#[test]
fn a_call_inside_a_css_comment_is_not_found() {
    assert_eq!(ids(r#"<p style="color: red /* var(--brand) */">Priya Raman</p>"#), Vec::<&str>::new());
    assert_eq!(ids(r#"<p style="color: red /* rgb(1 2 3) */">Priya Raman</p>"#), Vec::<&str>::new());
    // A real call beside a comment still is.
    assert_eq!(ids(r#"<p style="color: var(--brand) /* the brand */">Priya Raman</p>"#), ["OL-DEC-003"]);
    assert_eq!(ids(r#"<p style="color: /* x */ rgb(1 2 3)">Priya Raman</p>"#), ["OL-COL-002"]);
}

#[test]
fn a_declaration_after_a_css_comment_is_read_by_its_property() {
    assert_eq!(ids(r#"<style>.card { /* layout */ display: flex }</style><p class="card">Priya Raman</p>"#), ["OL-LAY-001"]);
    assert_eq!(ids(r#"<p style="/* layout */ display: flex">Priya Raman</p>"#), ["OL-LAY-001"]);
    assert_eq!(ids(r#"<p style="color: red; /* a */ /* b */ float: left">Priya Raman</p>"#), ["OL-LAY-003"]);
}

#[test]
fn a_variable_that_is_a_css_wide_keyword_is_not_written_in_its_place() {
    for keyword in ["initial", "inherit", "unset", "revert", "revert-layer", "INITIAL"] {
        let html = format!(r#"<p style="--brand: {keyword}; color: var(--brand, red)">Priya Raman</p>"#);
        let all = found(&html);
        let call = all.iter().find(|f| html.get(f.range.clone()) == Some("var(--brand, red)")).expect("the var() is found");
        assert_eq!(call.fix, None, "{keyword}: the keyword is written in the var()'s place");
    }
    // Control: a plain value still is.
    let html = r#"<p style="--brand: red; color: var(--brand)">Priya Raman</p>"#;
    let all = found(html);
    let call = all.iter().find(|f| html.get(f.range.clone()) == Some("var(--brand)")).expect("the var() is found");
    assert_eq!(call.fix.as_ref().map(|f| f.replacement.as_str()), Some("red"));
}
