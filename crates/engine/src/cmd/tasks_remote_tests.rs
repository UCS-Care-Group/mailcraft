// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! To-dos kept in step with Microsoft To Do (#248): downloads merged, changes worked out, and
//! what the server made of them taken in. The server's side is written out by hand, as
//! `mailcraft-net` brings it.

use mailcraft_model::remote_tasks::{MAX_TASK_LISTS, RemoteTask, RemoteTaskList, TaskOpKind, TaskOutcome, TaskResult, TasksDownload, TasksUploaded};
use mailcraft_model::{Id, Importance, Task};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::tasks_remote::{merge_tasks, task_upload};
use crate::Session;

const NOW: i64 = 1_790_000_000;

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn remote(id: &str, title: &str) -> RemoteTask {
    RemoteTask { id: id.into(), list: "L1".into(), change_key: format!("ck-{id}"), title: title.into(), ..Default::default() }
}

fn download(account: Id, tasks: Vec<RemoteTask>) -> TasksDownload {
    TasksDownload {
        account,
        email: "ada@example.org".into(),
        complete: true,
        lists: vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }],
        tasks,
    }
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn titled(s: &Session, title: &str) -> Task {
    s.mailbox.tasks.values().find(|t| t.title == title).map(|t| (**t).clone()).unwrap_or_else(|| panic!("no task {title}"))
}

#[test]
fn a_download_adds_updates_and_removes() {
    let (mut s, a) = session();
    let before = s.mailbox.tasks.len();
    let mut first = remote("t1", "Call Bo");
    first.due = Date::new(2026, 11, 1);
    first.importance = Importance::High;
    let merged = merge_tasks(&mut s.mailbox, &download(a, vec![first.clone(), remote("t2", "Send report")])).unwrap();
    assert_eq!((merged.added, merged.updated, merged.removed), (2, 0, 0));
    assert_eq!(s.mailbox.tasks.len(), before + 2);
    let t = titled(&s, "Call Bo");
    assert_eq!((t.origin.account, t.origin.id.as_str(), t.list.as_str(), t.importance), (a, "t1", "L1", Importance::High));
    assert_eq!(t.synced, Some(t.fields()));
    // Again: nothing changes.
    let again = merge_tasks(&mut s.mailbox, &download(a, vec![first, remote("t2", "Send report")])).unwrap();
    assert_eq!((again.added, again.updated, again.removed, again.unchanged), (0, 0, 0, 2));
    // t2 is gone from the server.
    let gone = merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    assert_eq!(gone.removed, 1);
    assert!(s.mailbox.tasks.values().all(|t| t.title != "Send report"));
    // Tasks that were made here are left alone.
    assert_eq!(s.mailbox.tasks.len(), before + 1);
    // What the server removed was not sent a deletion.
    assert!(s.mailbox.task_gone.is_empty());
}

#[test]
fn nothing_is_removed_for_a_download_that_is_not_whole() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "One"), remote("t2", "Two")])).unwrap();
    let mut partial = download(a, vec![remote("t1", "One")]);
    partial.complete = false;
    assert_eq!(merge_tasks(&mut s.mailbox, &partial).unwrap().removed, 0);
    let mut list = download(a, vec![remote("t1", "One")]);
    list.lists[0].complete = false;
    assert_eq!(merge_tasks(&mut s.mailbox, &list).unwrap().removed, 0);
    // A list that is gone takes its tasks with it.
    let mut none = download(a, Vec::new());
    none.lists.clear();
    assert_eq!(merge_tasks(&mut s.mailbox, &none).unwrap().removed, 2);
}

#[test]
fn a_field_changed_here_stays_and_the_rest_is_the_servers() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    run(&mut s, "tasks.edit", json!({"id": id, "title": "Call Bo today"}));
    // The server changed the importance and the title meanwhile.
    let mut there = remote("t1", "Call Bo (server)");
    there.importance = Importance::High;
    let merged = merge_tasks(&mut s.mailbox, &download(a, vec![there])).unwrap();
    assert_eq!(merged.updated, 1);
    let t = s.mailbox.task(id).unwrap();
    assert_eq!((t.title.as_str(), t.importance), ("Call Bo today", Importance::High));
    // Only the title is left to tell the server.
    let patch = t.patch().unwrap();
    assert_eq!(patch.title.as_deref(), Some("Call Bo today"));
    assert_eq!((patch.importance, patch.due, patch.notes), (None, None, None));
}

