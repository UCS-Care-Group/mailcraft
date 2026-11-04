// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's list of categories kept in step with its server both ways (#661): what
//! is sent, what the server's answers and lists do here, read-only lists, undo.

use mailcraft_model::remote::{
    CategoriesDownload, CategoriesUploaded, CategoryChange, CategoryChangeKind, CategoryOutcome, CategoryResult, EventOutcome, EventResult,
    EventsUploaded,
};
use mailcraft_model::remote_tasks::{TaskOutcome, TaskResult, TasksUploaded};
use mailcraft_model::sync::{MessageChange, MessageChangeDone, MessageOutcome, ServerMessage};
use mailcraft_model::{Calendar, Category, Contact, Event, FolderKind, GREY_CATEGORY, Id, Mailbox, Message, Origin, Rgb, Task, TaskFields};
use serde_json::json;

use crate::Session;
use crate::agent::{Class, Verdict, check};
use crate::cmd::{graph_up, remote, tasks_remote};

const NOW: i64 = 1_800_000_000;
/// Outlook's red, green and blue (`preset0`, `preset4`, `preset7`).
const RED: Rgb = Rgb(0xD1, 0x3B, 0x3B);
const GREEN: Rgb = Rgb(0x2E, 0x8B, 0x57);
const BLUE: Rgb = Rgb(0x2F, 0x6F, 0xD0);

fn tag(name: &str, color: Rgb) -> Category {
    Category { name: name.into(), color }
}

/// A Microsoft account whose server lists `on_server`, read once; the shared list is empty.
fn rig(on_server: Vec<Category>, writes: bool) -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories.clear();
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    read(&mut s, a, on_server);
    if writes {
        s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    }
    (s, a)
}

fn read(s: &mut Session, account: Id, categories: Vec<Category>) {
    let download = CategoriesDownload { account, email: "a@example.org".into(), categories, complete: true };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
}

fn changes(s: &mut Session, account: Id) -> Vec<CategoryChange> {
    let upload = s.run("sync.categoryChanges", &json!({"account": account})).unwrap();
    serde_json::from_value(upload["changes"].clone()).unwrap()
}

fn change(kind: CategoryChangeKind, name: &str, color: Rgb) -> CategoryChange {
    CategoryChange { kind, name: name.into(), color }
}

/// The server took every change of `sent`.
fn took(s: &mut Session, account: Id, sent: &[CategoryChange]) {
    let outcomes = sent
        .iter()
        .map(|c| CategoryOutcome {
            kind: c.kind,
            name: c.name.clone(),
            color: c.color,
            result: CategoryResult::Done,
            error: String::new(),
            found: false,
        })
        .collect();
    let result = CategoriesUploaded { account, email: "a@example.org".into(), outcomes, ..Default::default() };
    s.run("sync.applyCategoryChanges", &json!({"result": result})).unwrap();
}

fn answer(s: &mut Session, account: Id, kind: CategoryChangeKind, name: &str, color: Rgb, result: CategoryResult) -> serde_json::Value {
    let outcome = CategoryOutcome { kind, name: name.into(), color, result, error: String::new(), found: false };
    let result = CategoriesUploaded { account, email: "a@example.org".into(), outcomes: vec![outcome], ..Default::default() };
    s.run("sync.applyCategoryChanges", &json!({"result": result})).unwrap()
}

fn names(s: &Session, account: Id) -> Vec<String> {
    s.mailbox.categories_of(account).iter().map(|c| c.name.clone()).collect()
}

fn message_of(s: &mut Session, account: Id, categories: &[&str]) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let categories = categories.iter().map(|c| c.to_string()).collect();
    s.mailbox.add_message(Message { folder, categories, ..Default::default() }).unwrap()
}

#[test]
fn a_list_whose_server_may_not_be_changed_is_read_only_whichever_way_it_is_asked() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], false);
    let m = message_of(&mut s, a, &[]);
    let before = s.mailbox.categories_of(a).to_vec();
    for (command, params) in [
        ("category.set", json!({"account": a, "name": "New", "color": "#2E8B57"})),
        ("category.set", json!({"account": a, "name": "Red category", "color": "#2E8B57"})),
        ("category.rename", json!({"account": a, "from": "Red category", "to": "Urgent"})),
        ("category.remove", json!({"account": a, "name": "Red category"})),
        // A name nobody defined can't join the list either.
        ("mail.categorize", json!({"id": m, "category": "Brand new"})),
    ] {
        let refused = s.run(command, &params).unwrap_err().to_string();
        assert!(refused.contains("a@example.org") && refused.contains("no permission"), "{command}: {refused}");
    }
    assert_eq!(s.mailbox.categories_of(a), before);
    // A category of the list goes on an item as ever.
    s.run("mail.categorize", &json!({"id": m, "category": "red category"})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Red category"]);
    // Nothing waits to go, and the list says so.
    assert!(changes(&mut s, a).is_empty());
    assert_eq!(s.run("category.list", &json!({"account": a})).unwrap()["readOnly"], json!(true));
}

#[test]
fn what_is_changed_here_is_the_difference_from_the_servers_list_and_a_deletion_waits_on_what_is_made() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", RED), tag("client", GREEN)], true);
    assert!(changes(&mut s, a).is_empty(), "a read leaves nothing to send");
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "red category", "color": "#2F6FD0"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    // A name in other letters only is deleted and made again side by side; a deletion waits
    // while anything is to be made.
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    assert_eq!(
        changes(&mut s, a),
        [
            change(CategoryChangeKind::Delete, "client", GREEN),
            change(CategoryChangeKind::Create, "Client", GREEN),
            change(CategoryChangeKind::Create, "Travel", GREEN),
            change(CategoryChangeKind::Color, "Red category", BLUE),
        ]
    );
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", RED)]);
    // Taken: nothing is left to send, and a read of the same list changes nothing.
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert!(changes(&mut s, a).is_empty());
    let server = vec![tag("Red category", BLUE), tag("Client", GREEN), tag("Travel", GREEN)];
    read(&mut s, a, server);
    assert_eq!(names(&s, a), ["Red category", "Client", "Travel"]);
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn a_read_follows_the_server_for_what_wasnt_changed_here_and_keeps_what_waits() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Gone there", RED), tag("Mine", RED), tag("Kept", RED)], true);
    let m = message_of(&mut s, a, &["Gone there", "Kept"]);
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Kept"})).unwrap();
    // Outlook meanwhile: Red recoloured, Gone there deleted, Mine and Kept as they were, one new,
    // Red category written otherwise.
    read(&mut s, a, vec![tag("RED CATEGORY", BLUE), tag("Mine", RED), tag("Kept", RED), tag("From Outlook", GREEN)]);
    assert_eq!(names(&s, a), ["RED CATEGORY", "Mine", "From Outlook"]);
    assert_eq!(s.mailbox.category_color_of(a, "red category"), BLUE);
    assert_eq!(s.mailbox.category_color_of(a, "Mine"), GREEN, "changed here, not sent: stays");
    // A deletion there wins; what carries the name keeps it, in grey.
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Gone there"]);
    assert_eq!(s.mailbox.category_color_of(a, "Gone there"), GREY_CATEGORY);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Kept", RED), change(CategoryChangeKind::Color, "Mine", GREEN)]);
}

#[test]
fn a_long_list_still_takes_the_servers_colours_and_new_names() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    // More than 400 here (the shared list came across, say), kept on this computer.
    let mut long = s.mailbox.categories_of(a).to_vec();
    let mut local = Vec::new();
    for i in 0..450 {
        long.push(tag(&format!("Here {i}"), GREEN));
        local.push(format!("Here {i}"));
    }
    s.mailbox.account_categories.insert(a, long);
    s.mailbox.local_categories.insert(a, local);
    read(&mut s, a, vec![tag("Red category", BLUE), tag("New there", GREEN)]);
    assert_eq!(s.mailbox.category_color_of(a, "Red category"), BLUE);
    assert!(s.mailbox.category_of(a, "New there").is_some());
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn names_are_one_category_whatever_their_case_in_any_script() {
    let (mut s, a) = rig(vec![tag("Ärger", RED), tag("Проект", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "ärger", "color": "#2F6FD0"})).unwrap();
    assert_eq!(names(&s, a), ["Ärger", "Проект"], "recoloured, not added");
    let m = message_of(&mut s, a, &[]);
    s.run("mail.categorize", &json!({"id": m, "category": "проект"})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Проект"]);
    assert_eq!(names(&s, a).len(), 2);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Color, "Ärger", BLUE)]);
}

