// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing in rich text through commands: the selection, typing, formatting, links, lists,
//! alignment and undo inside the message, a picture moved by cut and paste, and messages written
//! before rich text.

use mailcraft_model::rich::{Align, ListKind, OBJECT, Selection};
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

/// How each paragraph of the message being written is aligned.
fn aligns(s: &Session) -> Vec<Align> {
    compose(s).body.text().paragraphs().iter().map(|p| p.align).collect()
}

#[test]
fn align_text_takes_the_paragraphs_of_the_selection_or_of_from_and_to_and_answers_the_value() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Avery\nLumen\nStudio"}));
    run(&mut s, "compose.select", json!({"from": 2, "to": 8}));
    assert_eq!(run(&mut s, "format.align", json!({"value": "center"}))["value"], "center");
    assert_eq!(aligns(&s), [Align::Center, Align::Center, Align::Left]);
    assert_eq!(run(&mut s, "format.align", json!({"value": "right", "from": 13}))["value"], "right");
    assert_eq!(aligns(&s), [Align::Center, Align::Center, Align::Right]);
    assert!(compose(&s).dirty);
    // As a menu writes it, too.
    assert_eq!(run(&mut s, "format.align", json!({"value": "Left", "from": 0, "to": 18}))["value"], "left");
    assert_eq!(aligns(&s), [Align::Left; 3]);
}

#[test]
fn align_text_leaves_a_list_item_left_and_says_so() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Agenda\nfirst"}));
    run(&mut s, "format.bullets", json!({"from": 7}));
    assert_eq!(run(&mut s, "format.align", json!({"value": "center", "from": 0, "to": 12}))["value"], "center");
    assert_eq!(aligns(&s), [Align::Center, Align::Left]);
    // The item alone: it stays left, and the answer says so.
    assert_eq!(run(&mut s, "format.align", json!({"value": "right", "from": 9}))["value"], "left");
    assert_eq!(aligns(&s), [Align::Center, Align::Left]);
}

#[test]
fn a_centred_paragraph_pasted_at_the_start_of_a_list_item_leaves_it_left() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "first"}));
    run(&mut s, "format.bullets", json!({"from": 0}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": "<div style=\"text-align:center\">X</div>"}));
    assert_eq!(compose(&s).body.plain(), "Xfirst");
    assert_eq!((compose(&s).body.text().list_at(0), aligns(&s)), (ListKind::Bullet, vec![Align::Left]));
    assert_eq!(run(&mut s, "format.align", json!({"value": "left", "from": 0}))["value"], "left");
}

#[test]
fn bullets_on_a_line_and_its_break_leave_the_centred_line_after_it_as_it_was() {
    for (from, to) in [(0, 6), (6, 0)] {
        let mut s = session();
        run(&mut s, "mail.new", json!({}));
        run(&mut s, "compose.set", json!({"body": "Avery\nLumen"}));
        run(&mut s, "format.align", json!({"value": "center", "from": 6}));
        run(&mut s, "format.bullets", json!({"from": from, "to": to}));
        let text = compose(&s).body.text();
        assert_eq!(
            (text.list_at(0), text.list_at(6), aligns(&s)),
            (ListKind::Bullet, ListKind::None, vec![Align::Left, Align::Center]),
            "{from} {to}"
        );
        // Again, Avery alone is a bullet, so it stops being one.
        run(&mut s, "format.bullets", json!({"from": from, "to": to}));
        let text = compose(&s).body.text();
        assert_eq!((text.list_at(0), text.list_at(6), aligns(&s)), (ListKind::None, ListKind::None, vec![Align::Left, Align::Center]), "{from} {to}");
    }
}

#[test]
fn align_text_refuses_junk_values_and_a_plain_text_message_and_changes_nothing() {
    let mut s = session();
    assert!(!s.enabled("format.align"));
    run(&mut s, "mail.new", json!({"body": "Hello"}));
    let before = compose(&s).clone();
    for params in [json!({}), json!({"value": "justify"}), json!({"value": 5}), json!({"value": null}), json!({"value": "center", "from": 99})] {
        assert!(s.run("format.align", &params).is_err(), "{params}");
        assert_eq!(compose(&s), &before, "{params}");
    }
    let e = s.run("format.align", &json!({"value": "middle"})).unwrap_err();
    assert!(e.to_string().contains("left, center or right"), "{e}");
    run(&mut s, "compose.formatPlain", json!({}));
    let before = compose(&s).clone();
    assert!(!s.enabled("format.align"));
    assert!(s.run("format.align", &json!({"value": "center"})).is_err());
    assert_eq!(compose(&s), &before);
}

