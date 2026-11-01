// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A newsletter with `@media` (#599): its look is worked out for the class of the pane's width and
//! the theme, once for each, kept with its blocks, and never per frame; a dark window uses the
//! sender's own dark palette.

use mailcraft_html::corpus::{DARK_NEWSLETTER, RESPONSIVE_NEWSLETTER};
use mailcraft_model::Mailbox;
use serde_json::json;

use super::super::Place;
use super::super::layout::ItemKind;
use super::super::view;
use super::{NOW, Window, html_message, select, served_by_example_org, window};
use crate::{MailApp, Services};
use mailcraft_engine::Session;

/// A mailbox with a responsive newsletter, a dark-palette one and a plain one; their ids.
fn app() -> (MailApp, u64, u64, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    served_by_example_org(&mut mailbox);
    let inbox = mailbox.inbox().unwrap();
    let mut add = |n: usize, subject: &str, html: &str| {
        let mut m = html_message(n, ("Saltmarsh Dairy", "news@saltmarsh.example"), subject, html);
        m.folder = inbox;
        mailbox.add_message(m).unwrap()
    };
    let responsive = add(0, "The autumn cheeses", RESPONSIVE_NEWSLETTER);
    let dark = add(1, "The Night Post", DARK_NEWSLETTER);
    let plain = add(2, "Lunch", "<p>Lunch on Friday?</p><style>p{color:#036}</style>");
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    (app, responsive, dark, plain)
}

fn text(harness: &Window, id: u64) -> String {
    let doc = view::current_doc(&harness.ctx, Place::Pane, id).expect("a body is kept");
    mailcraft_html::blocks_to_text(&doc.blocks)
}

/// Run `n` frames, noting after each the look of the message in use.
fn frames(harness: &mut Window, id: u64, n: usize, seen: &mut std::collections::HashSet<view::Look>) {
    for _ in 0..n {
        harness.step();
        if let Some(look) = view::current_look(&harness.ctx, Place::Pane, id) {
            seen.insert(look);
        }
    }
}

/// The blocks are made once for each class of pane and theme the message is seen in: not once per
/// frame, and not again when the pane comes back to a class it was in.
#[test]
fn the_look_is_worked_out_once_per_message_width_class_and_theme() {
    let (app, responsive, _, plain) = app();
    let mut harness = window(app, (1400.0, 900.0));
    let mut seen = std::collections::HashSet::new();
    harness.state_mut().execute("mail.select", json!({"id": responsive})).unwrap();
    frames(&mut harness, responsive, 40, &mut seen);
    assert!(text(&harness, responsive).contains("Desktop copy"));
    // However many frames, the blocks are made once for each class the pane was in.
    let made = view::docs_made(&harness.ctx);
    assert!(made >= 1 && made == seen.len(), "{made} made, {} classes seen: {seen:?}", seen.len());
    frames(&mut harness, responsive, 100, &mut seen);
    assert_eq!(view::docs_made(&harness.ctx), made);
    let begun = view::layouts_begun(&harness.ctx);
    let wide = view::current_look(&harness.ctx, Place::Pane, responsive).unwrap();

    // Panes of other widths, and back, again and again: a class is made once.
    let mut classes = std::collections::HashSet::from([wide]);
    for w in [900.0, 1400.0, 1100.0, 700.0, 1400.0, 900.0, 700.0, 1100.0] {
        harness.set_size(egui::vec2(w, 900.0));
        frames(&mut harness, responsive, 12, &mut seen);
        classes.insert(view::current_look(&harness.ctx, Place::Pane, responsive).unwrap());
        assert_eq!(view::docs_made(&harness.ctx), seen.len(), "{seen:?}");
    }
    assert!(classes.len() >= 3, "{classes:?}");
    // Dragging the edge by a few points, inside one class, makes nothing.
    let before = view::docs_made(&harness.ctx);
    let class = view::current_look(&harness.ctx, Place::Pane, responsive).unwrap();
    for w in [1101.0, 1103.0, 1104.0, 1102.0] {
        harness.set_size(egui::vec2(w, 900.0));
        frames(&mut harness, responsive, 4, &mut seen);
    }
    assert_eq!(view::current_look(&harness.ctx, Place::Pane, responsive).unwrap(), class);
    assert_eq!(view::docs_made(&harness.ctx), before);

    // The other theme is another look, made once.
    harness.state_mut().execute("ui.theme", json!({"value": "dark"})).unwrap();
    frames(&mut harness, responsive, 12, &mut seen);
    assert!(view::current_look(&harness.ctx, Place::Pane, responsive).unwrap().dark());
    assert_eq!(view::docs_made(&harness.ctx), before + 1);
    frames(&mut harness, responsive, 100, &mut seen);
    assert_eq!(view::docs_made(&harness.ctx), before + 1);
    // Layouts were begun for the changes and not for the frames in between.
    assert!(view::layouts_begun(&harness.ctx) > begun);

    // A message with a style sheet and no `@media` is not read again for any of it.
    harness.state_mut().execute("ui.theme", json!({"value": "light"})).unwrap();
    select(&mut harness, plain);
    harness.run_steps(12);
    let made = view::docs_made(&harness.ctx);
    for w in [1400.0, 500.0, 900.0, 400.0] {
        harness.set_size(egui::vec2(w, 900.0));
        harness.run_steps(6);
    }
    harness.state_mut().execute("ui.theme", json!({"value": "dark"})).unwrap();
    harness.run_steps(12);
    assert_eq!(view::docs_made(&harness.ctx), made);
}

