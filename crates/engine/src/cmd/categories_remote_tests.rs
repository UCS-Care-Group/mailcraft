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
use mailcraft_model::{
    Action, Calendar, Category, Condition, Contact, Event, FolderKind, GREY_CATEGORY, Id, Mailbox, Message, Origin, Rgb, Rights, Rule, Task,
    TaskFields,
};
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
    refused_with(s, account, kind, name, color, result, "")
}

/// [`answer`], with the server's reason.
fn refused_with(
    s: &mut Session,
    account: Id,
    kind: CategoryChangeKind,
    name: &str,
    color: Rgb,
    result: CategoryResult,
    why: &str,
) -> serde_json::Value {
    let outcome = CategoryOutcome { kind, name: name.into(), color, result, error: why.into(), found: false };
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
        assert!(refused.contains("a@example.org") && refused.contains("read-only here"), "{command}: {refused}");
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

/// A read that finds several names written otherwise in Outlook gives each item that carries
/// them every one of Outlook's spellings.
#[test]
fn a_read_with_several_names_written_otherwise_respells_them_all_on_the_items() {
    let (mut s, a) = rig(vec![tag("Client", RED), tag("Travel", GREEN), tag("Home", BLUE)], true);
    let both = message_of(&mut s, a, &["Client", "Travel", "Home"]);
    let one = message_of(&mut s, a, &["Travel"]);
    read(&mut s, a, vec![tag("CLIENT", RED), tag("travel", GREEN), tag("Home", BLUE)]);
    assert_eq!(names(&s, a), ["CLIENT", "travel", "Home"]);
    assert_eq!(s.mailbox.message(both).unwrap().categories, ["CLIENT", "travel", "Home"]);
    assert_eq!(s.mailbox.message(one).unwrap().categories, ["travel"]);
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
    // The deletion waits on what is made; when the server won't make it, the deletion, which is
    // no rename's, goes all the same (#661).
    let said = answer(&mut s, a, CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused);
    assert!(said["messages"][0].as_str().unwrap().contains("Travel"));
    assert!(!said["messages"].to_string().contains("wasn't deleted"), "{said}");
    answer(&mut s, a, CategoryChangeKind::Color, "Red category", GREEN, CategoryResult::Refused);
    assert_eq!(s.mailbox.category_color_of(a, "Red category"), RED);
    assert!(s.mailbox.category_is_local(a, "Travel"), "kept on this computer only");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", RED)]);
    // (Refused there, as below: it is put back.)
    answer(&mut s, a, CategoryChangeKind::Delete, "Old", RED, CategoryResult::Refused);
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
fn a_refusal_says_the_servers_reason_as_rules_and_to_dos_do() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    let why = "Microsoft Graph answered with status 400 (ErrorInvalidRequest).";
    let said = refused_with(&mut s, a, CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused, why);
    assert_eq!(
        said["messages"][0],
        "The server didn't take the category \"Travel\"; it stays on this computer only: Microsoft Graph answered with status 400 (ErrorInvalidRequest)."
    );
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let said = refused_with(&mut s, a, CategoryChangeKind::Delete, "Old", RED, CategoryResult::Refused, "  it is in use  ");
    assert_eq!(said["messages"][0], "The server didn't delete the category \"Old\"; it is back on the list: it is in use.");
    // With none, the line ends where it did.
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2E8B57"})).unwrap();
    let said = answer(&mut s, a, CategoryChangeKind::Color, "Red category", GREEN, CategoryResult::Refused);
    assert_eq!(said["messages"][0], "The server didn't take the new colour of the category \"Red category\"; it has the server's again.");
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
fn a_rename_of_a_name_outlook_hasnt_got_yet_leaves_nothing_waiting_in_the_file() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Fresh", "color": "#2E8B57"})).unwrap();
    s.run("category.rename", &json!({"account": a, "from": "Fresh", "to": "Fresher"})).unwrap();
    assert!(s.mailbox.category_renames.is_empty(), "{:?}", s.mailbox.category_renames);
    assert!(!serde_json::to_string(&s.mailbox).unwrap().contains("categoryRenames"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Fresher", GREEN)]);
}

#[test]
fn a_rename_mailcraft_closes_on_keeps_the_old_name_in_outlook_when_the_server_wont_make_the_new_one() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    // Saved and opened again before the job ran.
    let opened = crate::io::read_mailbox(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    let mut s = Session::at(opened, NOW, 0);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    let said = answer(&mut s, a, CategoryChangeKind::Create, "New", GREEN, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"Old\\\" wasn't deleted in Outlook"), "{said}");
    assert!(changes(&mut s, a).is_empty(), "nothing deleted there: {:?}", changes(&mut s, a));
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
fn an_undo_after_the_job_of_a_rename_brings_back_nothing_of_that_rename() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Blue", BLUE)], true);
    let m = message_of(&mut s, a, &[]);
    s.run("category.rename", &json!({"account": a, "from": "Blue", "to": "Navy"})).unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    // The job makes the new name, then deletes the old one.
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Create, "Navy", BLUE)]);
    took(&mut s, a, &sent);
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Delete, "Blue", BLUE)]);
    took(&mut s, a, &sent);
    assert!(s.mailbox.category_renames.is_empty());
    // An undo of the flag, then another rename before the next read: Navy waits on Teal.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_renames.is_empty(), "{:?}", s.mailbox.category_renames);
    s.run("category.rename", &json!({"account": a, "from": "Navy", "to": "Teal"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Teal", BLUE)]);
    let said = answer(&mut s, a, CategoryChangeKind::Create, "Teal", BLUE, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"Navy\\\" wasn't deleted in Outlook"), "{said}");
    assert!(changes(&mut s, a).is_empty(), "nothing deleted there: {:?}", changes(&mut s, a));
}

/// `sent` was the job, and the server took all of it; the next job is `next`.
fn job(s: &mut Session, a: Id, sent: &[CategoryChange], next: &[CategoryChange]) {
    assert_eq!(changes(s, a), sent);
    took(s, a, sent);
    assert_eq!(changes(s, a), next);
}

#[test]
fn an_undo_of_a_rename_both_jobs_took_keeps_the_new_name_in_outlook_when_the_server_wont_make_the_old_one() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    job(&mut s, a, &[change(CategoryChangeKind::Create, "New", GREEN)], &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    took(&mut s, a, &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Old", GREEN)]);
    let said = answer(&mut s, a, CategoryChangeKind::Create, "Old", GREEN, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"New\\\" wasn't deleted in Outlook"), "{said}");
    assert!(changes(&mut s, a).is_empty(), "nothing deleted there: {:?}", changes(&mut s, a));
    assert!(s.mailbox.category_is_local(a, "Old"));

    // A redo after the undo's jobs: the same the other way.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    job(&mut s, a, &[change(CategoryChangeKind::Create, "New", GREEN)], &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    took(&mut s, a, &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    job(&mut s, a, &[change(CategoryChangeKind::Create, "Old", GREEN)], &[change(CategoryChangeKind::Delete, "New", GREEN)]);
    took(&mut s, a, &[change(CategoryChangeKind::Delete, "New", GREEN)]);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    let said = answer(&mut s, a, CategoryChangeKind::Create, "New", GREEN, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"Old\\\" wasn't deleted in Outlook"), "{said}");
    assert!(changes(&mut s, a).is_empty(), "nothing deleted there: {:?}", changes(&mut s, a));

    // Taken, the undo's new name deletes the other there as a rename does.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    job(&mut s, a, &[change(CategoryChangeKind::Create, "New", GREEN)], &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    took(&mut s, a, &[change(CategoryChangeKind::Delete, "Old", GREEN)]);
    s.run("edit.undo", &json!({})).unwrap();
    job(&mut s, a, &[change(CategoryChangeKind::Create, "Old", GREEN)], &[change(CategoryChangeKind::Delete, "New", GREEN)]);
    took(&mut s, a, &[change(CategoryChangeKind::Delete, "New", GREEN)]);
    assert!(s.mailbox.category_renames.is_empty(), "{:?}", s.mailbox.category_renames);
    assert_eq!(names(&s, a), ["Red category", "Old"]);
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
fn a_file_from_an_older_build_with_a_name_in_two_spellings_sends_nothing_for_it() {
    // An older build compared names in ASCII only, so its list could hold both (#788).
    let (mut s, a) = rig(vec![tag("проект", GREEN)], true);
    s.mailbox.account_categories.insert(a, vec![tag("Проект", RED), tag("проект", GREEN)]);
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    for field in ["syncedCategories", "localCategories", "categoryWrites"] {
        file.as_object_mut().unwrap().remove(field);
    }
    file["serverCategories"] = json!({a.to_string(): ["проект"]});
    let mut opened = crate::io::read_mailbox(&serde_json::to_vec(&file).unwrap()).unwrap();
    opened.category_writes.insert(a);
    assert_eq!(opened.categories_of(a), [tag("проект", GREEN)], "the server's spelling and colour");
    assert!(opened.category_upload(a).is_empty(), "{:?}", opened.category_upload(a));
}

#[test]
fn a_category_with_no_colour_in_outlook_keeps_none_and_can_be_made_gray() {
    let none = mailcraft_model::outlook_colors::NO_COLOR;
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Plain", none)], true);
    s.run("category.rename", &json!({"account": a, "from": "Plain", "to": "Clear"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Clear", none)]);
    // Outlook's Gray is a colour of its own.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Plain", none)], true);
    s.run("category.set", &json!({"account": a, "name": "Plain", "color": GREY_CATEGORY.hex()})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Color, "Plain", GREY_CATEGORY)]);
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
    // The old letters are made there again first; then the deletion, not done for now, goes again.
    let sent = changes(&mut s, a);
    assert_eq!(sent, [change(CategoryChangeKind::Create, "client", GREEN)]);
    took(&mut s, a, &sent);
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
fn an_account_that_is_not_microsofts_any_more_keeps_its_categories_here_to_change() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("account.edit", &json!({"id": a, "kind": "imap"})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "a@example.org");
    // Nothing keeps it in step with Outlook: it is changed here, and nothing waits to go there.
    assert!(s.mailbox.categories_read_only(a).is_none() && s.mailbox.synced_list_owner(a).is_none());
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#123456"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert!(s.run("sync.categoryChanges", &json!({"account": a})).is_err() || changes(&mut s, a).is_empty());
    // A Microsoft account again: its server's list is read as a new address's is, and what is
    // only here stays here.
    s.run("account.edit", &json!({"id": a, "kind": "microsoft"})).unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Old", GREEN)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert!(s.mailbox.category_is_local(a, "Travel"));
    assert!(!s.mailbox.category_is_local(a, "Red category"));
    // Undone, the kind change sends nothing either.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("account.edit", &json!({"id": a, "kind": "imap"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(deletes(&mut s, a).is_empty(), "{:?}", deletes(&mut s, a));
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
        assert!(refused.contains("a@example.org") && refused.contains("read-only here"), "{command}: {refused}");
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
    assert!(refused.contains("read-only here"), "{command}: {refused}");
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
    // It makes the old name there again too (#661): asked as `send`, with Delete in the same
    // question.
    match verdict {
        Verdict::Ask(Class::Send, text) => assert!(
            text.contains("deletes the category \u{201c}New\u{201d} in Outlook")
                && text.contains("makes the category \u{201c}Old\u{201d} in Outlook"),
            "{text}"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_agents_undo_of_a_rename_in_other_letters_names_what_it_deletes_in_outlook() {
    // Outlook can't rename: the undo deletes "Client" there and makes "client" again.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    match verdict {
        Verdict::Ask(Class::Delete, text) => assert!(text.contains("deletes the category \u{201c}Client\u{201d} in Outlook"), "{text}"),
        other => panic!("{other:?}"),
    }
    s.agent = Some(vec![Class::Accounts, Class::Send, Class::Settings]);
    let refused = s.run("edit.undo", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("delete categories in Outlook"), "{refused}");
    s.agent = None;
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Client", GREEN), change(CategoryChangeKind::Create, "client", GREEN)]);
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
            // Said once: not also as categories taken away (#801's list).
            assert!(!text.contains("made since"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_undo_that_also_takes_away_a_category_kept_on_this_computer_still_says_so() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let local = s.run("account.add", &json!({"email": "kit@example.org", "kind": "local"})).unwrap()["id"].as_u64().unwrap();
    let (ma, ml) = (message_of(&mut s, a, &[]), message_of(&mut s, local, &[]));
    // One step puts "Brand new" on Outlook's list and on the one kept on this computer.
    s.run("mail.categorize", &json!({"ids": [ma, ml], "category": "Brand new"})).unwrap();
    assert!(s.mailbox.category_of(local, "Brand new").is_some());
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    assert_eq!(s.undo_deletes_categories(false), ["Brand new"]);
    // The undo deletes one in Outlook and takes the other away here for good: both are said.
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Delete, text) => {
            assert!(text.contains("deletes the category \u{201c}Brand new\u{201d} in Outlook") && text.contains("made since"), "{text}");
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
    let calendar = s.mailbox.alloc_id();
    // (Each its own id there, so that a change of one is never folded into another's.)
    let origin = Origin { account, id: format!("e-{calendar}"), ..Default::default() };
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Calendar".into(), visible: true, origin: origin.clone(), ..Default::default() });
    let categories = categories.iter().map(|c| c.to_string()).collect();
    s.mailbox.add_event(Event { calendar, title: "Meeting".into(), categories, origin, ..Default::default() }).unwrap()
}

#[test]
fn a_category_on_more_server_events_than_can_wait_is_refused_saying_so() {
    let max = mailcraft_model::remote::MAX_EVENT_CHANGES;
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let calendar = s.mailbox.alloc_id();
    let origin = Origin { account: a, id: "cal".into(), ..Default::default() };
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Calendar".into(), visible: true, origin, ..Default::default() });
    let mut events = Vec::new();
    for i in 0..=max {
        let origin = Origin { account: a, id: format!("e-{i}"), ..Default::default() };
        let event = Event { calendar, title: "Meeting".into(), categories: vec!["Old".into()], origin, ..Default::default() };
        events.push(s.mailbox.add_event(event).unwrap());
    }
    let before = s.mailbox.clone();
    for (command, params) in
        [("category.rename", json!({"account": a, "from": "Old", "to": "New"})), ("category.remove", json!({"account": a, "name": "Old"}))]
    {
        // Waiting for Send/Receive would never make room for them all: it doesn't say to.
        let refused = s.run(command, &params).unwrap_err().to_string();
        assert!(refused.contains("than can wait to be sent at once") && !refused.contains("Send/Receive"), "{command}: {refused}");
        assert!(s.mailbox.events == before.events && s.mailbox.event_changes.is_empty(), "{command}");
        assert!(s.mailbox.account_categories == before.account_categories && s.mailbox.category_renames == before.category_renames, "{command}");
    }
    // As many as can wait: it is done, and each event goes to its server.
    s.mailbox.remove_event(events[0]).unwrap();
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.event_changes.len(), max);
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

/// Graph names the owner of every calendar, the account's own too (#661).
fn owned_by(s: &mut Session, event: Id, address: &str) {
    let calendar = s.mailbox.event(event).unwrap().calendar;
    s.mailbox.calendars.iter_mut().find(|c| c.id == calendar).unwrap().owner =
        Some(mailcraft_model::CalendarOwner { address: address.into(), name: "A".into() });
}

#[test]
fn the_owners_own_calendar_named_as_theirs_by_graph_follows_a_rename_and_the_old_name_waits_for_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let e = server_event(&mut s, a, &["Old"]);
    owned_by(&mut s, e, "a@EXAMPLE.org");
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.event(e).unwrap().categories, ["New"], "the owner's own event takes the new name");
    let told: Vec<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
    assert_eq!(told, [e], "and its server is told");
    // "Old" is deleted in Outlook only once the event there has "New".
    waits_then_deletes(&mut s, a, |s| event_answer(s, a));
}

/// The account's main calendar, which its server says `address` owns.
fn main_calendar_owned_by(s: &mut Session, account: Id, address: &str) {
    let id = s.mailbox.alloc_id();
    let origin = Origin { account, ..Default::default() };
    let owner = Some(mailcraft_model::CalendarOwner { address: address.into(), name: "A".into() });
    s.mailbox.calendars.push(Calendar { id, name: "Calendar".into(), visible: true, origin, owner, ..Default::default() });
}

#[test]
fn an_account_added_by_an_alias_has_its_other_calendars_follow_a_rename_and_a_colleagues_not() {
    // Its server names the mailbox's main address as the owner of its calendars, the main
    // calendar's owner too, and not the alias it was added with (#661).
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    main_calendar_owned_by(&mut s, a, "Primary@example.org");
    let mine = server_event(&mut s, a, &["Old"]);
    owned_by(&mut s, mine, "primary@example.org");
    let theirs = server_event(&mut s, a, &["Old"]);
    owned_by(&mut s, theirs, "colleague@example.org");
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.event(mine).unwrap().categories, ["New"], "the owner's other calendar is the owner's own");
    assert_eq!(s.mailbox.event(theirs).unwrap().categories, ["Old"], "a colleague's is left alone");
    let told: Vec<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
    assert_eq!(told, [mine], "and only its server is told");
    // "Old" is deleted in Outlook only once the event there has "New".
    waits_then_deletes(&mut s, a, |s| event_answer(s, a));
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

    // An event of a calendar this account may only read keeps the old name there, and nothing
    // waits for it.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let e = server_event(&mut s, a, &["Old"]);
    let calendar = s.mailbox.event(e).unwrap().calendar;
    s.mailbox.calendars.iter_mut().find(|c| c.id == calendar).unwrap().rights = Rights::Read;
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    assert!(s.mailbox.event_changes.is_empty());
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

// ---- a change of the owner's list rewrites no other mailbox's items (#661) ----

#[test]
fn a_rename_or_a_removal_leaves_a_shared_mailboxs_items_and_other_peoples_calendars_alone() {
    for remove in [false, true] {
        let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
        let shared = s.run("account.addSharedMailbox", &json!({"account": a, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.category_list_owner(shared), Some(a), "a shared mailbox's items use its owner's list");
        let own = server_message(&mut s, a, &["Old"]);
        // A message of the shared mailbox that its server has, and an event of its calendar.
        let folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
        let there = ServerMessage { id: "m-2".into(), folder: "inbox".into(), categories: vec!["Old".into()], ..Default::default() };
        let theirs = s.mailbox.add_message(Message { folder, categories: vec!["Old".into()], server: Some(there), ..Default::default() }).unwrap();
        let shared_event = server_event(&mut s, shared, &["Old"]);
        // And an event of a colleague's calendar that the owner's account lists.
        let colleague = server_event(&mut s, a, &["Old"]);
        let calendar = s.mailbox.event(colleague).unwrap().calendar;
        s.mailbox.calendars.iter_mut().find(|c| c.id == calendar).unwrap().owner =
            Some(mailcraft_model::CalendarOwner { address: "ben@example.org".into(), name: "Ben".into() });
        // The owner's own calendar, whose owner Graph says is the account itself.
        let mine = server_event(&mut s, a, &["Old"]);
        owned_by(&mut s, mine, "A@example.org");
        if remove {
            s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
        } else {
            s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
        }
        let now = if remove { Vec::<String>::new() } else { vec!["New".to_string()] };
        assert_eq!(s.mailbox.message(own).unwrap().categories, now, "the owner's own mail");
        assert_eq!(s.mailbox.event(mine).unwrap().categories, now, "the owner's own event");
        assert_eq!(s.mailbox.message(theirs).unwrap().categories, ["Old"], "the shared mailbox's mail");
        assert_eq!(s.mailbox.event(shared_event).unwrap().categories, ["Old"], "the shared mailbox's event");
        assert_eq!(s.mailbox.event(colleague).unwrap().categories, ["Old"], "a colleague's event");
        // Only the owner's own event goes to its server.
        let told: Vec<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
        assert_eq!(told, [mine], "remove: {remove}");
        // Nothing of the shared mailbox's mail is to go.
        let (sent, _) = graph_up::message_changes(&s, shared, 10);
        assert!(sent.is_empty(), "{sent:?}");
    }
}

#[test]
fn on_the_list_the_rest_share_a_rename_reaches_a_shared_mailboxs_items_as_everybodys() {
    // A Microsoft account whose server's list wasn't read: it and its shared mailbox use the list
    // kept on no server, as on main before #661.
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = vec![tag("Old", GREEN)];
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": a, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.category_list_owner(shared), None, "the list the rest share");
    let folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.add_message(Message { folder, categories: vec!["Old".into()], ..Default::default() }).unwrap();
    s.run("category.rename", &json!({"from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.message(theirs).unwrap().categories, ["New"]);
}

/// On the list the rest share, a shared mailbox's events take a rename or a deletion here only,
/// as before #661: that list isn't the mailbox's own, so nothing goes to its server.
#[test]
fn on_the_list_the_rest_share_a_shared_mailboxs_events_change_here_only() {
    for (command, params, after) in
        [("category.rename", json!({"from": "Old", "to": "New"}), vec!["New".to_string()]), ("category.remove", json!({"name": "Old"}), Vec::new())]
    {
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        s.mailbox.categories = vec![tag("Old", GREEN)];
        let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let shared = s.run("account.addSharedMailbox", &json!({"account": a, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.category_list_owner(shared), None, "the list the rest share");
        let theirs = server_event(&mut s, shared, &["Old"]);
        s.run(command, &params).unwrap();
        assert_eq!(s.mailbox.event(theirs).unwrap().categories, after, "{command}: changed here");
        assert!(!s.mailbox.event_changes.iter().any(|c| c.event == theirs), "{command}: nothing goes to its server: {:?}", s.mailbox.event_changes);
    }
}

// ---- what an agent may do to Outlook's list without the owner (#661) ----

#[test]
fn an_agents_redo_of_add_to_outlook_asks_before_the_category_goes_there() {
    let (mut s, a) = rig_with_local();
    s.run("category.addToOutlook", &json!({"account": a, "name": "Old local"})).unwrap();
    // Undone before the job ran: kept on this computer only again, and an undo asks nothing more
    // of sending.
    assert_ne!(s.undo_reverts(false), Some(Class::Send));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Old local"));
    assert_eq!(s.undo_sends_categories(true), ["Old local"]);
    match check(&s, "edit.redo", &json!({}), &[]) {
        Verdict::Ask(Class::Send, text) => {
            assert!(text.contains("sends the category \u{201c}Old local\u{201d}, kept on this computer only, to Outlook"), "{text}");
            assert!(!text.contains("meeting"), "says only what the redo does: {text}");
        }
        other => panic!("an agent's redo sends a local category to Outlook unasked: {other:?}"),
    }
    // Redone, it goes with the next job, as Add to Outlook has it.
    s.run("edit.redo", &json!({})).unwrap();
    assert!(!s.mailbox.category_is_local(a, "Old local"));
    assert!(changes(&mut s, a).iter().any(|c| c.kind == CategoryChangeKind::Create && c.name == "Old local"));
}

#[test]
fn an_agent_asks_before_it_sends_a_category_to_outlook_or_renames_one_there() {
    let (s, a) = rig_with_local();
    // Sending one kept on this computer only to Outlook, as `tasks.sendLocal` for the to-dos.
    match check(&s, "category.addToOutlook", &json!({"account": a, "name": "Old local"}), &[]) {
        Verdict::Ask(Class::Send, text) => assert!(text.contains("\u{201c}Old local\u{201d}") && text.contains("to Outlook"), "{text}"),
        other => panic!("an agent sends a local category to Outlook unasked: {other:?}"),
    }
    // Renaming one Outlook has: the new name is made there and the old one deleted there.
    match check(&s, "category.rename", &json!({"account": a, "from": "Red category", "to": "Urgent"}), &[]) {
        // Whose Outlook: another account may have the same name.
        Verdict::Ask(Class::Send, text) => assert!(text.contains("in Outlook too") && text.contains("\u{201c}a@example.org\u{201d}"), "{text}"),
        other => panic!("an agent renames a category in Outlook unasked: {other:?}"),
    }
    assert_eq!(check(&s, "category.rename", &json!({"account": a, "from": "Red category", "to": "Urgent"}), &[Class::Send]), Verdict::Run);
    // Asked from a shared mailbox, both name the Outlook they change: its owner's.
    let mut s = s;
    let shared = s.run("account.addSharedMailbox", &json!({"account": a, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    for (command, params) in [
        ("category.addToOutlook", json!({"account": shared, "name": "Old local"})),
        ("category.rename", json!({"account": shared, "from": "Red category", "to": "Urgent"})),
    ] {
        match check(&s, command, &params, &[]) {
            Verdict::Ask(Class::Send, text) => {
                assert!(text.contains("\u{201c}a@example.org\u{201d}") && !text.contains("team@"), "{command}: {text}")
            }
            other => panic!("{command}: {other:?}"),
        }
    }
    // One kept on this computer only, or on the list IMAP accounts and this computer share, is
    // renamed here alone: nothing to ask.
    assert_eq!(check(&s, "category.rename", &json!({"account": a, "from": "Old local", "to": "Mine"}), &[]), Verdict::Run);
    let mut s = s;
    s.run("category.set", &json!({"name": "Shared", "color": "#2E8B57"})).unwrap();
    assert_eq!(check(&s, "category.rename", &json!({"from": "Shared", "to": "Other"}), &[]), Verdict::Run);
}

#[test]
fn an_agent_asks_before_it_makes_a_category_in_outlook_or_gives_one_another_colour_there() {
    let (mut s, a) = rig_with_local();
    let asks = |s: &Session, command: &str, params: &serde_json::Value, what: &str| match check(s, command, params, &[]) {
        Verdict::Ask(Class::Send, text) => {
            assert!(text.contains(what) && text.contains("\u{201c}a@example.org\u{201d}") && text.contains("in Outlook too"), "{command}: {text}")
        }
        other => panic!("{command} {params}: {other:?}"),
    };
    // New Category with a name the list hasn't got, or another colour for one Outlook has.
    asks(&s, "category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"}), "\u{201c}Travel\u{201d}");
    asks(&s, "category.set", &json!({"account": a, "name": "red CATEGORY", "color": "#2E8B57"}), "\u{201c}Red category\u{201d}");
    assert_eq!(check(&s, "category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"}), &[Class::Send]), Verdict::Run);
    // The colour it has, or one kept on this computer only: nothing goes to Outlook.
    assert_eq!(check(&s, "category.set", &json!({"account": a, "name": "Red category", "color": "#D13B3B"}), &[]), Verdict::Run);
    assert_eq!(check(&s, "category.set", &json!({"account": a, "name": "Old local", "color": "#2F6FD0"}), &[]), Verdict::Run);
    // What the command refuses asks nothing: a colour Outlook hasn't got, a list named wrongly.
    assert_eq!(check(&s, "category.set", &json!({"account": a, "name": "Travel", "color": "#123456"}), &[]), Verdict::Run);
    assert_eq!(check(&s, "category.set", &json!({"account": "a", "name": "Travel", "color": "#2E8B57"}), &[]), Verdict::Run);
    // Categorize with a name the item's list hasn't got: a message, named or selected.
    let m = message_of(&mut s, a, &[]);
    asks(&s, "mail.categorize", &json!({"ids": [m], "category": "Travel"}), "\u{201c}Travel\u{201d}");
    asks(&s, "mail.categorize", &json!({"id": m, "category": "Travel"}), "\u{201c}Travel\u{201d}");
    s.sel.messages = vec![m];
    asks(&s, "mail.categorize", &json!({"category": "Travel"}), "\u{201c}Travel\u{201d}");
    // One the list has, or a name taken off: nothing is made there.
    assert_eq!(check(&s, "mail.categorize", &json!({"ids": [m], "category": "red category"}), &[]), Verdict::Run);
    assert_eq!(check(&s, "mail.categorize", &json!({"ids": [m], "category": "Travel", "value": false}), &[]), Verdict::Run);
    // On a message of an account whose list isn't Outlook's, nothing goes there either.
    s.sel.messages.clear();
    let other = s.run("account.add", &json!({"email": "b@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = message_of(&mut s, other, &[]);
    assert_eq!(check(&s, "mail.categorize", &json!({"ids": [here], "category": "Travel"}), &[]), Verdict::Run);
    // An event, a contact and a to-do.
    for (command, id) in items_of(&mut s, a) {
        asks(&s, command, &json!({"id": id, "category": "Travel"}), "\u{201c}Travel\u{201d}");
        assert_eq!(check(&s, command, &json!({"id": id, "category": "Red category"}), &[]), Verdict::Run, "{command}");
    }
    // Done after the yes, it is made there.
    s.run("mail.categorize", &json!({"ids": [m], "category": "Travel"})).unwrap();
    assert!(changes(&mut s, a).iter().any(|c| c.kind == CategoryChangeKind::Create && c.name == "Travel"));
}

#[test]
fn an_agents_deletion_of_a_category_a_shared_to_do_has_needs_delete_too_when_it_goes_from_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let origin = Origin { account: a, id: "t-1".into(), ..Default::default() };
    s.mailbox.add_task(Task { title: "Ours".into(), categories: vec!["Red category".into()], origin, list: "l-1".into(), ..Default::default() });
    s.mailbox.shared_task_lists.insert(a, ["l-1".to_string()].into());
    let remove = json!({"account": a, "name": "Red category"});
    // One question names both, and its yes covers both.
    match check(&s, "category.remove", &remove, &[]) {
        Verdict::Ask(Class::Send, text) => assert!(text.contains("from Outlook") && text.contains("shared with other people"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(check(&s, "category.remove", &remove, &[Class::Send]), Verdict::Ask(Class::Delete, _)));
    assert_eq!(check(&s, "category.remove", &remove, &[Class::Send, Class::Delete]), Verdict::Run);
    // Kept on this computer alone, it goes from nowhere else: `send` is enough, as before.
    s.run("category.set", &json!({"account": a, "name": "Mine", "color": "#2E8B57"})).unwrap();
    s.mailbox.local_categories.entry(a).or_default().push("Mine".into());
    s.mailbox.task_mut(s.mailbox.tasks.keys().copied().next().unwrap()).unwrap().categories.push("Mine".into());
    assert_eq!(check(&s, "category.remove", &json!({"account": a, "name": "Mine"}), &[Class::Send]), Verdict::Run);
}

#[test]
fn an_agent_is_refused_at_once_a_change_of_a_list_mailcraft_may_not_change_in_outlook() {
    let (mut s, a) = rig_with_local();
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": false})).unwrap();
    let lists =
        |mb: &Mailbox| (mb.account_categories.clone(), mb.local_categories.clone(), mb.synced_categories.clone(), mb.category_renames.clone());
    let before = lists(&s.mailbox);
    for (command, params) in [
        ("category.set", json!({"account": a, "name": "Travel", "color": "#2E8B57"})),
        ("category.rename", json!({"account": a, "from": "Red category", "to": "Urgent"})),
        ("category.remove", json!({"account": a, "name": "Red category"})),
        ("category.addToOutlook", json!({"account": a, "name": "Old local"})),
    ] {
        match check(&s, command, &params, &[]) {
            Verdict::Refuse(text) => {
                // (Not "no permission": it may be that nobody looked yet.)
                assert!(
                    text.contains("read-only here until MailCraft may change them there")
                        && !text.contains("no permission")
                        && !text.contains("Outlook too"),
                    "{command}: {text}"
                )
            }
            other => panic!("{command}: {other:?}"),
        }
        assert!(s.run(command, &params).is_err(), "{command}");
    }
    // Categorize with a name the list hasn't got asks nothing of Outlook: the command refuses it.
    let m = message_of(&mut s, a, &[]);
    assert_eq!(check(&s, "mail.categorize", &json!({"ids": [m], "category": "Travel"}), &[]), Verdict::Run);
    assert!(s.run("mail.categorize", &json!({"ids": [m], "category": "Travel"})).is_err());
    assert!(lists(&s.mailbox) == before);
}

#[test]
fn a_refusal_for_a_category_taken_off_here_meanwhile_says_nothing_of_what_it_has_here() {
    // A new one, taken off while the job that makes it is on its way: kept nowhere.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Travel", GREEN)]);
    s.run("category.remove", &json!({"account": a, "name": "Travel"})).unwrap();
    let said = answers(&mut s, a, vec![outcome(CategoryChangeKind::Create, "Travel", GREEN, CategoryResult::Refused)]);
    let lines = said["messages"].to_string();
    assert!(lines.contains("didn't take the category") && !lines.contains("stays on this computer"), "{lines}");
    assert!(s.mailbox.category_of(a, "Travel").is_none() && !s.mailbox.category_is_local(a, "Travel"));
    // A new colour, and the category taken off meanwhile: it goes from Outlook next.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    s.run("category.set", &json!({"account": a, "name": "Old", "color": "#2F6FD0"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Color, "Old", BLUE)]);
    s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
    let said = answers(&mut s, a, vec![outcome(CategoryChangeKind::Color, "Old", BLUE, CategoryResult::Refused)]);
    let lines = said["messages"].to_string();
    assert!(lines.contains("didn't take the new colour") && !lines.contains("server's again"), "{lines}");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Old", GREEN)]);
}

#[test]
fn a_name_another_device_made_in_outlook_meanwhile_keeps_outlooks_colour_and_says_so() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": "#2E8B57"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Travel", GREEN)]);
    // The job found it there already, in blue: it is blue here too, and nothing more goes.
    let said = answers(&mut s, a, vec![CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "Travel", BLUE, CategoryResult::Done) }]);
    assert_eq!(s.mailbox.category_of(a, "Travel").map(|c| c.color), Some(BLUE));
    assert!(said["notices"].to_string().contains("was in Outlook already") && said["messages"] == json!([]), "{said}");
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    // Found there in the colour it has here: nothing to say.
    s.run("category.set", &json!({"account": a, "name": "Home", "color": "#2E8B57"})).unwrap();
    let said = answers(&mut s, a, vec![CategoryOutcome { found: true, ..outcome(CategoryChangeKind::Create, "Home", GREEN, CategoryResult::Done) }]);
    assert_eq!((&said["messages"], &said["notices"]), (&json!([]), &json!([])));
}

#[test]
fn other_letters_refused_after_the_old_ones_went_make_the_old_ones_there_again() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("client", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "client", "to": "Client"})).unwrap();
    let said = answers(
        &mut s,
        a,
        vec![
            outcome(CategoryChangeKind::Delete, "client", GREEN, CategoryResult::Done),
            outcome(CategoryChangeKind::Create, "Client", GREEN, CategoryResult::Refused),
        ],
    );
    assert!(said["messages"].to_string().contains("new spelling"), "{said}");
    // Not kept on this computer alone: the old letters go back there, in their colour.
    assert!(!s.mailbox.category_is_local(a, "client"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "client", GREEN)]);
}

#[test]
fn a_new_name_the_server_wont_make_keeps_its_old_name_there_and_lets_other_deletions_go() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN), tag("Gone", RED)], true);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "Newer"})).unwrap();
    // Renamed again before it went: what the server has is still "Old".
    s.run("category.rename", &json!({"account": a, "from": "Newer", "to": "New"})).unwrap();
    s.run("category.remove", &json!({"account": a, "name": "Gone"})).unwrap();
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "New", GREEN)]);
    let said = answer(&mut s, a, CategoryChangeKind::Create, "New", GREEN, CategoryResult::Refused);
    assert!(said["messages"].to_string().contains("\"Old\\\" wasn't deleted in Outlook"), "{said}");
    assert!(!said["messages"].to_string().contains("Gone"), "{said}");
    // The owner's own deletion goes; the rename's old name stays there.
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Gone", RED)]);
    assert!(s.mailbox.category_renames.is_empty());
}

