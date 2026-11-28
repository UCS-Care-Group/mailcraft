// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A plain-text message is read in the font the account it came to writes plain text in; a message
//! with HTML of its own is read as it was made.

use mailcraft_model::{Address, Body, Message};
use serde_json::json;

use super::Place;
use super::layout::ItemKind;
use super::tests::{app_with, select, window};
use super::view::with_laid;
use crate::MailApp;
use crate::font_index::font_index_tests::FontsDir;
use crate::fonts::{Store, family_of};

/// A font that is certainly one: the bundled Inter, standing in for an installed face.
const REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn plain_message(subject: &str, text: &str) -> Message {
    Message {
        from: Address::new("A Sender", "news@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@lumen-works.example")],
        subject: subject.into(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        read: true,
        body: Body { text: text.into(), html: None },
        uid: 1,
        ..Default::default()
    }
}

/// What the reading pane draws a message's text in: the family and size of every run.
fn faces(harness: &egui_kittest::Harness<'static, MailApp>, id: u64) -> Vec<(egui::FontFamily, f32)> {
    with_laid(&harness.ctx, Place::Pane, id, |laid| {
        laid.items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Text(text) => {
                    Some(text.job.sections.iter().map(|s| (s.format.font_id.family.clone(), s.format.font_id.size)).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect()
    })
    .unwrap_or_default()
}

/// What the reading pane draws a message's text in: the colour of every run.
fn inks(harness: &egui_kittest::Harness<'static, MailApp>, id: u64) -> Vec<egui::Color32> {
    with_laid(&harness.ctx, Place::Pane, id, |laid| {
        laid.items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Text(text) => Some(text.job.sections.iter().map(|s| s.format.color).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect()
    })
    .unwrap_or_default()
}

/// Eleven points, as they are drawn: a point is 4/3 of a pixel here, the same conversion the
/// composer makes, so a message is read at the size it is written in (#1549).
const ON_SCREEN: f32 = 11.0 * 4.0 / 3.0;

/// A window on one plain-text message, with Inter installed and the account's plain-text font set
/// to it at `size` (`None` leaves the message's own font standing). The folder comes back with it:
/// the face is read from it, and a folder dropped is a font gone — so each test names its own,
/// since tests run beside each other and a folder made again is a folder emptied.
fn pane(name: &str, size: Option<u8>) -> (egui_kittest::Harness<'static, MailApp>, u64, egui::FontFamily, FontsDir) {
    let dir = FontsDir::new(name);
    dir.put("inter.ttf", REGULAR);
    let (mut app, ids) = app_with(vec![plain_message("In plain text", "Line one.\nLine two.")]);
    let params = match size {
        Some(pt) => json!({"plain": {"font": "Inter", "size": pt}}),
        None => json!({"plain": null}),
    };
    app.run("signature.fonts", params).unwrap();
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    (harness, ids[0], family_of("inter", false), dir)
}

#[test]
fn a_plain_text_message_is_read_in_the_font_its_account_writes_plain_text_in() {
    let (harness, id, inter, _dir) = pane("plainread", Some(11));
    let seen = faces(&harness, id);
    assert!(!seen.is_empty(), "the message was laid out");
    assert!(seen.iter().all(|(family, _)| *family == inter), "every line is read in it: {seen:?}");
    assert!(seen.iter().all(|(_, size)| (*size - ON_SCREEN).abs() < 0.01), "at the size chosen: {seen:?}");
}

#[test]
fn a_plain_text_message_left_alone_is_read_as_it_was() {
    let (harness, id, inter, _dir) = pane("plainnone", None);
    let seen = faces(&harness, id);
    assert!(!seen.is_empty(), "the message was laid out");
    assert!(seen.iter().all(|(family, _)| *family != inter), "nothing was chosen: {seen:?}");
}

#[test]
fn a_message_with_html_is_read_as_it_was_made() {
    let dir = FontsDir::new("plainhtml");
    dir.put("inter.ttf", REGULAR);
    let mut message = plain_message("With html", "The text part.");
    message.body.html = Some("<p>Set in the message's own look</p>".into());
    let (mut app, ids) = app_with(vec![message]);
    app.run("signature.fonts", json!({"plain": {"font": "Inter", "size": 11}})).unwrap();
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let seen = faces(&harness, ids[0]);
    assert!(!seen.is_empty(), "the message was laid out");
    assert!(seen.iter().all(|(family, _)| *family != family_of("inter", false)), "its own look stands: {seen:?}");
}

#[test]
fn choosing_the_font_lays_the_message_out_again() {
    let (mut harness, id, _inter, _dir) = pane("plainagain", None);
    let before = faces(&harness, id);
    assert!(!before.is_empty() && before.iter().all(|(_, size)| (*size - ON_SCREEN).abs() >= 0.01), "not chosen yet: {before:?}");
    // The owner chooses it while the message is on screen: the message is laid out again in it.
    // (That it is drawn in the family, once the face has been read, is what the test above shows;
    // here the size is what can be said without waiting on the thread that reads it.)
    harness.state_mut().run("signature.fonts", json!({"plain": {"font": "Inter", "size": 11}})).unwrap();
    harness.run_steps(6);
    let seen = faces(&harness, id);
    assert!(!seen.is_empty(), "the message was laid out");
    assert!(seen.iter().all(|(_, size)| (*size - ON_SCREEN).abs() < 0.01), "and the message is read at the size chosen: {seen:?}");
}

#[test]
fn a_plain_text_message_is_read_in_the_colour_its_account_asks_for() {
    // An account that chose nothing: the window's own colour is what its plain text is read in.
    let (quiet, quiet_id, _inter, _quiet_dir) = pane("plaincolour_none", None);
    let own = inks(&quiet, quiet_id);
    assert!(!own.is_empty(), "the message was laid out");
    // The same message, its account asked for a colour of its own: read in it, kept readable on
    // the paper it sits on.
    let dir = FontsDir::new("plaincolour");
    let (mut app, ids) = app_with(vec![plain_message("In plain text", "Line one.")]);
    app.run("signature.fonts", json!({"plain": {"color": "#123456"}})).unwrap();
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let chosen = inks(&harness, ids[0]);
    assert!(!chosen.is_empty(), "the message was laid out");
    assert!(chosen.iter().all(|c| !own.contains(c)), "the colour chosen is what it is read in: {chosen:?} vs {own:?}");
}

#[test]
fn the_font_for_new_messages_does_not_read_plain_mail() {
    let dir = FontsDir::new("plainslot");
    dir.put("inter.ttf", REGULAR);
    let (mut app, ids) = app_with(vec![plain_message("In plain text", "Line one.")]);
    app.run("signature.fonts", json!({"new": {"font": "Inter", "size": 11}})).unwrap();
    let mut harness = window(app, (900.0, 700.0));
    Store::use_index(&harness.ctx, dir.index());
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let seen = faces(&harness, ids[0]);
    assert!(!seen.is_empty(), "the message was laid out");
    assert!(seen.iter().all(|(family, _)| *family != family_of("inter", false)), "that font is for what is written, not what is read: {seen:?}");
}
