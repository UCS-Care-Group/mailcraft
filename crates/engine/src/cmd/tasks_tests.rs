// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each account's tasks apart, and a new task in the place chosen (#632).

use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TaskOpKind, TasksDownload};
use mailcraft_model::{AccountChoice, Category, GREY_CATEGORY, Id, LocalTasks, Rgb, Task};
use serde_json::{Value, json};

use super::tasks::{new_task_place, shown_account};
use super::tasks_remote::{local_task_count, merge_tasks, task_upload};
use crate::agent::{Access, Class, Verdict, check, classify, describe};
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
    for junk in [json!({}), json!({"value": null}), json!({"value": "somebody"}), json!({"value": 12_345}), json!({"value": null})] {
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
    // 0 is this computer here too, as in `tasks.new`.
    run(&mut s, "view.taskAccount", json!({"value": work}));
    run(&mut s, "view.taskAccount", json!({"value": 0}));
    assert_eq!(shown_account(&s), AccountChoice::Here);
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
fn what_waited_for_an_answer_given_before_goes_there_once() {
    let (mut s, work, home) = session();
    let waiting = local_task_count(&s.mailbox);
    assert!(waiting > 0);
    s.mailbox.local_tasks = LocalTasks::SendTo(work);
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let mut back = Session::at(crate::io::read_mailbox(&bytes).unwrap(), NOW, 0);
    assert_eq!(going_to(&back, work).len(), waiting);
    assert_eq!((going_to(&back, home).len(), local_task_count(&back.mailbox)), (0, 0));
    // Once: what comes here afterwards waits for a yes of its own.
    back.mailbox.add_task(Task { title: "From a file".into(), ..Default::default() });
    let bytes = crate::io::write_mailbox(&back.mailbox).unwrap();
    let again = Session::at(crate::io::read_mailbox(&bytes).unwrap(), NOW, 0);
    assert_eq!((going_to(&again, work).len(), local_task_count(&again.mailbox)), (waiting, 1));
}

#[test]
fn a_task_sent_from_here_keeps_its_colours() {
    let list = |name: &str| vec![Category { name: name.into(), color: Rgb(1, 2, 3) }];
    let (mut s, work, home) = session();
    s.mailbox.account_categories.insert(work, list("Clients"));
    let blue = s.mailbox.category_color("Blue category");
    assert_ne!(blue, GREY_CATEGORY);
    assert_eq!(s.mailbox.category_color_of(work, "Blue category"), GREY_CATEGORY);
    run(&mut s, "tasks.new", json!({"title": "Here", "account": 0, "categories": ["Blue category"]}));
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert_eq!(s.mailbox.category_color_of(work, "Blue category"), blue);
    assert_eq!(s.mailbox.category_color_of(work, "Clients"), Rgb(1, 2, 3));
    // An account with no list of its own yet uses the shared one, and gets none from this.
    assert_eq!(s.mailbox.category_list_owner(home), None);

    // What a file from before had waiting comes across the same way.
    let (mut s, work, _) = session();
    s.mailbox.account_categories.insert(work, list("Clients"));
    s.mailbox.local_tasks = LocalTasks::SendTo(work);
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let back = Session::at(crate::io::read_mailbox(&bytes).unwrap(), NOW, 0);
    assert_eq!(back.mailbox.category_color_of(work, "Blue category"), blue);

    // A full list stays as it is: the name is still on the task, off the list.
    let (mut s, work, _) = session();
    const FULL: usize = 1_000;
    let full = (0..FULL).map(|n| Category { name: format!("List {n}"), color: Rgb(1, 2, 3) }).collect();
    s.mailbox.account_categories.insert(work, full);
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert_eq!(s.mailbox.categories_of(work).len(), FULL);
}

#[test]
fn an_answer_given_now_is_not_taken_for_one_from_before() {
    let (mut s, work, _) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    s.mailbox.add_task(Task { title: "From a file".into(), ..Default::default() });
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let back = Session::at(crate::io::read_mailbox(&bytes).unwrap(), NOW, 0);
    assert_eq!(local_task_count(&back.mailbox), 1);
}

#[test]
fn an_earlier_answer_for_an_account_that_keeps_no_tasks_moves_nothing() {
    let (mut s, ..) = session();
    let waiting = local_task_count(&s.mailbox);
    s.mailbox.local_tasks = LocalTasks::SendTo(9_999);
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let back = crate::io::read_mailbox(&bytes).unwrap();
    assert_eq!((local_task_count(&back), back.task_place), (waiting, None));
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
    assert_eq!(s.mailbox.task_place, Some(work));
    // An account with no To Do of its own: where a new task goes, and the list is on `home` now.
    let other = run(&mut s, "tasks.fromMessage", json!({"id": ids[1]}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.task(other).unwrap().origin.account, home);
    // With the list on this computer it stays here, whatever was answered about local tasks.
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    let here = run(&mut s, "tasks.fromMessage", json!({"id": ids[2]}))["id"].as_u64().unwrap();
    assert!(s.mailbox.task(here).unwrap().origin.is_local());
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
    assert!(s.run("tasks.edit", &json!({"id": id, "account": null})).is_err());
    assert_eq!(s.mailbox.task(id).unwrap().origin.account, work);
    // What `tasks.get` answers can come back with a change in it.
    run(&mut s, "tasks.edit", json!({"id": id, "account": work, "notes": "changed"}));
    assert_eq!(s.mailbox.task(id).unwrap().notes, "changed");
}

#[test]
fn an_empty_account_is_refused_not_taken_for_the_default() {
    let (mut s, work, _) = session();
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert!(s.run("tasks.new", &json!({"title": "Where?", "account": null})).is_err());
    assert!(s.run("tasks.list", &json!({"account": null})).is_err());
    assert!(!s.mailbox.tasks.values().any(|t| t.title == "Where?"));
}

#[test]
fn an_account_removed_doesnt_start_the_catch_up_again() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    let sent = going_to(&s, work).len();
    run(&mut s, "tasks.new", json!({"title": "In home", "account": home}));
    s.mailbox.add_task(Task { title: "From a file".into(), ..Default::default() });
    s.mailbox.remove_account(home).unwrap();
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let back = Session::at(crate::io::read_mailbox(&bytes).unwrap(), NOW, 0);
    assert_eq!(going_to(&back, work).len(), sent);
    assert_eq!(local_task_count(&back.mailbox), 2, "they wait for a yes of their own");
}

#[test]
fn the_choice_stays_with_its_mailbox() {
    let (mut s, work, _) = session();
    run(&mut s, "view.taskAccount", json!({"value": work}));
    let mut other = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    assert_eq!(add(&mut other, "someone.else@example.org"), work, "the same id, another account");
    s.replace_mailbox(other.mailbox.clone());
    assert_eq!((shown_account(&s), new_task_place(&s)), (AccountChoice::All, None));
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
fn an_agent_is_asked_before_a_task_goes_to_an_account() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": null}));
    let ask = Access::Owner(Class::Send);
    let new_in = |account: Value| json!({"title": "From an agent", "account": account});
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(home))), ask);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(0))), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), Access::Safe);
    // The window's question, for one that has no title yet, puts nothing anywhere.
    assert_eq!(classify(&s, "tasks.new", &json!({"account": home})), Access::Safe);
    // The list says nothing for the owner: anybody can move it.
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), ask);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(work))), ask);
    // The account the owner sent their local tasks to does: only they answer that.
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(work))), Access::Safe);
    assert_eq!(classify(&s, "tasks.new", &new_in(json!(home))), ask);
    // Add to Tasks on a message of the other account.
    let message = s.mailbox.messages.keys().copied().next().unwrap();
    s.mailbox.message_mut(message).unwrap().account = home;
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"id": message})), ask);
    assert!(describe(&s, "tasks.fromMessage", &json!({"id": message})).contains("home@example.org"));
}

