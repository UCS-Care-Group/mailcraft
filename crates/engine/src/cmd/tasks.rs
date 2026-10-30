// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks: to-do items.

use mailcraft_model::{AccountChoice, Id, Importance, Mailbox, Message, Origin, Task, clean_name};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::tasks_remote::{keeps_tasks, task_accounts, tasks_home};
use super::{bad, done, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Module, Session, TaskFilter, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tasks.new", "New Task", "Home › New", new).key("Mod+Shift+K").params(TASK),
        CommandSpec::new("tasks.edit", "Edit Task", "Task", edit)
            .params(r#"{"id"?: number, …the fields of tasks.new but `account`: a task stays in its account}"#)
            .selection(needs_task),
        CommandSpec::new("tasks.delete", "Delete", "Home › Delete", delete).params(ONE).selection(needs_task),
        CommandSpec::new("tasks.complete", "Mark Complete", "Home › Manage Task", complete)
            .params(r#"{"value"?: bool (default: toggle), "id"?: number}"#)
            .selection(needs_task),
        CommandSpec::new("tasks.select", "Select Task", "Tasks", select).pure().params(r#"{"id": number | null}"#),
        CommandSpec::new("tasks.get", "Get Task", "Tasks", get).pure().params(ONE),
        CommandSpec::new("tasks.arrange", "Arrange Tasks By", "View › Arrangement", arrange).pure().params(r#"{"by": "dueDate"|"categories"}"#),
        CommandSpec::new("tasks.list", "List Tasks", "Tasks", list).pure().params(
            r#"{"filter"?: "active"|"today"|"overdue"|"completed"|"all", "account"?: "all" | "here" | number} (default: what the list shows)"#,
        ),
        CommandSpec::new("tasks.categorize", "Categorize", "Home › Tags", categorize)
            .params(r#"{"category": string, "value"?: bool, "id"?: number}"#)
            .selection(needs_task),
        CommandSpec::new("tasks.fromMessage", "Add to Tasks", "Home › Tags", from_message)
            .params(r#"{"id"?: number, "due"?: "YYYY-MM-DD"} (a message; default: the selected one)"#)
            .selection(needs_message),
    ]
}

const ONE: &str = r#"{"id"?: number} (default: the selected task)"#;
const TASK: &str = r#"{"title": string, "notes"?: string, "due"?: "YYYY-MM-DD" | null, "start"?: "YYYY-MM-DD" | null, "importance"?: "low"|"normal"|"high", "categories"?: [string], "reminder"?: "YYYY-MM-DDTHH:MM" | null, "account"?: number} — `account`: the account whose To Do keeps it, 0 for this computer (default: whose tasks the list shows; with all of them, where the last one was put)"#;

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

/// Whose tasks the list shows: what the owner chose, while it means something. With no account
/// that keeps tasks there is nothing to choose, and an account that no longer does is All accounts.
pub fn shown_account(s: &Session) -> AccountChoice {
    match s.view.task_account {
        AccountChoice::Account(account) if keeps_tasks(&s.mailbox, account) => s.view.task_account,
        AccountChoice::Here if !task_accounts(&s.mailbox).is_empty() => AccountChoice::Here,
        _ => AccountChoice::All,
    }
}

/// Where a new task goes when nothing else says: where the owner last put one, else the account
/// they sent their local tasks to. `None` when they have never said: it stays here, and the
/// question about local tasks offers it like any other.
fn last_place(mailbox: &Mailbox) -> Option<Id> {
    if task_accounts(mailbox).is_empty() {
        return None;
    }
    mailbox.task_place.filter(|place| *place == 0 || keeps_tasks(mailbox, *place)).or_else(|| tasks_home(mailbox))
}

/// Where a new task goes unless told otherwise: an account, or 0 for this computer.
pub fn new_task_place(s: &Session) -> Option<Id> {
    match shown_account(s) {
        AccountChoice::Account(account) => Some(account),
        AccountChoice::Here => Some(0),
        AccountChoice::All => last_place(&s.mailbox),
    }
}

/// Where `tasks.new` puts a task: where `v` says, else [`new_task_place`].
fn place_of_new(s: &Session, v: &Value) -> Result<Option<Id>, CmdError> {
    let Some(account) = v.get("account").filter(|a| !a.is_null()) else { return Ok(new_task_place(s)) };
    let account = account.as_u64().ok_or_else(|| bad("`account` is an account's id, or 0 for this computer"))?;
    if account != 0 && !keeps_tasks(&s.mailbox, account) {
        return Err(bad("that account doesn't keep its to-dos with Microsoft"));
    }
    Ok(Some(account))
}

/// Where a to-do made from a message goes: that message's account when it keeps to-dos, else
/// wherever the owner last put one.
fn place_of_message(mailbox: &Mailbox, message: &Message) -> Option<Id> {
    if keeps_tasks(mailbox, message.account) { Some(message.account) } else { last_place(mailbox) }
}

/// Where the task `id` would make with `v` goes, for the agent gate: `None` when it makes none or
/// nobody has said.
pub fn place_of(s: &Session, id: &str, v: &Value) -> Option<Id> {
    match id {
        "tasks.new" if v.get("title").is_some_and(|title| !title.is_null()) => place_of_new(s, v).ok().flatten(),
        "tasks.fromMessage" => {
            let message = p::id(v, "id").or(s.sel.focus()).and_then(|m| s.mailbox.message(m))?;
            let made = s.mailbox.tasks.values().any(|t| t.message == Some(message.id));
            if made { None } else { place_of_message(&s.mailbox, message) }
        }
        _ => None,
    }
}

fn put(t: &mut Task, place: Id) {
    t.origin.account = place;
    t.kept_here = place == 0;
}

/// A task just made or picked is in view: where the list doesn't show it, the list moves to it.
fn reveal(s: &mut Session, origin: &Origin) {
    if !shown_account(s).shows(origin) {
        s.view.task_account = AccountChoice::of(origin);
    }
}

/// Where a task lives, as an answer says it.
fn kept_at(mailbox: &Mailbox, origin: &Origin) -> String {
    mailbox.account(origin.account).map_or_else(|| "this computer".to_string(), |a| a.email.trim().to_string())
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let mut t = Task::default();
    apply(s, &mut t, v)?;
    if t.title.is_empty() {
        return Err(bad("`title` is required"));
    }
    if let Some(place) = place_of_new(s, v)? {
        put(&mut t, place);
        s.mailbox.task_place = Some(place);
    }
    reveal(s, &t.origin);
    let (account, kept_at) = (t.origin.account, kept_at(&s.mailbox, &t.origin));
    let id = s.mailbox.add_task(t);
    s.sel.task = Some(id);
    Ok(json!({"id": id, "account": account, "keptAt": kept_at}))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    if v.get("account").is_some() {
        return Err(bad("a task stays in its account"));
    }
    let mut t = s.mailbox.task(id).cloned().ok_or_else(|| fail("the task is gone"))?;
    apply(s, &mut t, v)?;
    if t.title.is_empty() {
        return Err(fail("a task needs a title"));
    }
    // Changed again: a change the server refused is tried again.
    t.local_only = false;
    *s.mailbox.task_mut(id)? = t;
    Ok(json!({"id": id}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    if s.mailbox.task(id).is_some_and(|t| t.origin.is_remote() && !t.origin.id.is_empty())
        && s.mailbox.task_gone.len() >= mailcraft_model::MAX_TASK_GONE
    {
        return Err(fail("too many deletions are waiting to be sent to Microsoft To Do; wait for them to go first"));
    }
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
    t.local_only = false;
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
        "reminder": t.reminder.map(|r| s.local(r).iso()), "account": t.origin.account, "keptAt": kept_at(&s.mailbox, &t.origin),
    })
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.task(id).map(|t| task_json(s, t)).ok_or_else(|| fail("the task is gone"))
}

/// Tasks as the list shows them: open ones by due date (undated last), then finished ones,
/// most recently finished first.
pub fn sorted(s: &Session, filter: TaskFilter) -> Vec<Id> {
    sorted_of(s, filter, shown_account(s))
}

fn sorted_of(s: &Session, filter: TaskFilter, account: AccountChoice) -> Vec<Id> {
    let today = s.today();
    let mut v: Vec<&Task> = s
        .mailbox
        .tasks
        .values()
        .map(|t| t.as_ref())
        .filter(|t| account.shows(&t.origin))
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
    let account = match v.get("account").filter(|a| !a.is_null()) {
        Some(a) => AccountChoice::parse(a).ok_or_else(|| bad("`account` is all, here or an account's id"))?,
        None => shown_account(s),
    };
    let items: Vec<Value> = sorted_of(s, filter, account).into_iter().filter_map(|id| s.mailbox.task(id)).map(|t| task_json(s, t)).collect();
    Ok(json!({"filter": filter, "account": account.code(), "tasks": items}))
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
    if let Some(existing) = s.mailbox.tasks.values().find(|t| t.message == Some(mid)).cloned() {
        s.sel.task = Some(existing.id);
        reveal(s, &existing.origin);
        return Ok(json!({"id": existing.id, "existing": true}));
    }
    let place = place_of_message(&s.mailbox, m);
    let mut task = Task {
        title: m.topic(),
        notes: format!("From {}: {}", m.from.display(), crate::list::preview(m, 300)),
        due,
        message: Some(mid),
        importance: m.importance,
        ..Default::default()
    };
    if let Some(place) = place {
        put(&mut task, place);
    }
    reveal(s, &task.origin);
    let id = s.mailbox.add_task(task);
    let m = s.mailbox.message_mut(mid)?;
    if m.flag == mailcraft_model::FlagStatus::None {
        m.flag = mailcraft_model::FlagStatus::Flagged;
    }
    s.sel.task = Some(id);
    s.view.module = Module::Tasks;
    Ok(json!({"id": id, "existing": false}))
}
