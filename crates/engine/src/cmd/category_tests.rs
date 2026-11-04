// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each Microsoft account's own list of categories (#633): what a read of a server's list does to
//! it, and the commands that name a list.

use mailcraft_model::remote::CategoriesDownload;
use mailcraft_model::{AccountKind, Calendar, Category, Contact, Event, FolderKind, GREY_CATEGORY, Id, Mailbox, Message, Origin, Rgb, Rights, Task};
use serde_json::json;

use crate::Session;
use crate::agent::{Class, Verdict, check};

const NOW: i64 = 1_800_000_000;
const RED: Rgb = Rgb(200, 0, 0);
const WINE: Rgb = Rgb(90, 0, 30);

/// Two Microsoft accounts and a shared list with nothing on it, so that a list holds what a test
/// puts there; what comes across from the shared list has tests of its own.
fn rig() -> (Session, Id, Id) {
    let (mut s, a) = one_account();
    let b = add(&mut s, "b@example.org");
    s.mailbox.categories.clear();
    (s, a, b)
}

fn add(s: &mut Session, email: &str) -> Id {
    s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap()
}

fn one_account() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let a = add(&mut s, "a@example.org");
    (s, a)
}

fn tag(name: &str, color: Rgb) -> Category {
    Category { name: name.into(), color }
}

/// A read of `account`'s server's list, whose sign-in covers changing it (#661): its list here
/// may be changed. (Read-only lists have tests of their own.)
fn read(s: &mut Session, account: Id, categories: Vec<Category>) -> serde_json::Value {
    let email = s.mailbox.account(account).unwrap().email.clone();
    let owner = s.mailbox.junk_owner(account);
    s.mailbox.category_writes.insert(owner);
    super::remote::apply_categories(s, &CategoriesDownload { account, email, categories, complete: true }).unwrap()
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
    s.run("category.set", &json!({"account": a, "name": "Made here", "color": "#2E8B57"})).unwrap();
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

    s.run("category.set", &json!({"account": b, "name": "New", "color": "#2E8B57"})).unwrap();
    assert_eq!(names(&s, b), ["Red", "New"]);
    assert_eq!(names(&s, a), ["Red"]);
    s.run("category.rename", &json!({"account": a, "from": "Red", "to": "Urgent"})).unwrap();
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["Urgent"]);
    assert_eq!(s.mailbox.message(of_b).unwrap().categories, ["Red"]);
    s.run("category.remove", &json!({"account": b, "name": "Red"})).unwrap();
    assert!(s.mailbox.message(of_b).unwrap().categories.is_empty());
    assert_eq!(s.mailbox.message(of_a).unwrap().categories, ["Urgent"]);

    // With no account: the shared list, as before there were others.
    s.run("category.set", &json!({"name": "Shared", "color": "#2E8B57"})).unwrap();
    assert!(s.mailbox.category("Shared").is_some() && s.mailbox.category_of(a, "Shared").is_none());
    // An account that isn't one is refused, whatever the command, and changes nothing.
    let before = s.mailbox.clone();
    for (command, params) in [
        ("category.set", json!({"account": 9_999_999, "name": "X", "color": "#2E8B57"})),
        ("category.rename", json!({"account": 9_999_999, "from": "Shared", "to": "X"})),
        ("category.remove", json!({"account": 9_999_999, "name": "Shared"})),
        ("category.list", json!({"account": 9_999_999})),
    ] {
        assert!(s.run(command, &params).is_err(), "{command}");
    }
    assert_eq!(s.mailbox, before);
}