#[test]
fn an_agent_is_asked_however_it_names_the_message() {
    let (mut s, work, home) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    let ask = Access::Owner(Class::Send);
    let mut ids = s.mailbox.messages.keys().copied();
    let (message, other) = (ids.next().unwrap(), ids.next().unwrap());
    s.mailbox.message_mut(message).unwrap().account = home;
    s.mailbox.message_mut(other).unwrap().account = work;
    s.sel.messages.clear();
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"id": message})), ask);
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"ids": [message]})), ask);
    // The message the owner is on says nothing for the one named.
    s.sel.messages = vec![other];
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"ids": [message]})), ask);
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({})), Access::Safe);
    assert!(describe(&s, "tasks.fromMessage", &json!({"ids": [message]})).contains("home@example.org"));
    // Named wrongly, nothing can be said of where it goes: it isn't taken for the selection, and
    // the owner isn't asked about it.
    for wrong in [
        json!({"ids": [message, other]}),
        json!({"ids": []}),
        json!({"id": null}),
        json!({"id": message.to_string()}),
        json!({"id": other, "ids": [message]}),
    ] {
        assert_eq!(classify(&s, "tasks.fromMessage", &wrong), ask, "{wrong}");
        assert!(matches!(check(&s, "tasks.fromMessage", &wrong, &[Class::Send]), Verdict::Refuse(_)), "{wrong}");
        assert!(s.run("tasks.fromMessage", &wrong).is_err(), "{wrong}");
    }
}

