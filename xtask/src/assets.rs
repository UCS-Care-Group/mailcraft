// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask assets`: every non-code asset must be attributed in `ATTRIBUTION.md`.
//!
//! Scans all git-tracked (and untracked, not ignored) files with an asset extension, plus
//! everything under `assets/`, `docs/images/` and `examples/`, and fails if a path is not listed
//! as `` `path` `` in ATTRIBUTION.md. See the asset policy in AGENTS.md.

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
    files.iter().filter(|f| is_asset(f) && !attributed(f, assets_md)).cloned().collect()
}

/// Brand files are not under the project licence: their row must say so by pointing at TRADEMARKS.md.
const TRADEMARKS_DIR: &str = "assets/trademarks/";

fn attributed(file: &str, assets_md: &str) -> bool {
    let key = format!("`{file}`");
    let mut rows = assets_md.lines().filter(|l| l.contains(&key));
    if file.starts_with(TRADEMARKS_DIR) { rows.any(|l| l.contains("TRADEMARKS.md")) } else { rows.next().is_some() }
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
    if miss.is_empty() {
        println!("assets: all {} asset files attributed in ATTRIBUTION.md", files.iter().filter(|f| is_asset(f)).count());
        Ok(())
    } else {
        Err(format!("{} asset file(s) lack an ATTRIBUTION.md entry (author, source, licence):\n  {}", miss.len(), miss.join("\n  ")))
    }
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod assets_tests;
