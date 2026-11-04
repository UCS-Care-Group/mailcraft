// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft To Do through Graph (#248), against a small stand-in on loopback that keeps lists of
//! tasks and answers as the documented service does. Nothing here leaves this machine.

use std::sync::{Arc, Mutex};

use mailcraft_model::remote_tasks::{TaskOp, TaskOpKind, TaskResult, TasksUpload};
use mailcraft_model::{Importance, TaskPatch};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::tasks::{push_tasks_at, task_body, tasks_at};
use crate::NetError;
use crate::fake::{FakeHttp, json_answer};

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";

#[derive(Default)]
struct Todo {
    /// Lists: id, well-known name, tasks.
    lists: Vec<(String, String, Vec<Value>)>,
    /// Answer every write with this status instead of doing it.
    write_status: Option<u16>,
    /// Answer every request with this status.
    all_status: Option<u16>,
    /// Make a new checklist item, then answer with this status as if the answer were lost.
    made_then_status: Option<u16>,
    /// The requests for a checklist of its own (`…/checklistItems`), and for pages of tasks.
    checklist_reads: usize,
    task_pages: usize,
    next: u32,
}

fn raw(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

/// A server of lists and tasks. `Arc<Mutex<Todo>>` is what a test sets up and looks at.
fn serve(todo: &Arc<Mutex<Todo>>) -> FakeHttp {
    let todo = todo.clone();
    FakeHttp::start(move |_, request| {
        let mut todo = todo.lock().unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        let line = head.lines().next().unwrap_or("");
        let mut parts = line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if let Some(status) = todo.all_status {
            return raw(&format!("{status} Other"));
        }
        let path = target.split('?').next().unwrap_or("");
        let pieces: Vec<&str> = path.split('/').collect();
        // ["", "v1.0", "me", "todo", "lists", {list}, "tasks", {task}]
        if pieces.get(1..5) != Some(&["v1.0", "me", "todo", "lists"]) {
            return raw("404 Not Found");
        }
        let json = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
        if pieces.get(8) == Some(&"checklistItems") {
            if method == "GET" {
                todo.checklist_reads += 1;
            }
            if method != "GET"
                && let Some(status) = todo.write_status
            {
                return raw(&format!("{status} Other"));
            }
            let Some(task) = todo
                .lists
                .iter_mut()
                .find(|list| Some(&list.0.as_str()) == pieces.get(5))
                .and_then(|list| list.2.iter_mut().find(|task| Some(&task["id"].as_str().unwrap_or("")) == pieces.get(7)))
            else {
                return raw("404 Not Found");
            };
            if task.get("checklistItems").is_none() {
                task["checklistItems"] = json!([]);
            }
            let steps = task["checklistItems"].as_array_mut().unwrap();
            return match (method, pieces.get(9)) {
                ("GET", None) => json_answer("200 OK", &json!({"value": steps}).to_string()),
                ("POST", None) => {
                    let mut step = json;
                    step["id"] = json!(format!("S{}", steps.len() + 1));
                    steps.push(step.clone());
                    match todo.made_then_status {
                        Some(status) => raw(&format!("{status} Other")),
                        None => json_answer("201 Created", &step.to_string()),
                    }
                }
                ("PATCH", Some(id)) => match steps.iter_mut().find(|step| step["id"] == **id) {
                    Some(step) => {
                        for (key, value) in json.as_object().unwrap() {
                            step[key] = value.clone();
                        }
                        json_answer("200 OK", &step.to_string())
                    }
                    None => raw("404 Not Found"),
                },
                ("DELETE", Some(id)) => {
                    steps.retain(|step| step["id"] != **id);
                    raw("204 No Content")
                }
                _ => raw("405 Method Not Allowed"),
            };
        }
        match (method, pieces.get(5), pieces.get(6), pieces.get(7)) {
            ("GET", None, ..) => {
                let value: Vec<Value> = todo
                    .lists
                    .iter()
                    .map(|(id, well, _)| json!({"id": id, "wellknownListName": well, "displayName": "x", "isShared": *id == "L2"}))
                    .collect();
                json_answer("200 OK", &json!({"value": value}).to_string())
            }
            ("GET", Some(list), Some(&"tasks"), None) => {
                todo.task_pages += 1;
                // The checklists come with the tasks only when they are asked for.
                let expand = target.contains("expand=checklistItems");
                match todo.lists.iter().find(|l| l.0 == *list) {
                    Some(l) => {
                        let tasks: Vec<Value> =
                            l.2.iter()
                                .map(|task| {
                                    let mut task = task.clone();
                                    if let Some(fields) = task.as_object_mut() {
                                        if expand {
                                            fields.entry("checklistItems").or_insert(json!([]));
                                        } else {
                                            fields.remove("checklistItems");
                                        }
                                    }
                                    task
                                })
                                .collect();
                        json_answer("200 OK", &json!({"value": tasks}).to_string())
                    }
                    None => raw("404 Not Found"),
                }
            }
            (write, Some(list), Some(&"tasks"), id) => {
                if let Some(status) = todo.write_status {
                    return raw(&format!("{status} Other"));
                }
                todo.next += 1;
                let made = format!("T{}", todo.next);
                let Some(l) = todo.lists.iter_mut().find(|l| l.0 == *list) else { return raw("404 Not Found") };
                match (write, id) {
                    ("POST", None) => {
                        let mut task = json;
                        task["id"] = json!(made);
                        task["@odata.etag"] = json!("W/\"e1\"");
                        l.2.push(task.clone());
                        json_answer("201 Created", &task.to_string())
                    }
                    ("PATCH", Some(id)) => match l.2.iter_mut().find(|t| t["id"] == *id) {
                        Some(task) => {
                            for (k, v) in json.as_object().cloned().unwrap_or_default() {
                                task[k] = v;
                            }
                            json_answer("200 OK", &task.to_string())
                        }
                        None => raw("404 Not Found"),
                    },
                    ("DELETE", Some(id)) => {
                        let before = l.2.len();
                        l.2.retain(|t| t["id"] != *id);
                        raw(if l.2.len() < before { "204 No Content" } else { "404 Not Found" })
                    }
                    _ => raw("405 Method Not Allowed"),
                }
            }
            _ => raw("404 Not Found"),
        }
    })
}

fn todo() -> Arc<Mutex<Todo>> {
    let todo = Todo {
        lists: vec![
            ("L1".into(), "defaultList".into(), Vec::new()),
            ("L2".into(), "none".into(), Vec::new()),
            ("LF".into(), "flaggedEmails".into(), vec![json!({"id": "F1", "title": "A flagged message"})]),
        ],
        ..Default::default()
    };
    Arc::new(Mutex::new(todo))
}

fn upload(ops: Vec<TaskOp>) -> TasksUpload {
    TasksUpload { account: 1, email: "ada@example.org".into(), ops }
}

fn create(title: &str) -> TaskOp {
    let patch = TaskPatch { title: Some(title.into()), ..Default::default() };
    TaskOp { kind: TaskOpKind::Create, task: 7, patch, ..Default::default() }
}

#[test]
fn lists_and_tasks_are_read_with_their_fields() {
    let state = todo();
    state.lock().unwrap().lists[0].2 = vec![
        json!({"id": "A", "title": "Call Bo", "status": "notStarted", "importance": "high", "categories": ["Red category", "Red category"],
               "body": {"content": "<p>Bring the <b>tea</b> &amp; cake</p>", "contentType": "html"},
               "dueDateTime": {"dateTime": "2026-10-30T00:00:00.0000000", "timeZone": "UTC"},
               "startDateTime": {"dateTime": "2026-10-28T00:00:00.0000000", "timeZone": "UTC"},
               "isReminderOn": true, "reminderDateTime": {"dateTime": "2026-10-30T09:30:00.0000000", "timeZone": "UTC"},
               "@odata.etag": "W/\"x\""}),
        json!({"id": "B", "title": "Done thing", "status": "completed", "importance": "low",
               "completedDateTime": {"dateTime": "2026-10-20T10:00:00.0000000", "timeZone": "UTC"}}),
    ];
    state.lock().unwrap().lists[1].2 = vec![json!({"id": "C", "title": "In the other list"})];
    let server = serve(&state);
    let got = tasks_at(&server.url(""), TOKEN).unwrap();
    assert!(got.complete);
    // The flagged-mail list is not read, and not named.
    assert_eq!(got.lists.iter().map(|l| (l.id.as_str(), l.complete)).collect::<Vec<_>>(), [("L1", true), ("L2", true)]);
    // Whether a list is shared with other people comes with it (#461).
    assert_eq!(got.lists.iter().map(|l| l.shared).collect::<Vec<_>>(), [false, true]);
    assert_eq!(got.tasks.len(), 3);
    let a = &got.tasks[0];
    assert_eq!((a.id.as_str(), a.list.as_str(), a.title.as_str(), a.importance), ("A", "L1", "Call Bo", Importance::High));
    assert_eq!(a.notes, "Bring the tea & cake");
    assert_eq!((a.due, a.start), (Date::new(2026, 10, 30), Date::new(2026, 10, 28)));
    assert_eq!(a.categories, ["Red category"]);
    assert_eq!(a.reminder, Some(1_793_352_600));
    assert_eq!(a.change_key, "W/\"x\"");
    let b = &got.tasks[1];
    assert_eq!((b.completed, b.importance), (Some(1_792_490_400), Importance::Low));
    assert_eq!(got.tasks[2].list, "L2");
}

#[test]
fn hostile_entries_are_skipped_or_cut_to_size() {
    let state = todo();
    state.lock().unwrap().lists[0].2 = vec![
        json!({"title": "No id"}),
        json!({"id": "has space", "title": "Bad id"}),
        json!({"id": "A", "title": format!("long\u{0}{}", "x".repeat(5_000)), "dueDateTime": {"dateTime": "not a date"}, "importance": "weird"}),
        json!({"id": "A", "title": "Same id twice"}),
        json!("a string"),
        json!(null),
    ];
    let server = serve(&state);
    let got = tasks_at(&server.url(""), TOKEN).unwrap();
    assert_eq!(got.tasks.len(), 1);
    assert_eq!(got.tasks[0].title.chars().count(), 500);
    assert!(!got.tasks[0].title.contains('\u{0}'));
    assert_eq!((got.tasks[0].due, got.tasks[0].importance), (None, Importance::Normal));
    // Entries that couldn't be read mean the list isn't known to be whole: nothing is removed for it.
    assert!(!got.lists[0].complete);
}

#[test]
fn a_sign_in_without_the_permission_is_forbidden_for_a_read() {
    let state = todo();
    state.lock().unwrap().all_status = Some(403);
    let server = serve(&state);
    assert!(matches!(tasks_at(&server.url(""), TOKEN), Err(NetError::Forbidden(_))));
    state.lock().unwrap().all_status = Some(401);
    assert!(tasks_at(&server.url(""), TOKEN).unwrap_err().is_refused_sign_in());
}

#[test]
fn a_task_made_here_goes_to_the_default_list_and_is_named_by_the_server() {
    let state = todo();
    let server = serve(&state);
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![create("Write the report")]));
    assert_eq!(done.outcomes.len(), 1);
    let o = &done.outcomes[0];
    assert_eq!((o.result, o.task, o.id.as_str(), o.list.as_str(), o.change_key.as_str()), (TaskResult::Done, 7, "T1", "L1", "W/\"e1\""));
    assert_eq!(state.lock().unwrap().lists[0].2[0]["title"], "Write the report");
}

