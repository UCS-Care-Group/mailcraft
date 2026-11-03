// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send Status Report: a new message saying how a task stands, for the owner to address and
//! send. Nothing is sent from here.

use mailcraft_i18n::Lang;
use mailcraft_model::{Importance, Task};
use mailcraft_time::Date;
use serde_json::{Value, json};

use crate::cmd::fail;
use crate::{CmdError, CmdResult, Session, p};

/// Most lines of a task's notes a report carries.
const MAX_NOTE_LINES: usize = 200;

fn day_said(lang: Lang, d: Date) -> String {
    let day = mailcraft_i18n::date::Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
    mailcraft_i18n::date::long(&lang, day)
}

/// The task a report is of: `id`, read strictly, else the selected one.
fn named(s: &Session, v: &Value) -> Result<Task, CmdError> {
    let id = p::id_strict(v, "id")?.or(s.sel.task).ok_or_else(|| fail("select a task first"))?;
    s.mailbox.task(id).cloned().ok_or_else(|| failed!("no task with id {id}", id))
}

/// How the task stands, in the interface language: Completed, In Progress (it has started) or
/// Not Started.
fn status(s: &Session, t: &Task) -> String {
    let lang = s.interface_language;
    match t.completed {
        Some(at) => format!("{} ({})", lang.t("Completed"), day_said(lang, s.local(at).date)),
        None if t.start.is_some_and(|d| d <= s.today()) => lang.t("In Progress").to_string(),
        None => lang.t("Not Started").to_string(),
    }
}

/// The report's text: one line per thing known of the task, then its notes.
pub fn text(s: &Session, t: &Task) -> String {
    let lang = s.interface_language;
    let one = |text: &str| mailcraft_names::shown(text, 300);
    let mut out = String::new();
    out.push_str(&format!("{} {}\n", lang.t("Task:"), one(&t.title)));
    out.push_str(&format!("{} {}\n", lang.t("Status:"), status(s, t)));
    let none = lang.t("None");
    let start = t.start.map_or_else(|| none.to_string(), |d| day_said(lang, d));
    out.push_str(&format!("{} {start}\n", lang.t("Start date:")));
    let due = match t.due {
        Some(d) if t.is_overdue(s.today()) => format!("{} ({})", day_said(lang, d), lang.t("overdue")),
        Some(d) => day_said(lang, d),
        None => none.to_string(),
    };
    out.push_str(&format!("{} {due}\n", lang.t("Due date:")));
    let importance = match t.importance {
        Importance::High => lang.t("High"),
        Importance::Normal => lang.t("Normal"),
        Importance::Low => lang.t("Low"),
    };
    out.push_str(&format!("{} {importance}\n", lang.t("Importance:")));
    if !t.categories.is_empty() {
        let cats: Vec<String> = t.categories.iter().take(50).map(|c| one(c)).collect();
        out.push_str(&format!("{} {}\n", lang.t("Categories:"), cats.join(", ")));
    }
    let notes: Vec<&str> = t.notes.lines().take(MAX_NOTE_LINES).collect();
    if notes.iter().any(|l| !l.trim().is_empty()) {
        out.push('\n');
        for line in notes {
            out.push_str(&mailcraft_names::shown(line, 2_000));
            out.push('\n');
        }
    }
    out
}

pub(super) fn report(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "discard"])?;
    let t = named(s, v)?;
    let lang = s.interface_language;
    let subject = format!("{} {}", lang.t("Task Status Report:"), mailcraft_names::shown(&t.title, 200));
    let body = text(s, &t);
    let compose = s.run("mail.new", &json!({"subject": subject, "body": body}))?["compose"].clone();
    Ok(json!({"composing": true, "task": t.id, "compose": compose}))
}
