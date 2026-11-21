// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native text paste when the rich Wayland reader has no selection.

use super::*;

fn requested(h: &Window) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::RequestPaste)))
}

#[test]
fn unavailable_rich_clipboard_pastes_native_text_into_the_message_and_signature() {
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
        let mut native = requested(&h);
        for _ in 0..3 {
            if native {
                break;
            }
            frame(&mut h, vec![]);
            native = requested(&h);
        }
        assert!(native, "{kind}: an unavailable rich reader must try the native text reader");
        // The backend completes the request in the next frame, after focus and selection were
        // restored to the field whose menu was used.
        frame(&mut h, vec![Event::Paste("Native".into())]);
        settle(&mut h, 2);
        let actual = if signature { signature_text(&h) } else { h.state().session.compose.as_ref().unwrap().body.plain() };
        let expected = if signature { "Native Quinn\nLumen Studio" } else { "Native Quinn" };
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
fn the_menu_does_not_retry_a_timed_out_transfer() {
    struct TimedOut;
    impl RichClipboard for TimedOut {
        fn put(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn html(&self) -> Option<String> {
            None
        }
        fn paste_text(&self) -> std::io::Result<Option<String>> {
            Err(std::io::ErrorKind::TimedOut.into())
        }
    }
    let board = Board::default();
    let mut h = writing("Keep this", Some(&board));
    h.state_mut().services.clipboard = Some(Box::new(TimedOut));
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
