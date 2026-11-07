// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The composer's reading of HTML (a paste, a draft, a signature) stays a small multiple of the
//! HTML (AU3-01): a link over many lines is not copied into every line's run past a share of the
//! input, the paragraphs stop at `MAX_PARAGRAPHS`, and written out the text is at most `K` times
//! the input and a fixed allowance.

use mailcraft_mime::rich::{from_html, from_html_with_pictures};
use mailcraft_model::rich::{MAX_PARAGRAPHS, RichText};
use proptest::prelude::*;

/// The most the text written out may be per byte of input.
const K: usize = 64;

/// What a short document may make besides.
const ALLOWANCE: usize = 64 * 1024;

/// The link bytes the runs name, counted once per run.
fn link_bytes(rich: &RichText) -> usize {
    rich.paragraphs().iter().flat_map(|p| &p.runs).map(|r| r.style.link.as_deref().map_or(0, str::len)).sum()
}

fn runs(rich: &RichText) -> usize {
    rich.paragraphs().iter().map(|p| p.runs.len()).sum()
}

fn written(rich: &RichText) -> usize {
    serde_json::to_vec(rich).map_or(usize::MAX, |v| v.len())
}

fn both(html: &str) -> [(&'static str, RichText); 2] {
    [("from_html", from_html(html)), ("from_html_with_pictures", from_html_with_pictures(html, &mut |_| Some(1)))]
}

fn bounded(html: &str) -> Result<(), TestCaseError> {
    for (what, rich) in both(html) {
        let made = written(&rich);
        prop_assert!(made <= K * html.len() + ALLOWANCE, "{what}: {made} bytes from {} of input", html.len());
        let links = link_bytes(&rich);
        prop_assert!(links <= 4 * html.len() + ALLOWANCE, "{what}: {links} link bytes from {} of input", html.len());
        prop_assert!(rich.paragraphs().len() <= MAX_PARAGRAPHS, "{what}: {} paragraphs", rich.paragraphs().len());
    }
    Ok(())
}

#[test]
fn a_long_link_over_many_lines_is_not_copied_into_every_run() {
    let html = format!("<p><a href=\"https://example.org/{}\">{}</a></p>", "a".repeat(2000), "x<br>".repeat(400_000));
    for (what, rich) in both(&html) {
        let links = link_bytes(&rich);
        assert!(links <= 4 * html.len() + ALLOWANCE, "{what}: {links} link bytes from {} of input", html.len());
        assert!(rich.paragraphs().len() <= MAX_PARAGRAPHS, "{what}: {} paragraphs", rich.paragraphs().len());
        assert!(runs(&rich) <= 2 * MAX_PARAGRAPHS, "{what}: {} runs", runs(&rich));
        // The first lines still lead where the link said.
        assert!(rich.paragraphs().first().and_then(|p| p.runs.first()).is_some_and(|r| r.style.link.is_some()), "{what}");
    }
}

#[test]
fn a_short_link_over_a_few_lines_keeps_it_on_every_line() {
    let html = "<p><a href=\"https://example.org/a\">one<br>two<br>three</a></p>";
    for (what, rich) in both(html) {
        let linked = rich.paragraphs().iter().flat_map(|p| &p.runs).filter(|r| r.style.link.as_deref() == Some("https://example.org/a")).count();
        assert_eq!(linked, 3, "{what}: {rich:?}");
    }
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => Just("x".to_string()),
        3 => Just("<br>".to_string()),
        2 => prop::sample::select(vec!["<b>", "</b>", "<i>", "</i>", "<p>", "</a>", "<li>", "<div>"]).prop_map(str::to_string),
        1 => (0usize..70_000).prop_map(|n| format!("<a href=\"https://example.org/{}\">", "a".repeat(n))),
        1 => Just("<span style=\"font-family:georgia,serif;color:#123456;background:#fedcba\">".to_string()),
        1 => Just("<img src=\"cid:p@example.org\">".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn output_is_a_small_multiple_of_input(pieces in prop::collection::vec(piece(), 0..200), repeat in 1usize..200) {
        bounded(&pieces.concat().repeat(repeat))?;
    }
}
