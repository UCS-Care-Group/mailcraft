// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which names an agent may have written under `--files`: not the kinds the attachment opener
//! refuses or asks about first.

use crate::files::FilesDir;

fn folder(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mailcraft-mcp-risky-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn kinds_that_ask_before_opening_are_not_written() {
    let d = folder("refused");
    let f = FilesDir::new(&d).unwrap();
    for name in ["report.docm", "page.html", "page.HTM", "old.xls", "old.DOC", "deck.ppt", "x.txt.html", "pic.svg", "run.exe", "a.XLSM"] {
        assert!(f.target(name, "").is_err(), "{name} should be refused");
    }
    for name in ["notes.txt", "data.csv", "report.pdf", "Notes.TXT", "html.txt"] {
        assert!(f.target(name, "").is_ok(), "{name} should be allowed");
    }
    let _ = std::fs::remove_dir_all(&d);
}
