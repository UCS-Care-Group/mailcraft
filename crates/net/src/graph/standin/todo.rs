// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's Microsoft To Do and Outlook categories, as Graph v1.0 documents them ("todoTaskList",
//! "todoTask", "outlookCategory"):
//!
//! - `GET|POST /v1.0/me/todo/lists`, `GET|PATCH|DELETE /me/todo/lists/{id}` (a well-known list
//!   can't be renamed or deleted: 400), `GET|POST /me/todo/lists/{id}/tasks` and
//!   `GET|PATCH|DELETE /me/todo/lists/{id}/tasks/{id}`. Answers are 201 for what was made, 200 for
//!   a change, 204 for a deletion; lists come in pages with `@odata.nextLink`.
//! - A task says only the properties a `todoTask` has, each of its type (`title` a string,
//!   `status` one of the five, `importance` one of three, `dueDateTime` and the other days
//!   `dateTimeTimeZone`s, `categories` strings); anything else is 400. A task that is completed
//!   gets its `completedDateTime`, and one that isn't loses it.
//! - `GET|POST /me/outlook/masterCategories`, `GET|PATCH|DELETE …/{id}`: a name once (409), a colour
//!   one of `none` or `preset0`…`preset24`, and only the colour may be changed.

use serde_json::{Map, Value, json};

use super::{Answer, Asked, Refusal, State, bad, not_found, refuse, stamp};

const STATUSES: [&str; 5] = ["notStarted", "inProgress", "completed", "waitingOnOthers", "deferred"];
const IMPORTANCES: [&str; 3] = ["low", "normal", "high"];
const TASK_PROPERTIES: [&str; 11] = [
    "title",
    "body",
    "status",
    "importance",
    "categories",
    "dueDateTime",
    "startDateTime",
    "completedDateTime",
    "reminderDateTime",
    "isReminderOn",
    "recurrence",
];
/// Longest title To Do keeps, and most tasks and lists the stand-in holds.
const MAX_TITLE: usize = 255;
const MAX_KEPT: usize = 10_000;

/// A list of to-dos the stand-in has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodoList {
    pub id: String,
    pub name: String,
    /// `defaultList`, `flaggedEmails`, or `none`.
    pub well_known: String,
    pub shared: bool,
    pub tasks: Vec<TodoTask>,
}

/// A to-do the stand-in has. The days and instants are `dateTimeTimeZone` objects as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodoTask {
    pub id: String,
    pub title: String,
    pub body: String,
    pub html: bool,
    /// One of `notStarted`, `inProgress`, `completed`, `waitingOnOthers`, `deferred`.
    pub status: String,
    /// `low`, `normal` or `high`.
    pub importance: String,
    pub categories: Vec<String>,
    pub due: Option<Value>,
    pub start: Option<Value>,
    pub completed: Option<Value>,
    pub reminder: Option<Value>,
    pub reminder_on: bool,
    /// How many times it was written, for its `@odata.etag`.
    pub version: u64,
}

/// A colour category of the owner's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub color: String,
}

impl TodoTask {
    fn json(&self, stamped: &str) -> Value {
        let mut task = json!({
            "@odata.etag": format!("W/\"standin-{}\"", self.version),
            "id": self.id,
            "title": self.title,
            "status": self.status,
            "importance": self.importance,
            "isReminderOn": self.reminder_on,
            "hasAttachments": false,
            "categories": self.categories,
            "createdDateTime": stamped,
            "lastModifiedDateTime": stamped,
            "body": {"content": self.body, "contentType": if self.html { "html" } else { "text" }},
        });
        for (name, value) in
            [("dueDateTime", &self.due), ("startDateTime", &self.start), ("completedDateTime", &self.completed), ("reminderDateTime", &self.reminder)]
        {
            if let Some(value) = value {
                task[name] = value.clone();
            }
        }
        task
    }
}

fn no_way() -> Refusal {
    refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")
}

