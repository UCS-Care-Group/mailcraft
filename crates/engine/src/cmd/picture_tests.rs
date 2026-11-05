// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures in the text of a message being written (#547): put in with Insert › Pictures, deleted
//! like a letter, sent as inline parts the HTML refers to, kept in drafts, and shown again by a
//! paste only when the message holds it.

use mailcraft_model::pictures::FIRST_ID;
use mailcraft_model::rich::OBJECT;
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

fn png_bytes(extra: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    v.resize(v.len() + extra, 7);
    v
}

fn b64(bytes: &[u8]) -> String {
    mailcraft_charset::base64::STANDARD.encode(bytes)
}

/// A new message to somebody, with `Hello world` in it and no signature.
fn message() -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Plan"}));
    let text_end = compose(&s).body.text().len();
    run(&mut s, "compose.select", json!({"from": 0, "to": text_end}));
    run(&mut s, "compose.type", json!({"text": "Hello world"}));
    s
}

fn objects(s: &Session) -> usize {
    compose(s).body.plain().chars().filter(|c| *c == OBJECT).count()
}

#[test]
fn a_picture_goes_in_where_the_caret_is_and_is_one_character() {
    let mut s = message();
    run(&mut s, "compose.select", json!({"from": 5}));
    let out = run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(30)), "file": "chart.png", "alt": "Sales"}));
    assert_eq!(out["id"], FIRST_ID);
    assert!(compose(&s).body.plain().starts_with(&format!("Hello{OBJECT} world")), "{:?}", compose(&s).body.plain());
    assert_eq!(compose(&s).body.selection().head, 6, "the caret is after it");
    assert!(compose(&s).dirty && compose(&s).pictures.len() == 1);
    assert_eq!(compose(&s).pictures[0].name, "chart.png");
    assert_eq!(compose(&s).pictures[0].mime, "image/png");
    // `at` says where.
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8)), "at": 0}));
    assert!(compose(&s).body.plain().starts_with(&format!("{OBJECT}Hello{OBJECT}")));
    assert_eq!(objects(&s), 2);
}

#[test]
fn undo_takes_it_out_and_redo_puts_it_back() {
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8))}));
    assert_eq!(objects(&s), 1);
    // (Undo of the message's text, not of the mailbox.)
    let mailbox_undo = s.can_undo();
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(objects(&s), 0);
    run(&mut s, "compose.redo", json!({}));
    assert_eq!(objects(&s), 1);
    assert_eq!(s.can_undo(), mailbox_undo);
}

#[test]
fn backspace_and_delete_take_it_like_a_letter_and_undo_brings_it_back_whole() {
    let mut s = message();
    run(&mut s, "compose.select", json!({"from": 5}));
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(20))}));
    // Backspace right after it.
    run(&mut s, "compose.delete", json!({}));
    assert_eq!(objects(&s), 0);
    assert_eq!(compose(&s).body.plain(), "Hello world");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(objects(&s), 1);
    // Delete from before it.
    run(&mut s, "compose.select", json!({"from": 5}));
    run(&mut s, "compose.delete", json!({"forward": true}));
    assert_eq!(objects(&s), 0);
    // Undone, the picture is the same one (its bytes were kept).
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(objects(&s), 1);
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(sent).unwrap().attachments.iter().filter(|a| a.inline && a.content_id.starts_with("pic-")).count(), 1);
}

#[test]
fn a_deleted_picture_is_not_sent() {
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8))}));
    run(&mut s, "compose.delete", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.attachments.iter().all(|a| !a.content_id.starts_with("pic-")), "{:?}", m.attachments);
    assert!(!m.body.html.as_deref().unwrap_or("").contains("<img"));
}

#[test]
fn what_is_not_a_picture_or_too_much_is_refused_and_nothing_changes() {
    let mut s = message();
    let before = compose(&s).clone();
    for (params, why) in [
        (json!({"data": b64(b"just some text, not a picture")}), "PNG, JPEG, GIF or WebP"),
        (json!({"data": b64(b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>"), "file": "x.png"}), "PNG, JPEG, GIF or WebP"),
        (json!({"data": b64(&png_bytes(6 << 20))}), "at most 5 MB"),
        (json!({"data": "A".repeat(60 << 20)}), "at most 5 MB"),
        (json!({"data": "not base64 !!"}), "base64"),
        (json!({"path": "https://example.org/a.png"}), "not downloaded"),
        (json!({"path": "//example.org/a.png"}), "not downloaded"),
        (json!({"path": "/nonexistent/folder/a.png"}), "a.png"),
        (json!({"data": b64(&png_bytes(8)), "width": 0}), "width"),
        (json!({"data": b64(&png_bytes(8)), "at": 9999}), ""),
        (json!({}), ""),
    ] {
        let e = s.run("insert.picture", &params).unwrap_err().to_string();
        assert!(e.contains(why), "{params}: {e}");
    }
    assert_eq!(*compose(&s), before);
}

#[test]
fn a_message_holds_thirty_pictures_and_twenty_megabytes() {
    let mut s = message();
    for _ in 0..30 {
        run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8))}));
    }
    let e = s.run("insert.picture", &json!({"data": b64(&png_bytes(8))})).unwrap_err().to_string();
    assert!(e.contains("at most 30"), "the 31st is refused: {e}");
    // Together by size: four of 5 MB fill it.
    let mut s = message();
    for _ in 0..4 {
        run(&mut s, "insert.picture", json!({"data": b64(&png_bytes((5 << 20) - 20))}));
    }
    let e = s.run("insert.picture", &json!({"data": b64(&png_bytes(50 * 1024))})).unwrap_err().to_string();
    assert!(e.contains("20 MB together"), "{e}");
    // Taking one out makes room.
    run(&mut s, "compose.delete", json!({}));
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(50 * 1024))}));
}

