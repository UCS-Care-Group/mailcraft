// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The People and Tasks tabs' queries, and the commands they drive, through `Core`.

use std::sync::Arc;

use mailcraft_engine::{Session, sample};
use serde_json::{Value, json};

use crate::fake_tests::{FakePlatform, block_on, now, sample_core};
use crate::{Core, CoreError};

fn exec(core: &Core, id: &str, params: Value) -> Result<Value, CoreError> {
    block_on(core.execute(id.to_string(), params.to_string())).map(|s| serde_json::from_str(&s).unwrap())
}

/// The sample, and a Microsoft account that keeps its to-dos (nothing is signed in or fetched).
fn core_with_account() -> (Arc<Core>, u64) {
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
        let _ = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
        s
    })
    .unwrap();
    let list = block_on(core.folders(0)).unwrap();
    let id = list.accounts.iter().find(|a| a.email == "ada@example.org").map(|a| a.id).unwrap();
    (core, id)
}

#[test]
fn a_person_comes_with_every_number_and_address_and_an_unknown_one_is_refused() {
    let core = sample_core();
    let first = block_on(core.people(String::new(), 0, 1)).unwrap().people.remove(0);
    let one = block_on(core.person(first.id)).unwrap();
    assert_eq!((one.id, &one.name, one.favorite), (first.id, &first.name, first.favorite));
    assert_eq!(one.emails.iter().map(|e| e.value.clone()).collect::<Vec<_>>(), first.emails);
    assert_eq!(one.phones.iter().map(|e| e.value.clone()).collect::<Vec<_>>(), first.phones);
    assert!(matches!(block_on(core.person(u64::MAX)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.person(0)), Err(CoreError::BadRequest { .. })));
}

#[test]
fn a_person_is_found_by_part_of_an_address_and_a_search_with_junk_is_empty() {
    let core = sample_core();
    let first = block_on(core.people(String::new(), 0, 1)).unwrap().people.remove(0);
    let email = first.emails.first().cloned().unwrap_or_default();
    if !email.is_empty() {
        let part = email.split('@').next().unwrap_or("").to_string();
        assert!(block_on(core.people(part, 0, 50)).unwrap().people.iter().any(|p| p.id == first.id));
    }
    assert_eq!(block_on(core.people("\u{0}\r\n%".into(), 0, 5)).unwrap().total, 0);
    // A window past the end is empty, with the total still told.
    let all = block_on(core.people(String::new(), 0, 1)).unwrap();
    let past = block_on(core.people(String::new(), all.total + 10, 5)).unwrap();
    assert!(past.people.is_empty() && past.total == all.total);
}

