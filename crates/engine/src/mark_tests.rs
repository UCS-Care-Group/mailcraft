// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The marks on files written from mail (A4-7, A7-1); the platform parts are best effort.

use std::path::Path;

use crate::mark::{ZONE_IDENTIFIER, mark_from_internet, quarantine_value, zone_stream};

#[test]
fn the_zone_stream_is_the_files_own_stream_in_zone_three() {
    assert_eq!(zone_stream(Path::new("C:\\a\\x.doc")).to_string_lossy(), "C:\\a\\x.doc:Zone.Identifier");
    assert_eq!(ZONE_IDENTIFIER, b"[ZoneTransfer]\r\nZoneId=3\r\n");
}

#[test]
fn the_quarantine_value_has_flags_a_hex_time_and_our_name() {
    assert_eq!(quarantine_value(255), "0083;ff;MailCraft;");
}

#[test]
fn marking_a_missing_file_does_not_panic() {
    mark_from_internet(Path::new("/nonexistent-folder/for/mark.doc"));
}

#[test]
fn marking_leaves_the_bytes_alone() {
    let dir = std::env::temp_dir().join(format!("mc-mark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("saved.doc");
    std::fs::write(&file, b"bytes").unwrap();
    mark_from_internet(&file);
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
    #[cfg(windows)]
    assert_eq!(std::fs::read(zone_stream(&file)).unwrap(), ZONE_IDENTIFIER);
    let _ = std::fs::remove_dir_all(&dir);
}
