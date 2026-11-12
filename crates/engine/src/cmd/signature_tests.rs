// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signatures through commands (R4.5.1): rich text, pictures, the two defaults, and mailboxes
//! from before there were any of these.

use mailcraft_model::rich::Align;
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

fn png() -> String {
    mailcraft_charset::base64::STANDARD.encode(b"\x89PNG\r\n\x1a\nIHDR....")
}

fn account(s: &Session) -> u64 {
    s.mailbox.default_account().unwrap().id
}

#[test]
fn an_account_has_one_signature_for_new_mail_and_one_for_replies() {
    let mut s = session();
    let a = account(&s);
    // The sample's account is from before there were two: it has "Standard" for both.
    let acct = s.mailbox.account(a).unwrap();
    assert_eq!((acct.signature.as_str(), acct.reply_signature()), ("Standard", "Standard"));
    run(&mut s, "signature.set", json!({"name": "Short", "text": "Avery"}));
    let r = run(&mut s, "signature.defaults", json!({"account": a, "reply": "short"}));
    assert_eq!((r["new"].as_str(), r["reply"].as_str()), (Some("Standard"), Some("Short")));
    // New gets the long one, a reply the short one.
    run(&mut s, "mail.new", json!({"account": a}));
    assert!(s.compose.as_ref().unwrap().body.plain().ends_with("Avery Quinn\nLumen Studio"));
    run(&mut s, "compose.discard", json!({}));
    let id = s.mailbox.messages.values().find(|m| m.account == a).unwrap().id;
    run(&mut s, "mail.select", json!({"id": id}));
    run(&mut s, "mail.reply", json!({}));
    let body = s.compose.as_ref().unwrap().body.plain();
    assert!(body.starts_with("\n\nAvery") && !body.contains("Lumen Studio\n\n"), "{body:?}");
    run(&mut s, "compose.discard", json!({}));
    // None for replies; changing the new one leaves replies alone.
    run(&mut s, "signature.defaults", json!({"account": a, "reply": ""}));
    run(&mut s, "signature.defaults", json!({"account": a, "new": "Short"}));
    let acct = s.mailbox.account(a).unwrap();
    assert_eq!((acct.signature.as_str(), acct.reply_signature()), ("Short", ""));
    assert!(s.run("signature.defaults", &json!({"account": a, "new": "Nope"})).is_err());
    // Renaming follows into the accounts; removing leaves them none.
    run(&mut s, "signature.rename", json!({"name": "short", "to": "Brief"}));
    assert_eq!(s.mailbox.account(a).unwrap().signature, "Brief");
    assert!(s.run("signature.rename", &json!({"name": "Brief", "to": "standard"})).is_err());
    run(&mut s, "signature.remove", json!({"name": "Brief"}));
    let acct = s.mailbox.account(a).unwrap();
    assert_eq!((acct.signature.as_str(), acct.reply_signature()), ("", ""));
}

#[test]
fn a_signature_takes_rich_text_pasted_html_and_pictures_within_the_caps() {
    let mut s = session();
    run(
        &mut s,
        "signature.set",
        json!({"name": "Work", "html": "<p><b style=\"color:#1F6FEB\">Avery Quinn</b><br><img src=\"https://example.org/logo.png\">Lumen</p>"}),
    );
    let sig = s.mailbox.signature("work").unwrap();
    // The web picture in the paste is not taken; the colour and the bold are.
    assert_eq!((sig.plain().as_str(), sig.pictures().len()), ("Avery Quinn\nLumen", 0));
    assert_eq!(sig.body().style_at(2).color.as_deref(), Some("#1f6feb"));
    let r = run(&mut s, "signature.picture.add", json!({"name": "Work", "data": png(), "file": "lumen-logo.png"}));
    let id = r["id"].as_u64().unwrap();
    assert_eq!(r["heavy"], false);
    run(&mut s, "signature.picture.set", json!({"name": "Work", "id": id, "width": 120, "alt": "Lumen Studio"}));
    run(&mut s, "signature.layout", json!({"name": "Work", "value": "pictureLeft"}));
    let sig = s.mailbox.signature("Work").unwrap();
    assert_eq!(sig.plain(), "[Lumen Studio]Avery Quinn\nLumen");
    assert_eq!(sig.picture(u32::try_from(id).unwrap()).unwrap().width, Some(120));
    // Not a picture, a web address, and a picture that isn't there.
    let junk = { mailcraft_charset::base64::STANDARD.encode(b"<html>") };
    assert!(s.run("signature.picture.add", &json!({"name": "Work", "data": junk})).unwrap_err().to_string().contains("PNG, JPEG or GIF"));
    assert!(
        s.run("signature.picture.add", &json!({"name": "Work", "path": "https://example.org/logo.png"}))
            .unwrap_err()
            .to_string()
            .contains("not downloaded")
    );
    assert!(s.run("signature.picture.set", &json!({"name": "Work", "id": 99, "width": 10})).is_err());
    run(&mut s, "signature.picture.remove", json!({"name": "Work", "id": id}));
    assert!(s.mailbox.signature("Work").unwrap().pictures().is_empty());
    // A save and an open keep all of it.
    run(&mut s, "signature.picture.add", json!({"name": "Work", "data": png()}));
    let json = serde_json::to_string(&s.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(back.signatures, s.mailbox.signatures);
}

#[test]
fn colour_and_font_are_formats_of_the_message() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"format": "html"}));
    run(&mut s, "compose.type", json!({"text": "Hello"}));
    run(&mut s, "format.color", json!({"value": "#C00", "from": 0, "to": 5}));
    run(&mut s, "format.font", json!({"value": "georgia", "from": 0, "to": 5}));
    let body = &s.compose.as_ref().unwrap().body;
    let style = body.text().style_at(3);
    assert_eq!((style.color.as_deref(), style.font.map(|f| f.label())), (Some("#cc0000"), Some("Georgia")));
    assert!(s.run("format.color", &json!({"value": "red"})).is_err());
    assert!(s.run("format.font", &json!({"value": "Comic Sans"})).is_err());
    run(&mut s, "format.color", json!({"value": null, "from": 0, "to": 5}));
    assert_eq!(s.compose.as_ref().unwrap().body.text().style_at(3).color, None);
}

#[test]
fn a_mailbox_from_before_has_its_signatures_and_defaults_as_they_were() {
    let json = r#"{"accounts": [{"id": 1, "email": "avery@lumen.example", "signature": "Standard"}],
        "signatures": [{"name": "Standard", "text": "Avery Quinn\nLumen Studio"}]}"#;
    let mb: mailcraft_model::Mailbox = serde_json::from_str(json).unwrap();
    let a = mb.account(1).unwrap();
    assert_eq!((a.signature.as_str(), a.reply_signature()), ("Standard", "Standard"));
    assert_eq!(mb.signature("standard").unwrap().plain(), "Avery Quinn\nLumen Studio");
}

