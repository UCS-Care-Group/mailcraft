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

/// The Rules form the dialog shows.
fn form<'h>(h: &'h mut Harness<'_, MailApp>) -> &'h mut RulesForm {
    match h.state_mut().dialog.as_mut() {
        Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(f))) => f,
        _ => panic!("the Rules dialog isn't open"),
    }
}

#[test]
fn every_part_a_rule_can_have_comes_back_unchanged_from_the_rule_page() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let rule = json!({"name": "Everything", "account": account,
        "conditions": [{"type": "from", "value": ["a@example.org", "b@example.org"]}, {"type": "fromContains", "value": "shop"},
            {"type": "sentTo", "value": "c@example.org"}, {"type": "toContains", "value": ["team", "all"]}, {"type": "subjectContains", "value": "x"},
            {"type": "bodyContains", "value": "y"}, {"type": "subjectOrBodyContains", "value": "z"}, {"type": "headerContains", "value": "List-Id"},
            {"type": "sentOnlyToMe"}, {"type": "toMe"}, {"type": "ccMe"}, {"type": "toOrCcMe"}, {"type": "notToMe"}, {"type": "hasAttachment"},
            {"type": "meetingRequest"}, {"type": "automaticReply"}, {"type": "flagged"}, {"type": "importance", "value": "low"}],
        "exceptions": [{"type": "sensitivity", "value": "private"}, {"type": "sizeBetween", "value": {"min": 10, "max": 900}},
            {"type": "category", "value": "Blue category"}, {"type": "receivedBetween", "value": {"from": "2026-01-02"}}],
        "actions": [{"type": "moveTo", "value": inbox}, {"type": "copyTo", "value": inbox}, {"type": "categorize", "value": "Blue category"},
            {"type": "clearCategories"}, {"type": "flagFor", "value": "tomorrow"}, {"type": "clearFlag"}, {"type": "markRead"},
            {"type": "setImportance", "value": "high"}, {"type": "delete"}, {"type": "deleteForGood"}],
        "stop": true, "alert": true});
    let id = s.run("rules.add", &rule).unwrap()["id"].as_u64().unwrap();
    let before = s.mailbox.rules[0].clone();
    let mut f = RulesForm::new(&s, &json!({}));
    assert!(f.change(&s.mailbox, &before));
    assert_eq!((f.name.as_str(), f.editing), ("Everything", Some(id)));
    assert_eq!(f.lacks(), None);
    let (command, params) = f.command();
    assert_eq!(command, "rules.edit");
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.rules[0], before);
    // A rule that looks for two categories at once can't be shown on the page, and isn't.
    s.mailbox.rules[0].conditions.push(mailcraft_model::Condition::Category(vec!["A".into(), "B".into()]));
    let two = s.mailbox.rules[0].clone();
    assert!(not_changeable(&two).is_some());
    assert!(!f.change(&s.mailbox, &two));
}

#[test]
fn edit_rule_opens_the_rule_page_filled_in_and_save_changes_it_in_place() {
    let mut h = window(&["Alpha", "Beta"]);
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Edit Rule").click();
    h.run_steps(3);
    for said in ["Edit rule", "Save", "Cancel", "Conditions", "Actions"] {
        assert!(h.query_by_label(said).is_some(), "{said}");
    }
    assert!(h.query_by_label("Add Rule").is_none() && h.query_by_label("New Rule").is_none());
    assert_eq!(form(&mut h).name, "Alpha");
    // Cancel leaves it as it was.
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Alpha", "Beta"]);
    // Changed and saved: the same rule, in the same place, switched on as it was.
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Edit Rule").click();
    h.run_steps(3);
    let id = h.state().session.mailbox.rules[0].id;
    form(&mut h).name = "Alpha, read".into();
    form(&mut h).conds[0].text = "x; y".into();
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["Alpha, read", "Beta"]);
    let r = &h.state().session.mailbox.rules[0];
    assert_eq!((r.id, r.enabled), (id, true));
    assert_eq!(r.conditions, [mailcraft_model::Condition::SubjectContains(vec!["x".into(), "y".into()])]);
    assert!(h.query_by_label("Save").is_none() && h.query_by_label("Alpha, read").is_some(), "back on the lists");
}