#[test]
fn an_undo_of_a_rename_before_its_event_change_goes_sends_nothing_and_once_it_went_there_is_none() {
    // Undone while the event's change waits: the change goes with the undo, and nothing is sent.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    let e = server_event(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    assert_eq!(s.mailbox.event_changes.len(), 1);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.event(e).unwrap().categories, ["Old"]);
    assert!(s.mailbox.event_changes.is_empty(), "{:?}", s.mailbox.event_changes);
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));

    // Once the server took the event's change, the undo history is forgotten, as for any change
    // of an event that reached its server: there is nothing to undo, and nothing goes back.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Old", GREEN)], true);
    server_event(&mut s, a, &["Old"]);
    s.run("category.rename", &json!({"account": a, "from": "Old", "to": "New"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    event_answer(&mut s, a);
    assert!(!s.can_undo());
}

#[test]
fn an_agents_undo_of_an_address_change_sends_nothing_the_old_server_has_and_says_so() {
    let (mut s, a) = rig_with_local();
    s.run("account.edit", &json!({"id": a, "email": "other@example.org"})).unwrap();
    // The other mailbox's list: what the old one had is kept on this computer alone there.
    read_as(&mut s, a, "other@example.org", vec![tag("Theirs", GREEN)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    assert!(s.mailbox.category_is_local(a, "Red category"));
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Accounts, text) => assert!(!text.contains("sends the categor"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(s.undo_sends_categories(false).is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(a).unwrap().email, "a@example.org");
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
}

/// The old name's deletion in Outlook waits only for an item still to change there (#661): a
/// message gone from its server, or that it never gave an id, doesn't hold it, nor does a to-do
/// of an account whose to-dos aren't kept in To Do. Two deletions waiting are looked at together:
/// only the one an item still carries there waits.
#[test]
fn a_deletion_waits_only_for_an_item_still_to_change_there() {
    let colours = || vec![tag("Red category", RED), tag("Old", GREEN), tag("Other", BLUE)];
    for (gone, id) in [(false, "m-1"), (true, "m-1"), (false, "")] {
        let (mut s, a) = rig(colours(), true);
        let m = server_message(&mut s, a, &["Old"]);
        let there = s.mailbox.message_mut(m).unwrap().server.as_mut().unwrap();
        (there.gone, there.id) = (gone, id.into());
        s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
        s.run("category.remove", &json!({"account": a, "name": "Other"})).unwrap();
        let mut want = vec![change(CategoryChangeKind::Delete, "Other", BLUE)];
        if gone || id.is_empty() {
            want.insert(0, change(CategoryChangeKind::Delete, "Old", GREEN));
        }
        assert_eq!(changes(&mut s, a), want, "gone {gone}, id {id:?}");
    }
    for keeps in [true, false] {
        let (mut s, a) = rig(colours(), true);
        server_task(&mut s, a, &["Old"]);
        if !keeps {
            s.mailbox.accounts.iter_mut().find(|x| x.id == a).unwrap().oauth = None;
        }
        s.run("category.remove", &json!({"account": a, "name": "Old"})).unwrap();
        let goes = changes(&mut s, a).iter().any(|c| c.name == "Old");
        assert_eq!(goes, !keeps, "keeps to-dos: {keeps}");
    }
}

#[test]
fn an_agent_asks_before_a_new_or_changed_todo_or_event_makes_a_category_in_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.mailbox.accounts.iter_mut().find(|x| x.id == a).unwrap().oauth =
        Some(mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Microsoft, ..Default::default() });
    // (Where the owner's to-dos go: a new one there asks nothing for that.)
    s.mailbox.local_tasks = mailcraft_model::LocalTasks::SendTo(a);
    let items = items_of(&mut s, a);
    let (event, task) = (items[0].1, items[2].1);
    let calendar = s.mailbox.event(event).unwrap().calendar;
    let asks = |s: &Session, command: &str, params: serde_json::Value, what: &str| match check(s, command, &params, &[]) {
        Verdict::Ask(Class::Send, text) => assert!(
            text.contains(what) && text.contains("\u{201c}a@example.org\u{201d}") && text.contains("made in Outlook too"),
            "{command}: {text}"
        ),
        other => panic!("{command} {params}: {other:?}"),
    };
    let travel = "the category \u{201c}Travel\u{201d}";
    asks(&s, "tasks.new", json!({"title": "T", "categories": ["Travel"], "account": a}), travel);
    asks(&s, "tasks.edit", json!({"id": task, "categories": ["Travel"]}), travel);
    asks(&s, "calendar.new", json!({"title": "E", "calendar": calendar, "categories": ["Travel"]}), travel);
    asks(&s, "calendar.edit", json!({"id": event, "categories": ["Red category", "Travel"]}), travel);
    // Two new names are named both.
    asks(&s, "tasks.edit", json!({"id": task, "categories": ["Travel", "Trips"]}), "\u{201c}Travel\u{201d}; \u{201c}Trips\u{201d}");
    // Names the list has, in any letters, a to-do kept on this computer, or a call the command
    // refuses: nothing is made in Outlook, and nothing is asked.
    for (command, params) in [
        ("tasks.new", json!({"title": "T", "categories": ["red CATEGORY"], "account": a})),
        ("tasks.new", json!({"title": "T", "categories": ["Travel"], "account": 0})),
        ("tasks.new", json!({"title": "", "categories": ["Travel"], "account": a})),
        ("tasks.edit", json!({"id": task, "categories": ["Red category"]})),
        ("tasks.edit", json!({"id": task, "categories": ["Travel"], "account": 0})),
        ("calendar.edit", json!({"id": event, "categories": ["Travel"], "calendar": 999_999})),
    ] {
        assert_eq!(check(&s, command, &params, &[]), Verdict::Run, "{command} {params}");
    }
    assert_eq!(check(&s, "tasks.edit", &json!({"id": task, "categories": ["Travel"]}), &[Class::Send]), Verdict::Run);
    // What the question named is what the yes makes there.
    s.run("tasks.edit", &json!({"id": task, "categories": ["Travel", "Trips"]})).unwrap();
    let mut made: Vec<String> = changes(&mut s, a).into_iter().filter(|c| c.kind == CategoryChangeKind::Create).map(|c| c.name).collect();
    made.sort();
    assert_eq!(made, ["Travel", "Trips"]);
}

#[test]
fn an_agent_asks_before_a_rule_gives_mail_a_category_that_is_made_in_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let rule = |name: &str, account: Option<Id>| {
        let mut r = json!({"name": "R", "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "categorize", "value": name}], "server": false});
        if let Some(account) = account {
            r["account"] = json!(account);
        }
        r
    };
    // A rule for this account's mail, or for every account's, with a name its list hasn't got.
    for params in [rule("Travel", Some(a)), rule("Travel", None)] {
        match check(&s, "rules.add", &params, &[Class::Settings]) {
            Verdict::Ask(Class::Send, text) => assert!(text.contains("\u{201c}Travel\u{201d}") && text.contains("made in Outlook too"), "{text}"),
            other => panic!("{params}: {other:?}"),
        }
    }
    // One the list has: nothing more than the rule.
    assert_eq!(check(&s, "rules.add", &rule("red category", Some(a)), &[Class::Settings]), Verdict::Run);
    // Run over the folder, it gives the mail there the name, and the list takes it.
    s.run("rules.add", &rule("Travel", Some(a))).unwrap();
    assert!(matches!(check(&s, "rules.run", &json!({"folder": inbox}), &[]), Verdict::Ask(Class::Send, _)));
    // A rule for another account's mail asks nothing about this list.
    let other = s.run("account.add", &json!({"email": "b@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(check(&s, "rules.add", &rule("Travel", Some(other)), &[Class::Settings]), Verdict::Run);
}

/// A rule that moves mail into another account's folder before it gives it a category puts the
/// category on the list of the account the mail lands in, as the rule runs, or, where the move is
/// refused, on its own: an agent's question names both lists (#661).
#[test]
fn a_rule_that_moves_mail_before_it_categorises_it_names_the_list_the_mail_lands_in() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let b = s.run("account.add", &json!({"email": "b@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    read_as(&mut s, b, "b@example.org", vec![tag("Red category", RED)]);
    s.run("sync.categoryWrites", &json!({"account": b, "allowed": true})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "R".into(),
        enabled: true,
        account: a,
        conditions: vec![Condition::SubjectContains("x".into())],
        actions: vec![Action::MoveTo(theirs), Action::Categorize("Zed".into())],
        ..Default::default()
    });
    match check(&s, "rules.run", &json!({"folder": inbox}), &[]) {
        Verdict::Ask(Class::Send, text) => {
            assert!(text.contains("\u{201c}Zed\u{201d}") && text.contains("b@example.org") && text.contains("a@example.org"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    // One whole message, moved; one with only its headers here, which can't be.
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "x".into(), ..Default::default() }).unwrap();
    let headers = s.mailbox.add_message(Message { folder: inbox, subject: "x 2".into(), incomplete: true, ..Default::default() }).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().account, b);
    assert_eq!(s.mailbox.message(headers).unwrap().account, a);
    assert!(s.mailbox.category_of(b, "Zed").is_some() && s.mailbox.category_of(a, "Zed").is_some());
}

/// The rules run in one order on mail, those of the account it came to, wherever an earlier one
/// moved it: a rule that only categorises, after one that moves mail to another account, puts the
/// category on that account's list, and an agent is asked `send` for it, the list named (#661).
#[test]
fn a_category_given_after_another_rule_moved_the_mail_is_asked_for_on_the_list_it_lands_on() {
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Zed", RED)], true);
    let b = s.run("account.add", &json!({"email": "b@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    read_as(&mut s, b, "b@example.org", vec![tag("Red category", RED)]);
    s.run("sync.categoryWrites", &json!({"account": b, "allowed": true})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    s.mailbox.rules.push(Rule {
        id: 900,
        name: "Move".into(),
        enabled: true,
        account: a,
        conditions: vec![Condition::SubjectContains("x".into())],
        actions: vec![Action::MoveTo(theirs)],
        ..Default::default()
    });
    let tag_it = json!({"name": "Tag", "account": a, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "categorize", "value": "Zed"}], "server": false});
    let asked = |s: &Session, command: &str, params: &serde_json::Value, allow: &[Class]| match check(s, command, params, allow) {
        Verdict::Ask(Class::Send, text) => assert!(text.contains("\u{201c}Zed\u{201d}") && text.contains("b@example.org"), "{text}"),
        other => panic!("{command}: {other:?}"),
    };
    asked(&s, "rules.add", &tag_it, &[Class::Settings]);
    s.run("rules.add", &tag_it).unwrap();
    asked(&s, "rules.run", &json!({"folder": inbox}), &[]);
    // Switching the move on, with the rule that categorises after it, names the category too.
    let switch = |s: &mut Session, on: bool| s.mailbox.rules.iter_mut().filter(|r| r.id == 900).for_each(|r| r.enabled = on);
    switch(&mut s, false);
    asked(&s, "rules.enable", &json!({"id": 900, "value": true}), &[Class::Settings]);
    // And so does one that moves every account's mail.
    s.mailbox.rules.iter_mut().filter(|r| r.id == 900).for_each(|r| r.account = 0);
    asked(&s, "rules.enable", &json!({"id": 900, "value": true}), &[Class::Settings]);
    s.mailbox.rules.iter_mut().filter(|r| r.id == 900).for_each(|r| r.account = a);
    switch(&mut s, true);
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "x".into(), ..Default::default() }).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().account, b);
    assert!(s.mailbox.category_of(b, "Zed").is_some());
}

/// What a rule gives mail depends on which rules run before it: one that sets the importance a
/// later one matches, one that stops the rest, the order. So switching a rule on or off,
/// deleting or moving one asks `send` while a rule that runs with it gives mail a name new on a
/// list kept in step with Outlook, and names it (#661).
#[test]
fn a_change_of_which_rules_run_asks_for_the_categories_the_rules_may_then_give() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let rule = |id: Id, enabled: bool, stop: bool, conditions: Vec<Condition>, actions: Vec<Action>| Rule {
        id,
        name: format!("R{id}"),
        enabled,
        stop,
        account: a,
        conditions,
        actions,
        ..Default::default()
    };
    let high = mailcraft_model::Importance::High;
    s.mailbox.rules = vec![
        rule(901, false, false, vec![Condition::SubjectContains("x".into())], vec![Action::SetImportance(high)]),
        rule(902, true, true, vec![Condition::SubjectContains("y".into())], vec![Action::Flag]),
        rule(903, true, false, vec![Condition::Importance(high)], vec![Action::Categorize("Zed".into())]),
    ];
    let asked = |s: &Session, command: &str, params: serde_json::Value| match check(s, command, &params, &[Class::Settings]) {
        Verdict::Ask(Class::Send, text) => assert!(text.contains("\u{201c}Zed\u{201d}") && text.contains("a@example.org"), "{command}: {text}"),
        other => panic!("{command}: {other:?}"),
    };
    // Switching on the rule that sets the importance the last one matches.
    asked(&s, "rules.enable", json!({"id": 901, "value": true}));
    // Switching off, deleting or moving the one that stops the rest.
    asked(&s, "rules.enable", json!({"id": 902, "value": false}));
    asked(&s, "rules.remove", json!({"id": 902}));
    asked(&s, "rules.move", json!({"id": 902, "direction": "down"}));
    // Run, the name goes on the list, and nothing is asked for it again.
    s.mailbox.rules.iter_mut().filter(|r| r.id == 901).for_each(|r| r.enabled = true);
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(Message { folder: inbox, subject: "x".into(), ..Default::default() }).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert!(s.mailbox.category_of(a, "Zed").is_some());
    assert_eq!(check(&s, "rules.remove", &json!({"id": 902}), &[Class::Settings]), Verdict::Run);
}

/// An agent's undo or redo that puts back rules is asked as the rule call it undoes or redoes:
/// `send`, while the rules that then run give mail a name new on a list kept in step with
/// Outlook, and naming it (#661).
#[test]
fn an_undo_or_redo_of_a_rule_change_asks_for_the_categories_the_rules_may_then_give() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let rule = |id: Id, stop: bool, actions: Vec<Action>| Rule {
        id,
        name: format!("R{id}"),
        enabled: true,
        stop,
        account: a,
        conditions: vec![Condition::SubjectContains("y".into())],
        actions,
        ..Default::default()
    };
    s.mailbox.rules = vec![rule(902, true, vec![Action::Flag]), rule(903, false, vec![Action::Categorize("Zed".into())])];
    let asked = |s: &Session, command: &str| match check(s, command, &json!({}), &[Class::Settings]) {
        Verdict::Ask(Class::Send, text) => assert!(text.contains("\u{201c}Zed\u{201d}") && text.contains("a@example.org"), "{command}: {text}"),
        other => panic!("{command}: {other:?}"),
    };
    // The rule that gives the name, deleted and put back.
    s.run("rules.remove", &json!({"id": 903})).unwrap();
    asked(&s, "edit.undo");
    s.run("edit.undo", &json!({})).unwrap();
    // The one that stops the rest, deleted again.
    s.run("rules.remove", &json!({"id": 902})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    asked(&s, "edit.redo");
    // A step that leaves the rules as they are asks what it asked before.
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let m = s.mailbox.add_message(Message { folder: inbox, ..Default::default() }).unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    // With the name on the list, nothing is asked for it.
    s.run("mail.categorize", &json!({"ids": [m], "category": "Zed"})).unwrap();
    assert!(s.mailbox.category_of(a, "Zed").is_some());
    s.run("rules.remove", &json!({"id": 903})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Settings]), Verdict::Run);
}

/// Deleting a rule its server has is a delete, and while a rule that runs with it gives mail a
/// name new on a list kept in step with Outlook, a send too: both are asked at once (#661).
#[test]
fn deleting_a_server_rule_that_lets_another_give_a_new_category_asks_send_and_delete() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let server = mailcraft_model::ServerRule { id: "srv-1".into(), synced_enabled: true, ..Default::default() };
    s.mailbox.rules = vec![
        Rule {
            id: 911,
            name: "Stop".into(),
            enabled: true,
            stop: true,
            account: a,
            conditions: vec![Condition::SubjectContains("y".into())],
            actions: vec![Action::Flag],
            keep: mailcraft_model::Keep::Server,
            server: Some(server),
            ..Default::default()
        },
        Rule {
            id: 912,
            name: "Tag".into(),
            enabled: true,
            account: a,
            conditions: vec![Condition::SubjectContains("y".into())],
            actions: vec![Action::Categorize("Zed".into())],
            ..Default::default()
        },
    ];
    let params = json!({"id": 911});
    assert!(!matches!(check(&s, "rules.remove", &params, &[Class::Send]), Verdict::Run), "it deletes a rule on its server");
    assert!(!matches!(check(&s, "rules.remove", &params, &[Class::Delete]), Verdict::Run), "it makes a category in Outlook");
    assert!(matches!(check(&s, "rules.remove", &params, &[Class::Send, Class::Delete]), Verdict::Ask(Class::Settings, _)), "it changes the rules");
    assert_eq!(check(&s, "rules.remove", &params, &[Class::Send, Class::Delete, Class::Settings]), Verdict::Run);
}

/// A rule call asked about as a send (a category made in Outlook) is a change of rules too, and
/// so is an undo or redo that puts back rules, whatever else it is asked about as: `--allow send`
/// alone lets neither run, and one question covers both (#661).
#[test]
fn a_rule_call_or_an_undo_asked_about_for_something_else_needs_settings_too() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let rule = |id: Id, actions: Vec<Action>| Rule {
        id,
        name: format!("R{id}"),
        enabled: true,
        account: a,
        conditions: vec![Condition::SubjectContains("y".into())],
        actions,
        ..Default::default()
    };
    s.mailbox.rules = vec![rule(902, vec![Action::Flag]), rule(903, vec![Action::Categorize("Zed".into())])];
    let tag_it = json!({"name": "T", "account": a, "conditions": [{"type": "subjectContains", "value": "z"}], "actions": [{"type": "categorize", "value": "Zed"}]});
    for (command, params) in [("rules.remove", json!({"id": 902})), ("rules.add", tag_it)] {
        assert!(matches!(check(&s, command, &params, &[Class::Send]), Verdict::Ask(Class::Settings, _)), "{command}");
        assert_eq!(check(&s, command, &params, &[Class::Send, Class::Settings]), Verdict::Run, "{command}");
    }
    // Undone: the rules it puts back are said with the categories they give.
    s.run("rules.remove", &json!({"id": 903})).unwrap();
    match check(&s, "edit.undo", &json!({}), &[Class::Send]) {
        Verdict::Ask(Class::Settings, text) => {
            assert!(text.contains("puts the rules, signatures or Safe and Blocked Senders back") && text.contains("Zed"), "{text}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Send, Class::Settings]), Verdict::Run);
    // A step that also puts back the accounts: the accounts, and the rules too.
    if let Some(account) = s.history.last_mut().and_then(|step| step.mailbox.accounts.first_mut()) {
        account.name = "Before".into();
    }
    assert_eq!(s.undo_reverts(false), Some(Class::Accounts));
    assert!(crate::agent::also(&s, "edit.undo", &json!({}), Class::Accounts).contains(&Class::Settings));
    assert!(crate::agent::describe(&s, "edit.undo", &json!({})).contains("the rules, signatures or Safe and Blocked Senders"));
    // And a change that writes to people that it puts back is said as well.
    if let Some(step) = s.history.last_mut() {
        step.mailbox.event_changes.push(mailcraft_model::remote::EventChange { account: a, send: true, ..Default::default() });
    }
    assert!(s.undo_sends_items(false));
    assert!(crate::agent::describe(&s, "edit.undo", &json!({})).contains("a change to a meeting"));
}

#[test]
fn an_agents_undo_or_redo_that_makes_a_category_in_outlook_again_asks_send_and_says_so() {
    let ask = |s: &Session, command: &str, allow: &[Class], what: &[&str]| match check(s, command, &json!({}), allow) {
        Verdict::Ask(Class::Send, text) => {
            for w in what {
                assert!(text.contains(w), "{command}: {w} in {text}");
            }
        }
        other => panic!("{command}: {other:?}"),
    };
    // A deletion undone after it went: the next job makes the category there again.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Theirs", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Theirs"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    ask(&s, "edit.undo", &[], &["makes the category \u{201c}Theirs\u{201d} in Outlook"]);
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Send]), Verdict::Run);
    // Undone before anything went, nothing is written there: nothing to ask.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Theirs", GREEN)], true);
    s.run("category.remove", &json!({"account": a, "name": "Theirs"})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    // A new one redone after its undo went there.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Fresh", "color": "#2E8B57"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    s.run("edit.undo", &json!({})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    ask(&s, "edit.redo", &[], &["makes the category \u{201c}Fresh\u{201d} in Outlook"]);
    // A colour undone after it went.
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Red category", "color": "#2E8B57"})).unwrap();
    let sent = changes(&mut s, a);
    took(&mut s, a, &sent);
    ask(&s, "edit.undo", &[], &["gives the category \u{201c}Red category\u{201d} another colour in Outlook"]);
    // A rename undone after it went: the old name is made there again and the new one deleted
    // there, both said, and asked as one question whose yes covers both.
    let (mut s, a) = rig(vec![tag("Red category", RED), tag("Theirs", GREEN)], true);
    s.run("category.rename", &json!({"account": a, "from": "Theirs", "to": "Trips"})).unwrap();
    for _ in 0..2 {
        let sent = changes(&mut s, a);
        took(&mut s, a, &sent);
    }
    assert!(s.mailbox.synced_categories[&a].iter().all(|c| c.name != "Theirs"));
    ask(&s, "edit.undo", &[], &["makes the category \u{201c}Theirs\u{201d} in Outlook", "deletes the category \u{201c}Trips\u{201d} in Outlook"]);
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[Class::Send]), Verdict::Ask(Class::Delete, _)));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Send, Class::Delete]), Verdict::Run);
}

