// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fonts an account writes in, through commands (R4.5): each slot its own, refused when it
//! isn't a font, and taken up by whatever is written from that account.

use mailcraft_model::rich::{Font, Style};
use mailcraft_model::{Account, AccountKind, FontSlot};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn account(s: &Session) -> u64 {
    s.mailbox.default_account().unwrap().id
}

/// A second account, so that each one's own fonts can be told apart.
fn another(s: &mut Session) -> u64 {
    s.mailbox.add_account(Account { email: "avery@work.example".into(), name: "Work".into(), ..Default::default() })
}

fn font(s: &Session, account: u64, slot: FontSlot) -> Option<Style> {
    s.mailbox.account(account).and_then(|a| a.default_font(slot).cloned())
}

/// What the message being written would give a character typed where there is nothing to look
/// like — the font the account chose, or nothing.
fn written_in(s: &Session) -> Style {
    s.compose.as_ref().expect("a message is being written").body.default_style().clone()
}

#[test]
fn each_account_writes_in_its_own_fonts() {
    let mut s = session();
    let a = account(&s);
    let b = another(&mut s);
    let r = run(
        &mut s,
        "signature.fonts",
        json!({"account": a, "new": {"font": "Calibri", "size": 12}, "reply": {"font": "Georgia", "italic": true}, "plain": {"font": "Courier New", "size": 10}}),
    );
    assert_eq!(r["account"], json!(a));
    assert_eq!(r["new"]["size"], json!(12));
    assert_eq!(r["reply"]["italic"], json!(true));
    assert_eq!(font(&s, a, FontSlot::New).map(|f| f.size), Some(Some(12)));
    assert_eq!(font(&s, a, FontSlot::Reply).map(|f| f.italic), Some(true));
    assert_eq!(font(&s, a, FontSlot::Plain).map(|f| f.size), Some(Some(10)));
    // The other account says nothing about fonts, and one slot set alone leaves the others.
    assert!(font(&s, b, FontSlot::New).is_none() && font(&s, b, FontSlot::Reply).is_none());
    run(&mut s, "signature.fonts", json!({"account": b, "new": {"bold": true}}));
    assert_eq!(font(&s, b, FontSlot::New).map(|f| f.bold), Some(true));
    assert!(font(&s, b, FontSlot::Reply).is_none());
    assert_eq!(font(&s, a, FontSlot::New).map(|f| f.size), Some(Some(12)));
    // With no account named it is the default account's.
    run(&mut s, "signature.fonts", json!({"plain": null}));
    assert!(font(&s, a, FontSlot::Plain).is_none());
    assert!(font(&s, b, FontSlot::Plain).is_none());
}

#[test]
fn a_font_that_isnt_one_is_refused_and_changes_nothing() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Calibri", "size": 12}}));
    let before = font(&s, a, FontSlot::New);
    let junk = [
        json!({}),
        json!({"account": a}),
        json!({"new": "Calibri"}),
        json!({"new": 12}),
        json!({"new": {"font": "<script>"}}),
        json!({"new": {"font": 12}}),
        json!({"new": {"font": ""}}),
        json!({"new": {"size": 4}}),
        json!({"new": {"size": 40}}),
        json!({"new": {"size": "12"}}),
        json!({"new": {"size": 12.5}}),
        json!({"new": {"size": null, "colour": "#ff0000"}}),
        json!({"new": {"color": "red"}}),
        json!({"new": {"color": "#ff00"}}),
        json!({"new": {"bold": "yes"}}),
        json!({"new": {"font": "a".repeat(4000)}}),
        json!({"account": 999_999, "new": null}),
    ];
    // (`account` that isn't an id at all — a string, zero — says nothing, as everywhere else a
    // command takes an account, and so means the default one.)
    for params in junk {
        assert!(s.run("signature.fonts", &params).is_err(), "{params} was taken");
        assert_eq!(font(&s, a, FontSlot::New), before, "{params} changed something");
    }
    // An account a newer MailCraft added has its settings there, not here.
    let gone = s.mailbox.add_account(Account { email: "x@y.example".into(), kind: AccountKind::Unknown, ..Default::default() });
    assert!(s.run("signature.fonts", &json!({"account": gone, "new": null})).is_err());
}

#[test]
fn a_font_with_nothing_in_it_is_the_messages_own() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Georgia"}}));
    assert!(font(&s, a, FontSlot::New).is_some());
    // All its parts off is the same as asking for nothing.
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"bold": false, "size": null}}));
    assert!(font(&s, a, FontSlot::New).is_none());
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Georgia"}}));
    run(&mut s, "signature.fonts", json!({"account": a, "new": null}));
    assert!(font(&s, a, FontSlot::New).is_none());
}

#[test]
fn the_fonts_undo_as_one_step() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Verdana", "size": 14}, "plain": {"bold": true}}));
    assert!(font(&s, a, FontSlot::New).is_some() && font(&s, a, FontSlot::Plain).is_some());
    run(&mut s, "edit.undo", json!({}));
    assert!(font(&s, a, FontSlot::New).is_none() && font(&s, a, FontSlot::Plain).is_none());
    run(&mut s, "edit.redo", json!({}));
    assert!(font(&s, a, FontSlot::New).is_some() && font(&s, a, FontSlot::Plain).is_some());
}