#[test]
fn a_task_is_made_edited_completed_and_deleted_and_the_queries_follow() {
    let core = sample_core();
    let made = exec(&core, "tasks.new", json!({"title": "Water the plants", "due": "2026-10-14", "notes": "Both shelves", "account": 0})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let detail = block_on(core.task(id)).unwrap();
    assert_eq!(
        (detail.task.title.as_str(), detail.task.due.as_deref(), detail.notes.as_str()),
        ("Water the plants", Some("2026-10-14"), "Both shelves")
    );
    assert!(detail.only_here && !detail.task.completed);
    assert!(block_on(core.today()).unwrap().tasks.iter().any(|t| t.id == id));

    exec(&core, "tasks.edit", json!({"id": id, "due": "2026-10-20", "reminder": "2026-10-20T09:00"})).unwrap();
    let detail = block_on(core.task(id)).unwrap();
    assert_eq!(detail.task.due.as_deref(), Some("2026-10-20"));
    assert_eq!(detail.reminder.as_deref(), Some("2026-10-20T09:00:00"));
    assert!(!block_on(core.today()).unwrap().tasks.iter().any(|t| t.id == id), "no longer due");
    exec(&core, "tasks.edit", json!({"id": id, "due": null})).unwrap();
    assert_eq!(block_on(core.task(id)).unwrap().task.due, None);

    exec(&core, "tasks.complete", json!({"id": id, "value": true})).unwrap();
    assert!(block_on(core.task(id)).unwrap().task.completed);
    let open = block_on(core.tasks(0, String::new(), false, 0, 200)).unwrap();
    assert!(!open.tasks.iter().any(|t| t.id == id));
    let all = block_on(core.tasks(0, String::new(), true, 0, 200)).unwrap();
    assert!(all.tasks.iter().any(|t| t.id == id));

    exec(&core, "tasks.delete", json!({"id": id})).unwrap();
    assert!(matches!(block_on(core.task(id)), Err(CoreError::BadRequest { .. })));
}

#[test]
fn nothing_local_is_sent_until_the_owner_says_yes() {
    let (core, account) = core_with_account();
    let setup = block_on(core.task_setup()).unwrap();
    assert_eq!(setup.accounts.iter().map(|a| a.id).collect::<Vec<_>>(), [account]);
    assert!(setup.local_waiting > 0 && !setup.answered && setup.sent_to.is_none());
    let waiting = |core: &Core| {
        let ops = exec(core, "sync.taskChanges", json!({"account": account})).unwrap();
        ops["ops"].as_array().map_or(0, Vec::len)
    };
    // Asked of nobody: no create is waiting for the account.
    assert_eq!(waiting(&core), 0);

    exec(&core, "tasks.sendLocal", json!({"account": null})).unwrap();
    let setup = block_on(core.task_setup()).unwrap();
    assert!(setup.answered && setup.sent_to.is_none() && setup.local_waiting > 0);
    assert_eq!(waiting(&core), 0, "Not now keeps them here");

    exec(&core, "tasks.sendLocal", json!({"account": account})).unwrap();
    let setup = block_on(core.task_setup()).unwrap();
    assert_eq!((setup.sent_to, setup.local_waiting), (Some(account), 0));
    assert!(waiting(&core) > 0, "after yes they are to go at the next sync");
}

#[test]
fn a_task_made_for_an_account_is_listed_under_it_and_one_made_here_is_not() {
    let (core, account) = core_with_account();
    exec(&core, "tasks.new", json!({"title": "Book the room", "account": account})).unwrap();
    exec(&core, "tasks.new", json!({"title": "Phone only", "account": 0})).unwrap();
    let lists = block_on(core.task_lists()).unwrap();
    assert!(lists.lists.iter().any(|l| l.account == 0));
    // A task for an account waits for its list (the server names it at the next sync): it is in
    // the account's group with an empty list.
    let under = block_on(core.tasks(account, String::new(), false, 0, 50)).unwrap();
    assert!(under.tasks.iter().any(|t| t.title == "Book the room"));
    assert!(!under.tasks.iter().any(|t| t.title == "Phone only"));
    // Only a Microsoft account keeps tasks: a made-up one is refused and changes nothing.
    assert!(exec(&core, "tasks.new", json!({"title": "x", "account": 99_999})).is_err());
}

#[test]
fn next_events_are_not_over_not_cancelled_in_order_and_at_most_ten() {
    let core = sample_core();
    let today = block_on(core.today()).unwrap();
    let n = &today.next_events;
    assert!(n.len() <= 10);
    assert!(n.windows(2).all(|w| w[0].start <= w[1].start));
    assert!(n.iter().all(|e| !e.cancelled && e.end.as_str() > "2026-10-14T16:00:00"));
}

#[test]
fn a_morning_of_ended_meetings_leaves_room_for_what_is_to_come() {
    use mailcraft_model::Event;
    use mailcraft_time::{Date, DateTime};
    let at = |h: u8, m: u8| DateTime::new(Date::new(2026, 10, 14).unwrap(), h, m, 0).unwrap();
    let core = Core::start(FakePlatform::new(), move || {
        let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
        // More meetings that are over (the clock says 16:00) than the occurrences once counted.
        for i in 0..(crate::text::MAX_OCCURRENCES + 1) {
            let _ = s.mailbox.add_event(Event { title: format!("Over {i}"), start: at(1, 0), end: at(1, 1), ..Default::default() });
        }
        let _ = s.mailbox.add_event(Event { title: "Still to come".into(), start: at(23, 0), end: at(23, 30), ..Default::default() });
        s
    })
    .unwrap();
    let next = block_on(core.today()).unwrap().next_events;
    assert!(next.iter().any(|e| e.title == "Still to come"));
    assert!(!next.iter().any(|e| e.title.starts_with("Over ")));
}

#[test]
fn long_notes_and_names_are_cut() {
    let core = sample_core();
    let made = exec(&core, "tasks.new", json!({"title": "T", "notes": "n".repeat(50_000), "account": 0})).unwrap();
    let detail = block_on(core.task(made["id"].as_u64().unwrap())).unwrap();
    assert_eq!(detail.notes.chars().count(), 8_000);
    assert!(detail.notes_cut);
    let short = exec(&core, "tasks.new", json!({"title": "T2", "notes": "n".repeat(8_000), "account": 0})).unwrap();
    assert!(!block_on(core.task(short["id"].as_u64().unwrap())).unwrap().notes_cut);
}

/// [`core_with_account`], with a shared mailbox's contacts folder the account may only read, and
/// a person in it.
fn core_with_read_only_folder() -> (Arc<Core>, u64) {
    use mailcraft_model::Contact;
    use mailcraft_model::contact_sync::{ContactFolder, FolderOwner};
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
        let _ = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
        let account = s.mailbox.accounts.iter().find(|a| a.email == "ada@example.org").map(|a| a.id).unwrap();
        let owner = FolderOwner::Mailbox("sales@example.org".into());
        let folder = ContactFolder { account, remote: "sales".into(), name: "Sales".into(), owner, ..Default::default() };
        let folder = s.mailbox.add_contact_folder(folder).unwrap();
        let mut c = Contact { first: "Grace".into(), folder, ..Default::default() };
        c.origin.account = account;
        c.origin.id = "AAA".into();
        s.mailbox.add_contact(c);
        s
    })
    .unwrap();
    let id = block_on(core.people("Grace".into(), 0, 5)).unwrap().people.first().map(|p| p.id).unwrap();
    (core, id)
}

