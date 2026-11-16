// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn rust_fences_are_doctests_and_others_are_not() {
    for info in ["", "rust", "no_run", "rust,should_panic", "compile_fail", "edition2024"] {
        assert!(is_doctest_fence(info), "{info:?}");
    }
    for info in ["text", "ignore", "js", "rust,ignore", "sh", "toml"] {
        assert!(!is_doctest_fence(info), "{info:?}");
    }
}

#[test]
fn only_opening_fences_in_doc_comments_count() {
    let text = "//! ```text\n//! a\n//! ```\n/// ```\n/// let x = 1;\n/// ```\n// ```\nfn f() {}\n/// ```rust,ignore\n/// ```\n";
    assert_eq!(doctests_in(text), [4]);
}

#[test]
fn a_longer_fence_and_fences_inside_a_block_follow_commonmark() {
    // A four-backtick Rust block is a doc-test; the three-backtick lines inside it don't close it.
    let text = "/// ````rust\n/// ```\n/// ````\n/// ```text\n/// ~~~\n/// ```\n/// ```\n/// x\n/// ```\n";
    assert_eq!(doctests_in(text), [1, 7]);
}