/// The copy for the pane is the one shown: the mobile copy in a narrow pane, the desktop copy in a wide one.
#[test]
fn the_pane_shows_the_copy_for_its_width() {
    let (app, responsive, ..) = app();
    let mut harness = window(app, (1400.0, 900.0));
    select(&mut harness, responsive);
    harness.run_steps(12);
    let wide = text(&harness, responsive);
    assert!(wide.contains("Desktop copy") && !wide.contains("Mobile copy"), "{wide}");
    harness.set_size(egui::vec2(420.0, 900.0));
    harness.run_steps(12);
    let narrow = text(&harness, responsive);
    assert!(narrow.contains("Mobile copy") && !narrow.contains("Desktop copy"), "{narrow}");
}

/// In a dark window a newsletter with a dark palette is shown in it: its backgrounds are the
/// sender's own, and text that reads well on them is not touched.
#[test]
fn a_dark_window_uses_the_senders_dark_palette() {
    let (mut app, _, dark_id, _) = app();
    app.execute("ui.theme", json!({"value": "dark"})).unwrap();
    let mut harness = window(app, (1400.0, 900.0));
    select(&mut harness, dark_id);
    harness.run_steps(12);
    let doc = view::current_doc(&harness.ctx, Place::Pane, dark_id).unwrap();
    assert!(doc.sender_dark);
    let t = text(&harness, dark_id);
    assert!(t.contains("dark edition") && !t.contains("light edition"), "{t}");
    // Its card is the sender's own dark card, and its text colours on it are its own, kept as
    // they are because they can be read.
    let (paper, inks) = view::kept(&harness.ctx, dark_id, |laid, _| {
        let item = laid.items.iter().find_map(|i| match &i.kind {
            ItemKind::Text(t) if t.job.text.contains("Good evening") => Some(t),
            _ => None,
        });
        let item = item.expect("the first paragraph");
        (item.paper, item.job.sections.iter().map(|s| s.format.color).collect::<Vec<_>>())
    })
    .unwrap();
    assert_eq!(paper, egui::Color32::from_rgb(0x1f, 0x23, 0x29));
    assert!(inks.contains(&egui::Color32::from_rgb(0xe8, 0xe6, 0xe3)) && inks.contains(&egui::Color32::from_rgb(0xff, 0x8a, 0x80)), "{inks:?}");
    // In a light window it is the light edition, and no dark design.
    harness.state_mut().execute("ui.theme", json!({"value": "light"})).unwrap();
    harness.run_steps(12);
    let doc = view::current_doc(&harness.ctx, Place::Pane, dark_id).unwrap();
    assert!(!doc.sender_dark);
    assert!(text(&harness, dark_id).contains("light edition"));
}

/// A dark window that keeps messages on a light page (#634) shows a newsletter's light edition
/// there, without its dark design, and makes its blocks once: the first guess at the theme, made
/// before the body is laid out, is the page's and not the window's.
#[test]
fn a_light_page_in_a_dark_window_shows_the_light_edition() {
    let (mut app, _, dark_id, _) = app();
    app.execute("ui.theme", json!({"value": "dark"})).unwrap();
    app.execute("ui.lightMessages", json!({"value": true})).unwrap();
    let mut harness = window(app, (1400.0, 900.0));
    select(&mut harness, dark_id);
    harness.run_steps(12);
    assert!(harness.ctx.global_style().visuals.dark_mode, "the window stays dark");
    let doc = view::current_doc(&harness.ctx, Place::Pane, dark_id).unwrap();
    assert!(!doc.sender_dark);
    assert!(!view::current_look(&harness.ctx, Place::Pane, dark_id).unwrap().dark());
    let t = text(&harness, dark_id);
    assert!(t.contains("light edition") && !t.contains("dark edition"), "{t}");
    assert_eq!(view::docs_made(&harness.ctx), 1);
}

