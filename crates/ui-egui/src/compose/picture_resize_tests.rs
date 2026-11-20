// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture in the message sized by its corners (#1740): the corners of a selected picture,
//! dragged, keep its proportions; the width is set when the button is let go, as one step of the
//! message's undo; Escape while the button is down puts it back. Driven in a window without a
//! window.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use serde_json::json;

use crate::compose::tests::{Window, app, key, key_with, sel, window};

/// A PNG `w` by `h` pixels.
fn png(w: u32, h: u32) -> Vec<u8> {
    use image::ImageEncoder as _;
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 160]).collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

/// A new HTML message, `a`, a picture 200 by 100 pixels, `b`, with the picture selected.
fn with_picture() -> Window {
    with_picture_of(200, 100)
}

/// [`with_picture`], the picture `w` by `h` pixels.
fn with_picture_of(w: u32, h: u32) -> Window {
    let mut a = app();
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"body": "ab"})).unwrap();
    a.execute("compose.select", json!({"from": 1})).unwrap();
    a.execute("insert.picture", json!({"data": crate::files::base64_lite::encode(&png(w, h)), "file": "p.png"})).unwrap();
    a.execute("compose.select", json!({"from": 1, "to": 2})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.run_steps(6);
    h
}

/// The selected picture, where it is drawn.
fn picture(h: &Window) -> Rect {
    super::selected_picture_rect(&h.ctx, h.state(), super::Which::Message).expect("a selected picture")
}

/// The width the picture (at 1) has in the text, over its own.
fn width(h: &Window) -> Option<u16> {
    h.state().session.compose.as_ref().unwrap().body.text().picture_at(1).and_then(|(_, w)| w)
}

fn button(h: &mut Window, at: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.step();
}

/// Press at `from` and move to `to` in steps, the button still down.
fn hold_and_move(h: &mut Window, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    button(h, from, true);
    for n in 1..=6 {
        h.event(Event::PointerMoved(from + (to - from) * (n as f32 / 6.0)));
        h.step();
    }
}

fn drag(h: &mut Window, from: Pos2, to: Pos2) {
    hold_and_move(h, from, to);
    button(h, to, false);
    h.run_steps(3);
}

/// `events` all in one frame (the harness's queue gives each a frame of its own).
fn one_frame(h: &mut Window, events: Vec<Event>) {
    h.input_mut().events.extend(events);
    h.step();
}

fn up(at: Pos2) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }
}

fn down(at: Pos2) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }
}

fn escape(pressed: bool) -> Event {
    Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

fn cursor_over(h: &mut Window, at: Pos2) -> egui::CursorIcon {
    h.event(Event::PointerMoved(at));
    h.run_steps(2);
    h.output().platform_output.cursor_icon
}

#[test]
fn a_selected_picture_has_corners_that_size_it() {
    let mut h = with_picture();
    let r = picture(&h);
    assert!((r.width() - 200.0).abs() < 0.5 && (r.height() - 100.0).abs() < 0.5, "{r:?}");
    assert_eq!(cursor_over(&mut h, r.right_bottom()), egui::CursorIcon::ResizeNwSe);
    assert_eq!(cursor_over(&mut h, r.right_top()), egui::CursorIcon::ResizeNeSw);
    // Not selected, it has none: the pointer there is the text's.
    h.state_mut().execute("compose.select", json!({"from": 0})).unwrap();
    h.run_steps(2);
    assert_ne!(cursor_over(&mut h, r.right_bottom()), egui::CursorIcon::ResizeNwSe);
}

#[test]
fn dragging_a_corner_keeps_the_proportions_and_sets_the_width_when_let_go() {
    let mut h = with_picture();
    let r = picture(&h);
    // Half way in along the diagonal: half as wide.
    hold_and_move(&mut h, r.right_bottom(), r.right_bottom() - vec2(100.0, 50.0));
    assert_eq!(width(&h), None, "nothing is set while the button is down");
    assert!(h.state().session.compose.as_ref().unwrap().body.can_undo(), "(inserting the picture was a step)");
    button(&mut h, r.right_bottom() - vec2(100.0, 50.0), false);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    let now = picture(&h);
    assert!((now.width() - 100.0).abs() < 0.5 && (now.height() - 50.0).abs() < 0.5, "{now:?}");
    // The picture is still what is selected, and nothing else of the text changed.
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2));
    assert_eq!(h.state().session.compose.as_ref().unwrap().body.plain(), format!("a{}b", mailcraft_model::rich::OBJECT));
    // From the top left corner, outwards, it grows.
    let r = picture(&h);
    drag(&mut h, r.left_top(), r.left_top() - vec2(60.0, 30.0));
    assert_eq!(width(&h), Some(160));
    // One undo step for each drag.
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), Some(100));
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None);
    key_with(&mut h, Modifiers::COMMAND, Key::Y);
    assert_eq!(width(&h), Some(100));
}

