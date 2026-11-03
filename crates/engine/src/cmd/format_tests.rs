// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing in rich text through commands: the selection, typing, formatting, links, lists and
//! undo inside the message, and messages written before rich text.

use mailcraft_model::rich::{ListKind, Selection};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Compose, Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn compose(s: &Session) -> &Compose {
    s.compose.as_ref().unwrap()
}

/// What was written above the signature the sample account adds.
fn mine(s: &Session) -> String {
    let text = compose(s).body.plain();
    text.split("\n\nAvery Quinn").next().unwrap_or("").to_string()
}

#[test]
fn typing_and_formatting_have_their_own_undo() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    let mailbox_undo = s.can_undo();
    for c in ["Hel", "lo"] {
        run(&mut s, "compose.type", json!({"text": c}));
    }
    run(&mut s, "compose.type", json!({"text": "\nworld"}));
    assert!(compose(&s).body.plain().starts_with("Hello\nworld"));
    assert!(compose(&s).dirty);
    let r = run(&mut s, "format.bold", json!({"from": 0, "to": 5}));
    assert_eq!(r["bold"], true);
    assert!(compose(&s).body.text().all(0, 5, |st| st.bold).unwrap());
    run(&mut s, "format.size", json!({"size": 14}));
    run(&mut s, "format.bullets", json!({"from": 6}));
    assert_eq!(compose(&s).body.text().list_at(6), ListKind::Bullet);
    assert_eq!(run(&mut s, "format.numbering", json!({}))["list"], "numbered");
    run(&mut s, "compose.link", json!({"url": "https://example.org", "from": 6, "to": 11}));
    assert_eq!(compose(&s).body.text().link_span(8), Some((6, 11)));
    // Undo walks back through the message, and the mailbox's own history is untouched.
    for _ in 0..5 {
        run(&mut s, "compose.undo", json!({}));
    }
    assert!(!compose(&s).body.text().has_formatting());
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), "Hello");
    run(&mut s, "compose.redo", json!({}));
    assert_eq!(mine(&s), "Hello\nworld");
    assert_eq!(s.can_undo(), mailbox_undo);
    run(&mut s, "compose.undo", json!({}));
    run(&mut s, "compose.undo", json!({}));
    assert!(s.run("compose.undo", &json!({})).is_err());
    assert!(!s.enabled("compose.undo") && s.enabled("compose.redo"));
}

#[test]
fn deleting_and_selecting() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": "abc def"}));
    let r = run(&mut s, "compose.select", json!({"from": 7}));
    assert_eq!((r["from"].as_u64(), r["to"].as_u64()), (Some(7), Some(7)));
    run(&mut s, "compose.delete", json!({}));
    assert_eq!(mine(&s), "abc de");
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "compose.delete", json!({"forward": true}));
    run(&mut s, "compose.delete", json!({"from": 0, "to": 2}));
    assert_eq!(mine(&s), " de");
    assert_eq!(compose(&s).body.selection(), Selection::caret(0));
}

#[test]
fn hostile_positions_and_values_are_refused_and_change_nothing() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": "short"}));
    let before = compose(&s).clone();
    for (id, params) in [
        ("compose.select", json!({"from": 99})),
        ("compose.select", json!({"from": -1})),
        ("compose.select", json!({})),
        ("compose.select", json!({"from": "0"})),
        ("format.bold", json!({"from": 0, "to": 18446744073709551615u64})),
        ("format.size", json!({"size": 300})),
        ("format.size", json!({"size": "big"})),
        ("format.size", json!({})),
        ("compose.link", json!({"url": "javascript:alert(1)", "from": 0, "to": 5})),
        ("compose.link", json!({"url": 5})),
        ("compose.type", json!({})),
        ("compose.delete", json!({"from": 3, "to": 1e300})),
    ] {
        assert!(s.run(id, &params).is_err(), "{id} {params}");
        assert_eq!(compose(&s), &before, "{id} {params}");
    }
    // Control characters typed are dropped; a typed line break is a paragraph.
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "compose.type", json!({"text": "\u{0}a\u{7}\r\n"}));
    assert_eq!(mine(&s), "a\nshort");
}

#[test]
fn format_commands_need_a_message_being_written() {
    let mut s = session();
    for id in
        ["format.bold", "format.italic", "format.underline", "format.bullets", "format.numbering", "format.clear", "compose.type", "compose.undo"]
    {
        assert!(!s.enabled(id), "{id}");
        assert!(s.run(id, &json!({"text": "x"})).is_err(), "{id}");
    }
}

