// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The system's clipboard with HTML on it, for the body of a message being written
//! ([`mailcraft_ui_egui::Services::clipboard`]): a copy puts the selection there as HTML of the
//! set and as text, and a paste takes HTML when the clipboard has some (the engine reduces it to
//! the set: nothing in it runs or is fetched). egui's own clipboard carries text only. Paste in a
//! text's menu, for which no key press has the system hand over its text, reads the text here.
//!
//! Through `arboard`, which egui's window layer already uses: the Windows clipboard ("HTML
//! Format"), the macOS pasteboard (`public.html`), and on Linux and FreeBSD the Wayland
//! compositor's data-control protocol where it has one (KDE Plasma, wlroots compositors), else
//! the X11 clipboard (an X11 session, or XWayland, which hands it on to Wayland programs).
//!
//! Under Wayland in the Flatpak neither of those is there (no data-control protocol, no X11), so
//! HTML, pictures and text are read through winit's own `wl_data_device`, the one the window
//! already uses for its text and file drops: the types asked for and the caps are in
//! `clipboard_mime`. `arboard` is the way back when that has nothing to say (#1217).
//!
//! One clipboard is opened at the first copy or paste and kept: on X11 and Wayland what was
//! copied is served from this program for as long as it holds the clipboard. Wayland text writes
//! use the window's `wl_data_device`, which needs no compositor-specific clipboard extension.

use std::cell::RefCell;

use mailcraft_ui_egui::{RichClipboard, clipboard_picture_png as png};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use winit::platform::wayland::Clipboard as WaylandBoard;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::clipboard_mime::{HTML_MIMES, IMAGE_MIMES, MAX_HTML_BYTES, MAX_PICTURE_BYTES, decode_html, png_fits};

/// The most HTML one paste takes, in bytes (the engine's own limit).
const MAX_HTML: usize = 4 << 20;

/// The system's clipboard. `display` is the window's `wl_display` when it runs under Wayland.
pub fn board(display: Option<std::ptr::NonNull<std::ffi::c_void>>) -> Option<Box<dyn RichClipboard>> {
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(unused_mut))]
    let mut board = Board::default();
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        board.wayland = display.and_then(WaylandBoard::for_display);
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = display;
    Some(Box::new(board))
}

/// The window's `wl_display`, when it runs under Wayland.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn wayland_display(window: &impl winit::raw_window_handle::HasDisplayHandle) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
    match window.display_handle().ok()?.as_raw() {
        winit::raw_window_handle::RawDisplayHandle::Wayland(handle) => Some(handle.display),
        _ => None,
    }
}

#[derive(Default)]
struct Board {
    open: RefCell<Option<arboard::Clipboard>>,
    /// Winit's clipboard on the compositor's `wl_data_device`, under Wayland.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland: Option<WaylandBoard>,
}

impl Board {
    fn with<R>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error>) -> Result<R, String> {
        let mut open = self.open.try_borrow_mut().map_err(|_| "the clipboard is busy".to_string())?;
        if open.is_none() {
            *open = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        match open.as_mut() {
            Some(board) => f(board).map_err(|e| e.to_string()),
            None => Err("no clipboard".into()),
        }
    }
}

/// Handle egui's text-copy output on Wayland through winit's active seat and clipboard device.
pub fn copy_text_output(ctx: &egui::Context, board: Option<&dyn RichClipboard>) {
    let Some(board) = board else { return };
    route_copy_text_output(ctx, |text| board.put_text(text));
}

fn route_copy_text_output(
    ctx: &egui::Context,
    mut put_text: impl FnMut(&str) -> Result<(), String>,
) {
    ctx.output_mut(|output| {
        output.commands.retain(|command| match command {
            egui::OutputCommand::CopyText(text) => match put_text(text) {
                Ok(()) => false,
                Err(error) => {
                    log::warn!("the clipboard took no text; leaving the copy to the window backend: {error}");
                    true
                }
            },
            _ => true,
        });
    });
}