/// A signature with a logo beside the words, the default account's for new mail.
fn logo(s: &mut Session) {
    run(s, "signature.set", json!({"name": "Logo", "text": "Avery Quinn\nLumen Studio", "default": true}));
    let id =
        run(s, "signature.picture.add", json!({"name": "Logo", "data": png(), "file": "lumen-logo.png", "alt": "Lumen Studio logo"}))["id"].clone();
    run(s, "signature.picture.set", json!({"name": "Logo", "id": id, "width": 96}));
    run(s, "signature.layout", json!({"name": "Logo", "value": "pictureLeft"}));
}

#[test]
fn a_signature_with_a_logo_goes_out_with_the_picture_inside_the_message() {
    let mut s = session();
    logo(&mut s);
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Plan", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    let pic = m.attachments.iter().find(|a| a.inline).expect("the logo goes with the message");
    assert!(pic.content_id.starts_with("sig-") && pic.content_id.ends_with("@mailcraft.invalid") && pic.mime == "image/png");
    let html = m.body.html.clone().unwrap();
    assert!(html.contains(&format!("cid:{}", pic.content_id)) && html.contains("Lumen Studio logo") && html.contains("Hello"), "{html}");
    // The plain-text part has the words, and the picture said in words.
    assert!(m.body.text.starts_with("Hello\n\n[Lumen Studio logo]Avery Quinn\nLumen Studio"), "{:?}", m.body.text);
    // On the wire the picture is part of the HTML, and it reads back.
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    assert!(String::from_utf8_lossy(&bytes).contains("multipart/related"));
    let back = mailcraft_mime::parse(&bytes);
    let read = back.attachments.iter().find(|a| a.content_id == pic.content_id).expect("the picture reads back");
    assert_eq!(read.data, pic.data);
}

