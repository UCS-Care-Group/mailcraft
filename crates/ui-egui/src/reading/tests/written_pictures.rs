// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #533: pictures written into a message's HTML (`data:image/png;base64,…`), in a message as it
//! comes: `multipart/alternative`, plain text and HTML, nothing attached. They are shown as
//! attached pictures are; nothing is fetched for them.
//!
//! The people, addresses and pictures are invented.

use super::{in_frame, lay_out, png, shown_pictures, stand_ins};
use crate::reading::images::Pictures;

/// Base64 (RFC 4648), as a mail program writes it into HTML.
fn b64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let n = group.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= group.len() { char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' });
        }
    }
    out
}

/// Quoted-printable (RFC 2045), with its soft line breaks every 72 characters: they fall inside
/// the base64, as they do in real mail.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    let mut line = 0;
    for c in text.chars() {
        let piece = if c == '=' { "=3D".to_string() } else { c.to_string() };
        if c == '\n' {
            line = 0;
        } else if line + piece.len() > 72 {
            out.push_str("=\r\n");
            line = 0;
        }
        line += piece.len();
        out.push_str(&piece);
    }
    out
}

/// A message as it comes from the server, with `pictures` written into its HTML.
fn raw_message(pictures: &[String]) -> Vec<u8> {
    let images: String = pictures.iter().map(|src| format!("<p><img src=\"{src}\" alt=\"Chart\"></p>\n")).collect();
    let html = format!("<html><body>\n<p>Hello Avery,</p>\n<p>The figures for the quarter:</p>\n{images}<p>Regards,<br>Rowan</p>\n</body></html>\n");
    format!(
        "From: Rowan Ash <rowan@example.org>\r\nTo: Avery Quinn <avery@example.org>\r\nSubject: Quarter figures\r\n\
         Date: Mon, 26 Oct 2026 09:00:00 +0000\r\nMessage-ID: <q3@example.org>\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello Avery,\r\n\r\nThe figures for the quarter.\r\n\r\nRegards,\r\nRowan\r\n\
         --b1\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}\r\n--b1--\r\n",
        quoted_printable(&html)
    )
    .into_bytes()
}

fn data_png(w: u32, h: u32) -> String {
    format!("data:image/png;base64,{}", b64(&png(w, h)))
}

#[test]
fn two_pictures_written_into_a_message_are_shown() {
    let m = mailcraft_mime::parse(&raw_message(&[data_png(64, 40), data_png(32, 32)]));
    assert!(m.attachments.is_empty(), "nothing is attached");
    assert!(m.body.html.as_deref().is_some_and(|h| h.contains("data:image/png;base64,iVBOR")));
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let laid = lay_out(ctx, &m, 820.0);
            assert_eq!((laid.stats.pictures, laid.stats.placeholders, laid.stats.remote), (2, 0, 0), "{:?}", laid.stats);
            let sizes: Vec<(f32, f32)> = shown_pictures(&laid).iter().map(|i| (i.rect.width(), i.rect.height())).collect();
            assert_eq!(sizes, [(64.0, 40.0), (32.0, 32.0)]);
            // The text around them is there too.
            assert!(laid.stats.texts >= 3, "{:?}", laid.stats);
        });
    }
}

#[test]
fn what_is_written_in_but_is_no_picture_of_ours_is_a_stand_in() {
    let svg = format!("data:image/svg+xml;base64,{}", b64(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
    let html_page = format!("data:text/html;base64,{}", b64(b"<script>alert(1)</script>"));
    let not_png = format!("data:image/png;base64,{}", b64(b"no picture at all"));
    let broken = "data:image/png;base64,iVBOR*w0K".to_string();
    // A PNG's header on a body that is no PNG: past the first look, the picture checks refuse it.
    let mut damaged = png(16, 16);
    damaged.truncate(20);
    let damaged = format!("data:image/png;base64,{}", b64(&damaged));
    let m = mailcraft_mime::parse(&raw_message(&[svg, html_page, not_png, broken, damaged, data_png(24, 24)]));
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 820.0);
        // Five stand-ins, none of them for a picture on the web; the good one is shown.
        assert_eq!((laid.stats.pictures, laid.stats.placeholders, laid.stats.remote), (1, 5, 0), "{:?}", laid.stats);
        assert!(stand_ins(&laid).iter().all(|(label, remote)| *label == "Chart" && !remote), "{:?}", stand_ins(&laid));
        // Only the good picture's bytes, and the damaged one's that were looked at and refused.
        assert!(Pictures::of(ctx).lock().map(|p| p.counts().0 <= 2).unwrap_or(false));
    });
}