#[test]
fn a_message_switched_to_plain_text_is_sent_left_and_gets_its_alignment_back_as_html() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    run(&mut s, "format.align", json!({"value": "center", "from": 0}));
    // Plain text has no alignment to show, but the paragraph keeps it for when it is HTML again.
    run(&mut s, "compose.formatPlain", json!({}));
    assert_eq!(compose(&s).body.text().align_at(0), Align::Center);
    run(&mut s, "compose.formatHtml", json!({}));
    assert_eq!(compose(&s).body.text().align_at(0), Align::Center);
    // Sent as plain text: words alone.
    run(&mut s, "compose.formatPlain", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.body.html.is_none() && m.body.text.starts_with("Hello\n"), "{:?}", m.body);
}

#[test]
fn an_aligned_message_goes_out_with_text_align_inline() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Plan", "body": "Agenda\nfirst"}));
    run(&mut s, "compose.formatHtml", json!({}));
    run(&mut s, "format.align", json!({"value": "center", "from": 0}));
    run(&mut s, "format.bullets", json!({"from": 7}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    let html = m.body.html.clone().unwrap();
    assert!(
        html.contains("<div style=\"text-align:center\">Agenda</div>") && html.contains("<li>first</li>") && !html.contains("<li style"),
        "{html}"
    );
    // On the wire, and read back.
    let back = mailcraft_mime::parse(&mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default()));
    assert_eq!(mailcraft_mime::rich::from_html(back.body.html.as_deref().unwrap()).align_at(0), Align::Center);
}

#[test]
fn an_aligned_draft_reopens_aligned() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "\n\nCentred\nRight"}));
    run(&mut s, "compose.formatHtml", json!({}));
    run(&mut s, "format.align", json!({"value": "center", "from": 2}));
    run(&mut s, "format.align", json!({"value": "right", "from": 10}));
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    let written = compose(&s).body.text().clone();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    assert_eq!(compose(&s).body.text(), &unmarked(&written));
    assert_eq!(aligns(&s)[..4], [Align::Left, Align::Left, Align::Center, Align::Right]);
}

#[test]
fn pasted_html_keeps_its_alignment() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan"}));
    run(&mut s, "compose.select", json!({"from": 0}));
    let html = "<div style=\"text-align:center\">Lumen</div><p align=\"right\">Studio</p>";
    run(&mut s, "edit.paste", json!({"html": html}));
    assert_eq!(compose(&s).body.plain(), "Lumen\n\nStudioPlan");
    assert_eq!(aligns(&s), [Align::Center, Align::Left, Align::Right]);
    // Into a plain-text message, words alone.
    run(&mut s, "compose.discard", json!({}));
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"body": "Plan"}));
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": html}));
    assert_eq!(aligns(&s), [Align::Left; 3]);
}

#[test]
fn lines_pasted_into_a_centred_paragraph_stay_centred_as_typed_ones_do() {
    for (from, body) in [(6, "Hello a\nbworld"), (11, "Hello worlda\nb")] {
        let mut s = session();
        run(&mut s, "mail.new", json!({}));
        run(&mut s, "compose.set", json!({"body": "Hello world"}));
        run(&mut s, "format.align", json!({"value": "center", "from": 0}));
        run(&mut s, "compose.select", json!({"from": from}));
        run(&mut s, "edit.paste", json!({"text": "a\nb"}));
        assert_eq!((compose(&s).body.plain(), aligns(&s)), (body.to_string(), vec![Align::Center; 2]), "at {from}");
    }
}

