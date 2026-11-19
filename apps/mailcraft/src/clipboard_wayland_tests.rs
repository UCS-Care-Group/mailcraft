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