#[test]
fn alignment_in_the_open_signature_is_saved_and_goes_out_inside_the_table_and_above_the_picture() {
    let mut s = session();
    logo(&mut s);
    run(&mut s, "signature.open", json!({"name": "Logo"}));
    // The line the logo stands in centred, the next one right.
    run(&mut s, "format.align", json!({"value": "center", "from": 0}));
    run(&mut s, "format.align", json!({"value": "right", "from": 13}));
    run(&mut s, "signature.save", json!({}));
    run(&mut s, "signature.close", json!({}));
    let aligns: Vec<Align> = s.mailbox.signature("Logo").unwrap().body().paragraphs().iter().map(|p| p.align).collect();
    assert_eq!(aligns, [Align::Center, Align::Right]);
    let sent = |s: &mut Session| {
        run(s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
        run(s, "compose.formatHtml", json!({}));
        let id = run(s, "compose.send", json!({}))["id"].as_u64().unwrap();
        s.mailbox.message(id).unwrap().body.html.clone().unwrap()
    };
    // Beside the picture, its lines are aligned inside the text's cell.
    let html = sent(&mut s);
    assert!(
        html.contains("<td style=\"vertical-align:top\">\r\n<div style=\"text-align:center\">Avery Quinn</div>\r\n<div style=\"text-align:right\">Lumen Studio</div>"),
        "{html}"
    );
    // Above the text, the picture is centred as the line it stood in was.
    run(&mut s, "signature.layout", json!({"name": "Logo", "value": "pictureAbove"}));
    let html = sent(&mut s);
    assert!(html.contains("<div style=\"text-align:center\"><img src=\"cid:sig-"), "{html}");
}

/// File › Signatures' bar has the Message tab's Text Highlight Color too (#580, part 3): a
/// highlight in the open signature is saved and goes out with it.
#[test]
fn a_highlight_in_the_open_signature_is_saved_and_goes_out() {
    let mut s = session();
    logo(&mut s);
    run(&mut s, "signature.open", json!({"name": "Logo"}));
    run(&mut s, "format.highlight", json!({"value": "#ffff00", "from": 1, "to": 6}));
    run(&mut s, "signature.save", json!({}));
    run(&mut s, "signature.close", json!({}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert!(html.contains("<span style=\"background-color:#ffff00\">Avery</span>"), "{html}");
}

#[test]
fn a_plain_text_message_has_the_signature_in_words_and_no_pictures() {
    let mut s = session();
    logo(&mut s);
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatPlain", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.body.html.is_none() && m.attachments.is_empty());
    assert!(m.body.text.ends_with("[Lumen Studio logo]Avery Quinn\nLumen Studio"), "{:?}", m.body.text);
}

#[test]
fn changing_the_account_swaps_the_signature_unless_the_owner_edited_it() {
    let mut s = session();
    let a = account(&s);
    let b = run(&mut s, "account.add", json!({"email": "ada@example.org", "name": "Ada"}))["id"].as_u64().unwrap();
    let none = run(&mut s, "account.add", json!({"email": "bo@example.org", "name": "Bo"}))["id"].as_u64().unwrap();
    run(&mut s, "signature.set", json!({"name": "Ada", "text": "Ada Byron"}));
    run(&mut s, "signature.defaults", json!({"account": b, "new": "Ada"}));
    run(&mut s, "signature.defaults", json!({"account": none, "new": ""}));
    let body = |s: &Session| s.compose.as_ref().unwrap().body.plain();
    run(&mut s, "mail.new", json!({"account": a, "body": "Hi"}));
    run(&mut s, "compose.set", json!({"account": b}));
    assert_eq!(body(&s), "Hi\n\nAda Byron");
    run(&mut s, "compose.set", json!({"account": none}));
    assert!(!body(&s).contains("Ada"), "{:?}", body(&s));
    run(&mut s, "compose.discard", json!({}));
    // Edited, it stays as the owner made it.
    run(&mut s, "mail.new", json!({"account": b, "body": "Hi"}));
    let end = s.compose.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": ", PhD"}));
    run(&mut s, "compose.set", json!({"account": a}));
    assert_eq!(body(&s), "Hi\n\nAda Byron, PhD");
}

#[test]
fn choosing_a_signature_replaces_the_one_in_the_message() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Short", "text": "Avery"}));
    run(&mut s, "mail.new", json!({"body": "Hi"}));
    run(&mut s, "compose.signature", json!({"name": "Short"}));
    assert_eq!(s.compose.as_ref().unwrap().body.plain(), "Hi\n\nAvery");
    run(&mut s, "compose.signature", json!({}));
    assert_eq!(s.compose.as_ref().unwrap().body.plain(), "Hi\n\nAvery Quinn\nLumen Studio");
    // With none in it, it goes where the caret is.
    run(&mut s, "compose.discard", json!({}));
    run(&mut s, "signature.defaults", json!({"new": ""}));
    run(&mut s, "mail.new", json!({"body": "Hi"}));
    run(&mut s, "compose.select", json!({"from": 2}));
    run(&mut s, "compose.signature", json!({"name": "Short"}));
    assert_eq!(s.compose.as_ref().unwrap().body.plain(), "Hi\nAvery");
}

#[test]
fn a_signature_is_edited_as_a_message_is_and_saved() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let message = s.compose.as_ref().unwrap().body.text().clone();
    run(&mut s, "signature.open", json!({"name": "standard"}));
    // The editing commands now work on the signature, not on the message.
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": "\nDirector"}));
    run(&mut s, "format.bold", json!({"from": end + 1, "to": end + 9}));
    run(&mut s, "format.color", json!({"value": "#1f4e79"}));
    run(&mut s, "format.font", json!({"value": "Georgia"}));
    assert_eq!(s.compose.as_ref().unwrap().body.text(), &message);
    assert_eq!(s.mailbox.signature("Standard").unwrap().plain(), "Avery Quinn\nLumen Studio");
    // Unsaved, it can't be closed or another opened without saying so.
    run(&mut s, "signature.set", json!({"name": "Short", "text": "Avery"}));
    assert!(s.run("signature.open", &json!({"name": "Short"})).is_err());
    assert!(s.run("signature.close", &json!({})).is_err());
    run(&mut s, "compose.undo", json!({}));
    run(&mut s, "compose.redo", json!({}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!(sig.plain(), "Avery Quinn\nLumen Studio\nDirector");
    let last = sig.body().paragraphs().last().unwrap().runs.first().unwrap().style.clone();
    assert!(last.bold && last.color.as_deref() == Some("#1f4e79") && last.font.is_some(), "{last:?}");
    // Saving is a change of the mailbox, and undone as one.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.signature("Standard").unwrap().plain(), "Avery Quinn\nLumen Studio");
    run(&mut s, "signature.close", json!({"discard": true}));
    assert!(s.signature_edit.is_none());
    // With it closed, typing goes to the message again.
    run(&mut s, "compose.type", json!({"text": "Hi"}));
    assert!(s.compose.as_ref().unwrap().body.plain().starts_with("Hi"));
}

#[test]
fn pictures_go_in_at_the_caret_of_the_open_signature_and_out_with_it() {
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    run(&mut s, "compose.select", json!({"from": 0}));
    let r = run(&mut s, "signature.picture.add", json!({"data": png(), "file": "lumen-logo.png"}));
    let id = r["id"].as_u64().unwrap() as u32;
    let e = s.signature_edit.as_ref().unwrap();
    assert_eq!(e.body.text().pictures(), [id]);
    assert_eq!(super::signature::picture_places(e.body.text(), id), [0]);
    run(&mut s, "signature.picture.set", json!({"id": id, "width": 120}));
    run(&mut s, "signature.layout", json!({"value": "pictureLeft"}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!((sig.pictures().len(), sig.picture(id).unwrap().width, sig.layout), (1, Some(120), mailcraft_model::signature::Layout::PictureLeft));
    // Taken out of the text, it is let go when saved.
    run(&mut s, "signature.picture.remove", json!({"id": id}));
    assert!(s.signature_edit.as_ref().unwrap().body.text().pictures().is_empty());
    run(&mut s, "signature.save", json!({}));
    assert!(s.mailbox.signature("Standard").unwrap().pictures().is_empty());
    // Renaming it follows into the open one; deleting it closes it.
    run(&mut s, "signature.rename", json!({"name": "Standard", "to": "Full"}));
    assert_eq!(s.signature_edit.as_ref().unwrap().signature.name, "Full");
    run(&mut s, "signature.remove", json!({"name": "Full"}));
    assert!(s.signature_edit.is_none());
}

#[test]
fn a_signature_pasted_from_outlook_on_the_web_keeps_its_formatting() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Pasted", "text": ""}));
    run(&mut s, "signature.open", json!({"name": "Pasted"}));
    let html = r#"<div style="font-family: Calibri, Arial, Helvetica, sans-serif; font-size: 12pt; color: rgb(0, 0, 0);"><b>Avery Quinn</b><br><span style="color:#1F4E79">Lumen Studio</span><br><a href="https://lumen.example/">lumen.example</a><br><img src="https://lumen.example/logo.png" alt="logo"></div>"#;
    let r = run(&mut s, "edit.paste", json!({"html": html}));
    assert!(r["note"].as_str().is_some_and(|n| n.contains("Insert Picture")), "{r}");
    let text = s.signature_edit.as_ref().unwrap().body.text().clone();
    assert_eq!(text.to_plain(), "Avery Quinn\nLumen Studio\nlumen.example <https://lumen.example/>");
    let runs: Vec<_> = text.paragraphs().iter().flat_map(|p| p.runs.clone()).collect();
    let name = runs.iter().find(|r| r.text == "Avery Quinn").unwrap();
    assert!(name.style.bold && name.style.font == Some(mailcraft_model::rich::Font::Calibri) && name.style.size == Some(12), "{name:?}");
    let org = runs.iter().find(|r| r.text == "Lumen Studio").unwrap();
    assert_eq!(org.style.color.as_deref(), Some("#1f4e79"));
    assert!(runs.iter().any(|r| r.style.link.as_deref() == Some("https://lumen.example/")));
    assert!(text.pictures().is_empty());
}

#[test]
fn a_signature_made_from_details_has_working_links_and_follows_them_until_edited_by_hand() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": ""}));
    run(&mut s, "signature.picture.add", json!({"name": "Card", "data": png(), "file": "lumen-logo.png"}));
    let details = json!({"name": "Avery Quinn", "title": "Designer", "organisation": "Lumen Studio", "phone": "+44 (0)20 7946 0000", "mobile": "07700 900000", "email": "avery@lumen.example", "website": "lumen.example", "address": "1 Example Street\nLondon"});
    let r = run(&mut s, "signature.details", json!({"name": "Card", "details": details}));
    assert_eq!((r["laidOut"].as_bool(), r["handEdited"].as_bool()), (Some(true), Some(false)));
    let sig = s.mailbox.signature("Card").unwrap();
    assert_eq!(sig.layout, mailcraft_model::signature::Layout::PictureLeft);
    assert_eq!(sig.body().pictures().len(), 1);
    let html = mailcraft_mime::rich::signature_html(sig, &|p| format!("p{}", p.id));
    for link in ["href=\"tel:+442079460000\"", "href=\"tel:07700900000\"", "href=\"mailto:avery@lumen.example\"", "href=\"https://lumen.example\""] {
        assert!(html.contains(link), "{link} in {html}");
    }
    assert!(html.contains("<table") && html.contains("cid:p1"), "{html}");
    // In plain text a number says its link, so it isn't written out again.
    assert!(sig.plain().contains("Phone: +44 (0)20 7946 0000\n") && !sig.plain().contains("<tel:"), "{}", sig.plain());
    // Changing a detail lays it out again.
    let mut changed = details.clone();
    changed["title"] = json!("Director");
    run(&mut s, "signature.details", json!({"name": "Card", "details": changed}));
    assert!(s.mailbox.signature("Card").unwrap().plain().contains("Director"));
    // After a hand edit the details are kept but no longer lay it out, until the owner starts again.
    run(&mut s, "signature.open", json!({"name": "Card"}));
    // (The logo is the first paragraph.)
    run(&mut s, "compose.select", json!({"from": 2}));
    run(&mut s, "compose.type", json!({"text": "Dr "}));
    changed["title"] = json!("Partner");
    let r = run(&mut s, "signature.details", json!({"details": changed}));
    assert_eq!((r["laidOut"].as_bool(), r["handEdited"].as_bool()), (Some(false), Some(true)));
    let e = s.signature_edit.as_ref().unwrap();
    assert!(e.body.plain().contains("Dr Avery") && !e.body.plain().contains("Partner"));
    assert_eq!(e.signature.details.as_ref().unwrap().title, "Partner");
    let r = run(&mut s, "signature.details", json!({"details": changed, "restart": true}));
    assert_eq!(r["laidOut"].as_bool(), Some(true));
    let e = s.signature_edit.as_ref().unwrap();
    assert!(e.body.plain().contains("Partner") && !e.body.plain().contains("Dr "));
    // Starting again is one step of the editor's undo.
    run(&mut s, "compose.undo", json!({}));
    assert!(s.signature_edit.as_ref().unwrap().body.plain().contains("Dr Avery"));
    run(&mut s, "signature.save", json!({}));
    assert!(s.mailbox.signature("Card").unwrap().details.is_some());
    // Junk is refused.
    assert!(s.run("signature.details", &json!({"name": "Card", "details": "x"})).is_err());
}

/// A signature laid out by a signature generator: a table, a logo on the web, and what a hostile
/// paste could carry along.
const GENERATED: &str = r#"<html><head><style>td{color:red}</style><script>alert(1)</script></head><body>
<table cellpadding="0" style="font-family:Arial"><tr><td style="padding-right:12px"><img src="https://logo.example/lumen.png" width="80" alt="Lumen Studio" onerror="alert(2)"></td>
<td><b>Avery Quinn</b><br><a href="https://lumen.example">lumen.example</a> <a href="javascript:alert(3)">click</a><img src="https://track.example/p.gif" width="1" height="1"></td></tr></table></body></html>"#;

#[test]
fn an_html_signature_goes_out_as_written_cleaned_of_what_runs_or_tracks() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "", "default": true}));
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    let sig = s.mailbox.signature("Generated").unwrap();
    assert_eq!(sig.html(), Some(GENERATED));
    // A message being written shows its words.
    assert!(sig.plain().contains("Avery Quinn"), "{:?}", sig.plain());
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Plan", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    let html = m.body.html.clone().unwrap();
    assert!(html.contains("<table") && html.contains("https://logo.example/lumen.png") && html.contains("padding-right:12px"), "{html}");
    assert!(html.contains("Hello") && html.contains("https://lumen.example"), "{html}");
    for gone in ["<script", "alert", "<style", "onerror", "javascript:", "track.example"] {
        assert!(!html.contains(gone), "{gone} in {html}");
    }
    assert!(m.attachments.is_empty());
    assert!(m.body.text.starts_with("Hello\n\n") && m.body.text.contains("Avery Quinn"), "{:?}", m.body.text);
}

