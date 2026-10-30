// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each Microsoft account's own list of categories (#633) in the window: what Categorize offers
//! and whose list the Categories dialog shows and changes.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

/// A mailbox with this computer and two Microsoft accounts: `a` has Client X, `b` has Other, and
/// both have Red.
fn rig() -> (MailApp, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut add = |email: &str| s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (a, b) = (add("a@example.org"), add("b@example.org"));
    for (account, email, extra) in [(a, "a@example.org", "Client X"), (b, "b@example.org", "Other")] {
        let tag = |name: &str| Category { name: name.into(), color: RED };
        let download = CategoriesDownload { account, email: email.into(), categories: vec![tag("Red"), tag(extra)] };
        remote::apply_categories(&mut s, &download).unwrap();
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
    let mut h = Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(400.0, 500.0))
        .build_ui_state(move |ui, app: &mut MailApp| crate::ribbon::category_menu(ui, app, &accounts, "mail.categorize", |_, _| false, None), app);
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

const REMARK: &str = "Not in Outlook's list: no colour there";

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
    let (app, ..) = rig();
    let h = menu(app, vec![0]);
    assert!(h.query_by_label("Red category").is_some());
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
    // A selected message's account; a shared mailbox of no list, or junk, the shared list.
    let m = message_of(&mut app, a);
    app.session.sel.messages = vec![m];
    assert_eq!(account(&app, json!({})), Some(a));
    assert_eq!(account(&app, json!({"account": 0})), None);
    assert_eq!(account(&app, json!({"account": u64::MAX})), None);
    assert_eq!(account(&app, json!({"account": "\u{0}nonsense"})), Some(a));
}

#[test]
fn what_the_dialog_does_goes_to_the_list_in_view() {
    let (mut app, a, b) = rig();
    let mut f = CategoriesForm::new(&app.session, &json!({"account": b}));
    f.new_name = "Travel".into();
    let (command, params) = f.add_command();
    app.execute(command, params).unwrap();
    assert!(app.session.mailbox.category_of(b, "Travel").is_some());
    assert!(app.session.mailbox.category_of(a, "Travel").is_none() && app.session.mailbox.category("Travel").is_none());
}

#[test]
fn the_dialog_says_whose_list_it_shows_and_only_what_isnt_on_that_server() {
    let (mut app, a, b) = rig();
    app.session.run("category.set", &json!({"account": a, "name": "Made here", "color": "#112233"})).unwrap();
    let h = dialog(app, json!({"account": a}));
    assert!(h.query_by_label("Account").is_some());
    assert_eq!(h.query_all_by_label(REMARK).count(), 1, "Red and Client X are on A's server, Made here isn't");
    drop(h);

    let (app, ..) = rig();
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