#[test]
fn a_task_changed_here_is_made_again_when_the_server_deleted_it() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo"), remote("t2", "Other")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    run(&mut s, "tasks.edit", json!({"id": id, "notes": "bring tea"}));
    let merged = merge_tasks(&mut s.mailbox, &download(a, Vec::new())).unwrap();
    assert_eq!(merged.removed, 1);
    let t = s.mailbox.task(id).unwrap();
    assert!(t.origin.id.is_empty() && t.synced.is_none());
    let up = task_upload(&s.mailbox, a).unwrap();
    assert_eq!(up.ops.iter().filter(|op| op.kind == TaskOpKind::Create && op.task == id).count(), 1);
}

#[test]
fn a_download_for_another_account_is_refused_and_changes_nothing() {
    let (mut s, a) = session();
    let before = s.mailbox.tasks.len();
    let mut wrong = download(a, vec![remote("t1", "x")]);
    wrong.email = "someone@else.example".into();
    assert!(merge_tasks(&mut s.mailbox, &wrong).is_err());
    assert!(merge_tasks(&mut s.mailbox, &download(9_999, vec![remote("t1", "x")])).is_err());
    assert_eq!(s.mailbox.tasks.len(), before);
}

#[test]
fn hostile_downloads_are_cut_to_size() {
    let (mut s, a) = session();
    let mut bad = remote("t1", &format!("long\u{0}{}", "x".repeat(10_000)));
    bad.categories = (0..500).map(|n| format!("c{n}")).collect();
    bad.notes = "n".repeat(300_000);
    let tasks =
        vec![bad, remote("", "no id"), remote("has space", "bad id"), remote("t1", "again"), RemoteTask { id: "t9".into(), ..Default::default() }];
    let merged = merge_tasks(&mut s.mailbox, &download(a, tasks)).unwrap();
    assert_eq!((merged.added, merged.skipped), (1, 4));
    let t = s.mailbox.tasks.values().find(|t| t.origin.id == "t1").unwrap();
    assert_eq!(t.title.chars().count(), 500);
    assert!(t.categories.len() <= 25 && t.notes.chars().count() <= 100_000);
}

#[test]
fn local_tasks_go_up_to_the_account_the_owner_chose_and_are_claimed_by_its_answer() {
    let (mut s, a) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    let local = s.mailbox.tasks.len();
    let up = task_upload(&s.mailbox, a).unwrap();
    assert_eq!(up.ops.len(), local);
    assert!(up.ops.iter().all(|op| op.kind == TaskOpKind::Create && op.patch.title.is_some()));
    let first = up.ops[0].clone();
    let uploaded = TasksUploaded {
        account: a,
        email: "ada@example.org".into(),
        outcomes: vec![TaskOutcome {
            kind: TaskOpKind::Create,
            task: first.task,
            id: "srv-1".into(),
            list: "L1".into(),
            change_key: "ck".into(),
            result: TaskResult::Done,
            patch: first.patch.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };
    run(&mut s, "sync.applyTaskChanges", json!({"result": uploaded}));
    let t = s.mailbox.task(first.task).unwrap();
    assert_eq!((t.origin.account, t.origin.id.as_str(), t.list.as_str()), (a, "srv-1", "L1"));
    assert!(t.patch().is_none());
    // And the next upload no longer carries it.
    assert_eq!(task_upload(&s.mailbox, a).unwrap().ops.len(), local - 1);
}

#[test]
fn a_deletion_is_told_once_and_a_download_does_not_bring_the_task_back() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    run(&mut s, "tasks.delete", json!({"id": id}));
    assert_eq!(s.mailbox.task_gone.len(), 1);
    // The server still shows it: it stays deleted here.
    let merged = merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    assert_eq!((merged.added, merged.skipped), (0, 1));
    let up = task_upload(&s.mailbox, a).unwrap();
    let deletes: Vec<_> = up.ops.iter().filter(|op| op.kind == TaskOpKind::Delete).collect();
    assert_eq!((deletes.len(), deletes[0].id.as_str(), deletes[0].list.as_str()), (1, "t1", "L1"));
    let done = TasksUploaded {
        account: a,
        outcomes: vec![TaskOutcome { kind: TaskOpKind::Delete, id: "t1".into(), list: "L1".into(), result: TaskResult::Gone, ..Default::default() }],
        ..Default::default()
    };
    run(&mut s, "sync.applyTaskChanges", json!({"result": done}));
    assert!(s.mailbox.task_gone.is_empty());
}

#[test]
fn what_the_server_refuses_is_not_sent_over_and_over() {
    let (mut s, a) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    let up = task_upload(&s.mailbox, a).unwrap();
    let (first, second) = (up.ops[0].clone(), up.ops[1].clone());
    let refused = |op: &mailcraft_model::remote_tasks::TaskOp, result| TaskOutcome {
        kind: op.kind,
        task: op.task,
        result,
        error: "nope".into(),
        ..Default::default()
    };
    let uploaded = TasksUploaded {
        account: a,
        outcomes: vec![refused(&first, TaskResult::Refused), refused(&second, TaskResult::Failed)],
        ..Default::default()
    };
    let said = run(&mut s, "sync.applyTaskChanges", json!({"result": uploaded}));
    assert!(said["messages"][0].as_str().unwrap().contains("stays on this computer"), "{said}");
    let now = task_upload(&s.mailbox, a).unwrap();
    assert!(now.ops.iter().all(|op| op.task != first.task));
    assert!(now.ops.iter().any(|op| op.task == second.task));
    assert!(s.mailbox.task(first.task).unwrap().local_only);
}

#[test]
fn an_edit_after_the_upload_was_made_still_goes_up() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    run(&mut s, "tasks.edit", json!({"id": id, "title": "First"}));
    let op = task_upload(&s.mailbox, a).unwrap().ops.into_iter().find(|op| op.task == id).unwrap();
    // Edited again while the first change was on its way.
    run(&mut s, "tasks.edit", json!({"id": id, "title": "Second"}));
    let done = TasksUploaded {
        account: a,
        outcomes: vec![TaskOutcome { kind: op.kind, task: id, result: TaskResult::Done, patch: op.patch, ..Default::default() }],
        ..Default::default()
    };
    run(&mut s, "sync.applyTaskChanges", json!({"result": done}));
    let patch = s.mailbox.task(id).unwrap().patch().unwrap();
    assert_eq!(patch.title.as_deref(), Some("Second"));
}

