// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask test-files`: tests live in files of their own (`tests.rs`, `<name>_tests.rs`,
//! `tests/`), which code scanning skips; an inline `#[cfg(test)] mod … { … }` in a source file is
//! scanned as production code, and its made-up secrets are flagged.
//!
//! The files that had one when the check came in are listed in `xtask/inline-tests.txt`. A file
//! not on the list may not add one, and a file whose tests have moved out comes off the list, so
//! it only shrinks.

use std::path::Path;
use std::process::Command;

pub const LIST: &str = "xtask/inline-tests.txt";

/// Whether a Rust file is itself a test file, which may hold anything.
pub fn is_test_file(path: &str) -> bool {
    path.ends_with("/tests.rs") || path == "tests.rs" || path.ends_with("_tests.rs") || path.starts_with("tests/") || path.contains("/tests/")
}

/// Whether `text` has a `mod … {` under `#[cfg(test)]` (other attributes may sit between them).
pub fn has_inline_tests(text: &str) -> bool {
    let mut under_cfg_test = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#[") {
            under_cfg_test |= line == "#[cfg(test)]";
            continue;
        }
        let declares = line.strip_prefix("pub ").unwrap_or(line).starts_with("mod ") && line.ends_with('{');
        if under_cfg_test && declares {
            return true;
        }
        under_cfg_test = false;
    }
    false
}

/// What is wrong, given the source files with inline tests (`found`) and the list: files that
/// added them, and listed files that no longer have them.
pub fn problems<'a>(found: &[&'a str], listed: &[&'a str]) -> (Vec<&'a str>, Vec<&'a str>) {
    let added = found.iter().filter(|f| !listed.contains(f)).copied().collect();
    let gone = listed.iter().filter(|f| !found.contains(f)).copied().collect();
    (added, gone)
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "*.rs"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    let files: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).collect();
    let found: Vec<&str> = files
        .iter()
        .filter(|f| !is_test_file(f))
        .filter(|f| std::fs::read_to_string(root.join(f)).is_ok_and(|text| has_inline_tests(&text)))
        .map(String::as_str)
        .collect();
    let list = std::fs::read_to_string(root.join(LIST)).map_err(|e| format!("{LIST}: {e}"))?;
    let listed: Vec<&str> = list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let (added, gone) = problems(&found, &listed);
    let mut errors = Vec::new();
    if !added.is_empty() {
        errors.push(format!(
            "{} file(s) have an inline `#[cfg(test)] mod … {{ }}`; move the tests to `<name>_tests.rs` (see CONTRIBUTING.md):\n  {}",
            added.len(),
            added.join("\n  ")
        ));
    }
    if !gone.is_empty() {
        errors.push(format!("{} file(s) no longer have inline tests; take them off {LIST}:\n  {}", gone.len(), gone.join("\n  ")));
    }
    if errors.is_empty() {
        println!("test-files: no new inline test modules ({} older ones still to move)", listed.len());
        return Ok(());
    }
    Err(errors.join("\n"))
}

#[cfg(test)]
#[path = "test_files_tests.rs"]
mod tests;
