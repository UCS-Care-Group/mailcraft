// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture on the clipboard pasted into the body of a message (#547), and the caret and the
//! keys going over it as over a letter. Driven in a window without a window.

use egui::{Event, Key, Modifiers};
use mailcraft_model::rich::OBJECT;
use serde_json::json;

use super::tests::{Window, app, click, key, key_with, plain, sel, type_in, window};
use crate::RichClipboard;

/// A clipboard holding a picture (and perhaps HTML).
#[derive(Clone, Default)]
struct PictureBoard(std::rc::Rc<std::cell::RefCell<(Option<Vec<u8>>, Option<String>)>>);

impl RichClipboard for PictureBoard {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn html(&self) -> Option<String> {
        self.0.borrow().1.clone()
    }
    fn image(&self) -> Option<Vec<u8>> {
        self.0.borrow().0.clone()
    }
}

fn png(extra: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    v.resize(v.len() + extra, 7);
    v
}

fn writing_with(board: &PictureBoard) -> Window {
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
fn a_browsers_copy_image_pastes_the_picture_not_its_address() {
    // Copy Image puts an `<img>` of the picture's web address beside the picture, and a page's
    // address as text (#1217): the picture is pasted, not a link and its caption.
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    let copied = r#"<meta charset="utf-8"><a href="https://pics.example/p"><img src="https://pics.example/cat.png" alt="A cat"></a>"#;
    *board.0.borrow_mut() = (Some(png(40)), Some(copied.into()));
    h.event(Event::Paste("https://pics.example/cat.png".into()));
    h.run_steps(2);
    assert_eq!((plain(&h), pictures(&h)), (OBJECT.to_string(), 1));
    // A picture with words around it is the HTML's, as before.
    *board.0.borrow_mut() = (Some(png(40)), Some(r#"<p>Look: <img src="https://pics.example/cat.png" alt="A cat"></p>"#.into()));
    paste(&mut h);
    assert_eq!(pictures(&h), 1, "{:?}", plain(&h));
    assert!(plain(&h).contains("Look:"), "{:?}", plain(&h));
    // A table, a rule or a second picture beside it is the HTML's too (a signature, a letterhead,
    // a row of photos): nothing of it is lost to the one picture on the clipboard.
    for html in [
        r#"<img src="https://pics.example/logo.png"><table><tr><td>Jane Doe, Sales</td></tr></table>"#,
        r#"<img src="https://pics.example/a.png"><hr><p></p>"#,
        r#"<img src="https://pics.example/a.png"><img src="https://pics.example/b.png">"#,
    ] {
        *board.0.borrow_mut() = (Some(png(40)), Some(html.into()));
        paste(&mut h);
        assert_eq!(pictures(&h), 1, "{html}: {:?}", plain(&h));
    }
    let text = h.state().session.compose.as_ref().unwrap().body.text();
    let cell = text
        .paragraphs()
        .iter()
        .flat_map(|p| &p.runs)
        .find_map(|run| run.table.as_deref())
        .and_then(|table| table.rows().first())
        .and_then(|row| row.first())
        .unwrap();
    let contents = cell.lines.iter().flatten().map(|run| run.text.as_str()).collect::<String>();
    assert_eq!(contents, "Jane Doe, Sales");
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

fn width(h: &Window) -> Option<u16> {
    h.state().session.compose.as_ref().unwrap().pictures.first().and_then(|p| p.width)
}

/// The button pressed at `from`, moved by `by` in a few steps, and let go.
fn drag(h: &mut Window, from: egui::Pos2, by: egui::Vec2) {
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for n in 1..=5 {
        h.event(Event::PointerMoved(from + by * (n as f32 / 5.0)));
        h.step();
    }
    h.event(button(from + by, false));
    h.run_steps(3);
}

#[test]
fn a_pasted_picture_is_made_smaller_by_its_corner_and_undo_puts_it_back() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    // Not selected, it has no corner.
    assert!(h.state().hit.rect("pictureCorner", u64::from(mailcraft_model::pictures::FIRST_ID)).is_none());
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    let corner = h.state().hit.rect("pictureCorner", u64::from(mailcraft_model::pictures::FIRST_ID)).expect("a selected picture has a corner");
    // Pressed and let go without moving: nothing changes.
    drag(&mut h, corner.center(), egui::Vec2::ZERO);
    assert_eq!(width(&h), None);
    // (Without pixels to read, the test picture is shown 120 wide.)
    drag(&mut h, corner.center(), egui::vec2(-50.0, -10.0));
    assert_eq!(width(&h), Some(70));
    assert_eq!(plain(&h), OBJECT.to_string(), "dragging the corner selects no text and moves nothing");
    // The picture is laid out at its new width: the corner is where the drag left it.
    let moved = h.state().hit.rect("pictureCorner", u64::from(mailcraft_model::pictures::FIRST_ID)).expect("still selected");
    assert!((moved.center().x - (corner.center().x - 50.0)).abs() < 1.0, "{moved:?} {corner:?}");
    // Ctrl+Z in the body puts its width back.
    assert!(super::editor::has_focus(&h.ctx), "the keys are still the body's");
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!((width(&h), plain(&h)), (None, OBJECT.to_string()));
}

#[test]
fn a_corner_drag_dropped_when_the_selection_leaves_lets_a_click_place_the_caret() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "ab");
    key(&mut h, Key::ArrowLeft);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    let corner = h.state().hit.rect("pictureCorner", u64::from(mailcraft_model::pictures::FIRST_ID)).expect("a selected picture has a corner");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let held = corner.center() + egui::vec2(-40.0, 0.0);
    h.event(Event::PointerMoved(corner.center()));
    h.step();
    h.event(button(corner.center(), true));
    h.step();
    h.event(Event::PointerMoved(held));
    h.step();
    assert_eq!(width(&h), None, "the width waits until the button is let go");
    // A command moves the selection off the picture while the corner is still held.
    h.state_mut().execute("compose.select", json!({"from": 3})).unwrap();
    h.event(button(held, false));
    h.run_steps(2);
    assert_eq!((width(&h), sel(&h).head), (None, 3), "letting go sets nothing once the picture is not selected");
    let body = h.state().hit.rect("composeBody", 0).expect("the body");
    click(&mut h, body.min + egui::vec2(2.0, 8.0));
    assert_eq!(sel(&h).head, 0, "a click places the caret");
}

#[test]
fn a_corner_drag_is_not_handed_to_another_picture_selected_while_it_is_held() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    paste(&mut h);
    let widths = |h: &Window| h.state().session.compose.as_ref().unwrap().pictures.iter().map(|p| p.width).collect::<Vec<_>>();
    assert_eq!(widths(&h), [None, None]);
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    h.run_steps(2);
    let first = mailcraft_model::pictures::FIRST_ID;
    let corner = h.state().hit.rect("pictureCorner", u64::from(first)).expect("the first picture is selected");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let held = corner.center() + egui::vec2(-40.0, 0.0);
    h.event(Event::PointerMoved(corner.center()));
    h.step();
    h.event(button(corner.center(), true));
    h.step();
    h.event(Event::PointerMoved(held));
    h.step();
    // A command selects the second picture while the first one's corner is still held.
    h.state_mut().execute("compose.select", json!({"from": 1, "to": 2})).unwrap();
    h.run_steps(2);
    assert!(h.state().hit.rect("pictureCorner", u64::from(first + 1)).is_some(), "the second picture is selected");
    h.event(Event::PointerMoved(held + egui::vec2(-10.0, 0.0)));
    h.step();
    h.event(button(held + egui::vec2(-10.0, 0.0), false));
    h.run_steps(2);
    assert_eq!(widths(&h), [None, None], "neither picture takes the width of a drag it didn't start");
}

