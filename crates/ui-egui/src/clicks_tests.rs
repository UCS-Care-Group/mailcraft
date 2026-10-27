// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The clicks in a row a press is, known as the button goes down, are the clicks egui counts
//! when it comes up.

use egui::{Event, Modifiers, PointerButton, Pos2, Sense, pos2, vec2};
use egui_kittest::Harness;

use crate::clicks::{self, Unit};

/// What each press selects, and how many clicks egui counted when it came up.
#[derive(Default)]
struct Seen {
    pressed: Vec<Unit>,
    clicked: Vec<u8>,
}

fn click(h: &mut Harness<'_, Seen>, pos: Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

fn harness() -> Harness<'static, Seen> {
    Harness::builder().with_accessibility_check(false).with_size(vec2(400.0, 300.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, seen: &mut Seen| {
            let unit = clicks::pressed(ui.ctx());
            let response = ui.allocate_response(ui.available_size(), Sense::click());
            seen.pressed.extend(unit);
            if response.clicked() {
                seen.clicked.push(if response.triple_clicked() {
                    3
                } else if response.double_clicked() {
                    2
                } else {
                    1
                });
            }
        },
        Seen::default(),
    )
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

/// What each press counted as: 1, 2 or 3 clicks in a row.
fn counted(h: &Harness<'_, Seen>) -> Vec<u8> {
    h.state()
        .pressed
        .iter()
        .map(|unit| match unit {
            Unit::Char => 1,
            Unit::Word => 2,
            Unit::Paragraph => 3,
        })
        .collect()
}

#[test]
fn a_press_is_counted_as_egui_counts_its_click() {
    let mut h = harness();
    // Four quick clicks in one place (the fourth is a triple click again); after a pause, one
    // there, and two quick ones a little too far from it to be its second: the last of them is a
    // triple click all the same, for the two before it were quick enough, wherever they were.
    let (here, there) = (pos2(100.0, 100.0), pos2(110.0, 100.0));
    for _ in 0..4 {
        click(&mut h, here);
    }
    h.run_steps(30);
    for pos in [here, there, there] {
        click(&mut h, pos);
    }
    assert_eq!(h.state().clicked, [1, 2, 3, 3, 1, 1, 3]);
    assert_eq!(counted(&h), h.state().clicked);
}

#[test]
fn a_drag_let_go_in_the_frame_of_a_click_is_no_click() {
    let mut h = harness();
    let (from, here) = (pos2(100.0, 100.0), pos2(200.0, 100.0));
    h.event(button(from, true));
    h.step();
    h.event(Event::PointerMoved(pos2(150.0, 100.0)));
    h.step();
    // In one frame (the harness gives each event it queues a frame of its own): the drag let go,
    // and a click; then a second click there.
    h.input_mut().events.extend([Event::PointerMoved(here), button(here, false), button(here, true), button(here, false)]);
    h.step();
    click(&mut h, here);
    assert_eq!(h.state().clicked, [1, 2]);
    assert_eq!(counted(&h), [1, 1, 2]);
}