/// A PNG of `w` x `h`: a screenshot's flat areas and edges, or noise that hardly compresses.
fn real_png(w: u32, h: u32, noisy: bool) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = (seed >> 56) as u8;
            let px = if noisy {
                [n, (seed >> 48) as u8, (seed >> 40) as u8]
            } else {
                [(x / 40 % 256) as u8, (y / 30 % 256) as u8, if (x / 8 + y / 8) % 2 == 0 { 230 } else { 20 }]
            };
            rgb.extend(px);
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

fn size_of(bytes: &[u8]) -> (u32, u32) {
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().unwrap().into_dimensions().unwrap()
}

#[test]
fn a_screenshot_sized_picture_is_scaled_to_1920_wide_and_fits() {
    let mut s = message();
    let big = real_png(3000, 2000, false);
    run(&mut s, "insert.picture", json!({"data": b64(&big), "file": "Screenshot 1.png"}));
    let p = &compose(&s).pictures[0];
    assert_eq!(size_of(&p.data), (1920, 1280));
    assert!(
        p.data.len() <= mailcraft_model::pictures::MAX_PICTURE_BYTES && p.mime == "image/png" && p.name == "Screenshot 1.png",
        "{} {}",
        p.data.len(),
        p.name
    );
}

#[test]
fn a_picture_over_the_cap_is_saved_again_smaller_until_it_fits() {
    let mut s = message();
    // Noise: PNG can't make it small, so it is a JPEG, or narrower, but under the cap.
    let big = real_png(3000, 2000, true);
    assert!(big.len() > 5 << 20);
    run(&mut s, "insert.picture", json!({"data": b64(&big), "file": "noise.png"}));
    let p = &compose(&s).pictures[0];
    let (w, _) = size_of(&p.data);
    assert!(w <= 1920 && p.data.len() <= mailcraft_model::pictures::MAX_PICTURE_BYTES, "{w} {}", p.data.len());
    assert!(p.name.starts_with("noise.") && p.name.ends_with(if p.mime == "image/jpeg" { ".jpg" } else { ".png" }), "{} {}", p.name, p.mime);
    // Within the caps and the width it is left alone, byte for byte.
    let small = real_png(300, 200, false);
    run(&mut s, "insert.picture", json!({"data": b64(&small)}));
    assert_eq!(compose(&s).pictures[1].data.as_slice(), small.as_slice());
}

#[test]
fn an_animated_gif_is_never_flattened_and_is_refused_when_over_the_cap() {
    use image::{Delay, Frame, RgbaImage};
    let mut seed = 7u64;
    let mut gif = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut gif, 30);
        for _ in 0..4 {
            let mut img = RgbaImage::new(2000, 1500);
            for px in img.pixels_mut() {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *px = image::Rgba([(seed >> 56) as u8, (seed >> 48) as u8, (seed >> 40) as u8, 255]);
            }
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))).unwrap();
        }
    }
    assert!(gif.len() > 5 << 20, "{}", gif.len());
    let mut s = message();
    let e = s.run("insert.picture", &json!({"data": b64(&gif), "file": "dance.gif"})).unwrap_err().to_string();
    assert!(e.contains("at most 5 MB"), "{e}");
    assert!(compose(&s).pictures.is_empty());
}

#[test]
fn a_plain_text_message_or_an_open_signature_takes_none() {
    let mut s = message();
    run(&mut s, "compose.formatPlain", json!({}));
    assert!(!s.enabled("insert.picture"));
    assert!(s.run("insert.picture", &json!({"data": b64(&png_bytes(8))})).unwrap_err().to_string().contains("plain text"));
    let mut s = session();
    assert!(!s.enabled("insert.picture"), "no message is being written");
    assert!(s.run("insert.picture", &json!({"data": b64(&png_bytes(8))})).is_err());
}

