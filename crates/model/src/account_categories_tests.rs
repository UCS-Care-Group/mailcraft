// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each Microsoft account's own list of categories (#633): whose list an item uses, what changing
//! one list reaches, and what a file brings.

use crate::mailbox::MAX_CATEGORIES;
use crate::{
    Account, AccountKind, Action, Calendar, Category, Condition, Contact, Event, FolderKind, GREY_CATEGORY, Id, Mailbox, Message, Origin, Rgb, Rule,
    Task,
};

const RED: Rgb = Rgb(200, 0, 0);
const WINE: Rgb = Rgb(90, 0, 30);

fn microsoft(mb: &mut Mailbox, email: &str) -> Id {
    mb.add_account(Account { email: email.into(), kind: AccountKind::Microsoft, ..Default::default() })
}

/// A mailbox with two Microsoft accounts, the first shared-list account being this computer.
fn two_accounts() -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("", "");
    let a = microsoft(&mut mb, "a@example.org");
    let b = microsoft(&mut mb, "b@example.org");
    (mb, a, b)
}

fn tagged(name: &str, color: Rgb) -> Category {
    Category { name: name.into(), color }
}

/// A message of `account` with `categories`.
fn message_of(mb: &mut Mailbox, account: Id, categories: &[&str]) -> Id {
    let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mb.add_message(Message { folder, categories: categories.iter().map(|c| c.to_string()).collect(), ..Default::default() }).unwrap()
}

/// One item of each kind that is kept by `account`.
fn items_of(mb: &mut Mailbox, account: Id, name: &str) -> (Id, Id, Id, Id) {
    let origin = Origin { account, id: format!("{account}"), ..Default::default() };
    let categories = vec![name.to_string()];
    let message = message_of(mb, account, &[name]);
    let contact = mb.add_contact(Contact { origin: origin.clone(), categories: categories.clone(), ..Default::default() });
    let task = mb.add_task(Task { origin: origin.clone(), categories: categories.clone(), ..Default::default() });
    let calendar = mb.alloc_id();
    mb.calendars.push(Calendar { id: calendar, origin: origin.clone(), ..Default::default() });
    let event = mb.add_event(Event { calendar, categories, ..Default::default() }).unwrap();
    (message, contact, task, event)
}

fn names(mb: &Mailbox, items: (Id, Id, Id, Id)) -> [Vec<String>; 4] {
    let (message, contact, task, event) = items;
    [
        mb.message(message).unwrap().categories.clone(),
        mb.contact(contact).unwrap().categories.clone(),
        mb.task(task).unwrap().categories.clone(),
        mb.event(event).unwrap().categories.clone(),
    ]
}

#[test]
fn an_account_with_its_own_list_has_its_own_colours_and_the_rest_share_one() {
    let (mut mb, a, b) = two_accounts();
    let here = mb.accounts[0].id;
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    mb.account_categories.insert(b, vec![tagged("Red", WINE), tagged("Client X", RED)]);
    assert_eq!((mb.category_color_of(a, "red"), mb.category_color_of(b, "red")), (RED, WINE));
    // A's list doesn't have B's category: an item of A shows it grey, as Outlook does.
    assert_eq!(mb.category_color_of(a, "Client X"), GREY_CATEGORY);
    assert!(mb.category_of(a, "Client X").is_none() && mb.category_of(b, "Client X").is_some());
    // This computer, an account that is gone and no account at all use the shared list.
    for account in [here, 0, 9_999_999] {
        assert_eq!(mb.categories_of(account), mb.categories.as_slice());
        assert_eq!(mb.category_list_owner(account), None);
    }
    // So does a Microsoft account whose server's list wasn't read yet.
    let new = microsoft(&mut mb, "new@example.org");
    assert_eq!(mb.categories_of(new), mb.categories.as_slice());
}

