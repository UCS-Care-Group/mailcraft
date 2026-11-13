// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each Microsoft account's own list of categories (#633) in the window: what Categorize offers
//! and whose list the Categories dialog shows and changes.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT as _, Queryable};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::CategoriesDownload;
use mailcraft_model::{Category, Event, FolderKind, Id, Mailbox, Message, Rgb};
use serde_json::json;

use super::Form;
use super::categories::CategoriesForm;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const RED: Rgb = Rgb(200, 0, 0);

/// A mailbox with this computer and two Microsoft accounts: `a` has Client X, `b` has Other, both
/// have Red, and the shared list has nothing to bring across.
fn rig() -> (MailApp, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut add = |email: &str| s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (a, b) = (add("a@example.org"), add("b@example.org"));
    s.mailbox.categories.clear();
    for (account, email, extra) in [(a, "a@example.org", "Client X"), (b, "b@example.org", "Other")] {
        let tag = |name: &str| Category { name: name.into(), color: RED };
        let download = CategoriesDownload { account, email: email.into(), categories: vec![tag("Red"), tag(extra)], complete: true };
        remote::apply_categories(&mut s, &download).unwrap();
        // Its sign-in lets MailCraft change the list there (#661).
        s.run("sync.categoryWrites", &json!({"account": account, "allowed": true})).unwrap();
    }
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    (app, a, b)
}

fn message_of(app: &mut MailApp, account: Id) -> Id {
    let folder = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    app.session.mailbox.add_message(Message { folder, ..Default::default() }).unwrap()
}

/// The Categorize menu for `accounts`, drawn on its own.
fn menu(app: MailApp, accounts: Vec<Id>) -> Harness<'static, MailApp> {
    menu_carrying(app, accounts, Vec::new())
}

/// The Categorize menu for `accounts`, for items that have `carried`.
fn menu_carrying(app: MailApp, accounts: Vec<Id>, carried: Vec<String>) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(400.0, 500.0)).build_ui_state(
        move |ui, app: &mut MailApp| crate::ribbon::category_menu(ui, app, &accounts, &carried, "mail.categorize", |_, _| false, None),
        app,
    );
    h.run_steps(2);
    h
}

fn dialog(mut app: MailApp, params: serde_json::Value) -> Harness<'static, MailApp> {
    let mut params = params;
    params["name"] = json!("categories");
    app.execute("ui.dialog", params).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        move |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

/// What a row kept on this computer alone says, on a list kept in step with Outlook (#661).
const REMARK: &str = "Kept on this computer only";

#[test]
fn an_account_that_isnt_one_opens_no_list() {
    let (mut app, a, _) = rig();
    for junk in [json!(a.to_string()), json!("\u{0}nonsense"), json!(-1), json!(true), json!([a]), json!(u64::MAX)] {
        assert!(app.execute("ui.dialog", json!({"name": "categories", "account": junk})).is_err(), "{junk}");
    }
    assert!(app.execute("ui.dialog", json!({"name": "categories", "account": null})).is_ok());
}

#[test]
fn categorize_offers_the_list_of_the_items_account() {
    let (app, a, b) = rig();
    let h = menu(app, vec![a]);
    assert!(h.query_by_label("Client X").is_some() && h.query_by_label("Red").is_some());
    assert!(h.query_by_label("Other").is_none(), "B's category isn't offered on A's item");
    drop(h);

    // With items of both, what both have.
    let (app, a, _) = rig();
    let h = menu(app, vec![a, b]);
    assert!(h.query_by_label("Red").is_some());
    assert!(h.query_by_label("Client X").is_none() && h.query_by_label("Other").is_none());
    drop(h);

    // An item of this computer: the shared list, which the servers' lists didn't reach.
    let (mut app, ..) = rig();
    app.session.mailbox.set_category(0, "Here", RED).unwrap();
    let h = menu(app, vec![0]);
    assert!(h.query_by_label("Here").is_some());
    assert!(h.query_by_label("Client X").is_none());
}

