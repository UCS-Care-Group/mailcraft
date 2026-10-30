// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each account's tasks apart, and a new task in the place chosen (#632).

use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TaskOpKind, TasksDownload};
use mailcraft_model::{AccountChoice, Id, LocalTasks, Task};
use serde_json::{Value, json};

use super::tasks::{new_task_place, shown_account};
use super::tasks_remote::{local_task_count, merge_tasks, task_upload};
use crate::agent::{Access, Class, classify};
use crate::{Session, TaskFilter};

const NOW: i64 = 1_790_000_000;

fn add(s: &mut Session, email: &str) -> Id {
    s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"].as_u64().unwrap()
}

/// The sample with two Microsoft accounts.
fn session() -> (Session, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let (work, home) = (add(&mut s, "work@example.org"), add(&mut s, "home@example.org"));
    (s, work, home)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn titled<'a>(s: &'a Session, title: &str) -> &'a Task {
    s.mailbox.tasks.values().find(|t| t.title == title).unwrap_or_else(|| panic!("no task {title}"))
}

/// The titles of what `account`'s server is to make.
fn going_to(s: &Session, account: Id) -> Vec<String> {
    let ops = task_upload(&s.mailbox, account).unwrap().ops;
    ops.into_iter().filter(|op| op.kind == TaskOpKind::Create).filter_map(|op| op.patch.title).collect()
}

fn from_server(s: &mut Session, account: Id, email: &str, id: &str, title: &str) {
    let task = RemoteTask { id: id.into(), list: "L1".into(), title: title.into(), ..Default::default() };
    let lists = vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }];
    let download = TasksDownload { account, email: email.into(), complete: true, lists, tasks: vec![task] };
    merge_tasks(&mut s.mailbox, &download).unwrap();
}

#[test]
fn the_list_shows_one_accounts_tasks_or_those_on_this_computer_or_all() {
    let (mut s, work, home) = session();
    from_server(&mut s, work, "work@example.org", "w1", "Work task");
    from_server(&mut s, home, "home@example.org", "h1", "Home task");
    let listed = |s: &Session| -> Vec<String> {
        super::tasks::sorted(s, TaskFilter::All).into_iter().filter_map(|id| s.mailbox.task(id)).map(|t| t.title.clone()).collect()
    };
    let everything = listed(&s).len();
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(listed(&s), ["Work task"]);
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    assert_eq!(listed(&s).len(), everything - 2);
    assert!(!listed(&s).contains(&"Home task".to_string()));
    run(&mut s, "view.taskAccount", json!({"value": "all"}));
    assert_eq!(listed(&s).len(), everything);
    // A command can ask for any of them, whatever the list shows, and says which it answers for.
    let answer = run(&mut s, "tasks.list", json!({"filter": "all", "account": home}));
    assert_eq!((&answer["account"], answer["tasks"].as_array().unwrap().len()), (&json!(home), 1));
    assert_eq!((&answer["tasks"][0]["account"], &answer["tasks"][0]["keptAt"]), (&json!(home), &json!("home@example.org")));
}

#[test]
fn the_choice_refuses_what_it_cant_show_and_lets_go_of_what_it_no_longer_lists() {
    let (mut s, work, _) = session();
    from_server(&mut s, work, "work@example.org", "w1", "Work task");
    for junk in [json!({}), json!({"value": 0}), json!({"value": "somebody"}), json!({"value": 12_345}), json!({"value": null})] {
        assert!(s.run("view.taskAccount", &junk).is_err(), "{junk}");
    }
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    assert!(s.run("view.taskAccount", &json!({"value": imap})).is_err());
    let id = titled(&s, "Work task").id;
    run(&mut s, "tasks.select", json!({"id": id}));
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(s.sel.task, Some(id));
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    assert_eq!(s.sel.task, None);
}

#[test]
fn a_new_task_goes_to_the_account_chosen_and_no_other() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": null}));
    let made = run(&mut s, "tasks.new", json!({"title": "For home", "account": home}));
    assert_eq!((&made["account"], &made["keptAt"]), (&json!(home), &json!("home@example.org")));
    assert_eq!(going_to(&s, home), ["For home"]);
    assert!(going_to(&s, work).is_empty());
    for junk in [json!("home"), json!(-1), json!(1.5), json!(12_345)] {
        assert!(s.run("tasks.new", &json!({"title": "x", "account": junk})).is_err(), "{junk}");
    }
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    assert!(s.run("tasks.new", &json!({"title": "x", "account": imap})).is_err());
}

#[test]
fn a_new_task_goes_where_the_list_is_else_where_the_last_one_went() {
    let (mut s, work, home) = session();
    // Nobody has said anything: it stays here, and is offered like any other.
    assert_eq!(new_task_place(&s), None);
    run(&mut s, "tasks.new", json!({"title": "Unplaced"}));
    assert!(!titled(&s, "Unplaced").kept_here);
    assert_eq!(s.mailbox.task_place, None);
    // The list on an account.
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "One"}))["account"], json!(work));
    // All accounts: where the last one went.
    run(&mut s, "view.taskAccount", json!({"value": "all"}));
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "Two"}))["account"], json!(work));
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "Three", "account": home}))["account"], json!(home));
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "Four"}))["account"], json!(home));
    // And the list on this computer.
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    assert_eq!(new_task_place(&s), Some(0));
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "Five"}))["keptAt"], "this computer");
    assert!(titled(&s, "Five").kept_here);
}

