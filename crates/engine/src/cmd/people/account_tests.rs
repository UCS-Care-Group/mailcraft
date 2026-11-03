// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each account's contacts apart (#631).

use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::{Account, AccountChoice, AccountKind, Id, Labeled};
use serde_json::{Value, json};

use super::{contact_accounts, shown_account, sorted, sorted_of};
use crate::cmd::remote::merge_contacts;
use crate::{Module, Session};

const NOW: i64 = 1_790_000_000;

fn add(s: &mut Session, email: &str) -> Id {
    s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"].as_u64().unwrap()
}

fn imap(s: &mut Session) -> Id {
    run(s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap()
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// What `account`'s server has: a contact named `name`, known to it as `id`.
fn from_server(s: &mut Session, account: Id, id: &str, name: &str) -> Id {
    let email = s.mailbox.account(account).unwrap().email.clone();
    let contact = RemoteContact {
        id: id.into(),
        display_name: name.into(),
        emails: vec![Labeled::new("work", format!("{id}@example.org"))],
        ..Default::default()
    };
    merge_contacts(&mut s.mailbox, &ContactsDownload { account, email, complete: true, contacts: vec![contact] }).unwrap();
    s.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().id
}

/// The sample, whose contacts are on this computer, with two Microsoft accounts of one contact each.
fn session() -> (Session, Id, Id, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let (work, home) = (add(&mut s, "work@example.org"), add(&mut s, "home@example.org"));
    let (at_work, at_home) = (from_server(&mut s, work, "w1", "Wren Work"), from_server(&mut s, home, "h1", "Hollis Home"));
    (s, work, home, at_work, at_home)
}

fn here(s: &Session) -> usize {
    s.mailbox.contacts.values().filter(|c| c.origin.is_local()).count()
}

#[test]
fn the_list_shows_one_accounts_contacts_or_those_on_this_computer_or_all() {
    let (mut s, work, home, at_work, at_home) = session();
    let local = here(&s);
    assert_eq!(contact_accounts(&s.mailbox), [work, home]);
    assert_eq!((shown_account(&s), sorted(&s, "").len()), (AccountChoice::All, local + 2));

    run(&mut s, "view.peopleAccount", json!({"value": work}));
    assert_eq!(sorted(&s, ""), [at_work]);
    run(&mut s, "view.peopleAccount", json!({"value": home}));
    assert_eq!(sorted(&s, ""), [at_home]);
    assert_eq!(run(&mut s, "people.search", json!({"query": "Wren"}))["found"], 0, "the search looks in what the list shows");
    run(&mut s, "people.search", json!({"query": ""}));
    run(&mut s, "view.peopleAccount", json!({"value": "here"}));
    assert_eq!(sorted(&s, "").len(), local);
    assert!(!sorted(&s, "").contains(&at_work) && !sorted(&s, "").contains(&at_home));
    assert_eq!(run(&mut s, "view.peopleAccount", json!({"value": "all"}))["value"], "all");
    assert_eq!(sorted(&s, "").len(), local + 2);
}

#[test]
fn only_an_account_whose_server_has_its_contacts_is_a_choice() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    // With nothing to choose from, the list shows everything, whatever was chosen before.
    assert!(contact_accounts(&s.mailbox).is_empty());
    let local = here(&s);
    run(&mut s, "view.peopleAccount", json!({"value": "here"}));
    assert_eq!((shown_account(&s), sorted(&s, "").len()), (AccountChoice::All, local));

    let bo = imap(&mut s);
    assert!(contact_accounts(&s.mailbox).is_empty());
    assert!(s.run("view.peopleAccount", &json!({"value": bo})).is_err());
    // A shared mailbox brings no contacts of its own: the account it is opened through does.
    let work = add(&mut s, "work@example.org");
    let shared = s.mailbox.add_account(Account {
        email: "desk@example.org".into(),
        kind: AccountKind::Microsoft,
        shared_of: Some(work),
        oauth: s.mailbox.account(work).unwrap().oauth.clone(),
        ..Default::default()
    });
    assert_eq!(contact_accounts(&s.mailbox), [work]);
    assert!(s.run("view.peopleAccount", &json!({"value": shared})).is_err());
    for junk in [json!({}), json!({"value": null}), json!({"value": "somebody"}), json!({"value": 12_345}), json!({"value": [1]})] {
        assert!(s.run("view.peopleAccount", &junk).is_err(), "{junk}");
    }
}

#[test]
fn an_account_that_goes_leaves_every_account_shown() {
    let (mut s, work, home, _, at_home) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "account.remove", json!({"id": work}));
    assert_eq!(shown_account(&s), AccountChoice::All);
    assert!(sorted(&s, "").contains(&at_home));
    // The choice of an account this mailbox doesn't have, as a window's saved settings can hold.
    s.view.people_account = AccountChoice::Account(987_654);
    assert_eq!(shown_account(&s), AccountChoice::All);
    run(&mut s, "account.remove", json!({"id": home}));
    s.view.people_account = AccountChoice::Here;
    assert_eq!(shown_account(&s), AccountChoice::All, "nothing left to choose between");
}

