// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The control channel's key file, one rule for the app (which writes it) and the MCP bridge
//! (which reads it): `<settings>/control-key.local-<port>`. See `docs/control-protocol.md`, Keys.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};

pub mod proof;
#[cfg(test)]
mod proof_tests;
#[cfg(test)]
mod tests;

/// The app's settings folder (preferences, logs, key files): macOS
/// `~/Library/Application Support/MailCraft`, Windows `%APPDATA%\MailCraft`, otherwise
/// `$XDG_CONFIG_HOME/mailcraft` or `~/.config/mailcraft`.
pub fn settings_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/MailCraft"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("MailCraft"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("mailcraft"))
    }
}

/// `<settings>/control-key.local-<port>`: named by the port the app listens on, so a bridge that
/// connects to a port finds the key of the app on that port. (Inside Flatpak the name once held
/// the instance id, which differs for every `flatpak run`, so a bridge run as an instance of its
/// own never found the app's key, #1199.) Only one app can hold a port, so one file per port is
/// enough, and a stale file from an earlier run is replaced when the app starts.
pub fn key_path(settings: &Path, port: u16) -> PathBuf {
    settings.join(format!("control-key.local-{port}"))
}
