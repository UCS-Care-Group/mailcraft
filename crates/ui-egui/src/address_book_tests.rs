// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People's Address Book (#1262, #1393), the host played by a stand-in that answers as told: the
//! whole list downloaded and narrowed here, or (where there is none) what is looked up and when,
//! what is shown, and what is made of it on the owner's say-so.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::{Module, Session};
use mailcraft_model::sync::{DirectoryKind, DirectoryPerson, EwsAddressList, EwsResolveResult, EwsResolved};
use mailcraft_model::{Id, Mailbox};
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// What the host was asked: the texts looked up (none for a download of the whole list), whether
/// it was the whole list, and the job's cancel.
type Asked = Arc<Mutex<Vec<(Vec<String>, bool, Arc<AtomicBool>)>>>;

fn person(name: &str, email: &str) -> DirectoryPerson {
    DirectoryPerson { name: name.into(), email: email.into(), kind: DirectoryKind::Mailbox, ..Default::default() }
}

/// The two Adas and a list the stand-in host has for "ada"; nobody for anything else.
fn answer(text: &str) -> Vec<EwsResolved> {
    let people = if text == "ada" {
        vec![
            DirectoryPerson {
                title: "Controller".into(),
                department: "Accounts".into(),
                company: "Example Ltd".into(),
                phone: "+1 555 0100".into(),
                mobile: "+1 555 0199".into(),
                office: "Building 2".into(),
                ..person("Ada Lovelace", "ada.lovelace@example.org")
            },
            person("Ada Byron", "ada.byron@example.org"),
            DirectoryPerson { kind: DirectoryKind::List, ..person("Ada's team", "ada.team@example.org") },
            // An address mail can't go to is never shown.
            person("Not one", "not-an-address"),
        ]
    } else {
        Vec::new()
    };
    vec![EwsResolved { text: text.into(), people, more: text == "ada" }]
}

/// A window with one Exchange server account whose password was typed, and a host that writes
/// down what it was asked and answers a look-up at once, or (`hold`) never; and a download of the
/// whole list at once, with `list`, or saying that the server publishes none.
fn window(hold: bool) -> (MailApp, Id, Asked) {
    window_with(hold, None)
}

fn window_with(hold: bool, list: Option<Vec<DirectoryPerson>>) -> (MailApp, Id, Asked) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let added = app
        .execute(
            "account.add",
            json!({"email": "me@example.org", "kind": "exchange", "ews": {"url": "https://exchange.example.org/EWS/Exchange.asmx"}}),
        )
        .unwrap();
    let account = added["id"].as_u64().unwrap();
    app.passwords.insert(account, "an invented password".into());
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    app.services.ews_resolve = Some(Box::new(move |job| {
        log.lock().unwrap().push((job.request.texts.clone(), job.request.address_list, job.cancel.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        if job.request.address_list {
            let result = match &list {
                Some(people) => EwsResolveResult {
                    address_list: Some(EwsAddressList { name: "Address list".into(), sequence: 3, people: people.clone(), more: false }),
                    ..Default::default()
                },
                None => EwsResolveResult {
                    error: Some("Couldn't download the organisation's address book: the server publishes none".into()),
                    ..Default::default()
                },
            };
            tx.send(result).unwrap();
        } else if hold {
            std::mem::forget(tx);
        } else {
            let text = job.request.texts.first().cloned().unwrap_or_default();
            tx.send(EwsResolveResult { account: job.request.account, found: answer(&text), ..Default::default() }).unwrap();
        }
        rx
    }));
    (app, account, asked)
}

/// `text` typed in People's search box, and the pause after it gone by.
fn search(app: &mut MailApp, ctx: &egui::Context, text: &str) {
    app.type_people_search(text);
    app.poll_address_book(ctx);
    if let Some(book) = &mut app.address_book {
        book.typed.1 -= PAUSE_MS * 2.0;
    }
    app.poll_address_book(ctx);
    app.poll_address_book(ctx);
}

/// The texts looked up, one entry a look-up.
fn texts(asked: &Asked) -> Vec<Vec<String>> {
    asked.lock().unwrap().iter().filter(|(_, whole, _)| !whole).map(|(texts, _, _)| texts.clone()).collect()
}

/// How many times the whole list was asked for.
fn downloads(asked: &Asked) -> usize {
    asked.lock().unwrap().iter().filter(|(_, whole, _)| *whole).count()
}

#[test]
fn what_is_typed_is_looked_up_once_after_a_pause_and_who_is_found_is_shown() {
    let (mut app, account, asked) = window(false);
    let ctx = egui::Context::default();
    assert_eq!(app.run("people.addressBook", json!({})).unwrap(), json!({"account": account}));
    assert_eq!(app.session.view.module, Module::People);
    assert_eq!(app.address_book_shown(), Some(account));
    // One character is not looked up, nor is anything before typing pauses.
    search(&mut app, &ctx, "a");
    app.type_people_search("ada");
    app.poll_address_book(&ctx);
    assert!(texts(&asked).is_empty());
    search(&mut app, &ctx, "ada");
    search(&mut app, &ctx, "ada");
    assert_eq!(texts(&asked), [vec!["ada".to_string()]]);
    let book = app.address_book.as_ref().unwrap();
    let names: Vec<&str> = book.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "Ada Byron", "Ada's team"]);
    assert!(book.more && book.selected.is_none() && book.error.is_none());
    assert_eq!(second_line(&book.people[0]), "Controller · Accounts");
    assert_eq!(second_line(&book.people[2]), "ada.team@example.org · Distribution list");
    // Nobody is kept: not in the mailbox, and not once People shows something else.
    assert!(app.session.mailbox.contacts.is_empty());
    app.run("view.mail", json!({})).unwrap();
    app.poll_address_book(&ctx);
    assert!(app.address_book.is_none());
}

