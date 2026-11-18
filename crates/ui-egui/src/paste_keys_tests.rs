// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui::Modifiers;

struct Board {
    text: Option<&'static str>,
    picture: bool,
}

impl RichClipboard for Board {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn html(&self) -> Option<String> {
        None
    }
    fn image(&self) -> Option<Vec<u8>> {
        self.picture.then(|| vec![1])
    }
    fn text(&self) -> Option<String> {
        self.text.map(str::to_string)
    }
}

fn press(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn run(events: Vec<Event>, board: &Board) -> Vec<Event> {
    let mut events = events;
    translate(&mut events, Some(board));
    events
}

#[test]
fn a_session_is_wayland_when_a_socket_or_display_is_named() {
    assert!(wayland_session(None, Some("wayland-0")));
    assert!(wayland_session(Some("7"), None));
    assert!(!wayland_session(Some(""), Some("")));
    assert!(!wayland_session(None, None));
}

#[test]
fn ctrl_v_pastes_the_text() {
    let board = Board { text: Some("a\r\nb"), picture: true };
    let out = run(vec![press(Key::V, Modifiers::COMMAND)], &board);
    assert!(matches!(out.as_slice(), [Event::Paste(t)] if t == "a\nb"));
}

#[test]
fn ctrl_v_with_only_a_picture_sends_an_empty_paste() {
    let board = Board { text: None, picture: true };
    let out = run(vec![press(Key::V, Modifiers::COMMAND)], &board);
    assert!(matches!(out.as_slice(), [Event::Paste(t)] if t.is_empty()));
    let board = Board { text: Some(""), picture: true };
    assert!(matches!(run(vec![press(Key::Paste, Modifiers::NONE)], &board).as_slice(), [Event::Paste(t)] if t.is_empty()));
}

#[test]
fn ctrl_v_with_nothing_to_paste_sends_nothing() {
    let board = Board { text: None, picture: false };
    assert!(run(vec![press(Key::V, Modifiers::COMMAND)], &board).is_empty());
    let mut events = vec![press(Key::V, Modifiers::COMMAND)];
    translate(&mut events, None);
    assert!(events.is_empty());
}

#[test]
fn cut_and_copy_become_their_events() {
    let board = Board { text: None, picture: false };
    let out = run(vec![press(Key::X, Modifiers::COMMAND), press(Key::C, Modifiers::COMMAND), press(Key::Copy, Modifiers::NONE)], &board);
    assert!(matches!(out.as_slice(), [Event::Cut, Event::Copy, Event::Copy]));
}

#[test]
fn other_keys_and_other_modifiers_pass_through() {
    let board = Board { text: Some("x"), picture: false };
    let shifted = Modifiers { shift: true, ..Modifiers::COMMAND };
    let out = run(vec![press(Key::A, Modifiers::COMMAND), press(Key::V, shifted), press(Key::V, Modifiers::NONE), Event::Text("v".into())], &board);
    assert_eq!(out.len(), 4);
    assert!(out.iter().all(|e| !matches!(e, Event::Paste(_))));
    let release = Event::Key { key: Key::V, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::COMMAND };
    assert!(matches!(run(vec![release], &board).as_slice(), [Event::Key { pressed: false, .. }]));
}

#[test]
fn a_window_of_its_own_turns_the_keys_into_a_paste_when_the_window_layer_does_not() {
    use crate::{MailApp, Services};
    use mailcraft_engine::Session;
    let paste = |on: bool| {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0), Services::default());
        app.services.clipboard = Some(Box::new(Board { text: Some("hello"), picture: false }));
        app.services.clipboard_keys = on;
        let ctx = egui::Context::default();
        let raw = egui::RawInput { events: vec![press(Key::V, Modifiers::COMMAND)], ..Default::default() };
        let mut seen = Vec::new();
        let mut out = ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx();
            crate::windows::clipboard_keys(&app, ctx);
            seen = ctx.input(|i| i.events.clone());
        });
        out.textures_delta.clear();
        seen
    };
    assert!(matches!(paste(true).as_slice(), [Event::Paste(t)] if t == "hello"));
    assert!(matches!(paste(false).as_slice(), [Event::Key { key: Key::V, .. }]));
}

#[test]
fn an_empty_paste_asks_whether_a_picture_is_offered_and_does_not_read_it() {
    struct Offer(std::cell::Cell<u32>);
    impl RichClipboard for Offer {
        fn put(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn html(&self) -> Option<String> {
            None
        }
        fn image(&self) -> Option<Vec<u8>> {
            self.0.set(self.0.get() + 1);
            None
        }
        fn has_image(&self) -> bool {
            true
        }
    }
    let board = Offer(Default::default());
    let mut events = vec![press(Key::V, Modifiers::COMMAND)];
    translate(&mut events, Some(&board));
    assert!(matches!(events.as_slice(), [Event::Paste(t)] if t.is_empty()));
    assert_eq!(board.0.get(), 0);
}
