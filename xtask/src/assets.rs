// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask assets`: every non-code asset must be attributed in `ATTRIBUTION.md`.
//!
//! Scans all git-tracked (and untracked, not ignored) files with an asset extension, plus
//! everything under `assets/`, `docs/images/` and `examples/`, and fails if a path is not listed
//! as `` `path` `` in ATTRIBUTION.md. See the asset policy in AGENTS.md.
//!
//! The US English spelling dictionary's copyright notice (the `COPYRIGHT, SOURCES, and CREDITS`
//! section of `assets/dictionaries/en_US/README_en_US.txt`, up to `Build Date:`) must appear
//! verbatim in both `ATTRIBUTION.md` and `NOTICE`.

use std::path::Path;
use std::process::Command;

const ASSET_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "icns", "svg", "pdf", "ai", "eps", "psd", "icc", "icm", "ttf", "otf", "woff",
    "woff2", "ase", "indd", "idml", "aco", "abr", "docx", "odt", "rtf", "mp4", "wav", "mp3",
];
const ASSET_DIRS: &[&str] = &["assets/", "docs/images/", "examples/"];

pub fn is_asset(path: &str) -> bool {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    ASSET_DIRS.iter().any(|d| path.starts_with(d)) || ASSET_EXT.contains(&ext.as_str())
}

/// Paths that need an ATTRIBUTION.md entry but lack one.
pub fn missing(files: &[String], assets_md: &str) -> Vec<String> {
    files.iter().filter(|f| is_asset(f) && !assets_md.contains(&format!("`{f}`"))).cloned().collect()
}

/// The copyright lines the dictionary notice has to keep, wherever it is copied.
pub const DICTIONARY_COPYRIGHT_LINES: &[&str] =
    &["Copyright 2000-2026 by Kevin Atkinson", "Copyright 2016 by Benjamin Titze", "Copyright 1993, Geoff Kuenning, Granada Hills, CA"];

const NOTICE_START: &str = "COPYRIGHT, SOURCES, and CREDITS:";
const NOTICE_END: &str = "Build Date:";

/// CRLF and lone CR become LF, so a Windows checkout compares equal to the LF README.
pub fn normalise_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Byte index of a line whose whole text is `line`, or `None` when there is no such line.
fn line_at(text: &str, line: &str) -> Option<usize> {
    line_match(text, line, true)
}

/// Byte index of a line that starts with `prefix` (the rest of the line may continue).
fn line_starting_with(text: &str, prefix: &str) -> Option<usize> {
    line_match(text, prefix, false)
}

fn line_match(text: &str, needle: &str, whole_line: bool) -> Option<usize> {
    let mut rest = text;
    let mut offset = 0;
    loop {
        if rest.starts_with(needle) {
            let next = rest.as_bytes().get(needle.len());
            let ends_line = next.is_none() || next == Some(&b'\n');
            if !whole_line || ends_line {
                return Some(offset);
            }
        }
        let next = rest.find('\n')? + 1;
        offset += next;
        rest = &rest[next..];
    }
}

/// The dictionary's copyright notice: the line [`NOTICE_START`] through the line before
/// [`NOTICE_END`], trailing blank lines removed. `readme` may use CRLF.
pub fn dictionary_copyright_notice(readme: &str) -> Result<String, String> {
    let text = normalise_newlines(readme);
    let start = line_at(&text, NOTICE_START).ok_or_else(|| format!("dictionary README has no line {NOTICE_START:?}"))?;
    let after_start = text[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .ok_or_else(|| format!("dictionary README ends on the {NOTICE_START:?} line, before {NOTICE_END:?}"))?;
    let end = line_starting_with(&text[after_start..], NOTICE_END)
        .map(|i| after_start + i)
        .ok_or_else(|| format!("dictionary README has no line starting with {NOTICE_END:?} after the copyright notice"))?;
    let mut notice = text[start..end].to_string();
    while notice.ends_with('\n') {
        notice.pop();
    }
    while let Some(last) = notice.rfind('\n') {
        if notice[last + 1..].trim().is_empty() {
            notice.truncate(last);
        } else {
            break;
        }
    }
    if notice.is_empty() {
        return Err("dictionary copyright notice is empty".into());
    }
    for line in DICTIONARY_COPYRIGHT_LINES {
        if !notice.contains(line) {
            return Err(format!("dictionary copyright notice is missing {line:?}"));
        }
    }
    Ok(notice)
}

/// Documents whose text does not contain `notice` (newlines normalised). A missing `Copyright`
/// line is named; otherwise the message says the verbatim section is absent.
pub fn documents_missing_dictionary_notice(notice: &str, documents: &[(&str, &str)]) -> Vec<String> {
    let mut missing = Vec::new();
    for (name, body) in documents {
        let body = normalise_newlines(body);
        if body.contains(notice) {
            continue;
        }
        let absent: Vec<&str> = DICTIONARY_COPYRIGHT_LINES.iter().copied().filter(|line| !body.contains(line)).collect();
        if absent.is_empty() {
            missing.push(format!("{name} does not contain the dictionary copyright notice verbatim (from {NOTICE_START} up to {NOTICE_END})"));
        } else {
            missing.push(format!("{name} is missing dictionary copyright lines: {}", absent.join("; ")));
        }
    }
    missing
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    let files: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| root.join(l).exists()).map(str::to_owned).collect();
    let md = std::fs::read_to_string(root.join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let miss = missing(&files, &md);
    let notice = dictionary_notice_problem(root, &md);
    let mut problems = Vec::new();
    if !miss.is_empty() {
        problems.push(format!("{} asset file(s) lack an ATTRIBUTION.md entry (author, source, licence):\n  {}", miss.len(), miss.join("\n  ")));
    }
    if let Err(error) = notice {
        problems.push(error);
    }
    if problems.is_empty() {
        println!("assets: all {} asset files attributed in ATTRIBUTION.md", files.iter().filter(|f| is_asset(f)).count());
        println!("assets: dictionary copyright notice is in ATTRIBUTION.md and NOTICE");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// The README is required. Its copyright notice must be in `ATTRIBUTION.md` and `NOTICE`.
fn dictionary_notice_problem(root: &Path, attribution: &str) -> Result<(), String> {
    let rel = "assets/dictionaries/en_US/README_en_US.txt";
    let readme = std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    let notice_file = std::fs::read_to_string(root.join("NOTICE")).map_err(|e| format!("NOTICE: {e}"))?;
    let notice = dictionary_copyright_notice(&readme)?;
    let missing = documents_missing_dictionary_notice(&notice, &[("ATTRIBUTION.md", attribution), ("NOTICE", notice_file.as_str())]);
    if missing.is_empty() { Ok(()) } else { Err(missing.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unattributed_assets() {
        let files = vec!["assets/icons/a.svg".to_string(), "crates/x/src/lib.rs".into(), "docs/images/b.png".into(), "tests/fixture.png".into()];
        let md = "| `assets/icons/a.svg` | me | here | MIT |";
        assert_eq!(missing(&files, md), vec!["docs/images/b.png".to_string(), "tests/fixture.png".into()]);
        assert!(!is_asset("crates/x/src/lib.rs"));
        assert!(is_asset("assets/fonts/OFL.txt"));
    }
}

#[cfg(test)]
#[path = "assets_notice_tests.rs"]
mod assets_notice_tests;