#[test]
fn a_search_typed_further_lets_go_of_the_one_before() {
    let (mut app, _, asked) = window(true);
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    search(&mut app, &ctx, "ad");
    search(&mut app, &ctx, "ada");
    let asked: Vec<_> = asked.lock().unwrap().iter().filter(|(_, whole, _)| !whole).map(|(_, _, cancel)| cancel.clone()).collect();
    assert_eq!(asked.len(), 2);
    assert!(asked[0].load(Ordering::Relaxed), "the first look-up is told it isn't wanted");
    assert!(!asked[1].load(Ordering::Relaxed));
    assert!(app.address_book.as_ref().unwrap().searching());
}

#[test]
fn without_the_password_it_is_said_and_nothing_is_asked() {
    let (mut app, account, asked) = window(false);
    let ctx = egui::Context::default();
    app.passwords.remove(&account);
    app.run("people.addressBook", json!({"account": account})).unwrap();
    search(&mut app, &ctx, "ada");
    search(&mut app, &ctx, "ada");
    assert!(texts(&asked).is_empty());
    let error = app.address_book.as_ref().unwrap().error.clone().unwrap_or_default();
    assert!(error.contains("password"), "{error}");
}

#[test]
fn the_owner_writes_to_somebody_found_or_makes_a_contact_of_them() {
    let (mut app, account, _) = window(false);
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    search(&mut app, &ctx, "ada");
    let ada = app.address_book.as_ref().unwrap().people[0].clone();
    add_to_contacts(&mut app, &ada);
    let contact = app.session.mailbox.contacts.values().next().cloned().unwrap();
    assert_eq!((contact.display_name.as_str(), contact.company.as_str(), contact.title.as_str()), ("Ada Lovelace", "Example Ltd", "Controller"));
    let phones: Vec<&str> = contact.phones.iter().map(|p| p.value.as_str()).collect();
    assert_eq!(phones, ["+1 555 0100", "+1 555 0199"]);

    app.run("people.addressBook", json!({})).unwrap();
    search(&mut app, &ctx, "ada");
    new_email(&mut app, &ada);
    let compose = app.session.compose.as_ref().unwrap();
    assert_eq!((compose.to.as_str(), compose.account), ("Ada Lovelace <ada.lovelace@example.org>", account));
}

#[test]
fn people_shows_the_address_list_and_a_click_shows_the_card() {
    let (mut app, _, _) = window(false);
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    search(&mut app, &ctx, "ada");
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let row = h.state().hit.rect("addressListRow", 1).expect("the second row is drawn");
    h.event(egui::Event::PointerMoved(row.center()));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: row.center(), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert_eq!(h.state().address_book.as_ref().and_then(|b| b.chosen()).map(|p| p.name.clone()).as_deref(), Some("Ada Byron"));
    h.get_by_label("Email");
    // Back to the contacts, through the choice at the top of the list.
    h.state_mut().run("view.peopleAccount", json!({"value": "all"})).unwrap();
    h.run_steps(3);
    assert!(h.state().address_book.is_none());
}

/// What a script or an agent puts in the search box is searched for among the contacts, and not
/// looked up in the address list: only what the owner types is.
#[test]
fn a_search_an_agent_puts_in_the_box_is_not_looked_up() {
    let (mut app, _, asked) = window(false);
    let ctx = egui::Context::default();
    app.execute("people.addressBook", json!({})).unwrap();
    app.execute("people.search", json!({"query": "ada"})).unwrap();
    for _ in 0..3 {
        app.poll_address_book(&ctx);
        if let Some(book) = &mut app.address_book {
            book.typed.1 -= PAUSE_MS * 2.0;
        }
    }
    assert!(texts(&asked).is_empty());
    assert!(app.address_book_found().is_empty());
    // The owner types the same: now it is.
    search(&mut app, &ctx, "ada");
    assert_eq!(texts(&asked), [vec!["ada".to_string()]]);
}