#[test]
fn a_new_person_goes_to_the_accounts_contacts_folder_made_when_first_used_and_waits_to_go_up() {
    let (core, account) = core_with_account();
    // No contacts folder yet: its main one is offered, as the default, without an id.
    let offered = block_on(core.contact_folders()).unwrap();
    assert_eq!(offered.default_index, Some(0));
    let main = &offered.folders[0];
    assert_eq!((main.id, main.account, main.name.as_str(), main.kind.as_str()), (None, account, "Contacts (ada@example.org)", "own"));

    let made = exec(&core, "people.new", json!({"first": "Ada"})).unwrap();
    let person = block_on(core.person(made["id"].as_u64().unwrap())).unwrap();
    assert!(person.folder != 0 && person.not_sent && !person.read_only);
    assert_eq!((person.folder_name.as_str(), person.folder_kind.as_str()), ("Contacts (ada@example.org)", "own"));
    // Now made, it is offered by its id.
    let offered = block_on(core.contact_folders()).unwrap();
    assert_eq!(offered.folders.iter().map(|f| f.id).collect::<Vec<_>>(), vec![Some(person.folder)]);
    assert_eq!(offered.default_index, Some(0));

    // This phone is named by 0, and nothing waits to go anywhere.
    let here = exec(&core, "people.new", json!({"first": "Bea", "folder": 0})).unwrap();
    let here = block_on(core.person(here["id"].as_u64().unwrap())).unwrap();
    assert_eq!((here.folder, here.folder_name.as_str(), here.folder_kind.as_str(), here.not_sent), (0, "", "", false));
}

/// #1917: the card marks what the account's server has no place for; nothing is refused for it,
/// and on this phone nothing is marked.
#[test]
fn what_the_server_has_no_place_for_is_marked_and_kept() {
    let (core, _) = core_with_account();
    let emails: Vec<_> = (1..=4).map(|n| json!({"label": "work", "value": format!("ada{n}@example.org")})).collect();
    let marks = |id: &serde_json::Value| {
        let person = block_on(core.person(id["id"].as_u64().unwrap())).unwrap();
        person.emails.iter().map(|e| e.local_only).collect::<Vec<_>>()
    };
    let theirs = exec(&core, "people.new", json!({"first": "Ada", "emails": emails})).unwrap();
    assert_eq!(marks(&theirs), [false, false, false, true]);
    let here = exec(&core, "people.new", json!({"first": "Bea", "emails": emails, "folder": 0})).unwrap();
    assert_eq!(marks(&here), [false; 4]);
}

#[test]
fn a_read_only_folder_is_named_on_the_card_and_not_offered_for_a_new_person() {
    let (core, grace) = core_with_read_only_folder();
    let person = block_on(core.person(grace)).unwrap();
    assert!(person.read_only && !person.not_sent);
    assert_eq!((person.folder_name.as_str(), person.folder_kind.as_str()), ("Sales (sales@example.org)", "mailbox"));
    let offered = block_on(core.contact_folders()).unwrap();
    assert!(offered.folders.iter().all(|f| f.id != Some(person.folder)));
    assert!(exec(&core, "people.new", json!({"first": "X", "folder": person.folder})).is_err());
}

