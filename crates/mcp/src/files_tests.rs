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

#[test]
fn launchers_and_non_ascii_kinds_are_not_written() {
    let d = folder("au3-kinds");
    let f = FilesDir::new(&d).unwrap();
    for name in [
        "a.mad",
        "a.appcontent-ms",
        "a.vsmacros",
        "a.bgi",
        "a.asx",
        "a.mcf",
        "a.pcd",
        "a.plg",
        "a.osd",
        "a.OSD",
        "a.exe.",
        "a.exe ",
        "a.j\u{17f}",
        "a.\u{17f}cr",
        "a.p\u{131}f",
    ] {
        assert!(f.target(name, "").is_err(), "{name:?} should be refused");
    }
    assert!(f.target("a.pdf.", "").is_ok());
    let _ = std::fs::remove_dir_all(&d);
}
