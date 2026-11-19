// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility checker's fixes are bounded however the document is written
//! (AGENTS.md › Never crash: cap allocations on hostile input). A `var()`'s fix writes its
//! variable's value in its place, so a long value used in many calls was once copied into every
//! one of their findings: about 57.6 MB at a signature's 64 KB, and about 2.1 GB at the most
//! [`check`] reads ([`MAX_INPUT`]). A value longer than [`MAX_FIX_BYTES`] is now found without a
//! fix, every call
//! is still found, and the fixes of one check hold at most [`MAX_FINDINGS`] times
//! [`MAX_FIX_BYTES`] bytes. An ordinary variable is put in place as before.

use mailcraft_html::MAX_INPUT;
use mailcraft_html::compat::{CheckError, Finding, MAX_FINDINGS, MAX_FIX_BYTES, Rule, check};

/// The longest HTML a signature keeps (`mailcraft_model::signature::MAX_HTML`, which this crate
/// can't name).
const SIGNATURE_LIMIT: usize = 64 * 1024;

/// What the fixes of one check may hold at most, all together.
const BUDGET: usize = MAX_FINDINGS * MAX_FIX_BYTES;

// The bound is the one the checker documents: 256 KB.
const _: () = assert!(BUDGET <= 256 * 1024);

/// A `style` attribute that defines `--x` once with a long plain value, then uses it in `calls`
/// `var()`s; `limit` bytes in all, the value taking what the calls leave.
fn inline_copies(limit: usize, calls: usize) -> String {
    let uses = format!(";c:{}", "var(--x) ".repeat(calls));
    let (head, tail) = ("<p style=\"--x:", "\">x</p>");
    let value = limit.saturating_sub(head.len() + uses.len() + tail.len());
    let html = format!("{head}{}{uses}{tail}", "a".repeat(value));
    assert_eq!(html.len(), limit, "fixture guard: the document is as long as asked");
    html
}

/// The same with `--x` defined once at the root of a style sheet, and used in a `style`
/// attribute.
fn root_copies(limit: usize, calls: usize) -> String {
    let uses = format!("\"c:{}\"", "var(--x) ".repeat(calls));
    let (head, middle, tail) = ("<style>:root{--x:", "}</style><p style=", ">x</p>");
    let value = limit.saturating_sub(head.len() + middle.len() + uses.len() + tail.len());
    let html = format!("{head}{}{middle}{uses}{tail}", "a".repeat(value));
    assert_eq!(html.len(), limit, "fixture guard: the document is as long as asked");
    html
}

fn fix_bytes(found: &[Finding]) -> usize {
    found.iter().filter_map(|f| f.fix.as_ref()).map(|fix| fix.replacement.len()).sum()
}

/// Every call is found on its own bytes, the definition too, and no fix copies the long value.
fn assert_found_without_copies(html: &str, calls: usize) {
    let found = check(html).unwrap_or_else(|e| panic!("checked: {e:?}"));
    let variables: Vec<&Finding> = found.iter().filter(|f| f.rule == Rule::CssVariable).collect();
    assert_eq!(variables.len(), calls + 1, "the definition and every call are still found");
    let uses = variables.iter().filter(|f| html.get(f.range.clone()) == Some("var(--x)")).count();
    assert_eq!(uses, calls, "each call is found on its own bytes");
    let bytes = fix_bytes(&found);
    assert!(bytes <= BUDGET, "the fixes hold {bytes} bytes, more than the {BUDGET} a check may give");
    assert!(found.iter().all(|f| f.fix.as_ref().is_none_or(|fix| fix.replacement.len() <= MAX_FIX_BYTES)), "a fix longer than MAX_FIX_BYTES");
    assert!(variables.iter().all(|f| f.fix.is_none()), "a value longer than MAX_FIX_BYTES is offered as a fix");
}

/// At a signature's own limit, as many calls as are found without refusing: once about 57.6 MB of
/// copies of the value.
#[test]
fn a_long_variable_used_many_times_in_a_signature_is_found_without_copies() {
    let calls = MAX_FINDINGS - 1;
    assert_found_without_copies(&inline_copies(SIGNATURE_LIMIT, calls), calls);
    // The root rule is also refused as a sheet that can't be sent (MC-CSS-001): one finding more.
    let calls = MAX_FINDINGS - 2;
    assert_found_without_copies(&root_copies(SIGNATURE_LIMIT, calls), calls);
}

/// At the most the checker reads at all: once about 2.1 GB of copies.
#[test]
fn a_long_variable_used_many_times_at_the_public_limit_is_found_without_copies() {
    let calls = MAX_FINDINGS - 1;
    assert_found_without_copies(&inline_copies(MAX_INPUT, calls), calls);
}

/// A document with more calls than findings are given is still refused, as before, and builds no
/// copies on its way there.
#[test]
fn too_many_calls_of_a_long_variable_are_still_refused() {
    assert_eq!(check(&inline_copies(SIGNATURE_LIMIT, MAX_FINDINGS)), Err(CheckError::TooManyFindings));
    assert_eq!(check(&inline_copies(MAX_INPUT, MAX_FINDINGS)), Err(CheckError::TooManyFindings));
}

/// A value of MAX_FIX_BYTES is put in place; one byte more is found without a fix.
#[test]
fn a_value_up_to_the_limit_is_put_in_place_and_a_longer_one_is_not() {
    for (length, offered) in [(MAX_FIX_BYTES, true), (MAX_FIX_BYTES + 1, false)] {
        let value = "a".repeat(length);
        let html = format!("<p style=\"--x:{value};c:var(--x)\">x</p>");
        let found = check(&html).unwrap_or_else(|e| panic!("checked: {e:?}"));
        let call = found.iter().find(|f| html.get(f.range.clone()) == Some("var(--x)")).unwrap_or_else(|| panic!("the call is found: {found:?}"));
        assert_eq!(call.rule, Rule::CssVariable);
        assert_eq!(call.fix.as_ref().map(|fix| fix.replacement.as_str()), offered.then_some(value.as_str()), "a value of {length} bytes");
    }
}

/// An ordinary variable is put in place, from the element's own `style` or the root, as before.
#[test]
fn an_ordinary_variable_is_still_put_in_place() {
    for html in
        [r#"<p style="--brand:#0a6e4f;color:var(--brand)">Ada</p>"#, r#"<style>:root{--brand:#0a6e4f}</style><p style="color:var(--brand)">Ada</p>"#]
    {
        let found = check(html).unwrap_or_else(|e| panic!("checked: {e:?}"));
        let call = found.iter().find(|f| html.get(f.range.clone()) == Some("var(--brand)")).unwrap_or_else(|| panic!("the call is found: {found:?}"));
        let fix = call.fix.as_ref().unwrap_or_else(|| panic!("a fix for {html}"));
        assert_eq!((fix.range.clone(), fix.replacement.as_str()), (call.range.clone(), "#0a6e4f"));
    }
}