#[test]
fn what_the_server_refused_is_put_back_and_said() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2E8B57"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    // The deletion waits on what is made; when the server won't make it, the deletion is not
    // sent either, and the server's comes back with the next read.
    let said = answer(&mut s, a, CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused);
    assert!(said["messages"][0].as_str().unwrap().contains("Travel"));
    answer(&mut s, a, CategoryChangeKind::Color, "Red category", GREEN, CategoryResult::Refused);
    assert_eq!(s.mailbox.category_color_of(a, "Red category"), RED);
    assert!(s.mailbox.category_is_local(a, "Travel"), "kept on this computer only");
    assert!(changes(&mut s, a).is_empty());
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", RED)]);
    assert!(s.mailbox.category_of(a, "Old").is_some());
    // A deletion refused: back on the list.
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", RED)]);
    let said = answer(&mut s, a, CategoryChangeKind::Delete, "Old", RED, CategoryResult::Refused);
    assert!(said["messages"][0].as_str().unwrap().contains("back on the list"), "{said}");
    assert!(s.mailbox.category_of(a, "Old").is_some());
    assert!(changes(&mut s, a).is_empty());
    // Not now: it goes again.
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2F6FD0"})).unwrap();
    answer(&mut s, a, CategoryChangeKind::Color, "Red category", BLUE, CategoryResult::Failed);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Color, "Red category", BLUE)]);
    // The server says the sign-in doesn't cover it: read-only from now on.
    answer(&mut s, a, CategoryChangeKind::Color, "Red category", BLUE, CategoryResult::Denied);
    assert!(s.mailbox.categories_read_only(a).is_some());
}

#[test]
fn an_undo_of_a_change_the_server_took_sends_the_reverse() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let m = message_of(&mut s, a, &["Old"]);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // Outlook adds one meanwhile.
    read(&mut s, a, vec![tag("Red category", RED), tag("From Outlook", BLUE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(names(&s, a), ["Red category", "Old", "From Outlook"]);
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Old"]);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Old", GREEN)], "and nothing of what Outlook added");
    // Redo: deleted again, and the server is told so once it has it.
    s.run("edit.redo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "the server doesn't have it: nothing to do");
    // An undo of a change not sent yet leaves nothing to send.
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn a_category_kept_here_alone_goes_to_outlook_only_when_asked() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = vec![tag("Old local", Rgb(0xC2, 0x4E, 0x9A))];
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    assert!(s.mailbox.category_is_local(a, "Old local"));
    assert!(changes(&mut s, a).is_empty(), "never sent unasked");
    let list = s.run("category.list", &json!({"account": a})).unwrap();
    assert_eq!(list["categories"][0]["outlook"], json!(false));
    // A rename or a deletion of one stays here.
    s.run("category.rename", &json!({"account": a, "from": "Old local", "to": "Still local"})).unwrap();
    assert!(changes(&mut s, a).is_empty());
    s.run("category.addToOutlook", &json!({"account": a, "name": "still local"})).unwrap();
    assert!(!s.mailbox.category_is_local(a, "Still local"));
    let nearest = s.mailbox.category_color_of(a, "Still local");
    assert!(mailcraft_model::outlook_colors::outlook_preset(nearest).is_some());
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Still local", nearest)]);
    // One that is Outlook's already, or none at all, is refused.
    assert!(s.run("category.addToOutlook", &json!({"account": a, "name": "Red category"})).is_err());
    assert!(s.run("category.addToOutlook", &json!({"account": a, "name": "Nobody"})).is_err());
    // And a list that isn't a server's has nothing to add.
    assert!(s.run("category.addToOutlook", &json!({"account": 1, "name": "Red category"})).is_err());
}