#[test]
fn a_message_being_written_before_rich_text_still_opens() {
    let old = json!({"mode": "reply", "to": "maya@lumen.example", "subject": "RE: x", "body": "Thanks!\n\n-----Original Message-----\nFrom: Maya"});
    let c: Compose = serde_json::from_value(old).unwrap();
    assert_eq!(c.body.plain(), "Thanks!\n\n-----Original Message-----\nFrom: Maya");
    let again: Compose = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
    assert_eq!(again.body.text(), c.body.text());
}

#[test]
fn a_plain_edit_keeps_the_formatting_around_it() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": "Hello world"}));
    run(&mut s, "format.italic", json!({"from": 6, "to": 11}));
    let edited = compose(&s).body.plain().replacen("Hello world", "Hello, world", 1);
    run(&mut s, "compose.set", json!({"body": edited}));
    assert!(compose(&s).body.text().all(7, 12, |st| st.italic).unwrap());
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), "Hello world");
}

#[test]
fn a_link_may_go_to_any_scheme_but_the_unsafe_ones() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "VPN", "body": "Open the network settings now"}));
    run(&mut s, "compose.link", json!({"url": "ms-settings:network-vpn", "from": 9, "to": 25}));
    assert!(compose(&s).body.text().link_span(10).is_some());
    for bad in ["javascript:alert(1)", "JAVASCRIPT:alert(1)", " vbscript:x", "data:text/html,x", "File:///etc/passwd", "java\tscript:x", "nonsense"] {
        let err = s.run("compose.link", &json!({"url": bad, "from": 0, "to": 4})).unwrap_err().to_string();
        assert!(err.contains("a link goes to an address like"), "{bad}: {err}");
    }
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert!(html.contains("<a href=\"ms-settings:network-vpn\">"), "{html}");
}

#[test]
fn a_formatted_message_goes_out_as_html_with_a_plain_text_part() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Plan", "body": "Agenda\nfirst\nsecond\nsee <the> site"}));
    run(&mut s, "format.bold", json!({"from": 0, "to": 6}));
    run(&mut s, "format.numbering", json!({"from": 7, "to": 19}));
    run(&mut s, "compose.link", json!({"url": "https://example.org/?a=1&b=2", "from": 30, "to": 34}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    assert!(m.body.text.starts_with("Agenda\n1. first\n2. second\nsee <the> site <https://example.org/?a=1&b=2>"), "{:?}", m.body.text);
    let html = m.body.html.clone().unwrap();
    assert!(
        html.contains("<div><b>Agenda</b></div>")
            && html.contains("<ol ")
            && html.contains("see &lt;the&gt; <a href=\"https://example.org/?a=1&amp;b=2\">site</a>"),
        "{html}"
    );
    // On the wire: both forms, as alternatives, and they read back.
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("multipart/alternative") && text.contains("text/plain") && text.contains("text/html"));
    let back = mailcraft_mime::parse(&bytes);
    assert_eq!(mailcraft_mime::rich::from_html(back.body.html.as_deref().unwrap()), mailcraft_mime::rich::from_html(&html));
}

#[test]
fn a_formatted_draft_reopens_formatted() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "\n\nbold and plain"}));
    run(&mut s, "format.bold", json!({"from": 2, "to": 6}));
    run(&mut s, "format.bullets", json!({"from": 2}));
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    let written = compose(&s).body.text().clone();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    assert_eq!(compose(&s).body.text(), &unmarked(&written));
    assert_eq!(compose(&s).format, crate::BodyFormat::Html);
    // A draft saved before rich text (plain text only) opens as plain text.
    s.mailbox.message_mut(d).unwrap().body = mailcraft_model::Body { text: "Old\ndraft".into(), html: None };
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    assert_eq!(compose(&s).body.plain(), "Old\ndraft");
    assert_eq!(compose(&s).format, crate::BodyFormat::Plain);
}

