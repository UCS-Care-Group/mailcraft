// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #602: the pictures written into a message (`data:`) go with a reply or forward of it, as
//! Outlook sends them: each one the reading pane shows is a part of its own beside the HTML, which
//! refers to it by its Content-ID, in the original quoted below; every way a message goes (SMTP,
//! a Microsoft account, an Exchange server account) carries them. The people, addresses and
//! pictures are invented.

use mailcraft_model::sync::{EwsFolderDeltas, GraphFolder};
use mailcraft_model::{Address, Attachment, FolderKind, Id, Message};
use serde_json::json;

use super::compose::QUOTE_MARK;
use super::quote_tests::{compose, run, session};
use crate::{BodyFormat, Compose, Session};

/// A PNG's signature and header, then `extra` bytes: a picture by its own bytes.
fn png(extra: usize, fill: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.resize(v.len() + extra, fill);
    v
}

fn data(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", mailcraft_charset::base64::STANDARD.encode(bytes))
}

/// A message in `folder` written by another program: a chart and a logo written into its HTML,
/// a logo attached and shown (`cid:`), and a file.
fn report(s: &mut Session, account: Id, folder: Id, html: String) -> Id {
    let logo = Attachment { content_id: "logo@rowan.example".into(), inline: true, ..Attachment::new("logo.png", "image/png", png(10, 1)) };
    let file = Attachment::new("figures.pdf", "application/pdf", b"%PDF-1.4".to_vec());
    let m = Message {
        account,
        folder,
        message_id: "q3@rowan.example".into(),
        from: Address::new("Rowan Ash", "rowan@rowan.example"),
        to: vec![Address::new("Avery Quinn", "avery@lumen.example"), Address::new("Kit", "kit@lumen.example")],
        subject: "Quarter figures".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: "The figures.".into(), html: Some(html) },
        attachments: vec![logo, file],
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn chart_html() -> String {
    format!(
        "<html><body><p>The figures:</p><img src=\"{}\" alt=\"Chart\" width=\"64\"><img src=\"cid:logo@rowan.example\">\
         <p>Regards, Rowan</p></body></html>",
        data(&png(40, 2))
    )
}

fn in_inbox(s: &mut Session, html: String) -> Id {
    let (account, inbox) = (s.mailbox.accounts[0].id, s.mailbox.inbox().unwrap());
    report(s, account, inbox, html)
}

/// The written pictures a compose carries with its quote: those that aren't the original's.
fn written(c: &Compose) -> Vec<Attachment> {
    c.quote.as_ref().map(|q| q.pictures.iter().filter(|a| a.content_id.starts_with("quote-")).cloned().collect()).unwrap_or_default()
}

fn wire(m: &Message) -> String {
    String::from_utf8_lossy(&mailcraft_mime_model::write(m, &mailcraft_mime_model::WriteOptions::default())).to_string()
}

#[test]
fn reply_reply_all_and_forward_carry_the_pictures_written_into_the_original() {
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let mut s = session();
        let id = in_inbox(&mut s, chart_html());
        run(&mut s, command, json!({"id": id}));
        let c = compose(&s).clone();
        let q = c.quote.clone().unwrap();
        // The quote refers to the picture by a Content-ID of its own; nothing of the data: is left.
        let pics = written(&c);
        assert_eq!(pics.len(), 1, "{command}: {:?}", q.pictures);
        let pic = &pics[0];
        assert!(pic.inline && pic.mime == "image/png" && pic.data.as_slice() == png(40, 2).as_slice(), "{command}: {pic:?}");
        assert!(pic.content_id.ends_with("@mailcraft.invalid") && !pic.content_id.contains("lumen") && !pic.content_id.contains("avery"), "{pic:?}");
        assert!(pic.name.ends_with(".png") && !pic.name.is_empty());
        assert!(q.html.contains(&format!("<img src=\"cid:{}\" alt=\"Chart\" width=\"64\">", pic.content_id)), "{command}: {}", q.html);
        assert!(!q.html.contains("data:") && q.html.contains("<p>Regards, Rowan</p>"), "{command}: {}", q.html);
        // The attached logo is carried as before: with the quote for an answer, as an
        // attachment of the forward.
        let logo_with_quote = q.pictures.iter().any(|a| a.content_id == "logo@rowan.example");
        let logo_attached = c.attachments.iter().any(|a| a.content_id == "logo@rowan.example");
        assert_eq!((logo_with_quote, logo_attached), (command != "mail.forward", command == "mail.forward"), "{command}");

        if command == "mail.forward" {
            run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
        }
        let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        let m = s.mailbox.message(sent).unwrap().clone();
        let html = m.body.html.clone().unwrap();
        assert!(html.contains(&format!("cid:{}", pic.content_id)) && !html.contains("data:image"), "{command}");
        let part = m.attachments.iter().find(|a| a.content_id == pic.content_id).expect("the picture goes with the message");
        assert!(part.inline && part.data == pic.data, "{command}");
        // Each once.
        assert_eq!(m.attachments.iter().filter(|a| a.content_id == pic.content_id).count(), 1, "{command}");
        assert_eq!(m.attachments.iter().filter(|a| a.content_id == "logo@rowan.example").count(), 1, "{command}");
        // On the wire (SMTP, and an IMAP account's Sent Items), the picture is beside the HTML.
        let wire = wire(&m);
        assert!(wire.contains("multipart/related") && wire.contains(&format!("Content-ID: <{}>", pic.content_id)), "{command}");
        // The plain-text part is as before: words, no pictures.
        assert!(m.body.text.contains("The figures:") && !m.body.text.contains("data:"), "{command}");
    }
}