#[test]
fn escape_while_dragging_puts_it_back_and_closes_nothing() {
    let mut h = with_picture();
    let r = picture(&h);
    hold_and_move(&mut h, r.right_bottom(), r.right_bottom() + vec2(80.0, 40.0));
    key(&mut h, Key::Escape);
    h.event(Event::PointerMoved(r.right_bottom() + vec2(120.0, 60.0)));
    h.step();
    button(&mut h, r.right_bottom() + vec2(120.0, 60.0), false);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(h.state().session.compose.is_some(), "the message is still open");
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2), "and the picture still selected");
    assert!((picture(&h).width() - 200.0).abs() < 0.5);
    // The next drag sizes it as ever.
    drag(&mut h, r.right_bottom(), r.right_bottom() - vec2(50.0, 25.0));
    assert_eq!(width(&h), Some(150));
}

#[test]
fn a_picture_is_kept_between_the_narrowest_and_the_room_the_body_has() {
    let mut h = with_picture();
    let body = h.state().hit.rect("composeBody", 0).unwrap();
    let r = picture(&h);
    // Far past the right edge: as wide as the body has room for from the margin, no wider.
    drag(&mut h, r.right_bottom(), r.right_bottom() + vec2(5000.0, 2500.0));
    let w = width(&h).unwrap();
    assert!(f32::from(w) <= body.width() + 0.5 && f32::from(w) > body.width() - 2.0, "{w} in {body:?}");
    // Past the opposite corner: the narrowest.
    let r = picture(&h);
    drag(&mut h, r.right_bottom(), r.left_top() - vec2(40.0, 20.0));
    assert_eq!(width(&h), Some(super::MIN_PICTURE_WIDTH as u16));
}

#[test]
fn a_click_on_a_corner_without_a_drag_changes_nothing() {
    let mut h = with_picture();
    let r = picture(&h);
    drag(&mut h, r.right_bottom(), r.right_bottom());
    assert_eq!(width(&h), None);
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2));
}

#[test]
fn the_diagonal_decides_the_width() {
    let start = Rect::from_min_size(egui::pos2(10.0, 10.0), vec2(200.0, 100.0));
    for corner in 0..4 {
        let held = super::corners(start)[corner];
        assert!((super::dragged_width(start, corner, held) - 200.0).abs() < 0.01, "{corner}");
        let anchor = super::corners(start)[3 - corner];
        let half = anchor + (held - anchor) * 0.5;
        assert!((super::dragged_width(start, corner, half) - 100.0).abs() < 0.01, "{corner}");
        // Shown from the corner that stays.
        let shown = super::resized(start, corner, 100.0, 2.0);
        assert!((shown.width() - 100.0).abs() < 0.01 && (shown.height() - 50.0).abs() < 0.01, "{corner}: {shown:?}");
        assert!(super::corners(shown).contains(&anchor), "{corner}: {shown:?}");
    }
}