#[test]
fn an_account_that_does_not_keep_its_tasks_with_microsoft_is_refused() {
    let (mut s, _) = session();
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    assert!(s.run("sync.taskChanges", &json!({"account": imap})).is_err());
    assert!(s.run("sync.taskChanges", &json!({"account": 12_345})).is_err());
    assert!(s.run("sync.applyTasks", &json!({"download": {"account": imap}})).is_err());
}

#[test]
fn old_mailbox_files_without_the_new_fields_still_load() {
    let old = r#"{"id": 4, "title": "Old task", "notes": "", "importance": "normal", "uid": ""}"#;
    let task: Task = serde_json::from_str(old).unwrap();
    assert!(task.origin.is_local() && task.list.is_empty() && task.synced.is_none() && !task.local_only);
    // And a task that is only local writes nothing of them.
    let written = serde_json::to_string(&task).unwrap();
    assert!(!written.contains("origin") && !written.contains("synced") && !written.contains("list"), "{written}");
}

#[test]
fn local_tasks_are_not_uploaded_before_the_owner_says_yes() {
    let (mut s, a) = session();
    assert!(!s.mailbox.tasks.is_empty());
    assert!(task_upload(&s.mailbox, a).unwrap().ops.is_empty());
    // A task made now stays here too, and says so.
    let made = run(&mut s, "tasks.new", json!({"title": "Local one"}));
    assert_eq!(made["keptAt"], "this computer");
    assert!(task_upload(&s.mailbox, a).unwrap().ops.is_empty());
    // The server's own to-dos come in, and still nothing goes.
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    assert!(task_upload(&s.mailbox, a).unwrap().ops.is_empty());
}

#[test]
fn yes_sends_them_to_the_chosen_account_and_new_tasks_follow() {
    let (mut s, a) = session();
    let local = s.mailbox.tasks.len();
    let said = run(&mut s, "tasks.sendLocal", json!({"account": a}));
    assert_eq!(said["sent"], local);
    let up = task_upload(&s.mailbox, a).unwrap();
    assert_eq!(up.ops.len(), local);
    assert!(up.ops.iter().all(|op| op.kind == TaskOpKind::Create));
    let made = run(&mut s, "tasks.new", json!({"title": "After the yes"}));
    assert_eq!(made["keptAt"], "ada@example.org");
    assert_eq!(task_upload(&s.mailbox, a).unwrap().ops.len(), local + 1);
}