fn size_box(h: &Window, name: &str) -> Option<egui::Rect> {
    h.state().hit.rect(name, u64::from(mailcraft_model::pictures::FIRST_ID))
}

/// A pasted picture, selected.
fn selected_picture(board: &PictureBoard) -> Window {
    let mut h = writing_with(board);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    assert!(size_box(&h, "pictureWidth").is_none(), "no size box without a selected picture");
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    h
}

/// `text` typed into the size box `name`, and entered.
fn enter_size(h: &mut Window, name: &str, text: &str) {
    let r = size_box(h, name).expect("a selected picture has a size box");
    click(h, r.center());
    key_with(h, Modifiers::COMMAND, Key::A);
    type_in(h, text);
    key(h, Key::Enter);
    h.run_steps(2);
}

#[test]
fn a_width_typed_into_the_size_box_is_set_and_a_height_sets_the_width_in_proportion() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    enter_size(&mut h, "pictureWidth", "60");
    assert_eq!(width(&h), Some(60));
    // (The test picture is 120 by 40 without pixels to read: 50 high is 150 wide.)
    enter_size(&mut h, "pictureHeight", "50");
    assert_eq!(width(&h), Some(150));
    assert_eq!(plain(&h), OBJECT.to_string(), "typing in the box goes nowhere near the text");
    // Each is one step of the message's undo.
    let undo = |h: &mut Window| {
        h.state_mut().execute("compose.undo", json!({})).unwrap();
        h.run_steps(2);
    };
    undo(&mut h);
    assert_eq!(width(&h), Some(60));
    undo(&mut h);
    assert_eq!(width(&h), None);
}