#[test]
fn an_html_signature_edited_in_the_message_goes_out_as_edited() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "", "default": true}));
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    // Shown as it goes out, it is out of reach until the owner chooses to edit it as text.
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let end = s.compose.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": " PhD"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert!(html.contains("PhD") && !html.contains("<table"), "{html}");
    // In plain text, the words alone.
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatPlain", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.body.html.is_none() && m.body.text.contains("Avery Quinn") && !m.body.text.contains('<'), "{:?}", m.body.text);
}

/// Chosen to be edited as text, an HTML signature goes out as its simpler copy, though unedited;
/// going back to the HTML takes the caret out of the copy, which is then no longer shown.
#[test]
fn an_html_signature_chosen_as_text_goes_out_as_text_and_back_leaves_the_caret_in_the_text() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "", "default": true}));
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.signature_unedited() && c.signature_html().is_none());
    // Back to the HTML, with the caret at the end of the text, in the copy.
    let end = c.body.text().len();
    let start = c.body.text().signature_start().unwrap();
    run(&mut s, "compose.select", json!({"from": 2, "to": end}));
    run(&mut s, "compose.signatureAsText", json!({"value": false}));
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.body.selection().anchor, c.body.selection().head), (2, start - 1));
    assert!(c.signature_html().is_some());
    run(&mut s, "compose.type", json!({"text": "!"}));
    assert!(s.compose.as_ref().unwrap().signature_unedited());
    // As text, it goes out as the copy.
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert!(html.contains("Avery Quinn") && !html.contains("padding-right:12px"), "{html}");
    // Once the copy is edited, there is no way back to the HTML; without one, nothing to choose.
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let end = s.compose.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": " PhD"}));
    assert!(s.run("compose.signatureAsText", &json!({"value": false})).is_err());
    run(&mut s, "signature.set", json!({"name": "Plain", "text": "Avery", "default": true}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    assert!(s.run("compose.signatureAsText", &json!({"value": true})).is_err());
}

/// The caret never stays in an HTML signature shown as it goes out, however it got there: the
/// next key typed would edit the signature, and its simpler copy would go out in its place.
#[test]
fn a_caret_never_stays_in_an_html_signature_that_is_not_shown() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "", "default": true}));
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    let caret = |s: &Session| {
        let sel = s.compose.as_ref().unwrap().body.selection();
        (sel.anchor, sel.head)
    };
    // In Plain the signature is text, and Ctrl+End goes to its end; then HTML.
    run(&mut s, "compose.formatPlain", json!({}));
    let end = s.compose.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.formatHtml", json!({}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.signature_html_shown().is_some());
    let text_end = c.body.text().signature_start().unwrap() - 1;
    assert_eq!(caret(&s), (text_end, text_end));
    // An agent's selection into it, and typing after it.
    run(&mut s, "compose.select", json!({"from": 2, "to": end}));
    assert_eq!(caret(&s), (2, text_end));
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": "!"}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.signature_unedited() && c.body.text().text().starts_with("Hello\n!\nAvery Quinn"), "{:?}", c.body.text().text());
    // Nor does an undo put it back there.
    run(&mut s, "edit.undo", json!({}));
    let c = s.compose.as_ref().unwrap();
    assert!(caret(&s).1 < c.body.text().signature_start().unwrap());
}

