// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every reader of a mailbox file reads within the budget of `load_limit` (AU3-05, AU3-06,
//! AU3-07, H2): the code that opens, imports or is handed mailbox files never parses JSON with
//! `serde_json` itself, which would build what a hostile file says without counting it. A new
//! reader that does fails here; one that is wrapped in the budget is named in [`ALLOWED`].

use std::path::{Path, PathBuf};

/// Parsing with `serde_json` directly.
const PARSES: &[&str] =
    &["serde_json::from_slice", "serde_json::from_str", "serde_json::from_reader", "serde_json::from_value", "Deserializer::from_"];

/// (file, what it does) that reads within the budget: the stream's parser, which reads under
/// `load_limit::within` with every item `Counted`.
const ALLOWED: &[(&str, &str)] = &[("store_stream.rs", "serde_json::Deserializer::from_reader(&mut reader)")];

/// The engine's sources that read mailbox files: the store, `io`, `file.open` and the imports.
fn readers() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = vec![src.join("io.rs"), src.join("cmd/file.rs")];
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                if rel.starts_with("import") {
                    dirs.push(path);
                }
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let test = name.ends_with("_tests.rs") || name == "tests.rs" || rel.contains("/tests/") || name == "test_support.rs";
            if !test && name.ends_with(".rs") && (rel.starts_with("store") || rel.starts_with("import")) {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn every_mailbox_reader_reads_within_the_budget() {
    let files = readers();
    assert!(files.iter().any(|f| f.ends_with("store_index.rs")) && files.iter().any(|f| f.ends_with("import_pst.rs")), "{files:?}");
    let mut found = Vec::new();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if PARSES.iter().any(|p| code.contains(p)) && !ALLOWED.iter().any(|(f, what)| *f == name && code.contains(what)) {
                found.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "read a mailbox file with `mailcraft_model::load_limit::read_json` instead:\n{}", found.join("\n"));
}