#[test]
fn the_same_picture_twice_is_one_part() {
    let mut s = session();
    let dot = data(&png(8, 3));
    let id = in_inbox(&mut s, format!("<p>a</p><img src=\"{dot}\"><p>b</p><img src=\"{dot}\" alt=\"again\">"));
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    let pics = written(&c);
    assert_eq!(pics.len(), 1, "{pics:?}");
    assert_eq!(c.quote.unwrap().html.matches(&format!("cid:{}", pics[0].content_id)).count(), 2);
}

#[test]
fn a_content_id_the_original_uses_already_is_never_taken() {
    let mut s = session();
    let id = in_inbox(&mut s, format!("<img src=\"{}\"><img src=\"cid:logo@rowan.example\">", data(&png(8, 4))));
    run(&mut s, "mail.reply", json!({"id": id}));
    let first = written(compose(&s))[0].content_id.clone();
    run(&mut s, "compose.close", json!({"discard": true}));
    // A sender who knows which Content-ID would be given plants it on a part of his own.
    let planted = Attachment { content_id: first.clone(), inline: true, ..Attachment::new("planted.png", "image/png", png(8, 9)) };
    s.mailbox.message_mut(id).unwrap().attachments.push(planted);
    let html = format!("<img src=\"{}\"><img src=\"cid:{first}\">", data(&png(8, 4)));
    s.mailbox.message_mut(id).unwrap().body.html = Some(html);
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    let q = c.quote.unwrap();
    let ids: Vec<&str> = q.pictures.iter().map(|a| a.content_id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "{ids:?}");
    assert_eq!(q.pictures.len(), 2);
    let mine = q.pictures.iter().find(|a| a.data.as_slice() == png(8, 4).as_slice()).unwrap();
    assert_ne!(mine.content_id, first);
}

#[test]
fn a_draft_of_the_reply_keeps_the_pictures_and_sends_them() {
    let mut s = session();
    let id = in_inbox(&mut s, chart_html());
    run(&mut s, "mail.reply", json!({"id": id}));
    let before = compose(&s).quote.clone().unwrap();
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    let c = compose(&s).clone();
    let q = c.quote.clone().unwrap();
    assert_eq!(q.html, before.html);
    let mut was: Vec<String> = before.pictures.iter().map(|a| a.content_id.clone()).collect();
    let mut now: Vec<String> = q.pictures.iter().map(|a| a.content_id.clone()).collect();
    was.sort();
    now.sort();
    assert_eq!(now, was);
    // They are the quote's, not the owner's pictures nor attachments of his.
    assert!(c.pictures.is_empty() && c.attachments.is_empty(), "{:?} {:?}", c.pictures, c.attachments);
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert_eq!(m.attachments.len(), 2);
    assert!(m.attachments.iter().all(|a| a.inline));
    let html = m.body.html.clone().unwrap();
    assert_eq!(html.matches(QUOTE_MARK).count(), 1);
}

