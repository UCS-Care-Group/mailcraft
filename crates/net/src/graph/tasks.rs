// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft To Do through Graph (#248): the owner's lists of tasks and the tasks in them,
//! read whole each time (capped: [`MAX_TASK_LISTS`], [`MAX_TASKS_PER_LIST`], [`MAX_TASKS`]),
//! and the changes made here sent one by one.
//!
//! Everything is `/v1.0/me/todo/lists…` and needs `Tasks.ReadWrite`: a sign-in that doesn't
//! cover it is answered 403, which is [`NetError::Forbidden`] for a read and
//! [`TaskResult::Denied`] for a change. The list of flagged mail (`flaggedEmails`) is Outlook's
//! mirror of the flags on messages and is left alone, here as everywhere. Nothing of a task (its
//! title, its text, its id) is written to a log.
//!
//! What comes back is hostile JSON like everything else from a server: pages are capped, an
//! entry that is no task is skipped, and [`RemoteTask::tidy`] cuts the rest to size.

use std::collections::HashSet;

use mailcraft_model::remote_tasks::{
    MAX_TASK_LISTS, MAX_TASK_OPS, MAX_TASKS, MAX_TASKS_PER_LIST, RemoteTask, RemoteTaskList, TaskOpKind, TaskOutcome, TaskResult, TasksDownload,
    TasksUpload, TasksUploaded,
};
use mailcraft_model::{Importance, TaskPatch};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Value, json};

use super::{Doing, Graph, MAX_WRITTEN, PAGE_SIZE, WRITE_TIMEOUT, instant, nonsense, text, when};
use crate::NetError;
use crate::http;

const LISTS: &str = "/v1.0/me/todo/lists";
/// What Graph calls a well-known list (`defaultList`, `flaggedEmails`) on a `todoTaskList`.
const WELL_KNOWN: &str = "wellknownListName";
/// The longest title Microsoft To Do keeps.
const MAX_TITLE: usize = 255;
/// Longest id of a list or a task that is put into an address.
const MAX_ID: usize = 1_024;

/// The owner's to-dos, as Microsoft To Do has them.
pub fn tasks(token: &str) -> Result<TasksDownload, NetError> {
    tasks_at(&super::graph_base(), token)
}

/// Tell Microsoft To Do what was done here to the owner's to-dos, one change after the other;
/// what it made of each comes back ([`TaskOutcome`]).
///
/// - made (201), changed (200), deleted (204), or any other 2xx: [`TaskResult::Done`].
/// - 404 or 410: [`TaskResult::Gone`].
/// - 403: [`TaskResult::Denied`]; the rest of the upload is left untried.
/// - 429, 503, 504, any other 5xx, no connection, an answer that makes no sense:
///   [`TaskResult::Failed`], to be tried again; the rest is left untried.
/// - 409 and 412 (something changed there meanwhile): [`TaskResult::Failed`], to be tried again.
/// - any other refusal (400, 413…): [`TaskResult::Refused`], for good.
/// - 401: nothing more is sent, and the upload says the sign-in was refused.
pub fn push_tasks(token: &str, upload: &TasksUpload) -> TasksUploaded {
    push_tasks_at(&super::graph_base(), token, upload)
}

pub(super) fn tasks_at(base: &str, token: &str) -> Result<TasksDownload, NetError> {
    let graph = Graph::at(base, token)?;
    let mut download = TasksDownload::default();
    // Each list's id, its well-known name, and whether it is shared with other people.
    let mut found: Vec<(String, String, bool)> = Vec::new();
    let query = http::form(&[("$top", "100")]);
    let whole = graph.list(Doing::Tasks, graph.url(&format!("{LISTS}?{query}")), &[], MAX_TASK_LISTS, &mut |entry| {
        let id = text(entry, "id");
        if valid_id(id) && !found.iter().any(|(have, ..)| have == id) {
            let shared = entry.get("isShared").and_then(Value::as_bool).unwrap_or(false);
            found.push((id.to_string(), text(entry, WELL_KNOWN).to_string(), shared));
        }
    })?;
    download.complete = whole;
    let mut seen: HashSet<String> = HashSet::new();
    for (list, _, shared) in found.iter().filter(|(_, well_known, _)| well_known != "flaggedEmails") {
        let shared = *shared;
        let room = MAX_TASKS.saturating_sub(download.tasks.len());
        let mut unread = 0usize;
        let first = graph.url(&format!("{LISTS}/{}/tasks?{}", http::encode(list), http::form(&[("$top", PAGE_SIZE)])));
        let most = MAX_TASKS_PER_LIST.min(room);
        let read = if room == 0 {
            Ok(false)
        } else {
            graph.list(Doing::Tasks, first, &[], most, &mut |entry| match task_of(entry, list) {
                Some(task) if seen.insert(task.id.clone()) => download.tasks.push(task),
                Some(_) => {}
                None => unread += 1,
            })
        };
        match read {
            Ok(whole) => download.lists.push(RemoteTaskList { id: list.clone(), complete: whole && unread == 0, shared }),
            // A list that went while it was being read is not there any more: the next run knows.
            Err(NetError::Proto(mailcraft_proto::ProtoError::Rejected(_))) => {
                download.lists.push(RemoteTaskList { id: list.clone(), complete: false, shared });
            }
            Err(e) => return Err(e),
        }
    }
    log::debug!("tasks: {} of them in {} lists, complete: {}", download.tasks.len(), download.lists.len(), download.complete);
    Ok(download)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The date a `{"dateTime": "2026-10-12T00:00:00.0000000", "timeZone": "Europe/London"}` falls
/// on, as written in the zone it was written in: nothing is converted, because the days of a
/// task are days, not instants (a conversion to UTC moved a London midnight to the day before
/// in summer). Instants (reminder, done) are read with their own zone by `when`.
fn day_of(value: Option<&Value>) -> Option<mailcraft_time::Date> {
    let written = text(value?, "dateTime");
    mailcraft_time::Date::parse_iso(written.get(..10)?)
}

/// Words from HTML: tags dropped, the few entities that matter read.
fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len().min(1 << 16));
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

