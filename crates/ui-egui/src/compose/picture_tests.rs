// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture on the clipboard pasted into the body of a message (#547), the caret and the keys
//! going over it as over a letter, and a picture moved by cut and paste in the body and in a
//! signature (#580). Driven in a window without a window.

use egui::{Event, Key, Modifiers};
use mailcraft_model::rich::OBJECT;
use serde_json::json;

use super::tests::{Window, app, key, key_with, plain, sel, signature_page, type_in, window};
use crate::RichClipboard;

/// A clipboard holding a picture (and perhaps HTML); what is put on it is its HTML then, and the
/// text beside it is kept apart (what the system would hand egui for Ctrl+V).
#[derive(Clone, Default)]
pub(super) struct PictureBoard(
    pub(super) std::rc::Rc<std::cell::RefCell<(Option<Vec<u8>>, Option<String>)>>,
    std::rc::Rc<std::cell::RefCell<String>>,
);

impl RichClipboard for PictureBoard {
    fn put(&self, html: &str, text: &str) -> Result<(), String> {
        self.0.borrow_mut().1 = Some(html.to_string());
        *self.1.borrow_mut() = text.to_string();
        Ok(())
    }
    fn html(&self) -> Option<String> {
        self.0.borrow().1.clone()
    }
    fn image(&self) -> Option<Vec<u8>> {
        self.0.borrow().0.clone()
    }
}

pub(super) fn png(extra: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    v.resize(v.len() + extra, 7);
    v
}

pub(super) fn writing_with(board: &PictureBoard) -> Window {
    let mut a = app();
    a.services.clipboard = Some(Box::new(board.clone()));
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"body": ""})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.run_steps(2);
    h
}

fn pictures(h: &Window) -> usize {
    h.state().session.compose.as_ref().unwrap().pictures.len()
}

fn paste(h: &mut Window) {
    h.event(Event::Paste(String::new()));
    h.run_steps(2);
}

#[test]
fn ctrl_v_with_a_picture_and_nothing_else_inserts_it_at_the_caret() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "ab");
    key(&mut h, Key::ArrowLeft);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    assert_eq!(plain(&h), format!("a{OBJECT}b"));
    assert_eq!(pictures(&h), 1);
    assert_eq!(sel(&h).head, 2, "the caret is after it");
    // Sent, it is part of the message.
    let id = h.state_mut().execute("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    let m = h.state().session.mailbox.message(id).unwrap();
    assert!(m.attachments.iter().any(|a| a.inline && a.mime == "image/png" && a.content_id.starts_with("pic-")));
}

#[test]
fn html_or_text_on_the_clipboard_is_pasted_instead_of_the_picture() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(40)), Some("<p>from a page</p>".into()));
    paste(&mut h);
    assert_eq!((plain(&h).as_str(), pictures(&h)), ("from a page", 0));
    *board.0.borrow_mut() = (Some(png(40)), None);
    h.event(Event::Paste("some text".into()));
    h.run_steps(2);
    assert!(plain(&h).ends_with("some text") && pictures(&h) == 0, "{:?}", plain(&h));
}

#[test]
fn a_picture_too_big_or_not_one_is_refused_and_the_message_is_unchanged() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "keep");
    let before = h.state().session.compose.clone();
    for bytes in [png(6 << 20), b"not a picture at all".to_vec()] {
        *board.0.borrow_mut() = (Some(bytes), None);
        paste(&mut h);
    }
    assert_eq!(h.state().session.compose, before);
    assert!(h.state().status_msg.as_ref().is_some_and(|(m, _)| m.contains("at most 5 MB") || m.contains("PNG")), "{:?}", h.state().status_msg);
}

#[test]
fn a_large_screenshot_is_pasted_scaled_down_to_fit() {
    use image::ImageEncoder as _;
    let (w, h) = (3000u32, 2000u32);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w / 40 % 256) as u8, (i / w / 30 % 256) as u8, 90]).collect();
    let mut big = Vec::new();
    image::codecs::png::PngEncoder::new(&mut big).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(big), None);
    paste(&mut h);
    let c = h.state().session.compose.as_ref().unwrap();
    assert_eq!(c.pictures.len(), 1);
    let p = &c.pictures[0];
    let dims = image::ImageReader::new(std::io::Cursor::new(p.data.as_slice())).with_guessed_format().unwrap().into_dimensions().unwrap();
    assert_eq!(dims, (1920, 1280));
    assert!(p.data.len() <= mailcraft_model::pictures::MAX_PICTURE_BYTES);
}