#[test]
fn the_last_move_before_letting_go_counts() {
    let mut h = with_picture();
    let r = picture(&h);
    hold_and_move(&mut h, r.right_bottom(), r.right_bottom() - vec2(30.0, 15.0));
    // The last move and the release in one frame.
    let to = r.right_bottom() - vec2(100.0, 50.0);
    one_frame(&mut h, vec![Event::PointerMoved(to), up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
}

#[test]
fn a_corner_held_off_its_centre_and_let_go_changes_nothing() {
    let mut h = with_picture();
    let r = picture(&h);
    let off = r.right_bottom() + vec2(3.0, 2.0);
    h.event(Event::PointerMoved(off));
    h.step();
    button(&mut h, off, true);
    h.run_steps(2);
    button(&mut h, off, false);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    // Held off its centre and dragged, it goes as far as the pointer does.
    drag(&mut h, off, off - vec2(100.0, 50.0));
    assert_eq!(width(&h), Some(100));
}

#[test]
fn escape_and_letting_go_in_one_frame_still_put_it_back() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() + vec2(80.0, 40.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    one_frame(&mut h, vec![escape(true), up(to)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(h.state().session.compose.is_some(), "the message is still open");
    assert!(h.ctx.memory(|m| m.top_modal_layer()).is_none(), "and nothing asks about closing it");
    // Taken, dragged, Escape and let go of, all in one frame: the same.
    one_frame(&mut h, vec![down(r.right_bottom()), Event::PointerMoved(to), escape(true), up(to)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(h.state().session.compose.is_some() && h.ctx.memory(|m| m.top_modal_layer()).is_none());
}

#[test]
fn one_quick_drag_where_two_corners_meet_is_one_step() {
    // 200 by 10: the two right corners' handles overlap.
    let mut h = with_picture_of(200, 10);
    let r = picture(&h);
    let at = r.right_center();
    h.event(Event::PointerMoved(at));
    h.step();
    let to = at - vec2(100.0, 50.0);
    one_frame(&mut h, vec![down(at), Event::PointerMoved(to), up(to)]);
    h.run_steps(3);
    assert!(width(&h).is_some_and(|w| w < 200));
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None, "one undo step takes it back");
}

#[test]
fn a_move_after_letting_go_in_the_same_frame_does_not_count() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    one_frame(&mut h, vec![up(half), Event::PointerMoved(r.right_bottom())]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
}

#[test]
fn a_picture_outside_the_bounds_is_not_pushed_further_by_a_drag_that_has_not_moved() {
    let rect = |w: f32, h: f32| (w, w / h);
    // Where the room is less than the narrowest, the room: no wider than it.
    assert_eq!(super::kept_width(24.0, rect(24.0, 12.0).0, rect(24.0, 12.0).1, 10.0), 10.0);
    // Shown 10 wide, narrower than the narrowest: left as it is.
    assert_eq!(super::kept_width(10.0, rect(10.0, 5.0).0, rect(10.0, 5.0).1, 500.0), 10.0);
    // Otherwise kept between the two.
    assert_eq!(super::kept_width(5.0, rect(200.0, 100.0).0, rect(200.0, 100.0).1, 500.0), super::MIN_PICTURE_WIDTH);
    assert_eq!(super::kept_width(900.0, rect(200.0, 100.0).0, rect(200.0, 100.0).1, 500.0), 500.0);
    // And its height in proportion within what the body shows: 200 by 10 is no narrower than 80
    // (4 high), 10 by 200 no wider than 100 (2000 high).
    assert_eq!(super::kept_width(40.0, rect(200.0, 10.0).0, rect(200.0, 10.0).1, 500.0), 80.0);
    assert_eq!(super::kept_width(400.0, rect(10.0, 200.0).0, rect(10.0, 200.0).1, 500.0), 100.0);
    // Even where that is narrower than the narrowest: 10 by 1500 is no wider than 13.3.
    assert!((super::kept_width(16.0, rect(10.0, 1500.0).0, rect(10.0, 1500.0).1, 500.0) - 2000.0 / 150.0).abs() < 0.01);
}

#[test]
fn a_thin_picture_keeps_its_proportions_at_its_narrowest() {
    let mut h = with_picture_of(200, 10);
    let r = picture(&h);
    drag(&mut h, r.right_bottom(), r.left_top());
    let w = width(&h).unwrap();
    assert_eq!(w, 80, "4 high in proportion");
    let now = picture(&h);
    assert!((now.width() / now.height() - 20.0).abs() < 0.1, "{now:?}");
}

#[test]
fn a_drag_pressed_moved_and_let_go_in_one_frame_counts() {
    let mut h = with_picture();
    let r = picture(&h);
    h.event(Event::PointerMoved(r.right_bottom()));
    h.step();
    let to = r.right_bottom() - vec2(100.0, 50.0);
    one_frame(&mut h, vec![down(r.right_bottom()), Event::PointerMoved(to), up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2));
    // A click on a corner in one frame, without a move, changes nothing.
    let r = picture(&h);
    one_frame(&mut h, vec![down(r.right_bottom()), up(r.right_bottom())]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2));
}

#[test]
fn escape_is_the_drag_s_whatever_another_open_message_shows() {
    // Two texts in one window: the first holds a corner, the second (drawn after it) only has a
    // picture selected.
    let ctx = egui::Context::default();
    let press = |pressed| egui::RawInput { events: vec![Event::PointerMoved(egui::pos2(5.0, 5.0)), down_or_up(pressed)], ..Default::default() };
    ctx.run_ui(press(true), |_| {}).textures_delta.clear();
    let (a, b) = (egui::Id::unique("a"), egui::Id::unique("b"));
    ctx.run_ui(egui::RawInput::default(), |ui| {
        let held = |body, held| super::Held {
            viewport: egui::ViewportId::ROOT,
            body,
            layer: ui.layer_id(),
            pass: ui.ctx().cumulative_pass_nr(),
            handles: [Rect::from_min_size(egui::pos2(900.0, 900.0), vec2(4.0, 4.0)); 4],
            held,
            escape: None,
        };
        super::publish(ui.ctx(), a, Some(held(a, true)));
        super::publish(ui.ctx(), b, Some(held(b, false)));
    })
    .textures_delta
    .clear();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(super::escape_resize(ui.ctx(), &[]), "the held corner's");
        let all = ui.ctx().data(|d| d.get_temp::<Vec<super::Held>>(super::held_id())).unwrap();
        assert!(all.iter().any(|h| h.body == a && h.escape == Some(0)) && all.iter().all(|h| h.body != b || h.escape.is_none()));
    })
    .textures_delta
    .clear();
}

fn down_or_up(pressed: bool) -> Event {
    Event::PointerButton { pos: egui::pos2(5.0, 5.0), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn a_corner_out_of_view_takes_no_press() {
    let mut h = with_picture();
    let before = picture(&h);
    // Enough text after the picture that the body scrolls, scrolled until the picture is just
    // above what is in view (over what stands above the body, in the same window).
    let end = h.state().session.compose.as_ref().unwrap().body.text().len();
    h.state_mut().execute("compose.select", json!({"from": end})).unwrap();
    h.state_mut().run("compose.type", json!({"text": "line\n".repeat(120)})).unwrap();
    h.state_mut().execute("compose.select", json!({"from": 1, "to": 2})).unwrap();
    h.run_steps(3);
    h.event(Event::PointerMoved(before.center() + vec2(0.0, 200.0)));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -(before.height() + 30.0)),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(8);
    let r = picture(&h);
    let corner = r.right_bottom();
    assert!(corner.y < before.min.y - 5.0 && corner.y > 0.0, "just out of view: {r:?}");
    assert_eq!(h.ctx.layer_id_at(corner), Some(egui::LayerId::background()), "in the same window: {r:?}");
    let to = corner - vec2(100.0, 50.0);
    one_frame(&mut h, vec![Event::PointerMoved(corner)]);
    one_frame(&mut h, vec![down(corner), Event::PointerMoved(to), up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), None, "the corner is scrolled out of view: {r:?}");
}

#[test]
fn escape_right_after_a_resize_is_the_next_drag_s() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    one_frame(&mut h, vec![up(half)]);
    assert_eq!(width(&h), Some(100));
    // In the very next frame: the new corner taken, dragged, Escape, let go.
    let to = half + vec2(40.0, 20.0);
    one_frame(&mut h, vec![down(half), Event::PointerMoved(to), escape(true), up(to)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    assert!(h.state().session.compose.is_some() && h.ctx.memory(|m| m.top_modal_layer()).is_none());
}

#[test]
fn escape_right_after_undo_is_the_next_drag_s() {
    let mut h = with_picture();
    let r = picture(&h);
    drag(&mut h, r.right_bottom(), r.right_bottom() - vec2(100.0, 50.0));
    assert_eq!(width(&h), Some(100));
    one_frame(&mut h, vec![Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
    assert_eq!(width(&h), None, "undone");
    // In the very next frame: the corner as it is again taken, dragged, Escape, let go.
    let to = r.right_bottom() - vec2(40.0, 20.0);
    one_frame(
        &mut h,
        vec![
            Event::Key { key: Key::Z, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::COMMAND },
            down(r.right_bottom()),
            Event::PointerMoved(to),
            escape(true),
            up(to),
        ],
    );
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(h.state().session.compose.is_some() && h.ctx.memory(|m| m.top_modal_layer()).is_none());
}

#[test]
fn a_corner_taken_again_in_the_frame_after_a_resize_sizes_it() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    one_frame(&mut h, vec![up(half)]);
    assert_eq!(width(&h), Some(100));
    // Pressed again at once where the corner is now, then dragged in later frames.
    one_frame(&mut h, vec![down(half)]);
    for n in 1..=4 {
        one_frame(&mut h, vec![Event::PointerMoved(half + vec2(10.0, 5.0) * n as f32)]);
    }
    one_frame(&mut h, vec![up(half + vec2(40.0, 20.0))]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(140));
}

#[test]
fn a_press_and_its_first_move_in_one_frame_take_the_corner() {
    let mut h = with_picture();
    let r = picture(&h);
    h.event(Event::PointerMoved(r.right_bottom()));
    h.step();
    let to = r.right_bottom() - vec2(100.0, 50.0);
    one_frame(&mut h, vec![down(r.right_bottom()), Event::PointerMoved(to)]);
    one_frame(&mut h, vec![Event::PointerMoved(to)]);
    one_frame(&mut h, vec![up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    assert_eq!((sel(&h).start(), sel(&h).end()), (1, 2), "nothing of the text selected");
}

#[test]
fn a_release_before_a_press_in_one_frame_is_the_earlier_click_s() {
    let mut h = with_picture();
    let r = picture(&h);
    one_frame(&mut h, vec![Event::PointerMoved(r.center()), down(r.center())]);
    // Let go of the click on the picture, then a press on its corner, all in one frame.
    one_frame(&mut h, vec![up(r.center()), Event::PointerMoved(r.right_bottom()), down(r.right_bottom())]);
    assert_eq!(width(&h), None, "nothing is set while the button is down");
    let to = r.right_bottom() - vec2(100.0, 50.0);
    one_frame(&mut h, vec![Event::PointerMoved(to)]);
    one_frame(&mut h, vec![up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
}

#[test]
fn a_text_changed_while_a_corner_is_held_is_not_sized() {
    let mut h = with_picture();
    // The same picture twice, side by side (a draft can hold one twice).
    {
        let c = h.state_mut().session.compose.as_mut().unwrap();
        let id = c.pictures[0].id;
        let twice = mailcraft_model::rich::RichText::from_paragraphs(vec![mailcraft_model::rich::Paragraph::new(
            vec![mailcraft_model::rich::Run::picture(id, Default::default())],
            Default::default(),
        )]);
        c.body.select(2, 2).unwrap();
        c.body.paste(&twice).unwrap();
        c.body.select(1, 2).unwrap();
    }
    h.run_steps(3);
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    // The one being sized deleted while the button is down (an agent could do it): the other
    // takes its place.
    h.state_mut().execute("compose.delete", json!({"from": 1, "to": 2})).unwrap();
    h.state_mut().execute("compose.select", json!({"from": 1, "to": 2})).unwrap();
    h.step();
    one_frame(&mut h, vec![up(to)]);
    h.run_steps(3);
    let c = h.state().session.compose.as_ref().unwrap();
    assert_eq!(c.body.text().pictures().len(), 1);
    assert_eq!(width(&h), None, "the other one is left as it was");
}

#[test]
fn escape_after_a_click_and_a_corner_taken_in_one_frame_is_the_drag_s() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(60.0, 30.0);
    one_frame(
        &mut h,
        vec![
            Event::PointerMoved(r.center()),
            down(r.center()),
            up(r.center()),
            Event::PointerMoved(r.right_bottom()),
            down(r.right_bottom()),
            Event::PointerMoved(to),
            escape(true),
            up(to),
        ],
    );
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(h.state().session.compose.is_some() && h.ctx.memory(|m| m.top_modal_layer()).is_none());
}

#[test]
fn a_key_before_the_release_in_one_frame_leaves_the_width() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    one_frame(&mut h, vec![Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }, up(to)]);
    h.run_steps(3);
    let c = h.state().session.compose.as_ref().unwrap();
    assert!(c.body.text().pictures().is_empty(), "Delete took the picture");
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None, "and Undo brings it back as it was");
}

#[test]
fn a_picture_pasted_before_the_release_in_one_frame_leaves_the_width() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    let pasted = std::sync::Arc::new(egui::ColorImage::new([4, 4], vec![egui::Color32::RED; 16]));
    one_frame(&mut h, vec![Event::PasteImage(pasted), up(to)]);
    h.run_steps(3);
    let c = h.state().session.compose.as_ref().unwrap();
    assert_eq!(c.pictures.len(), 2, "the paste put the other picture in");
    // Undo takes the paste back, and the picture is as it was before the drag.
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None);
}

#[test]
fn a_middle_click_paste_before_the_release_in_one_frame_leaves_the_width() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    one_frame(&mut h, vec![Event::MiddleClickPaste { pos: r.left_center() - vec2(40.0, 0.0), text: "x".into() }, up(to)]);
    h.run_steps(3);
    assert!(h.state().session.compose.as_ref().unwrap().body.plain().contains('x'), "pasted");
    key_with(&mut h, Modifiers::COMMAND, Key::Z);
    assert_eq!(width(&h), None);
}

#[test]
fn a_drag_left_while_the_message_was_away_is_forgotten() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    // Calendar while the button is down; let go of there, then back to Mail.
    h.state_mut().run("view.calendar", json!({})).unwrap();
    h.run_steps(2);
    one_frame(&mut h, vec![up(to)]);
    h.run_steps(2);
    h.state_mut().run("view.mail", json!({})).unwrap();
    // The click that brought Mail back, let go of in the frame the message is drawn again.
    let far = egui::pos2(20.0, 400.0);
    one_frame(&mut h, vec![Event::PointerMoved(far), down(far)]);
    one_frame(&mut h, vec![up(far)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
}

#[test]
fn a_signature_s_picture_is_sized_by_its_corners_and_escape_after_is_the_drag_s() {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Standard", "text": "Avery"})).unwrap();
    a.execute("signature.open", json!({"name": "Standard"})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.execute("signature.picture.add", json!({"data": crate::files::base64_lite::encode(&png(200, 100)), "file": "logo.png"})).unwrap();
    a.execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    let mut h = window(a);
    h.run_steps(6);
    let shown = |h: &Window| super::selected_picture_rect(&h.ctx, h.state(), super::Which::Signature).expect("the signature's picture");
    let set = |h: &Window| {
        let e = h.state().session.signature_edit.as_ref().unwrap();
        e.signature.pictures().first().and_then(|p| p.width)
    };
    let r = shown(&h);
    let half = r.right_bottom() - vec2(r.width() / 2.0, r.height() / 2.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    one_frame(&mut h, vec![up(half)]);
    let w = set(&h).expect("sized");
    assert!((f32::from(w) - r.width() / 2.0).abs() <= 1.0, "{w} of {r:?}");
    // In the very next frame: its new corner taken, dragged, Escape, let go.
    let to = half + vec2(30.0, 15.0);
    one_frame(&mut h, vec![down(half), Event::PointerMoved(to), escape(true), up(to)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(set(&h), Some(w));
    assert!(h.state().ui.backstage, "File is still open");
}

#[test]
fn a_width_set_in_whole_pixels_keeps_the_height_in_bounds() {
    // 10 by 1850: 10.81 wide is 2000 high; 11 would be 2035.
    assert_eq!(super::whole_width(10.81, 10.0 / 1850.0), 10);
    // 200 by 1: 800 wide is 4 high; 799.6 rounds up, not down.
    assert_eq!(super::whole_width(799.6, 200.0), 800);
    assert_eq!(super::whole_width(150.4, 2.0), 150);
}

#[test]
fn a_picture_shown_out_of_proportion_is_previewed_as_it_will_be_shown() {
    // 200 by 1 is shown 4 high, as the body shows nothing less; made 400 wide it is still shown
    // 4 high, and the outline while dragging says so (not 8, as the 200 by 4 it was shown at would).
    let start = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(200.0, 4.0));
    let shown = super::resized(start, 3, 400.0, 200.0);
    assert_eq!((shown.width(), shown.height()), (400.0, 4.0));
}

#[test]
fn escape_after_the_release_in_one_frame_leaves_the_width_set() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    // Let go of, then Escape, in one frame: the width is set, as one step of undo.
    one_frame(&mut h, vec![up(half), escape(true)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    // (The Escape was the window's: the body gave up the keyboard, as on any Escape.)
    h.state_mut().run("compose.undo", json!({})).unwrap();
    assert_eq!(width(&h), None, "one undo step takes it back");
}

#[test]
fn picture_width_is_not_offered_in_search_commands() {
    // It takes a width, which only the corners (or a script) give: run from the list with no
    // params it could only fail.
    let h = with_picture();
    assert!(!crate::dialogs::all_commands(h.state()).iter().any(|c| c.0 == "format.pictureWidth"));
    // What the corners do still runs it, as a script can.
    assert!(h.state().session.registry.get("format.pictureWidth").is_some());
}

/// What an input method on Wayland (IBus) sends nearly every frame: nothing typed.
fn idle_ime() -> Event {
    Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None })
}

#[test]
fn an_input_method_s_empty_preedits_in_every_frame_do_not_stop_a_resize() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    one_frame(&mut h, vec![idle_ime(), Event::PointerMoved(r.right_bottom())]);
    one_frame(&mut h, vec![idle_ime(), down(r.right_bottom())]);
    for n in 1..=5 {
        one_frame(&mut h, vec![idle_ime(), Event::PointerMoved(r.right_bottom() + (to - r.right_bottom()) * (n as f32 / 5.0)), idle_ime()]);
    }
    // In the release frame too, before the release.
    one_frame(&mut h, vec![idle_ime(), up(to), idle_ime()]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100));
    // And again, the preedit and the release in the other order, with something being composed
    // meanwhile (shown, not typed).
    let r = picture(&h);
    let to = r.right_bottom() - vec2(50.0, 25.0);
    one_frame(&mut h, vec![Event::PointerMoved(r.right_bottom()), down(r.right_bottom()), idle_ime()]);
    let composing = Event::Ime(egui::ImeEvent::Preedit { text: "ka".into(), active_range_chars: None });
    one_frame(&mut h, vec![composing, Event::PointerMoved(to), idle_ime()]);
    one_frame(&mut h, vec![up(to), idle_ime()]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(50));
}

#[test]
fn an_input_method_s_commit_during_a_resize_gives_it_up() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    one_frame(&mut h, vec![Event::Ime(egui::ImeEvent::Commit("x".into())), up(to)]);
    h.run_steps(3);
    let c = h.state().session.compose.as_ref().unwrap();
    assert!(c.body.text().paragraphs().iter().flat_map(|p| &p.runs).all(|r| r.width.is_none()), "{:?}", c.body.text());
    assert!(c.body.plain().contains('x'), "typed: {:?}", c.body.plain());
}

#[test]
fn a_key_during_a_resize_gives_it_up_whatever_frame_the_release_comes_in() {
    let mut h = with_picture();
    let r = picture(&h);
    let to = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), to);
    // Right Arrow while the button is down (it takes the selection off the picture), let go of
    // frames later.
    one_frame(&mut h, vec![Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    one_frame(&mut h, vec![Event::Key { key: Key::ArrowRight, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
    h.run_steps(2);
    one_frame(&mut h, vec![up(to)]);
    h.run_steps(3);
    assert_eq!(width(&h), None);
    assert!(!h.state().session.compose.as_ref().unwrap().body.can_redo());
}

#[test]
fn escape_after_the_release_in_one_frame_lets_the_body_go_as_ever() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    assert!(super::has_focus(&h.ctx), "the body has the keyboard while the corner is held");
    one_frame(&mut h, vec![up(half), escape(true)]);
    one_frame(&mut h, vec![escape(false)]);
    h.run_steps(4);
    assert_eq!(width(&h), Some(100));
    assert!(!super::has_focus(&h.ctx), "Escape let the body go, as it does without a resize");
    // Without the Escape, the body keeps the keyboard after a resize.
    let mut h = with_picture();
    let r = picture(&h);
    drag(&mut h, r.right_bottom(), r.right_bottom() - vec2(100.0, 50.0));
    assert_eq!(width(&h), Some(100));
    assert!(super::has_focus(&h.ctx));
}

#[test]
fn a_drag_never_shows_a_width_the_command_would_not_set() {
    // A picture 2500 wide (an animated one isn't made smaller as it goes in), shown 2500 wide in
    // a body that has the room: dragged to 2400, it shows (and is set at) the widest sent, 1920.
    assert_eq!(super::kept_width(2400.0, 2500.0, 2.0, 1920.0), 1920.0);
    // In a signature, its 600.
    assert_eq!(super::kept_width(700.0, 800.0, 2.0, 600.0), 600.0);
}

#[test]
fn a_picture_wider_than_is_sent_clicked_on_a_corner_is_left_as_it_is() {
    let mut h = with_picture();
    h.set_size(egui::vec2(4200.0, 1600.0));
    {
        let c = h.state_mut().session.compose.as_mut().unwrap();
        let big =
            mailcraft_model::pictures::take(&c.pictures, mailcraft_model::pictures::FIRST_ID + 9, "big.png", png(2500, 1250), "", None).unwrap();
        let id = big.id;
        c.pictures.push(big);
        let run = mailcraft_model::rich::RichText::from_paragraphs(vec![mailcraft_model::rich::Paragraph::new(
            vec![mailcraft_model::rich::Run::picture(id, Default::default())],
            Default::default(),
        )]);
        c.body.select(1, 2).unwrap();
        c.body.paste(&run).unwrap();
        c.body.select(1, 2).unwrap();
    }
    h.run_steps(6);
    let r = picture(&h);
    assert!(r.width() > 1920.0, "shown wider than is sent: {r:?}");
    drag(&mut h, r.right_bottom(), r.right_bottom());
    assert_eq!(width(&h), None, "a click changes nothing");
    // Dragged in a little, it is the widest sent.
    drag(&mut h, r.right_bottom(), r.right_bottom() - vec2(100.0, 50.0));
    assert_eq!(width(&h), Some(1920));
}

#[test]
fn escape_is_the_drag_s_it_came_in_not_the_frame_s() {
    let mut h = with_picture();
    let r = picture(&h);
    let half = r.right_bottom() - vec2(100.0, 50.0);
    hold_and_move(&mut h, r.right_bottom(), half);
    // In one frame: the first drag let go of, the top left corner taken for another, Escape.
    one_frame(&mut h, vec![up(half), Event::PointerMoved(r.left_top()), down(r.left_top()), escape(true)]);
    one_frame(&mut h, vec![escape(false), Event::PointerMoved(r.left_top() - vec2(40.0, 20.0))]);
    one_frame(&mut h, vec![up(r.left_top() - vec2(40.0, 20.0))]);
    h.run_steps(3);
    assert_eq!(width(&h), Some(100), "the first is set, the second put back");
    h.state_mut().run("compose.undo", json!({})).unwrap();
    assert_eq!(width(&h), None, "one step");
}

/// Whatever width a drag shows is the width that is set: the command's (and Picture Size's) caps
/// never cut it, for any proportions, starting width, room and pointer.
#[test]
fn what_a_drag_shows_is_what_is_set() {
    let id = mailcraft_model::pictures::FIRST_ID;
    let mut text = mailcraft_model::rich::RichText::from_paragraphs(vec![mailcraft_model::rich::Paragraph::new(
        vec![mailcraft_model::rich::Run::picture(id, Default::default())],
        Default::default(),
    )]);
    let mut sig = mailcraft_model::signature::Signature::plain_text("Work", "");
    let pid = sig.add_picture("p.png", png(2, 2), None, Some(0)).unwrap();
    let aspects = [0.002, 0.01, 0.1, 0.5, 1.0, 2.0, 10.0, 100.0, 500.0, 3000.0];
    let starts = [1.0, 10.0, 24.0, 200.0, 599.6, 600.0, 1919.6, 1920.0, 2500.0, 5000.0];
    let dragged = [-100.0, 0.0, 5.0, 16.0, 500.0, 600.4, 1919.4, 1920.6, 2500.0, 9000.0];
    let rooms = [24.0, 300.5, 1000.0, 2500.0, 5000.0];
    for (cap, signature) in [(f32::from(mailcraft_model::pictures::MAX_SHOWN_WIDTH), false), (f32::from(mailcraft_model::signature::MAX_WIDTH), true)]
    {
        for aspect in aspects {
            for w in starts {
                for room in rooms {
                    let widest = cap.min(room);
                    for d in dragged {
                        let shown = super::kept_width(d, w, aspect, widest);
                        let whole = super::whole_width(shown, aspect);
                        let set = if signature {
                            sig.set_picture(pid, Some(Some(whole)), None).unwrap();
                            sig.picture(pid).unwrap().width
                        } else {
                            text.size_picture(0, Some(whole)).unwrap();
                            text.picture_at(0).unwrap().1
                        };
                        let case = format!("cap {cap} aspect {aspect} from {w} in {room} to {d}: shown {shown}, set {set:?}");
                        assert_eq!(set, Some(whole), "{case}");
                        assert!((f32::from(whole) - shown).abs() < 1.0, "{case}");
                        assert!(shown <= widest, "{case}");
                    }
                }
            }
        }
    }
}