#[test]
fn a_list_read_before_this_version_keeps_what_was_only_here_on_this_computer() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    // As an older build saved it: the server's names, and the list, nothing else.
    s.mailbox.account_categories.insert(a, vec![tag("Red category", RED), tag("Made earlier", GREEN)]);
    s.mailbox.synced_categories.clear();
    s.mailbox.local_categories.clear();
    s.mailbox.repair();
    assert_eq!(s.mailbox.synced_categories[&a], [tag("Red category", RED)]);
    assert!(s.mailbox.category_is_local(a, "Made earlier"));
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn only_the_host_says_whether_a_list_may_be_changed() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], false);
    assert!(matches!(check(&s, "sync.categoryWrites", &json!({"account": a, "allowed": true}), &[]), Verdict::Refuse(_)));
    assert!(matches!(check(&s, "sync.applyCategoryChanges", &json!({}), &[]), Verdict::Refuse(_)));
    // Not an account's of Microsoft's, or a shared mailbox's: refused.
    assert!(s.run("sync.categoryWrites", &json!({"account": 1, "allowed": true})).is_err());
    // An agent's deletion on a list kept in step says it goes from Outlook too.
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    match check(&s, "category.remove", &json!({"account": a, "name": "Red category"}), &[]) {
        Verdict::Ask(Class::Delete, text) => assert!(text.contains("from Outlook"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_file_an_older_build_saved_opens_with_nothing_to_send() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.mailbox.categories = vec![tag("Red category", RED)];
    s.mailbox.account_categories.insert(a, vec![tag("Red category", RED), tag("Made earlier", GREEN), tag("Yellow category", BLUE)]);
    // The file as an older build writes it: none of what #661 keeps.
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    for field in ["syncedCategories", "localCategories", "categoryWrites"] {
        file.as_object_mut().unwrap().remove(field);
    }
    let mut opened = crate::io::read_mailbox(&serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(opened.category_is_local(a, "Made earlier") && opened.category_is_local(a, "Yellow category"));
    assert!(opened.categories_read_only(a).is_some(), "until the host finds the sign-in covers it");
    opened.category_writes.insert(a);
    assert!(opened.category_upload(a).is_empty(), "nothing goes to the server unasked");
}

#[test]
fn a_list_the_server_gave_in_part_takes_nothing_for_deleted_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Later", GREEN)], true);
    // A list longer than is read: "Later" isn't in this part of it.
    let download = CategoriesDownload { account: a, email: "a@example.org".into(), categories: vec![tag("Red category", BLUE)], complete: false };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
    assert_eq!(names(&s, a), ["Red category", "Later"]);
    assert_eq!(s.mailbox.category_of(a, "Red category").map(|c| c.color), Some(BLUE));
    assert!(changes(&mut s, a).is_empty(), "nothing to make again or delete: {:?}", changes(&mut s, a));
    // The whole list says it is gone.
    read(&mut s, a, vec![tag("Red category", BLUE)]);
    assert_eq!(names(&s, a), ["Red category"]);
}

/// Fill `a`'s list to the limit with categories kept on this computer alone.
fn full(s: &mut Session, a: Id) {
    let mut long = s.mailbox.categories_of(a).to_vec();
    let mut local = s.mailbox.local_categories.get(&a).cloned().unwrap_or_default();
    let mut i = 0;
    while long.len() < mailcraft_model::MAX_CATEGORIES {
        long.push(tag(&format!("Here {i}"), GREEN));
        local.push(format!("Here {i}"));
        i += 1;
    }
    s.mailbox.account_categories.insert(a, long);
    s.mailbox.local_categories.insert(a, local);
}

fn deletes(s: &mut Session, a: Id) -> Vec<String> {
    changes(s, a).into_iter().filter(|c| c.kind == CategoryChangeKind::Delete).map(|c| c.name).collect()
}

// A delete on the server can't be undone: it goes only for a name the owner took off the list
// here, never for one the list had no room for, whichever way it came to be left out. One test
// per way.

#[test]
fn one_made_in_outlook_while_the_list_is_full_is_not_deleted_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    full(&mut s, a);
    let there = vec![tag("Red category", RED), tag("Made in Outlook", GREEN)];
    read(&mut s, a, there.clone());
    assert!(s.mailbox.category_of(a, "Made in Outlook").is_none());
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    read(&mut s, a, there.clone());
    assert!(changes(&mut s, a).is_empty());
    // Room made: it comes.
    s.run("category.remove", &json!({"account": a, "name": "Here 0"})).unwrap();
    read(&mut s, a, there);
    assert!(s.mailbox.category_of(a, "Made in Outlook").is_some());
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn an_undo_with_no_room_for_what_outlook_made_meanwhile_deletes_nothing_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", RED)], true);
    full(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", RED), tag("Made in Outlook", GREEN)]);
    assert!(s.mailbox.category_of(a, "Made in Outlook").is_some());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Old").is_some());
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn a_refused_delete_with_no_room_to_put_it_back_is_not_sent_again() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", RED)], true);
    full(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert_eq!(deletes(&mut s, a), ["Old"]);
    s.mailbox.account_categories.get_mut(&a).unwrap().push(tag("Fills it", GREEN));
    s.mailbox.local_categories.get_mut(&a).unwrap().push("Fills it".into());
    let said = answer(&mut s, a, CategoryChangeKind::Delete, "Old", RED, CategoryResult::Refused);
    assert!(said["messages"][0].as_str().unwrap().contains("stays in Outlook"), "{said}");
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn a_file_whose_list_is_cut_as_it_opens_deletes_nothing_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Far", RED)], true);
    let mut long = vec![tag("Red category", RED)];
    let mut local = Vec::new();
    for i in 0..mailcraft_model::MAX_CATEGORIES - 1 {
        long.push(tag(&format!("Here {i}"), GREEN));
        local.push(format!("Here {i}"));
    }
    long.push(tag("Far", RED));
    s.mailbox.account_categories.insert(a, long);
    s.mailbox.local_categories.insert(a, local);
    assert!(changes(&mut s, a).is_empty());
    let mut opened = crate::io::read_mailbox(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert!(opened.category_of(a, "Far").is_none(), "cut");
    assert!(opened.category_upload(a).is_empty(), "{:?}", opened.category_upload(a));
    // And what is kept here alone stays so.
    opened.account_categories.get_mut(&a).unwrap().retain(|c| c.name != "Here 5");
    assert!(opened.category_upload(a).is_empty());
}

/// A download of `account`'s server for `email`.
fn read_as(s: &mut Session, account: Id, email: &str, categories: Vec<Category>) {
    let download = CategoriesDownload { account, email: email.into(), categories, complete: true };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
}

#[test]
fn a_rename_deletes_the_old_name_only_once_the_server_has_the_new_one() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    // Not made for now: the old name still waits.
    answer(&mut s, a, CategoryChangeKind::Create, "New", GREEN, CategoryResult::Failed);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    // An answer lost: the next read shows the server made it, and only then the old one goes.
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN), tag("New", GREEN)]);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);

    // The server won't make it: the old name stays there, and comes back here.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    let said = answer(&mut s, a, CategoryChangeKind::Create, "New", GREEN, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"Old\\\" wasn't deleted in Outlook"), "{said}");
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN)]);
    assert_eq!(names(&s, a), ["Red category", "New", "Old"]);
    assert!(s.mailbox.category_is_local(a, "New"));
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn a_name_that_left_the_list_is_not_kept_here_alone_so_outlook_keeps_it_when_it_comes() {
    // Its making refused after it was taken off the list again.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Travel"})).unwrap();
    answer(&mut s, a, CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused);
    assert!(!s.mailbox.category_is_local(a, "Travel"));
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", BLUE)]);
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));

    // A name that left an account's list when another account's server showed it was that one's.
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = vec![tag("Project", RED)];
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let b = s.run("account.add", &json!({"email": "b@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    read_as(&mut s, b, "b@example.org", vec![tag("Project", RED)]);
    assert!(s.mailbox.category_of(a, "Project").is_none());
    assert!(!s.mailbox.category_is_local(a, "Project"));
    read(&mut s, a, vec![tag("Red category", RED), tag("Project", RED)]);
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn a_file_from_an_older_build_takes_no_name_the_list_lacked_for_deleted() {
    // A list that had no room for one, and one the owner took off it there (which stayed here).
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Far", RED), tag("Gone", GREEN)], true);
    let mut long = vec![tag("Red category", RED)];
    for i in 0..450 {
        long.push(tag(&format!("Here {i}"), GREEN));
    }
    s.mailbox.account_categories.insert(a, long);
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    for field in ["syncedCategories", "localCategories", "categoryWrites"] {
        file.as_object_mut().unwrap().remove(field);
    }
    file["serverCategories"] = json!({a.to_string(): ["Red category", "Far", "Gone"]});
    let mut opened = crate::io::read_mailbox(&serde_json::to_vec(&file).unwrap()).unwrap();
    opened.category_writes.insert(a);
    assert!(opened.category_upload(a).is_empty(), "{:?}", opened.category_upload(a));
    // What is kept here alone is on the list.
    let list = opened.categories_of(a).to_vec();
    assert!(opened.local_categories.get(&a).into_iter().flatten().all(|n| list.iter().any(|c| &c.name == n)));
}

#[test]
fn a_read_never_takes_a_name_kept_here_alone_off_the_list_for_a_deletion() {
    // However it came to be kept here alone without being on the list.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.mailbox.local_categories.insert(a, vec!["Ghost".into()]);
    read(&mut s, a, vec![tag("Red category", RED), tag("Ghost", GREEN)]);
    assert!(!s.mailbox.category_is_local(a, "Ghost"));
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn what_is_kept_here_alone_and_no_longer_on_the_list_is_let_go_of_as_a_file_opens() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.mailbox.local_categories.insert(a, vec!["Ghost".into()]);
    let opened = crate::io::read_mailbox(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert!(!opened.category_is_local(a, "Ghost"));
}

#[test]
fn an_undo_of_a_colour_keeps_outlooks_spelling_and_sends_the_colour_alone() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "client", "color": "#2F6FD0"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // Outlook writes it otherwise meanwhile.
    read(&mut s, a, vec![tag("Red category", RED), tag("Client", BLUE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Color, "Client", GREEN)]);

    // An undo of a name written otherwise here does write it back, and so deletes and makes.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Client", GREEN), change(CategoryChangeKind::Create, "client", GREEN)]);
}

#[test]
fn another_address_is_another_mailbox_and_nothing_of_the_old_ones_list_goes_to_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert!(!changes(&mut s, a).is_empty());
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    assert!(changes(&mut s, a).is_empty());
    // The new server's list: what it has is its, the rest stays on this computer.
    read_as(&mut s, a, "other@example.org", vec![tag("Red category", BLUE), tag("Theirs", GREEN)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert!(s.mailbox.category_is_local(a, "Travel"));
    assert!(s.mailbox.category_of(a, "Theirs").is_some());

    // Undone: the old address, and what waited for its server waits again; redone: nothing.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "a@example.org");
    assert_eq!(names(&s, a), ["Red category"]);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);
    s.run("edit.redo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    read_as(&mut s, a, "other@example.org", vec![tag("Theirs", GREEN)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert_eq!(names(&s, a), ["Red category", "Theirs"]);
}

#[test]
fn a_list_cut_to_size_is_not_taken_for_the_servers_whole_list() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Keep", GREEN)], true);
    let mut long: Vec<Category> = (0..mailcraft_model::remote::MAX_REMOTE_CATEGORIES).map(|i| tag(&format!("There {i}"), GREEN)).collect();
    long.push(tag("Red category", RED));
    long.push(tag("Keep", GREEN));
    read(&mut s, a, long);
    assert!(s.mailbox.category_of(a, "Keep").is_some());
    assert!(s.mailbox.category_of(a, "Red category").is_some());
}

fn outcome(kind: CategoryChangeKind, name: &str, color: Rgb, result: CategoryResult) -> CategoryOutcome {
    CategoryOutcome { kind, name: name.into(), color, result, error: String::new(), found: false }
}

fn answers(s: &mut Session, account: Id, outcomes: Vec<CategoryOutcome>) -> serde_json::Value {
    let result = CategoriesUploaded { account, email: "a@example.org".into(), outcomes, ..Default::default() };
    s.run("sync.applyCategoryChanges", &json!({"result": result})).unwrap()
}

/// A Microsoft account whose list came across with "Old local", kept on this computer only.
fn rig_with_local() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = vec![tag("Old local", GREEN)];
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    assert!(s.mailbox.category_is_local(a, "Old local"));
    (s, a)
}

#[test]
fn an_undo_never_deletes_in_outlook_one_of_outlooks_own_that_shares_a_name_kept_here() {
    // A removal undone after Outlook made one of that name.
    let (mut s, a) = rig_with_local();
    s.run("category.remove", &json!({"account": a, "name": "Old local"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
    assert!(!s.mailbox.category_is_local(a, "Old local"), "Outlook's now");
    // A rename undone after the same.
    let (mut s, a) = rig_with_local();
    s.run("category.rename", &json!({"account": a, "from": "Old local", "to": "Still local"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
    // But an undo of Add to Outlook takes it off Outlook again, once Outlook has it.
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Old local"));
    assert_eq!(deletes(&mut s, a), ["Old local"]);
}

#[test]
fn an_undo_after_a_new_address_was_read_deletes_nothing_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let m = message_of(&mut s, a, &[]);
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    // Anything that can be undone, before the new address's list is read.
    s.run("mail.flag", &json!({"id": m})).unwrap();
    read_as(&mut s, a, "other@example.org", vec![tag("Red category", RED), tag("Old", GREEN), tag("Theirs", BLUE)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "other@example.org");
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_of_a_colour_after_outlook_deleted_the_category_leaves_it_deleted() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Theirs", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "Theirs", "color": "#2F6FD0"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Theirs").is_none());
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

#[test]
fn other_letters_the_server_wont_take_go_back_to_the_servers_and_nothing_is_deleted_alone() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Client", "color": "#2F6FD0"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "client", GREEN), change(CategoryChangeKind::Create, "Client", BLUE)]);
    let said = answers(
        &mut s,
        a,
        vec![
            outcome(CategoryChangeKind::Delete, "client", GREEN, CategoryResult::Refused),
            outcome(CategoryChangeKind::Create, "Client", BLUE, CategoryResult::Refused),
        ],
    );
    assert!(said["messages"].to_string().contains("new spelling"), "{said}");
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
    assert_eq!(s.mailbox.category_of(a, "client").map(|c| c.name.as_str()), Some("client"));
    assert!(!s.mailbox.category_is_local(a, "client"));
}

#[test]
fn a_deletion_sent_in_the_same_job_as_a_refused_making_is_answered_by_its_own_answer() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN), tag("Old", RED)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert_eq!(
        changes(&mut s, a),
        [
            change(CategoryChangeKind::Delete, "client", GREEN),
            change(CategoryChangeKind::Create, "Client", GREEN),
            change(CategoryChangeKind::Delete, "Old", RED)
        ]
    );
    let said = answers(
        &mut s,
        a,
        vec![
            outcome(CategoryChangeKind::Delete, "client", GREEN, CategoryResult::Done),
            outcome(CategoryChangeKind::Create, "Client", GREEN, CategoryResult::Refused),
            outcome(CategoryChangeKind::Delete, "Old", RED, CategoryResult::Failed),
        ],
    );
    assert!(!said["messages"].to_string().contains("wasn't deleted"), "{said}");
    // Not deleted for now: it goes again.
    assert_eq!(deletes(&mut s, a), ["Old"]);
}

#[test]
fn another_address_by_way_of_another_kind_is_another_mailbox_too() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    s.run("account.edit", &json!({"id": a, "kind": "imap", "email": "other@example.org"})).unwrap();
    s.run("account.edit", &json!({"id": a, "kind": "microsoft"})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "other@example.org");
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

#[test]
fn an_undo_of_other_letters_half_done_there_makes_the_old_letters_again() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "client", GREEN), change(CategoryChangeKind::Create, "Client", GREEN)]);
    // The old spelling is deleted there; the new one fails for now (it goes with the next job).
    answers(
        &mut s,
        a,
        vec![
            outcome(CategoryChangeKind::Delete, "client", GREEN, CategoryResult::Done),
            outcome(CategoryChangeKind::Create, "Client", GREEN, CategoryResult::Failed),
        ],
    );
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Client", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "client").is_some(), "gone here: {:?}", names(&s, a));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "client", GREEN)]);
}

#[test]
fn two_undos_in_a_row_put_back_the_category_as_it_was_first() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Theirs", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "Theirs", "color": "#2F6FD0"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Theirs"})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Delete, "Theirs", GREEN)]);
    took(&mut s, a, &sent);
    // The removal undone: it comes back, blue, and is to be made there again.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Theirs", BLUE)]);
    // The colour undone too, before the job.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.category_of(a, "Theirs").map(|c| c.color), Some(GREEN), "gone here: {:?}", names(&s, a));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Theirs", GREEN)]);
}

#[test]
fn an_undo_of_add_to_outlook_after_outlook_made_one_of_that_name_deletes_nothing() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    // Before the job: a read shows Outlook has one of that name, made there meanwhile.
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    assert!(changes(&mut s, a).iter().all(|c| c.kind != CategoryChangeKind::Create), "{:?}", changes(&mut s, a));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn a_redo_of_removing_one_kept_here_after_outlook_made_one_of_that_name_deletes_nothing() {
    let (mut s, a) = rig_with_local();
    s.run("category.remove", &json!({"account": a, "name": "Old local"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Old local"));
    // Outlook makes one of that name; a read brings it: it is Outlook's.
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    assert!(!s.mailbox.category_is_local(a, "Old local"));
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    s.run("edit.redo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_of_a_new_category_after_outlook_made_one_of_that_name_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    // Before the job: Outlook has one of that name, made there meanwhile.
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", BLUE)]);
    assert!(changes(&mut s, a).iter().all(|c| c.kind != CategoryChangeKind::Create), "{:?}", changes(&mut s, a));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_of_removing_one_kept_here_sends_nothing_to_outlooks_own() {
    let (mut s, a) = rig_with_local();
    s.run("category.remove", &json!({"account": a, "name": "Old local"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

#[test]
fn an_undo_of_other_letters_after_outlook_wrote_it_otherwise_again_leaves_outlooks() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("CLIENT", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.category_of(a, "client").map(|c| c.name.as_str()), Some("CLIENT"));
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

/// The last of the names [`full`] kept on this computer alone.
fn last_here(s: &Session, a: Id) -> String {
    s.mailbox.categories_of(a).iter().rev().find(|c| c.name.starts_with("Here ")).map(|c| c.name.clone()).unwrap()
}

#[test]
fn an_undo_at_the_limit_never_drops_a_category_kept_here_alone() {
    // What the list grew by since is W, which Outlook made again after its deletion went: it was
    // on the server's list as the step found it, so it is no addition of the server's since.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("W", RED), tag("X", RED)], true);
    full(&mut s, a);
    let m = message_of(&mut s, a, &[]);
    let here = last_here(&s, a);
    let locals = s.mailbox.local_categories[&a].len();
    s.run("category.remove", &json!({"account": a, "name": "W"})).unwrap();
    // Put on the list by no step of the owner's (a rule, say): a full list takes no new name
    // from a command.
    assert!(s.run("mail.categorize", &json!({"id": m, "category": "V"})).is_err());
    s.mailbox.set_category(a, "V", GREEN).unwrap();
    let sent = changes(&mut s, a);
    let v = sent.iter().find(|c| c.name == "V").unwrap().color;
    took(&mut s, a, &sent);
    s.run("category.remove", &json!({"account": a, "name": "X"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("V", v), tag("W", RED)]);
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, &here).is_some(), "{here} dropped");
    assert_eq!(s.mailbox.local_categories.get(&a).map_or(0, Vec::len), locals, "a category kept here alone was dropped");
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
}

#[test]
fn a_redo_at_the_limit_never_drops_a_category_kept_here_alone() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "V", "color": "#2E8B57"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    // The list fills up meanwhile, by no step of the owner's.
    full(&mut s, a);
    let here = last_here(&s, a);
    s.run("edit.redo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, &here).is_some(), "{here} dropped");
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
}

#[test]
fn an_undo_of_something_else_keeps_a_category_put_on_a_full_list() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    full(&mut s, a);
    let m = message_of(&mut s, a, &[]);
    // Put on the full list by no step of the owner's (a rule, say).
    s.mailbox.set_category(a, "Brand new", GREY_CATEGORY).unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Brand new").is_some(), "{} on the list", s.mailbox.categories_of(a).len());
}

#[test]
fn at_the_limit_what_outlook_added_since_waits_and_its_older_ones_stay() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("X", RED), tag("W", RED)], true);
    full(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "X"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("W", RED), tag("Z", RED)]);
    // W, refused a deletion, comes back at the end of the list.
    s.run("category.remove", &json!({"account": a, "name": "W"})).unwrap();
    answer(&mut s, a, CategoryChangeKind::Delete, "W", RED, CategoryResult::Refused);
    assert_eq!(names(&s, a).last().map(String::as_str), Some("W"));
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "X").is_some());
    assert!(s.mailbox.category_of(a, "W").is_some(), "Outlook's own, there before the step, was dropped");
    assert!(s.mailbox.category_of(a, "Z").is_none(), "what Outlook added since waits");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "X", RED)]);
}

