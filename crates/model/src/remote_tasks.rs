// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! To-dos that a Microsoft account's server keeps (Microsoft To Do), as plain data like
//! [`crate::remote`]: a host with sockets brings a [`TasksDownload`] and takes a [`TasksUpload`]
//! to the server, and the engine merges the one and works out the other
//! (`sync.applyTasks`, `sync.taskChanges`, `sync.applyTaskChanges`).
//!
//! All of it is hostile: it comes from a server, or from a command's parameters.
//! [`RemoteTask::tidy`] cuts a task to size before any of it is kept.

use mailcraft_time::{Date, Timestamp};
use serde::{Deserialize, Serialize};

use crate::remote::{key, line, text};
use crate::{Account, Id, Importance, MAX_NAME, TaskPatch};

/// Most lists, most to-dos in one list and most to-dos in all of an account's lists that one
/// download is taken to hold; what is beyond is left out and the download is not complete.
pub const MAX_TASK_LISTS: usize = 50;
pub const MAX_TASKS_PER_LIST: usize = 2_000;
pub const MAX_TASKS: usize = 10_000;
/// Most changes one [`TasksUpload`] carries, and most to-dos in it that are made (the rest go
/// with the next).
pub const MAX_TASK_OPS: usize = 100;
pub const MAX_TASK_CREATES: usize = 50;
const MAX_LINE: usize = 500;
const MAX_NOTES: usize = 100_000;
const MAX_CATEGORIES: usize = 25;
/// Longest text kept of why a server didn't take a change.
const MAX_WHY: usize = 300;

/// A to-do as a server has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteTask {
    /// What the server calls it; a to-do without one is not merged.
    pub id: String,
    /// What it calls the list the to-do is in.
    pub list: String,
    /// The server's mark of this version of it.
    pub change_key: String,
    pub title: String,
    pub notes: String,
    pub start: Option<Date>,
    pub due: Option<Date>,
    pub completed: Option<Timestamp>,
    pub importance: Importance,
    pub categories: Vec<String>,
    pub reminder: Option<Timestamp>,
}

impl RemoteTask {
    /// Cut everything to the size the mailbox keeps.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.list = key(&self.list);
        self.change_key = key(&self.change_key);
        self.title = line(&self.title, MAX_LINE);
        self.notes = text(&self.notes, MAX_NOTES);
        self.start = self.start.and_then(|d| Date::new(d.year, d.month, d.day));
        self.due = self.due.and_then(|d| Date::new(d.year, d.month, d.day));
        self.categories = std::mem::take(&mut self.categories).into_iter().map(|name| line(&name, MAX_NAME)).filter(|name| !name.is_empty()).fold(
            Vec::new(),
            |mut kept: Vec<String>, name| {
                if kept.len() < MAX_CATEGORIES && !kept.iter().any(|have| have.eq_ignore_ascii_case(&name)) {
                    kept.push(name);
                }
                kept
            },
        );
    }
}

/// A list of to-dos as a server has it, and whether all of its to-dos are in the download.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteTaskList {
    pub id: String,
    pub complete: bool,
    /// Shared with other people (Graph's `isShared`).
    pub shared: bool,
}

impl RemoteTaskList {
    /// Cut to the size the mailbox keeps.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
    }
}

/// An account's to-dos, as its server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasksDownload {
    /// The account whose to-dos they are ([`TasksDownload::for_account`]).
    pub account: Id,
    /// That account's address (a download is not taken in by another account that happens to
    /// have the id).
    pub email: String,
    /// Every list the server has is named in `lists`. When not, nothing is removed for not being
    /// here.
    pub complete: bool,
    pub lists: Vec<RemoteTaskList>,
    pub tasks: Vec<RemoteTask>,
}

impl TasksDownload {
    pub fn for_account(mut self, account: &Account) -> TasksDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// What is done to a to-do on a server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskOpKind {
    /// Make it, in the account's default list.
    #[default]
    Create,
    /// Change the fields of the patch.
    Update,
    Delete,
}

/// One change to tell a server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskOp {
    pub kind: TaskOpKind,
    /// The to-do here; 0 for a deletion of one that is gone from here.
    pub task: Id,
    /// What the server calls it and its list (a creation has neither).
    pub id: String,
    pub list: String,
    pub patch: TaskPatch,
}

/// What a host is to tell an account's server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasksUpload {
    pub account: Id,
    pub email: String,
    pub ops: Vec<TaskOp>,
}

impl TasksUpload {
    pub fn for_account(mut self, account: &Account) -> TasksUpload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// What a server did with one change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskResult {
    /// Taken.
    Done,
    /// The server no longer has the to-do.
    Gone,
    /// The account may not change its to-dos (the owner agreed to reading only, or not at all).
    Denied,
    /// Not now: try again later.
    #[default]
    Failed,
    /// Refused for good.
    Refused,
}

/// What a server said of one change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskOutcome {
    pub kind: TaskOpKind,
    pub task: Id,
    /// The id it has, and its list and change key, as the server says (a creation).
    pub id: String,
    pub list: String,
    pub change_key: String,
    pub result: TaskResult,
    /// What the change said, as it was sent: what the server has of the to-do now, when `result`
    /// is [`TaskResult::Done`].
    pub patch: TaskPatch,
    /// The status of the answer; 0 when there was none.
    pub status: u16,
    /// Why not, in a few words (never what was sent).
    pub error: String,
}

impl TaskOutcome {
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.list = key(&self.list);
        self.change_key = key(&self.change_key);
        self.error = line(&self.error, MAX_WHY);
    }
}

/// What a server did with a [`TasksUpload`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasksUploaded {
    pub account: Id,
    pub email: String,
    pub outcomes: Vec<TaskOutcome>,
    /// The server stopped taking the sign-in part-way.
    pub token_refused: bool,
}

impl TasksUploaded {
    pub fn for_account(mut self, account: &Account) -> TasksUploaded {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}
