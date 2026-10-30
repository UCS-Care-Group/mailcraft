// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each Microsoft account's own list of categories (#633): what a read of a server's list does to
//! it, and the commands that name a list.

use mailcraft_model::remote::CategoriesDownload;
use mailcraft_model::{AccountKind, Category, FolderKind, GREY_CATEGORY, Id, Mailbox, Message, Rgb};
use serde_json::json;

use crate::Session;
use crate::agent::{Class, Verdict, check};

const NOW: i64 = 1_800_000_000;
const RED: Rgb = Rgb(200, 0, 0);
const WINE: Rgb = Rgb(90, 0, 30);

fn rig() -> (Session, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut add = |email: &str| s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (a, b) = (add("a@example.org"), add("b@example.org"));
    (s, a, b)
}

fn tag(name: &str, color: Rgb) -> Category {
    Category { name: name.into(), color }
}

fn read(s: &mut Session, account: Id, categories: Vec<Category>) -> serde_json::Value {
    let email = s.mailbox.account(account).unwrap().email.clone();
    super::remote::apply_categories(s, &CategoriesDownload { account, email, categories }).unwrap()
}

fn message_of(s: &mut Session, account: Id, categories: &[&str]) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let categories = categories.iter().map(|c| c.to_string()).collect();
    s.mailbox.add_message(Message { folder, categories, ..Default::default() }).unwrap()
}

fn names(s: &Session, account: Id) -> Vec<&str> {
    s.mailbox.categories_of(account).iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn each_server_fills_its_own_accounts_list_and_the_shared_list_stays() {
    let (mut s, a, b) = rig();
    let shared = s.mailbox.categories.clone();
    read(&mut s, a, vec![tag("Red", RED), tag("Client X", RED)]);
    read(&mut s, b, vec![tag("Red", WINE)]);
    assert_eq!(names(&s, a), ["Red", "Client X"]);
    assert_eq!(names(&s, b), ["Red"]);
    assert_eq!((s.mailbox.category_color_of(a, "Red"), s.mailbox.category_color_of(b, "Red")), (RED, WINE));
    assert_eq!(s.mailbox.categories, shared, "the servers' lists don't reach the shared one");
    assert_eq!(s.mailbox.server_categories[&a], ["Red", "Client X"]);
    assert_eq!(s.mailbox.server_categories[&b], ["Red"]);
    // Nothing new: nothing changes.
    assert_eq!(read(&mut s, a, vec![tag("Red", RED), tag("Client X", RED)])["changed"], 0);
}

#[test]
fn a_first_read_keeps_what_the_accounts_items_carry() {
    let (mut s, a, _) = rig();
    s.mailbox.set_category(0, "Mine", WINE).unwrap();
    let m = message_of(&mut s, a, &["Mine", "blue category"]);
    read(&mut s, a, vec![tag("Blue category", RED)]);
    assert_eq!(names(&s, a), ["Mine", "Blue category"], "the server's spelling, and what only the items had");
    assert_eq!(s.mailbox.category_color_of(a, "Mine"), WINE);
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Mine", "Blue category"]);
}

#[test]
fn a_name_its_server_stopped_listing_leaves_the_list_and_what_was_made_here_stays() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Red", RED), tag("Old", WINE)]);
    read(&mut s, b, vec![tag("Old", WINE)]);
    let m = message_of(&mut s, a, &["Old"]);
    s.run("category.set", &json!({"account": a, "name": "Made here", "color": "#112233"})).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    assert_eq!(names(&s, a), ["Red", "Made here"]);
    // Its items keep the name, in grey; the other account's list has its own.
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Old"]);
    assert_eq!(s.mailbox.category_color_of(a, "Old"), GREY_CATEGORY);
    assert_eq!(s.mailbox.category_color_of(b, "Old"), WINE);
}

#[test]
fn the_servers_spelling_reaches_only_its_accounts_items() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Client", RED)]);
    read(&mut s, b, vec![tag("Client", RED)]);
    let (of_a, of_b) = (message_of(&mut s, a, &["Client"]), message_of(&mut s, b, &["Client"]));
    read(&mut s, a, vec![tag("CLIENT", RED)]);
    assert_eq!(names(&s, a), ["CLIENT"]);
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["CLIENT"]);
    assert_eq!(s.mailbox.message(of_b).unwrap().categories, ["Client"]);
}