#[test]
fn a_shared_mailbox_uses_its_owners_list() {
    let (mut mb, a, _) = two_accounts();
    let shared = mb.add_account(Account { email: "team@example.org".into(), shared_of: Some(a), ..Default::default() });
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    assert_eq!(mb.category_list_owner(shared), Some(a));
    assert_eq!(mb.category_color_of(shared, "Red"), RED);
    // Its items are the owner's to rename.
    let m = message_of(&mut mb, shared, &["Red"]);
    mb.rename_category(a, "Red", "Urgent").unwrap();
    assert_eq!(mb.message(m).unwrap().categories, ["Urgent"]);
}

#[test]
fn renaming_or_deleting_reaches_only_the_items_that_use_that_list() {
    let (mut mb, a, b) = two_accounts();
    let here = mb.accounts[0].id;
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    mb.account_categories.insert(b, vec![tagged("Red", WINE)]);
    mb.set_category(0, "Red", RED).unwrap();
    let (of_a, of_b) = (items_of(&mut mb, a, "Red"), items_of(&mut mb, b, "Red"));
    let of_here = message_of(&mut mb, here, &["Red"]);
    let local = mb.add_contact(Contact { categories: vec!["Red".into()], ..Default::default() });

    mb.rename_category(a, "Red", "Urgent").unwrap();
    assert_eq!(names(&mb, of_a), [["Urgent"], ["Urgent"], ["Urgent"], ["Urgent"]]);
    assert_eq!(names(&mb, of_b), [["Red"], ["Red"], ["Red"], ["Red"]]);
    assert_eq!((mb.category_of(a, "Urgent").map(|c| c.color), mb.category_of(b, "Red").map(|c| c.color)), (Some(RED), Some(WINE)));
    assert!(mb.category("Urgent").is_none() && mb.category("Red").is_some());

    // The shared list's reaches what uses it: this computer's, and no account with its own.
    mb.rename_category(0, "Red", "Warm").unwrap();
    assert_eq!(mb.message(of_here).unwrap().categories, ["Warm"]);
    assert_eq!(mb.contact(local).unwrap().categories, ["Warm"]);
    assert_eq!(names(&mb, of_b), [["Red"], ["Red"], ["Red"], ["Red"]]);

    mb.remove_category(b, "Red").unwrap();
    assert_eq!(names(&mb, of_b), [[""; 0], [""; 0], [""; 0], [""; 0]]);
    assert_eq!(names(&mb, of_a), [["Urgent"], ["Urgent"], ["Urgent"], ["Urgent"]]);
    assert!(mb.remove_category(b, "Red").is_err() && mb.rename_category(b, "Nope", "X").is_err());
    // A name another list has is free here.
    mb.set_category(b, "Urgent", WINE).unwrap();
    assert!(mb.rename_category(b, "Urgent", "Calm").is_ok());
}

#[test]
fn an_accounts_first_list_is_what_its_items_carry_in_the_colours_they_had() {
    let (mut mb, a, b) = two_accounts();
    mb.set_category(0, "Client X", RED).unwrap();
    mb.set_category(0, "Only B", WINE).unwrap();
    items_of(&mut mb, a, "Client X");
    message_of(&mut mb, a, &["client x", "From elsewhere"]);
    message_of(&mut mb, b, &["Only B"]);
    let shared_list = mb.categories.clone();

    mb.start_account_categories(a);
    assert_eq!(mb.categories_of(a), [tagged("Client X", RED), tagged("From elsewhere", GREY_CATEGORY)]);
    assert_eq!(mb.categories, shared_list, "nothing leaves the shared list");
    assert_eq!(mb.categories_of(b), mb.categories.as_slice(), "and B isn't read yet");

    // It is made once.
    mb.set_category(a, "Mine", RED).unwrap();
    mb.start_account_categories(a);
    assert!(mb.category_of(a, "Mine").is_some());
}