#[test]
fn dragging_the_width_sets_it_once_when_let_go_and_reset_size_gives_the_picture_its_own() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    let r = size_box(&h, "pictureWidth").unwrap();
    drag(&mut h, r.center(), egui::vec2(30.0, 0.0));
    let wide = width(&h).expect("let go, the width is set");
    assert!(wide > 120, "{wide}");
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(width(&h), None, "the whole drag was one step");
    h.state_mut().execute("compose.redo", json!({})).unwrap();
    h.run_steps(2);
    let reset = size_box(&h, "pictureReset").unwrap();
    click(&mut h, reset.center());
    assert_eq!(width(&h), None);
    // Nothing to reset now: the button does nothing.
    click(&mut h, reset.center());
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(width(&h), Some(wide), "a reset is one step, and the greyed button added none");
}

#[test]
fn a_plain_text_message_has_no_size_box() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    assert!(size_box(&h, "pictureWidth").is_some());
    h.state_mut().execute("compose.formatPlain", json!({})).unwrap();
    h.run_steps(3);
    assert!(size_box(&h, "pictureWidth").is_none());
}

#[test]
fn a_picture_of_the_signature_in_a_message_has_no_corner() {
    let mut a = app();
    let png = png(40);
    a.execute("signature.set", json!({"name": "Logo", "text": "Ada"})).unwrap();
    a.execute("signature.picture.add", json!({"name": "Logo", "data": crate::files::base64_lite::encode(&png)})).unwrap();
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.signature", json!({"name": "Logo"})).unwrap();
    let c = a.session.compose.as_ref().unwrap();
    let id = c.signature.as_ref().and_then(|s| s.pictures.first()).expect("the signature brought its picture").id;
    let at = c.body.plain().chars().position(|ch| ch == OBJECT).expect("the signature's picture is in the text");
    a.execute("compose.select", json!({"from": at, "to": at + 1})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    assert!(h.state().hit.rect("composeBody", 0).is_some());
    assert!(h.state().hit.rect("pictureCorner", u64::from(id)).is_none(), "it is sized in File › Signatures");
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

/// The corner `name` of the selected (first) picture, where it is drawn.
fn corner(h: &Window, name: &str) -> egui::Pos2 {
    h.state().hit.rect(name, u64::from(mailcraft_model::pictures::FIRST_ID)).expect("a selected picture has four corners").center()
}

#[test]
fn every_corner_sizes_the_picture_from_the_one_opposite() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    // (Without pixels to read, the test picture is shown 120 wide.) A left corner makes it wider
    // going left, a right one going right.
    let at = corner(&h, "pictureCornerTopLeft");
    drag(&mut h, at, egui::vec2(-30.0, -10.0));
    assert_eq!(width(&h), Some(150));
    let at = corner(&h, "pictureCornerTopRight");
    drag(&mut h, at, egui::vec2(20.0, 0.0));
    assert_eq!(width(&h), Some(170));
    let at = corner(&h, "pictureCornerBottomLeft");
    drag(&mut h, at, egui::vec2(20.0, 5.0));
    assert_eq!(width(&h), Some(150));
    assert_eq!(plain(&h), OBJECT.to_string(), "dragging a corner selects no text and moves nothing");
    let s = sel(&h);
    assert_eq!((s.start(), s.end()), (0, 1), "the picture is still what is selected");
    // One step of the message's undo for each drag.
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), Some(170));
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), Some(150));
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None);
}