/// The organisation of the whole list: two Adas, a list, and Grace.
fn organisation() -> Vec<DirectoryPerson> {
    vec![
        DirectoryPerson { department: "Accounts".into(), ..person("Ada Lovelace", "ada.lovelace@example.org") },
        DirectoryPerson { department: "Engines".into(), ..person("Ada Byron", "ada.byron@example.org") },
        DirectoryPerson { kind: DirectoryKind::List, ..person("All Staff", "staff@example.org") },
        person("Grace Hopper", "grace@example.org"),
        // An address mail can't go to is never shown.
        person("Not one", "not-an-address"),
    ]
}

fn names(app: &MailApp) -> Vec<String> {
    app.address_book_found().iter().map(|p| p.name.clone()).collect()
}

/// The whole list is downloaded once, everybody in it listed, and what is typed narrows it here:
/// no name is looked up on the server.
#[test]
fn the_whole_list_is_listed_and_narrowed_without_asking_the_server() {
    let (mut app, account, asked) = window_with(false, Some(organisation()));
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    app.poll_address_book(&ctx);
    assert!(app.downloading_address_list());
    app.poll_address_book(&ctx);
    assert!(!app.downloading_address_list());
    assert_eq!(app.address_list(account).map(|l| l.people.len()), Some(4));
    assert_eq!(names(&app), ["Ada Lovelace", "Ada Byron", "All Staff", "Grace Hopper"]);
    search(&mut app, &ctx, "ada");
    assert_eq!(names(&app), ["Ada Lovelace", "Ada Byron"]);
    // Every word, in the name, the address, the department and the rest.
    search(&mut app, &ctx, "ADA engines");
    assert_eq!(names(&app), ["Ada Byron"]);
    assert_eq!(app.address_book.as_ref().unwrap().selected, Some(0), "one found is shown at once");
    search(&mut app, &ctx, "hopper");
    assert_eq!(names(&app), ["Grace Hopper"]);
    search(&mut app, &ctx, "nobody");
    assert!(names(&app).is_empty());
    assert!(texts(&asked).is_empty(), "nothing typed went to the server");
    assert_eq!(downloads(&asked), 1);
    // Away and back: the list held is shown again, and not downloaded again.
    app.run("view.mail", json!({})).unwrap();
    app.poll_address_book(&ctx);
    app.type_people_search("");
    app.run("people.addressBook", json!({})).unwrap();
    app.poll_address_book(&ctx);
    assert_eq!(names(&app).len(), 4);
    assert_eq!(downloads(&asked), 1);
    // Nobody is kept in the mailbox.
    assert!(app.session.mailbox.contacts.is_empty());
}

/// A day later it is downloaded again; a download that failed is tried again a minute later.
#[test]
fn the_list_is_downloaded_again_a_day_later() {
    let (mut app, _, asked) = window_with(false, Some(organisation()));
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    app.poll_address_book(&ctx);
    app.poll_address_book(&ctx);
    app.address_lists[0].at_ms -= REFRESH_MS - 1000.0;
    app.poll_address_book(&ctx);
    assert_eq!(downloads(&asked), 1);
    app.address_lists[0].at_ms -= 2000.0;
    app.poll_address_book(&ctx);
    assert_eq!(downloads(&asked), 2);
    // Meanwhile the list held is still listed.
    assert_eq!(names(&app).len(), 4);

    let (mut app, _, asked) = window(false);
    app.run("people.addressBook", json!({})).unwrap();
    app.poll_address_book(&ctx);
    app.poll_address_book(&ctx);
    assert!(app.address_lists[0].error.as_deref().is_some_and(|e| e.contains("publishes none")));
    app.poll_address_book(&ctx);
    assert_eq!(downloads(&asked), 1);
    app.address_lists[0].at_ms -= RETRY_MS;
    app.poll_address_book(&ctx);
    assert_eq!(downloads(&asked), 2);
}

/// Thousands of people: only the rows on screen are drawn, and a row far down is reached.
#[test]
fn a_long_list_draws_only_what_is_on_screen() {
    let many: Vec<DirectoryPerson> = (0..5000).map(|i| person(&format!("Person {i:04}"), &format!("p{i}@example.org"))).collect();
    let (mut app, _, _) = window_with(false, Some(many));
    let ctx = egui::Context::default();
    app.run("people.addressBook", json!({})).unwrap();
    app.poll_address_book(&ctx);
    app.poll_address_book(&ctx);
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(h.state().hit.rect("addressListRow", 0).is_some());
    assert!(h.state().hit.rect("addressListRow", 4000).is_none(), "a row far down is not drawn");
    assert_eq!(h.state().address_book_found().len(), 5000);
}
