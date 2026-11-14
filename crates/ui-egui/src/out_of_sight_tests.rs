// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A window out of sight a while lets go of what it keeps to show things again quickly, and
//! shows them again when it is back.

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use serde_json::json;

use crate::out_of_sight::{AFTER, tick_at, wait_from, window_out_of_sight};
use crate::reading::images::Pictures;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// The main window as the system says it is, seen from the logic that runs while it is hidden.
fn out_of_sight_with(info: egui::ViewportInfo, hidden: bool) -> bool {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.viewports.insert(egui::ViewportId::ROOT, info);
    let mut seen = None;
    let _ = ctx.run_logic(&raw, |ctx| seen = Some(window_out_of_sight(ctx, hidden)));
    seen.unwrap_or(false)
}

#[test]
fn out_of_sight_is_minimised_covered_or_hidden_and_never_only_unfocused() {
    let shown = egui::ViewportInfo { minimized: Some(false), occluded: Some(false), focused: Some(true), ..Default::default() };
    assert!(!out_of_sight_with(shown.clone(), false));
    assert!(!out_of_sight_with(egui::ViewportInfo { focused: Some(false), ..shown.clone() }, false));
    assert!(!out_of_sight_with(egui::ViewportInfo::default(), false), "nothing known: in sight");
    assert!(out_of_sight_with(egui::ViewportInfo { minimized: Some(true), ..shown.clone() }, false));
    assert!(out_of_sight_with(egui::ViewportInfo { occluded: Some(true), ..shown.clone() }, false));
    assert!(out_of_sight_with(shown, true), "hidden by the program");
}

#[test]
fn the_wait_is_three_minutes_unless_set() {
    assert_eq!(wait_from(None), AFTER);
    assert_eq!(AFTER, 180.0);
    assert_eq!(wait_from(Some(" 5 ")), 5.0);
    assert_eq!(wait_from(Some("0")), 0.0);
    for junk in ["", "soon", "-1", "NaN", "inf"] {
        assert_eq!(wait_from(Some(junk)), AFTER, "{junk}");
    }
    assert_eq!(wait_from(Some("1e12")), 86_400.0);
}

/// A flat PNG of `w`×`h` pixels.
fn flat(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(w, h, image::Rgba([0x1f, 0x5f, 0x8b, 0xff]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A window showing a message of four pictures written into it; the message's id.
fn window() -> (egui_kittest::Harness<'static, MailApp>, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    let inbox = mailbox.inbox().unwrap();
    let html: String = (0..4).map(|i| format!("<p>Picture {i}</p><img src=\"cid:p{i}@example.org\" width=40 height=40>")).collect();
    let m = Message {
        folder: inbox,
        from: Address::new("A", "a@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@lumen-works.example")],
        subject: "Pictures".into(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        read: true,
        body: Body { text: String::new(), html: Some(html) },
        attachments: (0..4)
            .map(|i| Attachment {
                content_id: format!("p{i}@example.org"),
                inline: true,
                ..Attachment::new(format!("p{i}.png"), "image/png", flat(40, 40 + i))
            })
            .collect(),
        ..Message::default()
    };
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    (harness, id)
}

fn pictures(harness: &egui_kittest::Harness<'static, MailApp>) -> (usize, usize) {
    Pictures::of(&harness.ctx).lock().unwrap().counts()
}

#[test]
fn out_of_sight_a_while_the_pictures_go_and_come_back_when_shown() {
    let (mut harness, id) = window();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(20);
    let (kept, decoded) = pictures(&harness);
    assert!(kept > 0 && decoded > 0, "{kept} kept, {decoded} decoded");

    // Out of sight, not long enough yet: nothing goes.
    let ctx = harness.ctx.clone();
    tick_at(harness.state_mut(), &ctx, 1_000.0, 60.0);
    tick_at(harness.state_mut(), &ctx, 30_000.0, 60.0);
    assert_eq!(pictures(&harness), (kept, decoded));
    assert!(!harness.state().out_of_sight.done());
    // A minute later: everything goes, once.
    tick_at(harness.state_mut(), &ctx, 61_500.0, 60.0);
    assert!(harness.state().out_of_sight.done());
    assert_eq!(pictures(&harness), (0, 0));

    // Back in sight: the message is shown with its pictures again.
    harness.run_steps(20);
    assert!(!harness.state().out_of_sight.done(), "the wait starts again");
    let (kept, decoded) = pictures(&harness);
    assert!(kept > 0 && decoded > 0, "{kept} kept, {decoded} decoded");
}