#[test]
fn the_default_list_is_found_by_graphs_own_name_wherever_it_comes() {
    let state = todo();
    let flagged = state.lock().unwrap().lists.remove(2);
    state.lock().unwrap().lists.insert(0, flagged);
    state.lock().unwrap().lists.insert(0, ("L0".into(), "none".into(), Vec::new()));
    let server = serve(&state);
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![create("Write the report")]));
    assert_eq!(done.outcomes[0].list, "L1");
    assert_eq!(state.lock().unwrap().lists[2].2[0]["title"], "Write the report");
}

#[test]
fn a_change_sends_only_the_fields_that_changed() {
    let state = todo();
    state.lock().unwrap().lists[1].2 =
        vec![json!({"id": "A", "title": "Old", "importance": "low", "body": {"content": "keep", "contentType": "text"}})];
    let server = serve(&state);
    let patch =
        TaskPatch { title: Some("New".into()), due: Some(Date::new(2026, 11, 2)), completed: Some(Some(1_790_000_000)), ..Default::default() };
    let op = TaskOp { kind: TaskOpKind::Update, task: 3, id: "A".into(), list: "L2".into(), patch, ..Default::default() };
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![op]));
    assert_eq!(done.outcomes[0].result, TaskResult::Done);
    let task = state.lock().unwrap().lists[1].2[0].clone();
    assert_eq!((task["title"].as_str(), task["status"].as_str(), task["importance"].as_str()), (Some("New"), Some("completed"), Some("low")));
    assert_eq!(task["body"]["content"], "keep");
    assert_eq!(task["dueDateTime"]["dateTime"], "2026-11-02T00:00:00.0000000");
}

