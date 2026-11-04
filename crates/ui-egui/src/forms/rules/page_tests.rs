// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Rules dialog laid out as Outlook's list of rules (#566): a search by name, a rule opened
//! to show what it does, its menu, and the New rule page in the lists' place.

use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT as _, Queryable as _};

use super::*;
use crate::people_view::testing::key;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A window with rules `names`, each marking mail with "x" in its subject read, and the Rules
/// dialog open.
fn window(names: &[&str]) -> Harness<'static, MailApp> {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    for name in names {
        let rule = json!({"name": name, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]});
        app.execute("rules.add", rule).unwrap();
    }
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
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

/// Type `text` in the dialog's search field, in place of what is there: the window's own search
/// box is the other text field.
fn type_search(h: &mut Harness<'_, MailApp>, text: &str) {
    h.get_all_by_role(egui::accesskit::Role::TextInput).last().unwrap().focus();
    h.run_steps(2);
    h.get_all_by_role(egui::accesskit::Role::TextInput).last().unwrap().set_text_value_accesskit(text);
    h.run_steps(3);
}

fn names(h: &Harness<'_, MailApp>) -> Vec<String> {
    h.state().session.mailbox.rules.iter().map(|r| r.name.clone()).collect()
}

#[test]
fn the_search_shows_the_rules_by_name_and_a_rule_moves_among_them_all() {
    let mut h = window(&["Alpha", "Beta", "Gamma"]);
    assert!(h.query_by_label("Alpha").is_some() && h.query_by_label("Gamma").is_some());
    type_search(&mut h, "GAM");
    assert!(h.query_by_label("Alpha").is_none() && h.query_by_label("Beta").is_none());
    assert!(h.query_by_label("Gamma").is_some());
    // Gamma alone is shown, and still goes up past Beta, which the search leaves out; it can't go
    // down, being last.
    h.get_by_label("Move this rule down: it runs later").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Alpha", "Beta", "Gamma"]);
    h.get_by_label("Move this rule up: it runs earlier").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Alpha", "Gamma", "Beta"]);
    // A search nothing has.
    type_search(&mut h, "zzz");
    assert!(h.query_by_label("No rule here has that name.").is_some());
}

#[test]
fn a_rule_opened_says_what_it_does_and_its_menu_runs_and_deletes_it() {
    let mut h = window(&["Alpha", "Beta"]);
    assert!(h.query_by_label("subject contains \"x\"").is_none());
    h.get_all_by_label("Show what this rule does").next().unwrap().click();
    h.run_steps(3);
    assert!(h.query_by_label("subject contains \"x\"").is_some() && h.query_by_label("mark it as read").is_some());
    // The name opens and closes it too.
    h.get_by_label("Alpha").click();
    h.run_steps(3);
    assert!(h.query_by_label("mark it as read").is_none());
    // Its menu: run it now over the folder in view, delete it.
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Run Rule Now").click();
    h.run_steps(3);
    assert!(h.query(By::new().label_contains("matched")).is_some());
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Delete Rule").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Beta"]);
}

#[test]
fn new_rule_takes_the_lists_place_and_cancel_makes_nothing() {
    let mut h = window(&["Alpha"]);
    assert!(h.query_by_label("Add Rule").is_none());
    h.get_by_label("New Rule").click();
    h.run_steps(3);
    // The page of the new rule, with its parts each in a box, and the lists gone.
    for said in ["Conditions", "Exceptions", "Actions", "Add Rule", "Cancel"] {
        assert!(h.query_by_label(said).is_some(), "{said}");
    }
    assert!(h.query_by_label("Alpha").is_none() && h.query_by_label("New Rule").is_none());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_label("Alpha").is_some() && h.query_by_label("Add Rule").is_none());
    assert_eq!(names(&h), ["Alpha"]);
    assert_eq!(h.state().dialog.as_ref().map(crate::dialogs::Dialog::name), Some("rules"));
}