#[test]
fn an_html_signature_is_rich_text_again_with_its_words_and_refuses_what_it_would_lose() {
    let mut s = session();
    // A logo alone still has a place in the message: its description.
    run(&mut s, "signature.set", json!({"name": "Logo", "text": ""}));
    run(&mut s, "signature.html", json!({"name": "Logo", "html": r#"<img src="https://logo.example/l.png" alt="Lumen Studio">"#}));
    assert_eq!(s.mailbox.signature("Logo").unwrap().plain(), "[Lumen Studio]");
    run(&mut s, "signature.html", json!({"name": "Logo", "html": null}));
    let sig = s.mailbox.signature("Logo").unwrap();
    assert!(sig.html().is_none() && sig.plain() == "[Lumen Studio]");
    // One with pictures would lose them; too much HTML is refused; nothing changes.
    logo(&mut s);
    assert!(s.run("signature.html", &json!({"name": "Logo", "html": "<b>Avery</b>"})).is_err());
    assert!(s.mailbox.signature("Logo").unwrap().html().is_none() && s.mailbox.signature("Logo").unwrap().pictures().len() == 1);
    let big = "x".repeat(mailcraft_model::signature::MAX_HTML + 1);
    assert!(s.run("signature.html", &json!({"name": "Standard", "html": big})).is_err());
    assert!(s.run("signature.html", &json!({"name": "Standard", "html": 7})).is_err());
    assert!(s.mailbox.signature("Standard").unwrap().html().is_none());
}

#[test]
fn the_open_signature_becomes_html_and_is_saved() {
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    run(&mut s, "signature.html", json!({"html": "<p><b>Avery Quinn</b></p>"}));
    let e = s.signature_edit.as_ref().unwrap();
    assert!(e.dirty && e.body.text().to_plain() == "Avery Quinn" && e.signature.html().is_some());
    // Saved, it keeps its HTML.
    run(&mut s, "signature.save", json!({}));
    assert_eq!(s.mailbox.signature("Standard").unwrap().html(), Some("<p><b>Avery Quinn</b></p>"));
}

#[test]
fn new_words_for_an_html_signature_make_it_rich_text_and_the_message_says_them_alone() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "", "default": true}));
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    // Through a command: the old table goes with the old words.
    run(&mut s, "signature.set", json!({"name": "Generated", "text": "Jo Bloggs"}));
    assert!(s.mailbox.signature("Generated").unwrap().html().is_none());
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello"}));
    run(&mut s, "compose.formatHtml", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    let html = m.body.html.clone().unwrap();
    assert!(html.contains("Jo Bloggs") && !html.contains("<table") && !html.contains("Avery Quinn"), "{html}");
    assert!(m.body.text.contains("Jo Bloggs") && !m.body.text.contains("Avery Quinn"), "{:?}", m.body.text);
    // Edited while open and saved: the same.
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    run(&mut s, "signature.open", json!({"name": "Generated"}));
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    run(&mut s, "compose.type", json!({"text": " PhD"}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Generated").unwrap();
    assert!(sig.html().is_none() && sig.plain().contains("PhD"), "{:?}", sig.plain());
    // Opened and saved unchanged, it stays HTML.
    run(&mut s, "signature.html", json!({"name": "Generated", "html": GENERATED}));
    run(&mut s, "signature.save", json!({}));
    assert_eq!(s.mailbox.signature("Generated").unwrap().html(), Some(GENERATED));
}

/// "Standard" open, with a picture described "Lumen Studio logo" at its start; its id.
fn open_with_logo(s: &mut Session) -> u32 {
    run(s, "signature.open", json!({"name": "Standard"}));
    run(s, "compose.select", json!({"from": 0}));
    let id = run(s, "signature.picture.add", json!({"data": png(), "file": "lumen-logo.png", "alt": "Lumen Studio logo"}))["id"].as_u64();
    u32::try_from(id.unwrap()).unwrap()
}

fn open_pictures(s: &Session) -> Vec<u32> {
    s.signature_edit.as_ref().unwrap().body.text().pictures()
}

#[test]
fn a_picture_of_the_open_signature_is_moved_by_cut_and_paste_and_saved_where_it_went() {
    let mut s = session();
    let id = open_with_logo(&mut s);
    run(&mut s, "signature.picture.set", json!({"id": id, "width": 96}));
    let cut = run(&mut s, "edit.cut", json!({"from": 0, "to": 1}));
    assert_eq!(cut["text"], "[Lumen Studio logo]");
    assert!(cut["html"].as_str().unwrap().contains("<img src=\"cid:sig-"), "{cut}");
    assert!(open_pictures(&s).is_empty());
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    let out = run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    assert!(out.get("note").is_none(), "nothing was left out: {out}");
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!(sig.plain(), "Avery Quinn\nLumen Studio[Lumen Studio logo]");
    assert_eq!((sig.pictures().len(), sig.picture(id).and_then(|p| p.width)), (1, Some(96)));
    assert_eq!(super::signature::picture_places(sig.body(), id), [end]);
}

#[test]
fn a_pasted_img_the_signature_does_not_hold_is_left_out_and_said() {
    let mut s = session();
    let id = open_with_logo(&mut s);
    let own = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}))["html"].as_str().unwrap().to_string();
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    // Its own picture comes back, and nothing is said.
    run(&mut s, "compose.select", json!({"from": end}));
    let out = run(&mut s, "edit.paste", json!({"html": own}));
    assert!(out.get("note").is_none(), "{out}");
    assert_eq!(open_pictures(&s), [id, id]);
    // A picture of the web, one it doesn't hold, and its own named as a message's are left out,
    // and that is said; its own beside them comes.
    let as_message = own.replace("cid:sig-", "cid:pic-");
    let pasted = format!(
        "<div>x<img src=\"https://example.org/logo.png\"><img src=\"cid:sig-{}.0123456789abcdef@mailcraft.invalid\"></div>{as_message}{own}",
        id + 1
    );
    let out = run(&mut s, "edit.paste", json!({"html": pasted}));
    assert!(out["note"].as_str().is_some_and(|n| n.contains("left out")), "{out}");
    assert_eq!(open_pictures(&s), [id, id, id]);
}