#[test]
fn an_undo_from_before_the_first_read_keeps_what_a_later_undo_put_back() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories.clear();
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN)]);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Old", GREEN)]);
    // The step from before the first read.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Old").is_some(), "the removal's undo is lost: {:?}", names(&s, a));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Old", GREEN)]);
}

#[test]
fn an_undo_of_a_new_category_its_server_refused_takes_it_away() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    answer(&mut s, a, CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused);
    assert!(s.mailbox.category_is_local(a, "Travel"));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert!(s.mailbox.category_of(a, "Travel").is_none(), "the undo left it: {:?}", names(&s, a));
}

#[test]
fn an_undo_of_a_colour_kept_here_after_outlook_made_it_in_that_colour_deletes_nothing() {
    let (mut s, a) = rig_with_local();
    s.run("category.set", &json!({"account": a, "name": "Old local", "color": "#2F6FD0"})).unwrap();
    assert!(changes(&mut s, a).is_empty());
    // Outlook makes one of that name, in that colour: a read makes it Outlook's.
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", BLUE)]);
    assert!(!s.mailbox.category_is_local(a, "Old local"));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
    assert!(s.mailbox.category_of(a, "Old local").is_some());
}

#[test]
fn at_the_limit_an_undo_never_drops_a_category_still_to_go_to_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("X", RED)], true);
    full(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "X"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // Put on the list by no step of the owner's (a rule, say), and not sent yet.
    s.mailbox.set_category(a, "V", GREEN).unwrap();
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "V").is_some(), "dropped: it was nowhere else");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "V", GREEN)]);
}

