// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A double click held and dragged in a one-line field (the Subject and To lines of a message
//! being written, the search box) goes on by words, as egui's own text fields do.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;

use crate::people_view::testing::{self, Window};

const TEXT: &str = "Plan for Friday";

/// The sample mailbox with a message being written, its To and Subject lines both [`TEXT`].
fn writing() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("mail.new", json!({"to": TEXT, "subject": TEXT})).unwrap();
    h.run_steps(4);
    h
}

/// The one-line fields that show `value`, top to bottom.
fn fields(h: &Window, value: &str) -> Vec<egui::Rect> {
    let mut found: Vec<egui::Rect> = h.query_all_by_role(Role::TextInput).filter(|f| f.value().as_deref() == Some(value)).map(|f| f.rect()).collect();
    found.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    found
}

/// Where character `n` of the field showing `value` whose top is `top` is on the screen, `part`
/// of the way across it (from the text runs the field tells the accessibility tree about).
fn on_char(h: &Window, value: &str, top: f32, n: usize, part: f32) -> Pos2 {
    let field = h
        .query_all_by_role(Role::TextInput)
        .find(|f| f.value().as_deref() == Some(value) && (f.rect().min.y - top).abs() < 1.0)
        .unwrap_or_else(|| panic!("no field shows {value:?}"));
    let run = field.children().find(|c| c.accesskit_node().role() == Role::TextRun).expect("the field's text");
    let rect = run.rect();
    let data = run.accesskit_node().data().clone();
    let (at, widths) = (data.character_positions().unwrap(), data.character_widths().unwrap());
    pos2(rect.min.x + at[n] + widths[n] * part, rect.center().y)
}

/// So many clicks in a row at `from`, the last of them held and dragged to `to`, one pointer
/// event a frame (1/60 s apart).
fn clicks_and_drag(h: &mut Window, from: Pos2, count: usize, to: Pos2) {
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.run_steps(45);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    for _ in 1..count {
        for pressed in [true, false] {
            h.input_mut().events.push(button(from, pressed));
            h.step();
        }
    }
    h.input_mut().events.push(button(from, true));
    h.step();
    for n in 1..=5 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (n as f32 / 5.0)));
        h.step();
    }
    h.input_mut().events.push(button(to, false));
    h.run_steps(2);
}

/// What is selected in the field that has the keyboard, which shows `value`.
fn selected(h: &Window, value: &str) -> Option<String> {
    let id = h.ctx.memory(|m| m.focused())?;
    let range = egui::TextEdit::load_state(&h.ctx, id)?.cursor.char_range()?;
    let [a, b] = range.sorted_cursors();
    Some(value.chars().skip(a.index.0).take(b.index.0 - a.index.0).collect())
}

/// In the field whose top is `top`: what a drag from two clicks into "for" to "Friday" selects,
/// as the field's first touch (it has not got the keyboard yet); what two clicks select; and what
/// a plain drag between the same two places does.
fn by_words(h: &mut Window, top: f32) -> [Option<String>; 3] {
    let (on, into) = (on_char(h, TEXT, top, 6, 0.5), on_char(h, TEXT, top, 11, 0.5));
    clicks_and_drag(h, on, 2, into);
    let dragged = selected(h, TEXT);
    clicks_and_drag(h, on, 2, on);
    let word = selected(h, TEXT);
    clicks_and_drag(h, on, 1, into);
    [dragged, word, selected(h, TEXT)]
}

/// Words, then by characters for the plain drag.
fn as_words() -> [Option<String>; 3] {
    [Some("for Friday".into()), Some("for".into()), Some("r Fri".into())]
}

#[test]
fn a_drag_from_two_clicks_in_the_subject_goes_on_by_words() {
    let mut h = writing();
    let found = fields(&h, TEXT);
    assert_eq!(found.len(), 2, "the To and Subject lines");
    let subject = found[1].min.y;
    assert_eq!(by_words(&mut h, subject), as_words());
}

#[test]
fn a_drag_from_two_clicks_in_the_to_line_goes_on_by_words() {
    let mut h = writing();
    let to = fields(&h, TEXT)[0].min.y;
    assert_eq!(by_words(&mut h, to), as_words());
}

#[test]
fn a_drag_from_two_clicks_in_the_search_box_goes_on_by_words() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    h.run_steps(4);
    testing::text(&mut h, TEXT);
    h.run_steps(4);
    let search = fields(&h, TEXT)[0].min.y;
    assert_eq!(by_words(&mut h, search), as_words());
}

/// As a quick hand on a slow frame gives it: the first click's release and the second press in
/// one frame, that press held and dragged.
#[test]
fn a_drag_from_two_clicks_run_together_in_the_subject_goes_on_by_words() {
    let mut h = writing();
    let subject = fields(&h, TEXT)[1].min.y;
    let (on, into) = (on_char(&h, TEXT, subject, 6, 0.5), on_char(&h, TEXT, subject, 11, 0.5));
    let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.input_mut().events.push(Event::PointerMoved(on));
    h.step();
    h.input_mut().events.push(button(on, true));
    h.step();
    h.input_mut().events.extend([button(on, false), button(on, true)]);
    h.step();
    for n in 1..=5 {
        h.input_mut().events.push(Event::PointerMoved(on + (into - on) * (n as f32 / 5.0)));
        h.step();
    }
    h.input_mut().events.push(button(into, false));
    h.run_steps(2);
    assert_eq!(selected(&h, TEXT).as_deref(), Some("for Friday"));
}
