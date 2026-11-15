// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use egui::Key;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

fn harness(a: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut h = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
    h.run_steps(3);
    h
}

fn click(h: &mut egui_kittest::Harness<'static, MailApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn press_key(h: &mut egui_kittest::Harness<'static, MailApp>, key: Key) {
    for pressed in [true, false] {
        h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

#[test]
fn save_template_dialog_creates_and_inserts_with_fields() {
    let mut a = app();
    a.run("mail.new", json!({"subject": "Follow-up", "body": "Hi {recipient.name},\nCompany: {recipient.company}\nDate: {date}"})).unwrap();

    // Opening template.create without a name opens the SaveTemplate dialog.
    let _ = a.run("template.create", json!({}));
    assert_eq!(a.dialog.as_ref().map(|d| d.name()), Some("saveTemplate"));

    // Submitting via execute saves the template and clears dialog.
    let created = a.execute("template.create", json!({"name": "Follow-up Note"})).unwrap();
    let tid = created["id"].as_u64().unwrap();
    a.execute("ui.closeDialog", json!({})).unwrap();
    assert_eq!(a.session.mailbox.templates().len(), 1);

    // Start a fresh message to a known contact and insert the template.
    a.execute("compose.discard", json!({})).unwrap();
    a.execute("mail.new", json!({"to": "lin.zhao@harborworks.example"})).unwrap();
    a.run("template.insert", json!({"id": tid})).unwrap();

    let c = a.session.compose.as_ref().unwrap();
    assert_eq!(c.subject, "Follow-up");
    assert!(c.body.plain().starts_with("Hi Lin Zhao,\nCompany: Harborworks\nDate: October 14, 2026"));
}

#[test]
fn templates_ribbon_label_command_search_and_gui_dialogs_work_end_to_end() {
    let mut h = harness(app());

    // Outside compose, template.create is disabled in Search Commands (not executable).
    let no_compose_cmds = crate::dialogs::all_commands(h.state());
    let create_entry = no_compose_cmds.iter().find(|c| c.0 == "template.create").unwrap();
    assert!(!create_entry.4, "template.create should be disabled outside compose");

    // Open a standalone compose window (the normal ComposeWindow layout, 980px wide).
    h.state_mut().ui.replies_in_window = true;
    let compose = h
        .state_mut()
        .run(
            "mail.new",
            json!({
                "to": "lin.zhao@harborworks.example",
                "subject": "Status Ping",
                "body": "Hi {recipient.name},\nCompany: {recipient.company}\nDate: {date}"
            }),
        )
        .unwrap()["compose"]
        .as_u64()
        .unwrap();
    h.run_steps(3);
    let w = h.state().windows.of_compose(compose).expect("compose window opened");

    // Simplified ribbon (default): Message tab shows the Templates button directly with visible "Templates" label.
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("templates")).is_some());
    assert!(h.query_by_label_contains("Templates").is_some(), "Templates label must be visible in Simplified Message ribbon");

    // Classic ribbon at compose-window width: Message tab also keeps Templates visible with label.
    h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    h.run_steps(3);
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("templates")).is_some());
    assert!(h.query_by_label_contains("Templates").is_some(), "Templates label must be visible in Classic Message ribbon");

    // Insert tab also contains Templates in both layouts.
    let insert_tab = h.state().hit.rect_in(w, "ribbonTab", crate::ribbon::hit_id("Insert")).expect("Insert tab");
    click(&mut h, insert_tab.center());
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("templates")).is_some());
    assert!(h.query_by_label_contains("Templates").is_some(), "Templates label must be visible in Classic Insert ribbon");
    h.state_mut().execute("ui.ribbonMode", json!({"value": "simplified"})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Templates").is_some(), "Templates label must be visible in Simplified Insert ribbon");

    // Command search matches both singular "template" and plural "templates" via label + "Message › Include › Templates".
    let cmds = crate::dialogs::all_commands(h.state());
    for id in ["template.create", "template.insert", "template.remove"] {
        let entry = cmds.iter().find(|c| c.0 == id).unwrap_or_else(|| panic!("missing {id} in all_commands"));
        assert!(entry.4, "{id} should be enabled while composing in active compose window");
        assert!(crate::dialogs::score("template", &entry.1, &entry.2, &entry.0).is_some(), "{id} should match 'template'");
        assert!(crate::dialogs::score("templates", &entry.1, &entry.2, &entry.0).is_some(), "{id} should match 'templates'");
    }

    // Opening Save as Template via ui.dialog {"name": "saveTemplate"} keeps the subject default instead of "saveTemplate",
    // and displays the plain-text notice.
    h.state_mut().execute("ui.dialog", json!({"name": "saveTemplate"})).unwrap();
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(crate::dialogs::Dialog::SaveTemplate { name, .. }) if name == "Status Ping"));
    assert!(h.query_by_label_contains("Only plain text is kept").is_some());

    // Passing explicit templateName pre-fills even "saveTemplate" cleanly.
    h.state_mut().execute("ui.dialog", json!({"name": "saveTemplate", "templateName": "saveTemplate"})).unwrap();
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(crate::dialogs::Dialog::SaveTemplate { name, .. }) if name == "saveTemplate"));

    // Reset back to "Status Ping" and save via the dialog's Save button.
    h.state_mut().execute("ui.dialog", json!({"name": "saveTemplate"})).unwrap();
    h.run_steps(2);

    // Save template via the dialog's Save button.
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.mailbox.templates().len(), 1);
    let tid = h.state().session.mailbox.templates_sorted()[0].id;

    // Clear the compose window body & subject, then insert the saved template via Search Commands' template.insert flow.
    h.state_mut().execute("compose.set", json!({"subject": "", "body": ""})).unwrap();
    h.run_steps(2);
    let _: Value = h.state_mut().run("template.insert", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("insertTemplate"));

    // Confirm template row is hit-registered and press Enter to insert into the compose window.
    assert!(h.state().hit.rect("pickTemplate", tid).is_some());
    press_key(&mut h, Key::Enter);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());

    let c = h.state().session.composing_by_id(compose).expect("compose still open");
    assert_eq!(c.subject, "Status Ping");
    assert!(c.body.plain().starts_with("Hi Lin Zhao,\nCompany: Harborworks\nDate: October 14, 2026"));

    // Running template.remove without an id opens Remove Template dialog and pressing Enter removes the template.
    h.state_mut().run("template.remove", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("removeTemplate"));
    press_key(&mut h, Key::Enter);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(h.state().session.mailbox.templates().is_empty());
}