fn name_of(body: &Value) -> Result<String, Refusal> {
    match body.get("displayName") {
        Some(Value::String(name)) if !name.is_empty() && name.chars().count() <= MAX_TITLE => Ok(name.clone()),
        _ => Err(bad("The property 'displayName' is a string of 1 to 255 characters.")),
    }
}

fn color_ok(color: &str) -> bool {
    color == "none" || color.strip_prefix("preset").and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| n <= 24)
}

impl State {
    /// A list of to-dos of the stand-in's making (`well_known` may be empty): its id.
    pub fn add_todo_list(&mut self, name: &str, well_known: &str) -> String {
        self.made += 1;
        let id = format!("AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OAAuAAAAAAAiQ8W967B7TKBjgx9rVEURAQ-todo{:05}AAA=", self.made);
        let well_known = if well_known.is_empty() { "none" } else { well_known };
        self.todo.push(TodoList { id: id.clone(), name: name.into(), well_known: well_known.into(), ..Default::default() });
        id
    }

    /// A to-do in the list `list`, not started: its id; `None` for a list it hasn't got.
    pub fn add_todo(&mut self, list: &str, title: &str) -> Option<String> {
        self.made += 1;
        let id = format!("AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAAAAAiQ8W967B7TKBjgx9rVEURBwB-task{:05}AAA=", self.made);
        let task = TodoTask {
            id: id.clone(),
            title: title.into(),
            status: "notStarted".into(),
            importance: "normal".into(),
            version: 1,
            ..Default::default()
        };
        self.todo.iter_mut().find(|l| l.id == list)?.tasks.push(task);
        Some(id)
    }

    /// The id of the default list of to-dos; empty when there is none.
    pub fn default_todo_list(&self) -> String {
        self.todo.iter().find(|l| l.well_known == "defaultList").map(|l| l.id.clone()).unwrap_or_default()
    }

    /// A colour category: its id.
    pub fn add_category(&mut self, name: &str, color: &str) -> String {
        self.made += 1;
        let id = format!("5d6bd4a1-2c3e-4f50-8a71-{:012x}", self.made);
        self.categories.push(Category { id: id.clone(), name: name.into(), color: color.into() });
        id
    }

    fn list_json(list: &TodoList) -> Value {
        json!({
            "@odata.etag": "W/\"standin-list\"",
            "id": list.id, "displayName": list.name, "isOwner": !list.shared, "isShared": list.shared,
            "wellknownListName": list.well_known,
        })
    }