#[test]
fn at_the_limit_an_undo_of_a_rename_keeps_the_new_name_outlook_changed_since() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    full(&mut s, a);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    for _ in 0..2 {
        let sent = changes(&mut s, a);
        took(&mut s, a, &sent);
    }
    assert!(changes(&mut s, a).is_empty());
    read(&mut s, a, vec![tag("Red category", RED), tag("New", BLUE)]);
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    s.run("edit.undo", &json!({})).unwrap();
    // No room for the old name to come back: it waits, and Outlook's New stays as it is.
    assert_eq!(s.mailbox.category_color_of(a, "New"), BLUE);
    assert!(s.mailbox.category_of(a, "Old").is_none());
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

#[test]
fn an_undo_from_before_the_first_read_keeps_what_is_kept_here_alone_so() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories.clear();
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    // Kept here alone by no step of the owner's (its server refused it).
    s.mailbox.set_category(a, "Trip", GREEN).unwrap();
    answer(&mut s, a, CategoryChangeKind::Create, "Trip", GREEN, CategoryResult::Refused);
    assert!(s.mailbox.category_is_local(a, "Trip"));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Trip"), "{:?}", names(&s, a));
    assert!(changes(&mut s, a).is_empty(), "sent unasked: {:?}", changes(&mut s, a));
}

#[test]
fn at_the_limit_an_undo_keeps_one_put_back_here_alone_and_outlooks_addition_waits() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("X", RED)], true);
    full(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "X"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("Z", RED)]);
    // L, which its server refused, goes there with Add to Outlook, and that is undone: kept here
    // alone again, with its deletion there to go.
    s.mailbox.set_category(a, "L", GREEN).unwrap();
    answer(&mut s, a, CategoryChangeKind::Create, "L", GREEN, CategoryResult::Refused);
    s.run("category.addToOutlook", &json!({"account": a, "name": "L"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "L"));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "L"), "dropped: {:?}", names(&s, a).last());
    assert!(s.mailbox.category_of(a, "Z").is_none(), "what Outlook added since waits");
}

#[test]
fn an_undo_after_an_address_change_takes_away_what_the_step_made() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "U", "color": "#2E8B57"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "U").is_none(), "{:?}", names(&s, a));
    assert!(s.mailbox.category_of(a, "Red category").is_some());
}

// ---- a new name put on an event, a contact or a to-do (#705) ----

/// An event, a contact and a to-do of `account` (0: this computer's), each with the command
/// that categorizes it.
fn items_of(s: &mut Session, account: Id) -> Vec<(&'static str, Id)> {
    let origin = if account == 0 { Origin::default() } else { Origin { account, id: "x".into(), ..Default::default() } };
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Cal".into(), visible: true, origin: origin.clone(), ..Default::default() });
    let event = s.mailbox.add_event(Event { calendar, title: "Meeting".into(), origin: origin.clone(), ..Default::default() }).unwrap();
    let contact = s.mailbox.add_contact(Contact { display_name: "Pat".into(), origin: origin.clone(), ..Default::default() });
    let task = s.mailbox.add_task(Task { title: "Do it".into(), origin, ..Default::default() });
    vec![("calendar.categorize", event), ("people.categorize", contact), ("tasks.categorize", task)]
}

#[test]
fn a_new_name_put_on_an_event_a_contact_or_a_todo_goes_to_outlook_in_grey() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let mut made = Vec::new();
    for (command, id) in items_of(&mut s, a) {
        let name = format!("New for {command}");
        s.run(command, &json!({"id": id, "category": name})).unwrap();
        made.push(change(CategoryChangeKind::Create, &name, GREY_CATEGORY));
    }
    let mut sent = changes(&mut s, a);
    sent.sort_by(|x, y| x.name.cmp(&y.name));
    made.sort_by(|x, y| x.name.cmp(&y.name));
    assert_eq!(sent, made);
    // The last undone before it went: it leaves the list, and nothing is sent for it.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "New for tasks.categorize").is_none());
    assert_eq!(changes(&mut s, a).len(), 2);
}

#[test]
fn a_new_name_on_an_event_a_contact_or_a_todo_of_a_read_only_list_is_refused() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], false);
    let before = s.mailbox.categories_of(a).to_vec();
    for (command, id) in items_of(&mut s, a) {
        let refused = s.run(command, &json!({"id": id, "category": "Brand new"})).unwrap_err().to_string();
        assert!(refused.contains("a@example.org") && refused.contains("no permission"), "{command}: {refused}");
        // One of the list goes on as ever.
        s.run(command, &json!({"id": id, "category": "red category"})).unwrap();
    }
    assert_eq!(s.mailbox.categories_of(a), before);
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn a_new_name_on_this_computers_event_contact_or_todo_goes_nowhere() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    for (command, id) in items_of(&mut s, 0) {
        s.run(command, &json!({"id": id, "category": "Here only"})).unwrap();
    }
    assert!(s.mailbox.category("Here only").is_some());
    assert!(s.mailbox.category_of(a, "Here only").is_none());
    assert!(changes(&mut s, a).is_empty());
}

// ---- what Outlook has of its own is never deleted there by an undo ----