#[test]
fn a_picture_goes_out_as_an_inline_part_the_html_refers_to_by_content_id() {
    let mut s = message();
    run(&mut s, "compose.select", json!({"from": 5}));
    // A name with a line break in it must not get into a header.
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(40)), "file": "chart\r\nBcc: evil@example.net\r\n.png", "alt": "Sales \"Q3\""}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    let pic = m.attachments.iter().find(|a| a.content_id.starts_with("pic-")).expect("the picture goes with the message");
    assert!(pic.inline && pic.mime == "image/png" && pic.content_id.ends_with("@mailcraft.invalid"));
    assert!(!pic.content_id.contains(char::is_whitespace) && !pic.name.contains(['\r', '\n']));
    let html = m.body.html.clone().unwrap();
    assert!(html.contains(&format!("cid:{}", pic.content_id)) && html.contains("Sales &quot;Q3&quot;"), "{html}");
    assert!(html.contains("Hello") && html.contains("world"));
    // The plain-text part says there was one, with its description.
    assert!(m.body.text.starts_with("Hello[Sales \"Q3\"] world"), "{:?}", m.body.text);
    assert!(!m.body.text.contains(OBJECT));
    // On the wire: multipart/related, the cid matching, no header made of the name.
    let bytes = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let wire = String::from_utf8_lossy(&bytes).to_string();
    assert!(wire.contains("multipart/related"), "{wire}");
    assert!(wire.contains(&format!("Content-ID: <{}>", pic.content_id)));
    assert!(!wire.lines().any(|l| l.starts_with("Bcc:") && l.contains("evil")), "{wire}");
    let back = mailcraft_mime::parse(&bytes);
    let read = back.attachments.iter().find(|a| a.content_id == pic.content_id).expect("the picture reads back");
    assert_eq!(read.data, pic.data);
    assert!(back.body.html.unwrap().contains(&format!("cid:{}", pic.content_id)));
}

#[test]
fn a_picture_without_a_description_is_a_placeholder_in_plain_text() {
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8))}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().body.text.starts_with("Hello world[picture]"), "{:?}", s.mailbox.message(id).unwrap().body.text);
    // A message sent as plain text has the words and no parts.
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8))}));
    run(&mut s, "compose.formatPlain", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.body.html.is_none() && m.attachments.is_empty() && m.body.text.contains("[picture]"), "{m:?}");
}

#[test]
fn a_draft_keeps_its_pictures_and_opens_with_them() {
    let mut s = message();
    run(&mut s, "compose.select", json!({"from": 5}));
    let one = png_bytes(20);
    let two = b"\xff\xd8\xff\xe0JFIF-picture".to_vec();
    run(&mut s, "insert.picture", json!({"data": b64(&one), "file": "one.png", "alt": "First", "width": 120}));
    run(&mut s, "compose.select", json!({"from": 0}));
    run(&mut s, "insert.picture", json!({"data": b64(&two), "file": "two.jpg"}));
    let before = compose(&s).body.plain();
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = compose(&s).draft.expect("saved");
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let c = compose(&s).clone();
    // The words and the pictures where they were, the pictures not attachments.
    assert_eq!(c.body.plain().replace("Avery Quinn", "").trim_end(), before.replace("Avery Quinn", "").trim_end());
    assert_eq!(c.body.text().pictures().len(), 2);
    assert!(c.attachments.iter().all(|a| !a.content_id.starts_with("pic-")), "{:?}", c.attachments);
    let datas: Vec<&[u8]> = c.pictures.iter().map(|p| p.data.as_slice()).collect();
    assert!(datas.contains(&one.as_slice()) && datas.contains(&two.as_slice()));
    let first = c.pictures.iter().find(|p| p.data.as_slice() == one).unwrap();
    assert_eq!((first.alt.as_str(), first.width, first.name.as_str()), ("First", Some(120), "one.png"));
    // New pictures don't take the ids the old ones have.
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(9))}));
    let mut ids: Vec<u32> = compose(&s).pictures.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    // And it goes out with all three, once each.
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.attachments.iter().filter(|a| a.content_id.starts_with("pic-")).count(), 3);
    assert_eq!(m.body.html.as_deref().unwrap().matches("<img").count(), 3);
}