#[test]
fn templates_menu_aligns_rows_and_remove_buttons_and_confirms_delete() {
    let mut a = app();
    let short = a.execute("template.create", json!({"name": "Hi", "subject": "S", "body": "B"})).unwrap()["id"].as_u64().unwrap();
    let long = a
        .execute(
            "template.create",
            json!({"name": "Extremely Long Template Name That Exceeds Menu Column Width Easily", "subject": "L", "body": "Body"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();

    let mut h = harness(a);
    h.state_mut().run("mail.new", json!({})).unwrap();
    h.run_steps(3);

    // Clicking the Templates button opens templates_menu; hovering it shows the "Templates" tooltip.
    let btn = h.state().hit.rect("ribbon", crate::ribbon::hit_id("templates")).expect("Templates ribbon button");
    h.event(egui::Event::PointerMoved(btn.center()));
    h.run_steps(45);
    assert!(h.query_all_by_label_contains("Templates").next().is_some(), "Templates tooltip/label must be visible");
    click(&mut h, btn.center());
    h.run_steps(2);

    let r_short = h.state().hit.rect("templateMenuRow", short).expect("short row hit");
    let r_long = h.state().hit.rect("templateMenuRow", long).expect("long row hit");
    assert!((r_short.min.x - r_long.min.x).abs() < 0.5, "short & long rows must align left");
    assert!((r_short.width() - r_long.width()).abs() < 0.5, "short & long rows must have equal fixed width");

    let x_short = h.state().hit.rect("templateMenuRemove", short).expect("short x hit");
    let x_long = h.state().hit.rect("templateMenuRemove", long).expect("long x hit");
    assert!((x_short.min.x - x_long.min.x).abs() < 0.5, "remove buttons must align in one column");
    assert!((x_short.max.x - x_long.max.x).abs() < 0.5);

    // Clicking the remove button asks for confirmation instead of deleting right away.
    click(&mut h, x_short.center());
    assert_eq!(h.state().session.mailbox.templates().len(), 2, "template must remain until confirmed");
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));

    h.get_by_label("Remove").click();
    h.run_steps(3);
    assert_eq!(h.state().session.mailbox.templates().len(), 1);
    assert!(h.state().session.mailbox.template(short).is_none());

    // Removal can also be undone.
    assert!(h.state_mut().session.undo());
    assert!(h.state().session.mailbox.template(short).is_some());
}