#[test]
fn an_undo_of_new_after_outlook_made_it_in_that_colour_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    // Before the job: Outlook has one of that name, made there meanwhile, in the same colour.
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", GREEN)]);
    assert!(changes(&mut s, a).is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
    assert!(s.mailbox.category_of(a, "Travel").is_some_and(|c| c.color == GREEN));
}

#[test]
fn an_undo_of_new_that_the_job_found_in_outlook_already_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Travel", GREEN)]);
    // Outlook had made one of that name since the last read: the job gave it this colour.
    let found = CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Done) };
    answers(&mut s, a, vec![found]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
    assert!(s.mailbox.category_of(a, "Travel").is_some());
}

#[test]
fn an_undo_of_new_that_the_job_made_deletes_it_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // A read since shows what the job made: still this one's.
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(deletes(&mut s, a), ["Travel"]);
}

#[test]
fn an_undo_of_add_to_outlook_after_outlook_made_it_in_that_colour_deletes_nothing() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let c = s.mailbox.category_color_of(a, "Old local");
    read(&mut s, a, vec![tag("Red category", RED), tag("Old local", c)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
    assert!(!s.mailbox.category_is_local(a, "Old local"), "Outlook's");
}

#[test]
fn a_redo_of_a_removal_after_outlook_made_it_again_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    // Before the job: Outlook makes it again itself, in that colour.
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN)]);
    assert!(changes(&mut s, a).is_empty());
    s.run("edit.redo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn a_redo_of_a_removal_of_outlooks_own_before_the_job_still_deletes_it() {
    // Outlook's own from the start: the step knew it, and its redo is the owner's again.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(changes(&mut s, a).is_empty());
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(deletes(&mut s, a), ["Old"]);
}

#[test]
fn at_the_limit_an_undo_that_cant_put_one_back_says_so() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    full(&mut s, a);
    let here = last_here(&s, a);
    s.run("category.remove", &json!({"account": a, "name": here})).unwrap();
    // Put on the list by no step of the owner's (a rule, say), not sent yet.
    s.mailbox.set_category(a, "V", GREEN).unwrap();
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    let out = s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "V").is_some());
    assert_eq!(out["notUndone"], json!([format!("The category '{here}' couldn't be put back: the list of categories is full.")]));
}

#[test]
fn an_undo_from_before_the_first_read_at_the_limit_keeps_whats_here_alone_through_a_reopen() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = (0..mailcraft_model::MAX_CATEGORIES - 2).map(|i| tag(&format!("Here {i}"), GREEN)).collect();
    s.mailbox.categories.push(tag("A", GREEN));
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    // The step from before the first read.
    s.run("category.remove", &json!({"account": a, "name": "A"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    // Kept here alone by no step of the owner's (its server refused it).
    s.mailbox.set_category(a, "Trip", GREEN).unwrap();
    answer(&mut s, a, CategoryChangeKind::Create, "Trip", GREEN, CategoryResult::Refused);
    assert!(s.mailbox.category_is_local(a, "Trip"));
    assert_eq!(s.mailbox.categories_of(a).len(), mailcraft_model::MAX_CATEGORIES);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Trip"));
    assert!(s.mailbox.categories_of(a).len() <= mailcraft_model::MAX_CATEGORIES);
    // What its server lists waits, and nothing is deleted there for it.
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    let opened = crate::io::read_mailbox(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert!(opened.category_is_local(a, "Trip"), "lost as the file opened");
}

#[test]
fn what_the_job_made_again_after_outlooks_own_went_is_this_ones_to_take_back() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "X", "color": "#2E8B57"})).unwrap();
    // Outlook made one of that name meanwhile: its own.
    read(&mut s, a, vec![tag("Red category", RED), tag("X", GREEN)]);
    s.run("category.remove", &json!({"account": a, "name": "X"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // Undone: the job makes it there again, and that one is this one's.
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Create, "X", GREEN)]);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(deletes(&mut s, a), ["X"]);
}

#[test]
fn what_outlook_was_found_to_have_after_a_step_is_still_known_after_its_undo() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "X", "color": "#2E8B57"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2F6FD0"})).unwrap();
    // After both steps, before the job: Outlook made X too.
    read(&mut s, a, vec![tag("Red category", RED), tag("X", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_from_before_the_first_read_at_the_limit_keeps_what_is_still_to_go() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = (0..mailcraft_model::MAX_CATEGORIES - 2).map(|i| tag(&format!("Here {i}"), GREEN)).collect();
    s.mailbox.categories.push(tag("A", GREEN));
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "A"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED)]);
    // Put on the list by no step of the owner's, to go with the next job.
    s.mailbox.set_category(a, "Trip", GREEN).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Trip").is_some(), "dropped: it was nowhere else");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Trip", GREEN)]);
}

// ---- review6: extra paths ----

#[test]
fn undo_while_the_job_is_on_its_way_and_it_finds_outlooks_own_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    // The job is built and goes.
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Travel", GREEN)]);
    // The owner undoes while it is on its way.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Travel").is_none());
    // The job found Outlook's own Travel there already, and its read lists it.
    let found = CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Done) };
    answers(&mut s, a, vec![found]);
    assert!(deletes(&mut s, a).is_empty(), "before the read, Outlook's own to be deleted: {:?}", deletes(&mut s, a));
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", GREEN)]);
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn undo_of_a_rename_while_the_job_is_on_its_way_and_it_finds_outlooks_own_new_name_deletes_nothing() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    // The job: New is to be made first; Old waits for it.
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    // Outlook had a New of its own: the job found it.
    let found = CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "New", GREEN, CategoryResult::Done) };
    answers(&mut s, a, vec![found]);
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN), tag("New", GREEN)]);
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn undo_while_the_job_is_on_its_way_and_it_made_it_deletes_it_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    let sent = changes(&mut s, a);
    s.run("edit.undo", &json!({})).unwrap();
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", GREEN)]);
    assert_eq!(deletes(&mut s, a), ["Travel"]);
}

#[test]
fn a_new_name_on_an_event_contact_or_todo_made_there_is_undone_there_and_redone() {
    for at in 0..3 {
        let (mut s, a) = rig(vec![tag("Red category", RED)], true);
        let (command, id) = items_of(&mut s, a)[at];
        s.run(command, &json!({"id": id, "category": "Fresh"})).unwrap();
        let sent = changes(&mut s, a);
        assert_eq!(sent.len(), 1, "{command}: {sent:?}");
        assert!(mailcraft_model::outlook_colors::outlook_preset(sent[0].color).is_some(), "{command}: not one of Outlook's colours");
        took(&mut s, a, &sent);
        read(&mut s, a, vec![tag("Red category", RED), tag("Fresh", sent[0].color)]);
        // An agent's undo of it takes a category away there: asked as Delete.
        assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Delete, _)), "{command}");
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(deletes(&mut s, a), ["Fresh"], "{command}");
        let sent = changes(&mut s, a);
        took(&mut s, a, &sent);
        s.run("edit.redo", &json!({})).unwrap();
        assert_eq!(
            changes(&mut s, a).iter().map(|c| (c.kind, c.name.clone())).collect::<Vec<_>>(),
            [(CategoryChangeKind::Create, "Fresh".to_string())],
            "{command}"
        );
    }
}

#[test]
fn a_new_name_put_on_an_item_of_a_long_read_only_list_says_read_only() {
    let long: Vec<Category> = (0..250).map(|i| tag(&format!("O{i}"), RED)).collect();
    let (mut s, a) = rig(long, false);
    let (command, id) = items_of(&mut s, a)[0];
    let refused = s.run(command, &json!({"id": id, "category": "Brand new"})).unwrap_err().to_string();
    assert!(refused.contains("no permission"), "{command}: {refused}");
}

#[test]
fn an_agents_undo_of_add_to_outlook_after_the_job_is_asked() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // The undo deletes it in Outlook.
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(deletes(&mut s, a), ["Old local"]);
    match verdict {
        Verdict::Ask(Class::Delete, text) => assert!(text.contains("deletes the category \u{201c}Old local\u{201d} in Outlook"), "{text}"),
        other => panic!("an agent deletes it in Outlook unasked: {other:?}"),
    }
}