/// An answer from the host that names no category (an empty or blank name) changes nothing of
/// what is known to be there, so nothing is deleted there for it with the next job.
#[test]
fn an_answer_about_a_category_with_no_name_is_left_out() {
    for kind in [CategoryChangeKind::Create, CategoryChangeKind::Color, CategoryChangeKind::Delete] {
        for result in [CategoryResult::Done, CategoryResult::Refused] {
            let (mut s, a) = rig(vec![tag("Red category", RED)], true);
            let before = s.mailbox.synced_categories.get(&a).cloned();
            answers(&mut s, a, vec![outcome(kind, " \n ", GREEN, result)]);
            assert_eq!(s.mailbox.synced_categories.get(&a).cloned(), before, "{kind:?} {result:?}");
            assert!(changes(&mut s, a).is_empty(), "{kind:?} {result:?}: {:?}", changes(&mut s, a));
            assert!(s.mailbox.local_categories.get(&a).is_none_or(|names| names.is_empty()), "{kind:?} {result:?}");
        }
    }
}

/// A name an agent gives with spaces round it is the name the command reads: one kept on this
/// computer only is renamed here alone, unasked, is deleted as it is without the spaces, and goes
/// to Outlook when asked to.
#[test]
fn a_name_with_spaces_round_it_is_read_as_the_command_reads_it() {
    let (mut s, a) = rig_with_local();
    assert_eq!(check(&s, "category.rename", &json!({"account": a, "from": " Old local ", "to": "Mine"}), &[]), Verdict::Run);
    let remove = |name: &str| check(&s, "category.remove", &json!({"account": a, "name": name}), &[]);
    assert_eq!(remove(" Old local "), remove("Old local"));
    s.run("category.addToOutlook", &json!({"account": a, "name": " Old local "})).unwrap();
    assert!(!s.mailbox.category_is_local(a, "Old local"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Create, "Old local", s.mailbox.category_of(a, "Old local").unwrap().color)]);
}