#[test]
fn a_draft_written_by_hand_cannot_bring_in_what_a_picture_may_not_be() {
    use mailcraft_model::{Attachment, Body};
    let mut s = session();
    let drafts = s.mailbox.folder_of_kind(s.mailbox.default_account().unwrap().id, mailcraft_model::FolderKind::Drafts).unwrap();
    let evil = Attachment {
        content_id: "pic-1048576.x@mailcraft.invalid".into(),
        inline: true,
        ..Attachment::new("a.png", "image/png", b"<html><script>alert(1)</script>".to_vec())
    };
    let other = Attachment { content_id: "pic-5.x@mailcraft.invalid".into(), inline: true, ..Attachment::new("b.png", "image/png", png_bytes(8)) };
    let m = mailcraft_model::Message {
        account: s.mailbox.default_account().unwrap().id,
        folder: drafts,
        draft: true,
        subject: "hand".into(),
        body: Body {
            text: "x".into(),
            html: Some(format!("<html><body><div>x<img src=\"cid:{}\"><img src=\"cid:{}\"></div></body></html>", evil.content_id, other.content_id)),
        },
        attachments: vec![evil, other],
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    run(&mut s, "mail.editDraft", json!({"id": id}));
    let c = compose(&s);
    assert!(c.pictures.is_empty() && c.body.text().pictures().is_empty(), "neither is taken as a picture");
}

#[test]
fn agents_give_the_picture_as_data_never_as_a_path() {
    use crate::agent::{Class, Verdict, check};
    let mut s = message();
    assert!(matches!(check(&s, "insert.picture", &json!({"path": "/home/me/.ssh/id_ed25519"}), &Class::ALL), Verdict::Refuse(_)));
    assert_eq!(check(&s, "insert.picture", &json!({"data": b64(&png_bytes(8))}), &[]), Verdict::Run);
    // A message the agent wrote: the owner's isn't the agent's to change (#577).
    s.compose.as_mut().unwrap().owner_changed = false;
    s.agent = Some(Vec::new());
    assert!(s.run("insert.picture", &json!({"path": "/etc/hostname"})).is_err());
    assert!(s.run("insert.picture", &json!({"data": b64(&png_bytes(8))})).is_ok());
}

#[test]
fn hostile_params_never_panic() {
    let mut s = message();
    for v in [
        json!(null),
        json!([]),
        json!("x"),
        json!({"path": 7}),
        json!({"data": 7}),
        json!({"data": null, "path": null}),
        json!({"data": b64(&png_bytes(8)), "at": -1}),
        json!({"data": b64(&png_bytes(8)), "at": u64::MAX}),
        json!({"data": b64(&png_bytes(8)), "width": 1e300}),
        json!({"data": b64(&png_bytes(8)), "width": 70000}),
        json!({"data": b64(&png_bytes(8)), "alt": 5, "file": []}),
        json!({"data": b64(&png_bytes(8)), "file": "\u{0}\u{202e}\u{fffc}"}),
        json!({"data": "====", "file": "\u{10ffff}"}),
        json!({"path": "\u{0}"}),
        json!({"path": "/dev/zero"}),
    ] {
        let _ = s.run("insert.picture", &v);
    }
    // Whatever got in is a sound message: it still sends.
    assert!(s.run("compose.send", &json!({})).is_ok());
}

#[test]
fn a_microsoft_account_sends_the_picture_up_as_an_inline_attachment() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    run(&mut s, "mail.new", json!({"account": account, "to": "kit@example.net", "subject": "Chart"}));
    run(&mut s, "compose.type", json!({"text": "Look: "}));
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(40)), "file": "chart.png", "alt": "Sales"}));
    // Saved as a draft: it goes up as a draft of the server's, the picture an inline attachment.
    run(&mut s, "compose.saveDraft", json!({}));
    let saved = compose(&s).draft.expect("saved");
    let draft = super::graph_drafts::draft_up_whole(s.mailbox.message(saved).unwrap()).expect("a draft goes up whole");
    let here = draft.attachments.clone().expect("with its attachments").here;
    let pic = here.iter().find(|a| a.inline).expect("the picture is an inline attachment");
    assert!(pic.content_id.starts_with("pic-") && pic.mime == "image/png" && pic.name == "chart.png" && pic.data.starts_with(b"\x89PNG"), "{pic:?}");
    let body = draft.body.clone().expect("the body goes up");
    assert!(body.html && body.content.contains(&format!("cid:{}", pic.content_id)), "{}", body.content);
    // Sent without a draft of the server's: the whole message goes as MIME, related.
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let up = super::graph_send::sends(&s, account).into_iter().find(|up| up.message == id).expect("queued to send");
    let m = up.draft.is_none().then_some(up.mime).flatten().map(|m| *m).unwrap_or_else(|| s.mailbox.message(id).unwrap().clone());
    assert!(m.attachments.iter().any(|a| a.inline && a.content_id.starts_with("pic-")));
    let wire = String::from_utf8_lossy(&mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())).to_string();
    assert!(wire.contains("multipart/related") && wire.contains("Content-ID: <pic-"), "{wire}");
}

