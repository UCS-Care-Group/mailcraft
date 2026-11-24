// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Interface direction: where a window reads from the right (Arabic), rows start at the right,
//! the panes that stand at the left stand at the right, and text that begins a line begins at
//! the right.
//!
//! This is the direction of the *interface*. The direction of a message's own text follows the
//! message (`dir`, or the first strong letter), never the window's language: an English message
//! in an Arabic window reads from the left, an Arabic one in an English window from the right.
//! The reading pane lays out its content by that rule and does not use these helpers.

use egui::{Align, InnerResponse, Layout, Panel, TextBuffer, TextEdit, Ui, vec2};

/// Whether the interface reads from the right.
pub fn rtl() -> bool {
    crate::i18n::rtl()
}

/// The side an item at the start of a line stands on.
pub fn start_align() -> Align {
    if rtl() { Align::Max } else { Align::Min }
}

/// A panel at the start of the window: the left, or the right in a right-to-left window.
pub fn start_panel(id: impl egui::AsIdSalt) -> Panel {
    if rtl() { Panel::right(id) } else { Panel::left(id) }
}

/// A panel at the end of the window: the right, or the left in a right-to-left window.
pub fn end_panel(id: impl egui::AsIdSalt) -> Panel {
    if rtl() { Panel::left(id) } else { Panel::right(id) }
}

/// A row whose items follow one another from the start: `Ui::left_to_right` or, mirrored, its opposite.
pub fn start_to_end(cross: Align) -> Layout {
    if rtl() { Layout::right_to_left(cross) } else { Layout::left_to_right(cross) }
}

/// A row whose items are put at the end of the line, the first one last: `Layout::right_to_left`
/// or, mirrored, its opposite.
pub fn end_to_start(cross: Align) -> Layout {
    if rtl() { Layout::left_to_right(cross) } else { Layout::right_to_left(cross) }
}

/// A column whose widgets stand at the start of the line.
pub fn column() -> Layout {
    Layout::top_down(start_align())
}

/// `Ui::horizontal`, from the right in a right-to-left window.
pub fn horizontal<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    if !rtl() {
        return ui.horizontal(add);
    }
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), add)
}

/// `Ui::horizontal_wrapped`, from the right in a right-to-left window.
pub fn horizontal_wrapped<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    if !rtl() {
        return ui.horizontal_wrapped(add);
    }
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center).with_main_wrap(true), add)
}

/// A one-line text box whose text starts at the start of the line; the caret and the selection
/// follow the text as they do in any left-to-right box.
pub fn singleline(text: &mut dyn TextBuffer) -> TextEdit<'_> {
    TextEdit::singleline(text).align(egui::Align2([start_align(), Align::Center]))
}

/// A text box of several lines whose text starts at the start of the line.
pub fn multiline(text: &mut dyn TextBuffer) -> TextEdit<'_> {
    TextEdit::multiline(text).align(egui::Align2([start_align(), Align::Min]))
}

#[cfg(test)]
#[path = "rtl_tests.rs"]
mod rtl_tests;
