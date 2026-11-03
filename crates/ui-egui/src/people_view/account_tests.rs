// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each account's contacts apart in People (#631).

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use serde_json::json;

use super::*;
use crate::pick_item::{PickKind, rows};

fn add(w: &mut Harness<'_, MailApp>, email: &str) -> Id {
    let params = json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"});
    w.state_mut().execute("account.add", params).unwrap()["id"].as_u64().unwrap()
}

fn from_server(w: &mut Harness<'_, MailApp>, account: Id, email: &str, name: &str) -> Id {
    let contact = RemoteContact { id: "w1".into(), display_name: name.into(), ..Default::default() };
    let download = ContactsDownload { account, email: email.into(), complete: true, contacts: vec![contact] };
    mailcraft_engine::cmd::remote::apply_contacts(&mut w.state_mut().session, &download).unwrap();
    w.state().session.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().id
}

fn show(w: &mut Harness<'_, MailApp>, value: serde_json::Value) {
    w.state_mut().execute("view.peopleAccount", json!({"value": value})).unwrap();
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.run();
}

#[test]
fn the_list_and_its_counts_follow_the_choice() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    let everyone = w.state().session.mailbox.contacts.len();
    let ctx = w.state().ctx.clone().unwrap();
    assert_eq!(shown_total(w.state(), &ctx), everyone);
    from_server(&mut w, work, "work@example.org", "Wren Work");
    assert_eq!(shown_total(w.state(), &ctx), everyone + 1);

    show(&mut w, json!(work));
    let one = list_layout(w.state(), &ctx);
    assert_eq!((one.total, listed(&one.layout)), (1, 1));
    show(&mut w, json!("here"));
    assert_eq!(shown_total(w.state(), &ctx), everyone);
    // A search counts what it finds of the account's, not of everybody.
    show(&mut w, json!(work));
    w.state_mut().execute("people.search", json!({"query": "wren"})).unwrap();
    let found = list_layout(w.state(), &ctx);
    assert_eq!((found.total, listed(&found.layout)), (1, 1));
    w.state_mut().execute("people.search", json!({"query": "nobody at all"})).unwrap();
    let none = list_layout(w.state(), &ctx);
    assert_eq!((none.total, listed(&none.layout)), (1, 0));
    // Every way of showing them paints.
    w.run();
    w.state_mut().execute("people.search", json!({"query": ""})).unwrap();
    show(&mut w, json!("all"));
}

#[test]
fn with_every_account_each_contact_says_where_it_is() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    let theirs = from_server(&mut w, work, "work@example.org", "Wren Work");
    let ours = w.state().session.mailbox.contacts.values().find(|c| c.origin.is_local()).unwrap().id;
    show(&mut w, json!("all"));
    assert!(labelled(w.state()));
    show(&mut w, json!(work));
    assert!(!labelled(w.state()), "one account's list needs no label");
    show(&mut w, json!("here"));
    assert!(!labelled(w.state()));
    // The card says where it is, whatever the list shows.
    let kept = |w: &Harness<'_, MailApp>, id: Id| kept_in(w.state(), w.state().session.mailbox.contact(id).unwrap());
    assert_eq!(kept(&w, ours).as_deref(), Some("On this computer"));
    assert_eq!(kept(&w, theirs).as_deref(), Some("work@example.org"));
}

#[test]
fn a_contact_the_server_keeps_says_it_is_read_only_here() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    let theirs = from_server(&mut w, work, "work@example.org", "Wren Work");
    let ours = w.state().session.mailbox.contacts.values().find(|c| c.origin.is_local()).unwrap().id;
    let read_only = |w: &Harness<'_, MailApp>, id: Id| read_only_here(w.state(), w.state().session.mailbox.contact(id).unwrap());
    assert!(read_only(&w, theirs));
    assert!(!read_only(&w, ours));
    // What the mark says is what Edit is told.
    assert!(w.state_mut().execute("people.edit", json!({"id": theirs, "first": "Ren"})).is_err());
    assert!(w.state_mut().execute("people.edit", json!({"id": ours, "notes": "Changed here"})).is_ok());
    // The card with the mark draws.
    w.state_mut().execute("people.select", json!({"id": theirs})).unwrap();
    w.run();
}

#[test]
fn without_an_account_that_keeps_contacts_nothing_changes() {
    let mut w = testing::window("view.people");
    let ours = people::sorted(&w.state().session, "")[0];
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("people.select", json!({"id": ours})).unwrap();
    w.run();
    assert_eq!(w.query_all_by_role(Role::ComboBox).count(), 0);
    assert!(!labelled(w.state()));
    assert_eq!(kept_in(w.state(), w.state().session.mailbox.contact(ours).unwrap()), None);
}