#[test]
fn at_the_limit_an_undo_while_a_colour_is_on_its_way_never_deletes_outlooks_own() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("X", RED)], true);
    full(&mut s, a);
    let here = last_here(&s, a);
    s.run("category.remove", &json!({"account": a, "name": here})).unwrap();
    // Outlook makes Z; a read brings it, into the room the removal left.
    read(&mut s, a, vec![tag("Red category", RED), tag("X", RED), tag("Z", RED)]);
    assert!(s.mailbox.category_of(a, "Z").is_some());
    s.run("category.set", &json!({"account": a, "name": "Z", "color": "#2F6FD0"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    // Its colour undone: the colour back is to go, and a job takes it.
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Color, "Z", RED)]);
    // While that job is on its way, the removal is undone: no room for Z, which waits.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Z").is_none());
    took(&mut s, a, &sent);
    read(&mut s, a, vec![tag("Red category", RED), tag("X", RED), tag("Z", RED)]);
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn a_redo_after_undos_and_reads_never_deletes_what_outlook_made_again() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("K", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2F6FD0"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("K", GREEN), tag("P1", RED), tag("P2", RED), tag("P3", RED)]);
    s.run("category.remove", &json!({"account": a, "name": "K"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    // Outlook deletes K and makes it again.
    read(&mut s, a, vec![tag("Red category", RED), tag("P1", RED), tag("P2", RED), tag("P3", RED)]);
    read(&mut s, a, vec![tag("Red category", RED), tag("K", GREEN), tag("P1", RED), tag("P2", RED), tag("P3", RED)]);
    s.run("edit.redo", &json!({})).unwrap();
    s.run("edit.redo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "K").is_some(), "Outlook's own taken off: {:?}", names(&s, a));
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_past_an_address_change_still_knows_what_outlook_had_of_its_own() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    // Before the job: Outlook made one of that name, in that colour.
    read(&mut s, a, vec![tag("Red category", RED), tag("Travel", GREEN)]);
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "a@example.org");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn an_undo_of_add_to_outlook_while_the_job_finds_outlooks_own_deletes_nothing() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent.len(), 1);
    // Kept here alone again while the job is on its way; it finds Outlook's own there.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Old local"));
    let found = CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "Old local", sent[0].color, CategoryResult::Done) };
    answers(&mut s, a, vec![found]);
    assert!(deletes(&mut s, a).is_empty(), "Outlook's own deleted: {:?}", deletes(&mut s, a));
}

#[test]
fn an_agents_undo_of_add_to_outlook_with_a_category_still_to_be_made_is_asked() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("category.remove", &json!({"account": a, "name": "Red category"})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Delete, "Red category", RED)]);
    took(&mut s, a, &sent);
    // The removal undone: Red category is to be made again.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Red category", RED)]);
    // An agent's undo of Add to Outlook now: it has Old local deleted in Outlook.
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert_eq!(deletes(&mut s, a), ["Old local"], "the next job but one deletes it in Outlook");
    assert!(matches!(verdict, Verdict::Ask(Class::Delete, _)), "an agent deletes it in Outlook unasked: {verdict:?}");
}