    /// Everything under `/me/todo`.
    pub(super) fn todo_at(&mut self, asked: &Asked, method: &str, rest: &[&str]) -> Result<Answer, Refusal> {
        let stamped = stamp(self.clock);
        match (method, rest) {
            ("GET", ["lists"]) => {
                let all = self.todo.iter().map(Self::list_json).collect();
                self.page(asked, all)
            }
            ("POST", ["lists"]) => {
                if self.todo.len() >= MAX_KEPT {
                    return Err(bad("Too many lists."));
                }
                let body = asked.json();
                let name = name_of(&body)?;
                if body.as_object().is_some_and(|o| o.keys().any(|k| k != "displayName")) {
                    return Err(bad("A list is made with its displayName alone."));
                }
                self.add_todo_list(&name, "");
                Ok((201, self.todo.last().map(Self::list_json)))
            }
            (_, ["lists", list]) => {
                let at = self.todo.iter().position(|l| l.id == *list).ok_or_else(not_found)?;
                let Some(found) = self.todo.get_mut(at) else { return Err(not_found()) };
                match method {
                    "GET" => Ok((200, Some(Self::list_json(found)))),
                    "PATCH" => {
                        let name = name_of(&asked.json())?;
                        if found.well_known != "none" {
                            return Err(refuse(400, "ErrorInvalidRequest", "A well-known list can't be renamed."));
                        }
                        found.name = name;
                        Ok((200, Some(Self::list_json(found))))
                    }
                    "DELETE" => {
                        if found.well_known != "none" {
                            return Err(refuse(400, "ErrorInvalidRequest", "A well-known list can't be deleted."));
                        }
                        self.todo.remove(at);
                        Ok((204, None))
                    }
                    _ => Err(no_way()),
                }
            }
            ("GET", ["lists", list, "tasks"]) => {
                let found = self.todo.iter().find(|l| l.id == *list).ok_or_else(not_found)?;
                let all = found.tasks.iter().map(|t| t.json(&stamped)).collect();
                self.page(asked, all)
            }
            ("POST", ["lists", list, "tasks"]) => {
                let found = self.todo.iter().find(|l| l.id == *list).ok_or_else(not_found)?;
                if found.tasks.len() >= MAX_KEPT {
                    return Err(bad("Too many tasks."));
                }
                let made = self.made;
                let id = self.add_todo(list, "").ok_or_else(not_found)?;
                match self.write_task(asked, list, &id) {
                    Ok((_, body)) => Ok((201, body)),
                    Err(refusal) => {
                        // Nothing was made: not the task, and not the number it took.
                        if let Some(l) = self.todo.iter_mut().find(|l| l.id == *list) {
                            l.tasks.retain(|t| t.id != id);
                        }
                        self.made = made;
                        Err(refusal)
                    }
                }
            }
            (_, ["lists", list, "tasks", task]) => {
                let found = self.todo.iter().find(|l| l.id == *list).ok_or_else(not_found)?;
                let at = found.tasks.iter().position(|t| t.id == *task).ok_or_else(not_found)?;
                match method {
                    "GET" => Ok((200, found.tasks.get(at).map(|t| t.json(&stamped)))),
                    "PATCH" => self.write_task(asked, list, task),
                    "DELETE" => {
                        if let Some(l) = self.todo.iter_mut().find(|l| l.id == *list) {
                            l.tasks.remove(at);
                        }
                        Ok((204, None))
                    }
                    _ => Err(no_way()),
                }
            }
            (_, ["lists"]) | (_, ["lists", _, "tasks"]) => Err(no_way()),
            _ => Err(refuse(400, "BadRequest", "Resource not found.")),
        }
    }

    /// A task's properties as written, checked and taken in: the whole of it or nothing.
    fn write_task(&mut self, asked: &Asked, list: &str, task: &str) -> Result<Answer, Refusal> {
        let body = asked.json();
        let Some(body) = body.as_object() else { return Err(bad("A task is a JSON object.")) };
        let mut changed =
            self.todo.iter().find(|l| l.id == list).and_then(|l| l.tasks.iter().find(|t| t.id == task)).cloned().ok_or_else(not_found)?;
        for (name, value) in body {
            if name.starts_with("@odata.") {
                continue;
            }
            if !TASK_PROPERTIES.contains(&name.as_str()) {
                return Err(bad(format!("Could not find a property named '{name}' on type 'microsoft.graph.todoTask'.")));
            }
            let wrong = || bad(format!("The property '{name}' has a value of the wrong type."));
            match name.as_str() {
                "title" => match value {
                    Value::String(title) if title.chars().count() <= MAX_TITLE => changed.title = title.clone(),
                    _ => return Err(wrong()),
                },
                "status" => match value.as_str() {
                    Some(status) if STATUSES.contains(&status) => changed.status = status.into(),
                    _ => return Err(wrong()),
                },
                "importance" => match value.as_str() {
                    Some(importance) if IMPORTANCES.contains(&importance) => changed.importance = importance.into(),
                    _ => return Err(wrong()),
                },
                "categories" => match value.as_array() {
                    Some(names) if names.iter().all(Value::is_string) => {
                        changed.categories = names.iter().filter_map(Value::as_str).map(str::to_string).collect();
                    }
                    _ => return Err(wrong()),
                },
                "body" => match (value.get("content").and_then(Value::as_str), value.get("contentType").and_then(Value::as_str)) {
                    (Some(content), Some(kind)) if kind.eq_ignore_ascii_case("text") || kind.eq_ignore_ascii_case("html") => {
                        changed.body = content.into();
                        changed.html = kind.eq_ignore_ascii_case("html");
                    }
                    _ => return Err(wrong()),
                },
                "isReminderOn" => changed.reminder_on = value.as_bool().ok_or_else(wrong)?,
                "dueDateTime" | "startDateTime" | "completedDateTime" | "reminderDateTime" => {
                    let slot = match name.as_str() {
                        "dueDateTime" => &mut changed.due,
                        "startDateTime" => &mut changed.start,
                        "completedDateTime" => &mut changed.completed,
                        _ => &mut changed.reminder,
                    };
                    let (time, zone) = (value.get("dateTime"), value.get("timeZone"));
                    match (value, time, zone) {
                        (Value::Null, ..) => *slot = None,
                        (Value::Object(_), Some(time @ Value::String(text)), Some(zone @ Value::String(name)))
                            if text.len() <= 64
                                && mailcraft_time::parse_iso(text).is_some()
                                && crate::graph::tasks::readable(value)
                                && !name.is_empty()
                                && name.len() <= 64
                                && !name.chars().any(char::is_control) =>
                        {
                            let mut kept = Map::new();
                            kept.insert("dateTime".into(), time.clone());
                            kept.insert("timeZone".into(), zone.clone());
                            *slot = Some(Value::Object(kept));
                        }
                        _ => return Err(wrong()),
                    }
                }
                // A recurrence is taken and not kept.
                _ => {}
            }
        }
        if changed.status == "completed" {
            if changed.completed.is_none() {
                changed.completed = Some(json!({"dateTime": stamp(self.clock), "timeZone": "UTC"}));
            }
        } else {
            changed.completed = None;
        }
        changed.version += 1;
        let json = changed.json(&stamp(self.clock));
        if let Some(slot) = self.todo.iter_mut().find(|l| l.id == list).and_then(|l| l.tasks.iter_mut().find(|t| t.id == task)) {
            *slot = changed;
        }
        Ok((200, Some(json)))
    }

