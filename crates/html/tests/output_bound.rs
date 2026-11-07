// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What `parse` makes stays a small multiple of what it reads (the crate's contract), whatever
//! the nesting of styles and links and however long the addresses: written out, the blocks are
//! at most `K` times the input and a fixed allowance.

use mailcraft_html::{Block, parse, parse_text_own};
use proptest::prelude::*;

/// The most the blocks written out may be per byte of input.
const K: usize = 64;

/// What a short document may make besides.
const ALLOWANCE: usize = 64 * 1024;

fn written(blocks: &[Block]) -> usize {
    serde_json::to_vec(blocks).map_or(usize::MAX, |v| v.len())
}

fn bounded(html: &str) -> Result<(), TestCaseError> {
    let limit = K * html.len() + ALLOWANCE;
    let made = written(&parse(html));
    prop_assert!(made <= limit, "{made} bytes from {} of input", html.len());
    let own = written(&parse_text_own(html, |u| Some(u.to_string())));
    prop_assert!(own <= limit, "own: {own} bytes from {} of input", html.len());
    Ok(())
}

/// One piece of a document: a tag that opens or closes a style, a block or a link, or text.
fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => Just("x".to_string()),
        2 => prop::sample::select(vec!["b", "i", "u", "s", "code", "em", "strong", "small", "big", "mark"]).prop_map(|t| format!("<{t}>")),
        2 => prop::sample::select(vec!["b", "i", "u", "s", "code", "em", "strong", "a", "span", "font", "p", "div", "td", "tr", "table"])
            .prop_map(|t| format!("</{t}>")),
        1 => prop::sample::select(vec!["<p>", "<div>", "<br>", "<li>", "<table><tr><td>", "<blockquote>", "<h1>", "<pre>"]).prop_map(str::to_string),
        1 => (0usize..8).prop_map(|n| format!("<span style=\"font-family:{}\">", vec!["abcdefghijklmnopqrstuvwxyzabcdef"; n.max(1)].join(","))),
        1 => (0usize..8).prop_map(|n| format!("<font face=\"{}\" color=\"#{:02x}0000\">", vec!["qwertyuiopasdfghjklzxcvbnmqwerty"; n.max(1)].join(","), n * 30)),
        1 => (0usize..70_000, prop::sample::select(vec!["mailto:u@example.org?body=", "tel:+1", "cid:", "https://example.org/"]))
            .prop_map(|(n, scheme)| format!("<a href=\"{scheme}{}\">", "a".repeat(n))),
        1 => Just("<img src=\"cid:p@example.org\" alt=\"x\">".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn output_is_a_small_multiple_of_input(pieces in prop::collection::vec(piece(), 0..400), repeat in 1usize..40) {
        let html = pieces.concat().repeat(repeat);
        bounded(&html)?;
    }
}

#[test]
fn known_shapes_stay_within_the_bound() -> Result<(), TestCaseError> {
    let long = "a".repeat(60 * 1024);
    let fonts = ["abcdefghijklmnopqrstuvwxyzabcdef"; 8].join(",");
    for html in [
        format!("<p><a href=\"mailto:u@example.org?body={long}\">{}</a></p>", "<b>x</b>x".repeat(1000)),
        format!("<a href=\"cid:{long}\">{}</a>", "<p>x</p>".repeat(5000)),
        format!("<a href=\"cid:{long}\">{}</a>", "<img src=\"cid:p@example.org\" alt=\"x\">".repeat(5000)),
        format!("<span style=\"font-family:{fonts}\">{}</span>", "<b>x</b>x".repeat(50_000)),
        "<b>x</b>x".repeat(200_000),
        "<p>x".repeat(100_000),
        "<table><tr><td>x".repeat(10_000),
    ] {
        bounded(&html).map_err(|e| TestCaseError::fail(format!("{}: {e}", html.get(..40).unwrap_or(&html))))?;
    }
    Ok(())
}