#[test]
fn a_rule_the_server_locks_moves_with_its_arrows_as_any_rule_does() {
    let mut s = Session::at(Mailbox::default(), NOW, 0);
    let account = s.run("account.add", &json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let rule = |id: &str, name: &str, sequence: i32, locked: bool| {
        json!({"id": id, "name": name, "sequence": sequence, "enabled": true, "readOnly": locked,
               "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]})
    };
    let download = json!({"account": account, "email": "noor@example.org", "complete": true,
        "rules": [rule("a", "Alpha", 1, false), rule("b", "Locked", 2, true), rule("c", "Gamma", 3, false), rule("d", "Delta", 4, false)]});
    s.run("sync.applyRules", &json!({"download": download})).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    let order = |h: &Harness<'_, MailApp>| h.state().session.mailbox.rules.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    let greyed = |h: &Harness<'_, MailApp>, label: &str| h.get_all_by_label(label).map(|n| n.accesskit_node().is_disabled()).collect::<Vec<_>>();
    let (up, down) = ("Move this rule up: it runs earlier", "Move this rule down: it runs later");
    // Greyed only at the ends of the list, the locked rule's arrows too.
    assert_eq!(greyed(&h, up), [true, false, false, false]);
    assert_eq!(greyed(&h, down), [false, false, false, true]);
    // Down on the locked rule moves it down; then Alpha goes down, and the locked rule up past it.
    h.get_all_by_label(down).nth(1).unwrap().click();
    h.run_steps(3);
    assert_eq!(order(&h), ["Alpha", "Gamma", "Locked", "Delta"]);
    assert!(h.state().dialog.as_ref().is_some_and(|d| d.name() == "rules"));
    // Its new place is told to the server, first.
    let ops = mailcraft_engine::cmd::rules_remote::rule_upload(&h.state().session.mailbox, account).unwrap().ops;
    assert!(ops.first().is_some_and(|op| op.id == "b" && op.locked && op.sequence == Some(3)), "{ops:?}");
    h.get_all_by_label(down).next().unwrap().click();
    h.run_steps(3);
    assert_eq!(order(&h), ["Gamma", "Alpha", "Locked", "Delta"]);
    h.get_all_by_label(up).nth(2).unwrap().click();
    h.run_steps(3);
    assert_eq!(order(&h), ["Gamma", "Locked", "Alpha", "Delta"]);
}

/// Escape on the New rule page goes back to the lists, as Cancel does; on the lists it closes the
/// dialog. Its window is one of its own (one the owner moves and sizes): Escape works the same
/// once a click in it made it the window the keys go to.
#[test]
fn escape_goes_back_from_the_new_rule_page_then_closes_the_rules() {
    for clicked in [false, true] {
        let mut h = window(&["Alpha"]);
        h.get_by_label("New Rule").click();
        h.run_steps(3);
        assert!(h.query_by_label("Add Rule").is_some());
        if clicked {
            let at = h.get_by_label("Conditions").rect().center();
            crate::people_view::testing::click(&mut h, egui::pos2(at.x, at.y), egui::PointerButton::Primary, 1);
            assert_eq!(crate::keys::active(&h.ctx), crate::dialogs::sized_layer("rules"));
        }
        key(&mut h, egui::Key::Escape);
        assert!(h.query_by_label("Add Rule").is_none() && h.query_by_label("Alpha").is_some(), "{clicked}: back on the lists");
        assert_eq!(h.state().dialog.as_ref().map(crate::dialogs::Dialog::name), Some("rules"), "{clicked}");
        key(&mut h, egui::Key::Escape);
        assert!(h.state().dialog.is_none(), "{clicked}");
        assert_eq!(names(&h), ["Alpha"]);
    }
}

/// The size the owner left the dialog at is the size it opens at, and opening it doesn't change
/// what is kept.
#[test]
fn the_rules_dialog_opens_at_the_size_it_was_left_at() {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    (app.ui.rules_width, app.ui.rules_height) = (900.0, 700.0);
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(8);
    assert_eq!((h.state().ui.rules_width, h.state().ui.rules_height), (900.0, 700.0));
    // Its corner dragged 100 points right and 50 down: that is the size kept.
    let rect = h.ctx.memory(|m| m.area_rect(crate::dialogs::sized_layer("rules").id)).unwrap();
    let corner = rect.max - egui::vec2(3.0, 3.0);
    h.input_mut().events.push(egui::Event::PointerMoved(corner));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: corner,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    for step in 1..=5 {
        h.input_mut().events.push(egui::Event::PointerMoved(corner + egui::vec2(20.0, 10.0) * step as f32));
        h.run_steps(2);
    }
    let end = corner + egui::vec2(100.0, 50.0);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(4);
    let kept = (h.state().ui.rules_width, h.state().ui.rules_height);
    assert!((kept.0 - 1000.0).abs() <= 2.0 && (kept.1 - 750.0).abs() <= 2.0, "{kept:?}");
}

/// A click in the search box gives it the keyboard, and what is typed lands there: the dialog's
/// own window is the one that takes the keys while it is open.
#[test]
fn a_click_in_the_search_box_lets_the_owner_type_in_it() {
    let mut h = window(&["Alpha", "Beta"]);
    let field = h.get_all_by_role(egui::accesskit::Role::TextInput).last().unwrap().rect().center();
    crate::people_view::testing::click(&mut h, egui::pos2(field.x, field.y), egui::PointerButton::Primary, 1);
    crate::people_view::testing::text(&mut h, "Bet");
    h.run_steps(3);
    assert!(h.query_by_label("Alpha").is_none() && h.query_by_label("Beta").is_some());
}

/// What a rule does is said in the window's language, in the list and in a rule opened there: its
/// conditions and exceptions, the words joining them, its actions with the folder a move goes to,
/// and the stop and the alert. The words it looks for stay as they are.
#[test]
fn a_rule_in_words_is_in_the_windows_language() {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    let inbox = app.session.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Inbox).map(|f| f.id).unwrap();
    let rule = json!({
        "name": "Atlas",
        "conditions": [{"type": "subjectContains", "value": "Atlas"}, {"type": "hasAttachment"}],
        "exceptions": [{"type": "flagged"}, {"type": "toMe"}],
        "actions": [{"type": "moveTo", "value": inbox}, {"type": "markRead"}],
        "stop": true,
        "alert": true,
    });
    let id = app.execute("rules.add", rule).unwrap()["id"].as_u64().unwrap();
    let mb = &app.session.mailbox;
    let r = mb.rules.iter().find(|r| r.id == id).unwrap();
    let english = describe(mb, r);
    assert_eq!(
        english,
        "When subject contains \"Atlas\" and has an attachment, except if is flagged or my address is in To: move it to Inbox, mark it as read, show a desktop alert"
    );
    let _language = crate::i18n::RestoreLanguage::hold();
    crate::i18n::set_current(crate::i18n::Lang::Pseudo);
    let said = describe(mb, r);
    let parts = describe_parts(mb, r);
    crate::i18n::set_current(crate::i18n::Lang::English);
    assert_eq!(
        said,
        "Whén súbjéct cóntáíns \"Atlas\" ánd hás án áttáchmént, éxcépt íf ís flággéd ór my áddréss ís ín Tó: móvé ít tó Ínbóx, márk ít ás réád, shów á désktóp álért"
    );
    let parts: Vec<String> = parts.into_iter().map(|(label, said)| format!("{label}: {said}")).collect();
    assert_eq!(
        parts,
        [
            "Whén: súbjéct cóntáíns \"Atlas\" ánd hás án áttáchmént",
            "Éxcépt íf: ís flággéd ór my áddréss ís ín Tó",
            "Thén: móvé ít tó Ínbóx, márk ít ás réád",
            "Álsó: stóp prócéssíng móré rúlés, shów á désktóp álért",
        ]
    );
}

/// The rules for every account, from before a rule was for one account, are listed apart, under
/// All accounts in "Rules for", and not in an account's list (#566). There they are switched on
/// or off, moved and deleted; a new rule isn't made there. Once there are none, All accounts goes.
#[test]
fn rules_for_every_account_are_listed_apart_under_all_accounts() {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let rule = json!({"name": "Mine", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]});
    app.execute("rules.add", rule).unwrap();
    let id = app.session.mailbox.alloc_id();
    let every = |id: Id, name: &str| Rule {
        id,
        name: name.into(),
        enabled: true,
        conditions: vec![mailcraft_model::Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    app.session.mailbox.rules.push(every(id, "Old"));
    let second = app.session.mailbox.alloc_id();
    app.session.mailbox.rules.push(every(second, "Older"));
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    // The account's list: its own rule, and a word on when those of All accounts run.
    assert!(h.query_by_label("Mine").is_some());
    assert!(h.query_by_label("Old").is_none() && h.query_by_label("Older").is_none());
    assert!(h.query_by_label_contains("those of All accounts first, then these").is_some());
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run_steps(3);
    h.get_by_label("All accounts").click();
    h.run_steps(4);
    assert!(h.query_by_label("Mine").is_none());
    assert!(h.query_by_label("Old").is_some() && h.query_by_label("Older").is_some());
    assert!(h.query_by_label_contains("They run on the mail of every account").is_some());
    // New Rule makes nothing here.
    h.get_by_label("New Rule").click();
    h.run_steps(3);
    assert!(h.query_by_label("Add Rule").is_none());
    // Older goes up past Old, among them alone.
    let downs = h.get_all_by_label("Move this rule up: it runs earlier").collect::<Vec<_>>().len();
    assert_eq!(downs, 2);
    h.get_all_by_label("Move this rule up: it runs earlier").last().unwrap().click();
    h.run_steps(3);
    let order: Vec<String> = h.state().session.mailbox.rules.iter().filter(|r| r.account == 0).map(|r| r.name.clone()).collect();
    assert_eq!(order, ["Older", "Old"]);
    // None left: All accounts goes, and the account's rules are shown.
    h.state_mut().execute("rules.remove", json!({"id": id})).unwrap();
    h.state_mut().execute("rules.remove", json!({"id": second})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Mine").is_some());
    assert!(h.query_by_label("All accounts").is_none());
}