    fn category_json(category: &Category) -> Value {
        json!({"id": category.id, "displayName": category.name, "color": category.color})
    }

    /// `/me/outlook/masterCategories…`.
    pub(super) fn categories_at(&mut self, asked: &Asked, method: &str, rest: &[&str]) -> Result<Answer, Refusal> {
        match (method, rest) {
            ("GET", []) => {
                let all = self.categories.iter().map(Self::category_json).collect();
                self.page(asked, all)
            }
            ("POST", []) => {
                let body = asked.json();
                let name = name_of(&body)?;
                let color = match body.get("color") {
                    None => "none".to_string(),
                    Some(Value::String(color)) => color.clone(),
                    Some(_) => return Err(bad("The property 'color' is 'none' or 'preset0' to 'preset24'.")),
                };
                if !color_ok(&color) {
                    return Err(bad("The property 'color' is 'none' or 'preset0' to 'preset24'."));
                }
                if self.categories.len() >= MAX_KEPT {
                    return Err(bad("Too many categories."));
                }
                if self.categories.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                    return Err(refuse(409, "ErrorNameAlreadyExists", "The specified category already exists."));
                }
                self.add_category(&name, &color);
                Ok((201, self.categories.last().map(Self::category_json)))
            }
            (_, [id]) => {
                let at = self.categories.iter().position(|c| c.id == *id).ok_or_else(not_found)?;
                let Some(found) = self.categories.get_mut(at) else { return Err(not_found()) };
                match method {
                    "GET" => Ok((200, Some(Self::category_json(found)))),
                    "PATCH" => {
                        let body = asked.json();
                        if body.as_object().is_none_or(|o| o.keys().any(|k| k != "color")) {
                            return Err(bad("Only the color of a category may be changed."));
                        }
                        let color = body
                            .get("color")
                            .and_then(Value::as_str)
                            .filter(|c| color_ok(c))
                            .ok_or_else(|| bad("The property 'color' is 'none' or 'preset0' to 'preset24'."))?;
                        found.color = color.into();
                        Ok((200, Some(Self::category_json(found))))
                    }
                    "DELETE" => {
                        self.categories.remove(at);
                        Ok((204, None))
                    }
                    _ => Err(no_way()),
                }
            }
            _ => Err(no_way()),
        }
    }
}