/// One day of a series keeps to the series' calendar, whatever `calendar` an agent names: a new
/// name it is given joins that calendar's list, kept in step with Outlook, and the agent is asked
/// before, as for the series (#661).
#[test]
fn an_agent_asks_before_one_day_of_a_series_makes_a_category_in_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let series = server_event(&mut s, a, &[]);
    let e = s.mailbox.event_mut(series).unwrap();
    e.start = mailcraft_time::parse_iso("2027-01-04T09:00").unwrap().0;
    e.end = mailcraft_time::parse_iso("2027-01-04T09:30").unwrap().0;
    e.recurrence = Some(mailcraft_model::Recurrence { count: Some(5), ..mailcraft_model::Recurrence::every(mailcraft_model::Freq::Daily) });
    let mine = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id: mine, name: "Mine".into(), visible: true, ..Default::default() });
    for calendar in [Some(json!(mine)), Some(json!(999_999)), None] {
        let mut params = json!({"id": series, "occurrence": "2027-01-05", "categories": ["Travel"]});
        if let Some(calendar) = calendar {
            params["calendar"] = calendar;
        }
        match check(&s, "calendar.edit", &params, &[]) {
            Verdict::Ask(Class::Send, text) => {
                assert!(text.contains("\u{201c}Travel\u{201d}") && text.contains("made in Outlook too"), "{params}: {text}")
            }
            other => panic!("{params}: {other:?}"),
        }
    }
    // What the yes runs makes it there.
    s.run("calendar.edit", &json!({"id": series, "occurrence": "2027-01-05", "calendar": mine, "categories": ["Travel"]})).unwrap();
    assert!(changes(&mut s, a).iter().any(|c| c.kind == CategoryChangeKind::Create && c.name == "Travel"));
}

