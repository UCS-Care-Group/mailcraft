// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The email-compatibility checker's work is linear in its input (the crate's contract), however
//! the input repeats one declaration, selector or call: sixteen times the text takes about sixteen
//! times as long, not two hundred and fifty-six. Each document here once made one of the checker's
//! passes go over the whole of something for every declaration or call it read, and its answer
//! is the same as it was.

use std::time::{Duration, Instant};

use mailcraft_html::compat::{CheckError, check};

/// One `style` attribute of plain declarations, then another of `var()` calls of a variable
/// that is never defined: each call once looked at every declaration of the document.
fn var_calls(limit: usize) -> String {
    let mut s = String::from("<p style=\"");
    while s.len() < limit / 2 {
        s.push_str("a:b;");
    }
    s.push_str("\">x</p><p style=\"c:");
    while s.len() < limit.saturating_sub(16) {
        s.push_str("var(--x)");
    }
    s.push_str("\">y</p>");
    s
}

/// One sheet rule with a long selector list and many `max-width` declarations: each one once
/// split the whole selector list again to see whether the rule is for tables alone.
fn table_widths(limit: usize) -> String {
    let mut s = String::from("<style>");
    while s.len() < limit / 2 {
        s.push_str("p a,");
    }
    s.push_str("p {");
    while s.len() < limit.saturating_sub(16) {
        s.push_str("max-width:1px;");
    }
    s.push_str("}</style>x");
    s
}

/// One sheet rule with a long selector list and many `content` declarations: each one once
/// wrote the whole selector list again in lower case to look for `::before` and `::after`.
fn contents(limit: usize) -> String {
    let mut s = String::from("<style>");
    while s.len() < limit / 2 {
        s.push_str("p a,");
    }
    s.push_str("p {");
    while s.len() < limit.saturating_sub(16) {
        s.push_str("content:x;");
    }
    s.push_str("}</style>x");
    s
}

/// A variable defined once in a sheet rule with a long selector list, and many `var()` calls of
/// it in a `style` attribute: each call once wrote that selector list again in lower case to see
/// whether the rule is the document's root.
fn rooted_calls(limit: usize) -> String {
    let mut s = String::from("<style>");
    while s.len() < limit / 2 {
        s.push_str("p a,");
    }
    s.push_str("p {--x:red}</style><p style=\"c:");
    while s.len() < limit.saturating_sub(16) {
        s.push_str("var(--x)");
    }
    s.push_str("\">y</p>");
    s
}

fn best_of(html: &str) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            let _ = check(html);
            start.elapsed()
        })
        .min()
        .unwrap_or_default()
}

/// Sixteen times the input (`small` bytes, then sixteen times that) in at most forty-eight times
/// the time of the small one (or of a floor below which timing is noise): linear work passes with
/// room to spare, and quadratic work, at two hundred and fifty-six times, does not.
fn assert_linear(name: &str, small: usize, make: fn(usize) -> String) {
    let large = make(16 * small);
    let small = make(small);
    let (s, l) = (best_of(&small), best_of(&large));
    let floor = Duration::from_millis(10);
    assert!(l <= s.max(floor) * 48, "{name}: {} bytes took {l:?}, {} bytes {s:?}: not linear", large.len(), small.len());
}

#[test]
fn var_calls_are_checked_in_linear_time() {
    assert_linear("var() calls", 32 * 1024, var_calls);
    assert_eq!(check(&var_calls(512 * 1024)), Err(CheckError::TooManyFindings));
}

#[test]
fn widths_of_a_rule_with_many_selectors_are_checked_in_linear_time() {
    assert_linear("max-width declarations", 16 * 1024, table_widths);
    assert_eq!(check(&table_widths(256 * 1024)), Err(CheckError::TooManyFindings));
}

#[test]
fn contents_of_a_rule_with_many_selectors_are_checked_in_linear_time() {
    assert_linear("content declarations", 16 * 1024, contents);
    // Nothing but the sheet itself, which is longer than a signature's sheet may be.
    let rules = check(&contents(256 * 1024)).map(|found| found.iter().map(|f| f.rule.id()).collect::<Vec<_>>());
    assert_eq!(rules, Ok(vec!["MC-CSS-001"]));
}

#[test]
fn calls_of_a_variable_defined_under_many_selectors_are_checked_in_linear_time() {
    assert_linear("var() calls of a sheet's variable", 16 * 1024, rooted_calls);
    assert_eq!(check(&rooted_calls(256 * 1024)), Err(CheckError::TooManyFindings));
}

/// The answers the cheaper passes give are the ones they gave: a variable defined once at the
/// root is still put in its call's place, one defined twice is not, and a rule for tables alone
/// still gives no width warning.
#[test]
fn the_cheaper_passes_answer_as_before() {
    let html = "<style>:root{--a:red}.t table, div > table{max-width:600px}</style><p style=\"color:var(--a)\">x</p>";
    let found = check(html).unwrap();
    let fixes: Vec<_> = found.iter().filter_map(|f| f.fix.as_ref()).map(|f| f.replacement.as_str()).collect();
    assert_eq!(fixes, ["red"], "{found:?}");
    assert!(found.iter().all(|f| f.rule.id() != "OL-LAY-005"), "{found:?}");

    let twice = "<style>:root{--a:red}body{--a:blue}</style><p style=\"color:var(--a)\">x</p>";
    assert!(check(twice).unwrap().iter().all(|f| f.fix.is_none()), "a variable defined twice was given a fix");

    let in_media = "<style>@media (max-width:600px){:root{--a:red}}</style><p style=\"color:var(--a)\">x</p>";
    assert!(check(in_media).unwrap().iter().all(|f| f.fix.is_none()), "a variable defined for some windows was given a fix");

    let not_tables = "<style>.t table, div > p{max-width:600px}</style>x";
    assert!(check(not_tables).unwrap().iter().any(|f| f.rule.id() == "OL-LAY-005"));

    let generated = "<style>P::Before, a{content:\"x\"}</style>x";
    assert!(check(generated).unwrap().iter().any(|f| f.rule.id() == "OL-SEL-003"));
}
