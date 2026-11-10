// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pointer during a message drag (#430).

use egui::CursorIcon;

use crate::drag_pointer::{drag_pointer, movable};

#[test]
fn a_folder_that_takes_the_drag_shows_the_arrow_and_anything_else_not_allowed() {
    assert_eq!(drag_pointer(Some(true)), CursorIcon::Default);
    assert_eq!(drag_pointer(Some(false)), CursorIcon::NotAllowed);
    assert_eq!(drag_pointer(None), CursorIcon::NotAllowed);
}

#[test]
fn the_drag_pointer_is_never_the_four_arrow_move_cursor() {
    for over in [None, Some(false), Some(true)] {
        assert!(!matches!(drag_pointer(over), CursorIcon::Grabbing | CursorIcon::Grab | CursorIcon::Move | CursorIcon::AllScroll));
    }
}

#[test]
fn a_folder_takes_only_messages_that_are_not_in_it() {
    let mb = mailcraft_engine::sample::mailbox(1_791_993_600, 0);
    let m = mb.messages.values().next().expect("sample has mail");
    let (id, here) = (m.id, m.folder);
    let elsewhere = mb.folders.iter().find(|f| f.id != here).expect("two folders").id;
    assert!(movable(&mb, &[id], here).is_empty());
    assert_eq!(movable(&mb, &[id], elsewhere), vec![id]);
    assert!(movable(&mb, &[u64::MAX], here).is_empty());
}