#[test]
fn clearing_a_field_sends_null_and_reminders_say_whether_they_are_on() {
    let body = task_body(&TaskPatch { due: Some(None), reminder: Some(None), completed: Some(None), ..Default::default() });
    assert_eq!(body, json!({"dueDateTime": null, "isReminderOn": false, "status": "notStarted"}));
    let body = task_body(&TaskPatch { reminder: Some(Some(1_793_352_600)), ..Default::default() });
    assert_eq!(body["isReminderOn"], true);
    assert_eq!(body["reminderDateTime"]["dateTime"], "2026-10-30T09:30:00.0000000");
    assert_eq!(task_body(&TaskPatch::default()), json!({}));
}

#[test]
fn a_deletion_is_done_when_the_task_is_there_and_when_it_is_gone() {
    let state = todo();
    state.lock().unwrap().lists[0].2 = vec![json!({"id": "A", "title": "x"})];
    let server = serve(&state);
    let del = |id: &str| TaskOp { kind: TaskOpKind::Delete, id: id.into(), list: "L1".into(), ..Default::default() };
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![del("A"), del("A")]));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [TaskResult::Done, TaskResult::Gone]);
    assert!(state.lock().unwrap().lists[0].2.is_empty());
}

#[test]
fn a_no_stops_the_rest_and_says_why() {
    let state = todo();
    let server = serve(&state);
    for (status, result) in [(403, TaskResult::Denied), (503, TaskResult::Failed), (500, TaskResult::Failed)] {
        state.lock().unwrap().write_status = Some(status);
        let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![create("a"), create("b")]));
        assert_eq!(done.outcomes.len(), 1, "{status}");
        assert_eq!(done.outcomes[0].result, result, "{status}");
    }
    // Refused for good does not stop the others.
    state.lock().unwrap().write_status = Some(400);
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![create("a"), create("b")]));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [TaskResult::Refused, TaskResult::Refused]);
    state.lock().unwrap().write_status = Some(401);
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![create("a")]));
    assert!(done.token_refused && done.outcomes.is_empty());
}

