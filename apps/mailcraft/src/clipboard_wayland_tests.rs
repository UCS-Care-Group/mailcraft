// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn a_wayland_window_without_its_rich_reader_does_not_open_an_x11_clipboard() {
    let board = Board { wayland_display: true, ..Default::default() };
    assert!(board.paste_text().unwrap().is_none());
    assert!(board.text().is_none());
    assert!(board.html().is_none());
    assert!(board.image().is_none());
    assert!(!board.has_image());
    assert!(board.open.borrow().is_none());
}

struct Selection {
    mimes: &'static [&'static str],
    error: std::io::ErrorKind,
}

impl RichClipboard for Selection {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn html(&self) -> Option<String> {
        None
    }
    fn paste_text(&self) -> std::io::Result<Option<String>> {
        // The pinned reader uses InvalidData both for no text format and for oversized text.
        wayland_text(Err(self.error.into()), |mimes| mimes.iter().any(|mime| self.mimes.contains(mime)))
    }
    fn image(&self) -> Option<Vec<u8>> {
        self.mimes.contains(&"image/png").then(|| png(1, 1, &[255; 4])).flatten()
    }
}

fn paste_key() -> egui::Event {
    egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }
}

#[test]
fn a_picture_only_selection_reaches_picture_paste() {
    let board = Selection { mimes: &["image/png"], error: std::io::ErrorKind::InvalidData };
    assert!(board.paste_text().unwrap().is_none());
    assert!(board.image().is_some());
    let mut events = vec![paste_key()];
    assert!(!mailcraft_ui_egui::paste_keys::translate(&mut events, Some(&board)));
    assert!(matches!(events.as_slice(), [egui::Event::Paste(text)] if text.is_empty()));
}

#[test]
fn oversized_text_is_not_classified_as_missing_even_with_a_picture() {
    for mime in ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"] {
        let result = wayland_text(Err(std::io::ErrorKind::InvalidData.into()), |mimes| mimes.contains(&mime));
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData, "{mime}");
    }
    let board = Selection { mimes: &["text/plain", "image/png"], error: std::io::ErrorKind::InvalidData };
    assert_eq!(board.paste_text().unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    let mut events = vec![paste_key()];
    assert!(!mailcraft_ui_egui::paste_keys::translate(&mut events, Some(&board)));
    assert!(events.is_empty());
}

#[test]
fn a_failed_transfer_is_not_classified_as_missing_even_without_text_offers() {
    for error in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::BrokenPipe, std::io::ErrorKind::NotConnected] {
        let board = Selection { mimes: &["image/png"], error };
        assert_eq!(board.paste_text().unwrap_err().kind(), error);
    }
}

#[test]
fn text_already_read_is_kept_even_without_an_external_offer() {
    for text in ["", "our own copied text"] {
        let result = wayland_text(Ok(text.into()), |_| panic!("a successful read needs no format query"));
        assert_eq!(result.unwrap().as_deref(), Some(text));
    }
}