pub(super) fn task_of(entry: &Value, list: &str) -> Option<RemoteTask> {
    let id = text(entry, "id");
    if !valid_id(id) {
        return None;
    }
    let body = entry.get("body");
    let content = body.map(|b| text(b, "content")).unwrap_or("");
    let notes =
        if body.map(|b| text(b, "contentType")).is_some_and(|kind| kind.eq_ignore_ascii_case("html")) { plain(content) } else { content.to_string() };
    let completed = if text(entry, "status") == "completed" {
        // (A completed task that doesn't say when is completed when it was last changed.)
        Some(when(entry.get("completedDateTime")).or_else(|| instant(text(entry, "lastModifiedDateTime"))).unwrap_or_else(mailcraft_time::now))
    } else {
        None
    };
    let reminder = if entry.get("isReminderOn").and_then(Value::as_bool) == Some(true) { when(entry.get("reminderDateTime")) } else { None };
    let categories =
        entry.get("categories").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(100).collect());
    let mut task = RemoteTask {
        id: id.to_string(),
        list: list.to_string(),
        change_key: text(entry, "@odata.etag").to_string(),
        title: text(entry, "title").to_string(),
        notes,
        start: day_of(entry.get("startDateTime")),
        due: day_of(entry.get("dueDateTime")),
        completed,
        importance: Importance::parse(text(entry, "importance")).unwrap_or_default(),
        categories: categories.unwrap_or_default(),
        reminder,
    };
    task.tidy();
    Some(task)
}

fn date_time(day: mailcraft_time::Date) -> Value {
    json!({"dateTime": format!("{}T00:00:00.0000000", day.iso()), "timeZone": "UTC"})
}

fn instant_value(at: Timestamp) -> Value {
    json!({"dateTime": format!("{}.0000000", DateTime::from_timestamp(at, 0).iso()), "timeZone": "UTC"})
}

/// The body of a creation or a change: only the fields of `patch`.
pub(super) fn task_body(patch: &TaskPatch) -> Value {
    let mut body = serde_json::Map::new();
    if let Some(title) = &patch.title {
        body.insert("title".into(), json!(title.chars().take(MAX_TITLE).collect::<String>()));
    }
    if let Some(notes) = &patch.notes {
        body.insert("body".into(), json!({"content": notes, "contentType": "text"}));
    }
    if let Some(start) = patch.start {
        body.insert("startDateTime".into(), start.map_or(Value::Null, date_time));
    }
    if let Some(due) = patch.due {
        body.insert("dueDateTime".into(), due.map_or(Value::Null, date_time));
    }
    if let Some(done) = patch.completed {
        body.insert("status".into(), json!(if done.is_some() { "completed" } else { "notStarted" }));
        if let Some(at) = done {
            body.insert("completedDateTime".into(), instant_value(at));
        }
    }
    if let Some(importance) = patch.importance {
        body.insert("importance".into(), json!(importance.code()));
    }
    if let Some(categories) = &patch.categories {
        body.insert("categories".into(), json!(categories));
    }
    if let Some(reminder) = patch.reminder {
        body.insert("isReminderOn".into(), json!(reminder.is_some()));
        if let Some(at) = reminder {
            body.insert("reminderDateTime".into(), instant_value(at));
        }
    }
    Value::Object(body)
}