#[test]
fn the_caret_and_the_keys_go_over_a_picture_as_over_one_letter() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "ab");
    key(&mut h, Key::ArrowLeft);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    assert_eq!(sel(&h).head, 2);
    // One step left is before the picture, one more before the "a".
    key(&mut h, Key::ArrowLeft);
    assert_eq!(sel(&h).head, 1);
    key(&mut h, Key::ArrowRight);
    assert_eq!(sel(&h).head, 2);
    // Shift and an arrow select it.
    key(&mut h, Key::ArrowLeft);
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowRight);
    let s = sel(&h);
    assert_eq!((s.start(), s.end()), (1, 2));
    // Typing over the selection replaces it.
    type_in(&mut h, "X");
    assert_eq!(plain(&h), "aXb");
    // Backspace and Delete take a picture like a letter.
    h.event(Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL });
    h.run_steps(2);
    paste(&mut h);
    assert_eq!(plain(&h).chars().filter(|c| *c == OBJECT).count(), 1);
    key(&mut h, Key::Backspace);
    assert!(!plain(&h).contains(OBJECT), "{:?}", plain(&h));
    paste(&mut h);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::Delete);
    assert!(!plain(&h).contains(OBJECT), "{:?}", plain(&h));
}

#[test]
fn a_picture_is_drawn_in_the_body() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    h.run_steps(3);
    // (The test PNG has no pixels to decode: it is drawn as a frame, and laying it out did not fail.)
    assert_eq!(pictures(&h), 1);
}

/// Ctrl+X where the editor has the keyboard, what it put on the clipboard as text (what the
/// system hands egui), and Ctrl+V of it, the editor reading the clipboard's HTML beside it.
fn cut_and_paste(h: &mut Window, board: &PictureBoard, to: usize) {
    h.event(Event::Cut);
    h.run_steps(2);
    let text = board.1.borrow().clone();
    assert!(!text.is_empty(), "a picture alone is cut as its description, so that the system pastes it");
    h.state_mut().execute("compose.select", json!({"from": to})).unwrap();
    h.run_steps(1);
    h.event(Event::Paste(text));
    h.run_steps(2);
}

#[test]
fn ctrl_x_and_ctrl_v_move_a_picture_in_the_body_and_in_a_signature() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "ab");
    let id = h.state_mut().execute("insert.picture", json!({"data": crate::files::base64_lite::encode(&png(40)), "alt": "Chart", "at": 0})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    h.run_steps(1);
    cut_and_paste(&mut h, &board, 2);
    assert_eq!(plain(&h), format!("ab{OBJECT}"));
    assert_eq!(h.state().session.compose.as_ref().unwrap().body.text().pictures(), [u32::try_from(id).unwrap()]);
    assert_eq!(pictures(&h), 1);
    // In File › Signatures, the open signature's own picture moves the same way.
    let mut h = signature_page();
    h.state_mut().services.clipboard = Some(Box::new(board.clone()));
    h.state_mut().execute("compose.select", json!({"from": 0})).unwrap();
    let id =
        h.state_mut().execute("signature.picture.add", json!({"data": crate::files::base64_lite::encode(&png(40)), "alt": "Logo"})).unwrap()["id"]
            .as_u64()
            .unwrap();
    // A click in its text gives it the keyboard.
    let at = h.state().hit.rect("signatureBody", 0).unwrap().max - egui::vec2(4.0, 4.0);
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(1);
    assert!(super::editor::has_focus(&h.ctx));
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    h.run_steps(1);
    let end = h.state().session.signature_edit.as_ref().unwrap().body.text().len() - 1;
    cut_and_paste(&mut h, &board, end);
    let e = h.state().session.signature_edit.as_ref().unwrap();
    assert_eq!(e.body.plain(), format!("Avery Quinn\nLumen Studio{OBJECT}"));
    assert_eq!(e.body.text().pictures(), [u32::try_from(id).unwrap()]);
}

#[test]
fn ctrl_x_of_the_break_that_would_join_a_picture_onto_the_signature_is_refused_and_says_why() {
    let board = PictureBoard::default();
    let mut a = app();
    a.services.clipboard = Some(Box::new(board.clone()));
    a.execute("mail.new", json!({"to": "maya@lumen.example", "body": "Hi"})).unwrap();
    a.execute("compose.formatHtml", json!({})).unwrap();
    // A picture on a line of its own after the signature.
    let end = a.session.compose.as_ref().unwrap().body.text().len();
    a.execute("compose.select", json!({"from": end})).unwrap();
    a.execute("compose.type", json!({"text": "\n"})).unwrap();
    a.execute("insert.picture", json!({"data": crate::files::base64_lite::encode(&png(40)), "alt": "Chart"})).unwrap();
    let line = a.session.compose.as_ref().unwrap().body.text().len() - 1;
    // The break before it selected, the editor with the keyboard.
    a.execute("compose.select", json!({"from": line - 1, "to": line})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.run_steps(2);
    let before = plain(&h);
    h.event(Event::Cut);
    h.run_steps(2);
    assert_eq!(plain(&h), before);
    assert!(board.0.borrow().1.is_none() && board.1.borrow().is_empty(), "nothing was put on the clipboard");
    let said = h.state().status_msg.clone();
    assert!(said.as_ref().is_some_and(|(said, _)| said.contains("would put the picture into the signature")), "{said:?}");
}