#[test]
fn a_picture_pasted_as_often_as_the_open_signature_holds_is_cut_and_pasted_back_whole_and_no_more_go_in() {
    let mut s = session();
    let id = open_with_logo(&mut s);
    let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}));
    let most = mailcraft_model::signature::MAX_PICTURES;
    for _ in 1..most {
        run(&mut s, "compose.select", json!({"from": 0}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    }
    assert_eq!(open_pictures(&s), vec![id; most]);
    // Each time its text shows a picture counts: one more is refused, pasted or new, and changes
    // nothing.
    let before = s.signature_edit.as_ref().unwrap().body.text().clone();
    let e = s.run("edit.paste", &json!({"html": copied["html"], "text": copied["text"]})).unwrap_err();
    assert!(e.to_string().contains("at most 10 pictures"), "{e}");
    let e = s.run("signature.picture.add", &json!({"data": png(), "file": "badge.png"})).unwrap_err();
    assert!(e.to_string().contains("at most 10 pictures"), "{e}");
    assert_eq!(s.signature_edit.as_ref().unwrap().body.text(), &before);
    // So all of them cut come back, every one, and are saved.
    let cut = run(&mut s, "edit.cut", json!({"from": 0, "to": most}));
    assert!(open_pictures(&s).is_empty());
    run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
    assert_eq!(open_pictures(&s), vec![id; most]);
    run(&mut s, "signature.save", json!({}));
    assert_eq!(s.mailbox.signature("Standard").unwrap().body().pictures(), vec![id; most]);
}

#[test]
fn a_new_picture_over_one_selected_when_the_open_signature_shows_as_many_as_it_holds_takes_its_place_and_one_put_at_a_place_is_refused() {
    let mut s = session();
    let id = open_with_logo(&mut s);
    let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}));
    let most = mailcraft_model::signature::MAX_PICTURES;
    for _ in 1..most {
        run(&mut s, "compose.select", json!({"from": 0}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    }
    // Over one of them, a new picture takes its place: as many are shown as before, and saved.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let badge = mailcraft_charset::base64::STANDARD.encode(b"\x89PNG\r\n\x1a\nIHDR....badge");
    let new = run(&mut s, "signature.picture.add", json!({"data": badge, "file": "badge.png"}))["id"].as_u64().unwrap();
    let shown = open_pictures(&s);
    assert_eq!(shown.len(), most);
    assert_eq!((shown[0], shown.iter().filter(|p| **p == id).count()), (u32::try_from(new).unwrap(), most - 1));
    // Put at a place, it is one more, whatever is selected.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let before = s.signature_edit.as_ref().unwrap().body.text().clone();
    let e = s.run("signature.picture.add", &json!({"data": png(), "file": "seal.png", "at": 0})).unwrap_err();
    assert!(e.to_string().contains("at most 10 pictures"), "{e}");
    assert_eq!(s.signature_edit.as_ref().unwrap().body.text(), &before);
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!((sig.body().pictures().len(), sig.pictures().len()), (most, 2));
}

/// A picture's bytes, told apart by `n`, `len` bytes long when that is more than its header.
fn png_of(n: u8, len: usize) -> String {
    let mut bytes = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    bytes.push(n);
    bytes.resize(len.max(bytes.len()), 0);
    mailcraft_charset::base64::STANDARD.encode(&bytes)
}

#[test]
fn a_new_picture_over_one_selected_when_the_open_signature_holds_as_many_different_pictures_as_it_may_is_refused_and_changes_nothing() {
    let most = mailcraft_model::signature::MAX_PICTURES;
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    for n in 1..=most {
        run(&mut s, "signature.picture.add", json!({"data": png_of(u8::try_from(n).unwrap(), 0), "file": "badge.png", "at": 0}));
    }
    assert_eq!(open_pictures(&s).len(), most);
    // Over one of them, a new one would let go of the one it replaces, which undo could not bring
    // back: refused, as any new picture at the cap is, and nothing changes.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let (text, held) = (s.signature_edit.as_ref().unwrap().body.text().clone(), s.signature_edit.as_ref().unwrap().signature.pictures().to_vec());
    let e = s.run("signature.picture.add", &json!({"data": png_of(0, 0), "file": "seal.png"})).unwrap_err();
    assert!(e.to_string().contains("at most 10 pictures"), "{e}");
    let e = s.signature_edit.as_ref().unwrap();
    assert_eq!((e.body.text(), e.signature.pictures()), (&text, &held[..]));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!((sig.body(), sig.pictures()), (&text, &held[..]));
}

#[test]
fn a_new_picture_over_one_selected_when_the_open_signatures_pictures_weigh_as_much_as_they_may_is_refused_and_changes_nothing() {
    use mailcraft_model::signature::{MAX_PICTURE_BYTES, MAX_PICTURES_BYTES};
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    for n in 0..MAX_PICTURES_BYTES / MAX_PICTURE_BYTES {
        run(&mut s, "signature.picture.add", json!({"data": png_of(u8::try_from(n).unwrap(), MAX_PICTURE_BYTES), "file": "photo.png", "at": 0}));
    }
    // It fits only if the one it takes the place of is let go: refused, and nothing changes.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let (text, held) = (s.signature_edit.as_ref().unwrap().body.text().clone(), s.signature_edit.as_ref().unwrap().signature.pictures().to_vec());
    let e = s.run("signature.picture.add", &json!({"data": png_of(9, MAX_PICTURE_BYTES), "file": "seal.png"})).unwrap_err();
    assert!(e.to_string().contains("KB together"), "{e}");
    let e = s.signature_edit.as_ref().unwrap();
    assert_eq!((e.body.text(), e.signature.pictures()), (&text, &held[..]));
}

#[test]
fn a_copy_of_a_picture_hidden_in_what_is_pasted_takes_no_room_in_the_open_signature() {
    let most = mailcraft_model::signature::MAX_PICTURES;
    let mut s = session();
    let id = open_with_logo(&mut s);
    let copied = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}));
    for _ in 2..most {
        run(&mut s, "compose.select", json!({"from": 0}));
        run(&mut s, "edit.paste", json!({"html": copied["html"], "text": copied["text"]}));
    }
    let html = copied["html"].as_str().unwrap();
    let start = html.find("<img").unwrap();
    let tag = &html[start..start + html[start..].find('>').unwrap() + 1];
    // In a comment and in hidden text it is not pasted, and leaves room for the one shown.
    let pasted = format!("<!--{tag}--><div style=\"display:none\">{tag}</div><div>{tag}</div>");
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "edit.paste", json!({"html": pasted, "text": copied["text"]}));
    assert_eq!(open_pictures(&s), vec![id; most]);
}