#[test]
fn with_no_account_that_keeps_contacts_a_new_person_stays_on_this_phone() {
    let core = sample_core();
    let offered = block_on(core.contact_folders()).unwrap();
    assert!(offered.folders.is_empty() && offered.default_index.is_none());
}

/// Two accounts whose writable folders fill the picker, and a third with no folder yet, whose
/// people the list shows: its main folder (not made yet) is the default.
#[test]
fn a_main_folder_not_made_yet_is_offered_without_going_past_the_most_listed() {
    use mailcraft_model::AccountChoice;
    use mailcraft_model::contact_sync::ContactFolder;
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
        for who in ["a", "b", "c"] {
            let email = format!("{who}@example.org");
            let _ = s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
        }
        let id = |s: &Session, email: &str| s.mailbox.accounts.iter().find(|a| a.email == email).map(|a| a.id).unwrap();
        for email in ["a@example.org", "b@example.org"] {
            let account = id(&s, email);
            loop {
                let remote = format!("f{}", s.mailbox.contact_folders.len());
                let folder = ContactFolder { account, remote, name: "F".into(), writable: true, ..Default::default() };
                if s.mailbox.add_contact_folder(folder).is_none() {
                    break;
                }
            }
        }
        let c = id(&s, "c@example.org");
        assert!(s.mailbox.main_contact_folder(c).is_none());
        s.view.people_account = AccountChoice::Account(c);
        s
    })
    .unwrap();
    let offered = block_on(core.contact_folders()).unwrap();
    assert!(offered.folders.len() <= crate::text::MAX_LISTED, "{} listed", offered.folders.len());
    let default = &offered.folders[offered.default_index.unwrap() as usize];
    assert_eq!((default.id, default.name.as_str()), (None, "Contacts (c@example.org)"));
}

#[test]
fn the_default_folder_is_offered_even_past_the_most_listed() {
    use mailcraft_model::contact_sync::ContactFolder;
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
        for who in ["a", "b", "c"] {
            let email = format!("{who}@example.org");
            let _ = s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
        }
        let id = |s: &Session, email: &str| s.mailbox.accounts.iter().find(|a| a.email == email).map(|a| a.id).unwrap();
        // The first two accounts fill the list; the third's folder is the one last used.
        for email in ["a@example.org", "b@example.org"] {
            let account = id(&s, email);
            loop {
                let remote = format!("f{}", s.mailbox.contact_folders.len());
                let folder = ContactFolder { account, remote, name: "F".into(), writable: true, ..Default::default() };
                if s.mailbox.add_contact_folder(folder).is_none() {
                    break;
                }
            }
        }
        let last = s.mailbox.ensure_main_contact_folder(id(&s, "c@example.org")).unwrap();
        s.mailbox.contact_place = Some(last);
        s
    })
    .unwrap();
    let offered = block_on(core.contact_folders()).unwrap();
    assert!(offered.folders.len() <= crate::text::MAX_LISTED);
    let at = offered.default_index.unwrap() as usize;
    assert_eq!(offered.folders[at].name, "Contacts (c@example.org)");
}

/// Move to contacts folder (#1144): somebody there with the same address is found for the picker
/// to ask about; the move itself doesn't ask.
#[test]
fn the_picker_hears_of_somebody_there_already_and_the_move_goes_ahead_anyway() {
    let (core, _) = core_with_account();
    let made = exec(&core, "people.new", json!({"first": "Ada", "email": "ada@example.org"})).unwrap();
    let (ada, folder) = (made["id"].as_u64().unwrap(), made["folder"].as_u64().unwrap());
    let here = exec(&core, "people.new", json!({"first": "Ada Home", "email": " ADA@example.org", "folder": 0})).unwrap()["id"].as_u64().unwrap();
    let there = block_on(core.already_in(vec![here], folder)).unwrap().unwrap();
    assert_eq!((there.id, there.name.as_str()), (ada, "Ada"));
    assert_eq!(block_on(core.already_in(vec![here], 0)).unwrap(), None);
    assert_eq!(block_on(core.already_in(vec![u64::MAX], folder)).unwrap(), None);
    exec(&core, "people.moveToFolder", json!({"ids": [here], "folder": folder})).unwrap();
    assert_eq!(block_on(core.person(here)).unwrap().folder, folder);
}
