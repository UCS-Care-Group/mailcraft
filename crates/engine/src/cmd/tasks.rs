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
        CommandSpec::new("tasks.recurrence", "Recurrence", "Task › Recurrence", recurrence)
            .params(r#"{"recurrence": {"freq": "daily"|"weekly"|"monthly"|"yearly", "interval"?: n, "byDay"?: ["mon",…], "nth"?: 1-5|-1, "count"?: n, "until"?: "YYYY-MM-DD"} | null, "id"?: number} — the task comes back: marking it complete makes the next one, due on the next day of the rule after its due date (a task without one is due today first); null stops it"#)
            .selection(needs_task),
        CommandSpec::new("tasks.select", "Select Task", "Tasks", select).pure().params(r#"{"id": number | null}"#),
        CommandSpec::new("tasks.get", "Get Task", "Tasks", get).pure().params(ONE),
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
const TASK: &str = r#"{"title": string, "notes"?: string, "due"?: "YYYY-MM-DD" | null, "start"?: "YYYY-MM-DD" | null, "importance"?: "low"|"normal"|"high", "categories"?: [string], "reminder"?: "YYYY-MM-DDTHH:MM" | null, "recurrence"?: rule | null (as tasks.recurrence)}"#;

fn needs_task(s: &Session) -> Option<&'static str> {
    if s.sel.task.is_none() { Some("select a task first") } else { None }
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").or(s.sel.task).ok_or_else(|| fail("select a task first"))?;
    s.mailbox.task(id).map(|t| t.id).ok_or_else(|| fail(format!("no task with id {id}")))
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
        t.reminder = p::datetime(v, "reminder")?.map(|at| s.zone().instant(at));
    }
    match v.get("recurrence") {
        None => {}
        Some(Value::Null) => t.recurrence = None,
        Some(r) if r.is_object() => {
            t.recurrence = Some(super::calendar::parse_recurrence(r)?);
            if t.due.is_none() {
                t.due = Some(t.start.unwrap_or(s.today()));
            }
        }
        Some(_) => return Err(bad("`recurrence` is a rule, or null")),
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

fn recurrence(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let rule = match v.get("recurrence") {
        None => return Err(bad("`recurrence` (a rule, or null) is required")),
        Some(Value::Null) => None,
        Some(r) if r.is_object() => Some(super::calendar::parse_recurrence(r)?),
        Some(_) => return Err(bad("`recurrence` is a rule, or null")),
    };
    let today = s.today();
    let t = s.mailbox.task_mut(id)?;
    // A task that repeats is due somewhere: on the first day of its rule.
    if rule.is_some() && t.due.is_none() {
        t.due = Some(t.start.unwrap_or(today));
    }
    t.recurrence = rule;
    let text = t.recurrence.as_ref().map(|r| r.describe(t.due.unwrap_or(today)));
    Ok(json!({"id": id, "recurrence": text, "due": t.due.map(Date::iso)}))
}

/// The next task of a series, when `t` repeats and the series goes on: the same task, due on the
/// rule's next day (its start as far before as it was), one occurrence fewer to go.
fn next_of(t: &Task, today: Date) -> Option<Task> {
    let rule = t.recurrence.as_ref()?;
    if rule.count.is_some_and(|c| c <= 1) {
        return None;
    }
    let due = t.next_due(today)?;
    let lead = match (t.start, t.due) {
        (Some(start), Some(old)) => Some(start.days_until(old).max(0)),
        _ => None,
    };
    let mut next_rule = rule.clone();
    next_rule.count = rule.count.map(|c| c.saturating_sub(1));
    Some(Task {
        id: 0,
        due: Some(due),
        start: lead.map(|days| due.add_days(-days)),
        completed: None,
        reminder: None,
        message: None,
        uid: String::new(),
        recurrence: Some(next_rule),
        ..t.clone()
    })
}

fn complete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let now = s.now();
    let today = s.today();
    let t = s.mailbox.task_mut(id)?;
    let done = p::bool(v, "value").unwrap_or(!t.is_complete());
    let finishing = done && !t.is_complete();
    t.completed = if done { Some(t.completed.unwrap_or(now)) } else { None };
    // A task that repeats: this one is done for good, and the next one takes the series on.
    let next = if finishing { next_of(t, today) } else { None };
    if finishing && t.recurrence.is_some() {
        t.recurrence = None;
    }
    let message = t.message;
    // A to-do made from a flagged message keeps that flag in step.
    if let Some(m) = message
        && let Ok(m) = s.mailbox.message_mut(m)
    {
        m.flag = if done { mailcraft_model::FlagStatus::Complete } else { mailcraft_model::FlagStatus::Flagged };
    }
    let next = next.map(|t| s.mailbox.add_task(t));
    if next.is_some() && s.sel.task == Some(id) {
        s.sel.task = next;
    }
    Ok(json!({"id": id, "complete": done, "next": next}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    s.sel.task = match p::id(v, "id") {
        Some(id) => Some(s.mailbox.task(id).map(|t| t.id).ok_or_else(|| fail(format!("no task with id {id}")))?),
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
        "recurrence": t.recurrence.as_ref().map(|r| json!({"rule": r, "text": r.describe(t.due.or(t.start).unwrap_or(s.today()))})),
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
    let m = s.mailbox.message(mid).ok_or_else(|| fail(format!("no message with id {mid}")))?;
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

#[cfg(test)]
mod tests {
    use mailcraft_time::Date;
    use serde_json::json;

    use crate::Session;

    fn session() -> Session {
        // Wednesday 2026-10-14.
        Session::at(mailcraft_model::Mailbox::with_local_account("Me", "me@example.org"), 1_791_993_600, 0)
    }

    fn day(s: &str) -> Date {
        Date::parse_iso(s).unwrap()
    }

    #[test]
    fn completing_a_task_that_repeats_makes_the_next_one() {
        let mut s = session();
        let id =
            s.run("tasks.new", &json!({"title": "Water the plants", "start": "2026-10-12", "due": "2026-10-14"})).unwrap()["id"].as_u64().unwrap();
        let set = s.run("tasks.recurrence", &json!({"recurrence": {"freq": "weekly", "byDay": ["wed", "sat"], "count": 3}})).unwrap();
        assert_eq!(set["recurrence"], "Every week on Wednesday, Saturday, 3 times");
        let done = s.run("tasks.complete", &json!({"id": id})).unwrap();
        let next = done["next"].as_u64().unwrap();
        let (old, new) = (s.mailbox.task(id).unwrap().clone(), s.mailbox.task(next).unwrap().clone());
        assert!(old.is_complete() && old.recurrence.is_none(), "the finished one is done for good");
        assert_eq!((new.due, new.start, new.title.as_str()), (Some(day("2026-10-17")), Some(day("2026-10-15")), "Water the plants"));
        assert_eq!(new.recurrence.as_ref().and_then(|r| r.count), Some(2));
        assert_eq!(s.sel.task, Some(next));
        // Undoing the completion brings back the series as it was.
        s.run("edit.undo", &json!({})).unwrap();
        assert!(s.mailbox.task(next).is_none() && s.mailbox.task(id).unwrap().recurrence.is_some());
        s.run("edit.redo", &json!({})).unwrap();
        // The last one of the series makes no other.
        let third = s.run("tasks.complete", &json!({"id": next})).unwrap()["next"].as_u64().unwrap();
        assert_eq!(s.mailbox.task(third).unwrap().due, Some(day("2026-10-21")));
        assert!(s.run("tasks.complete", &json!({"id": third})).unwrap()["next"].is_null());
        // Reopening a finished task makes nothing.
        assert!(s.run("tasks.complete", &json!({"id": third, "value": false})).unwrap()["next"].is_null());
    }

    #[test]
    fn a_rule_on_a_task_without_dates_starts_today_and_can_stop() {
        let mut s = session();
        let id = s.run("tasks.new", &json!({"title": "Report", "recurrence": {"freq": "monthly"}})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.task(id).unwrap().due, Some(day("2026-10-14")));
        let next = s.run("tasks.complete", &json!({"id": id})).unwrap()["next"].as_u64().unwrap();
        assert_eq!(s.mailbox.task(next).unwrap().due, Some(day("2026-11-14")));
        s.run("tasks.recurrence", &json!({"id": next, "recurrence": null})).unwrap();
        assert!(s.run("tasks.complete", &json!({"id": next})).unwrap()["next"].is_null());
        assert!(s.run("tasks.recurrence", &json!({"id": next, "recurrence": "weekly"})).is_err());
        assert!(s.run("tasks.recurrence", &json!({"id": next})).is_err());
        // An end date before the next day ends the series.
        let id = s
            .run("tasks.new", &json!({"title": "Once more", "due": "2026-10-14", "recurrence": {"freq": "daily", "until": "2026-10-14"}}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        assert!(s.run("tasks.complete", &json!({"id": id})).unwrap()["next"].is_null());
    }

    #[test]
    fn the_todo_bar_shows_the_days_ahead_and_what_is_to_do() {
        let mut s = session();
        s.run("tasks.new", &json!({"title": "Later", "due": "2026-10-20"})).unwrap();
        s.run("tasks.new", &json!({"title": "Sooner", "due": "2026-10-15"})).unwrap();
        let done = s.run("tasks.new", &json!({"title": "Done"})).unwrap()["id"].as_u64().unwrap();
        s.run("tasks.complete", &json!({"id": done})).unwrap();
        s.run("calendar.new", &json!({"title": "Over", "start": "2026-10-14T09:00", "end": "2026-10-14T10:00"})).unwrap();
        s.run("calendar.new", &json!({"title": "Tomorrow", "start": "2026-10-15T09:00", "end": "2026-10-15T10:00"})).unwrap();
        s.run("calendar.new", &json!({"title": "Far", "start": "2026-10-30T09:00", "end": "2026-10-30T10:00"})).unwrap();
        let bar = s.run("view.todoBar", &json!({})).unwrap();
        assert_eq!(bar["value"], true);
        assert!(s.view.todo_bar);
        let titles = |key: &str| bar[key].as_array().unwrap().iter().map(|x| x["title"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(titles("events"), ["Tomorrow"]);
        assert_eq!(titles("tasks"), ["Sooner", "Later"]);
        assert_eq!(s.run("view.todoBar", &json!({"value": false})).unwrap()["value"], false);
    }

    #[test]
    fn tasks_that_repeat_go_to_icalendar_and_back() {
        let mut s = session();
        let id = s.run("tasks.new", &json!({"title": "Backup", "due": "2026-10-16", "recurrence": {"freq": "weekly", "interval": 2}})).unwrap()["id"]
            .as_u64()
            .unwrap();
        let t = s.mailbox.task(id).unwrap().clone();
        let ics = mailcraft_ical::write(&[], &[&t], &mailcraft_ical::WriteOptions::default());
        assert!(ics.contains("DTSTART;VALUE=DATE:20261016\r\n") && ics.contains("RRULE:FREQ=WEEKLY;INTERVAL=2"), "{ics}");
        let back = mailcraft_ical::parse(&ics, &|_| 0);
        assert_eq!(back.tasks[0].recurrence, t.recurrence);
    }
}