#[test]
fn a_hostile_paste_cannot_bring_a_picture_in_by_its_cid() {
    use mailcraft_model::Attachment;
    let mut s = message();
    let mine = png_bytes(8);
    run(&mut s, "insert.picture", json!({"data": b64(&mine), "file": "chart.png", "alt": "Sales", "width": 120, "at": 5}));
    let attached = Attachment { content_id: "report@example.org".into(), inline: true, ..Attachment::new("r.png", "image/png", png_bytes(9)) };
    s.compose.as_mut().unwrap().attachments.push(attached.clone());
    let before = compose(&s).clone();
    let fp = super::format::fingerprint;
    let theirs = png_bytes(9);
    for src in [
        // An id it doesn't hold, with the bytes of a picture of the paste's own.
        format!("cid:pic-{}.{}@mailcraft.invalid", FIRST_ID + 1, fp(&theirs)),
        // Its picture's id, with other bytes.
        format!("cid:pic-{FIRST_ID}.{}@mailcraft.invalid", fp(&theirs)),
        // Its picture, named as a signature's, written otherwise, or as a draft names it.
        format!("cid:sig-{FIRST_ID}.{}@mailcraft.invalid", fp(&mine)),
        format!("cid:pic-0{FIRST_ID}.{}@mailcraft.invalid", fp(&mine)),
        format!("cid:PIC-{FIRST_ID}.{}@MAILCRAFT.INVALID", fp(&mine).to_uppercase()),
        format!("cid:pic-{FIRST_ID}.{}@mailcraft.invalid.example", fp(&mine)),
        format!("cid:pic-{FIRST_ID}.x@mailcraft.invalid"),
        // An attachment of the message, a picture in the HTML itself, one on the web.
        format!("cid:{}", attached.content_id),
        format!("data:image/png;base64,{}", b64(&theirs)),
        "https://example.org/a.png".to_string(),
    ] {
        let _ = s.run("edit.paste", &json!({"html": format!("<div><img src=\"{src}\" alt=\"x\" width=\"9999\"></div>")}));
        assert_eq!(*compose(&s), before, "{src}");
    }
    // Only the picture it holds is shown again, as the message has it, not as the paste says.
    let held = format!("cid:pic-{FIRST_ID}.{}@mailcraft.invalid", fp(&mine));
    run(&mut s, "edit.paste", json!({"html": format!("<div><img src=\"{held}\" alt=\"Not this\" width=\"9999\"></div>")}));
    let c = compose(&s);
    assert_eq!((c.body.text().pictures(), c.pictures.len(), c.attachments.len()), (vec![FIRST_ID, FIRST_ID], 1, 1));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert_eq!((html.matches("alt=\"Sales\"").count(), html.matches("width=\"120\"").count()), (2, 2), "{html}");
    assert!(!html.contains("Not this") && !html.contains("9999"), "{html}");
}

#[test]
fn a_paste_cannot_make_a_picture_by_writing_its_mark_as_an_entity() {
    let mut s = message();
    let mine = png_bytes(8);
    run(&mut s, "insert.picture", json!({"data": b64(&mine), "file": "chart.png", "at": 5}));
    // The mark a picture stands by in the text, written as entities: of its picture, of a
    // signature's, as often as a message holds pictures and more.
    let forged = |id: u32| format!("&#xFFFC;{id}&#xFFFC;");
    let pasted = format!("<div>x{}</div>", [forged(FIRST_ID), forged(1)].concat().repeat(1_000));
    run(&mut s, "edit.paste", json!({"html": pasted}));
    assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID]);
    // Beside the picture it names by its cid, it is still the one picture.
    let held = format!("<img src=\"cid:pic-{FIRST_ID}.{}@mailcraft.invalid\">", super::format::fingerprint(&mine));
    run(&mut s, "edit.paste", json!({"html": format!("<div>{}{held}{}</div>", forged(FIRST_ID).repeat(100), forged(FIRST_ID))}));
    assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID, FIRST_ID]);
}

#[test]
fn a_draft_naming_its_picture_more_often_than_a_message_holds_opens_with_as_many_as_it_holds() {
    use mailcraft_model::pictures::MAX_PICTURES;
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8)), "file": "chart.png", "at": 5}));
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = compose(&s).draft.expect("saved");
    run(&mut s, "compose.close", json!({}));
    let m = s.mailbox.message_mut(draft).unwrap();
    let html = m.body.html.clone().unwrap();
    let start = html.find("<img").unwrap();
    let tag = html[start..start + html[start..].find('>').unwrap() + 1].to_string();
    m.body.html = Some(html.replacen(&tag, &tag.repeat(MAX_PICTURES + 5), 1));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert_eq!(compose(&s).body.text().pictures(), vec![FIRST_ID; MAX_PICTURES]);
}

#[test]
fn a_picture_put_in_one_or_two_pixels_wide_is_cut_and_pasted_back_and_its_draft_reopens_with_it() {
    for width in [1, 2] {
        let mut s = message();
        run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8)), "file": "dot.png", "alt": "Dot", "width": width, "at": 5}));
        let cut = run(&mut s, "edit.cut", json!({"from": 5, "to": 6}));
        assert!(compose(&s).body.text().pictures().is_empty());
        run(&mut s, "compose.select", json!({"from": 5}));
        run(&mut s, "edit.paste", json!({"html": cut["html"], "text": cut["text"]}));
        assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID], "width {width}: {cut}");
        run(&mut s, "compose.saveDraft", json!({}));
        let draft = compose(&s).draft.expect("saved");
        run(&mut s, "compose.close", json!({}));
        run(&mut s, "mail.editDraft", json!({"id": draft}));
        assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID], "width {width}");
        // As narrow as a picture is shown.
        assert_eq!(compose(&s).pictures[0].width, Some(mailcraft_model::pictures::MIN_SHOWN_WIDTH));
    }
}