#[test]
fn as_plain_text_no_picture_goes() {
    let mut s = session();
    let id = in_inbox(&mut s, chart_html());
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.formatPlain", json!({}));
    assert_eq!(compose(&s).format, BodyFormat::Plain);
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert!(m.body.html.is_none() && m.attachments.is_empty() && m.body.text.contains("Regards, Rowan"));
}

#[test]
fn a_message_of_many_or_huge_pictures_sends_no_more_than_the_reading_pane_shows() {
    let mut s = session();
    // Forty different pictures: as many as the reading pane shows go, the rest are left out.
    let many: String = (0..40u8).map(|n| format!("<img src=\"{}\">", data(&png(16, n)))).collect();
    let id = in_inbox(&mut s, format!("<p>Hello</p>{many}<p>Regards, Rowan</p>"));
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    assert_eq!(written(&c).len(), mailcraft_html::MAX_DATA_PICTURES);
    let q = c.quote.clone().unwrap();
    assert_eq!(q.html.matches("<img").count(), mailcraft_html::MAX_DATA_PICTURES);
    assert!(q.html.contains("Regards, Rowan"));
    run(&mut s, "compose.close", json!({"discard": true}));
    // Pictures of 3 MB, five of them: far more than is read of a message besides its pictures;
    // the words after them are still quoted, and no picture over the cap goes.
    let big = mailcraft_html::MAX_DATA_PICTURE_BYTES / 10 * 3;
    let huge: String = (0..5u8).map(|n| format!("<p><img src=\"{}\"></p>", data(&png(big, n)))).collect();
    let over = data(&png(mailcraft_html::MAX_DATA_PICTURE_BYTES + 1, 9));
    let id = in_inbox(&mut s, format!("<p>Hello</p>{huge}<img src=\"{over}\"><p>Regards, Rowan</p>"));
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    let pics = written(&c);
    assert_eq!(pics.len(), 5);
    assert!(pics.iter().all(|a| a.data.len() <= mailcraft_html::MAX_DATA_PICTURE_BYTES));
    let total: usize = pics.iter().map(|a| a.data.len()).sum();
    assert!(total <= mailcraft_html::MAX_DATA_PICTURES_BYTES);
    let q = c.quote.unwrap();
    assert!(q.html.contains("Regards, Rowan") && q.text.contains("Regards, Rowan"));
    // The quote kept with the message being written refers to them; it doesn't hold them twice.
    assert!(q.html.len() < 10_000, "{}", q.html.len());
}

#[test]
fn a_microsoft_account_takes_the_pictures_up_as_inline_attachments() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "avery@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = report(&mut s, account, inbox, chart_html());
    run(&mut s, "mail.reply", json!({"id": id}));
    let pic = written(compose(&s))[0].clone();
    // Saved as a draft: the server's draft gets the picture as an inline attachment of its own.
    run(&mut s, "compose.saveDraft", json!({}));
    let saved = compose(&s).draft.expect("saved");
    let draft = super::graph_drafts::draft_up_whole(s.mailbox.message(saved).unwrap()).expect("a draft goes up whole");
    let here = draft.attachments.clone().expect("with its attachments").here;
    let up = here.iter().find(|a| a.content_id == pic.content_id).expect("the picture goes up");
    assert!(up.inline && up.mime == "image/png" && up.data.as_slice() == pic.data.as_slice(), "{up:?}");
    let body = draft.body.clone().expect("the body goes up");
    assert!(body.html && body.content.contains(&format!("cid:{}", pic.content_id)) && !body.content.contains("data:image"));
    // Sent: as MIME, related.
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let up = super::graph_send::sends(&s, account).into_iter().find(|up| up.message == sent).expect("queued to send");
    let m = up.draft.is_none().then_some(up.mime).flatten().map(|m| *m).unwrap_or_else(|| s.mailbox.message(sent).unwrap().clone());
    assert!(m.attachments.iter().any(|a| a.inline && a.content_id == pic.content_id));
    let wire = wire(&m);
    assert!(wire.contains("multipart/related") && wire.contains(&format!("Content-ID: <{}>", pic.content_id)), "{wire}");
}