/// A meeting kept on this computer (`send`: false) goes into a calendar of this computer's: a new
/// name it is given joins the list the accounts share, nothing is made in Outlook, and nothing
/// is asked for it (#661).
#[test]
fn a_meeting_kept_on_this_computer_makes_no_category_in_outlook_and_asks_nothing_for_it() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let event = server_event(&mut s, a, &[]);
    let calendar = s.mailbox.event(event).unwrap().calendar;
    for command in ["calendar.new", "calendar.newMeeting"] {
        let params = json!({"title": "M", "calendar": calendar, "attendees": "pat@example.org", "send": false, "categories": ["Travel"]});
        assert_eq!(check(&s, command, &params, &[]), Verdict::Run, "{command}");
    }
    s.run(
        "calendar.newMeeting",
        &json!({"title": "M", "calendar": calendar, "attendees": "pat@example.org", "send": false, "categories": ["Travel"]}),
    )
    .unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert!(s.mailbox.category_of(0, "Travel").is_some());
}

/// A meeting with a contact kept on this computer (`send: false` on a calendar on a server) is a
/// new meeting with the contact invited: its new category joins the list the accounts share, and
/// the agent's question doesn't say it is made in Outlook (#661).
#[test]
fn a_meeting_with_a_contact_kept_on_this_computer_is_not_said_to_make_a_category_in_outlook() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    let event = server_event(&mut s, a, &[]);
    let calendar = s.mailbox.event(event).unwrap().calendar;
    let pat = mailcraft_model::Labeled { label: "work".into(), value: "pat@example.org".into() };
    let contact = s.mailbox.add_contact(Contact { display_name: "Pat".into(), emails: vec![pat], ..Default::default() });
    let params = json!({"id": contact, "title": "M", "calendar": calendar, "send": false, "categories": ["Travel"]});
    let said = crate::agent::describe(&s, "people.meeting", &params);
    assert!(!said.contains("Outlook"), "{said}");
    s.run("people.meeting", &params).unwrap();
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    assert!(s.mailbox.category_of(0, "Travel").is_some());
}

