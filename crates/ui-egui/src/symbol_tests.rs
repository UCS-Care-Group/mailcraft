// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Symbol, Text Highlight Color and Format Painter in the window.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::cmd::format::SYMBOLS;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn writing(text: &str) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.new", json!({})).unwrap();
    let len = app.session.compose.as_ref().unwrap().body.text().len();
    app.execute("compose.select", json!({"from": 0, "to": len})).unwrap();
    app.execute("compose.type", json!({"text": text})).unwrap();
    app
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn body(app: &MailApp) -> String {
    app.session.compose.as_ref().unwrap().body.plain()
}

#[test]
fn every_symbol_offered_has_a_glyph_in_the_windows_fonts() {
    let h = window(writing("x"));
    let missing: Vec<char> =
        h.ctx.fonts_mut(|f| SYMBOLS.iter().copied().filter(|c| *c != '\u{a0}' && !f.has_glyph(&crate::theme::regular(17.0), *c)).collect());
    assert!(missing.is_empty(), "no glyph for {missing:?}");
}

#[test]
fn a_symbol_clicked_goes_in_and_the_dialog_stays_open() {
    let mut h = window(writing("ab"));
    h.state_mut().run("insert.symbol", json!({})).unwrap();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::Symbol(_))));
    let at = h.state().hit.rect("symbolCell", super::cell_id('€')).unwrap().center();
    crate::people_view::testing::click(&mut h, at, egui::PointerButton::Primary, 1);
    h.run_steps(3);
    assert_eq!(body(h.state()), "ab€");
    if let Some(Dialog::Symbol(d)) = h.state_mut().dialog.as_mut() {
        d.code = "U+2028".into();
    }
    h.run_steps(2);
    h.get_by_label("Insert").click();
    h.run_steps(3);
    assert_eq!(body(h.state()), "ab€");
    assert!(matches!(&h.state().dialog, Some(Dialog::Symbol(d)) if !d.error.is_empty()));
    let at = h.state().hit.rect("symbolClose", 0).unwrap().center();
    crate::people_view::testing::click(&mut h, at, egui::PointerButton::Primary, 1);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn the_tabs_have_highlight_format_painter_and_symbol() {
    let mut app = writing("x");
    app.ui.tab = "Message".into();
    let message = crate::ribbon::describe(&app);
    assert!(message.items().any(|i| i.key == "highlight"));
    assert!(message.items().any(|i| i.command.as_ref().is_some_and(|c| c.0 == "edit.formatPainter")));
    app.ui.tab = "Insert".into();
    let insert = crate::ribbon::describe(&app);
    assert!(insert.items().any(|i| !i.pending && i.command.as_ref().is_some_and(|c| c.0 == "insert.symbol")));
    for tab in [message, insert] {
        assert!(tab.items().all(|i| i.pending || crate::icons::NAMES.contains(&i.icon)));
    }
}
