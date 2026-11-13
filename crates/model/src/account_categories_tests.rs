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
    mb.categories.clear();
    mb.set_category(0, "Client X", RED).unwrap();
    mb.set_category(0, "Only B", WINE).unwrap();
    items_of(&mut mb, a, "Client X");
    message_of(&mut mb, a, &["client x", "From elsewhere"]);
    message_of(&mut mb, b, &["Only B"]);
    let shared_list = mb.categories.clone();
    mb.server_categories.insert(b, vec!["Only B".into()]);

    mb.start_account_categories(a, &[]);
    assert_eq!(mb.categories_of(a), [tagged("Client X", RED), tagged("From elsewhere", GREY_CATEGORY)]);
    assert_eq!(mb.categories, shared_list, "nothing leaves the shared list");
    assert_eq!(mb.categories_of(b), mb.categories.as_slice(), "and B isn't read yet");

    // It is made once.
    mb.set_category(a, "Mine", RED).unwrap();
    mb.start_account_categories(a, &[]);
    assert!(mb.category_of(a, "Mine").is_some());
}

#[test]
fn an_accounts_first_list_takes_only_names_fit_for_a_list() {
    let (mut mb, a, _) = two_accounts();
    mb.categories.clear();
    let long = "x".repeat(100_000);
    message_of(&mut mb, a, &["  ", "\u{0}", &long, "Fine"]);
    let many: Vec<String> = (0..500).map(|i| format!("c{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    message_of(&mut mb, a, &many);
    mb.start_account_categories(a, &[]);
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
        conditions: vec![Condition::SubjectContains(vec!["Weekly".into()])],
        actions: vec![Action::Categorize("From a rule".into()), Action::Categorize("Unknown".into())],
        ..Default::default()
    });
    let clock = crate::RuleClock { now: 0, zone: crate::time::Zone::fixed(0).unwrap(), due_hour: 17 };
    let m = message_of(&mut mb, a, &[]);
    mb.message_mut(m).unwrap().subject = "Weekly".into();
    mb.apply_rules(m, &clock);
    assert_eq!(mb.categories_of(a), [tagged("From a rule", RED), tagged("Unknown", GREY_CATEGORY)]);
    // The shared list stays as it was, as it always did for a rule.
    let m = message_of(&mut mb, here, &[]);
    mb.message_mut(m).unwrap().subject = "Weekly".into();
    mb.apply_rules(m, &clock);
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
    mb.start_account_categories(a, &[]);
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

#[test]
fn a_list_goes_with_an_account_whose_id_is_corrected() {
    let (mut mb, a, b) = two_accounts();
    let id_of = |mb: &Mailbox, email: &str| mb.accounts.iter().find(|x| x.email == email).unwrap().id;
    let shared = mb.categories.clone();
    for (account, id, name) in [(a, 0, "Mine"), (b, u64::MAX, "Theirs")] {
        mb.accounts.iter_mut().find(|x| x.id == account).unwrap().id = id;
        mb.account_categories.insert(id, vec![tagged(name, RED)]);
        mb.server_categories.insert(id, vec![name.to_string()]);
    }
    mb.repair();
    let (a, b) = (id_of(&mb, "a@example.org"), id_of(&mb, "b@example.org"));
    assert_eq!(mb.categories_of(a), [tagged("Mine", RED)]);
    assert_eq!(mb.categories_of(b), [tagged("Theirs", RED)]);
    assert_eq!(mb.server_categories[&a], ["Mine"]);
    // Nothing is left under the old ids, where 0 would stand for the shared list.
    assert_eq!(mb.categories_of(0), shared.as_slice());
    assert_eq!(mb.account_categories.keys().copied().collect::<Vec<_>>(), [a, b]);
    assert_eq!(mb.server_categories.keys().copied().collect::<Vec<_>>(), [a, b]);
}

#[test]
fn a_second_account_with_an_id_doesnt_take_the_firsts_list() {
    let (mut mb, a, b) = two_accounts();
    mb.accounts.iter_mut().find(|x| x.id == b).unwrap().id = a;
    mb.account_categories.insert(a, vec![tagged("Mine", RED)]);
    mb.repair();
    let other = mb.accounts.iter().find(|x| x.email == "b@example.org").unwrap().id;
    assert_ne!(other, a);
    assert_eq!(mb.categories_of(a), [tagged("Mine", RED)]);
    assert!(!mb.account_categories.contains_key(&other));
}

#[test]
fn a_first_read_doesnt_build_a_list_repair_would_cut() {
    let (mut mb, a, _) = two_accounts();
    mb.categories = (0..MAX_CATEGORIES).map(|i| tagged(&format!("c{i}"), RED)).collect();
    let folder = mb.folder_of_kind(a, FolderKind::Inbox).unwrap();
    mb.add_message(Message { folder, categories: vec!["carried".into()], ..Default::default() }).unwrap();
    mb.start_account_categories(a, &[]);
    assert!(mb.categories_of(a).len() <= MAX_CATEGORIES);
}

/// A rule of account A that categorizes new mail and moves it into a folder of account B, in
/// either order: the category goes on B's list, the list of the account the message is in now,
/// and not on A's.
#[test]
fn a_rule_moving_mail_into_another_account_puts_its_category_on_that_accounts_list() {
    for move_first in [true, false] {
        let (mut mb, a, b) = two_accounts();
        mb.account_categories.insert(a, Vec::new());
        mb.account_categories.insert(b, Vec::new());
        let into = mb.add_folder(b, None, "Kept for B").unwrap();
        let (moving, categorizing) = (Action::MoveTo(into), Action::Categorize("Zeta".into()));
        let actions = if move_first { vec![moving, categorizing] } else { vec![categorizing, moving] };
        let conditions = vec![Condition::SubjectContains(vec!["Weekly".into()])];
        mb.rules.push(Rule { enabled: true, account: a, conditions, actions, ..Default::default() });
        let clock = crate::RuleClock { now: 0, zone: crate::time::Zone::fixed(0).unwrap(), due_hour: 17 };
        let m = message_of(&mut mb, a, &[]);
        mb.message_mut(m).unwrap().subject = "Weekly".into();
        mb.apply_rules(m, &clock);
        let moved = mb.messages.values().find(|x| x.subject == "Weekly").unwrap();
        assert_eq!((moved.account, moved.categories.as_slice()), (b, ["Zeta".to_string()].as_slice()));
        let on = |acc: Id| mb.categories_of(acc).iter().any(|c| c.name == "Zeta");
        assert_eq!((on(a), on(b)), (false, true), "move first: {move_first}");
    }
}

/// A rule of Microsoft account A that categorizes new mail and copies it into a folder of
/// Microsoft account C, which has a list of its own: the copy lands in C with the category, and
/// the category is on C's list, as it is for a move there (#566).
#[test]
fn a_rule_copying_mail_into_another_account_puts_its_category_on_that_accounts_list() {
    let (mut mb, a, c) = two_accounts();
    mb.account_categories.insert(a, Vec::new());
    mb.account_categories.insert(c, Vec::new());
    let into = mb.add_folder(c, None, "Copied for C").unwrap();
    let conditions = vec![Condition::SubjectContains(vec!["Weekly".into()])];
    let actions = vec![Action::Categorize("Eta".into()), Action::CopyTo(into)];
    mb.rules.push(Rule { enabled: true, account: a, conditions, actions, ..Default::default() });
    let clock = crate::RuleClock { now: 0, zone: crate::time::Zone::fixed(0).unwrap(), due_hour: 17 };
    let m = message_of(&mut mb, a, &[]);
    mb.message_mut(m).unwrap().subject = "Weekly".into();
    mb.apply_rules(m, &clock);
    let copy = mb.messages.values().find(|x| x.subject == "Weekly" && x.account == c).unwrap();
    assert_eq!(copy.categories, ["Eta".to_string()]);
    let on = |acc: Id| mb.categories_of(acc).iter().any(|c| c.name == "Eta");
    assert_eq!((on(a), on(c)), (true, true), "on A's list / on C's list");
}

/// A rule for every account, which a mailbox from before rules were each for one account may hold,
/// follows a rename on the list IMAP accounts and this computer share, and not one on a Microsoft
/// account's own list; a rule for that Microsoft account follows its list (#566).
#[test]
fn a_rename_follows_into_the_rules_for_the_mail_that_uses_that_list() {
    let (mut mb, a, _) = two_accounts();
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    mb.categories = vec![tagged("Red", RED)];
    let rule = |account: Id| Rule {
        enabled: true,
        account,
        conditions: vec![Condition::SubjectContains(vec!["Weekly".into()])],
        actions: vec![Action::Categorize("Red".into())],
        ..Default::default()
    };
    mb.rules = vec![rule(0), rule(a)];
    let named = |mb: &Mailbox| mb.rules.iter().map(|r| r.actions.clone()).collect::<Vec<_>>();
    mb.rename_category(a, "Red", "Urgent").unwrap();
    assert_eq!(named(&mb), [vec![Action::Categorize("Red".into())], vec![Action::Categorize("Urgent".into())]]);
    let shared = mb.accounts.iter().find(|x| mb.category_list_owner(x.id).is_none()).map(|x| x.id).unwrap();
    mb.rename_category(shared, "Red", "Crimson").unwrap();
    assert_eq!(named(&mb), [vec![Action::Categorize("Crimson".into())], vec![Action::Categorize("Urgent".into())]]);
}

/// The clock the rules run by below.
fn weekly_clock() -> crate::RuleClock {
    crate::RuleClock { now: 0, zone: crate::time::Zone::fixed(0).unwrap(), due_hour: 17 }
}

/// New mail "Weekly" arrives in `account`'s Inbox carrying `categories` (what its server's rule
/// put on), and the rules run on it: whether it is still out of Deleted Items after them.
fn weekly_arrives(mb: &mut Mailbox, account: Id, categories: &[&str]) -> bool {
    let m = message_of(mb, account, categories);
    mb.message_mut(m).unwrap().subject = "Weekly".into();
    mb.apply_rules(m, &weekly_clock());
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    mb.message(m).is_some_and(|x| x.folder != deleted)
}

/// A rule of `account` (0: every account's) for mail "Weekly", with no actions yet.
fn weekly_rule(mb: &mut Mailbox, name: &str, account: Id) -> Rule {
    Rule {
        id: mb.alloc_id(),
        name: name.into(),
        enabled: true,
        account,
        conditions: vec![Condition::SubjectContains(vec!["Weekly".into()])],
        ..Default::default()
    }
}

/// A rule for every account's mail (from before rules had accounts) puts "Red" on new mail. A
/// Microsoft account A, whose own list has "Red" too (#633 started it from the shared list),
/// deletes such mail except if it has the category Red. The owner renames Red to
/// Crimson on the list IMAP accounts and this computer share: A's list keeps Red, and so does
/// the rule's Categorize, so A's new mail is left alone as before (#566).
#[test]
fn a_rename_on_the_shared_list_leaves_a_microsoft_accounts_mail_as_it_was() {
    let (mut mb, a, _) = two_accounts();
    mb.categories = vec![tagged("Red", RED)];
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    let mut tag = weekly_rule(&mut mb, "Tag", 0);
    tag.actions = vec![Action::Categorize("Red".into())];
    let mut purge = weekly_rule(&mut mb, "Purge", a);
    purge.exceptions = vec![Condition::Category(vec!["Red".into()])];
    purge.actions = vec![Action::Delete];
    mb.rules = vec![tag, purge];
    assert!(weekly_arrives(&mut mb, a, &[]), "deleted before the rename");
    let shared = mb.accounts.iter().find(|x| mb.category_list_owner(x.id).is_none()).map(|x| x.id).unwrap();
    mb.rename_category(shared, "Red", "Crimson").unwrap();
    assert!(mb.category_of(a, "Red").is_some(), "A's own list keeps Red");
    assert!(weekly_arrives(&mut mb, a, &[]), "A's new mail deleted after a rename on another list; the rules are now {:?}", mb.rules);
}

/// The same every-account rule. A's own rule files mail with the category Red and stops the
/// rules after it; a later rule of A deletes "Weekly". The owner renames Red to
/// Urgent on A's own list: the every-account rule still puts Red on A's mail, and A's rule looks
/// for both names, so it still stops the delete (#566).
#[test]
fn a_rename_on_a_microsoft_accounts_list_keeps_its_stop_rule_holding() {
    let (mut mb, a, _) = two_accounts();
    mb.categories = vec![tagged("Red", RED)];
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    let keep_in = mb.add_folder(a, None, "Kept").unwrap();
    let mut tag = weekly_rule(&mut mb, "Tag", 0);
    tag.actions = vec![Action::Categorize("Red".into())];
    let mut keep = weekly_rule(&mut mb, "Keep", a);
    keep.conditions = vec![Condition::Category(vec!["Red".into()])];
    keep.actions = vec![Action::MoveTo(keep_in)];
    keep.stop = true;
    let mut purge = weekly_rule(&mut mb, "Purge", a);
    purge.actions = vec![Action::Delete];
    mb.rules = vec![tag, keep, purge];
    assert!(weekly_arrives(&mut mb, a, &[]), "deleted before the rename");
    mb.rename_category(a, "Red", "Urgent").unwrap();
    assert!(weekly_arrives(&mut mb, a, &[]), "A's new mail deleted after the rename; the rules are now {:?}", mb.rules);
}

/// One account alone. A's server keeps a rule that puts Red on new mail (it keeps the name after
/// a rename here, #661); A's rule on this computer flags mail with the category Red and stops the
/// rules after it; a later one deletes "Weekly". The owner renames Red to Urgent on
/// A's list: the server's rule still puts Red on, and A's rule here looks for both names, so the
/// mail the server tagged is still kept (#566).
#[test]
fn a_rename_keeps_a_stop_rule_holding_for_mail_the_servers_rule_tagged() {
    let (mut mb, a, _) = two_accounts();
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    let mut on_server = weekly_rule(&mut mb, "Tag on the server", a);
    on_server.actions = vec![Action::Categorize("Red".into())];
    on_server.keep = crate::Keep::Server;
    on_server.server = Some(crate::ServerRule { id: "rule-1".into(), sequence: 1, synced_enabled: true, synced_sequence: 1, ..Default::default() });
    let mut keep = weekly_rule(&mut mb, "Keep", a);
    keep.conditions = vec![Condition::Category(vec!["Red".into()])];
    keep.actions = vec![Action::Flag];
    keep.stop = true;
    let mut purge = weekly_rule(&mut mb, "Purge", a);
    purge.actions = vec![Action::Delete];
    mb.rules = vec![on_server, keep, purge];
    assert!(weekly_arrives(&mut mb, a, &["Red"]), "deleted before the rename");
    mb.rename_category(a, "Red", "Urgent").unwrap();
    assert_eq!(mb.rules[0].actions, [Action::Categorize("Red".into())], "the server's rule keeps the old name");
    assert!(weekly_arrives(&mut mb, a, &["Red"]), "mail the server tagged deleted after the rename; the rules are now {:?}", mb.rules);
}

/// A rule for every account's mail deletes new mail with the category Red. Microsoft
/// account A has its own list (Red); an IMAP account uses the shared list, which has Red and a
/// category of its own, Archive. A rename of Red to Archive on A's list is refused: the rule would
/// then take the IMAP account's own Archive mail too (#566).
#[test]
fn a_rename_on_one_list_doesnt_make_an_every_account_rule_take_another_lists_category() {
    let (mut mb, a, _) = two_accounts();
    let imap = mb.add_account(Account { email: "i@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    mb.categories = vec![tagged("Red", RED), tagged("Archive", WINE)];
    mb.account_categories.insert(a, vec![tagged("Red", RED)]);
    let mut purge = weekly_rule(&mut mb, "Purge red", 0);
    purge.conditions.push(Condition::Category(vec!["Red".into()]));
    purge.actions = vec![Action::Delete];
    mb.rules = vec![purge];
    let refused = mb.rename_category(a, "Red", "Archive");
    assert!(refused.as_ref().is_err_and(|e| e.to_string().contains("other accounts have a category")), "{refused:?}");
    assert_eq!(mb.categories_of(a)[0].name, "Red", "nothing changed");
    assert!(weekly_arrives(&mut mb, imap, &["Archive"]), "the IMAP account's own Archive mail deleted");
}

/// The same the other way: a rename on the shared list (Red to Archive) while Microsoft account
/// A's own list has an Archive of its own.
#[test]
fn a_rename_on_the_shared_list_doesnt_make_an_every_account_rule_take_a_microsoft_accounts_category() {
    let (mut mb, a, _) = two_accounts();
    let imap = mb.add_account(Account { email: "i@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    mb.categories = vec![tagged("Red", RED)];
    mb.account_categories.insert(a, vec![tagged("Archive", WINE)]);
    let mut purge = weekly_rule(&mut mb, "Purge red", 0);
    purge.conditions.push(Condition::Category(vec!["Red".into()]));
    purge.actions = vec![Action::Delete];
    mb.rules = vec![purge];
    assert!(mb.rename_category(imap, "Red", "Archive").is_err());
    assert!(weekly_arrives(&mut mb, a, &["Archive"]), "A's own Archive mail deleted");
}

/// A rule for every account's mail puts Red on new mail "Weekly"; Microsoft account A's own list
/// has Red and a category of its own, Archive, and A's rule deletes "Weekly" with Archive. The owner renames Red to Important on A's list, then Red to Archive on the shared
/// list: that one is refused, else A's new "Weekly" would get A's own Archive and be deleted.
#[test]
fn a_rename_on_the_shared_list_doesnt_tag_a_microsoft_accounts_mail_into_its_own_other_category() {
    let (mut mb, a, _) = two_accounts();
    mb.categories = vec![tagged("Red", RED)];
    mb.account_categories.insert(a, vec![tagged("Red", RED), tagged("Archive", WINE)]);
    let mut tag = weekly_rule(&mut mb, "Tag", 0);
    tag.actions = vec![Action::Categorize("Red".into())];
    let mut purge = weekly_rule(&mut mb, "Purge archived", a);
    purge.conditions.push(Condition::Category(vec!["Archive".into()]));
    purge.actions = vec![Action::Delete];
    mb.rules = vec![tag, purge];
    assert!(weekly_arrives(&mut mb, a, &[]), "deleted before any rename");
    mb.rename_category(a, "Red", "Important").unwrap();
    let shared = mb.accounts.iter().find(|x| mb.category_list_owner(x.id).is_none()).map(|x| x.id).unwrap();
    assert!(mb.rename_category(shared, "Red", "Archive").is_err());
    assert_eq!(mb.rules[0].actions, [Action::Categorize("Red".into())]);
    assert!(weekly_arrives(&mut mb, a, &[]), "A's new mail deleted");
}

/// The list holds "Spam" and "büro"; a rule deletes "Weekly" with the category Spam. A rename of
/// Spam to "BÜRO", which differs from "büro" only in the case of a letter outside A to Z, is
/// refused as a name already there: the rule, which follows a rename and tells names apart in
/// every script, would else take the mail of "büro" too.
#[test]
fn a_rename_into_another_categorys_case_outside_a_to_z_is_refused() {
    let (mut mb, a, _) = two_accounts();
    mb.categories = vec![tagged("Spam", RED), tagged("büro", WINE)];
    let mut purge = weekly_rule(&mut mb, "Purge spam", a);
    purge.conditions.push(Condition::Category(vec!["Spam".into()]));
    purge.actions = vec![Action::Delete];
    mb.rules = vec![purge];
    assert!(weekly_arrives(&mut mb, a, &["büro"]), "büro mail deleted before the rename");
    let renamed = mb.rename_category(a, "Spam", "BÜRO");
    assert!(renamed.is_err(), "{renamed:?}");
    assert_eq!(mb.rules[0].conditions.last(), Some(&Condition::Category(vec!["Spam".into()])));
    assert!(weekly_arrives(&mut mb, a, &["büro"]), "büro mail deleted after the rename");
    // A category's own case changes, in any script, as before.
    mb.categories = vec![tagged("Spam", RED), tagged("büro", WINE)];
    assert!(mb.rename_category(a, "büro", "BÜRO").is_ok());
}

/// An IMAP account's rule puts Red on its mail and moves it into a folder of Microsoft account A,
/// whose own list has Archive, which A's rule deletes. A rename of Red to Archive on the shared
/// list would have the rule put A's own Archive on the mail it moves there: refused, as for a rule
/// for every account, and the rule keeps Red (#566). The other way too: A's rule copying into the
/// IMAP account's folder, renamed on A's list into a name the shared list has.
#[test]
fn a_rename_a_rules_move_would_put_on_another_accounts_own_category_is_refused() {
    let (mut mb, a, _) = two_accounts();
    let imap = mb.add_account(Account { email: "i@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    mb.categories = vec![tagged("Red", RED), tagged("Kept", RED)];
    mb.account_categories.insert(a, vec![tagged("Archive", WINE), tagged("Blue", WINE)]);
    let into = mb.add_folder(a, None, "From IMAP").unwrap();
    let mut tag = weekly_rule(&mut mb, "Tag and file", imap);
    tag.actions = vec![Action::Categorize("Red".into()), Action::MoveTo(into)];
    let back = mb.add_folder(imap, None, "From A").unwrap();
    let mut copy = weekly_rule(&mut mb, "Tag and copy", a);
    copy.actions = vec![Action::Categorize("Blue".into()), Action::CopyTo(back)];
    mb.rules = vec![tag, copy];
    let refused = mb.rename_category(imap, "Red", "Archive");
    assert!(refused.as_ref().is_err_and(|e| e.to_string().contains("of its own")), "{refused:?}");
    assert_eq!(mb.rules[0].actions[0], Action::Categorize("Red".into()));
    assert!(mb.rename_category(a, "Blue", "kept").is_err(), "A's rule would put the shared list's Kept on the copy");
    // A name no list it reaches has is fine.
    assert!(mb.rename_category(imap, "Red", "Orange").is_ok());
    assert_eq!(mb.rules[0].actions[0], Action::Categorize("Orange".into()));
}
