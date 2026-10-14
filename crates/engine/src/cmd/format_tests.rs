//! Tests for formatting the message being written.

use mailcraft_model::rich::{Align, List, PICTURE};
use mailcraft_model::{Account, AccountKind, Body, FolderKind, Mailbox, Message, Rgb, Security, Server};
use serde_json::{Value, json};

use crate::Session;

/// 2026-10-14 18:00 UTC, with one account that has a mail server.
fn session() -> (Session, u64) {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account {
        name: "me@example.org".into(),
        email: "me@example.org".into(),
        display_name: "Me".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls }),
        outgoing: Some(Server { host: "smtp.example.org".into(), port: 465, security: Security::Tls }),
        ..Default::default()
    });
    (Session::at(mb, 1_792_000_800, 0), account)
}

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    match s.run(id, &v) {
        Ok(v) => v,
        Err(e) => panic!("{id} {v}: {e}"),
    }
}

/// A tiny PNG (the signature and a little more: only the first bytes are looked at).
fn png() -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend_from_slice(&[0u8; 32]);
    b
}

fn b64(bytes: &[u8]) -> String {
    crate::cmd::base64_encode(bytes)
}

#[test]
fn new_messages_are_html_unless_the_owner_writes_plain_text() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"to": "ada@example.org"}));
    assert!(s.compose.as_ref().unwrap().html);
    run(&mut s, "file.options", json!({"composePlain": true}));
    run(&mut s, "mail.new", json!({}));
    let c = s.compose.as_ref().unwrap();
    assert!(!c.html);
    assert!(s.run("format.bold", &json!({})).is_err(), "no formatting in plain text");
    run(&mut s, "compose.formatHtml", json!({}));
    assert!(s.compose.as_ref().unwrap().html);
}

#[test]
fn formatting_a_selection_and_typing_with_a_chosen_look() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "body": "Hello world"}));
    run(&mut s, "compose.select", json!({"start": 6, "end": 11}));
    assert_eq!(run(&mut s, "format.bold", json!({}))["value"], true);
    let c = s.compose.clone().unwrap();
    assert!(c.styles.every(6..11, |st| st.bold) && !c.styles.every(0..6, |st| st.bold));
    assert!(super::state(&c).bold);
    // Again: off.
    assert_eq!(run(&mut s, "format.bold", json!({}))["value"], false);
    // With a caret, the look is for what is typed next.
    run(&mut s, "compose.select", json!({"start": 11}));
    run(&mut s, "format.italic", json!({}));
    assert!(super::state(s.compose.as_ref().unwrap()).italic);
    run(&mut s, "compose.set", json!({"body": "Hello world!!"}));
    let c = s.compose.clone().unwrap();
    assert!(c.styles.every(11..13, |st| st.italic) && !c.styles.every(0..11, |st| st.italic));
    assert!(c.typing.is_none());
    // Moving the caret forgets a look chosen at it.
    run(&mut s, "compose.select", json!({"start": 2}));
    run(&mut s, "format.underline", json!({}));
    run(&mut s, "compose.select", json!({"start": 3}));
    assert!(s.compose.as_ref().unwrap().typing.is_none());
}

#[test]
fn every_character_format_and_paragraph_format() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"body": "one\ntwo\nthree"}));
    run(&mut s, "format.size", json!({"size": 20, "start": 0, "end": 3}));
    run(&mut s, "format.size", json!({"size": "larger", "start": 4, "end": 7}));
    run(&mut s, "format.color", json!({"color": "#c00000", "start": 0, "end": 3}));
    run(&mut s, "format.highlight", json!({"start": 4, "end": 7}));
    run(&mut s, "format.strike", json!({"start": 8, "end": 13}));
    run(&mut s, "format.font", json!({"face": "fixed", "start": 8, "end": 13}));
    run(&mut s, "format.bullets", json!({"start": 0, "end": 5}));
    run(&mut s, "format.numbering", json!({"start": 9}));
    run(&mut s, "format.indent", json!({"start": 9}));
    run(&mut s, "format.align", json!({"align": "center", "start": 0}));
    let c = s.compose.clone().unwrap();
    assert!(c.styles.every(0..3, |st| st.size == Some(20) && st.color == Some(Rgb(0xC0, 0, 0))));
    assert!(c.styles.every(4..7, |st| st.size == Some(16) && st.highlight == Some(super::DEFAULT_HIGHLIGHT)));
    assert!(c.styles.every(8..13, |st| st.strike && st.fixed));
    assert_eq!(c.styles.paras.iter().map(|p| p.list).collect::<Vec<_>>(), vec![List::Bullet, List::Bullet, List::Number]);
    assert_eq!(c.styles.paras[2].indent, 1);
    assert_eq!(c.styles.paras[0].align, Align::Center);
    assert!(c.dirty);
    // Sent, it is HTML with a plain-text twin.
    let body = super::super::compose::body_of(&c);
    let html = body.html.unwrap();
    assert!(html.contains("<ul") && html.contains("<ol") && html.contains("color:#c00000") && html.contains("<s><code>three</code></s>"), "{html}");
    assert!(body.text.starts_with("• one\n• two\n    1. three"), "{}", body.text);
    // Cleared.
    run(&mut s, "format.clear", json!({"start": 0, "end": 13}));
    let c = s.compose.clone().unwrap();
    assert!(c.styles.is_plain());
    // Bad values are refused.
    for (id, v) in [
        ("format.size", json!({"size": "huge"})),
        ("format.color", json!({"color": "red"})),
        ("format.align", json!({"align": "justify"})),
        ("format.indent", json!({"by": 7})),
        ("format.font", json!({"face": "Comic"})),
        ("compose.link", json!({"url": "javascript:alert(1)"})),
    ] {
        assert!(s.run(id, &v).is_err(), "{id} {v}");
    }
}

