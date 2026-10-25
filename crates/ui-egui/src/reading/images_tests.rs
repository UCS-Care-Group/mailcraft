// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The budget of textures for the pictures of the message on screen holds, however large the
//! pictures are that the message declares small.

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use serde_json::json;

use super::images::{CACHE_BYTES, CACHE_PICTURES, Pictures};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// A flat PNG of `w`×`h` pixels (a few kilobytes however large).
fn flat(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(w, h, image::Rgba([0x1f, 0x5f, 0x8b, 0xff]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A message of `count` pictures, each declared `shown`×`shown` points, the n-th `w`×(`h`+n) pixels.
fn message(count: u32, w: u32, h: u32, shown: u32) -> Message {
    let html: String = (0..count).map(|i| format!("<img src=\"cid:p{i}@example.org\" width={shown} height={shown}>")).collect();
    Message {
        from: Address::new("A", "a@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@lumen-works.example")],
        subject: "Pictures".into(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        read: true,
        body: Body { text: String::new(), html: Some(html) },
        headers: vec![("Authentication-Results".into(), "mx.example.org; dmarc=pass header.from=example.org".into())],
        attachments: (0..count)
            .map(|i| Attachment {
                content_id: format!("p{i}@example.org"),
                inline: true,
                ..Attachment::new(format!("p{i}.png"), "image/png", flat(w, h + i))
            })
            .collect(),
        ..Message::default()
    }
}

/// A window on a mailbox of the given message and one without pictures; both ids.
fn window(m: Message) -> (egui_kittest::Harness<'static, MailApp>, u64, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    let inbox = mailbox.inbox().unwrap();
    let mut plain = message(0, 1, 1, 1);
    plain.subject = "Nothing to see".into();
    plain.folder = inbox;
    let plain = mailbox.add_message(plain).unwrap();
    let mut m = m;
    m.folder = inbox;
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness.ctx.input_mut(|i| i.max_texture_side = 8192);
    (harness, id, plain)
}

/// How many pictures are kept, how many have a texture, and the bytes of those textures.
fn held(harness: &egui_kittest::Harness<'static, MailApp>) -> (usize, usize, usize) {
    let pictures = Pictures::of(&harness.ctx);
    let pictures = pictures.lock().unwrap();
    let (kept, decoded) = pictures.counts();
    (kept, decoded, pictures.texture_bytes())
}

#[test]
fn tall_pictures_declared_small_take_textures_of_the_size_they_are_shown_at() {
    let (mut harness, id, _) = window(message(40, 860, 8192, 4));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(80);
    let (kept, decoded, bytes) = held(&harness);
    assert_eq!(kept, 40);
    assert!(decoded > 0, "no texture was made");
    assert!(bytes <= CACHE_BYTES, "{bytes} bytes of textures");
    // 4×4 points: a texture is a few pixels wide, not the pane's width.
    assert!(bytes <= 8 * 1024 * 1024, "{decoded} textures, {bytes} bytes");
}

#[test]
fn pictures_on_screen_do_not_pass_the_picture_count() {
    let (mut harness, id, _) = window(message(100, 16, 16, 4));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(200);
    let (_, decoded, bytes) = held(&harness);
    assert!(decoded <= CACHE_PICTURES, "{decoded} textures");
    assert!(bytes <= CACHE_BYTES);
}

#[test]
fn a_picture_is_given_a_texture_for_the_size_it_is_shown_at() {
    let (mut harness, id, _) = window(message(1, 800, 600, 400));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(10);
    let (_, decoded, bytes) = held(&harness);
    assert_eq!(decoded, 1);
    // Not the picture's own 800 pixels across, and not much less than the 400 points asked for.
    assert!((300 * 225 * 4..=700 * 525 * 4).contains(&bytes), "{bytes}");
}

#[test]
fn pictures_scrolled_out_and_back_are_not_decoded_again() {
    let (mut harness, id, plain) = window(message(10, 64, 64, 32));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(40);
    let (_, decoded, _) = held(&harness);
    let decodes = Pictures::of(&harness.ctx).lock().unwrap().decodes();
    assert!(decoded > 0 && decodes >= decoded);
    // Frames with no picture in them (a gap in a long message, or another message) keep them.
    harness.state_mut().execute("mail.select", json!({"id": plain})).unwrap();
    harness.run_steps(10);
    assert_eq!(held(&harness).1, decoded);
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(10);
    assert_eq!(held(&harness).1, decoded);
    assert_eq!(Pictures::of(&harness.ctx).lock().unwrap().decodes(), decodes);
}

#[test]
fn textures_are_let_go_when_no_picture_has_been_shown_for_a_while() {
    let (mut harness, id, plain) = window(message(10, 64, 64, 32));
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(40);
    assert!(held(&harness).1 > 0);
    harness.state_mut().execute("mail.select", json!({"id": plain})).unwrap();
    harness.run_steps(4);
    assert!(held(&harness).1 > 0);
    Pictures::of(&harness.ctx).lock().unwrap().release_after(6.0);
    assert_eq!(held(&harness).1, 0);
    assert_eq!(held(&harness).2, 0);
    // And they come back with the message.
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(40);
    assert!(held(&harness).1 > 0);
}