#[test]
fn a_rule_the_server_locks_moves_with_its_arrows_and_cant_be_changed() {
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
    // The locked rule's Edit Rule is greyed.
    h.get_all_by_label("More for this rule").nth(1).unwrap().click();
    h.run_steps(3);
    h.get_by_label("Edit Rule").click();
    h.run_steps(3);
    assert!(h.query_by_label("Save").is_none());
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

/// A window over two accounts, each with its standard folders, with rules `every` for every
/// account's mail (from before a rule was for one account, as they are read from a file), and
/// the Rules dialog on All accounts. The accounts' ids.
fn all_accounts(every: Vec<(&str, Vec<Action>)>) -> (Harness<'static, MailApp>, Id, Id) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("First", "first@example.org"), NOW, 0), Services::default());
    app.autosave = false;
    let first = app.session.mailbox.accounts[0].id;
    let second = app.execute("account.add", json!({"email": "second@example.org"})).unwrap()["id"].as_u64().unwrap();
    for (name, actions) in every {
        let id = app.session.mailbox.alloc_id();
        let conditions = vec![mailcraft_model::Condition::FromContains(vec!["news".into()])];
        app.session.mailbox.rules.push(Rule { id, name: name.into(), enabled: true, conditions, actions, ..Default::default() });
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
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run_steps(3);
    h.get_by_label("All accounts").click();
    h.run_steps(4);
    (h, first, second)
}

/// In All accounts, a rule's menu has For One Account and the accounts, and Edit Rule is greyed:
/// a rule for every account is changed as it is given to one (#566). Given to an account, it
/// leaves All accounts at once.
#[test]
fn for_one_account_gives_a_rule_for_every_account_to_one() {
    let (mut h, _, second) = all_accounts(vec![("Flagger", vec![Action::Flag]), ("Reader", vec![Action::MarkRead])]);
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    assert!(h.get_by_label("Edit Rule").accesskit_node().is_disabled());
    h.get_by_label("For One Account").click();
    h.run_steps(3);
    // The account in the menu: the last of its names on screen, over the folder pane's.
    h.get_all_by_label("second@example.org").last().unwrap().click();
    h.run_steps(4);
    let mb = &h.state().session.mailbox;
    let flagger = mb.rules.iter().find(|r| r.name == "Flagger").unwrap();
    assert_eq!((flagger.account, flagger.keep), (second, Keep::Here));
    // Out of All accounts, which still has the other.
    assert!(h.query_by_label("Flagger").is_none() && h.query_by_label("Reader").is_some());
}

/// A rule for every account that moves mail to the first account's Inbox, given to the second:
/// the rule page opens with no folder for the move, Save greyed until one is chosen, and the
/// rule stays in All accounts, as it was, until then; Cancel leaves it there (#566).
#[test]
fn a_rule_given_to_another_account_than_its_folders_opens_for_the_folder_to_be_chosen() {
    let (mut h, first, second) = all_accounts(vec![("News", vec![Action::MarkRead])]);
    let inbox = h.state().session.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap();
    h.state_mut().session.mailbox.rules[0].actions = vec![Action::MoveTo(inbox), Action::MarkRead];
    h.run_steps(2);
    let open = |h: &mut Harness<'_, MailApp>| {
        h.get_all_by_label("More for this rule").next().unwrap().click();
        h.run_steps(3);
        h.get_by_label("For One Account").click();
        h.run_steps(3);
        h.get_all_by_label("second@example.org").last().unwrap().click();
        h.run_steps(4);
    };
    open(&mut h);
    assert!(h.query_by_label_contains("choose the folder again").is_some());
    assert!(h.get_by_label("Save").accesskit_node().is_disabled());
    assert_eq!(h.state().session.mailbox.rules[0].account, 0, "nothing changes before Save");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert_eq!(h.state().session.mailbox.rules[0].account, 0);
    assert!(h.query_by_label("News").is_some(), "back on All accounts");
    // Again, and the second account's Inbox chosen (the first of the list, over the folder
    // pane's): saved, it is the second account's.
    open(&mut h);
    h.get_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.accesskit_node().value().as_deref() == Some("Choose a folder")).unwrap().click();
    h.run_steps(3);
    h.get_all_by_label("Inbox").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(4);
    let mb = &h.state().session.mailbox;
    let theirs = mb.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let r = &mb.rules[0];
    assert_eq!((r.account, r.actions.clone()), (second, vec![Action::MoveTo(theirs), Action::MarkRead]));
}

/// With no account yet, a rule is the first account's once one is added: the dialog lists it
/// with no All accounts, nor a word about them, and New Rule makes more (#566).
#[test]
fn with_no_account_yet_there_is_no_all_accounts() {
    let mut app = MailApp::new(Session::at(Mailbox::default(), NOW, 0), Services::default());
    app.autosave = false;
    let rule = json!({"name": "Early", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]});
    app.execute("rules.add", rule).unwrap();
    assert_eq!(app.session.mailbox.rules[0].account, 0);
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
    assert!(h.query_by_label("Early").is_some());
    assert!(h.query_by_label_contains("All accounts").is_none());
    h.get_by_label("New Rule").click();
    h.run_steps(3);
    assert!(h.query_by_label("Add Rule").is_some());
}

/// A rule whose folder was deleted since opens on Edit Rule with that folder to be chosen
/// again, and Save waits for it, rather than be refused after it (#566).
#[test]
fn edit_rule_has_a_deleted_folder_chosen_again() {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("First", "first@example.org"), NOW, 0), Services::default());
    app.autosave = false;
    let account = app.session.mailbox.accounts[0].id;
    let archive = app.session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let rule = json!({"name": "Filed", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "moveTo", "value": archive}]});
    app.execute("rules.add", rule).unwrap();
    app.session.mailbox.folders.retain(|f| f.id != archive);
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
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Edit Rule").click();
    h.run_steps(4);
    let combos: Vec<Option<String>> = h.get_all_by_role(egui::accesskit::Role::ComboBox).map(|n| n.accesskit_node().value()).collect();
    assert!(combos.contains(&Some("Choose a folder".to_string())), "{combos:?}");
    assert!(h.get_by_label("Save").accesskit_node().is_disabled());
}