/// An undo that puts an account back as a Microsoft account, with a category of its list still to
/// go to Outlook, needs `send` too, not only `accounts`, and asks both at once (#661).
#[test]
fn an_undo_that_puts_back_an_account_and_what_it_sends_needs_send_too() {
    let (mut s, a) = rig(vec![tag("Red category", RED)], true);
    s.run("category.set", &json!({"account": a, "name": "Travel", "color": GREEN.hex()})).unwrap();
    s.run("account.edit", &json!({"id": a, "kind": "imap"})).unwrap();
    assert_eq!(s.undo_reverts(false), Some(Class::Accounts));
    assert!(!s.undo_sends_categories(false).is_empty());
    assert!(!matches!(check(&s, "edit.undo", &json!({}), &[Class::Accounts]), Verdict::Run), "it sends a category to Outlook");
    assert_eq!(crate::agent::also(&s, "edit.undo", &json!({}), Class::Accounts), [Class::Send]);
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Accounts, Class::Send]), Verdict::Run);
}

/// The question about deleting a category says it goes from Outlook only when it does: not for
/// one kept on this computer only, nor for a name the list hasn't got (#661).
#[test]
fn the_question_about_a_deletion_says_outlook_only_when_it_goes_from_there() {
    let (s, a) = rig_with_local();
    let says = |name: &str| crate::agent::describe(&s, "category.remove", &json!({"account": a, "name": name})).contains("from Outlook");
    assert!(says("Red category"));
    assert!(!says("Old local"));
    assert!(!says("Not on the list"));
}