#[test]
fn an_exchange_server_account_sends_the_pictures_in_its_mime() {
    let mut s = Session::empty();
    let url = "https://exchange.example.org/EWS/Exchange.asmx";
    let account =
        s.run("account.add", &json!({"email": "avery@example.org", "kind": "exchange", "ews": {"url": url}})).unwrap()["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let list = vec![
        folder("F1", "Inbox", FolderKind::Inbox),
        folder("F2", "Sent Items", FolderKind::Sent),
        folder("F3", "Drafts", FolderKind::Drafts),
        folder("F4", "Deleted Items", FolderKind::Deleted),
    ];
    let piece = mailcraft_model::sync::EwsMailResult {
        account,
        email: "avery@example.org".into(),
        url: url.into(),
        folders: Some(EwsFolderDeltas { changed: list, afresh: true, ..Default::default() }),
        hierarchy: "H".into(),
        partial: true,
        ..Default::default()
    };
    super::ews_mail::apply_ews_mail(&mut s, piece).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = report(&mut s, account, inbox, chart_html());
    run(&mut s, "mail.forward", json!({"id": id}));
    let pic = written(compose(&s))[0].clone();
    run(&mut s, "compose.set", json!({"to": "maya@example.net"}));
    run(&mut s, "compose.send", json!({}));
    let request = super::ews_mail::ews_mail_request(&s, account, false).unwrap();
    let [send] = request.sends.as_slice() else { panic!("{:?}", request.sends) };
    let m = send.mime.as_deref().expect("goes as its MIME");
    let part = m.attachments.iter().find(|a| a.content_id == pic.content_id).expect("the picture goes");
    assert!(part.inline && part.data == pic.data);
    // The original's own logo and file go with the forward too, each once.
    assert_eq!(m.attachments.len(), 3, "{:?}", m.attachments.iter().map(|a| &a.name).collect::<Vec<_>>());
    let wire = wire(m);
    assert!(wire.contains("multipart/related") && wire.contains(&format!("Content-ID: <{}>", pic.content_id)));
}

#[test]
fn a_picture_the_quote_left_out_never_goes_though_its_content_id_is_written_in_the_text() {
    let mut s = session();
    // A picture before a `<body>` that comes later is left out of the quote. The sender, who
    // knows the Content-ID it would get, writes that in the words of the message.
    let head = data(&png(8, 5));
    let id = in_inbox(&mut s, "<p>x</p>".to_string());
    s.mailbox.message_mut(id).unwrap().body.html = Some(format!("<img src=\"{head}\"><body><p>Hello</p></body>"));
    run(&mut s, "mail.reply", json!({"id": id}));
    let handed = mailcraft_model::sync::fingerprint(&png(8, 5));
    let cid = format!("quote-1.{handed:016x}@mailcraft.invalid");
    run(&mut s, "compose.close", json!({"discard": true}));
    s.mailbox.message_mut(id).unwrap().body.html = Some(format!("<img src=\"{head}\"><body><p>Hello cid:{cid}</p></body>"));
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    assert!(written(&c).is_empty(), "{:?}", c.quote.as_ref().map(|q| &q.pictures));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert!(s.mailbox.message(sent).unwrap().attachments.iter().all(|a| !a.content_id.starts_with("quote-")));
}