#[test]
fn another_mailbox_starts_on_every_account() {
    let (mut s, work, ..) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    s.replace_mailbox(crate::sample::mailbox(NOW, 0));
    assert_eq!(s.view.people_account, AccountChoice::All);
}

#[test]
fn the_selected_contact_stays_selected_only_where_it_is_listed() {
    let (mut s, work, home, at_work, _) = session();
    run(&mut s, "people.select", json!({"id": at_work}));
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    assert_eq!(s.sel.contact, Some(at_work));
    run(&mut s, "view.peopleAccount", json!({"value": home}));
    assert_eq!(s.sel.contact, None);
    run(&mut s, "view.peopleAccount", json!({"value": 0}));
    assert_eq!(shown_account(&s), AccountChoice::Here, "0 is this computer, as in `tasks.new`");
}

#[test]
fn a_new_contact_stays_on_this_computer_and_the_list_moves_to_show_it() {
    let (mut s, work, ..) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    let made = run(&mut s, "people.new", json!({"first": "Noor", "email": "noor@example.org"}))["id"].as_u64().unwrap();
    assert!(s.mailbox.contact(made).unwrap().origin.is_local());
    assert_eq!((shown_account(&s), s.sel.contact), (AccountChoice::Here, Some(made)));
    assert!(sorted(&s, "").contains(&made));
    // With every account shown, it is in the list already.
    run(&mut s, "view.peopleAccount", json!({"value": "all"}));
    run(&mut s, "people.new", json!({"first": "Oren"}));
    assert_eq!(shown_account(&s), AccountChoice::All);
}

#[test]
fn add_to_contacts_shows_the_contact_wherever_it_is() {
    let (mut s, work, home, at_work, _) = session();
    let unknown = s.mailbox.messages.values().find(|m| m.from.is_valid()).unwrap().id;
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "people.fromMessage", json!({"id": unknown}));
    let made = s.sel.contact.unwrap();
    assert_eq!((shown_account(&s), s.view.module), (AccountChoice::Here, Module::People));
    assert!(s.mailbox.contact(made).unwrap().origin.is_local());

    // A sender who is a contact of an account already: the list moves to that account.
    let mut known = (**s.mailbox.messages.values().next().unwrap()).clone();
    known.from = mailcraft_model::Address::new("Wren Work", "w1@example.org");
    *s.mailbox.message_mut(unknown).unwrap() = mailcraft_model::Message { id: unknown, ..known };
    run(&mut s, "view.peopleAccount", json!({"value": home}));
    assert_eq!(run(&mut s, "people.fromMessage", json!({"id": unknown}))["existing"], true);
    assert_eq!((shown_account(&s), s.sel.contact), (AccountChoice::Account(work), Some(at_work)));
}

#[test]
fn an_imported_card_is_shown_on_this_computer() {
    let (mut s, work, ..) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    let card = b"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ivana Example\r\nEND:VCARD\r\n";
    let read = crate::io::import(&mut s, "ivana.vcf", card, None).unwrap();
    assert_eq!(read["contacts"], 1);
    assert_eq!(shown_account(&s), AccountChoice::Here);
    assert!(sorted(&s, "").contains(&s.sel.contact.unwrap()));
}

#[test]
fn a_merge_shows_the_contact_it_keeps() {
    let (mut s, work, ..) = session();
    let one = run(&mut s, "people.new", json!({"first": "Rowan", "email": "rowan@example.org"}))["id"].as_u64().unwrap();
    let two = run(&mut s, "people.new", json!({"first": "Rowan", "last": "Twice", "email": "rowan@example.org"}))["id"].as_u64().unwrap();
    run(&mut s, "people.select", json!({"id": null}));
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "people.merge", json!({"keep": one, "others": [two]}));
    assert_eq!((shown_account(&s), s.sel.contact), (AccountChoice::Here, Some(one)));
}

#[test]
fn selecting_a_contact_the_list_doesnt_show_moves_the_list() {
    let (mut s, work, home, at_work, _) = session();
    run(&mut s, "view.peopleAccount", json!({"value": home}));
    run(&mut s, "people.select", json!({"id": at_work}));
    assert_eq!(shown_account(&s), AccountChoice::Account(work));
    run(&mut s, "people.select", json!({"id": null}));
    assert_eq!(shown_account(&s), AccountChoice::Account(work), "selecting nobody leaves it where it is");
}

#[test]
fn a_delete_selects_the_neighbour_in_the_list_shown() {
    let (mut s, work, ..) = session();
    let last = sorted(&s, "").into_iter().rfind(|id| s.mailbox.contact(*id).is_some_and(|c| c.origin.is_local())).unwrap();
    run(&mut s, "view.peopleAccount", json!({"value": "here"}));
    let order = sorted(&s, "");
    let at = order.iter().position(|id| *id == last).unwrap();
    run(&mut s, "people.select", json!({"id": last}));
    run(&mut s, "people.delete", json!({}));
    let neighbour = order.get(at + 1).or_else(|| order.get(at - 1)).copied();
    assert_eq!(s.sel.contact, neighbour);
    assert!(s.mailbox.contact(neighbour.unwrap()).unwrap().origin.account != work);
}