#[test]
fn alignment_is_undone_inside_the_message_not_the_mailbox() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": "Avery\nLumen"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let mailbox_undo = s.can_undo();
    run(&mut s, "format.align", json!({"value": "right", "from": 0, "to": 11}));
    assert_eq!(aligns(&s)[..2], [Align::Right, Align::Right]);
    assert_eq!(s.can_undo(), mailbox_undo);
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(aligns(&s)[..2], [Align::Left, Align::Left]);
    run(&mut s, "compose.redo", json!({}));
    assert_eq!(aligns(&s)[..2], [Align::Right, Align::Right]);
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
    let links: Vec<String> =
        mailcraft_html::parse(&quote.html).iter().flat_map(|b| b.spans.iter()).filter_map(|sp| sp.link.as_deref().map(str::to_string)).collect();
    assert_eq!(links, vec!["https://example.org/".to_string()], "{}", quote.html);
    assert!(!compose(&s).body.text().paragraphs().iter().flat_map(|p| p.runs.iter()).any(|r| r.style.link.is_some()));
}

/// Lists as you type (#973): `* `, `- `, `1. ` and `1) ` at the start of a paragraph, typed with a
/// space from the keyboard (`autoList`), start a list; Undo puts the marker back; nothing else does.
#[test]
fn a_typed_marker_and_space_start_a_list_and_undo_puts_the_marker_back() {
    let key = |s: &mut Session, text: &str| {
        for c in text.chars() {
            let c = c.to_string();
            let params = if c == " " { json!({"text": c, "autoList": true}) } else { json!({"text": c}) };
            run(s, "compose.type", params);
        }
    };
    for (marker, kind) in [("* ", ListKind::Bullet), ("- ", ListKind::Bullet), ("1. ", ListKind::Numbered), ("1) ", ListKind::Numbered)] {
        let mut s = session();
        run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
        key(&mut s, marker);
        assert_eq!(compose(&s).body.text().list_at(0), kind, "{marker}");
        assert_eq!(compose(&s).body.selection(), Selection::caret(0), "{marker}");
        key(&mut s, "Milk");
        assert_eq!(mine(&s), "Milk", "{marker}");
        // Undo: the word, then the list, which leaves the marker as it was typed.
        run(&mut s, "compose.undo", json!({}));
        run(&mut s, "compose.undo", json!({}));
        assert_eq!(compose(&s).body.text().list_at(0), ListKind::None, "{marker}");
        assert_eq!(mine(&s), marker, "{marker}");
    }
    let stays = |setup: &dyn Fn(&mut Session), typed: &str, expect: &str| {
        let mut s = session();
        run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
        setup(&mut s);
        key(&mut s, typed);
        assert_eq!(compose(&s).body.text().list_at(0), ListKind::None, "{typed}");
        assert_eq!(mine(&s), expect, "{typed}");
    };
    // Not at the start of the paragraph, another marker, or a marker longer than the list's.
    stays(&|_| {}, "a * b", "a * b");
    stays(&|_| {}, "2. x", "2. x");
    stays(&|_| {}, "** x", "** x");
    stays(&|_| {}, "+ x", "+ x");
    // The owner turned it off.
    stays(&|s| drop(run(s, "file.options", json!({"autoLists": false}))), "* x", "* x");
    // A plain-text message.
    stays(&|s| drop(run(s, "compose.formatPlain", json!({}))), "* x", "* x");
    // A command without the keyboard's word (an agent, a script) types it as text.
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "compose.type", json!({"text": "* "}));
    run(&mut s, "compose.type", json!({"text": " "}));
    assert_eq!(compose(&s).body.text().list_at(0), ListKind::None);
    // Inside a list item already: the marker stays text.
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "format.bullets", json!({}));
    key(&mut s, "* ");
    assert_eq!(mine(&s), "* ");
}

/// A picture's bytes, told apart by `n`.
fn png(n: u8) -> String {
    let mut bytes = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    bytes.push(n);
    mailcraft_charset::base64::STANDARD.encode(&bytes)
}