pub(super) fn push_tasks_at(base: &str, token: &str, upload: &TasksUpload) -> TasksUploaded {
    let mut uploaded = TasksUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let Ok(graph) = Graph::at(base, token) else {
        uploaded.token_refused = true;
        return uploaded;
    };
    let mut default_list: Option<Result<String, NetError>> = None;
    for op in upload.ops.iter().take(MAX_TASK_OPS) {
        let outcome = |result: TaskResult, error: &str| TaskOutcome {
            kind: op.kind,
            task: op.task,
            id: op.id.clone(),
            list: op.list.clone(),
            patch: op.patch.clone(),
            result,
            error: error.into(),
            ..Default::default()
        };
        if op.kind != TaskOpKind::Create && !(valid_id(&op.id) && valid_id(&op.list)) {
            uploaded.outcomes.push(outcome(TaskResult::Refused, "that isn't a to-do the server could have"));
            continue;
        }
        if op.kind == TaskOpKind::Update && op.patch.is_empty() {
            uploaded.outcomes.push(outcome(TaskResult::Refused, "the change says nothing that could be sent"));
            continue;
        }
        // A to-do made here goes in the owner's default list, looked for once.
        let list = if op.kind == TaskOpKind::Create {
            match default_list.get_or_insert_with(|| default_list_of(&graph)) {
                Ok(list) => list.clone(),
                Err(e) if e.is_refused_sign_in() => {
                    uploaded.token_refused = true;
                    break;
                }
                Err(NetError::Forbidden(why)) => {
                    uploaded.outcomes.push(outcome(TaskResult::Denied, why));
                    break;
                }
                Err(e) => {
                    uploaded.outcomes.push(outcome(TaskResult::Failed, &e.to_string()));
                    break;
                }
            }
        } else {
            op.list.clone()
        };
        let lists = format!("{LISTS}/{}/tasks", http::encode(&list));
        let (method, path, body) = match op.kind {
            TaskOpKind::Create => ("POST", lists, Some(task_body(&op.patch).to_string())),
            TaskOpKind::Update => ("PATCH", format!("{lists}/{}", http::encode(&op.id)), Some(task_body(&op.patch).to_string())),
            TaskOpKind::Delete => ("DELETE", format!("{lists}/{}", http::encode(&op.id)), None),
        };
        let headers: &[(&str, &str)] = if body.is_some() { &[("Content-Type", "application/json")] } else { &[] };
        let reply = match graph.ask(&graph.url(&path), method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN) {
            Ok(reply) => reply,
            Err(e) => {
                uploaded.outcomes.push(outcome(TaskResult::Failed, &e.to_string()));
                break;
            }
        };
        let sent = body.as_deref().unwrap_or("");
        let mut done = outcome(TaskResult::Done, "");
        done.status = reply.status;
        let stop = match reply.status {
            401 => {
                uploaded.token_refused = true;
                break;
            }
            200..=299 => {
                if op.kind == TaskOpKind::Create {
                    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                    done.id = text(&json, "id").to_string();
                    done.list = list.clone();
                    done.change_key = text(&json, "@odata.etag").to_string();
                    // Made, and no word of what it is called: not made again (the next download
                    // shows the copy, which is taken in as this one); said, not hidden.
                    if !valid_id(&done.id) {
                        done.result = TaskResult::Refused;
                        done.error = "the server made it and didn't say what it is called".into();
                    }
                } else if op.kind == TaskOpKind::Update {
                    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                    done.change_key = text(&json, "@odata.etag").to_string();
                }
                false
            }
            404 | 410 => {
                done.result = TaskResult::Gone;
                false
            }
            403 => {
                done.result = TaskResult::Denied;
                done.error = "this account may not change its to-dos".into();
                true
            }
            409 | 412 | 429 | 503 | 504 => {
                done.result = TaskResult::Failed;
                done.error = "Microsoft Graph is busy or the to-do changed there meanwhile".into();
                true
            }
            status if status >= 500 => {
                done.result = TaskResult::Failed;
                done.error = format!("Microsoft Graph failed (status {status})");
                true
            }
            status => {
                done.result = TaskResult::Refused;
                done.error = match super::refusal(Doing::Tasks, &reply, &[token, sent], mailcraft_time::now()) {
                    NetError::Proto(mailcraft_proto::ProtoError::Rejected(why)) => why,
                    e => e.to_string(),
                };
                if done.error.is_empty() {
                    done.error = format!("Microsoft Graph refused it (status {status})");
                }
                false
            }
        };
        uploaded.outcomes.push(done);
        if stop {
            break;
        }
    }
    for outcome in &mut uploaded.outcomes {
        outcome.tidy();
    }
    uploaded
}

/// The id of the owner's default list (the one To Do calls `defaultList`), else the first.
fn default_list_of(graph: &Graph<'_>) -> Result<String, NetError> {
    let (mut default, mut first) = (String::new(), String::new());
    graph.list(Doing::Tasks, graph.url(&format!("{LISTS}?{}", http::form(&[("$top", "100")]))), &[], MAX_TASK_LISTS, &mut |entry| {
        let id = text(entry, "id");
        if !valid_id(id) || text(entry, WELL_KNOWN) == "flaggedEmails" {
            return;
        }
        if first.is_empty() {
            first = id.to_string();
        }
        if default.is_empty() && text(entry, WELL_KNOWN) == "defaultList" {
            default = id.to_string();
        }
    })?;
    let chosen = if default.is_empty() { first } else { default };
    if chosen.is_empty() { Err(nonsense("the account has no list of to-dos to make one in")) } else { Ok(chosen) }
}