/// A message read again is parsed again, and its pictures come as new bytes: each picture is
/// kept once, and nothing of it is held once no message read holds it.
#[test]
fn a_message_read_again_keeps_its_pictures_once() {
    let m = mailcraft_mime::parse(&raw_message(&[data_png(64, 40), data_png(32, 32)]));
    in_frame(false, |ctx| {
        let store = Pictures::of(ctx);
        for _ in 0..6 {
            let laid = lay_out(ctx, &m, 820.0);
            assert_eq!(laid.stats.pictures, 2);
            assert_eq!(store.lock().unwrap().counts().0, 2, "one entry for each picture, however often it is read");
        }
        // The last reading is gone: so are its pictures, at the end of the next frame.
        store.lock().unwrap().release_after(0.0);
        assert_eq!(store.lock().unwrap().counts().0, 0);
    });
}

/// HTML that is nothing but pictures written into it, with a text part beside it, is shown as
/// HTML: the pictures are what it says.
#[test]
fn a_message_of_pictures_alone_shows_them_rather_than_its_text() {
    let html = format!("<html><body><img src=\"{}\"><img src=\"{}\"></body></html>", data_png(64, 40), data_png(32, 32));
    let m = mailcraft_model::Message {
        body: mailcraft_model::Body { text: "See the pictures in the HTML version.".into(), html: Some(html) },
        ..mailcraft_model::Message::default()
    };
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 820.0);
        assert_eq!((laid.stats.pictures, laid.stats.placeholders), (2, 0), "{:?}", laid.stats);
        assert!(!laid.items.iter().any(|i| super::text_of(i).contains("HTML version")));
    });
}

/// #602: under a reply, a reply to all and a forward, the original quoted in the message being
/// written still shows the pictures written into it, and the one attached to it (`cid:`) beside
/// them; nothing is fetched for either.
#[test]
fn the_original_under_a_reply_or_forward_still_shows_its_written_pictures() {
    use super::{MailApp, NOW, Place, Services, window};
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let (account, inbox) = (app.session.mailbox.accounts[0].id, app.session.mailbox.inbox().unwrap());
        let logo = mailcraft_model::Attachment {
            content_id: "logo@example.org".into(),
            inline: true,
            ..mailcraft_model::Attachment::new("logo.png", "image/png", png(20, 20))
        };
        let html = format!(
            "<html><body><p>The figures:</p><img src=\"{}\" alt=\"Chart\"><img src=\"{}\" alt=\"Map\">\
             <img src=\"cid:logo@example.org\"><p>Regards, Rowan</p></body></html>",
            data_png(64, 40),
            data_png(32, 32)
        );
        let m = mailcraft_model::Message {
            account,
            folder: inbox,
            message_id: "q3@example.org".into(),
            from: mailcraft_model::Address::new("Rowan Ash", "rowan@example.org"),
            subject: "Quarter figures".into(),
            body: mailcraft_model::Body { text: "The figures.".into(), html: Some(html) },
            attachments: vec![logo],
            ..Default::default()
        };
        let id = app.session.mailbox.add_message(m).unwrap();
        app.execute("mail.select", serde_json::json!({"id": id})).unwrap();
        app.execute(command, serde_json::json!({"id": id})).unwrap();
        let mut harness = window(app, (1280.0, 900.0));
        harness.run_steps(6);
        let stats = super::super::view::kept_at(&harness.ctx, Place::Quote, id, |laid, _| {
            (laid.done, laid.stats.pictures, laid.stats.placeholders, laid.stats.remote)
        });
        assert_eq!(stats, Some((true, 3, 0, 0)), "{command}");
    }
}