#[test]
fn links_are_put_in_and_taken_off() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"body": "See the site."}));
    run(&mut s, "compose.link", json!({"url": "example.org/page", "start": 8, "end": 12}));
    let c = s.compose.clone().unwrap();
    assert!(c.styles.every(8..12, |st| st.link.as_deref() == Some("https://example.org/page")));
    // At a caret with text: put in, linked.
    run(&mut s, "compose.link", json!({"url": "ada@example.org", "text": "Ada", "start": 0}));
    let c = s.compose.clone().unwrap();
    assert!(c.body.starts_with("AdaSee"));
    assert!(c.styles.every(0..3, |st| st.link.as_deref() == Some("mailto:ada@example.org")));
    // Taken off where the caret is.
    run(&mut s, "compose.link", json!({"url": null, "start": 1}));
    assert!(s.compose.as_ref().unwrap().styles.every(0..3, |st| st.link.is_none()));
    assert!(s.run("compose.link", &json!({"url": null, "start": 1})).is_err(), "no link there now");
}

#[test]
fn a_picture_is_shown_in_the_message_and_goes_as_a_related_part() {
    let (mut s, _) = session();
    run(&mut s, "file.options", json!({"composePlain": true}));
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "subject": "Look", "body": "Above\nBelow"}));
    assert!(s.run("insert.picture", &json!({"name": "notes.txt", "data": b64(b"just text")})).is_err(), "not a picture by its bytes");
    let mut big = png();
    big.resize(super::MAX_PICTURE + 1, 0);
    assert!(s.run("insert.picture", &json!({"name": "big.png", "data": b64(&big)})).is_err());
    let r = run(&mut s, "insert.picture", json!({"name": "C:\\pics\\chart.png", "data": b64(&png()), "at": 5}));
    assert_eq!(r["switched"], true, "a picture needs HTML");
    let c = s.compose.clone().unwrap();
    assert_eq!(c.body, format!("Above\n{PICTURE}\nBelow"));
    let cid = r["contentId"].as_str().unwrap().to_string();
    assert_eq!(c.attachments[0].name, "chart.png");
    assert!(c.attachments[0].inline && c.attachments[0].content_id == cid);
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    assert!(m.body.html.as_deref().unwrap().contains(&format!("src=\"cid:{cid}\"")));
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("multipart/alternative") && text.contains("multipart/related") && text.contains(&format!("Content-ID: <{cid}>")), "{text}");
    // A picture taken out of the text doesn't go.
    run(&mut s, "mail.new", json!({"to": "ada@example.org"}));
    run(&mut s, "insert.picture", json!({"name": "a.png", "data": b64(&png())}));
    run(&mut s, "compose.set", json!({"body": "no picture now"}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().attachments.is_empty());
}

#[test]
fn an_html_draft_keeps_its_formatting_through_the_mail_format() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "subject": "Draft", "body": "Title\nfirst\nsecond\n\nsee here"}));
    run(&mut s, "format.bold", json!({"start": 0, "end": 5}));
    run(&mut s, "format.size", json!({"size": 24, "start": 0, "end": 5}));
    run(&mut s, "format.bullets", json!({"start": 6, "end": 13}));
    run(&mut s, "compose.link", json!({"url": "https://example.org", "start": 20, "end": 28}));
    run(&mut s, "insert.picture", json!({"name": "p.png", "data": b64(&png()), "at": 28}));
    let written = s.compose.clone().unwrap();
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    // Up to the server and back: written as MIME, read again.
    let m = s.mailbox.message(draft).unwrap().clone();
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions { include_bcc: true, ..Default::default() });
    let back = mailcraft_mime::parse(&bytes);
    assert!(back.attachments.iter().any(|a| a.inline && !a.content_id.is_empty()));
    let replaced = Message { id: m.id, folder: m.folder, account: m.account, draft: true, ..back };
    s.mailbox.replace_message(draft, replaced).unwrap();
    s.compose = None;
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let c = s.compose.clone().unwrap();
    assert!(c.html);
    assert_eq!(c.body, written.body);
    let mut want = written.styles.clone();
    want.normalize(&written.body);
    assert_eq!(c.styles, want);
}

