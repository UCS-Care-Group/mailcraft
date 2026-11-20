// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the email-compatibility checker offers a value in a `var()`'s place or reads a picture's
//! size from CSS, it reads the CSS as a CSS reader does: a definition in a sheet whose `media` may
//! not hold (one past the attributes kept, or one made only of spaces that aren't CSS white space)
//! is no value for every window, a CSS-wide keyword followed by a comment is still that
//! keyword, and a size a reader drops (`width:bogus`) leaves the one before it standing.
//!
//! The documents are invented; nothing here is fetched.

use mailcraft_html::compat::{Finding, Rule, check};

fn of(html: &str, rule: Rule) -> Vec<Finding> {
    check(html).unwrap_or_else(|e| panic!("the checker reads {html:?}: {e:?}")).into_iter().filter(|f| f.rule == rule).collect()
}

/// The fixes offered for the `var()` calls of `html`.
fn var_fixes(html: &str) -> Vec<Option<String>> {
    of(html, Rule::CssVariable)
        .into_iter()
        .filter(|f| html.get(f.range.clone()).is_some_and(|t| t.starts_with("var(")))
        .map(|f| f.fix.map(|x| x.replacement))
        .collect()
}

#[test]
fn a_definition_in_a_sheet_for_some_windows_is_not_put_in_a_var_s_place() {
    let body = r#"<p style="color:var(--brand,blue)">Priya Raman</p>"#;
    let conditional = format!(r#"<style media="(min-width: 600px)">body {{ --brand: red }}</style>{body}"#);
    assert_eq!(var_fixes(&conditional), [None], "below 600 pixels the words are blue, not red");
    let dark = format!(r#"<style media="(prefers-color-scheme: dark)">:root {{ --brand: red }}</style>{body}"#);
    assert_eq!(var_fixes(&dark), [None]);
    // Control: the same definition for every window is offered.
    let always = format!("<style>body {{ --brand: red }}</style>{body}");
    assert_eq!(var_fixes(&always), [Some("red".to_string())]);
}

#[test]
fn a_css_wide_keyword_followed_by_a_comment_is_not_put_in_a_var_s_place() {
    for value in ["initial/**/", "inherit /* note */", "/**/unset"] {
        let html = format!(r#"<p style="--brand:{value};color:var(--brand,blue)">Priya Raman</p>"#);
        assert_eq!(var_fixes(&html), [None], "{value}: the call stands for its fallback, not the keyword");
    }
    let plain = r#"<p style="--brand:red;color:var(--brand,blue)">Priya Raman</p>"#;
    assert_eq!(var_fixes(plain), [Some("red".to_string())], "control: a plain value is offered");
}

#[test]
fn a_size_a_css_reader_drops_leaves_the_one_before_it() {
    let html = r#"<img src="cid:mark@example.org" alt="mark" style="width:120px;width:bogus">"#;
    let found = of(html, Rule::CssOnlyImageSize);
    assert_eq!(found.len(), 1, "the picture's width is 120 pixels and only CSS says so: {found:?}");
    assert_eq!(found[0].fix.as_ref().map(|f| f.replacement.as_str()), Some(r#" width="120""#));
    // Control: a keyword a CSS reader knows does set the width aside.
    for value in ["auto", "AUTO", "inherit", "max-content"] {
        let reset = format!(r#"<img src="cid:mark@example.org" alt="mark" style="width:120px;width:{value}">"#);
        assert!(of(&reset, Rule::CssOnlyImageSize).is_empty(), "{value}: no width is left to write");
    }
}

/// The reviewer's finding (compat.rs:1324): a sheet whose `media` was past the attributes kept may
/// not hold, so a root definition in it is no value for every window.
#[test]
fn a_definition_in_a_sheet_whose_media_was_past_the_attributes_kept_is_not_put_in_a_var_s_place() {
    let many: String = (0..40).map(|i| format!(" data-a{i}=\"\"")).collect();
    let body = r#"<p style="color:var(--brand,blue)">Priya Raman</p>"#;
    let past = format!(r#"<style{many} media="(min-width: 600px)">body {{ --brand: red }}</style>{body}"#);
    assert_eq!(var_fixes(&past), [None], "below 600 pixels the words are blue, not red");
    // Control: a kept empty `media` is the one that counts, and the sheet is for every window.
    let kept = format!(r#"<style media=""{many} media="(min-width: 600px)">body {{ --brand: red }}</style>{body}"#);
    assert_eq!(var_fixes(&kept), [Some("red".to_string())]);
}

/// A `media` made only of spaces that aren't CSS white space is a media type a CSS reader doesn't
/// know: the sheet holds for no window, and its definitions are no value.
#[test]
fn a_definition_in_a_sheet_whose_media_is_spaces_that_arent_css_white_space_is_not_put_in_a_var_s_place() {
    let body = r#"<p style="color:var(--brand,blue)">Priya Raman</p>"#;
    for media in ["&nbsp;", "&#x2003;", "\u{a0}"] {
        let html = format!(r#"<style media="{media}">body {{ --brand: red }}</style>{body}"#);
        assert_eq!(var_fixes(&html), [None], "{media:?}: no window reads the definition");
    }
    // Control: CSS's own white space alone is no media.
    let html = format!(r#"<style media="&#12; ">body {{ --brand: red }}</style>{body}"#);
    assert_eq!(var_fixes(&html), [Some("red".to_string())]);
}