/// A new message in HTML, `Hello world` with a picture (picture `n`, described `alt`, 120 pixels
/// wide) after `Hello`; the picture's id.
fn with_picture(s: &mut Session, n: u8, alt: &str) -> u32 {
    run(s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello world"}));
    run(s, "compose.formatHtml", json!({}));
    let id = run(s, "insert.picture", json!({"data": png(n), "file": "chart.png", "alt": alt, "width": 120, "at": 5}))["id"].as_u64();
    u32::try_from(id.unwrap()).unwrap()
}

/// The Content-ID of the first picture `html` shows.
fn cid_in(html: &str) -> String {
    let at = html.find("cid:").unwrap() + 4;
    html[at..].split('"').next().unwrap().to_string()
}

#[test]
fn cutting_a_picture_alone_answers_its_description_as_text_and_pasting_it_back_moves_it_and_it_is_sent_once() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 6}));
    // A program that takes text gets the description, so a paste of it is never of nothing.
    assert_eq!(cut["text"], "[Sales chart]");
    let html = cut["html"].as_str().unwrap().to_string();
    assert!(html.contains("<img src=\"cid:pic-") && html.contains("@mailcraft.invalid\"") && html.contains("alt=\"Sales chart\""), "{html}");
    assert_eq!(mine(&s), "Hello world");
    // Pasted after "world", it is the same picture, moved.
    run(&mut s, "compose.select", json!({"from": 11}));
    run(&mut s, "edit.paste", json!({"html": html, "text": cut["text"]}));
    assert_eq!(mine(&s), format!("Hello world{OBJECT}"));
    assert_eq!((compose(&s).body.text().pictures(), compose(&s).pictures.len()), (vec![id], 1));
    // It goes out once, where it went, as wide as it was.
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    let parts: Vec<_> = m.attachments.iter().filter(|a| a.inline && a.content_id.starts_with("pic-")).collect();
    let html = m.body.html.as_deref().unwrap();
    assert_eq!((parts.len(), html.matches("<img").count()), (1, 1), "{html}");
    assert!(html.contains(&format!("world<img src=\"cid:{}\" width=\"120\"", parts[0].content_id)), "{html}");
}

#[test]
fn a_picture_copied_from_another_message_with_the_same_id_is_not_replaced_by_this_ones() {
    let mut s = session();
    let first = with_picture(&mut s, 1, "Sales chart");
    let copied = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    run(&mut s, "compose.discard", json!({}));
    // The next message's first picture has the same id, and other bytes.
    let second = with_picture(&mut s, 2, "Team photo");
    assert_eq!(first, second);
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    // What can be had of it is its description; the photo is not shown a second time.
    assert_eq!(mine(&s), format!("[Sales chart]Hello{OBJECT} world"));
    assert_eq!(compose(&s).body.text().pictures(), [second]);
}

#[test]
fn a_pasted_img_naming_a_picture_the_message_does_not_hold_is_left_out() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let held = cid_in(run(&mut s, "edit.copy", json!({"from": 5, "to": 6}))["html"].as_str().unwrap());
    // Another id with this picture's bytes, and this id with other bytes.
    let (stem, rest) = held.split_once('.').unwrap();
    let other_id = format!("pic-{}.{rest}", id + 1);
    let other_bytes = format!("{stem}.0123456789abcdef@mailcraft.invalid");
    let pasted = format!("<div>see <img src=\"cid:{held}\"> here</div><div><img src=\"cid:{other_id}\"><img src=\"cid:{other_bytes}\"></div>");
    run(&mut s, "compose.select", json!({"from": 12}));
    run(&mut s, "edit.paste", json!({"html": pasted}));
    assert_eq!(mine(&s), format!("Hello{OBJECT} worldsee {OBJECT} here"));
    assert_eq!(compose(&s).body.text().pictures(), [id, id]);
}

#[test]
fn a_signatures_picture_cut_from_the_message_is_not_pasted_back_and_the_mail_is_sent_whole() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Logo", "text": "Avery Quinn", "default": true}));
    run(&mut s, "signature.picture.add", json!({"name": "Logo", "data": png(1), "file": "lumen-logo.png", "alt": "Lumen Studio logo"}));
    // The logo as the open signature names it.
    run(&mut s, "signature.open", json!({"name": "Logo"}));
    let from_signature = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}));
    run(&mut s, "signature.close", json!({}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let at = compose(&s).body.plain().chars().position(|c| c == OBJECT).unwrap();
    // The message's copy names only the message's own pictures: a signature's goes out only in
    // its signature.
    let cut = run(&mut s, "edit.cut", json!({"from": at, "to": at + 1}));
    assert!(!cut["html"].as_str().unwrap().contains("<img"), "{cut}");
    run(&mut s, "compose.select", json!({"from": 5}));
    run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    // Nor is it taken back as the signature names it: its description stands for it.
    run(&mut s, "edit.paste", json!({"html": from_signature["html"], "text": from_signature["text"]}));
    assert!(compose(&s).body.text().pictures().is_empty());
    assert!(compose(&s).body.plain().starts_with("Hello[Lumen Studio logo]\n\nAvery Quinn"), "{:?}", compose(&s).body.plain());
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    let html = m.body.html.as_deref().unwrap();
    assert!(m.attachments.is_empty() && !html.contains("cid:") && html.contains("Avery Quinn"), "{html}");
}

#[test]
fn a_picture_alone_pasted_into_a_plain_message_gives_its_description() {
    let mut s = session();
    with_picture(&mut s, 1, "Sales chart");
    let copied = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    run(&mut s, "compose.discard", json!({}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "See "}));
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 4}));
    run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    assert_eq!(mine(&s), "See [Sales chart]");
    assert!(compose(&s).body.text().pictures().is_empty());
}

