// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a file with the system's program for its kind (#126), without a shell.
//!
//! The system's own opener is started as a program with the file's whole path as one argument:
//!
//! - **Windows:** `explorer.exe <path>`. Given the path of a file, Explorer opens it with the
//!   program registered for its kind, as a double-click does (the Windows shell's file
//!   association, with the file's zone honoured). No `cmd.exe`, so no `start` and no quoting
//!   rules of a command line; the path is one argument that Rust quotes for the program.
//! - **macOS:** `open -- <path>` (Launch Services); `--` ends the options.
//! - **Linux, FreeBSD and the like:** `xdg-open <path>` (the desktop's opener).
//!
//! The path must be absolute, so it can never be read as an option (a Unix path starts with `/`,
//! a Windows one with a drive letter and `:\` or with `\\`), and nothing in it is looked at by a
//! shell. Anything else is refused.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

/// Which opener a platform has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opener {
    Explorer,
    MacOpen,
    XdgOpen,
}

impl Opener {
    /// The opener of the platform this is built for.
    pub(crate) fn this_platform() -> Opener {
        if cfg!(windows) {
            Opener::Explorer
        } else if cfg!(target_os = "macos") {
            Opener::MacOpen
        } else {
            Opener::XdgOpen
        }
    }
}

/// Whether `path` is an absolute path in the way of `opener`'s platform, spelt so that no
/// program could take it for an option.
fn absolute_for(opener: Opener, path: &Path) -> bool {
    let b = path.as_os_str().as_encoded_bytes();
    if b.is_empty() || b.contains(&0) {
        return false;
    }
    match opener {
        Opener::Explorer => {
            let drive = matches!(b, [d, b':', b'\\' | b'/', ..] if d.is_ascii_alphabetic());
            drive || b.starts_with(b"\\\\")
        }
        Opener::MacOpen | Opener::XdgOpen => b.starts_with(b"/"),
    }
}

/// The command that opens `path` with `opener`, or `None` for a path that is not absolute.
pub(crate) fn command_for(opener: Opener, path: &Path) -> Option<Command> {
    if !absolute_for(opener, path) {
        return None;
    }
    let mut command = match opener {
        Opener::Explorer => Command::new("explorer.exe"),
        Opener::MacOpen => {
            let mut c = Command::new("open");
            c.arg("--");
            c
        }
        Opener::XdgOpen => Command::new("xdg-open"),
    };
    command.arg(OsStr::new(path.as_os_str()));
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    Some(command)
}

/// Open `path` with the system's program and leave it running on its own.
pub(crate) fn open_detached(path: &Path) -> std::io::Result<()> {
    let mut command =
        command_for(Opener::this_platform(), path).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "not an absolute path"))?;
    let mut child = command.spawn()?;
    // Reaped when it is done, so that no process is left behind it.
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
#[path = "file_launcher_tests.rs"]
mod file_launcher_tests;