#[test]
fn a_new_message_is_written_in_the_accounts_font_and_the_signature_keeps_its_own() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "signature.set", json!({"name": "Bold", "text": "Avery Quinn", "default": true, "account": a}));
    run(&mut s, "signature.open", json!({"name": "Bold"}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 5}));
    run(&mut s, "format.bold", json!({}));
    run(&mut s, "signature.save", json!({"name": "Bold"}));
    run(&mut s, "signature.close", json!({"name": "Bold"}));
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Georgia", "size": 14}}));

    run(&mut s, "mail.new", json!({"account": a}));
    // The line above the signature has nothing to look like, so it takes the account's font.
    run(&mut s, "compose.type", json!({"text": "Hello"}));
    let c = s.compose.as_ref().unwrap();
    let first = c.body.text().paragraphs().first().expect("a paragraph");
    assert_eq!(first.runs[0].style.font, Font::parse("Georgia"));
    assert_eq!(first.runs[0].style.size, Some(14));
    // The signature below is still its own bold self.
    let last = c.body.text().paragraphs().last().expect("a paragraph");
    assert!(last.runs.iter().any(|r| r.style.bold && r.style.font.is_none()), "{last:?}");
    run(&mut s, "compose.discard", json!({}));
}

#[test]
fn a_reply_takes_the_reply_font() {
    let mut s = session();
    let a = account(&s);
    let georgia = Style { font: Font::parse("Georgia"), italic: true, ..Style::default() };
    let courier = Style { font: Font::parse("Courier New"), size: Some(10), ..Style::default() };
    run(
        &mut s,
        "signature.fonts",
        json!({"account": a, "new": {"font": "Arial"}, "reply": {"font": "Georgia", "italic": true}, "plain": {"font": "Courier New", "size": 10}}),
    );
    // An HTML original: what is written above it is HTML, and takes the reply font.
    let html = s.mailbox.messages.values().find(|m| m.account == a && m.body.html.is_some()).expect("an HTML message in the sample").id;
    run(&mut s, "mail.select", json!({"id": html}));
    run(&mut s, "mail.reply", json!({}));
    assert_eq!(written_in(&s), georgia);
    run(&mut s, "compose.discard", json!({}));
    run(&mut s, "mail.select", json!({"id": html}));
    run(&mut s, "mail.forward", json!({"id": html}));
    assert_eq!(written_in(&s), georgia);
    run(&mut s, "compose.discard", json!({}));
    // A plain-text original: the reply is plain text too, so it is written in the plain font.
    let plain = s.mailbox.messages.values().find(|m| m.account == a && m.body.html.is_none()).expect("a plain-text message in the sample").id;
    run(&mut s, "mail.select", json!({"id": plain}));
    run(&mut s, "mail.reply", json!({}));
    assert_eq!(written_in(&s), courier);
    run(&mut s, "compose.discard", json!({}));
    // Forwarded whole, nothing of the original is written in: it is a new message.
    run(&mut s, "mail.select", json!({"id": plain}));
    run(&mut s, "mail.forwardAsAttachment", json!({"id": plain}));
    assert_eq!(written_in(&s), Style { font: Font::parse("Arial"), ..Style::default() });
    run(&mut s, "compose.discard", json!({}));
}

#[test]
fn a_plain_text_message_takes_the_plain_font() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Georgia"}, "plain": {"font": "Courier New", "size": 10}}));
    run(&mut s, "mail.new", json!({"account": a}));
    assert_eq!(written_in(&s).font, Font::parse("Georgia"));
    // Plain text carries no font of its own, so the account's plain font is what is written in.
    run(&mut s, "compose.formatPlain", json!({}));
    assert_eq!(written_in(&s).font, Font::parse("Courier New"));
    assert_eq!(written_in(&s).size, Some(10));
    run(&mut s, "compose.formatHtml", json!({}));
    assert_eq!(written_in(&s).font, Font::parse("Georgia"));
    // A plain draft reopened takes it again.
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.type", json!({"text": "Note"}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("a draft");
    // Put aside for another message (which keeps the draft), then opened again.
    run(&mut s, "mail.new", json!({"account": a}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert_eq!(written_in(&s).font, Font::parse("Courier New"));
    run(&mut s, "compose.discard", json!({}));
}

#[test]
fn changing_from_takes_the_other_accounts_font() {
    let mut s = session();
    let a = account(&s);
    let b = another(&mut s);
    run(&mut s, "signature.fonts", json!({"account": a, "new": {"font": "Georgia"}}));
    run(&mut s, "signature.fonts", json!({"account": b, "new": {"font": "Verdana", "size": 16}}));
    run(&mut s, "mail.new", json!({"account": a}));
    assert_eq!(written_in(&s).font, Font::parse("Georgia"));
    run(&mut s, "compose.set", json!({"account": b}));
    assert_eq!(written_in(&s).font, Font::parse("Verdana"));
    assert_eq!(written_in(&s).size, Some(16));
    // The same account again changes nothing.
    run(&mut s, "compose.set", json!({"account": b, "subject": "Hi"}));
    assert_eq!(written_in(&s).font, Font::parse("Verdana"));
    run(&mut s, "compose.discard", json!({}));
}

#[test]
fn an_account_with_no_font_writes_as_it_always_did() {
    let mut s = session();
    let a = account(&s);
    run(&mut s, "mail.new", json!({"account": a}));
    assert!(written_in(&s).is_plain());
    run(&mut s, "compose.type", json!({"text": "Hello"}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.body.text().paragraphs().first().expect("a paragraph").runs.iter().all(|r| r.style.is_plain()));
    run(&mut s, "compose.discard", json!({}));
}
