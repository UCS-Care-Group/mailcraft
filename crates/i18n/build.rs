// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Compiles in every catalogue in `po/` (`fr.po`, …): a language is offered once its file is
//! there. `messages.pot` (the template) is not a catalogue.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("po");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("po") {
                continue;
            }
            let Some(code) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            // A language code: letters, digits, `_` and `-` only.
            if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                println!("cargo:warning={} is not named by a language code; left out", path.display());
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            files.push((code.to_string(), path.display().to_string()));
        }
    }
    files.sort();
    let mut out =
        String::from("/// The catalogues compiled in: language code, text of its `.po` file.\npub(crate) static FILES: &[(&str, &str)] = &[\n");
    for (code, path) in &files {
        let _ = writeln!(out, "    ({code:?}, include_str!({path:?})),");
    }
    out.push_str("];\n");
    let Some(target) = std::env::var_os("OUT_DIR") else {
        println!("cargo:warning=OUT_DIR is not set");
        return;
    };
    if let Err(e) = std::fs::write(Path::new(&target).join("catalogues.rs"), out) {
        println!("cargo:warning=could not write the list of catalogues: {e}");
    }
}
