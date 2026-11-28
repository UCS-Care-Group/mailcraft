// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file-open filters (#1768).

use super::*;

#[test]
fn import_opens_on_all_files_off_macos() {
    let list = filter_list("import", false);
    assert_eq!(list.first(), Some(&ALL));
    assert!(list.len() > 1);
}

#[test]
fn import_has_no_filter_on_macos() {
    // The macOS panel merges filters and has no "*": a filter would grey out `Inbox`.
    assert!(filter_list("import", true).is_empty());
}

#[test]
fn other_purposes_are_the_same_on_macos() {
    for p in ["mailbox", "pst", "picture", "publicKey", "unknown"] {
        assert_eq!(filter_list(p, true), filter_list(p, false));
    }
}