#[test]
fn the_choice_is_at_the_top_of_the_list() {
    let mut w = testing::window("view.people");
    add(&mut w, "work@example.org");
    show(&mut w, json!("all"));
    assert_eq!(w.query_all_by_role(Role::ComboBox).count(), 1);
}

#[test]
fn an_account_without_contacts_says_so_and_offers_nothing_it_cant_do() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    show(&mut w, json!("all"));
    let buttons = w.query_all_by_label("New Contact").count();
    show(&mut w, json!(work));
    assert_eq!(w.query_all_by_label("No contacts from this account yet").count(), 1);
    assert_eq!(w.query_all_by_label("No contacts yet").count(), 0);
    assert_eq!(w.query_all_by_label("Select a contact to see the details").count(), 0);
    assert_eq!(w.query_all_by_label("New Contact").count(), buttons, "the ribbon's stays, and the list adds none");
    // Nobody on this computer: one can be made here, and the list says so as ever.
    let everybody: Vec<Id> = w.state().session.mailbox.contacts.keys().copied().collect();
    for id in everybody {
        w.state_mut().session.mailbox.remove_contact(id).unwrap();
    }
    show(&mut w, json!("here"));
    assert_eq!(w.query_all_by_label("No contacts yet").count(), 1);
    assert_eq!(w.query_all_by_label("New Contact").count(), buttons + 1);
}

#[test]
fn what_is_picked_to_merge_is_what_the_list_shows() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    let theirs = from_server(&mut w, work, "work@example.org", "Wren Work");
    let ours = people::sorted(&w.state().session, "")[0];
    w.state_mut().execute("people.select", json!({"id": theirs})).unwrap();
    w.state_mut().people_picked = vec![ours, theirs];
    show(&mut w, json!(work));
    assert_eq!(w.state().session.sel.contact, Some(theirs));
    assert!(w.state().people_picked.is_empty(), "one left is no pick");
}

#[test]
fn the_choice_is_remembered_with_the_other_view_choices() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    show(&mut w, json!(work));
    let saved = serde_json::to_value(w.state().prefs()).unwrap();
    assert_eq!(saved["view"]["peopleAccount"], json!({"account": work}));
    w.state_mut().execute("view.peopleAccount", json!({"value": "all"})).unwrap();
    w.state_mut().apply_prefs(serde_json::from_value(saved.clone()).unwrap());
    assert_eq!(people::shown_account(&w.state().session), AccountChoice::Account(work));
    // A file from before it was kept starts on every account.
    let mut older = saved;
    older["view"].as_object_mut().unwrap().remove("peopleAccount");
    w.state_mut().apply_prefs(serde_json::from_value(older).unwrap());
    assert_eq!(w.state().session.view.people_account, AccountChoice::All);
}

#[test]
fn one_accounts_contacts_are_remembered_for_that_account_only() {
    let app = |email: &str| {
        let now = 1_791_993_600;
        let mut a = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0), crate::Services::default());
        let params = json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"});
        let id = a.execute("account.add", params).unwrap()["id"].as_u64().unwrap();
        (a, id)
    };
    let (mut a, work) = app("work@example.org");
    a.execute("view.peopleAccount", json!({"value": work})).unwrap();
    let saved = serde_json::to_string(&a.prefs()).unwrap();
    // The same mailbox next time: that account's contacts.
    let (mut same, _) = app("work@example.org");
    same.apply_prefs(serde_json::from_str(&saved).unwrap());
    assert_eq!(same.session.view.people_account, AccountChoice::Account(work));
    // Another, where the same id is somebody else's account: every account's.
    let (mut other, theirs) = app("someone@example.org");
    assert_eq!(theirs, work);
    other.apply_prefs(serde_json::from_str(&saved).unwrap());
    assert_eq!(other.session.view.people_account, AccountChoice::All);
}

#[test]
fn the_contact_picker_lists_every_account_whatever_people_shows() {
    let mut w = testing::window("view.people");
    let work = add(&mut w, "work@example.org");
    let id = from_server(&mut w, work, "work@example.org", "Wren Work");
    let every = rows(w.state(), PickKind::Contact, "").len();
    show(&mut w, json!("here"));
    assert_eq!(rows(w.state(), PickKind::Contact, "").len(), every);
    assert!(rows(w.state(), PickKind::Contact, "wren").iter().any(|row| row.0 == id));
}
