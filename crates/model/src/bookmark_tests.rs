// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::time::Timestamp;
use crate::{Bookmark, MAX_BOOKMARK_NAME, MAX_BOOKMARK_QUOTE, Mailbox};

#[test]
fn bookmark_creation_and_cleaning() {
    let bm = Bookmark::new(1, 10, "  Important Note  ", "Here is the quote from the contract.", 42, 1_700_000_000);
    assert_eq!(bm.id, 1);
    assert_eq!(bm.message, 10);
    assert_eq!(bm.name, "Important Note");
    assert_eq!(bm.quote, "Here is the quote from the contract.");
    assert_eq!(bm.offset, 42);
}

#[test]
fn bookmark_empty_name_defaults_to_quote_snippet() {
    let bm = Bookmark::new(2, 10, "", "First line of quote\nSecond line", 100, Timestamp::default());
    assert_eq!(bm.name, "First line of quote");
}

#[test]
fn bookmark_control_characters_stripped() {
    let raw_name = "Header\x00\x07\x1bWith\x1fControls";
    let raw_quote = "Quote\x08With\x0cControls\tAnd\nNewlines";
    let bm = Bookmark::new(1, 10, raw_name, raw_quote, 0, Timestamp::default());
    assert_eq!(bm.name, "HeaderWithControls");
    assert_eq!(bm.quote, "QuoteWithControls\tAnd\nNewlines");
}

#[test]
fn bookmark_name_and_quote_caps_at_char_boundaries_multibyte() {
    // Each emoji / CJK character is multiple bytes (3 or 4 bytes each)
    let long_cjk_name: String = "🦀".repeat(MAX_BOOKMARK_NAME + 50);
    let long_cjk_quote: String = "日".repeat(MAX_BOOKMARK_QUOTE + 50);

    let bm = Bookmark::new(1, 10, &long_cjk_name, &long_cjk_quote, 0, Timestamp::default());
    assert_eq!(bm.name.chars().count(), MAX_BOOKMARK_NAME);
    assert_eq!(bm.quote.chars().count(), MAX_BOOKMARK_QUOTE);
    // Verifying it's valid UTF-8 and slicing was on char boundary
    assert!(bm.name.chars().all(|c| c == '🦀'));
    assert!(bm.quote.chars().all(|c| c == '日'));
}

#[test]
fn mailbox_bookmarks_crud() {
    let mut mb = Mailbox::default();
    assert!(mb.bookmarks().is_empty());

    let b1 = Bookmark::new(1, 100, "Section A", "quote 1", 5, Timestamp::default());
    let b2 = Bookmark::new(2, 100, "Section B", "quote 2", 50, Timestamp::default());
    let b3 = Bookmark::new(3, 200, "Other Msg", "quote 3", 0, Timestamp::default());

    assert!(mb.add_bookmark(b1.clone()));
    assert!(mb.add_bookmark(b2.clone()));
    assert!(mb.add_bookmark(b3.clone()));

    assert_eq!(mb.bookmarks().len(), 3);
    assert_eq!(mb.bookmark(1), Some(&b1));
    assert_eq!(mb.bookmarks_for_message(100).len(), 2);
    assert_eq!(mb.bookmarks_for_message(200).len(), 1);

    assert!(mb.remove_bookmark(1));
    assert_eq!(mb.bookmarks().len(), 2);
    assert_eq!(mb.bookmark(1), None);
    assert!(!mb.remove_bookmark(1));
}

#[test]
fn removing_message_cleans_up_bookmarks() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(crate::Account::default());
    let folder = mb.inbox().unwrap();

    let msg = crate::Message { folder, account, ..Default::default() };
    let mid = mb.add_message(msg).unwrap();

    let bm = Bookmark::new(1, mid, "Mark", "snippet", 10, Timestamp::default());
    mb.add_bookmark(bm);
    assert_eq!(mb.bookmarks_for_message(mid).len(), 1);

    // Forget or remove the message
    let _ = mb.forget_message(mid);
    assert_eq!(mb.bookmarks_for_message(mid).len(), 0);
}

#[test]
fn repair_drops_bookmarks_for_missing_messages() {
    let mut mb = Mailbox::default();
    let account = mb.add_account(crate::Account::default());
    let folder = mb.inbox().unwrap();

    let msg = crate::Message { folder, account, ..Default::default() };
    let mid = mb.add_message(msg).unwrap();

    // Valid bookmark for existing message
    let b_valid = Bookmark::new(1, mid, "Valid", "quote", 0, Timestamp::default());
    // Dangling bookmark for non-existent message
    let b_dangling = Bookmark::new(2, 99999, "Dangling", "quote", 0, Timestamp::default());

    mb.bookmarks.insert(b_valid.id, b_valid);
    mb.bookmarks.insert(b_dangling.id, b_dangling);
    assert_eq!(mb.bookmarks.len(), 2);

    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("bookmarks tidied")));
    assert_eq!(mb.bookmarks.len(), 1);
    assert!(mb.bookmarks.contains_key(&1));
    assert!(!mb.bookmarks.contains_key(&2));
}