#[test]
fn not_now_keeps_them_here_on_every_later_sync() {
    let (mut s, a) = session();
    let said = run(&mut s, "tasks.sendLocal", json!({"account": null}));
    assert_eq!(said["sent"], 0);
    for _ in 0..3 {
        merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
        assert!(task_upload(&s.mailbox, a).unwrap().ops.is_empty());
    }
    assert_eq!(run(&mut s, "tasks.new", json!({"title": "Still here"}))["keptAt"], "this computer");
    // Changed their mind.
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    assert!(!task_upload(&s.mailbox, a).unwrap().ops.is_empty());
}

#[test]
fn only_a_microsoft_account_can_be_chosen_and_removing_it_asks_again() {
    let (mut s, a) = session();
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}))["id"].as_u64().unwrap();
    assert!(s.run("tasks.sendLocal", &json!({"account": imap})).is_err());
    assert!(s.run("tasks.sendLocal", &json!({"account": 12_345})).is_err());
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    s.mailbox.remove_account(a).unwrap();
    assert_eq!(s.mailbox.local_tasks, mailcraft_model::LocalTasks::Unasked);
}

#[test]
fn an_agent_cannot_answer_for_the_owner() {
    use crate::agent::{Access, Class, access};
    assert_eq!(access("tasks.sendLocal"), Some(Access::Owner(Class::Send)));
}

#[test]
fn old_mailbox_files_load_with_the_question_not_yet_asked() {
    let (s, _) = session();
    let mut value = serde_json::to_value(&s.mailbox).unwrap();
    assert!(value.get("localTasks").is_none());
    let back: mailcraft_model::Mailbox = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(back.local_tasks, mailcraft_model::LocalTasks::Unasked);
    value["localTasks"] = json!({"sendTo": 7});
    let back: mailcraft_model::Mailbox = serde_json::from_value(value).unwrap();
    assert_eq!(back.local_tasks, mailcraft_model::LocalTasks::SendTo(7));
}

fn outcome(op: &mailcraft_model::remote_tasks::TaskOp, result: TaskResult) -> TaskOutcome {
    TaskOutcome {
        kind: op.kind,
        task: op.task,
        id: op.id.clone(),
        list: op.list.clone(),
        patch: op.patch.clone(),
        result,
        error: "no".into(),
        ..Default::default()
    }
}

fn told(account: Id, outcomes: Vec<TaskOutcome>) -> Value {
    json!({"result": TasksUploaded { account, outcomes, ..Default::default() }})
}

#[test]
fn a_refused_change_is_kept_not_reverted_and_tried_again_after_an_edit() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    run(&mut s, "tasks.edit", json!({"id": id, "title": "Call Bo today", "due": "2026-11-02"}));
    let op = task_upload(&s.mailbox, a).unwrap().ops.into_iter().find(|op| op.task == id).unwrap();
    run(&mut s, "sync.applyTaskChanges", told(a, vec![outcome(&op, TaskResult::Refused)]));
    // The valid part of the edit is not lost to the next download, and nothing is sent over and over.
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo")])).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().title, "Call Bo today");
    assert!(task_upload(&s.mailbox, a).unwrap().ops.iter().all(|op| op.task != id));
    run(&mut s, "tasks.edit", json!({"id": id, "due": "2026-11-03"}));
    assert!(task_upload(&s.mailbox, a).unwrap().ops.iter().any(|op| op.task == id));
}

#[test]
fn a_refused_new_task_is_tried_again_once_it_is_changed() {
    let (mut s, a) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    let id = run(&mut s, "tasks.new", json!({"title": "Odd one"}))["id"].as_u64().unwrap();
    let op = task_upload(&s.mailbox, a).unwrap().ops.into_iter().find(|op| op.task == id).unwrap();
    run(&mut s, "sync.applyTaskChanges", told(a, vec![outcome(&op, TaskResult::Refused)]));
    assert!(task_upload(&s.mailbox, a).unwrap().ops.iter().all(|op| op.task != id));
    run(&mut s, "tasks.edit", json!({"id": id, "start": null}));
    assert!(task_upload(&s.mailbox, a).unwrap().ops.iter().any(|op| op.task == id));
}