#[test]
fn pasted_html_is_reduced_to_the_set() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(
        &mut s,
        "edit.paste",
        json!({"html": "<script>alert(1)</script><p style=\"color:red\"><b>Hi</b> <a href=\"javascript:x\">there</a></p><table><tr><td><i>cell</i></td></tr></table><img src=\"https://tracker.example/p.gif\">"}),
    );
    let t = compose(&s).body.text().clone();
    assert!(t.text().starts_with("Hi there\n\ncell"), "{:?}", t.text());
    assert!(t.paragraphs().iter().flat_map(|p| &p.runs).all(|r| r.style.link.is_none()));
    assert!(t.all(0, 2, |st| st.bold).unwrap());
    // Plain text in a plain-text message, and nothing formatted can be asked for there.
    run(&mut s, "compose.formatPlain", json!({}));
    assert!(s.run("format.bold", &json!({})).is_err());
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": "<b>more</b>"}));
    assert!(compose(&s).body.plain().starts_with("moreHi"));
    assert!(!compose(&s).body.text().all(0, 4, |st| st.bold).unwrap());
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().body.html.is_none());
}

#[test]
fn a_paste_needs_something_to_paste() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    assert!(s.run("edit.paste", &json!({})).is_err());
    assert!(s.run("edit.paste", &json!({"html": 5})).is_err());
    assert!(s.run("edit.paste", &json!({"text": "x".repeat(5 << 20)})).is_err());
}

#[test]
fn html_that_comes_to_nothing_gives_way_to_the_text_and_nothing_changes_nothing() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan for Friday"}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 4}));
    let picture = "<p><img src=\"https://example.org/picture.png\"></p>";
    // A picture alone, which a message's text can't take: the text and its selection stay.
    run(&mut s, "edit.paste", json!({"html": picture}));
    assert_eq!((compose(&s).body.plain(), compose(&s).body.selection()), ("Plan for Friday".to_string(), Selection { anchor: 0, head: 4 }));
    // With a text beside it, the text.
    run(&mut s, "edit.paste", json!({"html": picture, "text": "Agenda"}));
    assert_eq!(compose(&s).body.plain(), "Agenda for Friday");
    // Nothing at all changes nothing.
    run(&mut s, "compose.select", json!({"from": 0, "to": 6}));
    run(&mut s, "edit.paste", json!({"text": ""}));
    assert_eq!((compose(&s).body.plain(), compose(&s).body.selection()), ("Agenda for Friday".to_string(), Selection { anchor: 0, head: 6 }));
}

#[test]
fn a_text_that_comes_to_nothing_once_cleaned_changes_nothing() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan for Friday"}));
    let picture = "<p><img src=\"https://example.org/picture.png\"></p>";
    // Characters a text never keeps (a NUL, the object replacement character), alone or beside a
    // picture alone: nothing is pasted, and the selection stays.
    for params in [json!({"text": "\u{0}"}), json!({"text": "\u{FFFC}"}), json!({"html": picture, "text": "\u{0}\u{FFFC}"})] {
        run(&mut s, "compose.select", json!({"from": 0, "to": 4}));
        run(&mut s, "edit.paste", params.clone());
        assert_eq!(
            (compose(&s).body.plain(), compose(&s).body.selection()),
            ("Plan for Friday".to_string(), Selection { anchor: 0, head: 4 }),
            "{params}"
        );
    }
}

#[test]
fn blank_lines_alone_in_html_are_pasted() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan for Friday"}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 4}));
    // Lines written blank on purpose are something to paste, as they were before; a picture alone
    // and a text that comes to nothing (above) are not.
    run(&mut s, "edit.paste", json!({"html": "<div><br></div><div><br></div>"}));
    let plain = compose(&s).body.plain();
    assert!(plain.ends_with(" for Friday") && plain.starts_with('\n') && !plain.contains("Plan"), "{plain:?}");
}

#[test]
fn html_that_can_be_pasted_is_pasted_beside_a_text_too_long_to_be() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan for Friday"}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 4}));
    // The text is only what is pasted when the HTML comes to nothing: it is weighed only then.
    run(&mut s, "edit.paste", json!({"html": "<b>Agenda</b>", "text": "x".repeat(5 << 20)}));
    assert_eq!(compose(&s).body.plain(), "Agenda for Friday");
    // Where it is the one pasted, it is weighed.
    run(&mut s, "compose.select", json!({"from": 0, "to": 6}));
    assert!(s.run("edit.paste", &json!({"html": "<p><img src=\"https://example.org/p.png\"></p>", "text": "x".repeat(5 << 20)})).is_err());
}