#[test]
fn a_new_picture_over_the_one_selected_never_takes_the_id_of_one_the_open_signature_held_so_undo_never_shows_it_in_that_ones_place() {
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    let add = |s: &mut Session, n: u8, at: Option<usize>| {
        let params = match at {
            Some(at) => json!({"data": png_of(n, 0), "file": "badge.png", "at": at}),
            None => json!({"data": png_of(n, 0), "file": "badge.png"}),
        };
        u32::try_from(run(s, "signature.picture.add", params)["id"].as_u64().unwrap()).unwrap()
    };
    let held = |s: &Session, id: u32| s.signature_edit.as_ref().unwrap().signature.picture(id).map(|p| p.data.as_ref().clone());
    let bytes = |n: u8| mailcraft_charset::base64::STANDARD.decode(png_of(n, 0)).unwrap();
    // Replacing the only picture it holds, then the one with the highest id (beside the first).
    let replace = |s: &mut Session, at: usize, n: u8| {
        let old = open_pictures(s)[at];
        let was = held(s, old);
        assert!(was.is_some());
        run(s, "compose.select", json!({"from": at, "to": at + 1}));
        let new = add(s, n, None);
        assert_ne!(new, old, "a new id");
        assert_eq!(open_pictures(s)[at], new);
        // Undone, the one it replaced is back in the text, as it was (not the new one by another
        // name, nor a picture it no longer holds).
        run(s, "compose.undo", json!({}));
        assert_eq!((open_pictures(s)[at], held(s, old)), (old, was));
        run(s, "compose.redo", json!({}));
        assert_eq!((open_pictures(s)[at], held(s, new)), (new, Some(bytes(n))));
    };
    add(&mut s, 1, Some(0));
    replace(&mut s, 0, 3);
    add(&mut s, 2, Some(1));
    replace(&mut s, 1, 4);
    // One deleted from the text, then a new one: the new one has an id of its own too.
    let last = *open_pictures(&s).iter().max().unwrap();
    let place = super::signature::picture_places(s.signature_edit.as_ref().unwrap().body.text(), last)[0];
    run(&mut s, "edit.cut", json!({"from": place, "to": place + 1}));
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    assert_ne!(add(&mut s, 9, Some(end)), last);
}

#[test]
fn a_picture_of_the_open_signature_one_or_two_pixels_wide_is_cut_and_pasted_back() {
    // Made so, or kept so in the file it was read from.
    let stored = json!({"name": "Dot", "body": {"paragraphs": [{"runs": [{"text": mailcraft_model::rich::OBJECT.to_string(), "picture": 1}]}]},
        "pictures": [{"id": 1, "name": "dot.png", "data": png(), "width": 1, "alt": "Dot"}]});
    for width in [Some(1), Some(2), None] {
        let mut s = session();
        let id = match width {
            Some(width) => {
                let id = open_with_logo(&mut s);
                run(&mut s, "signature.picture.set", json!({"id": id, "width": width}));
                id
            }
            None => {
                keep_stored(&mut s, stored.clone());
                run(&mut s, "signature.open", json!({"name": "Dot"}));
                1
            }
        };
        let cut = run(&mut s, "edit.cut", json!({"from": 0, "to": 1}));
        assert!(open_pictures(&s).is_empty());
        let out = run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
        assert!(out.get("note").is_none(), "width {width:?}: {out} {cut}");
        assert_eq!(open_pictures(&s), [id], "width {width:?}: {cut}");
    }
}

/// `json` read as a signature file holds it, and kept in the mailbox.
fn keep_stored(s: &mut Session, json: Value) {
    s.mailbox.signatures.push(serde_json::from_value(json).unwrap());
}

#[test]
fn a_stored_signature_whose_picture_has_the_last_id_there_is_refuses_a_new_picture_rather_than_give_it_that_id_again() {
    let mut s = session();
    let object = mailcraft_model::rich::OBJECT.to_string();
    keep_stored(
        &mut s,
        json!({"name": "Last", "body": {"paragraphs": [{"runs": [{"text": object, "picture": u32::MAX}, {"text": "Avery"}]}]},
            "pictures": [{"id": u32::MAX, "name": "logo.png", "data": png_of(1, 0), "alt": "Logo"}]}),
    );
    let bytes = |n: u8| mailcraft_charset::base64::STANDARD.decode(png_of(n, 0)).unwrap();
    let held = |s: &Session| s.signature_edit.as_ref().unwrap().signature.picture(u32::MAX).map(|p| p.data.as_ref().clone());
    run(&mut s, "signature.open", json!({"name": "Last"}));
    assert_eq!(open_pictures(&s), [u32::MAX]);
    // Over it, a new picture is refused, and changes nothing.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let before = s.signature_edit.as_ref().unwrap().body.text().clone();
    let e = s.run("signature.picture.add", &json!({"data": png_of(2, 0), "file": "badge.png"})).unwrap_err();
    assert!(e.to_string().contains("as many pictures as it can"), "{e}");
    assert_eq!((s.signature_edit.as_ref().unwrap().body.text(), held(&s)), (&before, Some(bytes(1))));
    // Deleted first, so is one put where it was: undo brings back the one it held, as it was.
    run(&mut s, "edit.cut", json!({"from": 0, "to": 1}));
    let e = s.run("signature.picture.add", &json!({"data": png_of(2, 0), "file": "badge.png", "at": 0})).unwrap_err();
    assert!(e.to_string().contains("as many pictures as it can"), "{e}");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!((open_pictures(&s), held(&s)), (vec![u32::MAX], Some(bytes(1))));
    // Closed, it refuses one too, rather than hold two pictures of that id.
    run(&mut s, "signature.close", json!({"discard": true}));
    let e = s.run("signature.picture.add", &json!({"name": "Last", "data": png_of(2, 0), "file": "badge.png", "at": 0})).unwrap_err();
    assert!(e.to_string().contains("as many pictures as it can"), "{e}");
    let sig = s.mailbox.signature("Last").unwrap();
    assert_eq!((sig.pictures().len(), sig.body().pictures()), (1, vec![u32::MAX]));
}

#[test]
fn the_open_signature_says_pictures_were_left_out_only_when_the_reader_shows_one_it_left_out() {
    let mut s = session();
    open_with_logo(&mut s);
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    // `<img` in an attribute, in a comment or as part of another name is no picture.
    for html in ["<div title=\"<img\">hello</div>", "<!-- <img -->hello", "<imgx>hello"] {
        let out = run(&mut s, "edit.paste", json!({"html": html}));
        assert!(out.get("note").is_none(), "{html}: {out}");
    }
    // A picture it does not hold, one of the web and one written into the HTML are.
    for html in
        ["hello<img src=\"cid:other@x\">", "hello<img src=\"https://example.org/a.png\">", "hello<img src=\"data:image/png;base64,iVBORw0KGgo=\">"]
    {
        let out = run(&mut s, "edit.paste", json!({"html": html}));
        assert!(out["note"].as_str().is_some_and(|n| n.contains("left out")), "{html}: {out}");
    }
}