#[test]
fn export_takes_every_contact_whatever_the_list_shows() {
    let (mut s, work, ..) = session();
    let every = s.mailbox.contacts.len();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    assert_eq!(sorted(&s, "").len(), 1);
    assert_eq!(run(&mut s, "file.exportContacts", json!({}))["contacts"], every);
    assert_eq!(sorted_of(&s, "", AccountChoice::All).len(), every);
}

#[test]
fn people_list_follows_the_window_unless_it_names_an_account_and_says_where_each_is() {
    let (mut s, work, home, at_work, at_home) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    let shown = run(&mut s, "people.list", json!({}));
    assert_eq!((&shown["account"], &shown["contacts"][0]["id"]), (&json!(work), &json!(at_work)));
    assert_eq!((&shown["contacts"][0]["account"], &shown["contacts"][0]["keptAt"]), (&json!(work), &json!("work@example.org")));

    let other = run(&mut s, "people.list", json!({"account": home}));
    assert_eq!((other["contacts"].as_array().unwrap().len(), &other["contacts"][0]["id"]), (1, &json!(at_home)));
    let every = run(&mut s, "people.list", json!({"account": "all", "query": "work"}));
    assert_eq!(every["contacts"][0]["name"], "Wren Work");
    let local = run(&mut s, "people.list", json!({"account": 0}));
    assert_eq!(
        (&local["account"], &local["contacts"][0]["account"], &local["contacts"][0]["keptAt"]),
        (&json!("here"), &json!(0), &json!("this computer"))
    );
    assert!(s.run("people.list", &json!({"account": "somebody"})).is_err());

    let card = run(&mut s, "people.get", json!({"id": at_home}));
    assert_eq!((&card["account"], &card["keptAt"]), (&json!(home), &json!("home@example.org")));
}

#[test]
fn an_account_without_contacts_on_its_server_is_refused_not_listed_as_empty() {
    let (mut s, work, ..) = session();
    let bo = imap(&mut s);
    for account in [json!(bo), json!(999_999)] {
        assert!(s.run("people.list", &json!({"account": account})).is_err(), "{account}");
    }
    assert!(s.run("people.list", &json!({"account": work})).is_ok());
}

#[test]
fn an_agent_may_change_what_the_list_shows() {
    let (s, work, ..) = session();
    let access = crate::agent::classify(&s, "view.peopleAccount", &json!({"value": work}));
    assert_eq!(access, crate::agent::Access::Safe);
}

#[test]
fn a_contact_made_or_picked_is_not_hidden_by_the_search() {
    let (mut s, _, _, at_work, _) = session();
    run(&mut s, "people.search", json!({"query": "zz-nobody"}));
    let made = run(&mut s, "people.new", json!({"first": "Noor", "email": "noor@example.org"}))["id"].as_u64().unwrap();
    assert_eq!(s.view.people_search, "", "the search that left it out is cleared");
    assert!(sorted(&s, &s.view.people_search).contains(&made));
    // A search the contact matches stays.
    run(&mut s, "people.search", json!({"query": "wren"}));
    run(&mut s, "people.select", json!({"id": at_work}));
    assert_eq!((s.view.people_search.as_str(), shown_account(&s)), ("wren", AccountChoice::All));
    // Picking one the search leaves out clears it.
    run(&mut s, "people.select", json!({"id": made}));
    assert_eq!(s.view.people_search, "");
    assert!(sorted(&s, "").contains(&made));
    // An imported card too.
    run(&mut s, "people.search", json!({"query": "zz-nobody"}));
    crate::io::import(&mut s, "ivana.vcf", b"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ivana Example\r\nEND:VCARD\r\n", None).unwrap();
    assert_eq!(s.view.people_search, "");
}

#[test]
fn this_computer_chosen_is_forgotten_with_the_last_account_that_keeps_contacts() {
    let (mut s, work, home, ..) = session();
    run(&mut s, "view.peopleAccount", json!({"value": "here"}));
    run(&mut s, "account.remove", json!({"id": work}));
    assert_eq!(s.view.people_account, AccountChoice::Here, "another account keeps contacts still");
    run(&mut s, "account.remove", json!({"id": home}));
    assert_eq!(s.view.people_account, AccountChoice::All);
    // A Microsoft account added later opens People on every account.
    add(&mut s, "later@example.org");
    assert_eq!(shown_account(&s), AccountChoice::All);
}

#[test]
fn undoing_a_new_contact_puts_the_list_back_on_the_account() {
    let (mut s, work, _, at_work, _) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "people.select", json!({"id": at_work}));
    let made = run(&mut s, "people.new", json!({"first": "Noor"}))["id"].as_u64().unwrap();
    assert_eq!(shown_account(&s), AccountChoice::Here);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.contact(made).is_none());
    assert_eq!((shown_account(&s), s.sel.contact), (AccountChoice::Account(work), Some(at_work)));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!((shown_account(&s), s.sel.contact), (AccountChoice::Here, Some(made)));
}
