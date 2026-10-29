// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The primary selection of Linux and the BSDs in the body of a message being written and in a
//! signature, as egui's own text fields have it: what is selected goes there once it settles (the
//! button let go, or a key that selects), and a middle click pastes it where the pointer is, as
//! plain text. What an agent or a script selects through the control channel does not go there.

use egui::{Event, Key, Modifiers, OutputCommand, PointerButton, Pos2};
use mailcraft_model::rich::Selection;
use serde_json::json;

use super::{Board, LINES, Window, editor, on_char, plain, sel, signature_page, signature_text, writing_lines};
use crate::control::{ControlRequest, Outcome, handle};

/// What the frame just run put in the primary selection.
fn settled(h: &Window) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            OutputCommand::TextSelectionSettled(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Each event in a frame of its own: what each of those frames put in the primary selection.
fn frames(h: &mut Window, events: impl IntoIterator<Item = Event>) -> Vec<Vec<String>> {
    events
        .into_iter()
        .map(|event| {
            h.event(event);
            h.step();
            settled(h)
        })
        .collect()
}

/// So many frames with nothing new: what they put in the primary selection.
fn quiet(h: &mut Window, count: usize) -> Vec<String> {
    (0..count)
        .flat_map(|_| {
            h.step();
            settled(h)
        })
        .collect()
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

/// A press at `from`, a move to `to` in five steps and the release there.
fn drag_events(from: Pos2, to: Pos2) -> Vec<Event> {
    let mut events = vec![Event::PointerMoved(from), button(from, PointerButton::Primary, true)];
    events.extend((1..=5).map(|n| Event::PointerMoved(from + (to - from) * (n as f32 / 5.0))));
    events.push(button(to, PointerButton::Primary, false));
    events
}

/// So many clicks in a row at `pos`, a frame down and a frame up each.
fn click_events(pos: Pos2, count: usize) -> Vec<Event> {
    let mut events = vec![Event::PointerMoved(pos)];
    for _ in 0..count {
        events.extend([true, false].map(|pressed| button(pos, PointerButton::Primary, pressed)));
    }
    events
}

/// A key pressed and let go with `modifiers`, and a frame after: what they put in the primary selection.
fn keyed(h: &mut Window, modifiers: Modifiers, key: Key) -> Vec<String> {
    let mut put = frames(h, [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers })).concat();
    put.extend(quiet(h, 1));
    put
}

/// A middle click at `at` on X11 or Wayland, with `text` in the primary selection: egui-winit
/// gives the press and the paste in the same frame.
fn middle_click(h: &mut Window, at: Pos2, text: &str) {
    h.event(Event::PointerMoved(at));
    h.step();
    h.input_mut().events.extend([button(at, PointerButton::Middle, true), Event::MiddleClickPaste { pos: at, text: text.to_string() }]);
    h.step();
    h.event(button(at, PointerButton::Middle, false));
    h.step();
    h.run_steps(2);
}

/// Long enough for the next click not to count as one more of the same row.
fn pause(h: &mut Window) {
    h.run_steps(8);
}

/// A request over the control channel, as the owner's script sends it or, with `agent`, as an
/// agent does through the MCP bridge, allowed to drive the window's input.
fn request(h: &mut Window, method: &str, params: serde_json::Value, agent: bool) {
    let (req, _rx) = ControlRequest::new(method, params);
    let req = if agent { req.from_agent(vec![mailcraft_engine::agent::Class::Input]) } else { req };
    let ctx = h.ctx.clone();
    match handle(h.state_mut(), &ctx, &req) {
        Outcome::Done(answer) => assert_eq!(answer["ok"], true, "{answer}"),
        _ => panic!("{method} was not answered at once"),
    }
}

/// Frames as the desktop runs them, each with what the control channel injected for it
/// (`raw_input_hook`), until all of it is in, and a few more. What they put in the primary
/// selection.
fn driven(h: &mut Window) -> Vec<String> {
    let mut put = Vec::new();
    for _ in 0..200 {
        let last = h.state().synthetic.is_empty();
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
        put.extend(settled(h));
        if last {
            break;
        }
    }
    put.extend(quiet(h, 4));
    put
}

/// Where the drag over "beta" starts and ends: the gaps before its first letter and after its last.
fn beta(h: &Window) -> (Pos2, Pos2) {
    (on_char(h, 0, 6), on_char(h, 0, 10))
}

#[test]
fn a_drag_in_the_body_settles_once_in_the_primary_selection() {
    let mut h = writing_lines();
    let ((from, to), elsewhere) = (beta(&h), on_char(&h, 1, 2));
    let put = frames(&mut h, drag_events(from, to));
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["beta"]);
    assert!(quiet(&mut h, 5).is_empty());
    // A click puts the caret: nothing is told for that; the same word selected again is.
    pause(&mut h);
    assert!(frames(&mut h, click_events(elsewhere, 1)).concat().is_empty());
    pause(&mut h);
    assert_eq!(frames(&mut h, drag_events(from, to)).concat(), ["beta"]);
}