#[test]
fn each_accounts_list_says_what_is_kept_in_step_with_its_own_server() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    read(&mut s, b, vec![tag("Other", RED)]);
    // A colour of Outlook's list is changed here and goes there (#661): it is no longer refused.
    s.run("category.set", &json!({"account": b, "name": "Red", "color": "#2E8B57"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Red", "color": "#2E8B57"})).unwrap();
    assert_eq!(s.mailbox.category_color_of(a, "Red"), Rgb(0x2E, 0x8B, 0x57));
    let list = s.run("category.list", &json!({"account": b})).unwrap();
    let outlook = |name: &str| list["categories"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()["outlook"].clone();
    assert_eq!((outlook("Red"), outlook("Other")), (json!(true), json!(true)), "both go to B's server");
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
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
    s.mailbox.server_categories.insert(a, vec!["Red".into(), "Later".into()]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Mine").is_none());
    assert_eq!(s.mailbox.server_categories[&a], ["Red", "Later"]);
}

#[test]
fn an_agents_undo_that_loses_an_accounts_category_is_a_deletion() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
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
    s.run("category.set", &json!({"name": "Shared", "color": "#2E8B57"})).unwrap();
    let before = s.mailbox.categories.clone();
    for junk in [json!(a.to_string()), json!(-1), json!(true), json!(1.5), json!(0), json!([a]), json!({"id": a}), json!(9_999_999)] {
        assert!(s.run("category.remove", &json!({"account": junk, "name": "Shared"})).is_err(), "{junk}");
        assert!(s.run("category.rename", &json!({"account": junk, "from": "Shared", "to": "Other"})).is_err(), "{junk}");
        assert!(s.run("category.set", &json!({"account": junk, "name": "New", "color": "#2E8B57"})).is_err(), "{junk}");
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
    s.run("category.set", &json!({"name": "Shared", "color": "#2E8B57"})).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category("Shared").is_none());
    assert_eq!(s.mailbox.category_list_owner(a), Some(a));
    // The read isn't what is undone, the owner's step is, and what it made goes from this list too.
    assert_eq!(names(&s, a), ["Red"]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(names(&s, a), ["Shared", "Red"]);
}

#[test]
fn a_name_that_left_a_list_stays_off_it_after_an_undo() {
    let (mut s, a, b) = rig();
    s.mailbox.set_category(0, "Invoices", WINE).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
    read(&mut s, b, vec![tag("Invoices", RED)]);
    assert_eq!(names(&s, a), ["Red", "Mine"]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(names(&s, a), ["Red"]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(names(&s, a), ["Red", "Mine"]);
}

#[test]
fn an_undo_that_takes_an_account_away_takes_its_servers_list_with_it() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    let c = add(&mut s, "c@example.org");
    read(&mut s, c, vec![tag("Teal", RED)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.account(c).is_none());
    assert!(!s.mailbox.server_categories.contains_key(&c));
    assert_eq!(s.mailbox.server_categories[&a], ["Red"]);
}

#[test]
fn the_colour_a_read_gave_stays_after_an_undo() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
    read(&mut s, a, vec![tag("Red", WINE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.category_color_of(a, "Red"), WINE);
}

#[test]
fn what_a_read_changed_on_a_list_stays_after_an_undo() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED), tag("Old", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
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

#[test]
fn with_one_microsoft_account_the_whole_shared_list_comes_across() {
    let (mut s, a) = one_account();
    s.mailbox.set_category(0, "Follow-up", WINE).unwrap();
    let shared = s.mailbox.categories.clone();
    read(&mut s, a, vec![tag("Client X", RED), tag("Blue category", RED)]);
    for c in shared.iter().filter(|c| c.name != "Blue category") {
        assert_eq!(s.mailbox.category_of(a, &c.name), Some(c), "{}", c.name);
    }
    // Its server's own come in its server's colour, and the shared list stays.
    assert_eq!(s.mailbox.category_color_of(a, "Blue category"), RED);
    assert_eq!(s.mailbox.category_color_of(a, "Client X"), RED);
    assert_eq!(s.mailbox.categories, shared);
}

#[test]
fn the_owners_own_come_across_to_each_account_and_another_servers_dont() {
    for first_is_a in [true, false] {
        let (mut s, a, b) = rig();
        // One list today: a name from each server, and one made here.
        for name in ["Client X", "Invoices", "Follow-up"] {
            s.mailbox.set_category(0, name, WINE).unwrap();
        }
        let (a_list, b_list) = (vec![tag("Client X", RED)], vec![tag("Invoices", RED)]);
        if first_is_a {
            read(&mut s, a, a_list);
            read(&mut s, b, b_list);
        } else {
            read(&mut s, b, b_list);
            read(&mut s, a, a_list);
        }
        assert_eq!(names(&s, a), ["Client X", "Follow-up"], "a first: {first_is_a}");
        assert_eq!(names(&s, b), ["Invoices", "Follow-up"], "a first: {first_is_a}");
        assert_eq!(s.mailbox.category_color_of(a, "Follow-up"), WINE);
        assert_eq!(s.mailbox.category_color_of(b, "Follow-up"), WINE);
    }
}

#[test]
fn a_category_made_here_stays_when_an_account_added_later_lists_it() {
    let (mut s, a) = one_account();
    s.mailbox.categories = vec![tag("Travel", WINE)];
    read(&mut s, a, vec![tag("Red", RED)]);
    assert!(names(&s, a).contains(&"Travel"));
    // The file keeps which accounts came after.
    s.mailbox = serde_json::from_slice(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    let b = add(&mut s, "b@example.org");
    read(&mut s, b, vec![tag("Travel", WINE)]);
    assert!(names(&s, a).contains(&"Travel"), "nothing of b's was ever on the shared list");
    assert_eq!(s.mailbox.category_color_of(a, "Travel"), WINE);
    assert_eq!(names(&s, b), ["Travel"]);
}

#[test]
fn an_undo_past_the_first_read_keeps_a_category_an_account_added_later_lists() {
    let (mut s, a) = one_account();
    s.mailbox.categories = vec![tag("Travel", WINE)];
    s.run("category.set", &json!({"name": "Step", "color": "#2E8B57"})).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    let b = add(&mut s, "b@example.org");
    read(&mut s, b, vec![tag("Travel", WINE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.account(b).is_none());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(names(&s, a).contains(&"Travel"));
    // Made again on the mailbox of then, an account added now still comes after the lists.
    let c = add(&mut s, "c@example.org");
    read(&mut s, c, vec![tag("Travel", WINE)]);
    assert!(names(&s, a).contains(&"Travel"));
}

#[test]
fn removing_the_account_read_first_makes_no_later_account_an_older_one() {
    let (mut s, a) = one_account();
    s.mailbox.categories = vec![tag("Travel", WINE)];
    read(&mut s, a, vec![tag("Red", RED)]);
    let (b, c) = (add(&mut s, "b@example.org"), add(&mut s, "c@example.org"));
    s.run("account.remove", &json!({"id": a})).unwrap();
    read(&mut s, b, vec![tag("Blue", RED)]);
    read(&mut s, c, vec![tag("Travel", WINE)]);
    assert!(names(&s, b).contains(&"Travel"));
}

#[test]
fn a_name_of_an_account_from_before_the_lists_leaves_those_made_since() {
    let (mut s, a, b) = rig();
    s.mailbox.set_category(0, "Invoices", WINE).unwrap();
    read(&mut s, a, vec![tag("Red", RED)]);
    let c = add(&mut s, "c@example.org");
    read(&mut s, c, vec![tag("Blue", RED)]);
    assert!(names(&s, a).contains(&"Invoices") && names(&s, c).contains(&"Invoices"));
    read(&mut s, b, vec![tag("Invoices", WINE)]);
    assert_eq!(names(&s, a), ["Red"]);
    assert_eq!(names(&s, c), ["Blue"]);
}

#[test]
fn what_is_used_or_changed_on_a_list_stays_when_another_server_lists_it() {
    let (mut s, a, b) = rig();
    for name in ["Unused", "On an item", "Recoloured", "On both"] {
        s.mailbox.set_category(0, name, WINE).unwrap();
    }
    message_of(&mut s, a, &["On an item"]);
    read(&mut s, a, vec![tag("On both", RED)]);
    s.run("category.set", &json!({"account": a, "name": "Recoloured", "color": "#2E8B57"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Made on this list", "color": "#2E8B57"})).unwrap();
    let on_b = ["Unused", "On an item", "Recoloured", "On both", "Made on this list"];
    read(&mut s, b, on_b.iter().map(|name| tag(name, RED)).collect());
    assert_eq!(names(&s, a), ["On an item", "Recoloured", "On both", "Made on this list"]);
    // The name that left stays on the shared list.
    assert!(s.mailbox.category("Unused").is_some());
}

// ---- categorize on an event, a contact and a to-do (#705) ----

fn origin(account: Id) -> Origin {
    if account == 0 { Origin::default() } else { Origin { account, id: "x".into(), ..Default::default() } }
}

fn calendar_of(s: &mut Session, account: Id) -> Id {
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id, name: "Cal".into(), visible: true, origin: origin(account), ..Default::default() });
    id
}

fn event_of(s: &mut Session, account: Id) -> Id {
    let calendar = calendar_of(s, account);
    s.mailbox.add_event(Event { calendar, title: "Meeting".into(), origin: origin(account), ..Default::default() }).unwrap()
}

fn contact_of(s: &mut Session, account: Id) -> Id {
    s.mailbox.add_contact(Contact { display_name: "Pat".into(), origin: origin(account), ..Default::default() })
}

fn task_of(s: &mut Session, account: Id) -> Id {
    s.mailbox.add_task(Task { title: "Do it".into(), origin: origin(account), ..Default::default() })
}

/// Every list of categories there is.
fn lists(s: &Session) -> (Vec<Category>, std::collections::BTreeMap<Id, Vec<Category>>) {
    (s.mailbox.categories.clone(), s.mailbox.account_categories.clone())
}

/// The three commands, each with an item of `account`.
fn items(s: &mut Session, account: Id) -> Vec<(&'static str, Id)> {
    vec![("calendar.categorize", event_of(s, account)), ("people.categorize", contact_of(s, account)), ("tasks.categorize", task_of(s, account))]
}

fn carried(s: &Session, command: &str, id: Id) -> Vec<String> {
    match command {
        "calendar.categorize" => s.mailbox.event(id).unwrap().categories.clone(),
        "people.categorize" => s.mailbox.contact(id).unwrap().categories.clone(),
        _ => s.mailbox.task(id).unwrap().categories.clone(),
    }
}

#[test]
fn a_new_name_on_an_event_a_contact_or_a_todo_joins_the_list_in_grey() {
    let (mut s, a, _) = rig();
    for (command, id) in items(&mut s, a) {
        let name = format!("New for {command}");
        s.run(command, &json!({"id": id, "category": name})).unwrap();
        assert_eq!(s.mailbox.category_of(a, &name).map(|c| c.color), Some(GREY_CATEGORY), "{command}");
        assert_eq!(carried(&s, command, id), [name.as_str()], "{command}");
    }
}

#[test]
fn a_new_name_joins_only_the_list_of_the_items_account() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![]);
    read(&mut s, b, vec![]);
    let shared = s.mailbox.categories.clone();
    for (command, id) in items(&mut s, a) {
        s.run(command, &json!({"id": id, "category": "Only A"})).unwrap();
        assert!(s.mailbox.category_of(a, "Only A").is_some(), "{command}");
        assert!(s.mailbox.category_of(b, "Only A").is_none(), "{command}");
        assert_eq!(s.mailbox.categories, shared, "{command}");
    }
    // An event takes its calendar's account.
    let on_b = event_of(&mut s, b);
    s.run("calendar.categorize", &json!({"id": on_b, "category": "Only B"})).unwrap();
    assert!(s.mailbox.category_of(b, "Only B").is_some());
    assert!(s.mailbox.category_of(a, "Only B").is_none());
}

#[test]
fn an_item_kept_here_puts_a_new_name_on_the_shared_list() {
    let (mut s, a, b) = rig();
    read(&mut s, a, vec![]);
    read(&mut s, b, vec![]);
    for (command, id) in items(&mut s, 0) {
        let name = format!("Local {command}");
        s.run(command, &json!({"id": id, "category": name})).unwrap();
        assert_eq!(s.mailbox.categories.iter().filter(|c| c.name == name).count(), 1, "{command}");
        assert!(s.mailbox.category_of(a, &name).is_none() && s.mailbox.category_of(b, &name).is_none(), "{command}");
    }
}

#[test]
fn an_item_takes_the_lists_spelling_without_a_second_entry() {
    let (mut s, a, _) = rig();
    read(&mut s, a, vec![tag("Red", RED)]);
    for (command, id) in items(&mut s, a) {
        s.run(command, &json!({"id": id, "category": " red "})).unwrap();
        assert_eq!(carried(&s, command, id), ["Red"], "{command}");
    }
    assert_eq!(names(&s, a), ["Red"]);
}

#[test]
fn taking_off_a_name_the_list_lacks_leaves_the_list_alone() {
    let (mut s, a, _) = rig();
    let before = s.mailbox.account_categories.clone();
    let shared = s.mailbox.categories.clone();
    for (command, id) in items(&mut s, a) {
        s.run(command, &json!({"id": id, "category": "Ghost", "value": false})).unwrap();
        assert!(carried(&s, command, id).is_empty(), "{command}");
    }
    assert_eq!((s.mailbox.account_categories.clone(), s.mailbox.categories.clone()), (before, shared));
}

#[test]
fn toggling_off_and_on_leaves_one_entry() {
    let (mut s, a, _) = rig();
    for (command, id) in items(&mut s, a) {
        let name = format!("Twice {command}");
        // On, off, on.
        for _ in 0..3 {
            s.run(command, &json!({"id": id, "category": name})).unwrap();
        }
        assert_eq!(carried(&s, command, id), [name.as_str()], "{command}");
        assert_eq!(s.mailbox.categories_of(a).iter().filter(|c| c.name == name).count(), 1, "{command}");
    }
}

#[test]
fn an_event_in_a_calendar_that_can_only_be_read_is_refused_and_the_list_stays() {
    let (mut s, a, _) = rig();
    let event = event_of(&mut s, a);
    let calendar = s.mailbox.event(event).unwrap().calendar;
    s.mailbox.calendars.iter_mut().find(|c| c.id == calendar).unwrap().rights = Rights::Read;
    let before = lists(&s);
    assert!(s.run("calendar.categorize", &json!({"id": event, "category": "Nope"})).is_err());
    assert_eq!(lists(&s), before);
    assert!(s.mailbox.event(event).unwrap().categories.is_empty());
}

#[test]
fn a_name_that_is_empty_is_refused_and_the_list_stays() {
    let (mut s, a, _) = rig();
    for (command, id) in items(&mut s, a) {
        let before = lists(&s);
        for name in ["", "   "] {
            assert!(s.run(command, &json!({"id": id, "category": name})).is_err(), "{command}");
        }
        assert_eq!(lists(&s), before, "{command}");
        assert!(carried(&s, command, id).is_empty(), "{command}");
    }
}

#[test]
fn an_undo_takes_the_added_name_off_the_list_and_a_redo_puts_it_back() {
    let (mut s, a, _) = rig();
    for (command, id) in items(&mut s, a) {
        let name = format!("Undo {command}");
        s.run(command, &json!({"id": id, "category": name})).unwrap();
        s.run("edit.undo", &json!({})).unwrap();
        assert!(s.mailbox.category_of(a, &name).is_none(), "{command}");
        assert!(carried(&s, command, id).is_empty(), "{command}");
        s.run("edit.redo", &json!({})).unwrap();
        assert!(s.mailbox.category_of(a, &name).is_some(), "{command}");
        assert_eq!(carried(&s, command, id), [name.as_str()], "{command}");
    }
}

#[test]
fn a_full_list_refuses_a_new_name_but_still_spells_one_it_has() {
    let (mut s, a, _) = rig();
    for i in 0..200 {
        s.mailbox.set_category(a, &format!("C{i}"), RED).unwrap();
    }
    for (command, id) in items(&mut s, a) {
        assert!(s.run(command, &json!({"id": id, "category": "One more"})).is_err(), "{command}");
        assert!(carried(&s, command, id).is_empty(), "{command}");
        s.run(command, &json!({"id": id, "category": "c7"})).unwrap();
        assert_eq!(carried(&s, command, id), ["C7"], "{command}");
    }
    assert_eq!(s.mailbox.categories_of(a).len(), 200);
}