#[test]
fn a_draft_whose_picture_is_written_one_or_two_pixels_wide_reopens_with_it_among_the_attachments() {
    // Only a command could make a picture that narrow before; the reading pane takes it for a
    // tracker, so the text does not take it back, and it is not lost either.
    for width in [1, 2] {
        let mut s = message();
        run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8)), "file": "chart.png", "alt": "Chart", "width": 120, "at": 5}));
        run(&mut s, "compose.saveDraft", json!({}));
        let draft = compose(&s).draft.expect("saved");
        run(&mut s, "compose.close", json!({}));
        let m = s.mailbox.message_mut(draft).unwrap();
        let html = m.body.html.clone().unwrap();
        assert!(html.contains("@mailcraft.invalid\" width=\"120\" alt=\"Chart\""), "{html}");
        m.body.html = Some(html.replace("width=\"120\"", &format!("width=\"{width}\"")));
        run(&mut s, "mail.editDraft", json!({"id": draft}));
        let c = compose(&s);
        assert_eq!((c.body.plain(), c.body.text().pictures().len(), c.pictures.len()), ("Hello world".to_string(), 0, 0), "width {width}");
        assert_eq!(c.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["chart.png"], "width {width}");
    }
}

#[test]
fn a_draft_whose_picture_comes_past_the_most_characters_a_text_holds_keeps_it_among_the_attachments() {
    // The text is cut short before the picture, so it is not in the text: it is not lost either,
    // reopened or saved again.
    let mut s = message();
    run(&mut s, "insert.picture", json!({"data": b64(&png_bytes(8)), "file": "chart.png", "alt": "Chart", "at": 5}));
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = compose(&s).draft.expect("saved");
    run(&mut s, "compose.close", json!({}));
    let m = s.mailbox.message_mut(draft).unwrap();
    let html = m.body.html.clone().unwrap();
    let at = html.find("<img").unwrap();
    m.body.html = Some(format!("{}{}{}", &html[..at], "x".repeat(mailcraft_model::rich::MAX_CHARS), &html[at..]));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let c = compose(&s);
    assert_eq!((c.body.text().pictures().len(), c.pictures.len()), (0, 0));
    assert_eq!(c.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["chart.png"]);
    run(&mut s, "compose.saveDraft", json!({}));
    let saved = s.mailbox.message(compose(&s).draft.expect("saved")).unwrap();
    assert_eq!(saved.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["chart.png"]);
}

#[test]
fn a_draft_with_another_attachment_named_as_its_picture_is_reopened_and_saved_with_both() {
    // A draft written by hand: the picture and a file share one Content-ID. The text takes back
    // the picture; the file stays an attachment, and saving keeps both.
    let cid = "pic-1048576.a@mailcraft.invalid";
    let file = mailcraft_model::Attachment {
        content_id: cid.into(),
        inline: true,
        ..mailcraft_model::Attachment::new("notes.txt", "text/plain", b"notes".to_vec())
    };
    let html = format!("<html><body><div>see <img src=\"cid:{cid}\"></div></body></html>");
    let mut s = session();
    let id = hand_draft(&mut s, &html, vec![inline_png(cid, &png_bytes(8)), file]);
    run(&mut s, "mail.editDraft", json!({"id": id}));
    assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID]);
    assert_eq!(compose(&s).attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["notes.txt"]);
    run(&mut s, "compose.saveDraft", json!({}));
    let saved = s.mailbox.message(compose(&s).draft.expect("saved")).unwrap();
    let mut names: Vec<&str> = saved.attachments.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["notes.txt", "p.png"]);
}