/// With the sender's own dark design, its backgrounds are painted as asked, and a text colour is
/// changed only when it cannot be read on them; without it a light background is turned dark.
#[test]
fn the_senders_dark_design_is_adjusted_only_where_it_falls_short() {
    use super::super::colors::{MIN_CONTRAST, Paper, c32, contrast, readable};
    use crate::theme::Tokens;
    use mailcraft_html::Rgb;
    let (own, window) = (Tokens { sender_dark: true, ..Tokens::dark() }, Tokens::dark());
    for bg in [Rgb(0x1f, 0x23, 0x29), Rgb(0xf4, 0xf1, 0xea), Rgb(255, 255, 255), Rgb(0x0b, 0x3d, 0x75)] {
        assert_eq!(Paper::of(bg, &own).fill, c32(bg), "{bg:?}");
    }
    assert_ne!(Paper::of(Rgb(255, 255, 255), &window).fill, c32(Rgb(255, 255, 255)));
    // A palette that reads is left alone, pair by pair.
    for (ink, bg) in [(0xe8e6e3, 0x1f2329), (0xff8a80, 0x1f2329), (0x8ab4f8, 0x14161a), (0xf2f6fc, 0x0b3d75), (0x222222, 0xf4f1ea)] {
        let (ink, paper) =
            (c32(Rgb((ink >> 16) as u8, (ink >> 8) as u8, ink as u8)), Paper::of(Rgb((bg >> 16) as u8, (bg >> 8) as u8, bg as u8), &own));
        assert!(contrast(ink, paper.fill) >= MIN_CONTRAST);
        assert_eq!(readable(ink, paper.fill), ink);
    }
    // A pair that falls short is changed.
    let (ink, paper) = (c32(Rgb(0x30, 0x30, 0x40)), Paper::of(Rgb(0x1f, 0x23, 0x29), &own));
    assert!(readable(ink, paper.fill) != ink && contrast(readable(ink, paper.fill), paper.fill) >= MIN_CONTRAST);
}

/// The look is for the width the body is laid out at, not the pane's (#599 review): a message
/// with a condition at every few pixels finds its edge within a few pixels of the layout's width,
/// whatever the spacing of its `media` attribute.
#[test]
fn the_look_is_for_the_width_the_body_is_laid_out_at() {
    let (mut mailbox, id) = {
        let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
        served_by_example_org(&mut mailbox);
        let inbox = mailbox.inbox().unwrap();
        let lists: String = (0..390).map(|i| format!("@media (max-width:{}px){{p{{color:#036}}}}", 100 + i * 3)).collect();
        let html = format!("<style>{lists}</style><style media = \"(max-width: 5000px)\">p{{margin:0}}</style><p>Wide enough to see</p>");
        let mut m = html_message(0, ("Saltmarsh Dairy", "news@saltmarsh.example"), "Lists", &html);
        m.folder = inbox;
        let id = mailbox.add_message(m).unwrap();
        (mailbox, id)
    };
    mailbox.repair();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness = window(app, (1100.0, 900.0));
    for w in [1100.0, 900.0, 700.0, 1000.0] {
        harness.set_size(egui::vec2(w, 900.0));
        select(&mut harness, id);
        harness.run_steps(12);
        let look = view::current_look(&harness.ctx, Place::Pane, id).unwrap();
        let laid = view::kept(&harness.ctx, id, |laid, _| laid.width()).unwrap();
        // The first list that holds is the narrowest `max-width` at or above the width.
        let first = look.verdicts().iter().position(|v| *v).expect("a list holds");
        let edge = (100 + first * 3) as f32;
        assert!(edge >= laid - 0.5 && edge - laid <= 3.5, "window {w}: lists say {edge}, body is {laid}");
    }
}

/// A message whose only dependence on the pane is a `<style media = "...">`, written with spaces
/// round its `=`, is made again when the window changes (#599 review).
#[test]
fn a_style_with_a_media_attribute_alone_follows_the_pane() {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    served_by_example_org(&mut mailbox);
    let inbox = mailbox.inbox().unwrap();
    let html = "<style media = \"(max-width: 700px)\">p{color:#036}</style><style>p{margin:0}</style><p>Narrow or wide</p>";
    let mut m = html_message(0, ("Saltmarsh Dairy", "news@saltmarsh.example"), "Media attribute", html);
    m.folder = inbox;
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness = window(app, (1800.0, 900.0));
    select(&mut harness, id);
    harness.run_steps(12);
    let doc = view::current_doc(&harness.ctx, Place::Pane, id).unwrap();
    assert!(doc.responsive && doc.lists.len() == 1, "{:?}", doc.lists);
    let colour = |harness: &Window| view::current_doc(&harness.ctx, Place::Pane, id).unwrap().blocks[0].spans[0].color;
    let wide = colour(&harness);
    let made = view::docs_made(&harness.ctx);
    harness.set_size(egui::vec2(400.0, 900.0));
    harness.run_steps(12);
    assert_ne!(colour(&harness), wide);
    assert!(view::docs_made(&harness.ctx) > made);
}
