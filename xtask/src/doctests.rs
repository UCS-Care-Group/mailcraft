// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether the workspace has a doc-test. nextest doesn't run them and `cargo test --doc` costs a
//! build of its own (minutes on CI) for none, so `cargo xtask ci` runs it only when this finds a
//! code block in a doc comment that rustdoc would run.

use std::path::Path;

/// Fence words that make rustdoc compile or run a block (an empty info string means Rust too).
const RUST_WORDS: &[&str] = &["rust", "no_run", "should_panic", "compile_fail", "test_harness", "standalone_crate"];

/// Whether a fence's info string (`rust,no_run`, `text`, empty) is a doc-test.
pub fn is_doctest_fence(info: &str) -> bool {
    info.split([',', ' ']).map(str::trim).filter(|w| !w.is_empty()).all(|w| RUST_WORDS.contains(&w) || w.starts_with("edition"))
}

/// The lines (1-based) of `text` that open a doc-test in a `///` or `//!` comment.
pub fn doctests_in(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    // The open block's fence: its character and length (CommonMark: a closing fence uses the same
    // character, is at least as long and has nothing after it).
    let mut open: Option<(char, usize)> = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        let Some(doc) = t.strip_prefix("///").or_else(|| t.strip_prefix("//!")) else {
            open = None;
            continue;
        };
        let doc = doc.trim_start();
        let Some(c) = doc.chars().next().filter(|c| *c == '`' || *c == '~') else { continue };
        let len = doc.chars().take_while(|x| *x == c).count();
        if len < 3 {
            continue;
        }
        let info = &doc[len * c.len_utf8()..];
        match open {
            Some((oc, olen)) => {
                if c == oc && len >= olen && info.trim().is_empty() {
                    open = None;
                }
            }
            None => {
                open = Some((c, len));
                if is_doctest_fence(info.trim()) {
                    found.push(i + 1);
                }
            }
        }
    }
    found
}

/// Every doc-test opening in the `src/` of the workspace's crates, as `path:line`.
pub fn find(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for top in ["crates", "apps"] {
        let Ok(entries) = std::fs::read_dir(root.join(top)) else { continue };
        for e in entries.flatten() {
            walk(&e.path().join("src"), root, &mut out);
        }
    }
    out.sort();
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, root, out);
        } else if path.extension().is_some_and(|x| x == "rs")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.extend(doctests_in(&text).into_iter().map(|l| format!("{rel}:{l}")));
        }
    }
}

#[cfg(test)]
#[path = "doctests_tests.rs"]
mod tests;
