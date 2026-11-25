// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a stacked conversation lists and what selecting in it reads (#1778).

use mailcraft_model::Id;

use crate::stack::{LISTED, WHOLE, listed, read_by_selecting};

#[test]
fn the_selected_message_is_listed_however_long_its_conversation() {
    let thread: Vec<Id> = (1..=250).collect();
    let newest = listed(&thread, 240);
    assert_eq!((newest.len(), newest.first(), newest.last()), (LISTED, Some(&51), Some(&250)));
    let oldest = listed(&thread, 1);
    assert_eq!((oldest.len(), oldest.first(), oldest.get(1), oldest.last()), (LISTED, Some(&1), Some(&52), Some(&250)));
    assert_eq!(listed(&thread[..5], 3), thread[..5].to_vec());
}

#[test]
fn selecting_reads_only_what_the_pane_shows_whole() {
    let thread: Vec<Id> = (1..=100).collect();
    // All unread: the selected one and the newest WHOLE others; the older ones stay unread.
    let read = read_by_selecting(&thread, 100, |_| true);
    assert_eq!(read.len(), WHOLE + 1);
    assert!(read.contains(&100) && read.contains(&99) && read.contains(&(100 - WHOLE as Id)));
    assert!(!read.contains(&(99 - WHOLE as Id)) && !read.contains(&1));
    // Only some unread: those, and nothing already read.
    let read = read_by_selecting(&thread, 50, |id| id % 10 == 0);
    assert_eq!(read.len(), 10);
    assert!(read.iter().all(|id| id % 10 == 0));
    // The selected one already read: only the others.
    assert_eq!(read_by_selecting(&thread, 1, |id| id == 2), vec![2]);
}
