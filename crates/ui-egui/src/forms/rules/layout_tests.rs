// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules and Alerts laid out (#2057): the buttons over the list, a long list that scrolls within
//! a dialog that stays on the screen, and the builder in a dialog of its own over the list,
//! opened from New Rule…, a rule's pencil, a double click on a rule and `ui.dialog`, and left
//! with Cancel or Escape with the list as it was.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::*;
use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::{MailApp, Services};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

/// A window with the account on this computer alone and `n` rules of its own.
fn app(n: usize) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    for i in 0..n {
        let rule = json!({"name": format!("Rule {i}"), "conditions": [{"type": "subjectContains", "value": format!("w{i}")}], "actions": [{"type": "markRead"}]});
        app.execute("rules.add", rule).unwrap();
    }
    app
}

fn harness(app: MailApp, size: (f32, f32)) -> Harness<'static, MailApp> {
    // (Steps short enough for two clicks a step apart to be a double click.)
    let builder = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_step_dt(0.05);
    let mut h = builder.with_size(egui::vec2(size.0, size.1)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn form<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut RulesForm {
    let Some(Dialog::Form(Form::Rules(f))) = &mut h.state_mut().dialog else { panic!("the Rules dialog is open") };
    f
}

/// Where the Rules dialog is on the screen.
fn dialog_rect(h: &Harness<'static, MailApp>) -> egui::Rect {
    h.ctx.memory(|m| m.area_rect(egui::Id::unique("dialog"))).expect("the dialog is shown")
}

/// The top of each rule's pencil, in order.
fn pencils(h: &Harness<'static, MailApp>) -> Vec<f32> {
    h.get_all_by_label("Change this rule").map(|n| n.rect().top()).collect()
}

#[test]
fn a_long_list_scrolls_within_a_dialog_that_stays_on_the_screen() {
    for size in [(1280.0, 800.0), (1100.0, 560.0)] {
        let mut a = app(60);
        a.execute("ui.dialog", json!({"name": "rules"})).unwrap();
        let mut h = harness(a, size);
        let screen = h.ctx.content_rect();
        let dialog = dialog_rect(&h);
        assert!(screen.contains_rect(dialog), "{size:?}: the dialog {dialog:?} is within {screen:?}");
        // The list is cut to the room left, and is longer than that.
        let (shown, whole) = form(&mut h).list;
        assert!(shown >= 120 && whole > 2 * shown, "{size:?}: {shown} of {whole}");
        // The buttons are over the list, in reach while it is long.
        for label in ["New Rule…", "Run Rules Now", "Close"] {
            assert!(dialog.contains_rect(h.get_by_label(label).rect()), "{size:?}: {label}");
        }
        let new_rule = h.get_by_label("New Rule…").rect();
        let tops = pencils(&h);
        assert_eq!(tops.len(), 60);
        assert!(tops.iter().all(|y| *y > new_rule.bottom()), "{size:?}: the list is under the buttons");
        assert!(tops.last().is_some_and(|y| *y > dialog.bottom()), "{size:?}: the last rule is out of sight");
        // Scrolled to the last rule: it is in sight, the first isn't, and the dialog stays.
        h.get_all_by_label("Change this rule").last().unwrap().scroll_to_me();
        h.run_steps(3);
        let tops = pencils(&h);
        assert!(tops.last().is_some_and(|y| *y < dialog.bottom() && *y > new_rule.bottom()), "{size:?}: {tops:?}");
        assert!(tops.first().is_some_and(|y| *y < new_rule.bottom()), "{size:?}");
        assert_eq!(dialog_rect(&h), dialog, "{size:?}");
    }
}

#[test]
fn the_builder_opens_over_the_list_from_new_rule_and_escape_leaves_the_list_as_it_was() {
    let mut a = app(30);
    a.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(a, (1280.0, 800.0));
    // Scrolled down a little: the list keeps that under the builder and after it.
    h.get_all_by_label("Change this rule").nth(12).unwrap().scroll_to_me();
    h.run_steps(3);
    let before = pencils(&h);
    assert!(!form(&mut h).modal() && h.query_by_label("Add Rule").is_none());
    h.get_by_label("New Rule…").click();
    h.run_steps(3);
    assert!(form(&mut h).open && form(&mut h).editing.is_none());
    assert!(h.query_by_label("Add Rule").is_some() && h.query_by_label("Cancel").is_some());
    // The name has the keyboard; the list under the builder can't be reached.
    assert!(h.ctx.memory(|m| m.focused()).is_some());
    form(&mut h).name = "Not kept".into();
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!form(&mut h).modal(), "Escape closed the builder");
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("rules"), "and not Rules and Alerts");
    assert_eq!(h.state().session.mailbox.rules.len(), 30, "nothing was added");
    assert_eq!(pencils(&h), before, "the list is where it was");
    // Opened again, it is a new rule's again; Cancel closes it too.
    h.get_by_label("New Rule…").click();
    h.run_steps(3);
    assert!(form(&mut h).name.is_empty());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!form(&mut h).modal());
}

#[test]
fn a_rule_opens_in_the_builder_from_its_pencil_and_from_a_double_click() {
    let mut a = app(3);
    let second = a.session.mailbox.rules.get(1).map(|r| r.id).unwrap();
    a.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(a, (1280.0, 800.0));
    // The pencil.
    h.get_all_by_label("Change this rule").nth(1).unwrap().click();
    h.run_steps(3);
    assert_eq!(form(&mut h).editing.map(|(id, _)| id), Some(second));
    assert!(h.query_by_label("Save").is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!form(&mut h).modal());
    // A double click on the rule, left of its buttons.
    let pencil = h.get_all_by_label("Change this rule").nth(1).unwrap().rect();
    let at = egui::pos2(pencil.left() - 200.0, pencil.center().y);
    // (Long enough after Cancel's click for the two to be a double click, not a triple.)
    h.run_steps(20);
    h.hover_at(at);
    h.step();
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(3);
    assert_eq!(form(&mut h).editing.map(|(id, _)| id), Some(second));
    // Saved: the builder closes, and the rule is changed in place.
    form(&mut h).name = "Changed".into();
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(!form(&mut h).modal());
    assert_eq!(h.state().session.mailbox.rules.get(1).map(|r| r.name.as_str()), Some("Changed"));
}

#[test]
fn ui_dialog_with_edit_opens_the_builder_over_the_list() {
    let mut a = app(3);
    let first = a.session.mailbox.rules.first().map(|r| r.id).unwrap();
    a.execute("ui.dialog", json!({"name": "rules", "edit": first})).unwrap();
    let mut h = harness(a, (1280.0, 800.0));
    assert!(form(&mut h).open && form(&mut h).editing.is_some_and(|(id, _)| id == first));
    assert!(h.query_by_label("Save").is_some());
    assert_eq!(pencils(&h).len(), 3, "the list is there under it");
    // Enter in the name is Save.
    form(&mut h).name = "Renamed".into();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(!form(&mut h).modal());
    assert_eq!(h.state().session.mailbox.rules.first().map(|r| r.name.as_str()), Some("Renamed"));
}

#[test]
fn the_list_takes_what_the_window_leaves_and_never_less_than_its_least() {
    assert_eq!(list_room(800.0, 100.0), 500.0);
    assert_eq!(list_room(200.0, 100.0), MIN_LIST);
    assert_eq!(list_room(f32::NAN, 0.0), MIN_LIST);
    assert_eq!(list_room(f32::INFINITY, 0.0), MIN_LIST);
}
