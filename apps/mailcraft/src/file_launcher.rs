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
//! - **Linux, FreeBSD and the like:** a file with `gio open -- <path>`, or where it isn't
//!   installed, `kde-open -- <path>` (#445), once the same desktop has said what kind it takes the
//!   file for (`gio info`, `kmimetypefinder`) and that is a kind of the file's name
//!   (`mailcraft_engine::opening::KNOWN`). The desktop types a file by what is in it when it
//!   doesn't know its name, or knows two kinds for it, and the sender chose what is in it; so a
//!   file it takes for anything else is not opened. Never `xdg-open` for a file: on a desktop it
//!   doesn't recognise it always goes by what is in the file. A folder with `xdg-open <path>`
//!   (the desktop's opener), or else `gio` or `kde-open`.
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

/// What is opened: a file, with the program for its kind, or a folder, in the file manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    File,
    Folder,
}

/// The commands that open `path` with `opener`, in the order they are tried; none for a path that
/// is not absolute.
pub(crate) fn commands_for(opener: Opener, target: Target, path: &Path) -> Vec<Command> {
    if !absolute_for(opener, path) {
        return Vec::new();
    }
    // `xdg-open` refuses `--` as an unknown option, so it is given the path alone.
    let programs: Vec<(&str, &[&str])> = match (opener, target) {
        (Opener::Explorer, _) => vec![("explorer.exe", &[])],
        (Opener::MacOpen, _) => vec![("open", &["--"])],
        (Opener::XdgOpen, Target::File) => TYPED.iter().map(|(_, _, opener, args)| (*opener, *args)).collect(),
        (Opener::XdgOpen, Target::Folder) => vec![("xdg-open", &[]), ("gio", &["open", "--"]), ("kde-open", &["--"])],
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

/// Linux and the BSDs: each opener of a file, after the program of the same desktop that says
/// what kind the opener will take the file for (program, its arguments, opener, its arguments).
const TYPED: &[(&str, &[&str], &str, &[&str])] = &[
    ("gio", &["info", "--attributes=standard::content-type", "--"], "gio", &["open", "--"]),
    ("kmimetypefinder", &["--"], "kde-open", &["--"]),
    ("kmimetypefinder5", &["--"], "kde-open5", &["--"]),
];

/// The commands that say what kind each of [`TYPED`]'s openers takes `path` for; none for a path
/// that is not absolute.
fn typers_for(path: &Path) -> Vec<Command> {
    if !absolute_for(Opener::XdgOpen, path) {
        return Vec::new();
    }
    TYPED
        .iter()
        .map(|(program, args, _, _)| {
            let mut command = Command::new(program);
            command.args(*args).arg(OsStr::new(path.as_os_str()));
            command.stdin(Stdio::null()).stderr(Stdio::null());
            command
        })
        .collect()
}

/// The kind a typer said: its last line, without `gio info`'s name for the attribute.
fn said_kind(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default();
    last.strip_prefix("standard::content-type:").unwrap_or(last).trim().to_string()
}

/// Open `path` with the first of [`TYPED`]'s openers that is installed, once its desktop has said
/// it takes the file for one of `kinds` (`output` runs a typer and gives what it wrote, `spawn`
/// starts the opener: the tests' stand-ins). A file it takes for anything else is refused
/// (`InvalidData`) and nothing is started.
fn open_typed<T>(
    path: &Path,
    kinds: &[&str],
    mut output: impl FnMut(&mut Command) -> std::io::Result<Vec<u8>>,
    mut spawn: impl FnMut(&mut Command) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut last = std::io::Error::new(std::io::ErrorKind::InvalidInput, "not an absolute path");
    for (mut typer, mut opener) in typers_for(path).into_iter().zip(commands_for(Opener::XdgOpen, Target::File, path)) {
        let said = match output(&mut typer) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                last = e;
                continue;
            }
            said => said?,
        };
        let kind = said_kind(&said);
        if !kinds.iter().any(|k| k.eq_ignore_ascii_case(&kind)) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the desktop takes the file for another kind"));
        }
        match spawn(&mut opener) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last = e,
            started => return started,
        }
    }
    Err(last)
}

/// What a typer wrote, when it ran and said yes.
fn typer_output(command: &mut Command) -> std::io::Result<Vec<u8>> {
    // lean: waits for the typer (a local program, quick); a deadline if one is ever seen to hang.
    let out = command.output()?;
    if out.status.success() { Ok(out.stdout) } else { Err(std::io::Error::other("the desktop couldn't say what kind the file is")) }
}

/// Open the file at `path` with the system's program and leave it running on its own. On Linux
/// and the BSDs only when the desktop takes it for a kind of its name ([`open_typed`]); a file
/// it takes for another kind is refused with `InvalidData`.
pub(crate) fn open_detached(path: &Path) -> std::io::Result<()> {
    if Opener::this_platform() != Opener::XdgOpen {
        return start_detached(Target::File, path);
    }
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    let kinds = mailcraft_engine::opening::desktop_types(&mailcraft_engine::opening::extension(name));
    open_typed(path, kinds, typer_output, Command::spawn).map(reap)
}

/// Open `path` as `target` and leave the opener running on its own.
fn start_detached(target: Target, path: &Path) -> std::io::Result<()> {
    first_started(commands_for(Opener::this_platform(), target, path), Command::spawn).map(reap)
}

/// The opener reaped when it is done, so that no process is left behind it.
fn reap(mut child: std::process::Child) {
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || child.wait());
}

/// Show the folder `path` in the system's file manager (each opener opens a folder there). A path
/// that isn't absolute, or isn't a folder, is refused before anything is started.
pub(crate) fn open_folder(path: &Path) -> Result<(), FolderTrouble> {
    open_folder_with(path, |p| start_detached(Target::Folder, p))
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