impl RichClipboard for Board {
    fn put(&self, html: &str, text: &str) -> Result<(), String> {
        match self.with(|b| b.set().html(html, Some(text))) {
            Ok(()) => Ok(()),
            Err(error) => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                if let Some(wayland) = &self.wayland {
                    return match wayland.store_text(text.to_owned()) {
                        Ok(()) => {
                            log::warn!("the clipboard took no HTML; only its text was stored on Wayland: {error}");
                            Ok(())
                        }
                        Err(wayland_error) => {
                            let error = format!("{error}; Wayland: {wayland_error}");
                            log::warn!("the clipboard took no HTML or text: {error}");
                            Err(error)
                        }
                    };
                }
                log::warn!("the clipboard took no HTML: {error}");
                Err(error)
            }
        }
    }

    fn put_text(&self, text: &str) -> Result<(), String> {
        match self.with(|b| b.set_text(text.to_owned())) {
            Ok(()) => Ok(()),
            Err(arboard_error) => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                if let Some(wayland) = &self.wayland {
                    return wayland
                        .store_text(text.to_owned())
                        .map_err(|wayland_error| {
                            format!("{arboard_error}; Wayland: {wayland_error}")
                        });
                }
                Err(arboard_error)
            }
        }
    }

    fn html(&self) -> Option<String> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(wayland) = &self.wayland {
            match wayland.load_mime(&HTML_MIMES, MAX_HTML_BYTES) {
                Ok((_, bytes)) => return html_of(&decode_html(&bytes)),
                // Nothing of that kind offered: `arboard` may still have some.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // The owner didn't answer in time: asking it again through `arboard` would wait as long.
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return None,
                Err(e) => log::debug!("the Wayland clipboard gave no HTML: {e}"),
            }
        }
        let html = self.with(|b| b.get().html()).ok()?;
        html_of(&html)
    }

    fn text(&self) -> Option<String> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(wayland) = &self.wayland {
            match wayland.load_text() {
                Ok(text) => return Some(text),
                // The owner didn't answer in time: asking it again through `arboard` would wait as long.
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return None,
                Err(_) => {}
            }
        }
        // All of it, as Ctrl+V has it: what is too much for a message is refused where the paste
        // is made (`edit.paste`), the same for both.
        self.with(|b| b.get_text()).ok()
    }

    fn image(&self) -> Option<Vec<u8>> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(wayland) = &self.wayland {
            match wayland.load_mime(&IMAGE_MIMES, MAX_PICTURE_BYTES) {
                Ok((_, bytes)) => return png_fits(&bytes).then_some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // The owner didn't answer in time: asking it again through `arboard` would wait as long.
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return None,
                Err(e) => log::debug!("the Wayland clipboard gave no picture: {e}"),
            }
        }
        let image = self.with(|b| b.get_image()).ok()?;
        png(image.width, image.height, &image.bytes)
    }

    fn has_image(&self) -> bool {
        // Winit says from the types on offer, without a read; when it can't, read as before.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(wayland) = &self.wayland
            && wayland.offers(&IMAGE_MIMES)
        {
            return true;
        }
        self.image().is_some()
    }
}

/// The HTML to paste out of what the clipboard held: none when it comes to nothing or to more
/// than the engine takes.
fn html_of(html: &str) -> Option<String> {
    let html = fragment(html);
    (!html.trim().is_empty() && html.len() <= MAX_HTML).then(|| html.to_string())
}

/// The HTML itself out of what a clipboard hands over. Windows' "HTML Format" can come with its
/// header (`Version:0.9 StartHTML:… <html>… <!--StartFragment-->…<!--EndFragment-->…`): then
/// what was copied is the fragment between the markers, else everything from the first tag.
fn fragment(html: &str) -> &str {
    if !html.starts_with("Version:") {
        return html;
    }
    const START: &str = "<!--StartFragment-->";
    const END: &str = "<!--EndFragment-->";
    if let Some(from) = html.find(START).map(|i| i + START.len())
        && let Some(to) = html.get(from..).and_then(|rest| rest.find(END)).map(|i| from + i)
    {
        return html.get(from..to).unwrap_or_default();
    }
    html.find('<').and_then(|i| html.get(i..)).unwrap_or_default()
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod clipboard_tests;