#[test]
fn an_events_form_offers_its_calendars_list() {
    let (mut app, a, _) = rig();
    let calendar = app.session.mailbox.alloc_id();
    let origin = mailcraft_model::Origin { account: a, id: "C-1".into(), ..Default::default() };
    app.session.mailbox.calendars.push(mailcraft_model::Calendar { id: calendar, name: "Team".into(), origin, ..Default::default() });
    let event = Event { calendar, title: "Review".into(), ..Default::default() };
    assert_eq!(app.session.mailbox.event_account(&event), a);
    let id = app.session.mailbox.add_event(event).unwrap();
    let Some(Form::Event(f)) = Form::open("editEvent", &app, &json!({"id": id})) else { panic!("no form") };
    assert_eq!(f.category_account(&app.session), a);
    // A new one goes into a calendar of this computer here.
    let Some(Form::Event(f)) = Form::open("event", &app, &json!({})) else { panic!("no form") };
    assert_eq!(f.category_account(&app.session), 0);
}

#[test]
fn the_dialog_opens_on_the_list_of_what_it_was_opened_for() {
    let (mut app, a, b) = rig();
    let account = |app: &MailApp, params: serde_json::Value| match Form::open("categories", app, &params) {
        Some(Form::Categories(f)) => f.add_command().1.get("account").and_then(serde_json::Value::as_u64),
        _ => panic!("no dialog"),
    };
    assert_eq!(account(&app, json!({"account": b})), Some(b));
    // A selected message's account, and the shared list when that is what was asked for.
    let m = message_of(&mut app, a);
    app.session.sel.messages = vec![m];
    assert_eq!(account(&app, json!({})), Some(a));
    assert_eq!(account(&app, json!({"account": null})), Some(a));
    assert_eq!(account(&app, json!({"account": 0})), None);
}

#[test]
fn what_the_dialog_does_goes_to_the_list_in_view() {
    let (mut app, a, b) = rig();
    let mut f = CategoriesForm::new(&app.session, &json!({"account": b})).unwrap();
    f.new_name = "Travel".into();
    let (command, params) = f.add_command();
    app.execute(command, params).unwrap();
    assert!(app.session.mailbox.category_of(b, "Travel").is_some());
    assert!(app.session.mailbox.category_of(a, "Travel").is_none() && app.session.mailbox.category("Travel").is_none());
}

#[test]
fn the_dialog_says_whose_list_it_shows_and_only_what_isnt_on_that_server() {
    let (mut app, a, _) = rig();
    // One an older build kept on A's list and the server never had.
    let here = Category { name: "Made here".into(), color: RED };
    app.session.mailbox.account_categories.entry(a).or_default().push(here);
    app.session.mailbox.local_categories.entry(a).or_default().push("Made here".into());
    let h = dialog(app, json!({"account": a}));
    assert!(h.query_by_label("Account").is_some());
    assert_eq!(h.query_all_by_label(REMARK).count(), 1, "Red and Client X are on A's server, Made here isn't");
    assert_eq!(h.query_all_by_label("Add to Outlook").count(), 1);
    drop(h);

    // Without leave to change the list there, it says so and offers nothing that would change it.
    let (mut app, a, _) = rig();
    app.session.mailbox.account_categories.entry(a).or_default().push(Category { name: "Made here".into(), color: RED });
    app.session.mailbox.local_categories.entry(a).or_default().push("Made here".into());
    app.services.graph_categories = Some(Box::new(|_| std::sync::mpsc::channel().1));
    app.stand_in_categories(false);
    let h = dialog(app, json!({"account": a}));
    assert!(h.query_by_label_contains("No permission to change them").is_some());
    assert!(h.query_by_label("Allow").is_some());
    assert_eq!(h.query_all_by_label("Add to Outlook").count(), 0);
    drop(h);

    let (app, _, b) = rig();
    let h = dialog(app, json!({"account": b}));
    assert_eq!(h.query_all_by_label(REMARK).count(), 0);
    drop(h);

    // The shared list is no server's: nothing to say.
    let (app, ..) = rig();
    let h = dialog(app, json!({"account": 0}));
    assert!(h.query_by_label("Account").is_some());
    assert_eq!(h.query_all_by_label(REMARK).count(), 0);
}

#[test]
fn with_no_account_of_its_own_the_dialog_is_as_it_was() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.run("account.add", &json!({"email": "lin@example.org"})).unwrap();
    let app = MailApp::new(s, Services::default());
    let h = dialog(app, json!({}));
    assert!(h.query_by_label("Account").is_none());
    assert!(h.query_by_label(REMARK).is_none());
}