#[test]
fn a_picture_not_selected_has_no_corners() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    let names = ["pictureCornerTopLeft", "pictureCornerTopRight", "pictureCornerBottomLeft", "pictureCorner"];
    for name in names {
        assert!(h.state().hit.rect(name, u64::from(mailcraft_model::pictures::FIRST_ID)).is_none(), "{name}");
    }
    // Selected, it has all four, at its corners.
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    let [tl, tr, bl, br] = names.map(|name| corner(&h, name));
    assert!(tl.x < tr.x && bl.x < br.x && tl.y < bl.y && tr.y < br.y, "{tl:?} {tr:?} {bl:?} {br:?}");
    assert!((tl.x - bl.x).abs() < 0.5 && (tr.x - br.x).abs() < 0.5 && (tl.y - tr.y).abs() < 0.5 && (bl.y - br.y).abs() < 0.5);
}

#[test]
fn the_outline_while_dragging_keeps_the_opposite_corner_where_it_is() {
    let r = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(130.0, 60.0));
    // Top left held, 150 wide: the bottom right stays.
    let shown = super::editor::sized_from(r, 0, 150.0);
    assert_eq!((shown.max, shown.width(), shown.height()), (r.max, 150.0, 50.0));
    // Top right: the bottom left stays.
    let shown = super::editor::sized_from(r, 1, 60.0);
    assert_eq!((shown.left_bottom(), shown.width(), shown.height()), (r.left_bottom(), 60.0, 20.0));
    // Bottom left: the top right stays.
    let shown = super::editor::sized_from(r, 2, 60.0);
    assert_eq!((shown.right_top(), shown.width()), (r.right_top(), 60.0));
    // Bottom right: the top left stays, as before.
    let shown = super::editor::sized_from(r, 3, 240.0);
    assert_eq!((shown.min, shown.width(), shown.height()), (r.min, 240.0, 80.0));
}

#[test]
fn escape_while_a_corner_is_held_puts_the_picture_back_and_does_nothing_else() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    let from = corner(&h, "pictureCornerTopRight");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for n in 1..=4 {
        h.event(Event::PointerMoved(from + egui::vec2(10.0 * n as f32, 0.0)));
        h.step();
    }
    key(&mut h, Key::Escape);
    // A second Escape, and one held until it repeats, with the button still down: the drag's
    // too, so the message stays open and the body keeps the keyboard.
    key(&mut h, Key::Escape);
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    for _ in 0..3 {
        h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE });
        h.step();
    }
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(super::editor::has_focus(&h.ctx), "the body keeps the keyboard while the button is down");
    // The button is still down and the pointer goes on moving: the drag stays given up.
    let to = from + egui::vec2(60.0, 0.0);
    h.event(Event::PointerMoved(to));
    h.step();
    h.event(button(to, false));
    h.run_steps(3);
    assert_eq!(width(&h), None, "nothing is set");
    assert!(h.state().session.compose.is_some(), "the message is still open");
    assert!(super::editor::has_focus(&h.ctx), "the keys are still the body's");
    assert_eq!(plain(&h), OBJECT.to_string());
    let s = sel(&h);
    assert_eq!((s.start(), s.end()), (0, 1), "the picture is still what is selected");
    // The next drag sizes it as ever.
    let at = corner(&h, "pictureCornerTopRight");
    drag(&mut h, at, egui::vec2(20.0, 0.0));
    assert_eq!(width(&h), Some(140));
    // And Escape once the button is up is the body's as before: it lets go of the keyboard.
    key(&mut h, Key::Escape);
    h.run_steps(2);
    assert!(!super::editor::has_focus(&h.ctx), "Escape after the drag lets the body go");
    assert_eq!(width(&h), Some(140));
}