#[test]
fn an_agent_is_asked_before_a_redo_puts_a_task_on_an_account() {
    let (mut s, work, home) = session();
    let ask = Access::Owner(Class::Send);
    // The owner's answer, which they took back: only they give it.
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    assert!(s.undo());
    assert_eq!(classify(&s, "edit.redo", &json!({})), ask);
    assert!(describe(&s, "edit.redo", &json!({})).contains("To Do"));
    assert!(s.redo());
    assert_eq!(classify(&s, "edit.undo", &json!({})), ask);
    // A task in another account than that one, which they took back.
    run(&mut s, "tasks.new", json!({"title": "Mine", "account": home}));
    assert!(s.undo());
    assert_eq!(classify(&s, "edit.redo", &json!({})), ask);
    // The account they sent their tasks to is free, as for a new task.
    run(&mut s, "tasks.new", json!({"title": "Also mine", "account": work}));
    assert!(s.undo());
    assert_eq!(classify(&s, "edit.redo", &json!({})), Access::Safe);
}

#[test]
fn an_agent_gets_no_further_by_moving_the_list() {
    let (mut s, _, home) = session();
    let ask = Access::Owner(Class::Send);
    assert_eq!(classify(&s, "view.taskAccount", &json!({"value": home})), Access::Safe);
    run(&mut s, "view.taskAccount", json!({"value": home}));
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), ask);
    // Nor by a task that exists, which moves the list to it.
    let message = s.mailbox.messages.keys().copied().next().unwrap();
    s.mailbox.message_mut(message).unwrap().account = home;
    run(&mut s, "tasks.fromMessage", json!({"id": message}));
    run(&mut s, "view.taskAccount", json!({"value": "here"}));
    assert_eq!(classify(&s, "tasks.fromMessage", &json!({"id": message})), Access::Safe);
    run(&mut s, "tasks.fromMessage", json!({"id": message}));
    assert_eq!(s.view.task_account, AccountChoice::Account(home));
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "From an agent"})), ask);
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

#[test]
fn an_agents_task_says_nothing_of_where_the_owners_go() {
    let (mut s, work, _) = session();
    run(&mut s, "tasks.new", json!({"title": "Mine", "account": work}));
    assert_eq!(new_task_place(&s), Some(work));
    // One the gate lets through without a question, kept on this computer.
    assert_eq!(classify(&s, "tasks.new", &json!({"title": "The agent's", "account": 0})), Access::Safe);
    s.agent = Some(Vec::new());
    run(&mut s, "tasks.new", json!({"title": "The agent's", "account": 0}));
    s.agent = None;
    assert!(titled(&s, "The agent's").kept_here);
    assert_eq!((s.mailbox.task_place, new_task_place(&s)), (Some(work), Some(work)), "the owner's next one goes where theirs did");
}

/// Every `key` in `value`, at any depth, taken out: what a build that doesn't know it writes.
fn without(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.remove(key);
            map.values_mut().for_each(|v| without(v, key));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| without(v, key)),
        _ => {}
    }
}

#[test]
fn a_task_kept_here_stays_here_in_a_build_from_before() {
    let (mut s, work, _) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": work}));
    let id = run(&mut s, "tasks.new", json!({"title": "Kept", "account": 0}))["id"].as_u64().unwrap();
    // Changed and done, as any other.
    run(&mut s, "tasks.edit", json!({"id": id, "notes": "Still here"}));
    run(&mut s, "tasks.complete", json!({"id": id}));
    // A build from before #632 knows nothing of `keptHere`: it reads what it does for a to-do the
    // server refused, which it neither sends nor offers.
    assert!(titled(&s, "Kept").kept_here && titled(&s, "Kept").local_only);
    // Saved again by such a build, without what it doesn't know, it stays here when read back.
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    without(&mut file, "keptHere");
    without(&mut file, "taskPlace");
    let mut back: mailcraft_model::Mailbox = serde_json::from_value(file).unwrap();
    back.repair();
    let mut s = Session::at(back, NOW, 0);
    // Nor once it is changed here again.
    run(&mut s, "tasks.edit", json!({"id": id, "notes": "Changed after"}));
    run(&mut s, "tasks.complete", json!({"id": id, "value": false}));
    let kept = titled(&s, "Kept");
    assert!(kept.origin.account == 0 && kept.kept_here);
    assert!(!going_to(&s, work).contains(&"Kept".to_string()));
    assert_eq!(local_task_count(&s.mailbox), 0, "nor is it offered");
}

#[test]
fn an_account_with_no_tasks_of_its_own_is_refused_not_listed_as_empty() {
    let (mut s, work, _) = session();
    let mail_only = s.mailbox.accounts.iter().map(|a| a.id).find(|a| !super::tasks_remote::keeps_tasks(&s.mailbox, *a)).unwrap();
    assert!(s.run("tasks.list", &json!({"account": mail_only})).is_err());
    assert!(s.run("tasks.list", &json!({"account": 999_999})).is_err());
    assert!(s.run("tasks.list", &json!({"account": work})).is_ok());
}