#[test]
fn a_task_the_server_made_without_the_answer_arriving_is_taken_in_not_made_twice() {
    let (mut s, a) = session();
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    let id = run(&mut s, "tasks.new", json!({"title": "Lost answer", "due": "2026-11-05"}))["id"].as_u64().unwrap();
    let mut there = remote("srv-9", "Lost answer");
    there.due = Date::new(2026, 11, 5);
    let before = s.mailbox.tasks.len();
    merge_tasks(&mut s.mailbox, &download(a, vec![there])).unwrap();
    assert_eq!(s.mailbox.tasks.len(), before, "no second copy");
    let t = s.mailbox.task(id).unwrap();
    assert_eq!((t.origin.id.as_str(), t.list.as_str()), ("srv-9", "L1"));
    assert!(task_upload(&s.mailbox, a).unwrap().ops.iter().all(|op| op.task != id));
}

#[test]
fn undo_cannot_bring_back_what_the_server_was_told_was_gone_or_made() {
    let (mut s, a) = session();
    // (a) a delete that went up
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "One")])).unwrap();
    let id = titled(&s, "One").id;
    run(&mut s, "tasks.delete", json!({"id": id}));
    let gone = TaskOutcome { kind: TaskOpKind::Delete, id: "t1".into(), list: "L1".into(), result: TaskResult::Done, ..Default::default() };
    run(&mut s, "sync.applyTaskChanges", told(a, vec![gone]));
    assert!(!s.undo(), "nothing to undo after the server was told");
    merge_tasks(&mut s.mailbox, &download(a, vec![])).unwrap();
    assert!(s.mailbox.tasks.values().all(|t| t.title != "One"));
    // (b) a create that went up
    run(&mut s, "tasks.sendLocal", json!({"account": a}));
    let made = run(&mut s, "tasks.new", json!({"title": "Fresh"}))["id"].as_u64().unwrap();
    let op = task_upload(&s.mailbox, a).unwrap().ops.into_iter().find(|op| op.task == made).unwrap();
    let mut done = outcome(&op, TaskResult::Done);
    done.id = "t9".into();
    done.list = "L1".into();
    run(&mut s, "sync.applyTaskChanges", told(a, vec![done]));
    assert!(!s.undo());
    assert!(s.mailbox.tasks.values().any(|t| t.title == "Fresh"));
}

#[test]
fn removing_an_account_leaves_its_tasks_as_tasks_of_this_computer() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Kept")])).unwrap();
    s.mailbox.remove_account(a).unwrap();
    let t = titled(&s, "Kept");
    assert!(t.origin.is_local() && t.list.is_empty() && t.synced.is_none());
}

#[test]
fn removing_the_account_takes_its_to_dos_but_keeps_what_was_changed_here_and_not_sent() {
    let (mut s, a) = session();
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t1", "Call Bo"), remote("t2", "From the server")])).unwrap();
    let id = titled(&s, "Call Bo").id;
    // Changed here, then deleted on the server: it waits to be made again, with no id yet.
    run(&mut s, "tasks.edit", json!({"id": id, "notes": "bring tea"}));
    merge_tasks(&mut s.mailbox, &download(a, vec![remote("t2", "From the server")])).unwrap();
    assert!(s.mailbox.task(id).is_some_and(|t| t.origin.account == a && t.origin.id.is_empty()));
    run(&mut s, "account.remove", json!({"id": a}));
    assert!(!s.mailbox.tasks.values().any(|t| t.title == "From the server"), "the server's to-do goes with the account");
    let t = s.mailbox.task(id).unwrap_or_else(|| panic!("the owner's change is kept"));
    assert!(t.origin.is_local() && t.notes.contains("bring tea"), "kept as this computer's own: {t:?}");
}

/// Ada's account with one list of her own (L1) and one shared with other people (L2), a to-do in
/// each: "Mine" and "Ours".
fn shared_lists() -> (Session, Id, Id, Id) {
    let (mut s, a) = session();
    let mut ours = remote("t2", "Ours");
    ours.list = "L2".into();
    let mut d = download(a, vec![remote("t1", "Mine"), ours]);
    d.lists.push(RemoteTaskList { id: "L2".into(), complete: true, shared: true });
    merge_tasks(&mut s.mailbox, &d).unwrap();
    let (mine, ours) = (titled(&s, "Mine").id, titled(&s, "Ours").id);
    (s, a, mine, ours)
}