#[test]
fn the_open_signature_says_nothing_of_a_picture_past_the_most_blocks_the_reading_pane_shows() {
    let mut s = session();
    let id = open_with_logo(&mut s);
    let own = run(&mut s, "edit.copy", json!({"from": 0, "to": 1}))["html"].as_str().unwrap().to_string();
    let held = own.split("cid:").nth(1).and_then(|rest| rest.split('"').next()).unwrap().to_string();
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": end}));
    // The reading pane gives the line three blocks and each rule one, and stops at the most it
    // gives: the picture of the web after them is never shown, so nothing shown was left out.
    let rules = "<hr>".repeat(mailcraft_html::MAX_BLOCKS - 3);
    let html = format!("<div>a<img src=\"cid:{held}\">b</div>{rules}<img src=\"https://example.org/a.png\">");
    let out = run(&mut s, "edit.paste", json!({"html": html}));
    assert!(out.get("note").is_none(), "{out}");
    assert_eq!(open_pictures(&s), [id, id]);
}

#[test]
fn the_open_signature_says_a_picture_was_left_out_when_the_paste_is_nothing_else() {
    for params in [json!({"html": "<img src=\"https://example.org/a.png\">"}), json!({"html": "<img src=\"https://example.org/a.png\">", "text": ""})]
    {
        let mut s = session();
        open_with_logo(&mut s);
        let end = s.signature_edit.as_ref().unwrap().body.text().len();
        run(&mut s, "compose.select", json!({"from": end}));
        let before = s.signature_edit.as_ref().unwrap().body.clone();
        let out = run(&mut s, "edit.paste", params.clone());
        assert!(out["note"].as_str().is_some_and(|n| n.contains("left out")), "{params}: {out}");
        let after = &s.signature_edit.as_ref().unwrap().body;
        assert_eq!((after.text(), after.selection()), (before.text(), before.selection()), "{params}");
    }
}

#[test]
fn the_open_signature_says_a_picture_was_left_out_past_the_most_paragraphs_a_text_holds() {
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    let end = s.signature_edit.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": 0, "to": end}));
    let html = format!("<pre>{}</pre><img src=\"https://example.org/chart.png\">", "x\n".repeat(50_000));
    let out = run(&mut s, "edit.paste", json!({"html": html}));
    assert!(out["note"].as_str().is_some_and(|n| n.contains("left out")), "{out}");
}

/// A message whose signature, read from its file, has a picture of the id the message's first
/// picture gets: the message, written in HTML with that picture in "Hello world", and where the
/// signature's picture and the signature start.
fn with_a_signature_picture_of_the_messages_id(s: &mut Session) -> (u32, usize, usize) {
    let id = mailcraft_model::pictures::FIRST_ID;
    let object = mailcraft_model::rich::OBJECT.to_string();
    keep_stored(
        s,
        json!({"name": "Overlap", "body": {"paragraphs": [{"runs": [{"text": "Avery "}, {"text": object, "picture": id}]}]},
            "pictures": [{"id": id, "name": "logo.png", "data": png_of(1, 0), "alt": "Logo"}]}),
    );
    let a = account(s);
    run(s, "signature.defaults", json!({"account": a, "new": "Overlap"}));
    run(s, "mail.new", json!({"to": "maya@lumen.example", "body": "Hello world"}));
    run(s, "compose.formatHtml", json!({}));
    let added = run(s, "insert.picture", json!({"data": png_of(2, 0), "file": "chart.png", "alt": "Chart", "at": 5}))["id"].as_u64();
    assert_eq!(added, Some(u64::from(id)));
    let text = s.compose.as_ref().unwrap().body.text().clone();
    let objects: Vec<usize> = text.text().chars().enumerate().filter(|(_, c)| *c == mailcraft_model::rich::OBJECT).map(|(i, _)| i).collect();
    assert_eq!(objects.len(), 2, "{}", text.text());
    (id, objects[1], text.signature_start().unwrap())
}

#[test]
fn a_copy_of_the_signatures_picture_in_a_message_names_none_of_the_messages_pictures() {
    let mut s = session();
    let (_, logo, start) = with_a_signature_picture_of_the_messages_id(&mut s);
    // Alone, and with the signature's text around it: as before, it is not taken along.
    for (from, to) in [(logo, logo + 1), (start, logo + 1)] {
        let copied = run(&mut s, "edit.copy", json!({"from": from, "to": to}));
        let (html, text) = (copied["html"].as_str().unwrap(), copied["text"].as_str().unwrap());
        assert!(!html.contains("<img") && !text.contains("Chart"), "{from}..{to}: {copied}");
    }
    // Its plain text, which is also what the primary selection is given, says nothing of it.
    let part = s.compose.as_ref().unwrap().body.text().slice(start, logo + 1).unwrap();
    assert_eq!(super::format::plain_text(&s, &part), "Avery ");
    // The message's own picture of that id is still copied as itself.
    let copied = run(&mut s, "edit.copy", json!({"from": 5, "to": 6}));
    assert_eq!(copied["text"], "[Chart]");
}

#[test]
fn the_signatures_picture_in_a_message_takes_none_of_the_room_of_the_messages_pictures() {
    let mut s = session();
    with_a_signature_picture_of_the_messages_id(&mut s);
    let most = mailcraft_model::pictures::MAX_PICTURES;
    for n in 2..most {
        run(&mut s, "insert.picture", json!({"data": png_of(u8::try_from(n + 2).unwrap(), 0), "file": "chart.png", "at": 0}));
    }
    // Twenty-nine of the message's, and the signature's: the thirtieth of the message's goes in.
    run(&mut s, "insert.picture", json!({"data": png_of(200, 0), "file": "chart.png", "at": 0}));
    assert_eq!(s.compose.as_ref().unwrap().pictures.len(), most);
}

#[test]
fn a_picture_the_open_signature_replaced_is_brought_back_by_undo_and_kept_by_a_save() {
    let mut s = session();
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    let bytes = |n: u8| mailcraft_charset::base64::STANDARD.decode(png_of(n, 0)).unwrap();
    let old = run(&mut s, "signature.picture.add", json!({"data": png_of(1, 0), "file": "a.png", "at": 0}))["id"].as_u64().unwrap();
    let old = u32::try_from(old).unwrap();
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    run(&mut s, "signature.picture.add", json!({"data": png_of(2, 0), "file": "b.png"}));
    run(&mut s, "compose.undo", json!({}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature("Standard").unwrap();
    assert_eq!((sig.body().pictures(), sig.picture(old).map(|p| p.data.as_ref().clone())), (vec![old], Some(bytes(1))));
    // Opened again, it is there.
    run(&mut s, "signature.close", json!({}));
    run(&mut s, "signature.open", json!({"name": "Standard"}));
    assert_eq!(open_pictures(&s), [old]);
}
