// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a file with the system's program for its kind (#126), and a folder in the system's
//! file manager (#460: File › Info › Open MailCraft's data folder), without a shell.
//!
//! The system's own opener is started as a program with the file's whole path as one argument:
//!
//! - **Windows:** `explorer.exe <path>`. Given the path of a file, Explorer opens it with the
//!   program registered for its kind, as a double-click does (the Windows shell's file
//!   association, with the file's zone honoured). No `cmd.exe`, so no `start` and no quoting
//!   rules of a command line; the path is one argument that Rust quotes for the program.
//! - **macOS:** `open -- <path>` (Launch Services); `--` ends the options.
//! - **Linux, FreeBSD and the like:** `xdg-open <path>` (the desktop's opener), or where it isn't
//!   installed (it comes with `xdg-utils`), `gio open -- <path>` or else `kde-open -- <path>`
//!   (#445).
//!
//! The path must be absolute, so it can never be read as an option (a Unix path starts with `/`,
//! a Windows one with a drive letter and `:\` or with `\\`), and nothing in it is looked at by a
//! shell. Anything else is refused.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use mailcraft_ui_egui::data_removal::FolderTrouble;

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

/// The commands that open `path` with `opener`, in the order they are tried; none for a path that
/// is not absolute.
pub(crate) fn commands_for(opener: Opener, path: &Path) -> Vec<Command> {
    if !absolute_for(opener, path) {
        return Vec::new();
    }
    // `xdg-open` refuses `--` as an unknown option, so it is given the path alone.
    let programs: Vec<(&str, &[&str])> = match opener {
        Opener::Explorer => vec![("explorer.exe", &[])],
        Opener::MacOpen => vec![("open", &["--"])],
        Opener::XdgOpen => vec![("xdg-open", &[]), ("gio", &["open", "--"]), ("kde-open", &["--"])],
    };
    programs
        .into_iter()
        .map(|(program, first)| {
            let mut command = Command::new(program);
            command.args(first);
            command.arg(OsStr::new(path.as_os_str()));
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            command
        })
        .collect()
}

/// Start the first of `commands` whose program is there: one that isn't found gives way to the
/// next, and any other failure is the answer.
fn first_started<T>(commands: Vec<Command>, mut spawn: impl FnMut(&mut Command) -> std::io::Result<T>) -> std::io::Result<T> {
    let mut last = std::io::Error::new(std::io::ErrorKind::InvalidInput, "not an absolute path");
    for mut command in commands {
        match spawn(&mut command) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last = e,
            started => return started,
        }
    }
    Err(last)
}

/// Open `path` with the system's program and leave it running on its own.
pub(crate) fn open_detached(path: &Path) -> std::io::Result<()> {
    let mut child = first_started(commands_for(Opener::this_platform(), path), Command::spawn)?;
    // Reaped when it is done, so that no process is left behind it.
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || child.wait());
    Ok(())
}

/// Show the folder `path` in the system's file manager (each opener opens a folder there). A path
/// that isn't absolute, or isn't a folder, is refused before anything is started.
pub(crate) fn open_folder(path: &Path) -> Result<(), FolderTrouble> {
    open_folder_with(path, open_detached)
}

/// [`open_folder`], with the opener given (the tests' stand-in).
fn open_folder_with(path: &Path, open: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<(), FolderTrouble> {
    if !absolute_for(Opener::this_platform(), path) || !path.is_dir() {
        return Err(FolderTrouble::NotThere);
    }
    open(path).map_err(|e| {
        log::warn!("couldn't open a folder in the file manager ({:?})", e.kind());
        FolderTrouble::NoFileManager
    })
}

#[cfg(test)]
#[path = "file_launcher_tests.rs"]
mod file_launcher_tests;