#[test]
fn a_shared_list_is_remembered_until_the_server_says_otherwise() {
    let (mut s, a, ..) = shared_lists();
    assert_eq!(s.mailbox.shared_task_lists, [(a, ["L2".to_string()].into())].into());
    assert!(s.mailbox.in_shared_task_list(&titled(&s, "Ours")) && !s.mailbox.in_shared_task_list(&titled(&s, "Mine")));
    // Saved with the mailbox, and a mailbox of before (none named) loads with none.
    let saved = serde_json::to_value(&s.mailbox).unwrap();
    assert_eq!(saved["sharedTaskLists"], json!([{"account": a, "id": "L2"}]));
    let mut old = saved.clone();
    old.as_object_mut().unwrap().remove("sharedTaskLists");
    assert!(serde_json::from_value::<mailcraft_model::Mailbox>(old).unwrap().shared_task_lists.is_empty());
    // Junk in its place doesn't stop the mailbox loading.
    let mut junk = saved;
    junk["sharedTaskLists"] = json!([7, "x", {"account": "no"}]);
    assert!(serde_json::from_value::<mailcraft_model::Mailbox>(junk).is_ok());
    // A download that doesn't name every list leaves L2 as it was.
    let mut partial = download(a, vec![remote("t1", "Mine")]);
    partial.complete = false;
    merge_tasks(&mut s.mailbox, &partial).unwrap();
    assert!(s.mailbox.in_shared_task_list(&titled(&s, "Ours")));
    // One that names L2 as no longer shared forgets it.
    let mut ours = remote("t2", "Ours");
    ours.list = "L2".into();
    let mut d = download(a, vec![remote("t1", "Mine"), ours]);
    d.lists.push(RemoteTaskList { id: "L2".into(), complete: true, shared: false });
    merge_tasks(&mut s.mailbox, &d).unwrap();
    assert!(s.mailbox.shared_task_lists.is_empty());
    // Removing the account forgets its lists.
    let (mut s, a, ..) = shared_lists();
    run(&mut s, "account.remove", json!({"id": a}));
    assert!(s.mailbox.shared_task_lists.is_empty());
}

#[test]
fn an_agents_change_to_a_task_in_a_shared_list_asks_for_send() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, _, mine, ours) = shared_lists();
    for (id, params) in [
        ("tasks.edit", json!({"title": "Renamed"})),
        ("tasks.complete", json!({})),
        ("tasks.categorize", json!({"category": "Red category"})),
        ("tasks.delete", json!({})),
    ] {
        let with = |task: Id| {
            let mut p = params.clone();
            p["id"] = json!(task);
            p
        };
        // In the shared list: the owner is asked, and the question says who sees it.
        match check(&s, id, &with(ours), &[]) {
            Verdict::Ask(Class::Send, what) => assert!(what.contains("shared with other people") && what.contains("Ours"), "{id}: {what}"),
            other => panic!("{id} in a shared list: {other:?}"),
        }
        assert_eq!(check(&s, id, &with(ours), &[Class::Send]), Verdict::Run, "{id}");
        // In a list of the owner's own: as before.
        let mine_now = check(&s, id, &with(mine), &[]);
        if id == "tasks.delete" {
            assert!(matches!(mine_now, Verdict::Ask(Class::Delete, _)), "{id}: {mine_now:?}");
        } else {
            assert_eq!(mine_now, Verdict::Run, "{id} in a list that isn't shared");
        }
    }
    // No id: the selected to-do, as the command takes it; a junk id means the same.
    run(&mut s, "tasks.select", json!({"id": ours}));
    for junk in [json!({}), json!({"id": "seven"}), json!({"id": -1}), json!({"id": null}), json!({"id": 1.5}), json!([1]), json!(null)] {
        assert!(matches!(check(&s, "tasks.edit", &junk, &[]), Verdict::Ask(Class::Send, _)), "{junk}");
    }
    // A to-do that isn't there: the command changes nothing, and nothing is asked.
    assert_eq!(check(&s, "tasks.edit", &json!({"id": u64::MAX, "title": "x"}), &[]), Verdict::Run);
    run(&mut s, "tasks.select", json!({"id": null}));
    assert_eq!(check(&s, "tasks.complete", &json!({}), &[]), Verdict::Run);
    // The owner's own change runs as ever.
    run(&mut s, "tasks.edit", json!({"id": ours, "title": "Renamed by the owner"}));
    assert_eq!(titled(&s, "Renamed by the owner").id, ours);
    // An undo that would change it back asks too.
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(Class::Send, what) => assert!(what.contains("shared list"), "{what}"),
        other => panic!("undo of a shared to-do: {other:?}"),
    }
    run(&mut s, "tasks.edit", json!({"id": mine, "title": "Mine, renamed"}));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
}