#[test]
fn ids_that_cannot_be_ids_are_not_sent_anywhere() {
    let state = todo();
    let server = serve(&state);
    let op = TaskOp { kind: TaskOpKind::Delete, id: "../../me/messages".into(), list: "L1".into(), ..Default::default() };
    let empty = TaskOp { kind: TaskOpKind::Update, id: "A".into(), list: "L1".into(), ..Default::default() };
    let done = push_tasks_at(&server.url(""), TOKEN, &upload(vec![op, empty]));
    // (The first has a slash, which is encoded into one piece of the address and finds nothing.)
    assert!(done.outcomes.iter().all(|o| matches!(o.result, TaskResult::Refused | TaskResult::Gone)));
    assert_eq!(done.outcomes[1].result, TaskResult::Refused);
}

#[test]
fn at_most_a_hundred_changes_go_in_one_upload() {
    let state = todo();
    let server = serve(&state);
    let done = push_tasks_at(&server.url(""), TOKEN, &upload((0..150).map(|n| create(&format!("t{n}"))).collect()));
    assert_eq!(done.outcomes.len(), 100);
}

#[test]
fn a_due_date_written_in_a_zone_stays_on_its_day() {
    let entry =
        serde_json::json!({"id": "t1", "title": "x", "dueDateTime": {"dateTime": "2026-07-14T00:00:00.0000000", "timeZone": "Europe/London"}});
    let task = super::tasks::task_of(&entry, "L1").unwrap();
    assert_eq!(task.due, mailcraft_time::Date::new(2026, 7, 14));
}

#[test]
fn task_steps_come_with_their_tasks_in_one_request_per_page() {
    let state = todo();
    state.lock().unwrap().lists[0].2 = (0..30)
        .map(|n| json!({"id": format!("T{n}"), "title": "Trip", "checklistItems": [{"id": "S1", "displayName": "Pack tea", "isChecked": true}]}))
        .collect();
    let server = serve(&state);
    let got = tasks_at(&server.url(""), TOKEN).unwrap();
    assert_eq!(got.tasks.len(), 30);
    let task = serde_json::to_value(&got.tasks[0]).unwrap();
    assert_eq!(task["steps"], json!([{ "id": "S1", "title": "Pack tea", "completed": true }]));
    let todo = state.lock().unwrap();
    assert_eq!(todo.checklist_reads, 0, "no request per to-do");
    assert_eq!(todo.task_pages, 2, "the stand-in answers both lists' pages whole");
}

