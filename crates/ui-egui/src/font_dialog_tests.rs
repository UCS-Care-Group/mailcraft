// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The font an account writes in: the Default fonts section of File › Signatures, the dialog its
//! buttons open, and what OK writes back.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_model::FontSlot;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::font_dialog::FontDialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

fn window_with(app: MailApp, tall: bool) -> Harness<'static, MailApp> {
    // The page an account's fonts are chosen on is a long one; its buttons are only where the
    // window is tall enough to show them.
    let size = if tall { egui::vec2(1280.0, 1700.0) } else { egui::vec2(1280.0, 900.0) };
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(size).build_ui_state(
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

fn window(app: MailApp) -> Harness<'static, MailApp> {
    window_with(app, false)
}

/// The page an account's fonts are chosen on.
fn page() -> Harness<'static, MailApp> {
    let mut a = app();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    window_with(a, true)
}

/// The account the sample mailbox signs in with.
fn account(app: &MailApp) -> u64 {
    app.session.mailbox.default_account().unwrap().id
}

/// The dialog, opened the way the page's button opens it.
fn open(h: &mut Harness<'static, MailApp>, slot: &str) {
    h.state_mut().run("ui.dialog", json!({"name": "font", "slot": slot})).unwrap();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::Font(_))), "the {slot} dialog did not open");
}

/// The dialog out of the harness, so a test can set a field as the owner would with the mouse.
fn dialog_mut<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut FontDialog {
    let Some(Dialog::Font(d)) = &mut h.state_mut().dialog else {
        panic!("no font dialog is open");
    };
    d
}

fn font(app: &MailApp, slot: FontSlot) -> Option<mailcraft_model::rich::Style> {
    app.session.mailbox.account(account(app)).and_then(|a| a.default_font(slot).cloned())
}

#[test]
fn the_page_offers_each_font_of_each_account() {
    let mut h = page();
    // Nothing chosen yet: every button says the message's own font stands.
    assert_eq!(h.query_all_by_label("(the message's own)").count(), 3);
    h.get_all_by_label("(the message's own)").next().unwrap().click();
    h.run_steps(3);
    let Some(Dialog::Font(d)) = &h.state().dialog else { panic!("the dialog did not open") };
    // The first button of the row is the font for new messages, of the default account.
    assert_eq!((d.account, d.slot), (account(h.state()), FontSlot::New));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "Cancel closed it");
}

#[test]
fn ok_writes_the_font_chosen_and_closes() {
    let mut h = page();
    open(&mut h, "reply");
    {
        let d = dialog_mut(&mut h);
        d.family = "Georgia".into();
        d.size = Some(14);
        d.bold = true;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "OK closed it");
    let now = font(h.state(), FontSlot::Reply).expect("a reply font");
    assert_eq!((now.font.map(|f| f.key().to_string()), now.size, now.bold), (Some("georgia".into()), Some(14), true));
    // The other two are as they were.
    assert!(font(h.state(), FontSlot::New).is_none() && font(h.state(), FontSlot::Plain).is_none());
}

#[test]
fn the_dialog_shows_what_is_chosen_already() {
    let mut a = app();
    a.run("signature.fonts", json!({"new": {"font": "Georgia", "size": 14, "italic": true}})).unwrap();
    let mut h = window(a);
    open(&mut h, "new");
    {
        let d = dialog_mut(&mut h);
        assert_eq!(d.family, "georgia,serif");
        assert_eq!(d.size, Some(14));
        assert!(d.italic && !d.bold);
        assert_eq!(d.shown_style(), "Italic");
    }
    // The box says whose font it is, and which of its three; the account's address is in it.
    let state = h.state_mut();
    let title = match &state.dialog {
        Some(Dialog::Font(d)) => d.title(state),
        _ => panic!("the dialog is open"),
    };
    assert!(title.starts_with("Font"), "{title}");
    assert!(title.contains(&state.session.mailbox.default_account().unwrap().email), "{title}");
}

#[test]
fn a_font_that_isnt_one_is_said_and_the_dialog_stays_open() {
    let mut h = page();
    open(&mut h, "new");
    {
        let d = dialog_mut(&mut h);
        d.family = "<script>".into();
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let Some(Dialog::Font(d)) = &h.state().dialog else { panic!("a font that isn't one keeps the dialog open") };
    assert!(!d.error.is_empty(), "why it was refused is said");
    assert!(font(h.state(), FontSlot::New).is_none(), "nothing was written");
    // Fixed, it goes in.
    dialog_mut(&mut h).family = "Verdana".into();
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(font(h.state(), FontSlot::New).and_then(|s| s.font).map(|f| f.key().to_string()).as_deref(), Some("verdana"));
}

#[test]
fn giving_the_font_back_to_the_message_is_a_clear_choice() {
    let mut a = app();
    a.run("signature.fonts", json!({"plain": {"font": "Courier New", "size": 10}})).unwrap();
    let mut h = window(a);
    open(&mut h, "plain");
    {
        let d = dialog_mut(&mut h);
        d.family.clear();
        d.size = None;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(font(h.state(), FontSlot::Plain).is_none(), "the message's own font stands again");
}

#[test]
fn a_slot_that_is_nobody_s_or_an_account_there_isnt_opens_nothing() {
    let mut a = app();
    assert!(a.run("ui.dialog", json!({"name": "font", "slot": "signature"})).is_err());
    assert!(a.dialog.is_none());
    assert!(a.run("ui.dialog", json!({"name": "font"})).is_err());
    assert!(a.run("ui.dialog", json!({"name": "font", "slot": "new", "account": 999_999})).is_err());
    assert!(a.dialog.is_none());
    // An account a newer MailCraft added keeps its settings there.
    let gone = a.session.mailbox.add_account(mailcraft_model::Account {
        email: "x@y.example".into(),
        kind: mailcraft_model::AccountKind::Unknown,
        ..Default::default()
    });
    assert!(a.run("ui.dialog", json!({"name": "font", "slot": "new", "account": gone})).is_err());
    assert!(a.dialog.is_none());
}

#[test]
fn the_font_a_message_is_written_in_shows_on_the_page() {
    let mut a = app();
    a.run("signature.fonts", json!({"new": {"font": "Georgia", "size": 14}, "reply": {"bold": true}})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Georgia, 14").is_some(), "the new-message font is on its button");
    assert!(h.query_by_label("Bold").is_some(), "the reply font is on its button");
    assert!(h.query_by_label("(the message's own)").is_some(), "and the plain-text one isn't chosen");
}
