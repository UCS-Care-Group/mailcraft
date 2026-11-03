// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pointer while messages are dragged (#430): the system's own pointers only, chosen by what
//! is under it. Over a folder that takes the messages it is the ordinary arrow (the drag label
//! beside it and the folder's outline say "drop here"; winit draws neither `Grabbing` nor `Move`
//! as anything but the four-arrow cursor on Windows, and has no drop cursor there); over a folder
//! that refuses them, or anywhere that is no folder, it is the "not allowed" one.

use egui::{CursorIcon, Id as EguiId};
use mailcraft_model::{Id, Mailbox};

/// What the folder under the pointer wrote this frame: whether it takes the drag (kept by its
/// type, under the null id).
#[derive(Clone, Copy, Default)]
struct OverFolder(Option<bool>);

/// Of the dragged messages, those that are not in `folder` already: what a drop on it moves.
pub(crate) fn movable(mailbox: &Mailbox, dragged: &[Id], folder: Id) -> Vec<Id> {
    dragged.iter().copied().filter(|m| mailbox.message(*m).is_some_and(|m| m.folder != folder)).collect()
}

/// The pointer for a drag: `over_folder` is `Some(takes)` when a folder is under it.
pub(crate) fn drag_pointer(over_folder: Option<bool>) -> CursorIcon {
    match over_folder {
        Some(true) => CursorIcon::Default,
        _ => CursorIcon::NotAllowed,
    }
}

/// A folder row under the pointer says whether it takes the messages.
pub(crate) fn note_folder(ctx: &egui::Context, takes: bool) {
    ctx.data_mut(|d| d.insert_temp(EguiId::NULL, OverFolder(Some(takes))));
}

/// At the end of the frame: the pointer for a drag in progress.
pub(crate) fn apply(ctx: &egui::Context) {
    let over = ctx.data_mut(|d| d.remove_temp::<OverFolder>(EguiId::NULL).and_then(|o| o.0));
    if egui::DragAndDrop::has_payload_of_type::<crate::mail_view::MessageDrag>(ctx) {
        ctx.set_cursor_icon(drag_pointer(over));
    }
}
