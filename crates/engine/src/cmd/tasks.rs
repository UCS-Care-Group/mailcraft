// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks: to-do items.

use mailcraft_model::{Id, Importance, Task, clean_name};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::{bad, done, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Module, Session, TaskFilter, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tasks.new", "New Task", "Home › New", new).key("Mod+Shift+K").params(TASK),
        CommandSpec::new("tasks.edit", "Edit Task", "Task", edit).params(r#"{"id"?: number, …the fields of tasks.new}"#).selection(needs_task),
        CommandSpec::new("tasks.delete", "Delete", "Home › Delete", delete).params(ONE).selection(needs_task),
        CommandSpec::new("tasks.complete", "Mark Complete", "Home › Manage Task", complete)
            .params(r#"{"value"?: bool (default: toggle), "id"?: number}"#)
            .selection(needs_task),
        CommandSpec::new("tasks.select", "Select Task", "Tasks", select).pure().params(r#"{"id": number | null}"#),
        CommandSpec::new("tasks.get", "Get Task", "Tasks", get).pure().params(ONE),
        CommandSpec::new("tasks.arrange", "Arrange Tasks By", "View › Arrangement", arrange).pure().params(r#"{"by": "dueDate"|"categories"}"#),
        CommandSpec::new("tasks.list", "List Tasks", "Tasks", list).pure().params(r#"{"filter"?: "active"|"today"|"overdue"|"completed"|"all"}"#),
        CommandSpec::new("tasks.categorize", "Categorize", "Home › Tags", categorize)
            .params(r#"{"category": string, "value"?: bool, "id"?: number}"#)
            .selection(needs_task),
        CommandSpec::new("tasks.fromMessage", "Add to Tasks", "Home › Tags", from_message)
            .params(r#"{"id"?: number, "due"?: "YYYY-MM-DD"} (a message; default: the selected one)"#)
            .selection(needs_message),
    ]
}

const ONE: &str = r#"{"id"?: number} (default: the selected task)"#;
const TASK: &str = r#"{"title": string, "notes"?: string, "due"?: "YYYY-MM-DD" | null, "start"?: "YYYY-MM-DD" | null, "importance"?: "low"|"normal"|"high", "categories"?: [string], "reminder"?: "YYYY-MM-DDTHH:MM" | null}"#;

fn needs_task(s: &Session) -> Option<&'static str> {
    if s.sel.task.is_none() { Some("select a task first") } else { None }
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").or(s.sel.task).ok_or_else(|| fail("select a task first"))?;
    s.mailbox.task(id).map(|t| t.id).ok_or_else(|| failed!("no task with id {id}", id))
}

fn apply(s: &Session, t: &mut Task, v: &Value) -> Result<(), CmdError> {
    if let Some(title) = p::str(v, "title") {
        t.title = title.replace(['\r', '\n'], " ").trim().chars().take(500).collect();
    }
    if let Some(n) = p::str(v, "notes") {
        t.notes = n.to_string();
    }
    if v.get("due").is_some() {
        t.due = p::date(v, "due")?;
    }
    if v.get("start").is_some() {
        t.start = p::date(v, "start")?;
    }
    if let Some(i) = p::str(v, "importance") {
        t.importance = Importance::parse(i).ok_or_else(|| bad("`importance` is low, normal or high"))?;
    }
    if let Some(c) = p::strings(v, "categories") {
        t.categories = c;
    }
    if v.get("reminder").is_some() {
        t.reminder = p::datetime(v, "reminder")?.map(|at| at.to_timestamp(s.offset_at(s.now())));
    }
    Ok(())
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let mut t = Task::default();
    apply(s, &mut t, v)?;
    if t.title.is_empty() {
        return Err(bad("`title` is required"));
    }
    let id = s.mailbox.add_task(t);
    s.sel.task = Some(id);
    Ok(json!({"id": id}))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let mut t = s.mailbox.task(id).cloned().ok_or_else(|| fail("the task is gone"))?;
    apply(s, &mut t, v)?;
    if t.title.is_empty() {
        return Err(fail("a task needs a title"));
    }
    *s.mailbox.task_mut(id)? = t;
    Ok(json!({"id": id}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let order = sorted(s, s.view.task_filter);
    let at = order.iter().position(|x| *x == id);
    s.mailbox.remove_task(id)?;
    if s.sel.task == Some(id) {
        s.sel.task = at.and_then(|i| order.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| order.get(j)))).copied();
    }
    Ok(json!({"deleted": id}))
}

fn complete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let now = s.now();
    let t = s.mailbox.task_mut(id)?;
    let done = p::bool(v, "value").unwrap_or(!t.is_complete());
    t.completed = if done { Some(t.completed.unwrap_or(now)) } else { None };
    let message = t.message;
    // A to-do made from a flagged message keeps that flag in step.
    if let Some(m) = message
        && let Ok(m) = s.mailbox.message_mut(m)
    {
        m.flag = if done { mailcraft_model::FlagStatus::Complete } else { mailcraft_model::FlagStatus::Flagged };
    }
    Ok(json!({"id": id, "complete": done}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    s.sel.task = match p::id(v, "id") {
        Some(id) => Some(s.mailbox.task(id).map(|t| t.id).ok_or_else(|| failed!("no task with id {id}", id))?),
        None => None,
    };
    done()
}

fn task_json(s: &Session, t: &Task) -> Value {
    json!({
        "id": t.id, "title": t.title, "notes": t.notes, "due": t.due.map(Date::iso), "start": t.start.map(Date::iso),
        "complete": t.is_complete(), "completed": t.completed.map(mailcraft_time::format_iso_utc), "overdue": t.is_overdue(s.today()),
        "dueLabel": t.due_label(s.today()), "importance": t.importance, "categories": t.categories, "message": t.message,
        "reminder": t.reminder.map(|r| s.local(r).iso()),
    })
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.task(id).map(|t| task_json(s, t)).ok_or_else(|| fail("the task is gone"))
}

/// Tasks as the list shows them: open ones by due date (undated last), then finished ones,
/// most recently finished first.
pub fn sorted(s: &Session, filter: TaskFilter) -> Vec<Id> {
    let today = s.today();
    let mut v: Vec<&Task> = s
        .mailbox
        .tasks
        .values()
        .map(|t| t.as_ref())
        .filter(|t| match filter {
            TaskFilter::Active => !t.is_complete(),
            TaskFilter::Today => !t.is_complete() && t.due.is_some_and(|d| d <= today),
            TaskFilter::Overdue => t.is_overdue(today),
            TaskFilter::Completed => t.is_complete(),
            TaskFilter::All => true,
        })
        .collect();
    v.sort_by(|a, b| {
        a.is_complete()
            .cmp(&b.is_complete())
            .then_with(|| b.completed.cmp(&a.completed))
            .then_with(|| a.due.is_none().cmp(&b.due.is_none()))
            .then_with(|| a.due.cmp(&b.due))
            .then_with(|| b.importance.cmp(&a.importance))
            .then(a.id.cmp(&b.id))
    });
    v.into_iter().map(|t| t.id).collect()
}

fn arrange(s: &mut Session, v: &Value) -> CmdResult {
    s.view.task_sort = crate::session::parse_enum::<crate::TaskSort>(p::req_str(v, "by")?).ok_or_else(|| bad("`by` is dueDate or categories"))?;
    Ok(json!({"by": s.view.task_sort}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let filter = match p::str(v, "filter") {
        Some(f) => crate::session::parse_enum::<TaskFilter>(f).ok_or_else(|| bad("`filter` is active, today, overdue, completed or all"))?,
        None => s.view.task_filter,
    };
    let items: Vec<Value> = sorted(s, filter).into_iter().filter_map(|id| s.mailbox.task(id)).map(|t| task_json(s, t)).collect();
    Ok(json!({"filter": filter, "tasks": items}))
}

fn categorize(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let name = clean_name(p::req_str(v, "category")?).ok_or_else(|| bad("`category` is empty"))?;
    let t = s.mailbox.task_mut(id)?;
    let has = t.categories.iter().any(|x| x.eq_ignore_ascii_case(&name));
    let on = p::bool(v, "value").unwrap_or(!has);
    t.categories.retain(|x| !x.eq_ignore_ascii_case(&name));
    if on {
        t.categories.push(name);
    }
    Ok(json!({"id": id, "categories": t.categories}))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let mid = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let due = p::date(v, "due")?;
    let m = s.mailbox.message(mid).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    // One to-do per message.
    if let Some(existing) = s.mailbox.tasks.values().find(|t| t.message == Some(mid)).map(|t| t.id) {
        s.sel.task = Some(existing);
        return Ok(json!({"id": existing, "existing": true}));
    }
    let task = Task {
        title: m.topic(),
        notes: format!("From {}: {}", m.from.display(), crate::list::preview(m, 300)),
        due,
        message: Some(mid),
        importance: m.importance,
        ..Default::default()
    };
    let id = s.mailbox.add_task(task);
    let m = s.mailbox.message_mut(mid)?;
    if m.flag == mailcraft_model::FlagStatus::None {
        m.flag = mailcraft_model::FlagStatus::Flagged;
    }
    s.sel.task = Some(id);
    s.view.module = Module::Tasks;
    Ok(json!({"id": id, "existing": false}))
}
