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
    assert_eq!(
        super::png(MAX_SIDE + 1, 1, &[0; 4 * (MAX_SIDE + 1)]),
        None
    );
}

#[test]
fn the_html_is_taken_out_of_what_windows_hands_over() {
    assert_eq!(fragment("<b>x</b>"), "<b>x</b>");
    let windows = "Version:0.9\r\nStartHTML:00000097\r\nEndHTML:00000170\r\n<html><body>\r\n<!--StartFragment--><ul><li>a</li></ul><!--EndFragment-->\r\n</body></html>";
    assert_eq!(fragment(windows), "<ul><li>a</li></ul>");
    assert_eq!(fragment("Version:0.9\r\nStartHTML:1\r\n<p>y</p>"), "<p>y</p>");
    assert_eq!(
        fragment("Version:0.9\r\n<!--StartFragment-->no end"),
        "<!--StartFragment-->no end"
    );
    assert_eq!(fragment("Version:"), "");
}

#[test]
fn wayland_copy_output_consumes_only_successful_text_writes() {
    let ctx = egui::Context::default();
    ctx.output_mut(|output| {
        output
            .commands
            .push(egui::OutputCommand::CopyText("available".into()));
        output
            .commands
            .push(egui::OutputCommand::CopyText("unavailable".into()));
    });

    let mut written = Vec::new();
    route_copy_text_output(&ctx, |text| {
        written.push(text.to_string());
        if text == "available" { Ok(()) } else { Err("clipboard is unavailable".into()) }
    });

    assert_eq!(
        written,
        vec!["available".to_string(), "unavailable".to_string()]
    );
    assert_eq!(
        ctx.output(|output| output.commands.clone()),
        vec![egui::OutputCommand::CopyText("unavailable".into())]
    );
}

#[test]
fn missing_clipboard_keeps_native_copy_output() {
    let ctx = egui::Context::default();
    ctx.output_mut(|output| {
        output
            .commands
            .push(egui::OutputCommand::CopyText("text".into()))
    });

    copy_text_output(&ctx, None);

    assert_eq!(
        ctx.output(|output| output.commands.clone()),
        vec![egui::OutputCommand::CopyText("text".into())]
    );
}

/// The real clipboard of the machine the test runs on: HTML put there comes back, and its
/// text is there for programs that take text. Run by hand in each session type
/// (`WAYLAND_DISPLAY` unset for X11): `cargo test -p mailcraft clipboard -- --ignored`.
#[test]
#[ignore = "uses the system's clipboard"]
fn html_goes_on_the_system_clipboard_and_comes_back() {
    let board = Board::default();
    board
        .put("<ul><li>one<ul><li>two</li></ul></li></ul>", "- one\n   - two")
        .unwrap();
    let other = Board::default();
    assert_eq!(
        other.html().as_deref(),
        Some("<ul><li>one<ul><li>two</li></ul></li></ul>")
    );
    let text = arboard::Clipboard::new().unwrap().get_text().unwrap();
    assert_eq!(text, "- one\n   - two");
    arboard::Clipboard::new().unwrap().set_text("plain").unwrap();
    assert_eq!(other.html(), None);
}
