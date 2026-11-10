// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is selected in a message being read goes to the primary selection of Linux and the BSDs
//! once it settles (the button let go, or Select All), as egui's own text does: the text a copy
//! would have, once, and not again while it stays the same. What an agent or a script selects
//! through the control channel does not go there.

use egui::{Event, Modifiers, OutputCommand, PointerButton};
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use super::super::Place;
use super::super::select::{Point, Zone};
use super::{Window, button, clicks, copy_key, copy_of, corpus_app, gap, gap_at, id_of, pause, quick_window, select, selection, whole};
use crate::control::{ControlRequest, handle};

/// What the frame just run put in the primary selection, with Windows' line ends read as `\n`.
fn settled(harness: &Window) -> Vec<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            OutputCommand::TextSelectionSettled(text) => Some(text.replace("\r\n", "\n")),
            _ => None,
        })
        .collect()
}

/// Each event in a frame of its own: what each of those frames put in the primary selection.
fn frames(harness: &mut Window, events: impl IntoIterator<Item = Event>) -> Vec<Vec<String>> {
    events
        .into_iter()
        .map(|event| {
            harness.event(event);
            harness.step();
            settled(harness)
        })
        .collect()
}

/// So many frames with nothing new: what they put in the primary selection.
fn quiet(harness: &mut Window, count: usize) -> Vec<String> {
    (0..count)
        .flat_map(|_| {
            harness.step();
            settled(harness)
        })
        .collect()
}

/// Ctrl+A (Cmd+A on a Mac), and a few frames after it: what they put in the primary selection.
fn select_all(harness: &mut Window) -> Vec<String> {
    let key = |pressed| Event::Key { key: egui::Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND };
    let events = [Event::ModifiersChanged(Modifiers::COMMAND), key(true), key(false), Event::ModifiersChanged(Modifiers::NONE)];
    let mut put = frames(harness, events).concat();
    put.extend(quiet(harness, 3));
    put
}

/// A press at `from`, a move to `to` in five steps and the release there.
fn drag_events(from: egui::Pos2, to: egui::Pos2) -> Vec<Event> {
    let mut events = vec![Event::PointerMoved(from), button(from, PointerButton::Primary, true, Modifiers::NONE)];
    events.extend((1..=5).map(|n| Event::PointerMoved(from + (to - from) * (n as f32 / 5.0))));
    events.push(button(to, PointerButton::Primary, false, Modifiers::NONE));
    events
}

/// So many clicks in a row at `pos`, a frame down and a frame up each.
fn click_events(pos: egui::Pos2, count: usize) -> Vec<Event> {
    let mut events = vec![Event::PointerMoved(pos)];
    for _ in 0..count {
        events.extend([true, false].map(|pressed| button(pos, PointerButton::Primary, pressed, Modifiers::NONE)));
    }
    events
}

/// The gmail reply in the pane.
fn reading() -> (Window, u64) {
    let (app, ids) = corpus_app(false);
    let id = id_of(&ids, "gmail reply");
    let mut harness = quick_window(app, (1280.0, 1400.0));
    select(&mut harness, id);
    (harness, id)
}

/// A request over the control channel, as the owner's script sends it or, with `agent`, as an
/// agent does through the MCP bridge, allowed to drive the window's input.
fn request(harness: &mut Window, method: &str, params: serde_json::Value, agent: bool) {
    let (req, _rx) = ControlRequest::new(method, params);
    let req = if agent { req.from_agent(vec![mailcraft_engine::agent::Class::Input]) } else { req };
    let ctx = harness.ctx.clone();
    match handle(harness.state_mut(), &ctx, &req) {
        crate::control::Outcome::Done(answer) => assert_eq!(answer["ok"], true, "{answer}"),
        _ => panic!("{method} was not answered at once"),
    }
}

/// Frames as the desktop runs them, each with what the control channel injected for it
/// (`raw_input_hook`), until all of it is in, and a few more. What they put in the primary
/// selection.
fn driven(harness: &mut Window) -> Vec<String> {
    let mut put = Vec::new();
    for _ in 0..200 {
        let last = harness.state().synthetic.is_empty();
        let mut raw = std::mem::take(harness.input_mut());
        harness.state_mut().raw_input_hook(&mut raw);
        *harness.input_mut() = raw;
        harness.step();
        put.extend(settled(harness));
        if last {
            break;
        }
    }
    put.extend(quiet(harness, 4));
    put
}

#[test]
fn a_drag_in_the_pane_settles_once_in_the_primary_selection() {
    let (mut harness, id) = reading();
    let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Yes to both", 6));
    let priya = gap(&harness, id, "Hi Priya", 8);
    let put = frames(&mut harness, drag_events(from, to));
    // Nothing while the button is down; the text a copy has once it is let go.
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["Priya,\n\nYes to"]);
    // Not again while it stays as it is, and not for a copy of it.
    assert!(quiet(&mut harness, 5).is_empty());
    assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes to"));
    assert!(settled(&harness).is_empty());
    // Another one is told in its turn.
    pause(&mut harness);
    let put = frames(&mut harness, drag_events(from, priya)).concat();
    assert_eq!(put, ["Priya"]);
    // A click lets go of it: nothing is told for nothing selected; the same text selected again is.
    pause(&mut harness);
    assert!(frames(&mut harness, click_events(to, 1)).concat().is_empty());
    assert!(selection(&harness, Place::Pane).is_none());
    pause(&mut harness);
    assert_eq!(frames(&mut harness, drag_events(from, priya)).concat(), ["Priya"]);
}