#[test]
fn a_picture_pasted_with_text_into_a_plain_message_is_its_description_where_it_stood() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 12}));
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 12}));
    run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    assert_eq!(mine(&s), format!("Hello{OBJECT} worldHello[Sales chart] world"));
    assert_eq!(compose(&s).body.text().pictures(), [id]);
}

#[test]
fn a_paste_naming_a_held_picture_more_often_than_a_message_holds_is_refused() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let html = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}))["html"].as_str().unwrap().to_string();
    let start = html.find("<img").unwrap();
    let tag = &html[start..start + html[start..].find('>').unwrap() + 1];
    let pasted = format!("<div>{}</div>", tag.repeat(super::format::MAX_PASTE / tag.len() - 1));
    assert!(pasted.len() <= super::format::MAX_PASTE);
    run(&mut s, "compose.select", json!({"from": 12}));
    let before = compose(&s).body.text().clone();
    let e = s.run("edit.paste", &json!({"html": pasted, "text": "x"})).unwrap_err();
    assert!(e.to_string().contains("at most 30 pictures"), "{e}");
    assert_eq!((compose(&s).body.text(), compose(&s).body.text().pictures()), (&before, vec![id]));
}

#[test]
fn a_picture_pasted_as_often_as_a_message_holds_is_cut_and_pasted_back_whole_and_no_more_go_in() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let copied = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    let most = mailcraft_model::pictures::MAX_PICTURES;
    for _ in 1..most {
        run(&mut s, "compose.select", json!({"from": 5}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    }
    assert_eq!(compose(&s).body.text().pictures(), vec![id; most]);
    // Each time the text shows a picture counts: one more is refused, pasted or new, and changes
    // nothing.
    let before = compose(&s).body.text().clone();
    let e = s.run("edit.paste", &json!({"html": copied["html"], "text": copied["text"]})).unwrap_err();
    assert!(e.to_string().contains("at most 30 pictures"), "{e}");
    let e = s.run("insert.picture", &json!({"data": png(2), "file": "photo.png"})).unwrap_err();
    assert!(e.to_string().contains("at most 30 pictures"), "{e}");
    assert_eq!(compose(&s).body.text(), &before);
    // So all of them cut come back, every one.
    let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 5 + most}));
    assert!(compose(&s).body.text().pictures().is_empty());
    run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    assert_eq!(compose(&s).body.text().pictures(), vec![id; most]);
}