/// [`shared_lists`] with "Red category" on both to-dos, or on "Mine" alone.
fn categorised(on_ours: bool) -> Session {
    let (mut s, _, mine, ours) = shared_lists();
    if s.mailbox.category("Red category").is_none() {
        run(&mut s, "category.set", json!({"name": "Red category", "color": "#cc3333"}));
    }
    s.mailbox.task_mut(mine).unwrap().categories = vec!["Red category".into()];
    if on_ours {
        s.mailbox.task_mut(ours).unwrap().categories = vec!["Red category".into()];
    }
    s
}

#[test]
fn renaming_or_deleting_a_category_a_shared_to_do_has_asks_for_send() {
    use crate::agent::{Class, Verdict, check};
    let rename = json!({"from": "Red category", "to": "Agent was here"});
    let remove = json!({"name": " red CATEGORY "});
    let s = categorised(true);
    match check(&s, "category.rename", &rename, &[]) {
        Verdict::Ask(Class::Send, what) => assert!(what.contains("shared with other people"), "{what}"),
        other => panic!("rename: {other:?}"),
    }
    match check(&s, "category.remove", &remove, &[Class::Delete]) {
        Verdict::Ask(Class::Send, what) => assert!(what.contains("shared with other people"), "{what}"),
        other => panic!("remove: {other:?}"),
    }
    assert_eq!(check(&s, "category.rename", &rename, &[Class::Send]), Verdict::Run);
    // Only on a to-do in a list that isn't shared: as before.
    let s = categorised(false);
    assert_eq!(check(&s, "category.rename", &rename, &[]), Verdict::Run);
    assert!(matches!(check(&s, "category.remove", &remove, &[]), Verdict::Ask(Class::Delete, _)));
    // Junk params change nothing and don't panic.
    for junk in [json!({}), json!({"from": 7}), json!(null), json!([1]), json!({"name": null})] {
        let _ = check(&s, "category.rename", &junk, &[]);
        let _ = check(&s, "category.remove", &junk, &[]);
    }
}

#[test]
fn an_agents_command_that_changes_a_shared_to_do_by_another_way_is_undone() {
    use crate::agent::{Class, overstep};
    let s = categorised(true);
    let before = s.mailbox.clone();
    let mut after = before.clone();
    after.rename_category("Red category", "Agent was here").unwrap();
    assert_eq!(overstep(&before, &after, &[]), Some(Class::Send));
    assert_eq!(overstep(&before, &after, &[Class::Send]), None);
    // A deletion from a shared list, which the server is to be told of.
    let mut gone = before.clone();
    let ours = titled(&s, "Ours").id;
    gone.remove_task(ours).unwrap();
    assert_eq!(overstep(&before, &gone, &[Class::Delete]), Some(Class::Send));
    // The same to a list that isn't shared, and nothing at all: fine.
    let s = categorised(false);
    let mut mine = s.mailbox.clone();
    mine.rename_category("Red category", "Agent was here").unwrap();
    assert_eq!(overstep(&s.mailbox, &mine, &[]), None);
    assert_eq!(overstep(&s.mailbox, &s.mailbox, &[]), None);
    // Run as an agent, the rename is refused and nothing changes.
    let mut s = categorised(true);
    let was = s.mailbox.clone();
    s.agent = Some(Vec::new());
    assert!(s.run("category.rename", &json!({"from": "Red category", "to": "Agent was here"})).is_err());
    s.agent = None;
    assert_eq!(s.mailbox, was);
}

/// A to-do of `account`'s in its list `list`.
fn in_list(account: Id, list: &str) -> Task {
    Task { origin: mailcraft_model::Origin { account, id: "x".into(), ..Default::default() }, list: list.into(), ..Default::default() }
}