#[test]
fn categorize_shows_what_the_items_carry_off_the_list_and_takes_it_off() {
    let (mut app, a, _) = rig();
    let m = message_of(&mut app, a);
    app.session.mailbox.message_mut(m).unwrap().categories = vec!["Came with it".into(), "red".into()];
    app.session.sel.messages = vec![m];
    let carried = crate::ribbon::selected_categories(&app);
    let mut h = menu_carrying(app, vec![a], carried);
    // One of each, the list's own spelling for what is on it.
    assert_eq!(h.query_all_by_label("Came with it").count(), 1);
    assert_eq!(h.query_all_by_label("Red").count(), 1);
    assert_eq!(h.query_all_by_label("red").count(), 0);
    h.get_by_label("Came with it").click();
    h.run_steps(2);
    let app = h.state();
    assert_eq!(app.session.mailbox.message(m).unwrap().categories, ["red"]);
    assert!(app.session.mailbox.category_of(a, "Came with it").is_none(), "taking it off doesn't list it");
}

#[test]
fn a_list_that_goes_leaves_the_dialog_on_the_shared_list() {
    let (mut app, a, _) = rig();
    let mut form = CategoriesForm::new(&app.session, &json!({"account": a})).unwrap();
    form.new_name = "Meant for that list".into();
    app.session.run("account.remove", &json!({"id": a})).unwrap();
    let shared = CategoriesForm::new(&app.session, &json!({"account": 0})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(600.0, 600.0)).build_ui_state(
        |ui, (app, form): &mut (MailApp, CategoriesForm)| {
            form.show(app, ui);
        },
        (app, form),
    );
    h.run_steps(2);
    // What it adds goes there, in a colour free there.
    assert_eq!(h.state().1.add_command(), shared.add_command());
}

#[test]
fn an_events_window_offers_what_the_event_has_off_its_list() {
    let (mut app, a, _) = rig();
    let calendar = app.session.mailbox.alloc_id();
    let origin = mailcraft_model::Origin { account: a, id: "C-1".into(), ..Default::default() };
    app.session.mailbox.calendars.push(mailcraft_model::Calendar { id: calendar, name: "Team".into(), origin, ..Default::default() });
    let event = Event { calendar, title: "Review".into(), categories: vec!["Came with it".into(), "red".into()], ..Default::default() };
    let id = app.session.mailbox.add_event(event).unwrap();
    let Some(Form::Event(f)) = Form::open("editEvent", &app, &json!({"id": id})) else { panic!("no form") };
    let offered: Vec<String> = f.offered_categories(&app.session).into_iter().map(|(name, _)| name).collect();
    assert_eq!(offered, ["Red", "Client X", "Came with it"]);
}

#[test]
fn a_read_only_list_greys_out_its_fields_and_keeps_allow() {
    for allowed in [true, false] {
        let (mut app, a, _) = rig();
        app.services.graph_categories = Some(Box::new(|_| std::sync::mpsc::channel().1));
        app.stand_in_categories(allowed);
        let h = dialog(app, json!({"account": a}));
        for name in ["Red", "Client X"] {
            let field = h.get(By::new().role(Role::TextInput).value(name));
            assert_eq!(field.accesskit_node().is_disabled(), !allowed, "{name}, allowed: {allowed}");
        }
        if !allowed {
            assert!(!h.get_by_label("Allow").accesskit_node().is_disabled());
        }
    }
}

#[test]
fn an_item_carrying_a_name_of_the_list_in_other_letters_shows_one_chip() {
    let list = [Category { name: "Équipe".into(), color: RED }, Category { name: "Red".into(), color: RED }];
    let names = |chosen: &[&str]| -> Vec<String> {
        let chosen: Vec<String> = chosen.iter().map(|c| c.to_string()).collect();
        super::kit::chips_of(&list, &chosen).into_iter().map(|(name, _)| name).collect()
    };
    assert_eq!(names(&["équipe", "RED"]), ["Équipe", "Red"]);
    assert_eq!(names(&["Came with it"]), ["Équipe", "Red", "Came with it"]);
    // A click on its chip takes the carried spelling off, and a second puts the list's on.
    let mut chosen = vec!["équipe".to_string(), "RED".to_string()];
    super::kit::toggle_chip(&mut chosen, "Équipe");
    assert_eq!(chosen, ["RED"]);
    super::kit::toggle_chip(&mut chosen, "Équipe");
    assert_eq!(chosen, ["RED", "Équipe"]);
}