/// A forward saved as a draft and opened again carries the picture written into the original in
/// its quote, and the original's own picture its HTML calls by a Content-ID written otherwise
/// (`cid:logo%40…`) too: that is shown in place (AU3-15, [`mailcraft_model::shown`]), so it is the
/// quote's, as any picture the quote shows. The original quoted under it shows both.
#[test]
fn a_reopened_forward_still_shows_both_kinds_of_picture_in_its_original() {
    use super::{MailApp, NOW, Place, Services, window};
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let (account, inbox) = (app.session.mailbox.accounts[0].id, app.session.mailbox.inbox().unwrap());
    let logo = mailcraft_model::Attachment {
        content_id: "logo@rowan.example".into(),
        inline: true,
        ..mailcraft_model::Attachment::new("logo.png", "image/png", png(20, 20))
    };
    let html = format!(
        "<html><body><p>The figures:</p><img src=\"{}\" alt=\"Chart\">\
         <img src=\"cid:logo%40rowan.example\"><p>Regards, Rowan</p></body></html>",
        data_png(64, 40)
    );
    let m = mailcraft_model::Message {
        account,
        folder: inbox,
        message_id: "q4@rowan.example".into(),
        from: mailcraft_model::Address::new("Rowan Ash", "rowan@rowan.example"),
        subject: "Quarter figures".into(),
        body: mailcraft_model::Body { text: "The figures.".into(), html: Some(html) },
        attachments: vec![logo],
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    app.execute("mail.select", serde_json::json!({"id": id})).unwrap();
    app.execute("mail.forward", serde_json::json!({"id": id})).unwrap();
    app.execute("compose.set", serde_json::json!({"to": "avery@example.org"})).unwrap();
    let draft = app.execute("compose.saveDraft", serde_json::json!({})).unwrap()["draft"].as_u64().unwrap();
    app.execute("compose.close", serde_json::json!({})).unwrap();
    app.execute("mail.editDraft", serde_json::json!({"id": draft})).unwrap();
    let c = app.session.compose.as_ref().unwrap();
    assert_eq!(c.mode, mailcraft_engine::ComposeMode::Draft);
    let quoted = c.quote.as_ref().map_or(Vec::new(), |q| q.pictures.iter().map(|a| a.content_id.clone()).collect());
    assert_eq!(quoted.len(), 2, "the written picture and the logo the quote shows are the quote's");
    assert!(quoted.iter().any(|cid| cid == "logo@rowan.example"), "the logo, shown in place, goes with the quote");
    assert!(!c.attachments.iter().any(|a| a.content_id == "logo@rowan.example"), "and once: not also among the files");
    let shown_as = c.source.and_then(|s| app.session.mailbox.messages.get(&s)).map_or(0, |s| s.id);
    let mut harness = window(app, (1280.0, 900.0));
    harness.run_steps(6);
    let stats = super::super::view::kept_at(&harness.ctx, Place::Quote, shown_as, |laid, _| {
        (laid.done, laid.stats.pictures, laid.stats.placeholders, laid.stats.remote)
    });
    assert_eq!(stats, Some((true, 2, 0, 0)));
}

/// Two forwards of one message being written at once, each with a picture of its own: the
/// original quoted under each carries that one's picture, not the other's, though they have as
/// many (the windows of both share one context), and is kept while it stays the same.
#[test]
fn each_message_being_written_has_its_own_quoted_original() {
    use super::{MailApp, NOW, Services};
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let (account, inbox) = (app.session.mailbox.accounts[0].id, app.session.mailbox.inbox().unwrap());
    let picture = |name: &str, side: u32| mailcraft_model::Attachment {
        content_id: "chart@example.org".into(),
        inline: true,
        ..mailcraft_model::Attachment::new(name, "image/png", png(side, side))
    };
    let m = mailcraft_model::Message {
        account,
        folder: inbox,
        message_id: "chart@example.org".into(),
        from: mailcraft_model::Address::new("Rowan Ash", "rowan@example.org"),
        subject: "The chart".into(),
        body: mailcraft_model::Body { text: "The chart.".into(), html: Some("<p>The chart:</p><img src=\"cid:chart@example.org\">".into()) },
        attachments: vec![picture("chart.png", 20)],
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    for _ in 0..2 {
        app.execute("mail.select", serde_json::json!({"id": id})).unwrap();
        app.execute("mail.forward", serde_json::json!({"id": id})).unwrap();
    }
    assert_eq!(app.session.all_composing().count(), 2);
    // The second forward's picture is another of the same name and count.
    app.session.compose.as_mut().unwrap().attachments = vec![picture("chart.png", 30)];
    let ctx = egui::Context::default();
    let switch = |app: &mut MailApp| std::mem::swap(app.session.compose.as_mut().unwrap(), &mut app.session.composing[0]);
    let carried = |app: &MailApp| crate::reading::quote_message(app, &ctx).unwrap();
    let second = carried(&app);
    switch(&mut app);
    let first = carried(&app);
    let bytes = |m: &mailcraft_model::Message| m.attachments.iter().map(|a| a.data.clone()).collect::<Vec<_>>();
    assert_eq!(bytes(&first), vec![std::sync::Arc::new(png(20, 20))]);
    assert_eq!(bytes(&second), vec![std::sync::Arc::new(png(30, 30))]);
    // Each is kept for its own message while it stays the same.
    assert!(std::sync::Arc::ptr_eq(&carried(&app), &first));
    switch(&mut app);
    assert!(std::sync::Arc::ptr_eq(&carried(&app), &second));
    // Its picture changed for one of the same count: the original is made again.
    app.session.compose.as_mut().unwrap().attachments = vec![picture("chart.png", 40)];
    assert_eq!(bytes(&carried(&app)), vec![std::sync::Arc::new(png(40, 40))]);
}

/// The quoted original kept for a message being written goes once that message is closed, even
/// when no other message is being written.
#[test]
fn a_closed_message_lets_go_of_its_quoted_original() {
    use super::{MailApp, NOW, Services};
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let (account, inbox) = (app.session.mailbox.accounts[0].id, app.session.mailbox.inbox().unwrap());
    let m = mailcraft_model::Message {
        account,
        folder: inbox,
        message_id: "plan@example.org".into(),
        from: mailcraft_model::Address::new("Rowan Ash", "rowan@example.org"),
        subject: "The plan".into(),
        body: mailcraft_model::Body { text: "The plan.".into(), html: Some("<p>The plan:</p><img src=\"cid:plan@example.org\">".into()) },
        attachments: vec![mailcraft_model::Attachment {
            content_id: "plan@example.org".into(),
            inline: true,
            ..mailcraft_model::Attachment::new("plan.png", "image/png", png(20, 20))
        }],
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    app.execute("mail.select", serde_json::json!({"id": id})).unwrap();
    app.execute("mail.forward", serde_json::json!({"id": id})).unwrap();
    let ctx = egui::Context::default();
    let carried = crate::reading::quote_message(&app, &ctx).unwrap();
    crate::reading::tick(&mut app, &ctx);
    assert!(std::sync::Arc::strong_count(&carried) > 1, "kept while it is being written");
    app.execute("compose.close", serde_json::json!({"discard": true})).unwrap();
    assert_eq!(app.session.all_composing().count(), 0);
    crate::reading::tick(&mut app, &ctx);
    assert_eq!(std::sync::Arc::strong_count(&carried), 1, "the closed message's original is let go of");
}
