// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The room to write under the owner's text (#1732): a signature laid out in the text of a reply
//! ends it, and the original in HTML starts right under it, however short the signature; the
//! room stays where nothing in the text ends it.

use egui::{Event, Modifiers, PointerButton, Rect};
use serde_json::json;

use super::tests::{Window, app, sel, window};

/// The room kept to write in, eight lines.
const ROOM: f32 = 8.0 * 19.0;
/// About a line.
const LINE: f32 = 24.0;

const LONG: &str = "Avery Quinn\nSenior Designer\nLumen Studio\n12 Harbour Street\nPortsmouth\n+44 0000 000000\nlumen.example";

/// The sample's message whose original is in HTML.
fn html_original(a: &crate::MailApp) -> u64 {
    a.session.mailbox.messages.values().find(|m| m.subject == "Your October invoice is ready" && m.body.html.is_some()).map(|m| m.id).unwrap()
}

/// A reply to the HTML original, with `default` as the signature of replies ("" for none).
fn reply(default: &str) -> Window {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Long", "text": LONG})).unwrap();
    a.execute("signature.set", json!({"name": "Short", "text": "Avery"})).unwrap();
    a.execute("signature.defaults", json!({"reply": default})).unwrap();
    let id = html_original(&a);
    a.execute("mail.reply", json!({"id": id})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    h
}

fn rect(h: &Window, kind: &str) -> Rect {
    h.state().hit.rect(kind, 0).unwrap_or_else(|| panic!("{kind} is drawn"))
}

/// How far below the end of the owner's text the original starts.
fn gap(h: &Window) -> f32 {
    rect(h, "composeQuote").min.y - rect(h, "composeBody").max.y
}

#[test]
fn the_original_starts_right_under_a_short_signature() {
    let h = reply("Short");
    assert!(h.state().session.compose.as_ref().unwrap().body.text().paragraphs().iter().any(|p| p.signature));
    assert!(rect(&h, "composeBody").height() < ROOM, "no room kept under the signature: {:?}", rect(&h, "composeBody"));
    assert!((0.0..=LINE).contains(&gap(&h)), "the original is within a line of the signature: {}", gap(&h));
}

#[test]
fn a_forward_of_an_html_original_starts_it_right_under_a_short_signature() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Short", "text": "Avery"})).unwrap();
    a.execute("signature.defaults", json!({"reply": "Short"})).unwrap();
    let id = html_original(&a);
    a.execute("mail.forward", json!({"id": id})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    let c = h.state().session.compose.as_ref().unwrap();
    assert_eq!(c.mode, mailcraft_engine::ComposeMode::Forward);
    assert!(c.body.text().paragraphs().iter().any(|p| p.signature));
    assert!(rect(&h, "composeBody").height() < ROOM, "{:?}", rect(&h, "composeBody"));
    assert!((0.0..=LINE).contains(&gap(&h)), "the original is within a line of the signature: {}", gap(&h));
}

/// An original in plain text is quoted in the text itself, right after the signature: there is no
/// original drawn under the text, and nothing laid out between the two but the blank line before it.
#[test]
fn a_reply_to_a_plain_text_original_quotes_it_right_after_the_signature() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Short", "text": "Avery"})).unwrap();
    a.execute("signature.defaults", json!({"reply": "Short"})).unwrap();
    let id = a
        .session
        .mailbox
        .messages
        .values()
        .find(|m| m.subject == "Q4 roadmap review moved to Thursday" && m.body.html.is_none())
        .map(|m| m.id)
        .unwrap();
    a.execute("mail.reply", json!({"id": id})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    assert!(h.state().hit.rect("composeQuote", 0).is_none(), "no original drawn under the text");
    let text = h.state().session.compose.as_ref().unwrap().body.plain();
    assert!(text.contains("Avery\n\n-----Original Message-----"), "the original follows the signature: {text:?}");
}

#[test]
fn switching_from_a_long_signature_to_a_short_one_leaves_no_blank_lines() {
    let mut h = reply("Long");
    let long = rect(&h, "composeBody").height();
    assert!((0.0..=LINE).contains(&gap(&h)), "{}", gap(&h));
    h.state_mut().run("compose.signature", json!({"name": "Short"})).unwrap();
    h.run_steps(3);
    let short = rect(&h, "composeBody");
    // Three lines (the empty ones before it and the signature) where there were nine: the body
    // follows its text, whatever a line's height is on this system.
    assert!(short.height() * 2.0 <= long, "the body is as tall as its text: {} after {long}", short.height());
    assert!((0.0..=LINE).contains(&gap(&h)), "the original follows the short signature: {}", gap(&h));
}

#[test]
fn a_reply_without_a_signature_keeps_its_room_to_write() {
    let h = reply("");
    assert!(!h.state().session.compose.as_ref().unwrap().body.text().paragraphs().iter().any(|p| p.signature));
    assert!(rect(&h, "composeBody").height() >= ROOM, "{:?}", rect(&h, "composeBody"));
}

#[test]
fn a_reply_under_an_html_signature_shown_as_designed_keeps_its_room() {
    let mut a = app();
    a.execute("signature.html", json!({"name": "Standard", "html": "<p><b style=\"color:#1F6FEB\">Avery Quinn</b><br>Lumen Studio</p>"})).unwrap();
    a.execute("signature.defaults", json!({"reply": "Standard"})).unwrap();
    let id = html_original(&a);
    a.execute("mail.reply", json!({"id": id})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    assert!(super::editor::html_signature(h.state()).is_some(), "the signature is shown as it goes out");
    assert!(h.state().hit.rect("composeSignature", 0).is_some());
    assert!(rect(&h, "composeBody").height() >= ROOM, "{:?}", rect(&h, "composeBody"));
}

/// An HTML signature edited as its simpler copy: the copy is laid out in the text, as any
/// signature in it is, and only the way back to the HTML is drawn under it; the original follows
/// that, with no room kept under the copy.
#[test]
fn a_reply_under_the_simpler_copy_of_an_html_signature_ends_with_it() {
    let mut a = app();
    a.execute("signature.html", json!({"name": "Standard", "html": "<p><b style=\"color:#1F6FEB\">Avery Quinn</b><br>Lumen Studio</p>"})).unwrap();
    a.execute("signature.defaults", json!({"reply": "Standard"})).unwrap();
    let id = html_original(&a);
    a.execute("mail.reply", json!({"id": id})).unwrap();
    a.execute("compose.signatureAsText", json!({"value": true})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    assert!(super::editor::html_signature(h.state()).is_none(), "the HTML is not drawn");
    assert!(super::editor::unedited_html_signature(h.state()).is_some(), "the copy is unedited");
    assert!(h.state().hit.rect("composeSignature", 0).is_none());
    let body = rect(&h, "composeBody");
    let back = rect(&h, "composeSignatureAsHtml");
    let quote = rect(&h, "composeQuote");
    assert!(body.height() < ROOM, "the copy ends the text, with no room under it: {body:?}");
    assert!(back.min.y >= body.max.y - 1.0 && quote.min.y >= back.max.y, "the link, then the original, under the text: {body:?} {back:?} {quote:?}");
    assert!(quote.min.y - back.max.y <= LINE, "the original follows the link: {back:?} {quote:?}");
}

#[test]
fn a_new_message_still_fills_the_window() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Short", "text": "Avery", "default": true})).unwrap();
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    let h = window(a);
    assert!(h.state().session.compose.as_ref().unwrap().body.text().paragraphs().iter().any(|p| p.signature));
    assert!(h.state().hit.rect("composeQuote", 0).is_none());
    assert!(rect(&h, "composeBody").height() > 2.0 * ROOM, "{:?}", rect(&h, "composeBody"));
}

#[test]
fn a_click_at_the_end_of_the_signature_puts_the_caret_after_it() {
    let mut h = reply("Short");
    let body = rect(&h, "composeBody");
    // Right of the signature's last line, at the bottom of the body.
    let at = egui::pos2(body.max.x - 8.0, body.max.y - 4.0);
    h.event(Event::PointerMoved(at));
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let end = h.state().session.compose.as_ref().unwrap().body.text().len();
    assert_eq!(sel(&h).head, end, "the caret is after the signature");
}
