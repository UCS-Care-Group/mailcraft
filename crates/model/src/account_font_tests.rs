// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fonts an account writes in: kept in the mailbox file beside its signatures, read from a
//! file from before there were any, and each account's own.

use serde_json::json;

use crate::rich::{Font, Style};
use crate::{Account, FontSlot, Mailbox};

fn calibri12() -> Style {
    Style { font: Font::parse("Calibri"), size: Some(12), ..Style::default() }
}

fn courier10() -> Style {
    Style { font: Font::parse("Courier New"), size: Some(10), ..Style::default() }
}

/// Each font is written with its account and read back as it was; an account with none writes
/// nothing for them.
#[test]
fn an_accounts_fonts_are_kept_in_the_file() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let work = mb.add_account(Account {
        email: "ada@work.example".into(),
        new_font: Some(calibri12()),
        reply_font: Some(Style { bold: true, ..Style::default() }),
        plain_font: Some(courier10()),
        ..Default::default()
    });
    let text = serde_json::to_string(&mb).unwrap();
    let back: Mailbox = serde_json::from_str(&text).unwrap();
    let a = back.account(work).unwrap();
    assert_eq!(a.default_font(FontSlot::New), Some(&calibri12()));
    assert_eq!(a.default_font(FontSlot::Reply), Some(&Style { bold: true, ..Style::default() }));
    assert_eq!(a.default_font(FontSlot::Plain), Some(&courier10()));
    // The other account says nothing about fonts at all.
    let other = back.accounts.iter().find(|o| o.id != work).unwrap();
    assert!(other.new_font.is_none() && other.reply_font.is_none() && other.plain_font.is_none());
    let first = serde_json::to_value(other).unwrap();
    for key in ["newFont", "replyFont", "plainFont"] {
        assert!(first.get(key).is_none(), "{key} written for an account with none: {first}");
    }
}

/// A file from before there were fonts has none; one that holds something this version can't read
/// as a font loses that font and nothing else.
#[test]
fn a_file_without_fonts_or_with_a_bad_one_still_opens() {
    for bad in [json!("Calibri"), json!(12), json!([1, 2]), json!({"size": "big"}), json!(true)] {
        let account = json!({
            "id": 7, "email": "ada@example.org", "signature": "Work",
            "newFont": bad, "replyFont": bad, "plainFont": bad,
        });
        let read: Account = serde_json::from_value(account).unwrap();
        for slot in [FontSlot::New, FontSlot::Reply, FontSlot::Plain] {
            assert_eq!(read.default_font(slot), None, "{slot:?} read from {bad}");
        }
        assert_eq!(read.email, "ada@example.org");
        assert_eq!(read.signature, "Work");
    }
    // A font whose family this version can't keep still keeps its size.
    let read: Account = serde_json::from_value(json!({"id": 7, "email": "a@b.example", "newFont": {"font": "<x>", "size": 14}})).unwrap();
    assert_eq!(read.new_font, Some(Style { size: Some(14), ..Style::default() }));
}

/// What is written in each of the three ways is the account's own, and a slot with nothing chosen
/// is the message's own.
#[test]
fn each_slot_is_the_accounts_own_and_none_is_the_messages_own() {
    let mut a = Account { email: "ada@example.org".into(), ..Default::default() };
    for slot in [FontSlot::New, FontSlot::Reply, FontSlot::Plain] {
        assert_eq!(a.default_font(slot), None);
        assert!(a.writing_font(slot).is_plain(), "an account with no font writes in nothing");
    }
    a.set_default_font(FontSlot::New, Some(calibri12()));
    a.set_default_font(FontSlot::Plain, Some(courier10()));
    assert_eq!(a.writing_font(FontSlot::New), calibri12());
    assert_eq!(a.writing_font(FontSlot::Plain), courier10());
    assert!(a.writing_font(FontSlot::Reply).is_plain());
    a.set_default_font(FontSlot::New, None);
    assert!(a.new_font.is_none() && a.plain_font.is_some());
}

/// The words a command names the slots by are the ones they parse back from.
#[test]
fn a_slot_is_named_by_one_word() {
    for (word, slot) in [("new", FontSlot::New), ("Reply", FontSlot::Reply), (" plain ", FontSlot::Plain)] {
        assert_eq!(FontSlot::parse(word), Some(slot), "{word:?}");
        assert_eq!(slot.key(), word.trim().to_lowercase());
    }
    for word in ["", "newer", "forward", "new message"] {
        assert_eq!(FontSlot::parse(word), None, "{word:?}");
    }
    // Space around the word, of any kind, says the same word.
    assert_eq!(FontSlot::parse("new\u{a0}"), Some(FontSlot::New));
}