#[test]
fn a_draft_naming_its_picture_by_another_cid_shows_it_only_by_its_own() {
    use mailcraft_model::pictures::MAX_PICTURES;
    use mailcraft_model::{Attachment, Body};
    let mut s = session();
    let drafts = s.mailbox.folder_of_kind(s.mailbox.default_account().unwrap().id, mailcraft_model::FolderKind::Drafts).unwrap();
    let first =
        Attachment { content_id: "pic-1048576.a@mailcraft.invalid".into(), inline: true, ..Attachment::new("a.png", "image/png", png_bytes(8)) };
    let second =
        Attachment { content_id: "pic-1048577.b@mailcraft.invalid".into(), inline: true, ..Attachment::new("b.png", "image/png", png_bytes(9)) };
    // The first picture's id under a cid it was never attached by, as often as fills the rest.
    let aliases = "<img src=\"cid:pic-1048576.missing@example.org\">".repeat(MAX_PICTURES - 1);
    let html = format!("<html><body><div><img src=\"cid:{}\">{aliases}<img src=\"cid:{}\"></div></body></html>", first.content_id, second.content_id);
    let m = mailcraft_model::Message {
        account: s.mailbox.default_account().unwrap().id,
        folder: drafts,
        draft: true,
        subject: "hand".into(),
        body: Body { text: "x".into(), html: Some(html) },
        attachments: vec![first, second],
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    run(&mut s, "mail.editDraft", json!({"id": id}));
    assert_eq!(compose(&s).body.text().pictures(), [FIRST_ID, FIRST_ID + 1]);
}

/// A draft written by hand, with Message-ID `a@example.org`: its id.
fn hand_draft(s: &mut Session, html: &str, attachments: Vec<mailcraft_model::Attachment>) -> u64 {
    let account = s.mailbox.default_account().unwrap().id;
    let drafts = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Drafts).unwrap();
    let m = mailcraft_model::Message {
        account,
        folder: drafts,
        draft: true,
        subject: "hand".into(),
        message_id: "a@example.org".into(),
        body: mailcraft_model::Body { text: "x".into(), html: Some(html.to_string()) },
        attachments,
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

/// An inline PNG attachment named `cid`.
fn inline_png(cid: &str, bytes: &[u8]) -> mailcraft_model::Attachment {
    mailcraft_model::Attachment { content_id: cid.into(), inline: true, ..mailcraft_model::Attachment::new("p.png", "image/png", bytes.to_vec()) }
}

/// What each `cid:` of the saved draft `id`'s HTML names, in order: the bytes of the one
/// attachment by that name, or `None` when there isn't exactly one; and whether each payload of
/// `bytes` is there exactly once.
fn resolved(s: &Session, id: u64, bytes: &[&[u8]]) -> (Vec<Option<Vec<u8>>>, bool) {
    let m = s.mailbox.message(id).unwrap();
    let html = m.body.html.clone().unwrap();
    let lower = html.to_ascii_lowercase();
    let named = lower.match_indices("cid:").map(|(at, _)| html[at + 4..].split('"').next().unwrap().to_string());
    let found = named
        .map(|cid| {
            let by: Vec<_> = m.attachments.iter().filter(|a| a.content_id.eq_ignore_ascii_case(&cid)).collect();
            (by.len() == 1).then(|| by[0].data.to_vec())
        })
        .collect();
    let once = bytes.iter().all(|b| m.attachments.iter().filter(|a| a.data.as_slice() == *b).count() == 1);
    (found, once)
}

#[test]
fn a_draft_taking_back_one_of_two_pictures_of_the_same_id_keeps_the_other_as_an_attachment() {
    let (a, b) = (png_bytes(8), png_bytes(9));
    let mut s = session();
    let html = "<html><body><div><img src=\"cid:pic-1048576.b@mailcraft.invalid\"></div></body></html>";
    // The Message-ID names the picture a save writes as the other attachment is named.
    let id = hand_draft(&mut s, html, vec![inline_png("pic-1048576.a@mailcraft.invalid", &a), inline_png("pic-1048576.b@mailcraft.invalid", &b)]);
    for round in 0..2 {
        run(&mut s, "mail.editDraft", json!({"id": id}));
        // The one the text names is its picture; the other stays an attachment, as it was.
        let c = compose(&s);
        let kept: Vec<(&str, bool, Vec<u8>)> = c.attachments.iter().map(|x| (x.content_id.as_str(), x.inline, x.data.to_vec())).collect();
        assert_eq!(kept, vec![("pic-1048576.a@mailcraft.invalid", true, a.clone())], "round {round}");
        assert_eq!(
            (c.body.text().pictures(), c.pictures.iter().map(|p| p.data.to_vec()).collect::<Vec<_>>()),
            (vec![FIRST_ID], vec![b.clone()]),
            "round {round}"
        );
        // Saved, the text's picture is named by a cid nothing else has, and each is there once.
        run(&mut s, "compose.saveDraft", json!({}));
        assert_eq!(resolved(&s, id, &[&a, &b]), (vec![Some(b.clone())], true), "round {round}");
        run(&mut s, "compose.close", json!({}));
    }
}

#[test]
fn a_draft_whose_quote_shows_a_picture_by_the_name_a_save_gives_its_own_keeps_both() {
    let (a, b) = (png_bytes(8), png_bytes(9));
    let mut s = session();
    let html = "<html><body><img src=\"cid:pic-1048576.b@mailcraft.invalid\"><div id=\"mailcraft-quote\"><img src=\"cid:pic-1048576.a@mailcraft.invalid\"></div></body></html>";
    let id = hand_draft(&mut s, html, vec![inline_png("pic-1048576.a@mailcraft.invalid", &a), inline_png("pic-1048576.b@mailcraft.invalid", &b)]);
    for round in 0..2 {
        run(&mut s, "mail.editDraft", json!({"id": id}));
        let c = compose(&s);
        assert_eq!(c.pictures.iter().map(|p| p.data.to_vec()).collect::<Vec<_>>(), vec![b.clone()], "round {round}");
        assert!(c.quote.as_ref().unwrap().html.contains("cid:pic-1048576.a@mailcraft.invalid"), "round {round}");
        // Saved, the text shows B and the quote A, by cids of their own, each once.
        run(&mut s, "compose.saveDraft", json!({}));
        assert_eq!(resolved(&s, id, &[&a, &b]), (vec![Some(b.clone()), Some(a.clone())], true), "round {round}");
        run(&mut s, "compose.close", json!({}));
    }
}

#[test]
fn a_picture_the_quote_names_by_an_uppercase_scheme_keeps_its_name_through_a_save() {
    let a = png_bytes(8);
    let mut s = session();
    let html = "<html><body><div>x</div><div id=\"mailcraft-quote\"><img src=\"CID:pic-1048576.a@mailcraft.invalid\"></div></body></html>";
    let id = hand_draft(&mut s, html, vec![inline_png("pic-1048576.a@mailcraft.invalid", &a)]);
    for round in 0..2 {
        run(&mut s, "mail.editDraft", json!({"id": id}));
        run(&mut s, "compose.saveDraft", json!({}));
        let m = s.mailbox.message(id).unwrap();
        let kept: Vec<(&str, bool)> = m.attachments.iter().map(|x| (x.content_id.as_str(), x.inline)).collect();
        assert_eq!(kept, vec![("pic-1048576.a@mailcraft.invalid", true)], "round {round}");
        assert_eq!(resolved(&s, id, &[&a]), (vec![Some(a.clone())], true), "round {round}");
        run(&mut s, "compose.close", json!({}));
    }
}

#[test]
fn a_new_picture_over_one_selected_when_the_messages_pictures_weigh_as_much_as_they_may_is_refused_and_changes_nothing() {
    let mut s = message();
    for n in 0..4 {
        run(&mut s, "insert.picture", json!({"data": b64(&png_bytes((5 << 20) - 20 - n)), "at": 0}));
    }
    // It fits only if the one it takes the place of is let go: refused, and nothing changes.
    run(&mut s, "compose.select", json!({"from": 0, "to": 1}));
    let (text, held) = (compose(&s).body.text().clone(), compose(&s).pictures.clone());
    let e = s.run("insert.picture", &json!({"data": b64(&png_bytes(50 * 1024))})).unwrap_err().to_string();
    assert!(e.contains("20 MB together"), "{e}");
    assert_eq!((compose(&s).body.text(), &compose(&s).pictures), (&text, &held));
}

/// A draft whose picture's name a save would give, and the `taken - 1` after it, are names of
/// its files (empty, not shown), reopened in HTML or in plain text: the session, its id and the
/// picture's bytes.
fn draft_with_taken_names(taken: usize, plain: bool) -> (Session, u64, Vec<u8>) {
    let png = png_bytes(8);
    let mut attachments = vec![inline_png("pic-1048576.z@mailcraft.invalid", &png)];
    attachments.extend((0..taken).map(|n| {
        let cid = if n == 0 { "pic-1048576.a@mailcraft.invalid".to_string() } else { format!("pic-1048576.a-{n}@mailcraft.invalid") };
        mailcraft_model::Attachment { content_id: cid, ..mailcraft_model::Attachment::new("x.bin", "application/octet-stream", Vec::new()) }
    }));
    let mut s = session();
    let html = "<html><body><div><img src=\"cid:pic-1048576.z@mailcraft.invalid\"></div></body></html>";
    let id = hand_draft(&mut s, html, attachments);
    run(&mut s, "mail.editDraft", json!({"id": id}));
    if plain {
        run(&mut s, "compose.formatPlain", json!({}));
    }
    (s, id, png)
}

#[test]
fn a_draft_whose_attachments_take_many_of_the_names_its_picture_could_have_is_saved_with_a_free_one() {
    const TAKEN: usize = 1_000;
    let (mut s, id, png) = draft_with_taken_names(TAKEN, false);
    run(&mut s, "compose.saveDraft", json!({}));
    assert_eq!(resolved(&s, id, &[&png]), (vec![Some(png.clone())], true));
    let html = s.mailbox.message(id).unwrap().body.html.clone().unwrap();
    assert!(html.contains(&format!("cid:pic-1048576.a-{TAKEN}@mailcraft.invalid")), "{html}");
    // In plain text, which sends no picture, it is saved without one.
    let (mut s, id, _) = draft_with_taken_names(TAKEN, true);
    run(&mut s, "compose.saveDraft", json!({}));
    assert!(s.mailbox.message(id).unwrap().body.html.is_none());
}

#[test]
#[ignore = "a timing check, which a shared runner can make fail: run by hand"]
fn a_draft_whose_attachments_take_a_hundred_thousand_of_the_names_its_picture_could_have_is_saved_in_a_moment() {
    for plain in [false, true] {
        let (mut s, _, _) = draft_with_taken_names(100_000, plain);
        let started = std::time::Instant::now();
        run(&mut s, "compose.saveDraft", json!({}));
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "plain {plain}: {:?}", started.elapsed());
    }
}