#[test]
fn a_new_picture_over_one_selected_when_the_message_shows_as_many_as_it_holds_takes_its_place_and_one_put_at_a_place_is_refused() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    let copied = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    let most = mailcraft_model::pictures::MAX_PICTURES;
    for _ in 1..most {
        run(&mut s, "compose.select", json!({"from": 5}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    }
    // Over one of them, a new picture takes its place: as many are shown as before.
    run(&mut s, "compose.select", json!({"from": 5, "to": 6}));
    let new = run(&mut s, "insert.picture", json!({"data": png(2), "file": "photo.png"}))["id"].as_u64().unwrap();
    let shown = compose(&s).body.text().pictures();
    assert_eq!(shown.len(), most);
    assert_eq!((shown[0], shown.iter().filter(|p| **p == id).count()), (u32::try_from(new).unwrap(), most - 1));
    // Put at a place, it is one more, whatever is selected.
    run(&mut s, "compose.select", json!({"from": 5, "to": 6}));
    let before = compose(&s).body.text().clone();
    let e = s.run("insert.picture", &json!({"data": png(3), "file": "map.png", "at": 0})).unwrap_err();
    assert!(e.to_string().contains("at most 30 pictures"), "{e}");
    assert_eq!(compose(&s).body.text(), &before);
}

#[test]
fn a_picture_of_the_message_pasted_or_put_in_its_signature_is_refused_and_the_mail_is_sent_and_kept_whole() {
    let mut s = session();
    let id = with_picture(&mut s, 1, "Sales chart");
    // The picture alone, and amid text.
    let alone = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    let amid = run(&mut s, "edit.cut", json!({"from": 0, "to": 8}));
    assert_eq!(amid["text"], "Hello[Sales chart] w");
    // The signature goes out with its own pictures only, so the message's do not go in there,
    // and nothing of what is pasted is lost unsaid.
    let at = compose(&s).body.plain().find("Avery").unwrap() + "Avery".len();
    run(&mut s, "compose.select", json!({"from": at}));
    let before = compose(&s).body.text().clone();
    for pasted in [&alone, &amid] {
        let e = s.run("edit.paste", &json!({"html": pasted["html"], "text": pasted["text"]})).unwrap_err();
        assert!(e.to_string().contains("its own pictures only"), "{e}");
        assert_eq!(compose(&s).body.text(), &before);
    }
    let e = s.run("insert.picture", &json!({"data": png(2), "file": "photo.png"})).unwrap_err();
    assert!(e.to_string().contains("its own pictures only"), "{e}");
    assert_eq!((compose(&s).body.text(), compose(&s).pictures.len()), (&before, 1));
    // Pasted back above it, it is whole; a draft keeps it so, and the mail sent shows it once.
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": amid["html"], "text": amid["text"]}));
    assert_eq!(mine(&s), format!("Hello{OBJECT} world"));
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    assert_eq!((mine(&s), compose(&s).body.text().pictures()), (format!("Hello{OBJECT} world"), vec![id]));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    let html = m.body.html.as_deref().unwrap();
    assert_eq!((m.attachments.len(), html.matches("<img").count()), (1, 1), "{html}");
    assert!(html.contains(&format!("Hello<img src=\"cid:{}\"", m.attachments[0].content_id)) && html.contains("Avery Quinn"), "{html}");
}

#[test]
fn a_new_picture_over_one_selected_when_the_message_holds_as_many_different_pictures_as_it_may_is_refused_and_changes_nothing() {
    let mut s = session();
    with_picture(&mut s, 1, "Sales chart");
    let most = mailcraft_model::pictures::MAX_PICTURES;
    for n in 2..=most {
        run(&mut s, "insert.picture", json!({"data": png(u8::try_from(n).unwrap()), "file": "chart.png", "at": 5}));
    }
    assert_eq!((compose(&s).body.text().pictures().len(), compose(&s).pictures.len()), (most, most));
    // Over one of them, a new one would let go of the one it replaces, which undo could not bring
    // back: refused, as any new picture at the cap is, and nothing changes.
    run(&mut s, "compose.select", json!({"from": 5, "to": 6}));
    let (text, held) = (compose(&s).body.text().clone(), compose(&s).pictures.clone());
    let e = s.run("insert.picture", &json!({"data": png(0), "file": "photo.png"})).unwrap_err();
    assert!(e.to_string().contains("at most 30 pictures"), "{e}");
    assert_eq!((compose(&s).body.text(), &compose(&s).pictures), (&text, &held));
    // It goes out with all of them, each once.
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert_eq!(m.attachments.iter().filter(|a| a.inline && a.content_id.starts_with("pic-")).count(), most);
}

#[test]
fn a_picture_cut_from_a_centred_line_and_pasted_inside_a_right_aligned_one_leaves_it_right() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello world\nRight side"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "insert.picture", json!({"data": png(1), "file": "chart.png", "alt": "Sales chart", "width": 120, "at": 5}))["id"].as_u64();
    run(&mut s, "format.align", json!({"value": "center", "from": 0, "to": 1}));
    run(&mut s, "format.align", json!({"value": "right", "from": 13, "to": 14}));
    let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 6}));
    // The copy says the line it came from was centred, as any copy of a line's part does.
    assert!(cut["html"].as_str().unwrap().contains("text-align:center"), "{cut}");
    // Pasted between "Right" and " side", it joins that line, which stays right.
    run(&mut s, "compose.select", json!({"from": 17}));
    run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    assert_eq!(mine(&s), format!("Hello world\nRight{OBJECT} side"));
    assert_eq!(aligns(&s)[..2], [Align::Center, Align::Right]);
    assert_eq!(compose(&s).body.text().pictures(), [u32::try_from(id.unwrap()).unwrap()]);
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    let html = m.body.html.as_deref().unwrap();
    assert!(html.contains(&format!("<div style=\"text-align:right\">Right<img src=\"cid:{}\"", m.attachments[0].content_id)), "{html}");
}

