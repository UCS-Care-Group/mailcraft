// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Wayland menu paste stays with the bounded reader and the field that requested it.

use super::*;

fn requested(h: &Window) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::RequestPaste)))
}

#[test]
fn unavailable_rich_clipboard_never_requests_unbounded_paste_in_a_message_or_signature() {
    for signature in [false, true] {
        let board = Board::default();
        let mut h = if signature { signature_page(&board) } else { writing("Avery Quinn", Some(&board)) };
        h.state_mut().services.clipboard_keys = true;
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
        settle(&mut h, 2);
        let kind = if signature { "signatureBody" } else { "composeBody" };
        let start = h.state().hit.rect(kind, 0).unwrap().min + vec2(2.0, 8.0);
        menu_at(&mut h, start);
        choose(&mut h, &paste());
        for _ in 0..4 {
            assert!(!requested(&h), "{kind}: never bypass the bounded reader");
            frame(&mut h, vec![]);
        }
        let actual = if signature { signature_text(&h) } else { h.state().session.compose.as_ref().unwrap().body.plain() };
        let expected = if signature { "Avery Quinn\nLumen Studio" } else { "Avery Quinn" };
        assert_eq!(actual, expected, "{kind}");
    }
}

#[test]
fn an_empty_rich_clipboard_does_not_request_another_selection() {
    let board = Board::holding("");
    let mut h = writing("Keep this", Some(&board));
    h.state_mut().services.clipboard_keys = true;
    let start = h.state().hit.rect("composeBody", 0).unwrap().min + vec2(2.0, 8.0);
    menu_at(&mut h, start);
    choose(&mut h, &paste());
    for _ in 0..3 {
        assert!(!requested(&h));
        frame(&mut h, vec![]);
    }
    assert_eq!(h.state().session.compose.as_ref().unwrap().body.plain(), "Keep this");
}

#[test]
fn the_menu_does_not_retry_failed_or_oversized_transfers() {
    struct Failed(std::io::ErrorKind);
    impl RichClipboard for Failed {
        fn put(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn html(&self) -> Option<String> {
            None
        }
        fn paste_text(&self) -> std::io::Result<Option<String>> {
            Err(self.0.into())
        }
        fn image(&self) -> Option<Vec<u8>> {
            panic!("a failed text transfer must not start a picture read")
        }
    }
    for error in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::InvalidData, std::io::ErrorKind::BrokenPipe] {
        let board = Board::default();
        let mut h = writing("Keep this", Some(&board));
        h.state_mut().services.clipboard = Some(Box::new(Failed(error)));
        h.state_mut().services.clipboard_keys = true;
        let start = h.state().hit.rect("composeBody", 0).unwrap().min + vec2(2.0, 8.0);
        menu_at(&mut h, start);
        choose(&mut h, &paste());
        for _ in 0..3 {
            assert!(!requested(&h), "{error:?}");
            frame(&mut h, vec![]);
        }
        assert_eq!(h.state().session.compose.as_ref().unwrap().body.plain(), "Keep this");
    }
}

#[test]
fn wayland_menu_paste_replaces_the_selection_without_a_delayed_native_event() {
    for signature in [false, true] {
        let board = Board::holding("Native");
        let mut h = if signature { signature_page(&board) } else { writing("Avery Quinn", Some(&board)) };
        h.state_mut().services.clipboard_keys = true;
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
        settle(&mut h, 2);
        let kind = if signature { "signatureBody" } else { "composeBody" };
        let start = h.state().hit.rect(kind, 0).unwrap().min + vec2(2.0, 8.0);
        menu_at(&mut h, start);
        choose(&mut h, &paste());
        for _ in 0..4 {
            assert!(!requested(&h));
            frame(&mut h, vec![]);
        }
        let actual = if signature { signature_text(&h) } else { h.state().session.compose.as_ref().unwrap().body.plain() };
        let expected = if signature { "Native Quinn\nLumen Studio" } else { "Native Quinn" };
        assert_eq!(actual, expected, "{kind}");
        assert_eq!(board.0.borrow().reads, 1, "one bounded read, captured for its target");
    }
}

#[test]
fn a_wayland_paste_followed_by_a_click_never_goes_to_another_field() {
    let board = Board::holding("Clipboard text");
    let mut h = writing("Keep this", Some(&board));
    h.state_mut().services.clipboard_keys = true;
    h.ctx.options_mut(|o| o.max_passes = std::num::NonZero::new(1).unwrap());
    let start = body_start(&h);
    let line = subject_line(&h);
    choose_then_click(&mut h, start, &paste(), line.center());
    for _ in 0..4 {
        assert!(!requested(&h));
        frame(&mut h, vec![]);
    }
    assert_eq!(subject(&h), "Plan for Friday");
    assert_eq!(h.state().session.compose.as_ref().unwrap().body.plain(), "Keep this");
}

#[test]
fn wayland_plain_fields_copy_and_cut_through_the_hosts_clipboard() {
    let board = Board::default();
    let mut h = writing("", Some(&board));
    h.state_mut().services.clipboard_keys = true;
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    for label in [copy(), cut()] {
        menu_at(&mut h, line.center());
        choose(&mut h, &label);
        settle(&mut h, 3);
        assert_eq!(board.0.borrow().put.last().map(|(_, text)| text.as_str()), Some("Plan for Friday"));
        assert!(!h.output().platform_output.commands.iter().any(|c| matches!(c, OutputCommand::CopyText(_))));
    }
    assert_eq!(board.0.borrow().put.len(), 2);
    assert_eq!(subject(&h), "");
}

#[test]
fn a_wayland_detached_windows_plain_field_copies_through_the_hosts_clipboard() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().services.clipboard_keys = true;
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let native = natively(&mut h);
    let id = h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap()["window"].as_u64().unwrap();
    let viewport = viewport_of(id);
    settle(&mut h, 4);
    let title = rect_in(&native, viewport, native_field("")).unwrap();
    click_in(&mut h, &native, viewport, title.center(), PointerButton::Primary);
    in_window(&mut h, &native, viewport, vec![Event::Text("Synthetic appointment".into())]);
    in_window(
        &mut h,
        &native,
        viewport,
        vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }],
    );
    in_window(&mut h, &native, viewport, vec![Event::Copy]);
    settle(&mut h, 2);
    assert_eq!(board.0.borrow().put.last().map(|(_, text)| text.as_str()), Some("Synthetic appointment"));
    assert_eq!(board.0.borrow().put.len(), 1);
    assert!(native.borrow().outs.values().all(|out| !out.platform_output.commands.iter().any(|c| matches!(c, OutputCommand::CopyText(_)))));
}