#[test]
fn another_kind_of_account_adds_to_the_shared_list() {
    let (mut s, a, _) = rig();
    s.mailbox.accounts.iter_mut().find(|x| x.id == a).unwrap().kind = AccountKind::Imap;
    read(&mut s, a, vec![tag("From elsewhere", RED)]);
    assert!(s.mailbox.category("From elsewhere").is_some());
    assert!(s.mailbox.account_categories.is_empty() && s.mailbox.server_categories.is_empty());
}

#[test]
fn the_commands_change_the_list_they_name() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    read(&mut s, b, vec![tag("Red", WINE)]);
    let (of_a, of_b) = (message_of(&mut s, a, &["Red"]), message_of(&mut s, b, &["Red"]));

    s.run("category.set", &json!({"account": b, "name": "New", "color": "#112233"})).unwrap();
    assert_eq!(names(&s, b), ["Red", "New"]);
    assert_eq!(names(&s, a), ["Red"]);
    s.run("category.rename", &json!({"account": a, "from": "Red", "to": "Urgent"})).unwrap();
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["Urgent"]);
    assert_eq!(s.mailbox.message(of_b).unwrap().categories, ["Red"]);
    s.run("category.remove", &json!({"account": b, "name": "Red"})).unwrap();
    assert!(s.mailbox.message(of_b).unwrap().categories.is_empty());
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["Urgent"]);

    // With no account: the shared list, as before there were others.
    s.run("category.set", &json!({"name": "Shared", "color": "#112233"})).unwrap();
    assert!(s.mailbox.category("Shared").is_some() && s.mailbox.category_of(a, "Shared").is_none());
    // An account that isn't one is refused, whatever the command, and changes nothing.
    let before = s.mailbox.clone();
    for (command, params) in [
        ("category.set", json!({"account": 9_999_999, "name": "X", "color": "#112233"})),
        ("category.rename", json!({"account": 9_999_999, "from": "Shared", "to": "X"})),
        ("category.remove", json!({"account": 9_999_999, "name": "Shared"})),
        ("category.list", json!({"account": 9_999_999})),
    ] {
        assert!(s.run(command, &params).is_err(), "{command}");
    }
    assert_eq!(s.mailbox, before);
}

#[test]
fn outlooks_colour_is_refused_on_its_own_accounts_list_alone() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    read(&mut s, b, vec![tag("Other", RED)]);
    s.run("category.set", &json!({"account": b, "name": "Red", "color": "#112233"})).unwrap();
    assert!(s.run("category.set", &json!({"account": a, "name": "Red", "color": "#112233"})).is_err());
    let list = s.run("category.list", &json!({"account": b})).unwrap();
    let outlook = |name: &str| list["categories"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()["outlook"].clone();
    assert_eq!((outlook("Red"), outlook("Other")), (json!(false), json!(true)), "Red is A's server's, not B's");
    let accounts = list["accounts"].as_array().unwrap();
    assert_eq!(accounts.iter().map(|x| x["id"].as_u64().unwrap()).collect::<Vec<_>>(), [a, b]);
    // The shared list is no server's.
    let list = s.run("category.list", &json!({})).unwrap();
    assert!(list["categories"].as_array().unwrap().iter().all(|c| c["outlook"].is_null()));
}

#[test]
fn categorize_uses_the_list_of_each_messages_account() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("CLIENT", RED)]);
    read(&mut s, b, vec![tag("Client", WINE)]);
    let (of_a, of_b) = (message_of(&mut s, a, &[]), message_of(&mut s, b, &[]));
    s.run("mail.categorize", &json!({"ids": [of_a, of_b], "category": "client"})).unwrap();
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["CLIENT"]);
    assert_eq!(s.mailbox.message(of_b).unwrap().categories, ["Client"]);

    // A name nobody defined joins the list of the message's account alone, in grey.
    let shared = s.mailbox.categories.clone();
    s.run("mail.categorize", &json!({"id": of_a, "category": "Brand new"})).unwrap();
    assert_eq!(s.mailbox.category_of(a, "Brand new").map(|c| c.color), Some(GREY_CATEGORY));
    assert!(s.mailbox.category_of(b, "Brand new").is_none());
    assert_eq!(s.mailbox.categories, shared);
}

#[test]
fn the_server_list_isnt_the_owners_to_undo_but_the_accounts_list_is() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#112233"})).unwrap();
    s.mailbox.server_categories.insert(a, vec!["Red".into(), "Later".into()]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Mine").is_none());
    assert_eq!(s.mailbox.server_categories[&a], ["Red", "Later"]);
}