#[test]
fn a_paste_whose_pictures_come_past_the_most_paragraphs_a_text_holds_is_not_refused_and_shows_none_of_them() {
    use mailcraft_model::rich::MAX_PARAGRAPHS;
    let mut s = session();
    run(&mut s, "signature.defaults", json!({"new": ""}));
    let id = with_picture(&mut s, 1, "Sales chart");
    let html = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}))["html"].as_str().unwrap().to_string();
    let start = html.find("<img").unwrap();
    let tag = &html[start..start + html[start..].find('>').unwrap() + 1];
    // More pictures than there is room for, after as many lines as a text holds: none of them is
    // pasted, so none counts.
    let pasted = format!("<pre>{}</pre><div>{}</div>", "x\n".repeat(MAX_PARAGRAPHS), tag.repeat(mailcraft_model::pictures::MAX_PICTURES));
    run(&mut s, "compose.select", json!({"from": 0}));
    let pasted = s.run("edit.paste", &json!({"html": pasted})).map(|_| ()).map_err(|e| e.to_string());
    assert_eq!(pasted, Ok(()));
    assert_eq!((compose(&s).body.text().paragraphs().len(), compose(&s).body.text().pictures()), (MAX_PARAGRAPHS, vec![id]));
}

#[test]
fn a_picture_copied_from_another_message_with_text_around_it_is_its_description_where_it_stood_in_a_plain_message() {
    let mut s = session();
    for (n, alt, said) in [(1, "Sales chart", "[Sales chart]"), (2, "", "[picture]")] {
        with_picture(&mut s, n, alt);
        let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 12}));
        run(&mut s, "compose.discard", json!({}));
        run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
        run(&mut s, "compose.formatPlain", json!({}));
        run(&mut s, "compose.select", json!({"from": 0}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
        // As a copy of it in its own message says it, and as the text beside the HTML says it.
        let said = format!("Hello{said} world");
        assert_eq!((mine(&s), copied["text"].as_str()), (said.clone(), Some(said.as_str())));
        run(&mut s, "compose.discard", json!({}));
    }
}

#[test]
fn a_list_and_a_link_pasted_into_a_plain_message_are_the_text_a_copy_of_them_gives() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 0}));
    let html = "<ul><li>one<ul><li>two</li></ul></li></ul><ol><li>first</li><li>second</li></ol><div>see <a href=\"https://example.org/plan\">the plan</a></div>";
    run(&mut s, "edit.paste", json!({"html": html}));
    assert_eq!(mine(&s), "- one\n   - two\n\n1. first\n2. second\n\nsee the plan <https://example.org/plan>");
}

#[test]
fn a_pasted_img_one_or_two_pixels_wide_is_left_out_as_the_reading_pane_hides_it() {
    for width in [1, 2] {
        let mut s = session();
        let id = with_picture(&mut s, 1, "Sales chart");
        let held = cid_in(run(&mut s, "edit.copy", json!({"from": 5, "to": 6}))["html"].as_str().unwrap());
        let pasted = format!("<div>see <img src=\"cid:{held}\" width=\"{width}\" alt=\"Chart\"> here</div>");
        run(&mut s, "compose.select", json!({"from": 12}));
        run(&mut s, "edit.paste", json!({"html": pasted}));
        assert_eq!((mine(&s), compose(&s).body.text().pictures()), (format!("Hello{OBJECT} worldsee here"), vec![id]), "width {width}");
        // Nor is it said in a plain-text message.
        run(&mut s, "compose.formatPlain", json!({}));
        run(&mut s, "compose.select", json!({"from": 0}));
        run(&mut s, "edit.paste", json!({"html": pasted}));
        assert!(mine(&s).starts_with("see hereHello"), "width {width}: {}", mine(&s));
    }
}

