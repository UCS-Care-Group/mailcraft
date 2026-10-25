// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! To-do items.

use mailcraft_time::{Date, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{Id, Importance, Origin};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Task {
    pub id: Id,
    pub title: String,
    pub notes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<Date>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Date>,
    /// When it was marked complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<Timestamp>,
    pub importance: Importance,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<Timestamp>,
    /// The flagged message this to-do stands for, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Id>,
    /// iCalendar `UID` (VTODO).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
    /// Set for a to-do that an account's server keeps (Microsoft To Do): what the server calls it.
    /// An account with no id yet is waiting to be made there.
    #[serde(skip_serializing_if = "Origin::is_local")]
    pub origin: Origin,
    /// What the server calls the list the to-do is in; empty for one that was never there.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub list: String,
    /// What the server had when the to-do was last in step with it: a change made here since
    /// is what is left to tell it ([`Task::patch`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced: Option<TaskFields>,
    /// The server refused to make this to-do: it stays on this computer.
    #[serde(skip_serializing_if = "is_false")]
    pub local_only: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What of a to-do a server keeps and this program changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskFields {
    pub title: String,
    pub notes: String,
    pub start: Option<Date>,
    pub due: Option<Date>,
    pub completed: Option<Timestamp>,
    pub importance: Importance,
    pub categories: Vec<String>,
    pub reminder: Option<Timestamp>,
}

/// The fields of a to-do that differ from what the server has; only those are sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<Option<Date>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Option<Date>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<Option<Timestamp>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importance: Option<Importance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder: Option<Option<Timestamp>>,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        *self == TaskPatch::default()
    }
}

impl TaskFields {
    /// What of `self` differs from `base`.
    pub fn since(&self, base: &TaskFields) -> TaskPatch {
        fn diff<T: Clone + PartialEq>(now: &T, then: &T) -> Option<T> {
            (now != then).then(|| now.clone())
        }
        TaskPatch {
            title: diff(&self.title, &base.title),
            notes: diff(&self.notes, &base.notes),
            start: diff(&self.start, &base.start),
            due: diff(&self.due, &base.due),
            completed: diff(&self.completed, &base.completed),
            importance: diff(&self.importance, &base.importance),
            categories: diff(&self.categories, &base.categories),
            reminder: diff(&self.reminder, &base.reminder),
        }
    }

    /// Every field, as a patch to a to-do that doesn't exist yet.
    pub fn whole(&self) -> TaskPatch {
        TaskPatch::default().with_all(self)
    }

    /// `self` after `patch` was applied.
    pub fn after(mut self, patch: &TaskPatch) -> TaskFields {
        if let Some(v) = &patch.title {
            self.title = v.clone();
        }
        if let Some(v) = &patch.notes {
            self.notes = v.clone();
        }
        if let Some(v) = patch.start {
            self.start = v;
        }
        if let Some(v) = patch.due {
            self.due = v;
        }
        if let Some(v) = patch.completed {
            self.completed = v;
        }
        if let Some(v) = patch.importance {
            self.importance = v;
        }
        if let Some(v) = &patch.categories {
            self.categories = v.clone();
        }
        if let Some(v) = patch.reminder {
            self.reminder = v;
        }
        self
    }

    /// Per field: what `here` has when it was changed since `base` (this is `base`), else what
    /// `there` has. The three-way merge a download makes with a to-do that was changed here.
    pub fn merged(&self, here: &TaskFields, there: &TaskFields) -> TaskFields {
        let mine = here.since(self);
        there.clone().after(&mine)
    }
}

impl TaskPatch {
    fn with_all(mut self, f: &TaskFields) -> TaskPatch {
        self.title = Some(f.title.clone());
        self.notes = Some(f.notes.clone());
        self.start = Some(f.start);
        self.due = Some(f.due);
        self.completed = Some(f.completed);
        self.importance = Some(f.importance);
        self.categories = Some(f.categories.clone());
        self.reminder = Some(f.reminder);
        self
    }
}

/// A to-do that was deleted here and that its server is yet to be told about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskGone {
    pub account: Id,
    /// What the server calls it.
    pub id: String,
    /// And the list it is in.
    pub list: String,
}

/// What the owner said about the to-dos that exist only in this mailbox when an account that
/// keeps to-dos (Microsoft To Do) comes along. Nothing local goes to a server before the owner
/// has said yes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalTasks {
    /// Not asked, or not answered: they stay on this computer.
    #[default]
    Unasked,
    /// "Not now": they stay on this computer, and the owner can offer them again.
    NotNow,
    /// Sent to that account's default list; so is every to-do made here from now on.
    SendTo(Id),
}

impl LocalTasks {
    pub fn is_unasked(&self) -> bool {
        *self == LocalTasks::Unasked
    }
}

/// Most deletions that wait for a server at once; the rest are forgotten.
pub const MAX_TASK_GONE: usize = 5_000;

impl Task {
    /// What of the to-do a server keeps.
    pub fn fields(&self) -> TaskFields {
        TaskFields {
            title: self.title.clone(),
            notes: self.notes.clone(),
            start: self.start,
            due: self.due,
            completed: self.completed,
            importance: self.importance,
            categories: self.categories.clone(),
            reminder: self.reminder,
        }
    }

    /// Put `fields` in the to-do.
    pub fn set_fields(&mut self, fields: TaskFields) {
        self.title = fields.title;
        self.notes = fields.notes;
        self.start = fields.start;
        self.due = fields.due;
        self.completed = fields.completed;
        self.importance = fields.importance;
        self.categories = fields.categories;
        self.reminder = fields.reminder;
    }

    /// What was changed here since the server had it; `None` for a to-do the server doesn't have.
    pub fn patch(&self) -> Option<TaskPatch> {
        let base = self.synced.as_ref()?;
        Some(self.fields().since(base)).filter(|patch| !patch.is_empty())
    }

    pub fn is_complete(&self) -> bool {
        self.completed.is_some()
    }

    /// Due before `today` and not done.
    pub fn is_overdue(&self, today: Date) -> bool {
        !self.is_complete() && self.due.is_some_and(|d| d < today)
    }

    /// `Today`, `Tomorrow`, `Overdue`, the weekday within a week, else the date; `No date`.
    pub fn due_label(&self, today: Date) -> String {
        let Some(due) = self.due else { return "No date".into() };
        match today.days_until(due) {
            0 => "Today".into(),
            1 => "Tomorrow".into(),
            -1 => "Yesterday".into(),
            n if (2..7).contains(&n) => due.weekday().name().into(),
            _ => due.medium(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_labels_and_overdue() {
        let today = Date::new(2026, 10, 14).unwrap();
        let task = |due: Option<Date>| Task { due, ..Default::default() };
        assert_eq!(task(None).due_label(today), "No date");
        assert_eq!(task(Some(today)).due_label(today), "Today");
        assert_eq!(task(Some(today.add_days(1))).due_label(today), "Tomorrow");
        assert_eq!(task(Some(today.add_days(3))).due_label(today), "Saturday");
        assert_eq!(task(Some(today.add_days(30))).due_label(today), "Nov 13, 2026");
        let mut late = task(Some(today.add_days(-2)));
        assert!(late.is_overdue(today));
        late.completed = Some(1);
        assert!(!late.is_overdue(today) && late.is_complete());
    }
}