#[test]
fn allow_in_the_window_puts_the_question_and_waits_for_the_answer() {
    let (mut app, a, _) = rig();
    app.services.graph_categories = Some(Box::new(|_| std::sync::mpsc::channel().1));
    app.stand_in_categories(false);
    let mut h = dialog(app, json!({"account": a}));
    h.get_by_label("Allow").click();
    h.run_steps(3);
    // The question is on screen, over everything, and nothing is taken for a no meanwhile.
    let asked = matches!(&h.state().dialog, Some(crate::dialogs::Dialog::Confirm { command, .. }) if command == "ui.allowSync");
    assert!(asked, "the owner is asked");
    assert!(!h.state().sync_declined.contains(&a));
}

/// A held Space or Enter on a category's Delete button deletes that category alone: its repeats
/// don't reach the button that takes its place, the next category's.
#[test]
fn a_held_key_deletes_one_category() {
    for held in [egui::Key::Space, egui::Key::Enter] {
        let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        let before: Vec<String> = app.session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
        assert!(before.len() >= 3);
        let mut h = dialog(app, json!({}));
        h.get_all_by_label("Delete this category").next().unwrap().focus();
        h.run_steps(2);
        let key = |pressed, repeat| egui::Event::Key { key: held, physical_key: None, pressed, repeat, modifiers: egui::Modifiers::NONE };
        h.event(key(true, false));
        h.step();
        for _ in 0..6 {
            h.event(key(true, true));
            h.step();
        }
        h.event(key(false, false));
        h.run_steps(3);
        let names: Vec<String> = h.state().session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, before[1..], "{held:?}");
    }
}

/// A list kept in step with Outlook offers Outlook's 25 colours, and a new category takes the
/// first of them that no category of the list has (#661); the list the rest share keeps the
/// program's own colours.
#[test]
fn a_list_kept_in_step_with_outlook_offers_outlooks_colours() {
    use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let taken = OUTLOOK_COLORS[..12].iter().enumerate().map(|(i, c)| Category { name: format!("C{i}"), color: *c }).collect();
    remote::apply_categories(&mut s, &CategoriesDownload { account: a, email: "a@example.org".into(), categories: taken, complete: true }).unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert_eq!(super::categories::palette_of(&s.mailbox, a), &OUTLOOK_COLORS[..]);
    let mut f = CategoriesForm::new(&s, &json!({"account": a})).unwrap();
    f.new_name = "Travel".into();
    let (_, params) = f.add_command();
    assert_eq!(params["color"], json!(OUTLOOK_COLORS[12].hex()), "{params}");
    assert_eq!(super::categories::palette_of(&s.mailbox, 0), &super::categories::PALETTE[..]);
}

/// A list that comes to be kept in step with Outlook while the dialog is open (Outlook's list is
/// read for the first time meanwhile) gives a new category one of Outlook's colours, so Add still
/// adds it (#661).
#[test]
fn a_list_that_comes_in_step_with_outlook_while_the_dialog_is_open_adds_in_outlooks_colours() {
    use mailcraft_model::outlook_colors::outlook_preset;
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    // Its own list from before Outlook's was read (#633), in seven of the program's colours, so
    // the one a new category is offered is not one of Outlook's.
    let own: Vec<Category> = mailcraft_model::PALETTE[..7].iter().enumerate().map(|(i, c)| Category { name: format!("C{i}"), color: *c }).collect();
    s.mailbox.account_categories.insert(a, own.clone());
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    let form = CategoriesForm::new(&app.session, &json!({"account": a})).unwrap();
    let offered = Rgb::parse(form.add_command().1["color"].as_str().unwrap()).unwrap();
    assert!(outlook_preset(offered).is_none(), "{offered:?}");
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(600.0, 600.0)).build_ui_state(
        |ui, (app, form): &mut (MailApp, CategoriesForm)| {
            form.show(app, ui);
        },
        (app, form),
    );
    h.run_steps(2);
    {
        let (app, _) = h.state_mut();
        let download = CategoriesDownload { account: a, email: "a@example.org".into(), categories: own, complete: true };
        remote::apply_categories(&mut app.session, &download).unwrap();
        app.session.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
        assert!(app.session.mailbox.synced_list_owner(a).is_some());
    }
    h.run_steps(2);
    let (app, form) = h.state_mut();
    form.new_name = "Travel".into();
    let (command, params) = form.add_command();
    app.execute(command, params).unwrap();
    assert!(app.session.mailbox.category_of(a, "Travel").is_some());
}