#[test]
fn two_clicks_settle_a_word_and_three_a_paragraph_in_the_body() {
    let mut h = writing_lines();
    let on = on_char(&h, 0, 7);
    let put = frames(&mut h, click_events(on, 2));
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["beta"]);
    pause(&mut h);
    let put = frames(&mut h, click_events(on, 3)).concat();
    assert_eq!(put.last().map(String::as_str), Some(LINES[0]), "{put:?}");
}

#[test]
fn a_selection_made_with_the_keys_settles_in_the_primary_selection() {
    let mut h = writing_lines();
    let word = Modifiers::CTRL | Modifiers::SHIFT;
    assert_eq!(keyed(&mut h, word, Key::ArrowLeft), ["zeta"]);
    assert_eq!(keyed(&mut h, word, Key::ArrowLeft), ["epsilon zeta"]);
    // Moving the caret lets go of it, and tells nothing.
    assert!(keyed(&mut h, Modifiers::NONE, Key::ArrowRight).is_empty());
    assert_eq!(keyed(&mut h, Modifiers::COMMAND, Key::A), [LINES.join("\n")]);
}

#[test]
fn a_middle_click_pastes_the_primary_selection_where_the_pointer_is_as_plain_text() {
    let board = Board::default();
    let mut h = writing_lines();
    let ((at, _), start, word) = (beta(&h), on_char(&h, 0, 0), on_char(&h, 1, 2));
    // What the clipboard holds is not what a middle click pastes.
    h.state_mut().services.clipboard = Some(Box::new(board.clone()));
    board.0.borrow_mut().1 = Some("<p>the clipboard</p>".into());
    middle_click(&mut h, at, "<b>new</b> ");
    assert_eq!(plain(&h), "alpha <b>new</b> beta gamma delta\nepsilon zeta");
    let body = h.state().session.compose.as_ref().unwrap().body.clone();
    assert!(!body.text().style_at(8).bold, "the primary selection is plain text");
    // The caret after what was pasted, and the keyboard in the body.
    let after = "alpha <b>new</b> ".chars().count();
    assert_eq!(sel(&h), Selection { anchor: after, head: after });
    assert!(editor::has_focus(&h.ctx));
    // Undone in one step.
    h.event(Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.run_steps(2);
    assert_eq!(plain(&h), LINES.join("\n"));
    // With a word selected, it goes where the pointer is, and the word stays.
    pause(&mut h);
    frames(&mut h, click_events(word, 2));
    middle_click(&mut h, start, "> ");
    assert_eq!(plain(&h), "> alpha beta gamma delta\nepsilon zeta");
    // Too much for one paste is refused as a paste of it would be; the text stays as it was.
    middle_click(&mut h, start, &"x".repeat(5 << 20));
    assert_eq!(plain(&h), "> alpha beta gamma delta\nepsilon zeta");
}

#[test]
fn a_middle_click_pastes_into_a_signature_and_not_into_a_body_that_is_only_drawn() {
    let mut h = signature_page();
    let r = h.state().hit.rect("signatureBody", 0).unwrap();
    middle_click(&mut h, r.min + egui::vec2(1.0, 8.0), "Dr ");
    assert_eq!(signature_text(&h), "Dr Avery Quinn\nLumen Studio");
    // A message whose body is only drawn while a signature is open takes nothing.
    let mut h = writing_lines();
    h.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    h.run_steps(3);
    let signature = signature_text(&h);
    let (at, _) = beta(&h);
    middle_click(&mut h, at, "nothing ");
    assert_eq!((plain(&h), signature_text(&h)), (LINES.join("\n"), signature));
}

#[test]
fn what_a_script_or_an_agent_selects_in_the_body_stays_out_of_the_primary_selection() {
    for agent in [false, true] {
        let mut h = writing_lines();
        let ((from, to), (in_from, in_to)) = (beta(&h), (on_char(&h, 1, 0), on_char(&h, 1, 7)));
        request(&mut h, "engine.execute", json!({"command": "compose.select", "params": {"from": 0, "to": 5}}), agent);
        assert!(driven(&mut h).is_empty());
        assert_eq!(sel(&h), Selection { anchor: 0, head: 5 });
        // The owner's own key from there is told.
        assert_eq!(keyed(&mut h, Modifiers::SHIFT, Key::ArrowRight), ["alpha "]);
        // A drag the control channel injects selects, and is not told.
        request(&mut h, "ui.drag", json!({"x": in_from.x, "y": in_from.y, "toX": in_to.x, "toY": in_to.y}), agent);
        assert!(driven(&mut h).is_empty());
        assert_eq!(sel(&h).start()..sel(&h).end(), 23..30);
        // The owner's drag after it is.
        pause(&mut h);
        assert_eq!(frames(&mut h, drag_events(from, to)).concat(), ["beta"]);
    }
}

#[test]
fn a_selection_that_undo_brings_back_is_not_the_owners() {
    for agent in [false, true] {
        let mut h = writing_lines();
        // Somebody else's selection, typed over.
        request(&mut h, "engine.execute", json!({"command": "compose.select", "params": {"from": 0, "to": 5}}), agent);
        request(&mut h, "engine.execute", json!({"command": "compose.type", "params": {"text": "X"}}), agent);
        assert!(driven(&mut h).is_empty());
        assert!(plain(&h).starts_with("X beta"));
        // The owner's Ctrl+Z brings back the text and that selection: it is not told.
        assert!(keyed(&mut h, Modifiers::COMMAND, Key::Z).is_empty());
        assert_eq!((plain(&h), sel(&h)), (LINES.join("\n"), Selection { anchor: 0, head: 5 }));
        assert!(quiet(&mut h, 3).is_empty());
        // Ctrl+Y and Ctrl+Z again: neither is.
        assert!(keyed(&mut h, Modifiers::COMMAND, Key::Y).is_empty());
        assert!(keyed(&mut h, Modifiers::COMMAND, Key::Z).is_empty());
        // What the owner selects from there is.
        assert_eq!(keyed(&mut h, Modifiers::SHIFT, Key::ArrowRight), ["alpha "]);
    }
}

#[test]
fn the_owners_select_all_takes_back_the_same_selection_from_an_agent() {
    let mut h = writing_lines();
    let all = LINES.join("\n");
    let len = all.chars().count();
    request(&mut h, "engine.execute", json!({"command": "compose.select", "params": {"from": 0, "to": len}}), true);
    assert!(driven(&mut h).is_empty());
    // The same selection, made by the owner: told, and told again when made again (another
    // program may have taken the primary selection since).
    assert_eq!(keyed(&mut h, Modifiers::COMMAND, Key::A), std::slice::from_ref(&all));
    assert_eq!(keyed(&mut h, Modifiers::COMMAND, Key::A), [all]);
    // So in a signature.
    let mut h = signature_page();
    let at = h.state().hit.rect("signatureBody", 0).unwrap().min + egui::vec2(1.0, 8.0);
    frames(&mut h, click_events(at, 1));
    let whole = signature_text(&h);
    let len = whole.chars().count();
    request(&mut h, "engine.execute", json!({"command": "compose.select", "params": {"from": 0, "to": len}}), true);
    assert!(driven(&mut h).is_empty());
    assert_eq!(keyed(&mut h, Modifiers::COMMAND, Key::A), [whole]);
}
