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
//! `clipboard_mime`. All Wayland paste reads stay on that bounded reader. Copying can use
//! arboard or winit. When a rich copy has to write text through winit, its HTML is kept in
//! this process for a matching paste of our own selection (#1217, #1708).
//!
//! One clipboard is opened at the first copy or paste and kept: on X11 and Wayland what was
//! copied is served from this program for as long as it holds the clipboard.

use std::cell::RefCell;

use mailcraft_ui_egui::{RichClipboard, clipboard_picture_png as png};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use winit::platform::wayland::Clipboard as WaylandBoard;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::clipboard_mime::{HTML_MIMES, IMAGE_MIMES, MAX_HTML_BYTES, MAX_PICTURE_BYTES, TEXT_MIMES, decode_html, png_fits};

/// The most HTML one paste takes, in bytes (the engine's own limit).
const MAX_HTML: usize = 4 << 20;

/// The system's clipboard. `display` is the window's `wl_display` when it runs under Wayland.
pub fn board(display: Option<std::ptr::NonNull<std::ffi::c_void>>) -> Option<Box<dyn RichClipboard>> {
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(unused_mut))]
    let mut board = Board::default();
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        board.wayland_display = display.is_some();
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

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
#[derive(Default)]
struct FallbackHtml(RefCell<Option<(String, String)>>);

#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
impl FallbackHtml {
    fn remember(&self, text: &str, html: &str) {
        *self.0.borrow_mut() = Some((text.to_string(), html.to_string()));
    }

    fn matching(&self, text: &str) -> Option<String> {
        let cached = self.0.borrow().clone();
        match cached {
            Some((cached_text, html)) if cached_text == text => Some(html),
            Some(_) => {
                self.clear();
                None
            }
            None => None,
        }
    }

    fn clear(&self) {
        *self.0.borrow_mut() = None;
    }
}

#[derive(Default)]
struct Board {
    open: RefCell<Option<arboard::Clipboard>>,
    /// Winit's clipboard on the compositor's `wl_data_device`, under Wayland.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland: Option<WaylandBoard>,
    /// A Wayland window must not silently fall back to X11 when its reader is unavailable.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland_display: bool,
    /// HTML accompanying text written through winit, whose Wayland API only writes text.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fallback_html: FallbackHtml,
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

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write(&self, primary: Result<(), String>, text: &str) -> (Result<(), String>, bool) {
        let Some(wayland) = &self.wayland else { return (primary, false) };
        let fallback_used = primary.is_err() && !text.is_empty();
        (finish_wayland_write(primary, text, |text| wayland.store_text(text.to_owned()).map_err(|e| e.to_string())), fallback_used)
    }

    fn read_html(&self, _pasted_text: Option<&str>) -> Option<String> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.wayland_display {
            let wayland = self.wayland.as_ref()?;
            return match wayland.load_mime(&HTML_MIMES, MAX_HTML_BYTES) {
                Ok((_, bytes)) => {
                    self.fallback_html.clear();
                    html_of(&decode_html(&bytes))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Our own winit text has no external offer. Use the text already read by
                    // this paste to match its cached HTML, without requesting text again.
                    if wayland.offers(&TEXT_MIMES) {
                        self.fallback_html.clear();
                        return None;
                    }
                    self.fallback_html.matching(_pasted_text?).and_then(|html| html_of(&html))
                }
                Err(e) => {
                    log::debug!("the Wayland clipboard gave no HTML: {e}");
                    None
                }
            };
        }
        html_of(&self.with(|b| b.get().html()).ok()?)
    }
}

impl RichClipboard for Board {
    fn put(&self, html: &str, text: &str) -> Result<(), String> {
        let primary = self.with(|b| b.set().html(html, Some(text)));
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let (r, fallback_used) = self.write(primary, text);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let r = primary;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if r.is_ok() && self.wayland.is_some() {
            if fallback_used {
                self.fallback_html.remember(text, html);
            } else {
                self.fallback_html.clear();
            }
        }
        if let Err(e) = &r {
            log::warn!("the clipboard took no HTML or text: {e}");
        }
        r
    }

    fn put_text(&self, text: &str) -> Result<(), String> {
        let primary = self.with(|b| b.set_text(text.to_owned()));
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let (r, _) = self.write(primary, text);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let r = primary;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if r.is_ok() {
            self.fallback_html.clear();
        }
        if let Err(e) = &r {
            log::warn!("the clipboard took no text: {e}");
        }
        r
    }

    fn html(&self) -> Option<String> {
        self.read_html(None)
    }

    fn html_for_text(&self, text: &str) -> Option<String> {
        self.read_html(Some(text))
    }

    fn text(&self) -> Option<String> {
        self.paste_text().ok().flatten()
    }

    fn paste_text(&self) -> std::io::Result<Option<String>> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.wayland_display {
            let Some(wayland) = &self.wayland else { return Ok(None) };
            return wayland_text(wayland.load_text(), |mimes| wayland.offers(mimes));
        }
        // All of it, as Ctrl+V has it: what is too much for a message is refused where the paste
        // is made (`edit.paste`), the same for both.
        Ok(self.with(|b| b.get_text()).ok())
    }

    fn image(&self) -> Option<Vec<u8>> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.wayland_display {
            return match self.wayland.as_ref()?.load_mime(&IMAGE_MIMES, MAX_PICTURE_BYTES) {
                Ok((_, bytes)) => png_fits(&bytes).then_some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                // The owner didn't answer in time: asking it again through `arboard` would wait as long.
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => None,
                Err(e) => {
                    log::debug!("the Wayland clipboard gave no picture: {e}");
                    None
                }
            };
        }
        let image = self.with(|b| b.get_image()).ok()?;
        png(image.width, image.height, &image.bytes)
    }

    fn has_image(&self) -> bool {
        // Winit says from the types on offer, without a read; when it can't, read as before.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.wayland_display {
            return self.wayland.as_ref().is_some_and(|w| w.offers(&IMAGE_MIMES));
        }
        self.image().is_some()
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn wayland_text(read: std::io::Result<String>, offers: impl FnOnce(&[&str]) -> bool) -> std::io::Result<Option<String>> {
    match read {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // Winit also reports a picture-only selection as InvalidData. Query its announced
        // formats without reading again: oversized text has the same error kind and must stay
        // a failed transfer, even when a picture is offered beside it.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData && !offers(&TEXT_MIMES) => Ok(None),
        // Retrying a timed-out or broken transfer on the window thread would wait on
        // the same unresponsive owner again, through a different clipboard reader.
        Err(e) => Err(e),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
#[path = "clipboard_wayland_tests.rs"]
mod wayland_tests;

fn finish_wayland_write(primary: Result<(), String>, text: &str, fallback: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
    match primary {
        Ok(()) => Ok(()),
        Err(primary) if !text.is_empty() => fallback(text).map_err(|fallback| format!("{primary}; Wayland text fallback failed: {fallback}")),
        Err(primary) => Err(primary),
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