#[test]
fn an_accounts_first_list_takes_only_names_fit_for_a_list() {
    let (mut mb, a, _) = two_accounts();
    let long = "x".repeat(100_000);
    message_of(&mut mb, a, &["  ", "\u{0}", &long, "Fine"]);
    let many: Vec<String> = (0..500).map(|i| format!("c{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    message_of(&mut mb, a, &many);
    mb.start_account_categories(a);
    let list = mb.categories_of(a);
    assert_eq!(list.len(), crate::remote::MAX_REMOTE_CATEGORIES);
    assert!(list.iter().all(|c| !c.name.trim().is_empty() && c.name.chars().count() <= crate::MAX_NAME));
    assert!(list.iter().any(|c| c.name == "Fine"));
}

#[test]
fn a_selection_is_offered_what_all_its_accounts_have_and_a_search_every_name() {
    let (mut mb, a, b) = two_accounts();
    mb.categories = vec![tagged("Red", WINE), tagged("Shared", RED)];
    mb.account_categories.insert(a, vec![tagged("Red", RED), tagged("Client X", RED)]);
    mb.account_categories.insert(b, vec![tagged("RED", WINE), tagged("Other", RED)]);
    assert_eq!(mb.categories_for(&[a]), mb.categories_of(a));
    assert_eq!(mb.categories_for(&[a, b]), [tagged("Red", RED)]);
    assert_eq!(mb.categories_for(&[]), mb.categories);
    let every: Vec<String> = mb.every_category().into_iter().map(|c| c.name).collect();
    assert_eq!(every, ["Red", "Shared", "Client X", "Other"]);
    assert_eq!(mb.all_categories().count(), 6);
}

#[test]
fn a_rule_puts_its_category_on_the_list_of_an_account_with_its_own() {
    let (mut mb, a, _) = two_accounts();
    let here = mb.accounts[0].id;
    mb.set_category(0, "From a rule", RED).unwrap();
    mb.account_categories.insert(a, Vec::new());
    mb.rules.push(Rule {
        enabled: true,
        conditions: vec![Condition::SubjectContains(String::new())],
        actions: vec![Action::Categorize("From a rule".into()), Action::Categorize("Unknown".into())],
        ..Default::default()
    });
    let m = message_of(&mut mb, a, &[]);
    mb.apply_rules(m);
    assert_eq!(mb.categories_of(a), [tagged("From a rule", RED), tagged("Unknown", GREY_CATEGORY)]);
    // The shared list stays as it was, as it always did for a rule.
    let m = message_of(&mut mb, here, &[]);
    mb.apply_rules(m);
    assert!(mb.category("Unknown").is_none());
}

#[test]
fn a_removed_account_takes_its_list_and_a_file_is_tidied() {
    let (mut mb, a, b) = two_accounts();
    let shared = mb.add_account(Account { email: "team@example.org".into(), shared_of: Some(a), ..Default::default() });
    for account in [a, b] {
        mb.account_categories.insert(account, vec![tagged("Red", RED)]);
        mb.server_categories.insert(account, vec!["Red".into()]);
    }
    mb.remove_account(b).unwrap();
    assert!(mb.account_categories.keys().eq([&a]) && mb.server_categories.keys().eq([&a]));

    // From a file: an account that isn't there and a shared mailbox have no list; lists are capped
    // and their names clean.
    mb.account_categories.insert(9_999_999, vec![tagged("Stray", RED)]);
    mb.server_categories.insert(shared, vec!["Stray".into()]);
    mb.account_categories.insert(a, (0..MAX_CATEGORIES + 50).map(|i| tagged(&format!("c{i}\n"), RED)).collect());
    mb.server_categories.insert(a, vec!["n".into(); MAX_CATEGORIES + 50]);
    assert!(!mb.repair().is_empty());
    assert!(mb.account_categories.keys().eq([&a]) && mb.server_categories.keys().eq([&a]));
    assert_eq!((mb.account_categories[&a].len(), mb.server_categories[&a].len()), (MAX_CATEGORIES, MAX_CATEGORIES));
    assert_eq!(mb.account_categories[&a][0].name, "c0");
}

#[test]
fn a_file_keeps_the_lists_and_one_from_before_opens() {
    let (mut mb, a, _) = two_accounts();
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    mb.server_categories.insert(a, vec!["Red".into()]);
    let text = serde_json::to_string(&mb).unwrap();
    let back: Mailbox = serde_json::from_str(&text).unwrap();
    assert_eq!((back.account_categories, back.server_categories), (mb.account_categories.clone(), mb.server_categories.clone()));

    // The old mailbox-wide list of Outlook's names is read again from each server.
    let mut value = serde_json::to_value(&mb).unwrap();
    let file = value.as_object_mut().unwrap();
    file.remove("accountCategories");
    file.remove("serverCategories");
    file.insert("outlookCategories".to_string(), serde_json::json!(["Red"]));
    let old: Mailbox = serde_json::from_value(value).unwrap();
    assert!(old.account_categories.is_empty() && old.server_categories.is_empty());
    assert_eq!(old.categories_of(a), old.categories.as_slice());
    // An empty mailbox writes neither.
    let text = serde_json::to_string(&Mailbox::default()).unwrap();
    assert!(!text.contains("accountCategories") && !text.contains("serverCategories"));
}

#[test]
fn a_to_do_that_stays_keeps_the_colours_of_the_list_that_goes() {
    let (mut mb, a, _) = two_accounts();
    mb.start_account_categories(a);
    mb.set_category(a, "Client X", WINE).unwrap();
    mb.set_category(a, "Home", WINE).unwrap();
    mb.set_category(a, "Unused", WINE).unwrap();
    mb.set_category(0, "Home", RED).unwrap();
    let origin = Origin { account: a, id: "T-1".into(), ..Default::default() };
    let task = mb.add_task(Task { origin, categories: vec!["client x".into(), "Home".into(), "Nobody's".into()], ..Default::default() });

    mb.remove_account(a).unwrap();
    assert!(mb.task(task).is_some_and(|t| t.origin.is_local()));
    assert_eq!(mb.category_color_of(0, "client x"), WINE);
    // The shared list's colour stays, and only what a to-do carries comes across.
    assert_eq!(mb.category_color_of(0, "Home"), RED);
    assert!(mb.category("Unused").is_none() && mb.category("Nobody's").is_none());
}

#[test]
fn every_category_of_a_list_in_a_file_counts() {
    let (mut mb, a, _) = two_accounts();
    let room = 200;
    assert!(crate::from_json(&serde_json::to_vec(&mb).unwrap(), room).is_ok());
    mb.account_categories.insert(a, (0..room).map(|i| tagged(&format!("c{i}"), RED)).collect());
    let bytes = serde_json::to_vec(&mb).unwrap();
    assert!(crate::from_json(&bytes, room).is_err());
    assert!(crate::from_json(&bytes, 2 * room + 1).is_ok());

    mb.account_categories.clear();
    mb.server_categories.insert(a, (0..room).map(|i| format!("c{i}")).collect());
    let bytes = serde_json::to_vec(&mb).unwrap();
    assert!(crate::from_json(&bytes, room).is_err());
    assert!(crate::from_json(&bytes, 2 * room + 1).is_ok());
}

#[test]
fn what_isnt_a_list_costs_that_much_and_not_the_file() {
    let (mb, a, _) = two_accounts();
    let good = serde_json::to_value(tagged("Red", RED)).unwrap();
    let mut value = serde_json::to_value(&mb).unwrap();
    let file = value.as_object_mut().unwrap();
    let id = a.to_string();
    file.insert("serverCategories".into(), serde_json::json!({&id: ["Red", 7], "12": "x", "nonsense": ["Blue"]}));
    file.insert("accountCategories".into(), serde_json::json!({&id: [good, {"name": "Bad", "color": [300, 0, 0]}, "x"], "12": 7}));
    let back: Mailbox = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(back.server_categories.into_iter().collect::<Vec<_>>(), [(a, vec!["Red".to_string()])]);
    assert_eq!(back.account_categories.into_iter().collect::<Vec<_>>(), [(a, vec![tagged("Red", RED)])]);

    let file = value.as_object_mut().unwrap();
    file.insert("serverCategories".into(), serde_json::json!("x"));
    file.insert("accountCategories".into(), serde_json::json!(7));
    let back: Mailbox = serde_json::from_value(value).unwrap();
    assert!(back.server_categories.is_empty() && back.account_categories.is_empty());
}