#[test]
fn every_accounts_shared_lists_are_kept_however_many_accounts_have_them() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, a, _, ours) = shared_lists();
    // Many other accounts, each with as many shared lists as a download names.
    for other in 1_000..1_020 {
        s.mailbox.shared_task_lists.insert(other, (0..MAX_TASK_LISTS).map(|n| format!("S{n}")).collect());
    }
    // Ada's next download still has L2 shared, and an agent's edit there is still asked about.
    let mut ours_task = remote("t2", "Ours");
    ours_task.list = "L2".into();
    let mut d = download(a, vec![remote("t1", "Mine"), ours_task]);
    d.lists.push(RemoteTaskList { id: "L2".into(), complete: true, shared: true });
    merge_tasks(&mut s.mailbox, &d).unwrap();
    assert!(matches!(check(&s, "tasks.edit", &json!({"id": ours, "title": "x"}), &[]), Verdict::Ask(Class::Send, _)));
    for other in 1_000..1_020 {
        assert!(s.mailbox.in_shared_task_list(&in_list(other, &format!("S{}", MAX_TASK_LISTS - 1))), "{other}");
    }
    // Saved and read back: all of them.
    let back: mailcraft_model::Mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    assert_eq!(back.shared_task_lists, s.mailbox.shared_task_lists);
}

#[test]
fn a_mailbox_file_with_too_many_shared_lists_for_one_account_keeps_the_others() {
    let too_many = mailcraft_model::remote_tasks::MAX_TASKS + 500;
    let mut lists: Vec<Value> = (0..too_many * 2).map(|i| json!({"account": 1, "id": format!("L{}", i % too_many)})).collect();
    lists.extend([json!({"account": 1, "id": ""}), json!({"account": 2, "id": "Bell\u{7}"}), json!({"account": 2, "id": "Kept"})]);
    let mb: mailcraft_model::Mailbox = serde_json::from_value(json!({"sharedTaskLists": lists})).unwrap();
    let one = mb.shared_task_lists.get(&1).unwrap();
    assert_eq!(one.len(), mailcraft_model::remote_tasks::MAX_TASKS);
    // What was kept of that account's is gated; the other account's are all there.
    let first = one.iter().next().unwrap().clone();
    assert!(mb.in_shared_task_list(&in_list(1, &first)));
    assert!(mb.in_shared_task_list(&in_list(2, "Kept")));
    // Ids are cut as a download's are.
    assert!(mb.shared_task_lists.values().flatten().all(|id| !id.is_empty() && !id.chars().any(char::is_control)));
}

#[test]
fn deleting_the_message_a_shared_to_do_was_made_from_is_not_a_send() {
    let (mut s, _, _, ours) = shared_lists();
    let m = *s.mailbox.messages.keys().next().unwrap();
    s.mailbox.task_mut(ours).unwrap().message = Some(m);
    s.agent = Some(vec![crate::agent::Class::Delete]);
    let deleted = s.run("mail.deletePermanently", &json!({"ids": [m]}));
    s.agent = None;
    assert!(deleted.is_ok(), "{deleted:?}");
    assert!(s.mailbox.message(m).is_none());
    assert_eq!(s.mailbox.task(ours).unwrap().message, None);
}

#[test]
fn a_shared_list_with_a_to_do_in_it_is_not_forgotten_for_lack_of_room() {
    use crate::agent::{Class, Verdict, check};
    let (mut s, a) = session();
    // Two downloads that can't name every list, each naming a different 50 shared ones.
    let half = |from: usize, task: Option<RemoteTask>| {
        let lists = (from..from + MAX_TASK_LISTS).map(|n| RemoteTaskList { id: format!("A{n:02}"), complete: true, shared: true }).collect();
        TasksDownload { account: a, email: "ada@example.org".into(), complete: false, lists, tasks: task.into_iter().collect() }
    };
    let mut first = remote("t0", "In A00");
    first.list = "A00".into();
    merge_tasks(&mut s.mailbox, &half(0, Some(first))).unwrap();
    merge_tasks(&mut s.mailbox, &half(MAX_TASK_LISTS, None)).unwrap();
    let t = titled(&s, "In A00");
    assert!(s.mailbox.in_shared_task_list(&t), "the list a to-do is in is kept");
    assert!(matches!(check(&s, "tasks.edit", &json!({"id": t.id, "title": "x"}), &[]), Verdict::Ask(Class::Send, _)));
    // Lists with no to-do in them are kept only while there is room.
    assert_eq!(s.mailbox.shared_task_lists.get(&a).map(|l| l.len()), Some(MAX_TASK_LISTS + 1));
}

#[test]
fn a_to_do_taken_out_of_a_shared_list_by_another_way_is_an_overstep() {
    use crate::agent::{Class, overstep};
    let (s, _, _, ours) = shared_lists();
    let mut after = s.mailbox.clone();
    after.task_mut(ours).unwrap().list = "L1".into();
    assert_eq!(overstep(&s.mailbox, &after, &[]), Some(Class::Send));
}