#[test]
fn a_new_task_that_the_list_wouldnt_show_moves_the_list_to_it() {
    let (mut s, work, home) = session();
    run(&mut s, "view.taskAccount", json!({"value": work}));
    let id = run(&mut s, "tasks.new", json!({"title": "For home", "account": home}))["id"].as_u64().unwrap();
    assert_eq!((s.view.task_account, s.sel.task), (AccountChoice::Account(home), Some(id)));
    run(&mut s, "tasks.new", json!({"title": "Here", "account": 0}));
    assert_eq!(s.view.task_account, AccountChoice::Here);
}

#[test]
fn someone_who_sent_their_local_tasks_is_first_offered_that_account() {
    let (mut s, work, _) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert_eq!(new_task_place(&s), Some(work));
}

#[test]
fn a_task_kept_on_this_computer_stays_whatever_was_answered() {
    let (mut s, work, _) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    let sent = going_to(&s, work).len();
    let id = run(&mut s, "tasks.new", json!({"title": "Mine alone", "account": 0}))["id"].as_u64().unwrap();
    assert_eq!(going_to(&s, work).len(), sent);
    assert_eq!(local_task_count(&s.mailbox), 0, "not offered either");
    // Nor by being changed, or by the question being answered again.
    run(&mut s, "tasks.edit", json!({"id": id, "notes": "still mine"}));
    run(&mut s, "tasks.complete", json!({"id": id}));
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert_eq!(going_to(&s, work).len(), sent);
    assert!(s.mailbox.task(id).unwrap().origin.is_local());
}

#[test]
fn the_question_is_about_the_tasks_there_now_not_the_ones_to_come() {
    let (mut s, work, _) = session();
    let local = local_task_count(&s.mailbox);
    assert!(local > 0);
    assert_eq!(run(&mut s, "tasks.sendLocal", json!({"account": work}))["sent"], local);
    assert_eq!(going_to(&s, work).len(), local);
    // Something that comes here later (from a file, say) waits for a yes of its own.
    s.mailbox.add_task(Task { title: "From a file".into(), ..Default::default() });
    assert_eq!(going_to(&s, work).len(), local);
    assert_eq!(local_task_count(&s.mailbox), 1);
}

#[test]
fn add_to_tasks_puts_it_in_the_messages_account() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    let ids: Vec<Id> = s.mailbox.messages.keys().copied().take(3).collect();
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    s.mailbox.message_mut(ids[0]).unwrap().account = home;
    s.mailbox.message_mut(ids[1]).unwrap().account = imap;
    run(&mut s, "view.taskAccount", json!({"value": work}));
    // The list moves to the task, and with it the place new tasks go.
    let made = run(&mut s, "tasks.fromMessage", json!({"id": ids[0]}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.task(made).unwrap().origin.account, home);
    assert_eq!(s.view.task_account, AccountChoice::Account(home));
    // It doesn't count as the place the owner last chose.
    assert_eq!(s.mailbox.task_place, None);
    // An account with no To Do of its own: as with All accounts.
    let other = run(&mut s, "tasks.fromMessage", json!({"id": ids[1]}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.task(other).unwrap().origin.account, work);
    // One to-do per message: picking it again shows it.
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    assert_eq!(run(&mut s, "tasks.fromMessage", json!({"id": ids[0]}))["existing"], true);
    assert_eq!(s.view.task_account, AccountChoice::Account(home));
}

#[test]
fn a_task_stays_in_its_account() {
    let (mut s, work, home) = session();
    let id = run(&mut s, "tasks.new", json!({"title": "Stays", "account": work}))["id"].as_u64().unwrap();
    assert!(s.run("tasks.edit", &json!({"id": id, "account": home})).is_err());
    assert_eq!(s.mailbox.task(id).unwrap().origin.account, work);
}

#[test]
fn an_account_that_goes_takes_its_choice_with_it() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.new", json!({"title": "Stays", "account": home}));
    run(&mut s, "view.taskAccount", json!({"value": home}));
    s.mailbox.remove_account(home).unwrap();
    assert_eq!((shown_account(&s), s.mailbox.task_place, new_task_place(&s)), (AccountChoice::All, None, None));
    // With nothing left to choose from, the list is everything and a new task isn't placed.
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    s.mailbox.task_place = Some(0);
    s.mailbox.remove_account(work).unwrap();
    assert_eq!((shown_account(&s), new_task_place(&s)), (AccountChoice::All, None));
}

#[test]
fn an_agent_is_asked_before_a_task_goes_anywhere_the_owners_wouldnt() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": null}));
    let ask = Access::Owner(Class::Send);
    let new_in = |account: Value| json!({"title": "From an agent", "account": account});
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(home))), ask);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(0))), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), Access::Safe);
    // The window's question, for one that has no title yet, puts nothing anywhere.
    assert_eq!(classify(&s, "tasks.new", &json!({"account": home})), Access::Safe);
    // Where the owner's own goes now, it is no more than theirs.
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(work))), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(home))), ask);
    // Add to Tasks on a message of the other account.
    let message = s.mailbox.messages.keys().copied().next().unwrap();
    s.mailbox.message_mut(message).unwrap().account = home;
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"id": message})), ask);
    assert!(crate::agent::describe(&s, "tasks.fromMessage", &json!({"id": message})).contains("home@example.org"));
}

#[test]
fn where_a_task_was_put_is_kept_with_the_mailbox() {
    let (mut s, ..) = session();
    run(&mut s, "tasks.new", json!({"title": "Kept", "account": 0}));
    let text = serde_json::to_string(&s.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&text).unwrap();
    assert_eq!((back.task_place, back.local_tasks), (Some(0), LocalTasks::Unasked));
    assert!(back.tasks.values().any(|t| t.title == "Kept" && t.kept_here));
}