#[test]
fn a_reply_is_written_the_way_the_original_was() {
    let mut s = session();
    let plain = s.mailbox.messages.values().find(|m| m.body.html.is_none() && !m.draft).map(|m| m.id).unwrap();
    run(&mut s, "mail.reply", json!({"id": plain}));
    assert_eq!(compose(&s).format, crate::BodyFormat::Plain);
    assert!(!s.enabled("format.bold"));
    run(&mut s, "compose.formatHtml", json!({}));
    assert!(s.enabled("format.bold"));
}

#[test]
fn list_items_indent_and_the_selection_is_copied_as_html_and_text() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "a\nb"}));
    run(&mut s, "format.bullets", json!({"from": 0, "to": 3}));
    let r = run(&mut s, "format.indent", json!({"from": 2}));
    assert_eq!(r["level"], 1);
    let levels: Vec<u8> = compose(&s).body.text().paragraphs().iter().map(|p| p.level).collect();
    assert_eq!(levels, [0, 1]);
    // The selection as the clipboard takes it: the set's HTML (a list in an item) and the text.
    let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 3}));
    assert_eq!(copied["text"], "- a\n   - b");
    let html = copied["html"].as_str().unwrap();
    assert!(html.starts_with("<ul") && html.contains("<li>a\r\n<ul") && html.contains("<li>b</li>"), "{html}");
    assert_eq!(compose(&s).body.plain(), "a\nb", "copying changes nothing");
    // Out a level, then out of the list.
    assert_eq!(run(&mut s, "format.outdent", json!({"from": 2}))["level"], 0);
    assert_eq!(run(&mut s, "format.outdent", json!({"from": 2}))["list"], "none");
    // Cut takes it out; with nothing selected there is nothing to copy or cut.
    let cut = run(&mut s, "edit.cut", json!({"from": 0, "to": 1}));
    assert_eq!(cut["text"], "- a");
    assert_eq!(compose(&s).body.plain(), "\nb");
    assert!(s.run("edit.copy", &json!({})).is_err() && s.run("edit.cut", &json!({})).is_err());
    // In a plain-text message the clipboard gets text only.
    run(&mut s, "compose.formatPlain", json!({}));
    assert!(run(&mut s, "edit.copy", json!({"from": 0, "to": 2}))["html"].is_null());
    assert!(s.run("format.indent", &json!({})).is_err());
}

/// The text without its signature marked: a draft reopened from its HTML doesn't know which
/// paragraphs were the signature (R4.5.2).
pub(crate) fn unmarked(t: &mailcraft_model::RichText) -> mailcraft_model::RichText {
    mailcraft_model::RichText::from_paragraphs(
        t.paragraphs().iter().map(|p| mailcraft_model::rich::Paragraph { signature: false, ..p.clone() }).collect(),
    )
}

#[test]
fn a_link_to_any_scheme_survives_saving_and_reopening_the_draft() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "VPN", "body": "Open the network settings now"}));
    run(&mut s, "compose.link", json!({"url": "ms-settings:network-vpn", "from": 9, "to": 25}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert_eq!(compose(&s).body.text().link_span(10), Some((9, 25)));
    let link = compose(&s).body.text().paragraphs().iter().flat_map(|p| p.runs.iter()).find_map(|r| r.style.link.clone());
    assert_eq!(link.as_deref(), Some("ms-settings:network-vpn"));
}

#[test]
fn a_received_message_quoted_in_a_reply_is_still_read_by_the_reading_rule() {
    let mut s = session();
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let m = mailcraft_model::Message {
        account,
        folder: inbox,
        message_id: "settings@news.example".into(),
        from: mailcraft_model::Address::new("Harbor", "news@news.example"),
        to: vec![mailcraft_model::Address::new("Avery Quinn", "avery@lumen.example")],
        subject: "Notice".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body {
            text: "x".into(),
            html: Some("<p><a href=\"ms-settings:network-vpn\">Open</a> and <a href=\"https://example.org/\">web</a></p>".into()),
        },
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let quote = compose(&s).quote.clone().unwrap();
    let links: Vec<String> = mailcraft_html::parse(&quote.html).iter().flat_map(|b| b.spans.iter()).filter_map(|sp| sp.link.clone()).collect();
    assert_eq!(links, vec!["https://example.org/".to_string()], "{}", quote.html);
    assert!(!compose(&s).body.text().paragraphs().iter().flat_map(|p| p.runs.iter()).any(|r| r.style.link.is_some()));
}
