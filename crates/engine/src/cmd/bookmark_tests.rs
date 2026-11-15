// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::json;

use crate::{Session, sample};

const NOW: i64 = 1_791_993_600;

fn sample_session() -> (Session, mailcraft_model::Id) {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let id = s.mailbox.messages.keys().next().copied().unwrap();
    s.run("mail.select", &json!({"id": id})).unwrap();
    (s, id)
}

#[test]
fn add_and_remove_bookmark() {
    let (mut s, mid) = sample_session();

    let res = s
        .run(
            "bookmark.add",
            &json!({
                "id": mid,
                "name": "Important Contract Clause",
                "quote": "Payment shall be made within 30 days.",
                "offset": 120
            }),
        )
        .unwrap();

    let bid = res["id"].as_u64().unwrap();
    assert_eq!(res["message"], mid);
    assert_eq!(res["name"], "Important Contract Clause");
    assert_eq!(res["offset"], 120);

    // Verify in mailbox
    let bm = s.mailbox.bookmark(bid).unwrap();
    assert_eq!(bm.name, "Important Contract Clause");
    assert_eq!(bm.message, mid);

    // List bookmarks for message
    let list = s.run("bookmark.list", &json!({"id": mid})).unwrap();
    let bms = list["bookmarks"].as_array().unwrap();
    assert_eq!(bms.len(), 1);
    assert_eq!(bms[0]["id"], bid);

    // Jump
    let jump = s.run("bookmark.jump", &json!({"id": bid})).unwrap();
    assert_eq!(jump["id"], bid);
    assert_eq!(jump["message"], mid);

    // Remove
    s.run("bookmark.remove", &json!({"id": bid})).unwrap();
    assert!(s.mailbox.bookmark(bid).is_none());

    let list_after = s.run("bookmark.list", &json!({"id": mid})).unwrap();
    assert_eq!(list_after["bookmarks"].as_array().unwrap().len(), 0);
}

#[test]
fn bookmark_conversation_list() {
    let (mut s, mid) = sample_session();

    let res = s
        .run(
            "bookmark.add",
            &json!({
                "id": mid,
                "quote": "Some paragraph text",
                "offset": 10
            }),
        )
        .unwrap();
    let bid = res["id"].as_u64().unwrap();

    let list = s.run("bookmark.list", &json!({"id": mid, "conversation": true})).unwrap();
    let bms = list["bookmarks"].as_array().unwrap();
    assert!(bms.iter().any(|b| b["id"] == bid));
}

#[test]
fn bookmark_hostile_offset_clamped() {
    let (mut s, mid) = sample_session();

    // Negative offset clamps to 0
    let res = s.run("bookmark.add", &json!({"id": mid, "offset": -50})).unwrap();
    assert_eq!(res["offset"], 0);

    // Huge offset clamps to MAX_BOOKMARK_OFFSET
    let res = s.run("bookmark.add", &json!({"id": mid, "offset": 999_999_999})).unwrap();
    assert_eq!(res["offset"], mailcraft_model::MAX_BOOKMARK_OFFSET);
}

#[test]
fn bookmark_defaults_name_to_subject() {
    let (mut s, mid) = sample_session();
    let subject = s.mailbox.message(mid).unwrap().subject.clone();

    let res = s.run("bookmark.add", &json!({"id": mid})).unwrap();
    assert_eq!(res["name"], subject.trim());
}

#[test]
fn bookmark_max_bookmarks_limit() {
    let (mut s, mid) = sample_session();

    for i in 0..mailcraft_model::MAX_BOOKMARKS {
        let id = s.mailbox.alloc_id();
        let bm = mailcraft_model::Bookmark::new(id, mid, "mark", "", i + 1, NOW);
        s.mailbox.add_bookmark(bm);
    }
    assert_eq!(s.mailbox.bookmarks().len(), mailcraft_model::MAX_BOOKMARKS);

    let err = s.run("bookmark.add", &json!({"id": mid})).unwrap_err();
    assert!(err.to_string().contains("too many bookmarks"));
}

#[test]
fn jump_to_deleted_message_returns_error() {
    let (mut s, mid) = sample_session();

    let res = s.run("bookmark.add", &json!({"id": mid})).unwrap();
    let bid = res["id"].as_u64().unwrap();

    // Forget the message directly from mailbox
    s.mailbox.messages.remove(&mid);

    let err = s.run("bookmark.jump", &json!({"id": bid})).unwrap_err();
    assert!(err.to_string().contains("no longer exists"));
}

#[test]
fn duplicate_bookmark_returns_existing_even_at_capacity() {
    let (mut s, mid) = sample_session();

    let first = s
        .run(
            "bookmark.add",
            &json!({
                "id": mid,
                "name": "First",
                "quote": "Exact quote text",
                "offset": 42
            }),
        )
        .unwrap();
    let first_id = first["id"].as_u64().unwrap();

    // Fill remaining capacity up to MAX_BOOKMARKS.
    while s.mailbox.bookmarks().len() < mailcraft_model::MAX_BOOKMARKS {
        let id = s.mailbox.alloc_id();
        let off = id as usize;
        s.mailbox.add_bookmark(mailcraft_model::Bookmark::new(id, mid, "filler", "other", off, NOW));
    }
    assert_eq!(s.mailbox.bookmarks().len(), mailcraft_model::MAX_BOOKMARKS);

    // Adding the exact same (message, quote, offset) — even with surrounding spaces or U+202E — returns existing.
    let dup = s
        .run(
            "bookmark.add",
            &json!({
                "id": mid,
                "name": "Different Label",
                "quote": "  Exact\u{202E} quote text  ",
                "offset": 42
            }),
        )
        .unwrap();
    assert_eq!(dup["id"], first_id);
    assert_eq!(s.mailbox.bookmarks().len(), mailcraft_model::MAX_BOOKMARKS);
}

#[test]
fn bookmark_add_strips_format_characters() {
    let (mut s, mid) = sample_session();
    let res = s
        .run(
            "bookmark.add",
            &json!({
                "id": mid,
                "name": "Safe\u{202E}Name",
                "quote": "Clean\u{200B}Quote",
                "offset": 0
            }),
        )
        .unwrap();
    assert_eq!(res["name"], "SafeName");
    assert_eq!(res["quote"], "CleanQuote");
}

#[test]
fn jump_selects_message_in_another_folder_outside_current_view() {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let sent = s.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Sent).unwrap().id;
    let inbox = s.mailbox.inbox().unwrap();
    let sent_msg = s.mailbox.messages.values().find(|m| m.folder == sent).unwrap().id;
    let inbox_msg = s.mailbox.messages.values().find(|m| m.folder == inbox).unwrap().id;

    // Put Inbox in view and focus an Inbox message (so sent_msg is outside current view).
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    s.run("mail.select", &json!({"id": inbox_msg})).unwrap();
    assert!(s.in_list(&[sent_msg]).is_empty());

    let added = s.run("bookmark.add", &json!({"id": sent_msg, "quote": "reply", "offset": 5})).unwrap();
    let bid = added["id"].as_u64().unwrap();

    let jumped = s.run("bookmark.jump", &json!({"id": bid})).unwrap();
    assert_eq!(jumped["message"], sent_msg);
    assert_eq!(s.sel.focus(), Some(sent_msg));
}