#[test]
fn a_picture_described_by_a_space_alone_pasted_into_a_plain_message_is_said_as_a_picture() {
    // One of another message, whose description the plain text would empty.
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": "<div>before<img src=\"cid:other@x\" alt=\" \">after</div>"}));
    assert_eq!(mine(&s), "before[picture]after");
}

#[test]
fn a_picture_described_by_an_object_character_alone_cut_alone_is_said_as_a_picture() {
    // The plain text takes the character out of a description; the cut's text still says it.
    let mut s = session();
    with_picture(&mut s, 1, "\u{FFFC}");
    let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 6}));
    assert_eq!(cut["text"], "[picture]");
}

#[test]
fn a_paste_into_a_plain_message_whose_picture_is_past_the_most_characters_keeps_the_text_before_it() {
    let html = format!("<div>Heading</div><div>{}<img src=\"cid:foreign@example.org\" alt=\"Chart\"></div>", "x".repeat(1 << 20));
    for params in [json!({"html": html}), json!({"html": html, "text": "fallback"})] {
        let mut s = session();
        run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
        run(&mut s, "compose.formatPlain", json!({}));
        // Over the whole text, the signature too: the paste is as long as a message may be.
        let end = compose(&s).body.text().len();
        run(&mut s, "compose.select", json!({"from": 0, "to": end}));
        run(&mut s, "edit.paste", params);
        let max = mailcraft_model::rich::MAX_CHARS;
        assert_eq!(compose(&s).body.plain(), format!("Heading\n{}", "x".repeat(max - 8)));
    }
}

/// The paragraphs of the message being written: whether each is the signature's, and its text.
fn paragraphs(s: &Session) -> Vec<(bool, String)> {
    compose(s).body.text().paragraphs().iter().map(|p| (p.signature, p.runs.iter().map(|r| r.text.as_str()).collect())).collect()
}

#[test]
fn a_line_with_a_picture_is_not_joined_onto_the_signature_above_it_and_the_picture_still_moves() {
    let mut s = session();
    with_picture(&mut s, 1, "Sales chart");
    // The picture cut, and pasted on a line of its own after the signature.
    let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 6}));
    let end = compose(&s).body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": "\n"}));
    run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    let before = paragraphs(&s);
    assert_eq!(before.last(), Some(&(false, OBJECT.to_string())), "{before:?}");
    // Backspace at the start of that line, and Delete at the end of the signature, would join it
    // onto the signature's last line, and the picture would go out as the signature's (or not at
    // all) and copy as nothing: both are refused, and nothing changes.
    let line = compose(&s).body.text().len() - 1;
    run(&mut s, "compose.select", json!({"from": line}));
    let back = s.run("compose.delete", &json!({}));
    assert!(back.is_err_and(|e| e.to_string().contains("would put the picture into the signature")));
    run(&mut s, "compose.select", json!({"from": line - 1}));
    assert!(s.run("compose.delete", &json!({"forward": true})).is_err());
    // Nor does a selection over the break, typed over.
    run(&mut s, "compose.select", json!({"from": line - 3, "to": line}));
    assert!(s.run("compose.type", &json!({"text": "x"})).is_err());
    assert_eq!(paragraphs(&s), before);
    // The picture is still the message's own: it copies as itself.
    let copied = run(&mut s, "edit.copy", json!({"from": line, "to": line + 1}));
    assert!(copied["html"].as_str().unwrap().contains("<img src=\"cid:pic-"), "{copied}");
    assert_eq!(copied["text"], "[Sales chart]");
    // A line of text after the signature still joins onto it, as before.
    run(&mut s, "compose.select", json!({"from": line, "to": line + 1}));
    run(&mut s, "compose.type", json!({"text": "PS"}));
    run(&mut s, "compose.select", json!({"from": line}));
    run(&mut s, "compose.delete", json!({}));
    let joined = paragraphs(&s);
    assert!(joined.last().is_some_and(|(signature, text)| *signature && text.ends_with("PS")) && joined.len() + 1 == before.len(), "{joined:?}");
}