#[test]
fn an_agents_undo_that_lets_the_owners_own_deletion_go_is_not_said_to_delete_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("K", GREEN)], true);
    // The owner deletes K, then makes Y: K's deletion waits on Y.
    s.run("category.remove", &json!({"account": a, "name": "K"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Y", "color": "#2E8B57"})).unwrap();
    assert!(deletes(&mut s, a).is_empty());
    // An agent's undo of making Y: K's deletion, the owner's, goes with the next job.
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    match verdict {
        Verdict::Ask(_, text) => assert!(!text.contains("deletes the category \u{201c}K\u{201d}"), "{text}"),
        Verdict::Run => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_agents_undo_of_a_rename_both_jobs_took_names_what_it_deletes_in_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Delete, "Old", GREEN)]);
    took(&mut s, a, &sent);
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert_eq!(deletes(&mut s, a), ["New"]);
    match verdict {
        Verdict::Ask(Class::Delete, text) => assert!(text.contains("deletes the category \u{201c}New\u{201d} in Outlook"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_agent_without_delete_cannot_undo_what_deletes_a_category_in_outlook() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    let before = s.mailbox.clone();
    s.agent = Some(vec![Class::Accounts, Class::Send, Class::Settings]);
    let refused = s.run("edit.undo", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("delete categories in Outlook"), "{refused}");
    s.agent = None;
    assert!(s.mailbox == before);
    assert!(deletes(&mut s, a).is_empty());
}

#[test]
fn an_agents_undo_that_also_takes_a_draft_still_names_the_category_it_deletes_in_outlook() {
    // A draft saved since the step, outside the history (as the composer's own saving is).
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    let folder = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(Message { folder, draft: true, subject: "Hello".into(), ..Default::default() }).unwrap();
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Delete, text) => {
            assert!(text.contains("\u{201c}Hello\u{201d}") && text.contains("deletes the category \u{201c}Old local\u{201d} in Outlook"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_category_an_undo_deletes_in_two_accounts_is_named_once() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let b = s.run("account.add", &json!({"email": "b@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let download = CategoriesDownload { account: b, email: "b@example.org".into(), categories: vec![tag("Red category", RED)], complete: true };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
    s.run("sync.categoryWrites", &json!({"account": b, "allowed": true})).unwrap();
    let (ma, mb) = (message_of(&mut s, a, &[]), message_of(&mut s, b, &[]));
    s.run("mail.categorize", &json!({"ids": [ma, mb], "category": "Brand new"})).unwrap();
    for (account, email) in [(a, "a@example.org"), (b, "b@example.org")] {
        let sent = changes(&mut s, account);
        let outcomes = sent
            .iter()
            .map(|c| CategoryOutcome {
                kind: c.kind,
                name: c.name.clone(),
                color: c.color,
                result: CategoryResult::Done,
                error: String::new(),
                found: false,
            })
            .collect();
        let result = CategoriesUploaded { account, email: email.into(), outcomes, ..Default::default() };
        s.run("sync.applyCategoryChanges", &json!({"result": result})).unwrap();
    }
    assert_eq!(s.undo_deletes_categories(false), ["Brand new", "Brand new"]);
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Delete, text) => {
            assert!(text.contains("deletes the category \u{201c}Brand new\u{201d} in Outlook") && text.matches("Brand new").count() == 1, "{text}");
        }
        other => panic!("{other:?}"),
    }
}

/// Every name the jobs delete in Outlook, each job taken whole, until nothing is left to send; a
/// deletion that the same job makes again in other letters is left out.
fn deleted_by_the_jobs(s: &mut Session, a: Id) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..20 {
        let sent = changes(s, a);
        if sent.is_empty() {
            break;
        }
        let made: Vec<&str> = sent.iter().filter(|c| c.kind == CategoryChangeKind::Create).map(|c| c.name.as_str()).collect();
        for c in sent.iter().filter(|c| c.kind == CategoryChangeKind::Delete) {
            if !made.iter().any(|n| mailcraft_model::sync::same_category(n, &c.name)) {
                out.push(c.name.clone());
            }
        }
        took(s, a, &sent);
    }
    out
}

/// What the agent gate says an undo (`redo`: redo) deletes in Outlook, and what the jobs after it
/// newly delete there.
fn said_and_deleted(s: &mut Session, a: Id, redo: bool) -> (Vec<String>, Vec<String>) {
    let mut said = s.undo_deletes_categories(redo);
    let mut without = Session::at(s.mailbox.clone(), NOW, 0);
    let anyway = deleted_by_the_jobs(&mut without, a);
    s.run(if redo { "edit.redo" } else { "edit.undo" }, &json!({})).unwrap();
    let mut deleted: Vec<String> =
        deleted_by_the_jobs(s, a).into_iter().filter(|n| !anyway.iter().any(|w| mailcraft_model::sync::same_category(w, n))).collect();
    said.sort();
    deleted.sort();
    (said, deleted)
}

#[test]
fn the_gate_names_what_the_jobs_then_delete_in_outlook() {
    // An undo of Add to Outlook while another step's job is done.
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("category.remove", &json!({"account": a, "name": "Red category"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(said_and_deleted(&mut s, a, false), (vec!["Old local".to_string()], vec!["Old local".to_string()]));
    // A redo of a removal whose undo the server took.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Create, "Old", GREEN)]);
    took(&mut s, a, &sent);
    assert_eq!(said_and_deleted(&mut s, a, true), (vec!["Old".to_string()], vec!["Old".to_string()]));
    // A redo of a removal after a change of letters, none of it sent yet.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Foo", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Foo", "to": "foo"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "foo"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(said_and_deleted(&mut s, a, true), (vec!["Foo".to_string()], vec!["Foo".to_string()]));
    // Nothing, when the owner's own deletion waits on a creation, or a removal is undone before
    // anything was sent.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("K", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "K"})).unwrap();
    s.run("category.set", &json!({"account": a, "name": "Y", "color": "#2E8B57"})).unwrap();
    assert_eq!(said_and_deleted(&mut s, a, false), (vec![], vec![]));
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "New", "color": "#2E8B57"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert_eq!(said_and_deleted(&mut s, a, false), (vec![], vec![]));
    // Nothing, when the list may no longer be changed there: it says nothing of Outlook either.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "X", "color": "#2E8B57"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": false})).unwrap();
    if let Verdict::Ask(_, text) = check(&s, "edit.undo", &json!({}), &[]) {
        assert!(!text.contains("in Outlook"), "{text}");
    }
    assert_eq!(said_and_deleted(&mut s, a, false), (vec![], vec![]));
}

// ---- a rename: the items take the new name in Outlook before the old one is deleted there ----

/// A message of `account`'s Inbox that its server has, with `categories` here and there.
fn server_message(s: &mut Session, account: Id, categories: &[&str]) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let categories: Vec<String> = categories.iter().map(|c| c.to_string()).collect();
    let server = ServerMessage { id: "m-1".into(), folder: "inbox".into(), categories: categories.clone(), ..Default::default() };
    s.mailbox.add_message(Message { folder, categories, server: Some(server), ..Default::default() }).unwrap()
}

/// The server's answer to the message's change of categories.
fn mail_answer(s: &mut Session, account: Id, outcome: MessageOutcome) {
    let (sent, _) = graph_up::message_changes(s, account, 10);
    let [change @ MessageChange::Update { categories: Some(_), .. }] = sent.as_slice() else { panic!("{sent:?}") };
    let done = MessageChangeDone { change: change.clone(), outcome, there: None };
    assert!(graph_up::message_done(&mut s.mailbox, account, &done, &mut Default::default()));
}

/// A to-do of `account`'s that its server has, with `categories` here and there.
fn server_task(s: &mut Session, account: Id, categories: &[&str]) -> Id {
    let categories: Vec<String> = categories.iter().map(|c| c.to_string()).collect();
    let synced = TaskFields { title: "Do it".into(), categories: categories.clone(), ..Default::default() };
    let origin = Origin { account, id: "t-1".into(), ..Default::default() };
    s.mailbox.add_task(Task { title: "Do it".into(), categories, origin, list: "l-1".into(), synced: Some(synced), ..Default::default() })
}

fn task_answer(s: &mut Session, account: Id, result: TaskResult) {
    let upload = tasks_remote::task_upload(&s.mailbox, account).unwrap();
    let [op] = upload.ops.as_slice() else { panic!("{:?}", upload.ops) };
    let outcome =
        TaskOutcome { kind: op.kind, task: op.task, id: op.id.clone(), list: op.list.clone(), result, patch: op.patch.clone(), ..Default::default() };
    let uploaded = TasksUploaded { account, email: "a@example.org".into(), outcomes: vec![outcome], ..Default::default() };
    tasks_remote::apply_task_changes(s, &uploaded).unwrap();
}

/// An event of `account`'s calendar that its server has, with `categories`.
fn server_event(s: &mut Session, account: Id, categories: &[&str]) -> Id {
    let origin = Origin { account, id: "e-1".into(), ..Default::default() };
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Calendar".into(), visible: true, origin: origin.clone(), ..Default::default() });
    let categories = categories.iter().map(|c| c.to_string()).collect();
    s.mailbox.add_event(Event { calendar, title: "Meeting".into(), categories, origin, ..Default::default() }).unwrap()
}

fn event_answer(s: &mut Session, account: Id) {
    let upload = remote::event_upload(s, account).unwrap();
    let [op] = upload.ops.as_slice() else { panic!("{:?}", upload.ops) };
    let outcome = EventOutcome { op: op.clone(), result: EventResult::Done, ..Default::default() };
    let uploaded = EventsUploaded { account, email: "a@example.org".into(), outcomes: vec![outcome], ..Default::default() };
    remote::apply_event_outcomes(s, &uploaded).unwrap();
}

/// The old name is not deleted in Outlook while an item there still has it: first the new name
/// is made, then the items take it, then the old one goes.
fn waits_then_deletes(s: &mut Session, a: Id, moved: impl FnOnce(&mut Session)) {
    let sent = changes(s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Create, "New", GREEN)]);
    took(s, a, &sent);
    assert!(changes(s, a).is_empty(), "deleted while an item there has it: {:?}", changes(s, a));
    moved(s);
    assert_eq!(changes(s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);
}

#[test]
fn a_rename_deletes_the_old_name_in_outlook_only_once_the_mail_there_has_the_new_one() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let m = server_message(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    waits_then_deletes(&mut s, a, |s| {
        let done = MessageOutcome::Done { categories: Some(vec!["New".into()]), flag: None, folder: String::new(), id: String::new() };
        mail_answer(s, a, done);
    });
    assert_eq!(s.mailbox.message(m).unwrap().server.as_ref().unwrap().categories, ["New"]);
}

#[test]
fn a_mail_change_outlook_refuses_lets_the_old_name_go_there() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let m = server_message(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    // The message is put back as the server has it, and nothing waits for it.
    waits_then_deletes(&mut s, a, |s| mail_answer(s, a, MessageOutcome::Refused { status: 403 }));
    assert_eq!(s.mailbox.message(m).unwrap().categories, ["Old"]);
}

#[test]
fn a_rename_deletes_the_old_name_in_outlook_only_once_the_todo_there_has_the_new_one() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let t = server_task(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    waits_then_deletes(&mut s, a, |s| task_answer(s, a, TaskResult::Done));
    assert_eq!(s.mailbox.task(t).unwrap().synced.as_ref().unwrap().categories, ["New"]);

    // One Microsoft To Do refused stays on this computer and is not sent again: nothing waits for it.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let t = server_task(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    waits_then_deletes(&mut s, a, |s| task_answer(s, a, TaskResult::Refused));
    assert!(s.mailbox.task(t).unwrap().local_only);
}

#[test]
fn a_rename_or_a_removal_is_told_to_the_calendar_and_the_old_name_waits_for_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let e = server_event(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.event(e).unwrap().categories, ["New"]);
    assert_eq!(s.mailbox.event_changes.len(), 1, "the calendar's server is told");
    waits_then_deletes(&mut s, a, |s| event_answer(s, a));
    assert!(s.mailbox.event_changes.is_empty());

    // Removed: the event there is told too, and the name goes once it has.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let e = server_event(&mut s, a, &["Old", "Red category"]);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert_eq!(s.mailbox.event(e).unwrap().categories, ["Red category"]);
    assert!(changes(&mut s, a).is_empty(), "deleted while the event there has it");
    event_answer(&mut s, a);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);

    // An event on this computer alone tells nobody.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Here".into(), visible: true, ..Default::default() });
    s.mailbox.add_event(Event { calendar, title: "Here".into(), categories: vec!["Old".into()], ..Default::default() }).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn other_letters_alone_go_at_once_for_the_items_there_are_the_same_name() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    server_message(&mut s, a, &["client"]);
    server_task(&mut s, a, &["client"]);
    server_event(&mut s, a, &["client"]);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "client", GREEN), change(CategoryChangeKind::Create, "Client", GREEN)]);
}

#[test]
fn mail_in_the_outbox_is_sent_not_changed_and_nothing_waits_for_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let m = server_message(&mut s, a, &["Old"]);
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&m).unwrap()).folder = outbox;
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);
}
