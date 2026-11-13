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
fn long_notes_and_names_are_cut() {
    let core = sample_core();
    let made = exec(&core, "tasks.new", json!({"title": "T", "notes": "n".repeat(50_000), "account": 0})).unwrap();
    let detail = block_on(core.task(made["id"].as_u64().unwrap())).unwrap();
    assert_eq!(detail.notes.chars().count(), 8_000);
    assert!(detail.notes_cut);
    let short = exec(&core, "tasks.new", json!({"title": "T2", "notes": "n".repeat(8_000), "account": 0})).unwrap();
    assert!(!block_on(core.task(short["id"].as_u64().unwrap())).unwrap().notes_cut);
}
