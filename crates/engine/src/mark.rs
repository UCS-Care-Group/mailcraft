// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Marking a file written from mail as coming from the Internet, as a browser marks a download,
//! so that the program that opens it later treats it with care (Office's Protected View,
//! Gatekeeper). Best effort: a failure is logged and never fails the save.
//!
//! - Windows: the `Zone.Identifier` stream with `ZoneId=3`.
//! - macOS: the `com.apple.quarantine` attribute, set by `/usr/bin/xattr` with fixed arguments
//!   (no shell; the path is one argument, made absolute so that it can't read as an option).
//! - Elsewhere there is no such mark.

use std::path::{Path, PathBuf};

/// What the `Zone.Identifier` stream holds: zone 3, the Internet.
pub const ZONE_IDENTIFIER: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

/// The alternate data stream of `path` that holds its zone.
pub fn zone_stream(path: &Path) -> PathBuf {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    PathBuf::from(stream)
}

/// The value of `com.apple.quarantine`: flags (downloaded, not yet opened), the time in hex
/// seconds, the name of the program that wrote the file.
pub fn quarantine_value(unix_seconds: u64) -> String {
    format!("0083;{unix_seconds:x};MailCraft;")
}

/// Mark `path` as from the Internet where the system has such a mark.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    if let Err(e) = std::fs::write(zone_stream(path), ZONE_IDENTIFIER) {
        log::warn!("a file from mail wasn't marked as from the Internet ({:?})", e.kind());
    }
    #[cfg(target_os = "macos")]
    {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let done = std::process::Command::new("/usr/bin/xattr")
            .arg("-w")
            .arg("com.apple.quarantine")
            .arg(quarantine_value(now))
            .arg(&absolute)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !done.is_ok_and(|s| s.success()) {
            log::warn!("a file from mail wasn't quarantined");
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
}
