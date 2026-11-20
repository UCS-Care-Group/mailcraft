// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The authored-signature writer's own reading of CSS (`authored_css`), which every sheet it puts
//! in a message is held to: `scoped` takes nothing but `@media` blocks and rules each of whose
//! selectors starts with the scope class, and refuses anything else, whatever reads the text
//! differently. Also the writer's output read the same way, for arbitrary sheets, and the one
//! helper that says where a slice of the text is (`tokenizer::span_of`).

use proptest::prelude::*;

use crate::authored_css::{balanced, functions_known, scoped};
use crate::tokenizer::span_of;
use crate::{AuthoredScope, authored_fragment_with_rebound_cids};

const SCOPE: &str = "mc-sig";

#[test]
fn sheets_whose_every_selector_starts_with_the_scope_are_scoped() {
    let eight_deep = format!("{}.mc-sig p {{color:red}}{}", "@media screen{".repeat(8), "}".repeat(8));
    for css in [
        "",
        ".mc-sig p {color:red}",
        ".mc-sig p, div.mc-sig td {color:red}\n",
        ".mc-sig {padding:4px}",
        "div.mc-sig > p {margin:0}",
        ".mc-sig .x:not(.y) {color:red}",
        ".mc-sig li:nth-child(2n+1) {color:red}",
        "@media (max-width: 600px){.mc-sig p {color:red}}",
        "@media screen{@media (prefers-color-scheme: dark){.mc-sig p {color:#fff}}}",
        ".mc-sig p {font-family:\"A, B\", sans-serif}",
        "/* a note */ .mc-sig p {color:red}",
        eight_deep.as_str(),
    ] {
        assert!(scoped(css, SCOPE), "refused: {css:?}");
    }
}

#[test]
fn a_sheet_with_anything_outside_the_scope_is_refused() {
    let nine_deep = format!("{}.mc-sig p {{color:red}}{}", "@media screen{".repeat(9), "}".repeat(9));
    for css in [
        // Selectors that don't start with the scope class.
        "p {color:red}",
        ".mc-sig p, p {color:red}",
        ".mc-sig p {color:red} p {color:blue}",
        ".other p {color:red}",
        ".mc-sig-x p {color:red}",
        ".MC-SIG p {color:red}",
        "div .mc-sig p {color:red}",
        "*.mc-sig p {color:red}",
        ".\\6d c-sig p {color:red}",
        "@media print {p {color:red}}",
        // A sibling of the wrapper, which stands outside the signature.
        ".mc-sig + p {color:red}",
        ".mc-sig ~ p {color:red}",
        ".mc-sig:first-child + p {color:red}",
        // Arguments the cascade doesn't read.
        ":is(.mc-sig) p {color:red}",
        ".mc-sig p:has(a) {color:red}",
        ".mc-sig p::not(a) {color:red}",
        // Other at-rules.
        "@import 'x.css';",
        "@font-face {font-family:x}",
        ".mc-sig p {@media screen{}}",
        // What a CSS reader would read into another shape.
        "@media screen {.mc-sig p {color:red}",
        ".mc-sig p {color:red",
        ".mc-sig p {color:red}}",
        ".mc-sig p {a{color:red}}",
        ".mc-sig p {color:red} ;",
        ".mc-sig p [ {color:red}",
        ".mc-sig p {background:url(x.png)}",
        ".mc-sig p {color:red} <",
        ".mc-sig p {color:red} -->",
        ".mc-sig p {content:'unterminated}",
        ".mc-sig p {content:'a\nb'}",
        "/* open comment .mc-sig p {color:red}",
        nine_deep.as_str(),
    ] {
        assert!(!scoped(css, SCOPE), "taken: {css:?}");
    }
}

#[test]
fn brackets_and_functions_are_read_as_a_css_reader_reads_them() {
    for (css, paired) in [("a(b[c]{d})", true), ("'(' x", true), ("/* ( */", true), ("(]", false), ("(", false), (")", false), ("[(])", false)] {
        assert_eq!(balanced(css), paired, "{css:?}");
    }
    for (css, known) in [(":not(.a)", true), ("p:nth-of-type(2)", true), ("::not(a)", false), ("x(a)", false), (":is(a)", false), (": not(a)", false)]
    {
        assert_eq!(functions_known(css), known, "{css:?}");
    }
}

#[test]
fn span_of_finds_a_slice_of_the_text_and_nothing_else() {
    let whole = "<p style=\"color:red\">é</p>";
    assert_eq!(whole.get(10..19).and_then(|part| span_of(whole, part)), Some(10..19));
    assert_eq!(span_of(whole, whole), Some(0..whole.len()));
    assert_eq!(whole.get(whole.len()..).and_then(|end| span_of(whole, end)), Some(whole.len()..whole.len()));
    let copy = whole.to_string();
    assert_eq!(copy.get(10..19).and_then(|part| span_of(whole, part)), None, "a copy is no slice of the text");
    assert_eq!(span_of(whole.get(..10).unwrap_or(""), whole), None, "a slice longer than the text is no slice of it");
}

/// The sheets of a written head (`<style …>CSS</style>`, again and again), or `None` when it is
/// anything else.
fn sheets_of(head: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut rest = head;
    while !rest.is_empty() {
        let open = rest.strip_prefix("<style")?;
        let tag_end = open.find('>')?;
        let body = open.get(tag_end + 1..)?;
        let close = body.find("</style>")?;
        out.push(body.get(..close)?);
        rest = body.get(close + "</style>".len()..)?;
    }
    Some(out)
}

/// One piece of a style sheet: selectors, combinators, brackets, at-rules, strings, escapes and
/// what the writer must refuse.
fn sheet_piece() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop::sample::select(vec![
            "p", ".a", "td", "body", "*", "html", ":root", "#b", "[x]", " ", ",", ">", "+", "~", "{", "}", ";", "color:red", "--v:1px",
            "@media (max-width:600px)", "@media screen", "@import", "@font-face", ":not(.x)", ":nth-child(2)", "::before", ":is(p)",
            "(", ")", "[", "]", "'", "\"", "/*", "*/", "\\", "\\61 ", "mc-sig", "url(x)", "<", "-->", "\n", "\r", "!important",
        ])
        .prop_map(str::to_string),
        1 => any::<char>().prop_map(String::from),
    ]
}

proptest! {
    /// Whatever the sheet, what the writer puts in the head is nothing but `<style>` elements
    /// whose text `scoped` takes, and the body is the signature in its wrapper.
    #[test]
    fn every_sheet_the_writer_puts_out_is_scoped(pieces in prop::collection::vec(sheet_piece(), 0..40), text in "[a-z <>/=\"]{0,40}") {
        let html = format!("<style>{}</style><p class=\"a\">{text}</p>", pieces.concat());
        let Ok(scope) = AuthoredScope::new(SCOPE) else { return Err(TestCaseError::fail("no scope")) };
        if let Ok(fragment) = authored_fragment_with_rebound_cids(&html, &scope, &mut |cid: &str| Some(cid.to_string())) {
            let sheets = sheets_of(&fragment.head);
            prop_assert!(sheets.is_some(), "the head is not whole style elements: {:?}", fragment.head);
            for sheet in sheets.unwrap_or_default() {
                prop_assert!(scoped(sheet, SCOPE), "unscoped sheet written: {sheet:?} from {html:?}");
            }
            prop_assert!(fragment.body.starts_with("<div class=\"mc-sig"), "{:?}", fragment.body);
            prop_assert!(fragment.body.ends_with("</div>"), "{:?}", fragment.body);
        }
    }
}
