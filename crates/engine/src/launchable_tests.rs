// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The kinds of file that are launchable or ask first (A4-V1, A7-2, A7-3).

use crate::opening::{BLOCKED, MACROS, Verdict, is_launchable, verdict};

#[test]
fn more_launchers_are_never_opened() {
    for ext in [
        "wll",
        "ppkg",
        "provxml",
        "job",
        "ops",
        "mof",
        "inx",
        "dqy",
        "oqy",
        "rqy",
        "ica",
        "mobileconfig",
        "mobileprovision",
        "rdg",
        "term",
        "wflow",
        "flatpakrepo",
        "cdxml",
        "swf",
    ] {
        assert_eq!(verdict(&format!("x.{ext}")), Verdict::Blocked, "{ext}");
        assert_eq!(verdict(&format!("X.{}", ext.to_uppercase())), Verdict::Blocked, "{ext}");
        assert!(is_launchable(ext));
    }
}

#[test]
fn legacy_office_binaries_ask_first() {
    for ext in ["doc", "dot", "xls", "xlt", "xlw", "ppt", "pot", "pps", "ppa", "pub", "wbk", "vsd", "vss", "vst", "vdx"] {
        assert_eq!(verdict(&format!("invoice.{ext}")), Verdict::Macros, "{ext}");
    }
    assert_eq!(verdict("a.docx"), Verdict::Open);
}

#[test]
fn the_agent_write_list_is_the_refuse_to_open_list() {
    // What an agent may not have written is what the owner may not open: one list.
    for ext in [
        "bat",
        "cmd",
        "exe",
        "js",
        "py",
        "sh",
        "scf",
        "search-ms",
        "library-ms",
        "msc",
        "iso",
        "msix",
        "xll",
        "chm",
        "rdp",
        "terminal",
        "workflow",
        "scpt",
        "wll",
    ] {
        assert!(is_launchable(ext), "{ext}");
    }
    assert!(BLOCKED.iter().all(|e| is_launchable(e)));
    assert!(!is_launchable("pdf") && !is_launchable("txt") && !is_launchable(""));
    // A kind is in one list only.
    assert!(MACROS.iter().all(|e| !BLOCKED.contains(e)));
}
