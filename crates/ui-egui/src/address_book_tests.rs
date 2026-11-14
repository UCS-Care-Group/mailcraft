// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People's Address Book (#1262), the host played by a stand-in that answers as told: what is
//! looked up and when, what is shown, and what is made of it on the owner's say-so.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::{Module, Session};
use mailcraft_model::sync::{DirectoryKind, DirectoryPerson, EwsResolveResult, EwsResolved};
use mailcraft_model::{Id, Mailbox};
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Asked = Arc<Mutex<Vec<(Vec<String>, Arc<AtomicBool>)>>>;

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
/// down what it was asked and answers at once, or (`hold`) never.
fn window(hold: bool) -> (MailApp, Id, Asked) {
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
        log.lock().unwrap().push((job.request.texts.clone(), job.cancel.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        if hold {
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

fn texts(asked: &Asked) -> Vec<Vec<String>> {
    asked.lock().unwrap().iter().map(|(texts, _)| texts.clone()).collect()
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
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert!(asked[0].1.load(Ordering::Relaxed), "the first look-up is told it isn't wanted");
    assert!(!asked[1].1.load(Ordering::Relaxed));
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