#[test]
fn steps_are_capped_in_all_and_the_rest_are_unknown_not_empty() {
    let per = mailcraft_model::MAX_TASK_STEPS;
    let tasks = super::tasks::MAX_STEPS_IN_ALL / per + 2;
    let state = todo();
    state.lock().unwrap().lists[0].2 = (0..tasks)
        .map(|n| {
            let steps: Vec<Value> = (0..per).map(|i| json!({"id": format!("S{i}"), "displayName": "Step", "isChecked": false})).collect();
            json!({"id": format!("T{n}"), "title": "Trip", "checklistItems": steps})
        })
        .collect();
    let server = serve(&state);
    let got = tasks_at(&server.url(""), TOKEN).unwrap();
    let read: usize = got.tasks.iter().filter_map(|t| t.steps.as_ref()).map(Vec::len).sum();
    assert!(read <= super::tasks::MAX_STEPS_IN_ALL);
    assert_eq!(got.tasks.iter().filter(|t| t.steps.is_none()).count(), 2);
}

#[test]
fn incomplete_or_malformed_checklists_are_unknown_not_empty() {
    let too_many: Vec<Value> =
        (0..=mailcraft_model::MAX_TASK_STEPS).map(|i| json!({"id": format!("S{i}"), "displayName": "Step", "isChecked": false})).collect();
    for task in [
        json!({"id": "T1", "title": "Trip", "checklistItems": [{"id": "S1", "displayName": "missing checked state"}]}),
        json!({"id": "T1", "title": "Trip", "checklistItems": too_many}),
        json!({"id": "T1", "title": "Trip", "checklistItems": [], "checklistItems@odata.nextLink": "https://graph.microsoft.com/more"}),
        json!({"id": "T1", "title": "Trip"}),
        json!({"id": "T1", "title": "Trip", "checklistItems": "nonsense"}),
    ] {
        let server = FakeHttp::start(move |_, request| {
            let target = request.split_whitespace().nth(1).unwrap();
            let value = if target.contains("/tasks") { json!({"value": [task]}) } else { json!({"value": [{"id": "L1"}]}) };
            json_answer("200 OK", &value.to_string())
        });
        let got = tasks_at(&server.url(""), TOKEN).unwrap();
        assert_eq!(got.tasks[0].title, "Trip");
        assert!(got.tasks[0].steps.is_none());
    }
}

#[test]
fn checklist_changes_use_their_own_endpoints_and_keep_the_parent() {
    let state = todo();
    state.lock().unwrap().lists[0].2 = vec![json!({"id": "T1", "title": "Trip", "body": {"content": "keep"}})];
    let server = serve(&state);
    let step_op = |kind, id: &str, title: &str, completed| TaskOp {
        kind,
        task: 1,
        id: "T1".into(),
        list: "L1".into(),
        patch: TaskPatch { steps: Some(vec![mailcraft_model::TaskStep { id: id.into(), title: title.into(), completed }]), ..Default::default() },
        known_steps: Vec::new(),
    };
    let created = push_tasks_at(&server.url(""), TOKEN, &upload(vec![step_op(TaskOpKind::StepCreate, "local:1", "Pack tea", false)]));
    assert_eq!(created.outcomes[0].result, TaskResult::Done);
    assert_eq!(created.outcomes[0].step_id, "S1");
    let changed = push_tasks_at(&server.url(""), TOKEN, &upload(vec![step_op(TaskOpKind::StepUpdate, "S1", "Pack cake", true)]));
    assert_eq!(changed.outcomes[0].result, TaskResult::Done);
    let got = tasks_at(&server.url(""), TOKEN).unwrap();
    let steps = got.tasks[0].steps.as_ref().unwrap();
    assert_eq!((steps[0].title.as_str(), steps[0].completed), ("Pack cake", true));
    let removed = push_tasks_at(&server.url(""), TOKEN, &upload(vec![step_op(TaskOpKind::StepDelete, "S1", "Pack cake", true)]));
    assert_eq!(removed.outcomes[0].result, TaskResult::Done);
    let task = state.lock().unwrap().lists[0].2[0].clone();
    assert_eq!(task["title"], "Trip");
    assert_eq!(task["body"]["content"], "keep");
    assert_eq!(task["checklistItems"], json!([]));
}