#[test]
fn an_html_message_is_answered_with_its_formatting_and_nothing_hostile() {
    let (mut s, account) = session();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let html = "<p>Hi <b>there</b>, see <a href=\"javascript:alert(1)\">this</a> and <a href=\"https://example.org\">that</a>.</p>\
                <script>steal()</script><img src=\"https://tracker.example/p.gif\" width=1 height=1><img src=\"cid:logo@example.org\"><p style=\"color:#336699\">Blue</p>";
    let mut logo = mailcraft_model::Attachment::new("logo.png", "image/png", png());
    logo.inline = true;
    logo.content_id = "logo@example.org".into();
    let id = s
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            from: mailcraft_model::Address::new("Bob", "bob@example.org"),
            to: vec![mailcraft_model::Address::new("Me", "me@example.org")],
            subject: "Plans".into(),
            body: Body { text: "Hi there".into(), html: Some(html.into()) },
            attachments: vec![logo],
            message_id: "plans@example.org".into(),
            ..Default::default()
        })
        .unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = s.compose.clone().unwrap();
    assert!(c.html);
    assert!(c.body.contains("From: Bob <bob@example.org>") && c.body.contains("Hi there, see this and that."), "{}", c.body);
    assert!(!c.body.contains("steal") && !c.body.contains("javascript"));
    assert!(c.styles.runs.iter().all(|r| r.style.link.as_deref().is_none_or(|l| l == "https://example.org")));
    assert_eq!(c.attachments.len(), 1, "the picture the quote shows goes with it");
    // The signature goes above the quote.
    s.mailbox.signatures.push(mailcraft_model::Signature { name: "Me".into(), text: "-- \nMe".into() });
    run(&mut s, "compose.signature", json!({"name": "Me"}));
    let c = s.compose.clone().unwrap();
    let sig = c.body.find("-- \nMe").unwrap();
    assert!(sig < c.body.find("From: Bob").unwrap());
    let start = super::super::compose::quote_start(&c).unwrap();
    assert_eq!(super::own_text(&c).chars().count(), start);
    // The owner who writes plain text answers in plain text.
    run(&mut s, "file.options", json!({"composePlain": true}));
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = s.compose.clone().unwrap();
    assert!(!c.html && c.body.contains("-----Original Message-----"));
}

#[test]
fn switching_to_plain_text_writes_the_formatting_out() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"body": "a\nb"}));
    run(&mut s, "format.bullets", json!({"start": 0, "end": 3}));
    run(&mut s, "compose.formatPlain", json!({}));
    let c = s.compose.clone().unwrap();
    assert!(!c.html && c.body == "• a\n• b");
}

#[test]
fn find_replace_count_date_symbol_and_rule() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"body": "cat Cat cat"}));
    assert_eq!(run(&mut s, "edit.find", json!({"text": "cat", "from": 1}))["start"], 4);
    assert_eq!(run(&mut s, "edit.find", json!({"text": "cat", "matchCase": true, "from": 1}))["start"], 8);
    assert_eq!(run(&mut s, "edit.find", json!({"text": "dog"}))["found"], false);
    assert_eq!(run(&mut s, "edit.replace", json!({"text": "cat", "with": "dog", "all": true}))["replaced"], 3);
    assert_eq!(s.compose.as_ref().unwrap().body, "dog dog dog");
    let n = run(&mut s, "review.wordCount", json!({}));
    assert_eq!((n["words"].as_u64(), n["characters"].as_u64()), (Some(3), Some(11)));
    run(&mut s, "insert.dateTime", json!({"at": 0, "format": "short"}));
    assert!(s.compose.as_ref().unwrap().body.starts_with("2026-10-14dog"));
    run(&mut s, "insert.symbol", json!({"symbol": "©", "at": 0}));
    assert!(s.compose.as_ref().unwrap().body.starts_with("©2026"));
    run(&mut s, "insert.rule", json!({"at": 0}));
    let c = s.compose.clone().unwrap();
    assert!(c.styles.paras[1].rule && c.body.ends_with('\n'));
    assert!(super::super::compose::body_of(&c).html.unwrap().contains("<hr>"));
}

#[test]
fn spelling_finds_the_owner_s_misspelt_words() {
    let (mut s, _) = session();
    run(&mut s, "mail.new", json!({"body": "Teh meeting is tomorow."}));
    let r = run(&mut s, "review.spelling", json!({}));
    let words: Vec<&str> = r["misspelled"].as_array().unwrap().iter().map(|m| m["word"].as_str().unwrap()).collect();
    assert_eq!(words, vec!["Teh", "tomorow"]);
    assert!(r["misspelled"][1]["suggestions"].as_array().unwrap().iter().any(|w| w == "tomorrow"));
    let r = run(&mut s, "review.spelling", json!({"text": "all good here"}));
    assert!(r["misspelled"].as_array().unwrap().is_empty());
}

#[test]
fn larger_and_smaller_step_through_the_sizes() {
    use super::step_size;
    assert_eq!(step_size(None, true), Some(16));
    assert_eq!(step_size(None, false), Some(12));
    assert_eq!(step_size(Some(12), true), None);
    assert_eq!(step_size(Some(16), false), None);
    assert_eq!(step_size(Some(32), true), Some(32));
    assert_eq!(step_size(Some(10), false), Some(10));
    assert_eq!(step_size(Some(20), true), Some(24));
}
