// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ctrl+X, Ctrl+C and Ctrl+V under Wayland, where the window layer can't see a picture (#1217).
//!
//! `egui-winit` turns the clipboard shortcuts into `Event::Cut`, `Copy` and `Paste` itself, and
//! for a paste it reads the text on the clipboard; only when there is no text and its own
//! clipboard (`arboard`) opened does it look for a picture. In the Flatpak under Wayland that
//! clipboard can't open (no data-control protocol, no X11), so a picture copied alone (a
//! screenshot, "Copy Image") produced no event at all and nothing was pasted.
//!
//! So under Wayland the window layer's shortcuts are off (`NativeOptions::clipboard_shortcuts`)
//! and the keys come through as ordinary key events, which [`translate`] turns into the same three
//! events (`Services::clipboard_keys`; the main window does it in `raw_input_hook`, which eframe
//! doesn't call for a window of its own that is shown immediately, so those do it in
//! `windows`): a paste reads the text from the system clipboard (winit's data device) and, when there
//! is none but there is a picture, sends an empty paste, which a message being written answers by
//! asking the clipboard for the picture (`RichClipboard::image`). Nothing else changes.

use egui::{Event, Key};

use crate::RichClipboard;

/// Whether this session is Wayland, as winit decides it (`WAYLAND_SOCKET` or `WAYLAND_DISPLAY`
/// set and not empty).
pub fn wayland_session(wayland_socket: Option<&str>, wayland_display: Option<&str>) -> bool {
    [wayland_socket, wayland_display].iter().any(|v| v.is_some_and(|v| !v.is_empty()))
}

enum Shortcut {
    Cut,
    Copy,
    Paste,
}

fn shortcut(key: Key, modifiers: egui::Modifiers) -> Option<Shortcut> {
    match key {
        Key::Cut => Some(Shortcut::Cut),
        Key::Copy => Some(Shortcut::Copy),
        Key::Paste => Some(Shortcut::Paste),
        Key::X if modifiers.command_only() => Some(Shortcut::Cut),
        Key::C if modifiers.command_only() => Some(Shortcut::Copy),
        Key::V if modifiers.command_only() => Some(Shortcut::Paste),
        _ => None,
    }
}

/// Replace the key presses that are clipboard shortcuts by their events.
pub fn translate(events: &mut Vec<Event>, board: Option<&dyn RichClipboard>) {
    if !events.iter().any(|e| matches!(e, Event::Key { pressed: true, key, modifiers, .. } if shortcut(*key, *modifiers).is_some())) {
        return;
    }
    let mut out = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let Event::Key { pressed: true, key, modifiers, .. } = &event else {
            out.push(event);
            continue;
        };
        match shortcut(*key, *modifiers) {
            Some(Shortcut::Cut) => out.push(Event::Cut),
            Some(Shortcut::Copy) => out.push(Event::Copy),
            Some(Shortcut::Paste) => {
                let text = board.and_then(|b| b.text()).map(|t| t.replace("\r\n", "\n")).unwrap_or_default();
                if !text.is_empty() {
                    out.push(Event::Paste(text));
                } else if board.is_some_and(|b| b.has_image()) {
                    out.push(Event::Paste(String::new()));
                }
            }
            None => out.push(event),
        }
    }
    *events = out;
}

#[cfg(test)]
#[path = "paste_keys_tests.rs"]
mod paste_keys_tests;