#[test]
fn the_header_settles_in_the_primary_selection_too() {
    let (mut harness, _) = reading();
    // The sender in the header, not in the message list beside it.
    let sender = harness.query_all_by_label("Kofi Mensah").map(|node| node.rect()).max_by(|a, b| a.min.x.total_cmp(&b.min.x)).unwrap();
    let put = frames(&mut harness, drag_events(sender.left_center() + egui::vec2(1.0, 0.0), sender.right_center() - egui::vec2(1.0, 0.0)));
    assert_eq!(selection(&harness, Place::Pane).map(|s| s.0), Some(Zone::Header));
    assert_eq!(put.concat(), ["Kofi Mensah"]);
}

#[test]
fn two_clicks_settle_a_word_and_three_a_paragraph() {
    let (mut harness, id) = reading();
    let (text, _) = gap_at(&harness, Place::Pane, id, "Yes to both", 0);
    let word = text.find("projector").unwrap();
    let on = gap(&harness, id, "Yes to both", word + 3) + egui::vec2(1.5, 0.0);
    let put = frames(&mut harness, click_events(on, 2));
    // The word is taken as the button goes down the second time, and told when it comes up.
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["projector"]);
    pause(&mut harness);
    let put = frames(&mut harness, click_events(on, 3)).concat();
    assert_eq!(put.last().map(String::as_str), Some(text.as_str()), "{put:?}");
    // A drag from two clicks goes on by words, and is told once it is let go.
    pause(&mut harness);
    let thursday = text.find("Thursday").unwrap();
    let to = gap(&harness, id, "Yes to both", thursday + 2) + egui::vec2(1.5, 0.0);
    let mut events = click_events(on, 1);
    events.extend(drag_events(on, to));
    let put = frames(&mut harness, events);
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["projector, and Thursday"]);
}

#[test]
fn select_all_by_the_owner_settles_the_whole_message() {
    let (mut harness, id) = reading();
    let (all, origin) = whole(&harness, id);
    clicks(&mut harness, origin + egui::vec2(-14.0, 30.0), 1, Modifiers::NONE);
    let put = select_all(&mut harness);
    assert_eq!(selection(&harness, Place::Pane), Some((Zone::Body, Point::START, Point::END)));
    assert_eq!(put, [all]);
}

#[test]
fn a_message_window_settles_its_own_selection() {
    let (mut harness, id) = reading();
    let own = harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap()["window"].as_u64().unwrap();
    harness.run_steps(8);
    let there = Place::Window(own);
    let (from, to) = (gap_at(&harness, there, id, "Hi Priya", 0).1, gap_at(&harness, there, id, "Hi Priya", 2).1);
    let put = frames(&mut harness, drag_events(from, to));
    let (last, before) = put.split_last().unwrap();
    assert!(before.iter().all(Vec::is_empty), "{put:?}");
    assert_eq!(last, &["Hi"]);
    assert_eq!(selection(&harness, there).map(|s| s.0), Some(Zone::Body));
    assert!(quiet(&mut harness, 5).is_empty());
    // Ctrl+A there is the window's whole text.
    let all = super::super::view::kept_at(&harness.ctx, there, id, |laid, _| copy_of(laid)).unwrap();
    assert_eq!(select_all(&mut harness), [all]);
}

#[test]
fn what_a_script_or_an_agent_selects_stays_out_of_the_primary_selection() {
    for agent in [false, true] {
        let (mut harness, id) = reading();
        let (all, _) = whole(&harness, id);
        // Select All through the control channel: selected, and copied by the owner's Ctrl+C,
        // but not told to the desktop.
        request(&mut harness, "engine.execute", json!({"command": "ui.selectAllInMessage", "params": {"pane": true}}), agent);
        assert!(driven(&mut harness).is_empty());
        assert_eq!(selection(&harness, Place::Pane), Some((Zone::Body, Point::START, Point::END)));
        assert_eq!(copy_key(&mut harness).as_deref(), Some(all.as_str()));
        assert!(quiet(&mut harness, 3).is_empty());
        // The owner's own Select All of the same is told.
        assert_eq!(select_all(&mut harness), std::slice::from_ref(&all));
        // The control channel's again, then the owner's again: told again, for another program
        // may have taken the primary selection since.
        request(&mut harness, "engine.execute", json!({"command": "ui.selectAllInMessage", "params": {"pane": true}}), agent);
        assert!(driven(&mut harness).is_empty());
        assert_eq!(select_all(&mut harness), std::slice::from_ref(&all));
        // A drag the control channel injects selects, and is not told either.
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        let (from, to) = (gap(&harness, id, "Hi Priya", 3), gap(&harness, id, "Yes to both", 6));
        let priya = gap(&harness, id, "Hi Priya", 8);
        request(&mut harness, "ui.drag", json!({"x": from.x, "y": from.y, "toX": to.x, "toY": to.y}), agent);
        assert!(driven(&mut harness).is_empty());
        assert_eq!(copy_key(&mut harness).as_deref(), Some("Priya,\n\nYes to"));
        // The owner's drag after it is.
        pause(&mut harness);
        assert_eq!(frames(&mut harness, drag_events(from, priya)).concat(), ["Priya"]);
    }
}
