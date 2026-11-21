// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files copied in a file manager, pasted into the message being written: attached (#807), as
//! files picked with Attach File are, and as Outlook does with them.
//!
//! The system's backend for egui (egui-winit) reads only text and a picture for Ctrl+V. A file
//! manager on Linux or macOS puts the files' names beside them as text, so Ctrl+V comes as a paste
//! of that text, and the body asks the clipboard for files first ([`files`]). Explorer puts the
//! files alone on the clipboard: then the backend has nothing to hand over and keeps the key
//! press to itself, and only the V's release comes through. [`Keys`] tells that release from a V
//! typed: no press of it was seen, the command key (Ctrl, Cmd) was down a moment ago, and no
//! paste came of it.
//!
//! Only for the owner's own keys and menus: the control channel's input never attaches what the
//! owner copied (an agent could otherwise take any file the owner copied into a message). It has
//! no paste of its own to make: it sends keys (a press with its release), text and the pointer,
//! and a text's menu refuses its Paste. A press it sent is seen, so its V is never a lost Ctrl+V;
//! and a lost one is not looked for in a pass it made (`MailApp::driven`). The macOS menu bar's
//! Paste (Cmd+V there) is the owner's, and comes as a paste.

use std::borrow::Cow;

use egui::{Event, Key};

use crate::MailApp;

/// How long after the command key was last seen down a V let go of is still its Ctrl+V, in seconds.
const COMMAND_GRACE: f64 = 1.0;

/// How long after a paste arrived a V let go of is that paste's, in seconds.
const PASTE_GRACE: f64 = 2.0;

/// What the body remembers of the keys between passes, to find a Ctrl+V the backend kept.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Keys {
    /// A press of V was seen and not yet its release.
    v_down: bool,
    /// When a paste last arrived (of text or a picture).
    pasted_at: Option<f64>,
    /// When the command key was last seen down.
    command_at: Option<f64>,
}

impl Keys {
    /// One pass's input while the body has the keyboard, at `now` (seconds), the command key
    /// down or not as the pass began. Whether a Ctrl+V came in it that nothing else was told of.
    pub(super) fn lost_paste(&mut self, now: f64, command: bool, events: &[Event]) -> bool {
        if command {
            self.command_at = Some(now);
        }
        let mut lost = false;
        for event in events {
            match event {
                Event::Paste(_) | Event::PasteImage(_) => self.pasted_at = Some(now),
                Event::Key { key: Key::V, pressed: true, .. } => self.v_down = true,
                Event::Key { key: Key::V, pressed: false, modifiers, .. } => {
                    let within = |at: Option<f64>, grace: f64| at.is_some_and(|at| now - at >= 0.0 && now - at <= grace);
                    let commanded = modifiers.command || within(self.command_at, COMMAND_GRACE);
                    lost |= !self.v_down && commanded && !within(self.pasted_at, PASTE_GRACE);
                    self.v_down = false;
                }
                _ => {}
            }
        }
        lost
    }
}

/// The files on the clipboard that a paste into the message being written attaches: none for a
/// signature, without a clipboard that has files, or when what is on it is no files. With the text
/// that came with the paste (`text`) only the files that text names, and none when it names
/// anything else: a file manager leaves beside the files it copies their paths, addresses or
/// names, but a page can put a list of `file://` addresses on the clipboard beside the text the
/// owner copied from it, and that text is then pasted as text, and no file of the owner's goes
/// into the message.
///
/// With no text at all (a Ctrl+V the backend kept) that is Explorer's files alone, on Windows.
/// Elsewhere files alone on the clipboard are not attached: a page can't be told from a file
/// manager by what is on the clipboard (a browser may put a page's `text/uri-list` there as that
/// real type, listing paths it guessed), and the file managers there put the names beside the
/// files as text. A bare file name (`id_rsa`) names a file only on macOS, where Finder leaves
/// nothing else: a page could set that text beside a list of guessed paths and get every listed
/// file of that name attached with only the name showing. Elsewhere the text must be the path or
/// the `file://` address of each file. Outside Windows the owner must confirm the full paths
/// before the app checks or reads any file. On Windows a path of the network (`\\host\share\…`)
/// is left out: looking at it would contact that host.
pub(super) fn files(app: &MailApp, which: super::Which, text: &str) -> Option<Vec<String>> {
    files_on(app, which, text, Os::here())
}

/// The systems that differ in what a paste of files may be told by (see [`files`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Os {
    Windows,
    Mac,
    /// Linux and every other system.
    Other,
}

impl Os {
    /// The system this was built for. The only place that looks at the target: the rest takes
    /// an `Os`, so its tests run on every system in CI.
    pub(super) fn here() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }

    /// Whether files alone on the clipboard, with no text, are attached (Explorer's).
    fn takes_files_alone(self) -> bool {
        self == Os::Windows
    }

    /// Whether a bare file name in the pasted text names a file (Finder's).
    fn takes_bare_names(self) -> bool {
        self == Os::Mac
    }
}