#[test]
fn a_part_whose_content_id_begins_one_of_ours_goes_only_when_the_original_shows_it() {
    // The original's own parts `quote-1` and `q`, which its HTML never shows, are not shown by
    // the Content-ID its written picture goes out under (`quote-1.…`).
    for planted in ["quote-1", "q", "quote-1.", "quote"] {
        let mut s = session();
        let id = in_inbox(&mut s, format!("<p>a</p><img src=\"{}\">", data(&png(8, 6))));
        let part = Attachment { content_id: planted.into(), inline: true, ..Attachment::new("unseen.png", "image/png", png(8, 7)) };
        s.mailbox.message_mut(id).unwrap().attachments.push(part);
        run(&mut s, "mail.reply", json!({"id": id}));
        let c = compose(&s).clone();
        let q = c.quote.clone().unwrap();
        assert_eq!(written(&c).len(), 1, "{planted}: {:?}", q.pictures);
        assert!(q.pictures.iter().all(|a| a.content_id != planted), "{planted}: {:?}", q.pictures);
        let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        assert!(s.mailbox.message(sent).unwrap().attachments.iter().all(|a| a.content_id != planted), "{planted}");
    }
    // One the HTML shows goes as before.
    let mut s = session();
    let id = in_inbox(&mut s, format!("<img src=\"{}\"><img src=\"cid:quote-1\">", data(&png(8, 6))));
    let part = Attachment { content_id: "quote-1".into(), inline: true, ..Attachment::new("shown.png", "image/png", png(8, 7)) };
    s.mailbox.message_mut(id).unwrap().attachments.push(part);
    run(&mut s, "mail.reply", json!({"id": id}));
    let q = compose(&s).quote.clone().unwrap();
    assert!(q.pictures.iter().any(|a| a.content_id == "quote-1"), "{:?}", q.pictures);
}

#[test]
fn two_pictures_of_one_length_are_two_parts_and_each_goes_as_what_its_bytes_say() {
    let mut s = session();
    // Of one length, differing in their last byte; the second says it is a GIF but is a PNG.
    let (a, b) = (png(30, 1), {
        let mut b = png(30, 1);
        *b.last_mut().unwrap() = 2;
        b
    });
    let as_gif = format!("data:image/gif;base64,{}", mailcraft_charset::base64::STANDARD.encode(&b));
    let id = in_inbox(&mut s, format!("<img src=\"{}\"><img src=\"{as_gif}\">", data(&a)));
    run(&mut s, "mail.reply", json!({"id": id}));
    let pics = written(compose(&s));
    assert_eq!(pics.len(), 2);
    assert_ne!(pics[0].content_id, pics[1].content_id);
    assert!(pics.iter().all(|p| p.mime == "image/png" && p.name.ends_with(".png")), "{pics:?}");
}

#[test]
fn a_draft_of_the_forward_reopens_with_each_part_once() {
    let mut s = session();
    let id = in_inbox(&mut s, chart_html());
    run(&mut s, "mail.forward", json!({"id": id}));
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let pic = written(compose(&s))[0].clone();
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    let c = compose(&s).clone();
    let q = c.quote.clone().unwrap();
    let all: Vec<&str> = q.pictures.iter().chain(&c.attachments).map(|a| a.name.as_str()).collect();
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(sorted, ["figures.pdf", "logo.png", pic.name.as_str()], "{all:?}");
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert_eq!(m.attachments.len(), 3, "{:?}", m.attachments.iter().map(|a| &a.name).collect::<Vec<_>>());
    assert_eq!(m.attachments.iter().filter(|a| a.content_id == pic.content_id).count(), 1);
}

#[test]
fn a_reply_to_the_reply_carries_the_pictures_again_once_each() {
    let mut s = session();
    let id = in_inbox(&mut s, chart_html());
    run(&mut s, "mail.reply", json!({"id": id}));
    let pic = written(compose(&s))[0].clone();
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    // The answer comes back: its pictures are attached ones now, which its HTML shows.
    let mut back = s.mailbox.message(sent).unwrap().clone();
    back.folder = s.mailbox.inbox().unwrap();
    back.message_id = "back@lumen.example".into();
    let back = s.mailbox.add_message(back).unwrap();
    run(&mut s, "mail.reply", json!({"id": back}));
    let q = compose(&s).quote.clone().unwrap();
    let ids: Vec<&str> = q.pictures.iter().map(|a| a.content_id.as_str()).collect();
    assert_eq!(ids.iter().filter(|c| **c == pic.content_id).count(), 1, "{ids:?}");
    assert_eq!(ids.iter().filter(|c| **c == "logo@rowan.example").count(), 1, "{ids:?}");
    assert_eq!(ids.len(), 2, "{ids:?}");
}
