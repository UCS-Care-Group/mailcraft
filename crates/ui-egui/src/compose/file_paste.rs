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
/// that came with the paste (`text`; empty for a Ctrl+V the backend kept, which is Explorer's files
/// alone) only the files that text names, and none when it names anything else: a file manager
/// leaves beside the files it copies their paths, addresses or names, but a page can put a list of
/// `file://` addresses on the clipboard beside the text the owner copied from it, and that text is
/// then pasted as text, and no file of the owner's goes into the message.
pub(super) fn files(app: &MailApp, which: super::Which, text: &str) -> Option<Vec<String>> {
    if which != super::Which::Message {
        return None;
    }
    let board = app.services.clipboard.as_ref()?;
    let files: Vec<String> = board.files().iter().filter_map(|p| p.to_str().map(str::to_string)).collect();
    let named = named_by(text, &files)?;
    (!named.is_empty()).then_some(named)
}

/// Of `files`, those that `text` names: all of them for no text at all; else each line of it must
/// be the path of one of them, its `file://` address or its name, and the files named are the
/// ones that are attached. `None` when a line names no file.
fn named_by(text: &str, files: &[String]) -> Option<Vec<String>> {
    // (Slashes either way: a Windows path in an address has them forward.)
    let slashed = |text: &str| text.replace('\\', "/");
    let names = |file: &str, line: &str, decoded: &str| {
        let path = slashed(file);
        let name = path.rsplit('/').next().unwrap_or(&path);
        path == line || path == decoded || name == line || name == decoded
    };
    // (A paste of more lines than files can name is no list of them.)
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(files.len().saturating_add(1)).collect();
    if lines.is_empty() {
        return Some(files.to_vec());
    }
    if lines.len() > files.len() {
        return None;
    }
    let mut named = vec![false; files.len()];
    for line in lines {
        let (line, decoded) = (slashed(line), slashed(&decoded_address(line)));
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

/// `line` with a `file://` address read as the path it names: the scheme and a local host taken
/// off, `%20` and the like decoded. Any other line as it is.
fn decoded_address(line: &str) -> String {
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
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => decoded.get(1..).unwrap_or_default().to_string(),
        _ => decoded,
    }
}

/// Attach `paths`, as a pick of Attach File's would be: at most so many and so large, each in
/// turn, and said once how many went in, and which: so that files that came from the clipboard
/// are seen for what they are.
pub(super) fn attach(app: &mut MailApp, paths: &[String]) {
    if crate::files::attach_all(app, paths, None).is_err() {
        return;
    }
    let shown: Vec<&str> = paths.iter().take(3).map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p)).collect();
    if let Some((said, _)) = app.status_msg.clone() {
        let more = paths.len().saturating_sub(shown.len());
        let names = if more > 0 { format!("{}, …", shown.join(", ")) } else { shown.join(", ") };
        app.status(format!("{said} ({names})"));
    }
}

#[cfg(test)]
#[path = "file_paste_tests.rs"]
mod file_paste_tests;
