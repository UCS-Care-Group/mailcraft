// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Replies and forwards of HTML mail: the original goes out below the owner's text with its look
//! and without anything in it that runs, embeds or tracks, a draft keeps it, and a plain-text
//! original is still quoted in the body.

use std::sync::Arc;

use mailcraft_model::{Address, Attachment, FolderKind, Id, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::compose::QUOTE_MARK;
use crate::{BodyFormat, Compose, Session, sample};

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

/// A newsletter laid out with a style sheet, a table and a picture of its own.
const NEWSLETTER: &str = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>.col{width:50%}</style><title>x</title></head>\
<BODY bgcolor=\"#ffffff\"><table class=\"grid\"><tr><td class=\"col\"><h1>October</h1><img src=\"cid:logo@news.example\"></td>\
<td class=\"col\"><p style=\"color:#336\">Five <b>new</b> arrivals</p></td></tr></table></BODY></html>";

const BODY: &str = "<table class=\"grid\"><tr><td class=\"col\"><h1>October</h1><img src=\"cid:logo@news.example\"></td>\
<td class=\"col\"><p style=\"color:#336\">Five <b>new</b> arrivals</p></td></tr></table>";

fn newsletter(s: &mut Session) -> Id {
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let logo = Attachment { content_id: "logo@news.example".into(), inline: true, ..Attachment::new("logo.png", "image/png", vec![137, 80, 78, 71]) };
    let menu = Attachment::new("menu.pdf", "application/pdf", vec![1, 2, 3]);
    let m = Message {
        account,
        folder: inbox,
        message_id: "news.1@news.example".into(),
        from: Address::new("Harbor <News> & Co", "news@news.example"),
        to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
        subject: "October <arrivals>".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: "October\n\nFive new arrivals".into(), html: Some(NEWSLETTER.into()) },
        attachments: vec![logo, menu],
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn a_reply_carries_the_original_html_with_its_look() {
    let mut s = session();
    let id = newsletter(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    assert_eq!(c.format, BodyFormat::Html);
    let q = c.quote.clone().unwrap();
    // The owner's text is only theirs (and the signature); the original is below, not in it.
    assert!(!c.body.plain().contains("Original Message") && !c.body.plain().contains("arrivals"));
    assert!(q.html.starts_with(QUOTE_MARK));
    // Its layout, classes, inline styles and picture are carried; its head and style sheet are not.
    assert!(q.html.contains(BODY), "{}", q.html);
    assert!(!q.html.contains("<style") && !q.html.contains(".col{"), "{}", q.html);
    // Its header lines are escaped.
    assert!(
        q.html.contains("<b>From:</b> &quot;Harbor &lt;News&gt; &amp; Co&quot; &lt;news@news.example&gt;")
            || q.html.contains("<b>From:</b> Harbor &lt;News&gt; &amp; Co"),
        "{}",
        q.html
    );
    assert!(q.html.contains("<b>Subject:</b> October &lt;arrivals&gt;"));
    assert!(!q.html.contains("<title>") && !q.html.contains("<BODY"));
    assert_eq!(q.pictures.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["logo.png"]);
    assert!(c.attachments.is_empty());

    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "compose.type", json!({"text": "Thanks, "}));
    run(&mut s, "format.bold", json!({"from": 0, "to": 6}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap().clone();
    let html = m.body.html.clone().unwrap();
    let mine = html.find("<b>Thanks</b>").unwrap();
    let theirs = html.find(QUOTE_MARK).unwrap();
    assert!(mine < theirs && html[theirs..].contains(BODY));
    assert!(
        m.body.text.starts_with("Thanks, ")
            && m.body.text.contains("-----Original Message-----\nFrom: ")
            && m.body.text.ends_with("Five new arrivals\n")
    );
    // The picture the quote shows travels with it, in place; the original's other file doesn't.
    assert_eq!(m.attachments.iter().map(|a| (a.name.as_str(), a.inline)).collect::<Vec<_>>(), [("logo.png", true)]);
    assert!(!m.has_attachments());
    // On the wire, the picture is beside the HTML that shows it.
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let wire = String::from_utf8_lossy(&bytes);
    assert!(wire.contains("multipart/related") && wire.contains("Content-ID: <logo@news.example>"), "{wire}");
    assert!(s.mailbox.message(id).unwrap().answered);
}

/// An original that tries to run, track and hide things, with a picture of its own in each place.
const HOSTILE: &str = "<html><head><style>body{background:url(https://evil.example/bg.gif)}</style>\
<script>fetch('https://evil.example/open')</script></head><body onload=\"x()\">\
<p onclick=\"steal()\" style=\"color:#336\">Kept <a href=\"javascript:alert(1)\">link</a> \
<a href=\"https://shop.example/\">shop</a></p>\
<img src=\"https://evil.example/pixel.gif\" width=\"1\" height=\"1\"><img src=\"cid:logo@news.example\" alt=\"logo\">\
<div style=\"display:none\">hidden <img src=\"cid:secret@news.example\"></div>\
<script>document.write('<img src=cid:written@news.example>')</script>\
<iframe src=\"https://evil.example/frame\"></iframe><form action=\"https://evil.example/c\"><input name=a></form></body></html>";

#[test]
fn a_reply_carries_nothing_that_runs_tracks_or_was_hidden() {
    let mut s = session();
    let id = newsletter(&mut s);
    let secret = Attachment { content_id: "secret@news.example".into(), inline: true, ..Attachment::new("secret.png", "image/png", vec![137, 80]) };
    s.mailbox.message_mut(id).unwrap().attachments.push(secret);
    s.mailbox.message_mut(id).unwrap().body.html = Some(HOSTILE.into());
    run(&mut s, "mail.reply", json!({"id": id}));
    let q = compose(&s).quote.clone().unwrap();
    let lower = q.html.to_ascii_lowercase();
    for gone in ["<script", "<style", "<iframe", "<form", "<input", "onload", "onclick", "javascript:", "pixel.gif", "evil.example", "hidden"] {
        assert!(!lower.contains(gone), "{gone}: {}", q.html);
    }
    for kept in [
        "<p style=\"color:#336\">Kept <a>link</a> <a href=\"https://shop.example/\">shop</a></p>",
        "<img src=\"cid:logo@news.example\" alt=\"logo\">",
    ] {
        assert!(q.html.contains(kept), "{kept}: {}", q.html);
    }
    // Only the picture the quote still shows goes with the reply, not the hidden one.
    assert_eq!(q.pictures.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["logo.png"]);
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(sent).unwrap().body.html.clone().unwrap().to_ascii_lowercase();
    assert!(!html.contains("<script") && !html.contains("evil.example") && !html.contains("onclick"), "{html}");
}

#[test]
fn a_draft_carrying_an_original_as_it_came_reopens_without_its_scripts() {
    let mut s = session();
    let id = newsletter(&mut s);
    // A draft from an older MailCraft, or from anywhere: the original below the mark as it came.
    let drafts = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Drafts).unwrap();
    let mut draft = s.mailbox.message(id).unwrap().clone();
    draft.folder = drafts;
    draft.draft = true;
    draft.body.html =
        Some(format!("<html><body><p>Mine</p>{QUOTE_MARK}<b>From:</b> x<br><script>alert(1)</script><p onclick=y>Theirs</p></div></body></html>"));
    let d = s.mailbox.add_message(draft).unwrap();
    run(&mut s, "mail.editDraft", json!({"id": d}));
    let q = compose(&s).quote.clone().unwrap();
    assert_eq!(q.html, format!("{QUOTE_MARK}<b>From:</b> x<br><p>Theirs</p></div>"));
}

#[test]
fn the_quoted_original_reads_through_the_html_reader() {
    let mut s = session();
    let id = newsletter(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    let q = compose(&s).quote.clone().unwrap();
    let text = mailcraft_html::to_text(&q.html);
    assert!(text.contains("From:") && text.contains("Five new arrivals"), "{text}");
}

#[test]
fn a_forward_carries_every_attachment_and_the_original() {
    let mut s = session();
    let id = newsletter(&mut s);
    run(&mut s, "mail.forward", json!({"id": id}));
    let c = compose(&s).clone();
    assert!(c.quote.as_ref().is_some_and(|q| q.pictures.is_empty() && q.html.contains(BODY)));
    assert_eq!(c.attachments.len(), 2);
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(sent).unwrap().attachments.len(), 2);
}

#[test]
fn a_draft_of_a_reply_keeps_the_original_below() {
    let mut s = session();
    let id = newsletter(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "compose.type", json!({"text": "Lovely"}));
    run(&mut s, "format.italic", json!({"from": 0, "to": 6}));
    let before = compose(&s).clone();
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    let drafts = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Drafts).unwrap();
    assert_eq!(s.mailbox.message(d).unwrap().folder, drafts);
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    let c = compose(&s).clone();
    assert_eq!(c.body.text(), &super::format_tests::unmarked(before.body.text()));
    let (q, was) = (c.quote.clone().unwrap(), before.quote.clone().unwrap());
    assert_eq!(q.html, was.html);
    assert_eq!(q.text, was.text);
    assert_eq!(q.pictures.len(), 1);
    assert!(c.attachments.is_empty());
    // Sent from the draft, it is the same message.
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(sent).unwrap().body.html.clone().unwrap();
    assert_eq!(html.matches(QUOTE_MARK).count(), 1);
    assert!(html.contains("<i>Lovely</i>") && html.contains(BODY));
}

#[test]
fn a_plain_text_reply_is_as_it_was() {
    let mut s = session();
    let plain = s.mailbox.messages.values().find(|m| m.body.html.is_none() && !m.draft && m.subject == "Release checklist").map(|m| m.id).unwrap();
    run(&mut s, "mail.reply", json!({"id": plain}));
    let c = compose(&s).clone();
    assert!(c.quote.is_none() && c.format == BodyFormat::Plain);
    assert!(c.body.plain().contains("-----Original Message-----"));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert!(m.body.html.is_none() && m.body.text.contains("Freeze the main branch"));
}

#[test]
fn as_plain_text_the_original_goes_as_text() {
    let mut s = session();
    let id = newsletter(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.formatPlain", json!({}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert!(m.body.html.is_none() && m.body.text.contains("Five new arrivals") && m.attachments.is_empty());
}

#[test]
fn originals_of_any_shape_are_carried_without_panicking() {
    let mut s = session();
    for html in [
        "",
        "<body",
        "<BODY>x",
        "</body><body>",
        "<style>",
        "<style><body>x</body>",
        "<html><head><style>a</style>",
        "é<body\u{0}>ü</body>",
        "<body>\r\nBcc: x@example.org\r\n</body>",
        "<script>",
        "<div hidden>",
        "<img src=cid:",
        "<a href=\"javascript:",
    ] {
        let id = newsletter(&mut s);
        s.mailbox.message_mut(id).unwrap().body.html = Some(html.into());
        run(&mut s, "mail.reply", json!({"id": id}));
        let q = compose(&s).quote.clone().unwrap();
        assert!(q.html.starts_with(QUOTE_MARK), "{html:?}");
        let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        let m = Arc::new(s.mailbox.message(sent).unwrap().clone());
        let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
        let head_end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        assert!(!String::from_utf8_lossy(&bytes[..head_end]).to_ascii_lowercase().contains("\r\nbcc:"), "{html:?}");
    }
}

#[test]
fn a_reply_quotes_in_the_interface_language_and_its_draft_reopens_in_any() {
    use mailcraft_i18n::{Lang, test_support};
    let fr = test_support::po(&[
        ("-----Original Message-----", "-----Message d'origine-----"),
        ("From:", "De\u{202f}:"),
        ("Sent:", "Envoyé\u{202f}:"),
        ("To:", "À\u{202f}:"),
        ("Subject:", "Objet\u{202f}:"),
        ("{weekday}, {month} {day}, {year}", "{weekday} {day} {month} {year}"),
        ("Saturday", "samedi"),
        ("October", "octobre"),
    ]);
    test_support::with_catalogue(Lang::French, &fr, || {
        let mut s = session();
        s.interface_language = Lang::French;
        s.prefs.twelve_hour = false;
        let id = newsletter(&mut s);
        run(&mut s, "mail.reply", json!({"id": id}));
        let was = compose(&s).quote.clone().unwrap();
        assert!(was.text.starts_with("\n\n-----Message d'origine-----\nDe\u{202f}: ") && was.text.contains("\nObjet\u{202f}: "), "{}", was.text);
        assert!(was.text.contains("\nEnvoyé\u{202f}: samedi 3 octobre 2026 04:00\n"), "{}", was.text);
        assert!(was.html.contains("<b>De\u{202f}:</b> ") && was.html.contains("<b>Envoyé\u{202f}:</b> "), "{}", was.html);
        let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
        run(&mut s, "compose.close", json!({}));
        // Reopened after the owner has gone back to English, the French original is still known.
        s.interface_language = Lang::English;
        run(&mut s, "mail.editDraft", json!({"id": d}));
        assert_eq!(compose(&s).quote.clone().unwrap().text, was.text);
    });
}

#[test]
fn a_draft_quoted_in_any_shipped_language_reopens_with_its_quote() {
    use mailcraft_i18n::Lang;
    let markers = [
        (Lang::French, "-----Message d'origine-----", "De\u{202f}: "),
        (Lang::German, "-----Ursprüngliche Nachricht-----", "Von: "),
        (Lang::Spanish, "-----Mensaje original-----", "De: "),
        (Lang::Italian, "-----Messaggio originale-----", "Da: "),
    ];
    for (lang, marker, from) in markers {
        let mut s = session();
        s.interface_language = lang;
        let id = newsletter(&mut s);
        run(&mut s, "mail.reply", json!({"id": id}));
        let was = compose(&s).quote.clone().unwrap();
        assert!(was.text.starts_with(&format!("\n\n{marker}\n{from}")), "{lang:?}: {}", was.text);
        let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
        run(&mut s, "compose.close", json!({}));
        // Reopened in English, the quote is still found by its language's marker.
        s.interface_language = Lang::English;
        run(&mut s, "mail.editDraft", json!({"id": d}));
        assert_eq!(compose(&s).quote.clone().unwrap().text, was.text, "{lang:?}");
    }
}