#[test]
fn an_agents_undo_that_loses_an_accounts_category_is_a_deletion() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#112233"})).unwrap();
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Delete, _)));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Delete]), Verdict::Run);
}

#[test]
fn an_agent_is_told_whose_category_goes() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    let Verdict::Ask(Class::Delete, text) = check(&s, "category.remove", &json!({"account": a, "name": "Red"}), &[]) else { panic!() };
    assert!(text.contains("a@example.org"), "{text}");
    for junk in [json!({"account": "x", "name": 7}), json!({"account": u64::MAX, "name": "Red"}), json!({"account": -1})] {
        let _ = check(&s, "category.remove", &junk, &[]);
        let _ = check(&s, "category.rename", &junk, &[]);
    }
}

#[test]
fn an_account_that_isnt_one_is_refused_and_no_list_changes() {
    let (mut s, a, _) = rig();
    s.run("category.set", &json!({"name": "Shared", "color": "#112233"})).unwrap();
    let before = s.mailbox.categories.clone();
    for junk in [json!(a.to_string()), json!(-1), json!(true), json!(1.5), json!(0), json!([a]), json!({"id": a}), json!(9_999_999)] {
        assert!(s.run("category.remove", &json!({"account": junk, "name": "Shared"})).is_err(), "{junk}");
        assert!(s.run("category.rename", &json!({"account": junk, "from": "Shared", "to": "Other"})).is_err(), "{junk}");
        assert!(s.run("category.set", &json!({"account": junk, "name": "New", "color": "#112233"})).is_err(), "{junk}");
        assert!(s.run("category.list", &json!({"account": junk})).is_err(), "{junk}");
    }
    assert_eq!(s.mailbox.categories, before);
    // No account, said or not, is the shared list.
    s.run("category.rename", &json!({"account": null, "from": "Shared", "to": "Other"})).unwrap();
    assert!(s.mailbox.category("Other").is_some() && s.mailbox.category("Shared").is_none());
}

#[test]
fn a_list_read_since_the_step_stays_after_an_undo() {
    let (mut s, a, _) = rig();
    s.run("category.set", &json!({"name": "Shared", "color": "#112233"})).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category("Shared").is_none());
    assert_eq!(s.mailbox.category_list_owner(a), Some(a));
    assert_eq!(names(&s, a), ["Red"]);
}

#[test]
fn what_a_read_changed_on_a_list_stays_after_an_undo() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED), tag("Old", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#112233"})).unwrap();
    let now = vec![tag("Red", RED), tag("New", WINE)];
    read(&mut s, a, now.clone());
    s.run("edit.undo", &json!({})).unwrap();
    // The owner's step is undone; what the server said isn't, and no name is left stranded here.
    assert_eq!(names(&s, a), ["Red", "New"]);
    read(&mut s, a, now);
    assert_eq!(names(&s, a), ["Red", "New"]);
}

#[test]
fn taking_off_a_name_no_list_has_leaves_the_list_alone() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    let m = message_of(&mut s, a, &["Came with it"]);
    s.run("mail.categorize", &json!({"id": m, "category": "Came with it"})).unwrap();
    assert!(s.mailbox.message(m).unwrap().categories.is_empty());
    assert_eq!(names(&s, a), ["Red"]);
    // Put on, it joins the list, to be given a colour.
    s.run("mail.categorize", &json!({"id": m, "category": "Came with it"})).unwrap();
    assert_eq!(names(&s, a), ["Red", "Came with it"]);
}

#[test]
fn an_agent_is_told_the_account_only_when_the_list_is_its_own() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    let imap = s.run("account.add", &json!({"email": "imap@example.org"})).unwrap()["id"].as_u64().unwrap();
    let local = s.mailbox.accounts.iter().find(|x| x.kind == AccountKind::Local).unwrap().id;
    let text = |s: &Session, account: Id| {
        let Verdict::Ask(Class::Delete, text) = check(s, "category.remove", &json!({"account": account, "name": "Red"}), &[]) else { panic!() };
        text
    };
    assert!(text(&s, a).contains("a@example.org"));
    // These use the shared list, which is no one account's.
    assert!(!text(&s, b).contains("b@example.org"));
    assert!(!text(&s, imap).contains("imap@example.org"));
    assert!(!text(&s, local).contains("no subject"));
}