#[test]
fn a_failed_checklist_write_is_not_acknowledged_and_stops_the_upload() {
    // A 503 is checked at once: the checklist read again shows nothing was made, so it can go again.
    for (status, result) in [(429, TaskResult::Failed), (503, TaskResult::Failed), (403, TaskResult::Denied)] {
        let state = todo();
        state.lock().unwrap().lists[0].2 = vec![json!({"id": "T1", "title": "Trip"})];
        state.lock().unwrap().write_status = Some(status);
        let server = serve(&state);
        let op = TaskOp {
            kind: TaskOpKind::StepCreate,
            task: 1,
            id: "T1".into(),
            list: "L1".into(),
            patch: TaskPatch {
                steps: Some(vec![mailcraft_model::TaskStep { id: "local:1".into(), title: "Tea".into(), completed: false }]),
                ..Default::default()
            },
            known_steps: Vec::new(),
        };
        let uploaded = push_tasks_at(&server.url(""), TOKEN, &upload(vec![op.clone(), op]));
        assert_eq!(uploaded.outcomes.len(), 1);
        assert_eq!(uploaded.outcomes[0].result, result);
        assert!(uploaded.outcomes[0].step_id.is_empty());
    }
    let server = FakeHttp::answering(json_answer("201 Created", "{}"));
    let op = TaskOp {
        kind: TaskOpKind::StepCreate,
        task: 1,
        id: "T1".into(),
        list: "L1".into(),
        patch: TaskPatch {
            steps: Some(vec![mailcraft_model::TaskStep { id: "local:1".into(), title: "Tea".into(), completed: false }]),
            ..Default::default()
        },
        known_steps: Vec::new(),
    };
    let uploaded = push_tasks_at(&server.url(""), TOKEN, &upload(vec![op]));
    assert_eq!(uploaded.outcomes[0].result, TaskResult::Uncertain);
}

#[test]
fn review_a_lost_create_reply_is_uncertain_not_automatically_retried() {
    let server = FakeHttp::answering(Vec::new());
    let op = TaskOp {
        kind: TaskOpKind::StepCreate,
        task: 1,
        id: "T1".into(),
        list: "L1".into(),
        patch: TaskPatch {
            steps: Some(vec![mailcraft_model::TaskStep { id: "local:1".into(), title: "Tea".into(), completed: false }]),
            ..Default::default()
        },
        known_steps: Vec::new(),
    };
    let uploaded = push_tasks_at(&server.url(""), TOKEN, &upload(vec![op.clone(), op]));
    assert_eq!(uploaded.outcomes.len(), 1);
    assert_eq!(uploaded.outcomes[0].result, TaskResult::Uncertain);
    assert!(uploaded.outcomes[0].step_id.is_empty());
}

fn step_create(title: &str, known: &[&str]) -> TaskOp {
    TaskOp {
        kind: TaskOpKind::StepCreate,
        task: 1,
        id: "T1".into(),
        list: "L1".into(),
        patch: TaskPatch {
            steps: Some(vec![mailcraft_model::TaskStep { id: "local:1".into(), title: title.into(), completed: false }]),
            ..Default::default()
        },
        known_steps: known.iter().map(|k| k.to_string()).collect(),
    }
}

#[test]
fn a_step_made_without_its_answer_is_found_at_once_not_made_twice() {
    for status in [503, 500] {
        let state = todo();
        state.lock().unwrap().lists[0].2 =
            vec![json!({"id": "T1", "title": "Trip", "checklistItems": [{"id": "S1", "displayName": "Tea", "isChecked": false}]})];
        state.lock().unwrap().made_then_status = Some(status);
        let server = serve(&state);
        // "Tea" S1 was there before: the one made now is S2.
        let uploaded = push_tasks_at(&server.url(""), TOKEN, &upload(vec![step_create("Tea", &["S1"])]));
        assert_eq!(uploaded.outcomes[0].result, TaskResult::Done, "{status}");
        assert_eq!(uploaded.outcomes[0].step_id, "S2");
        assert_eq!(state.lock().unwrap().lists[0].2[0]["checklistItems"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn a_step_that_cannot_be_checked_again_stays_uncertain() {
    // Every request fails: whether it was made can't be told.
    let server = FakeHttp::answering(raw("500 Internal Server Error"));
    let uploaded = push_tasks_at(&server.url(""), TOKEN, &upload(vec![step_create("Tea", &[])]));
    assert_eq!(uploaded.outcomes.len(), 1);
    assert_eq!(uploaded.outcomes[0].result, TaskResult::Uncertain);
    assert!(uploaded.outcomes[0].step_id.is_empty());
}