/// [`files`], as it is on `os`.
fn files_on(app: &MailApp, which: super::Which, text: &str, os: Os) -> Option<Vec<String>> {
    if which != super::Which::Message {
        return None;
    }
    let board = app.services.clipboard.as_ref()?;
    let files: Vec<String> =
        board.files().iter().filter_map(|p| p.to_str()).filter(|p| !(os == Os::Windows && on_the_network(p))).map(str::to_string).collect();
    let named = named_by(text, &files, os)?;
    (!named.is_empty()).then_some(named)
}

/// Whether a Windows path is of the network (`\\host\share`, or a device's `\\.\`, `\\?\`):
/// asking about it for its size would contact the host.
fn on_the_network(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

/// Of `files`, those that `text` names: all of them for no text at all where `os` takes files
/// alone (Explorer's on Windows), else none; else each line of it must be the path of one of
/// them, its `file://` address or, where `os` takes bare names (Finder's on macOS), its name, and
/// the files named are the ones that are attached. `None` when a line names no file.
fn named_by(text: &str, files: &[String], os: Os) -> Option<Vec<String>> {
    let names = |file: &str, line: &str, decoded: &str| {
        let path = clipboard_path(file, os);
        let matches = |name: &str| name == line || name == decoded;
        matches(path.as_ref()) || (os.takes_bare_names() && file.rsplit('/').next().is_some_and(matches))
    };
    // (A paste of more lines than files can name is no list of them.)
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(files.len().saturating_add(1)).collect();
    if lines.is_empty() {
        return os.takes_files_alone().then(|| files.to_vec());
    }
    if lines.len() > files.len() {
        return None;
    }
    let mut named = vec![false; files.len()];
    for line in lines {
        let decoded = decoded_address(line, os);
        let (line, decoded) = (clipboard_path(line, os), clipboard_path(&decoded, os));
        // (Every file that goes by the name: two files of one name in different folders, both.)
        let mut found = false;
        for (i, f) in files.iter().enumerate() {
            if names(f, &line, &decoded) {
                (named[i], found) = (true, true);
            }
        }
        if !found {
            return None;
        }
    }
    Some(files.iter().zip(named).filter(|(_, named)| *named).map(|(f, _)| f.clone()).collect())
}

/// Windows uses either slash as a separator; elsewhere a backslash is part of the name.
fn clipboard_path(path: &str, os: Os) -> Cow<'_, str> {
    if os == Os::Windows && path.contains('\\') { Cow::Owned(path.replace('\\', "/")) } else { Cow::Borrowed(path) }
}

/// `line` with a `file://` address read as the path it names: the scheme and a local host taken
/// off, `%20` and the like decoded. Any other line as it is.
fn decoded_address(line: &str, os: Os) -> String {
    let Some(rest) = line.strip_prefix("file://") else { return line.to_string() };
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while let Some(&b) = raw.get(i) {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b, raw.get(i + 1).copied().and_then(hex), raw.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                bytes.push((hi * 16 + lo) as u8);
                i += 3;
            }
            _ => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    let decoded = String::from_utf8_lossy(&bytes).into_owned();
    // A Windows path in an address: `/C:/Users/…`.
    match decoded.as_bytes() {
        [b'/', drive, b':', ..] if os == Os::Windows && drive.is_ascii_alphabetic() => decoded.get(1..).unwrap_or_default().to_string(),
        _ => decoded,
    }
}

/// Windows supplies its own file clipboard format. Other systems require an owner's click
/// before inspecting a path, since the text and file list can come from the same web page.
pub(super) fn offer(app: &mut MailApp, paths: Vec<String>, os: Os) {
    let Some(compose) = app.session.compose_id() else { return };
    if app.driven {
        return;
    }
    if os == Os::Windows {
        attach(app, compose, &paths);
    } else if app.dialog.is_none() {
        app.set_dialog(Some(crate::dialogs::Dialog::PasteAttachments { compose, paths }));
    }
}

/// Attach the confirmed paths to the named message. Report the names that actually went in.
pub(crate) fn attach(app: &mut MailApp, compose: u64, paths: &[String]) {
    let Some(before) = app.session.composing_by_id(compose).map(|draft| draft.attachments.len()) else { return };
    if app.driven || crate::files::attach_all(app, paths, Some(compose)).is_err() {
        return;
    }
    let Some(draft) = app.session.composing_by_id(compose) else { return };
    let shown: Vec<&str> = draft.attachments.iter().skip(before).take(3).map(|a| a.name.as_str()).collect();
    if let Some((said, _)) = app.status_msg.as_ref()
        && !shown.is_empty()
    {
        let more = draft.attachments.len().saturating_sub(before) > shown.len();
        let names = shown.join(", ");
        let more = if more { ", …" } else { "" };
        app.status(format!("{said} ({names}{more})"));
    }
}

#[cfg(test)]
#[path = "file_paste_tests.rs"]
mod file_paste_tests;