/// A PNG `w` by `h` pixels that can be read.
fn real_png(w: u32, h: u32) -> Vec<u8> {
    use image::ImageEncoder as _;
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 160]).collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

#[test]
fn a_signature_s_picture_in_file_signatures_has_four_corners_and_escape_is_the_drag_s() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Standard", "text": "Avery"})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    a.execute("signature.open", json!({"name": "Standard"})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.execute("signature.picture.add", json!({"data": crate::files::base64_lite::encode(&real_png(200, 100)), "file": "logo.png"})).unwrap();
    a.execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    let mut h = window(a);
    h.run_steps(6);
    let picture = |h: &Window| h.state().session.signature_edit.as_ref().unwrap().signature.pictures().first().map(|p| (p.id, p.width)).unwrap();
    let (id, before) = picture(&h);
    assert_eq!(before, None);
    let at = |h: &Window, name: &str| h.state().hit.rect(name, u64::from(id)).expect("the signature's picture has four corners").center();
    // The top left corner, 20 to the left: 20 wider.
    let from = at(&h, "pictureCornerTopLeft");
    drag(&mut h, from, egui::vec2(-20.0, 0.0));
    assert_eq!(picture(&h).1, Some(220));
    // Escape while the bottom left corner is held: nothing set, and File is still open.
    let from = at(&h, "pictureCornerBottomLeft");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    h.event(Event::PointerMoved(from + egui::vec2(30.0, 0.0)));
    h.step();
    key(&mut h, Key::Escape);
    // Escape again, and held until it repeats, with the button still down: still the drag's.
    key(&mut h, Key::Escape);
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    for _ in 0..3 {
        h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE });
        h.step();
    }
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().ui.backstage, "File is still open while the button is down");
    h.event(button(from + egui::vec2(30.0, 0.0), false));
    h.step();
    assert_eq!(picture(&h).1, Some(220));
    assert!(h.state().ui.backstage, "File is still open");
    // With the button up, Escape closes File as ever, even in the very next frame.
    key(&mut h, Key::Escape);
    h.run_steps(2);
    assert!(!h.state().ui.backstage, "Escape after the drag closes File");
}

#[test]
fn a_corner_dragged_with_the_secondary_button_sizes_nothing() {
    let board = PictureBoard::default();
    let mut h = selected_picture(&board);
    let from = corner(&h, "pictureCornerTopRight");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for n in 1..=4 {
        h.event(Event::PointerMoved(from + egui::vec2(10.0 * n as f32, 0.0)));
        h.step();
    }
    h.event(button(from + egui::vec2(40.0, 0.0), false));
    h.run_steps(3);
    assert_eq!(width(&h), None);
}

#[test]
fn a_drag_ended_by_shift_escape_is_let_go_of() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    type_in(&mut h, "ab");
    key(&mut h, Key::ArrowLeft);
    *board.0.borrow_mut() = (Some(png(40)), None);
    paste(&mut h);
    key_with(&mut h, Modifiers::SHIFT, Key::ArrowLeft);
    h.run_steps(2);
    let from = corner(&h, "pictureCornerTopRight");
    let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for n in 1..=4 {
        h.event(Event::PointerMoved(from + egui::vec2(10.0 * n as f32, 0.0)));
        h.step();
    }
    // egui ends the drag on Escape with Shift too; the window's keys don't take that one.
    key_with(&mut h, Modifiers::SHIFT, Key::Escape);
    h.event(button(from + egui::vec2(40.0, 0.0), false));
    h.run_steps(3);
    assert_eq!(width(&h), None, "nothing is set");
    // Nothing is left of the drag: a click places the caret, and Escape is the body's as ever.
    let body = h.state().hit.rect("composeBody", 0).expect("the body");
    click(&mut h, body.min + egui::vec2(2.0, 8.0));
    assert_eq!(sel(&h).head, 0, "a click places the caret");
    key(&mut h, Key::Escape);
    h.run_steps(2);
    assert!(!super::editor::has_focus(&h.ctx), "Escape lets the body go");
}
