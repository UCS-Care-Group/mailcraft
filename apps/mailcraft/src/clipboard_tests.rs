// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_ui_egui::MAX_PICTURE_SIDE as MAX_SIDE;

#[test]
fn a_picture_on_the_clipboard_is_kept_as_png() {
    let png = png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    assert_eq!(super::png(2, 2, &[0; 8]), None);
    assert_eq!(super::png(0, 0, &[]), None);
    assert_eq!(super::png(MAX_SIDE + 1, 1, &[0; 4 * (MAX_SIDE + 1)]), None);
}

#[test]
fn the_html_is_taken_out_of_what_windows_hands_over() {
    assert_eq!(fragment("<b>x</b>"), "<b>x</b>");
    let windows = "Version:0.9\r\nStartHTML:00000097\r\nEndHTML:00000170\r\n<html><body>\r\n<!--StartFragment--><ul><li>a</li></ul><!--EndFragment-->\r\n</body></html>";
    assert_eq!(fragment(windows), "<ul><li>a</li></ul>");
    assert_eq!(fragment("Version:0.9\r\nStartHTML:1\r\n<p>y</p>"), "<p>y</p>");
    assert_eq!(fragment("Version:0.9\r\n<!--StartFragment-->no end"), "<!--StartFragment-->no end");
    assert_eq!(fragment("Version:"), "");
}

#[test]
fn fallback_html_is_reused_only_for_matching_text_and_cleared_on_mismatch() {
    let cached = FallbackHtml::default();
    cached.remember("A", "<b>A</b>");
    assert_eq!(cached.matching("A").as_deref(), Some("<b>A</b>"));
    assert_eq!(cached.matching("B"), None);
    assert_eq!(cached.matching("A"), None);
}

#[test]
fn an_observed_native_html_offer_invalidates_the_text_fallback_cache() {
    let cached = FallbackHtml::default();
    cached.remember("A", "<a href=\"https://example.org\"><b>A</b></a>");
    // A successful native HTML read clears the cache before returning the newly offered HTML.
    cached.clear();
    assert_eq!(cached.matching("A"), None);
}

#[test]
fn replacing_cached_fallback_html_replaces_the_old_offer() {
    let cached = FallbackHtml::default();
    cached.remember("A", "<b>A</b>");
    cached.remember("B", "<i>B</i>");
    assert_eq!(cached.matching("B").as_deref(), Some("<i>B</i>"));
    assert_eq!(cached.matching("A"), None);
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn wayland_text_fallback_recovers_when_the_rich_clipboard_is_unavailable() {
    let mut written = None;
    let result = finish_wayland_write(Err("no data-control clipboard".into()), "selected text", |text| {
        written = Some(text.to_string());
        Ok(())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(written.as_deref(), Some("selected text"));
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn wayland_text_fallback_keeps_the_rich_clipboard_when_it_succeeds() {
    let mut tried = false;
    let result = finish_wayland_write(Ok(()), "text", |_| {
        tried = true;
        Err("fallback must not run".into())
    });
    assert_eq!(result, Ok(()));
    assert!(!tried);
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn wayland_text_fallback_preserves_both_failures() {
    let result = finish_wayland_write(Err("rich clipboard unavailable".into()), "text", |_| Err("no active seat".into()));
    assert_eq!(result, Err("rich clipboard unavailable; Wayland text fallback failed: no active seat".into()));
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn wayland_text_fallback_does_not_replace_an_empty_clipboard_with_empty_text() {
    let mut tried = false;
    let result = finish_wayland_write(Err("rich clipboard unavailable".into()), "", |_| {
        tried = true;
        Ok(())
    });
    assert_eq!(result, Err("rich clipboard unavailable".into()));
    assert!(!tried);
}

/// The real clipboard of the machine the test runs on: HTML put there comes back, and its
/// text is there for programs that take text. Run by hand, in each kind of session
/// (`WAYLAND_DISPLAY` unset for X11): `cargo test -p mailcraft clipboard -- --ignored`.
#[test]
#[ignore = "uses the system's clipboard"]
fn html_goes_on_the_system_clipboard_and_comes_back() {
    let board = Board::default();
    board.put("<ul><li>one<ul><li>two</li></ul></li></ul>", "- one\n   - two").unwrap();
    // Another clipboard of this program reads it through the system.
    let other = Board::default();
    assert_eq!(other.html().as_deref(), Some("<ul><li>one<ul><li>two</li></ul></li></ul>"));
    let text = arboard::Clipboard::new().unwrap().get_text().unwrap();
    assert_eq!(text, "- one\n   - two");
    // Text alone on the clipboard: no HTML to paste.
    arboard::Clipboard::new().unwrap().set_text("plain").unwrap();
    assert_eq!(other.html(), None);
}
