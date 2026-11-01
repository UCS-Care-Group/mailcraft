// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The to-dos of a Microsoft account, kept in step with Microsoft To Do (#248).
//!
//! The engine does no I/O: a host with sockets (`mailcraft_net::graph::tasks`) brings a
//! [`TasksDownload`] (`sync.applyTasks`, [`merge_tasks`]), asks what is to be told to the server
//! (`sync.taskChanges`, [`task_upload`]) and says what the server made of it
//! (`sync.applyTaskChanges`, [`apply_task_outcomes`]).
//!
//! **What a to-do remembers.** A to-do that the server keeps has its [`Origin`] (account and the
//! server's id), its list, and `synced`: the fields as the server last had them. What was changed
//! here since is the difference between the two ([`Task::patch`]), and only that is sent.
//!
//! **Conflicts.** A download is merged field by field against `synced`: a field that was changed
//! here stays as it is here (and goes up), every other field is the server's. A to-do the server
//! no longer has goes from here too, unless it was changed here since: then it is made again (a
//! change made here is never lost to a deletion elsewhere). A to-do deleted here is remembered
//! ([`mailcraft_model::TaskGone`]) until the server has been told, and is not brought back by a
//! download that still shows it. To-dos made here (no account yet) go up to the first Microsoft
//! account's default list.
//!
//! Everything from outside is hostile: ids and text are cut by [`RemoteTask::tidy`], counts are
//! capped, and a download or an answer for another account is refused whole.

use std::collections::{BTreeSet, HashMap, HashSet};

use mailcraft_model::remote_tasks::{
    MAX_TASK_CREATES, MAX_TASK_LISTS, MAX_TASK_OPS, MAX_TASKS, RemoteTask, RemoteTaskList, TaskOp, TaskOpKind, TaskResult, TasksDownload,
    TasksUpload, TasksUploaded,
};
use mailcraft_model::{AccountKind, Id, LocalTasks, Mailbox, Origin, Task, TaskFields};
use serde_json::{Value, json};

use super::remote::{Merged, owner};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

const MAX_OUTCOMES: usize = 1_000;
const MAX_SHOWN: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tasks.sendLocal", "Send Local Tasks to To Do", "File › Info", send_local_command)
            .params(r#"{"account": number | null} — the owner's answer to "Send your local tasks to <account>'s To Do?": a Microsoft account sends the to-dos that exist only on this computer, and every one made from now on, to its default list at the next Send/Receive of tasks; null ("Not now") keeps them here. Until it is answered nothing local is sent. Answers {"sent": number (how many are to go), "keptHere": number}"#),
        CommandSpec::new("sync.taskChanges", "To-do Changes to Send", "Send / Receive", task_changes_command)
            .pure()
            .params(r#"{"account": number} — what that Microsoft account's server (Microsoft To Do) is to be told of to-dos made, changed or deleted here: {"account", "email", "ops": [{"kind": "create"|"update"|"delete", "task": number, "id"?: string, "list"?: string, "patch": {title?, notes?, start?, due?, completed?, importance?, categories?, reminder?}}]}"#),
        CommandSpec::new("sync.applyTaskChanges", "Apply Sent To-do Changes", "Send / Receive", apply_task_changes_command)
            .pure()
            .params(r#"{"result": TasksUploaded} — what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"kind", "task": number, "id"?: string, "list"?: string, "changeKey"?: string, "result": "done"|"gone"|"denied"|"failed"|"refused", "patch": {…}, "error"?: string}]} — answers {"done", "failed", "denied", "refused", "messages": [string]}"#),
        CommandSpec::new("sync.applyTasks", "Apply Downloaded To-dos", "Send / Receive", apply_tasks_command)
            .pure()
            .params(r#"{"download": TasksDownload} — what a host brought from the account's server: {"account": number, "email"?: string, "complete": bool, "lists": [{"id": string, "complete": bool}], "tasks": [RemoteTask]} — answers {"added", "updated", "removed", "unchanged", "skipped", "complete", "messages": [string]}"#),
    ]
}

/// Whether the account is one whose to-dos Microsoft keeps: it signs in with Microsoft and is not
/// a shared mailbox.
fn keeps_tasks(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| {
        !a.is_shared() && a.kind != AccountKind::Jmap && a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft)
    })
}

/// The account that to-dos made here (with no account yet) go up to: the one the owner said yes
/// to ([`LocalTasks::SendTo`]); none before that, so nothing local leaves the computer unasked.
pub fn tasks_home(mailbox: &Mailbox) -> Option<Id> {
    match mailbox.local_tasks {
        LocalTasks::SendTo(account) if keeps_tasks(mailbox, account) => Some(account),
        _ => None,
    }
}

/// How many to-dos exist only on this computer and could go to an account's To Do.
pub fn local_task_count(mailbox: &Mailbox) -> usize {
    mailbox.tasks.values().filter(|t| t.origin.is_local() && t.origin.id.is_empty() && !t.local_only).count()
}

/// Where a to-do made here lives: the address of the account whose To Do it goes to, or `None`
/// for this computer only.
pub fn tasks_kept_at(mailbox: &Mailbox) -> Option<String> {
    tasks_home(mailbox).and_then(|a| mailbox.account(a)).map(|a| a.email.trim().to_string())
}

/// The accounts whose to-dos are kept in step now.
pub fn task_accounts(mailbox: &Mailbox) -> Vec<Id> {
    mailbox.accounts.iter().map(|a| a.id).filter(|id| keeps_tasks(mailbox, *id)).collect()
}

/// How many changes wait to be told to `account`'s server.
pub fn tasks_waiting(mailbox: &Mailbox, account: Id) -> usize {
    task_upload_of(mailbox, account).map_or(0, |ops| ops.len())
}

fn task_upload_of(mailbox: &Mailbox, account: Id) -> Result<Vec<TaskOp>, CmdError> {
    if !keeps_tasks(mailbox, account) {
        return Err(bad("that account doesn't keep its to-dos with Microsoft"));
    }
    let home = tasks_home(mailbox) == Some(account);
    let mut ops: Vec<TaskOp> = Vec::new();
    for gone in mailbox.task_gone.iter().filter(|gone| gone.account == account) {
        ops.push(TaskOp { kind: TaskOpKind::Delete, task: 0, id: gone.id.clone(), list: gone.list.clone(), patch: Default::default() });
    }
    for task in mailbox.tasks.values().filter(|t| t.origin.account == account && !t.origin.id.is_empty() && !t.local_only) {
        if let Some(patch) = task.patch() {
            ops.push(TaskOp { kind: TaskOpKind::Update, task: task.id, id: task.origin.id.clone(), list: task.list.clone(), patch });
        }
    }
    let waiting =
        |t: &&std::sync::Arc<Task>| !t.local_only && t.origin.id.is_empty() && (t.origin.account == account || (home && t.origin.account == 0));
    for task in mailbox.tasks.values().filter(waiting).take(MAX_TASK_CREATES) {
        ops.push(TaskOp { kind: TaskOpKind::Create, task: task.id, id: String::new(), list: String::new(), patch: task.fields().whole() });
    }
    ops.truncate(MAX_TASK_OPS);
    Ok(ops)
}

/// What `account`'s server is to be told.
pub fn task_upload(mailbox: &Mailbox, account: Id) -> Result<TasksUpload, CmdError> {
    let ops = task_upload_of(mailbox, account)?;
    let a = mailbox.account(account).ok_or_else(|| fail("no such account"))?;
    Ok(TasksUpload { account, email: a.email.trim().to_string(), ops })
}

/// Whether two sets of fields are the same to-do for adopting one the server made (the title is
/// cut to what To Do keeps).
fn same_todo(here: &TaskFields, there: &TaskFields) -> bool {
    let cut = |s: &str| s.trim().chars().take(255).collect::<String>();
    cut(&here.title) == cut(&there.title)
        && here.notes.trim() == there.notes.trim()
        && (here.start, here.due, here.importance, here.completed.is_some()) == (there.start, there.due, there.importance, there.completed.is_some())
}

fn from_remote(remote: &RemoteTask) -> TaskFields {
    TaskFields {
        title: if remote.title.is_empty() { "(No title)".into() } else { remote.title.clone() },
        notes: remote.notes.clone(),
        start: remote.start,
        due: remote.due,
        completed: remote.completed,
        importance: remote.importance,
        categories: remote.categories.clone(),
        reminder: remote.reminder,
    }
}

/// Merge the to-dos a server has into the mailbox (see the module). Nothing is changed when this
/// returns an error. Doing it twice changes nothing the second time.
pub fn merge_tasks(mailbox: &mut Mailbox, download: &TasksDownload) -> Result<Merged, CmdError> {
    let account = owner(mailbox, download.account, &download.email)?.id;
    if !keeps_tasks(mailbox, account) {
        return Err(bad("that account doesn't keep its to-dos with Microsoft"));
    }
    note_shared_lists(mailbox, account, download);
    let mut merged = Merged::default();
    let mut known: HashMap<String, Id> = HashMap::new();
    for task in mailbox.tasks.values().filter(|t| t.origin.account == account && !t.origin.id.is_empty()) {
        known.entry(task.origin.id.clone()).or_insert(task.id);
    }
    let told: HashSet<&str> = mailbox.task_gone.iter().filter(|g| g.account == account).map(|g| g.id.as_str()).collect();
    let told: HashSet<String> = told.into_iter().map(str::to_string).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let home = tasks_home(mailbox);
    for remote in download.tasks.iter().take(MAX_TASKS) {
        let mut remote = remote.clone();
        remote.tidy();
        if remote.id.is_empty() || remote.list.is_empty() || !seen.insert(remote.id.clone()) || told.contains(&remote.id) {
            merged.skipped += 1;
            continue;
        }
        let there = from_remote(&remote);
        let origin = Origin { account, id: remote.id.clone(), change_key: remote.change_key.clone(), ..Default::default() };
        match known.get(&remote.id).and_then(|id| mailbox.task(*id)) {
            Some(old) => {
                // The fields changed here since stay; the rest are the server's.
                let base = old.synced.clone().unwrap_or_else(|| old.fields());
                let fields = base.merged(&old.fields(), &there);
                let mut fresh = old.clone();
                fresh.set_fields(fields);
                fresh.synced = Some(there);
                fresh.origin = origin;
                fresh.list = remote.list.clone();
                if fresh == *old {
                    merged.unchanged += 1;
                } else if let Ok(task) = mailbox.task_mut(fresh.id) {
                    *task = fresh;
                    merged.updated += 1;
                }
            }
            None => {
                // A to-do made here whose answer was lost (the server made it, the word didn't
                // come) is the same one: taken in, not made twice.
                let twin = mailbox
                    .tasks
                    .values()
                    .find(|t| {
                        t.origin.id.is_empty()
                            && (t.origin.account == account || t.origin.is_local() && home == Some(account))
                            && same_todo(&t.fields(), &there)
                    })
                    .map(|t| t.id);
                if let Some(twin) = twin
                    && let Ok(task) = mailbox.task_mut(twin)
                {
                    task.origin = origin;
                    task.list = remote.list.clone();
                    task.synced = Some(there);
                    task.local_only = false;
                    merged.unchanged += 1;
                    continue;
                }
                let mut task = Task { origin, list: remote.list.clone(), synced: Some(there.clone()), ..Default::default() };
                task.set_fields(there);
                mailbox.add_task(task);
                merged.added += 1;
            }
        }
    }
    // What the server no longer has: only when the download says it showed everything there is of
    // the list (or the list itself is gone).
    let whole = |list: &str| download.complete && download.lists.iter().find(|l| l.id == list).is_none_or(|l| l.complete);
    if download.tasks.len() <= MAX_TASKS {
        let missing: Vec<Id> = known
            .iter()
            .filter(|(remote, _)| !seen.contains(*remote))
            .map(|(_, id)| *id)
            .filter(|id| mailbox.task(*id).is_some_and(|t| whole(&t.list)))
            .collect();
        for id in missing {
            let Some(task) = mailbox.task(id) else { continue };
            if task.patch().is_some() {
                // Changed here since: it is made again rather than lost.
                if let Ok(task) = mailbox.task_mut(id) {
                    task.origin = Origin { account, ..Default::default() };
                    task.list.clear();
                    task.synced = None;
                    merged.updated += 1;
                }
            } else if mailbox.remove_task_quietly(id).is_ok() {
                merged.removed += 1;
            }
        }
    }
    Ok(merged)
}

/// Which of the account's lists are shared with other people (#461), as the download says: a list
/// it names is as it says; one it doesn't name is forgotten only when the download names them all.
/// A remembered list that a download doesn't name stays while a to-do of the account is in it (so
/// the gate never forgets a list it may yet be asked about), and otherwise while there is room
/// for it among [`MAX_TASK_LISTS`].
fn note_shared_lists(mailbox: &mut Mailbox, account: Id, download: &TasksDownload) {
    let mut lists: Vec<RemoteTaskList> = download.lists.iter().take(MAX_TASK_LISTS).cloned().collect();
    lists.iter_mut().for_each(RemoteTaskList::tidy);
    let had = mailbox.shared_task_lists.remove(&account).unwrap_or_default();
    let mut shared: BTreeSet<String> = lists.iter().filter(|l| l.shared && !l.id.is_empty()).map(|l| l.id.clone()).collect();
    if !download.complete {
        let in_use: HashSet<&str> = mailbox.tasks.values().filter(|t| t.origin.account == account).map(|t| t.list.as_str()).collect();
        let (used, unused): (Vec<String>, Vec<String>) =
            had.into_iter().filter(|id| !lists.iter().any(|l| l.id == *id)).partition(|id| in_use.contains(id.as_str()));
        shared.extend(used);
        for id in unused {
            if shared.len() >= MAX_TASK_LISTS {
                break;
            }
            shared.insert(id);
        }
    }
    if !shared.is_empty() {
        mailbox.shared_task_lists.insert(account, shared);
    }
}

/// Take in what a server made of the changes it was told. Not an undo step.
pub fn apply_task_outcomes(mailbox: &mut Mailbox, uploaded: &TasksUploaded) -> Result<Merged, CmdError> {
    let account = owner(mailbox, uploaded.account, &uploaded.email)?.id;
    if !keeps_tasks(mailbox, account) {
        return Err(bad("that account doesn't keep its to-dos with Microsoft"));
    }
    let mut merged = Merged::default();
    let mut failed = 0usize;
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let why = || if outcome.error.is_empty() { String::new() } else { format!(": {}", outcome.error) };
        match (outcome.kind, outcome.result) {
            (TaskOpKind::Delete, TaskResult::Done | TaskResult::Gone | TaskResult::Refused) => {
                mailbox.task_gone.retain(|g| !(g.account == account && g.id == outcome.id));
                if outcome.result == TaskResult::Refused {
                    merged.say(format!("A deleted to-do couldn't be deleted from Microsoft To Do{}", why()));
                }
                merged.removed += 1;
            }
            (TaskOpKind::Create, TaskResult::Done) => {
                let sent = TaskFields::default().after(&outcome.patch);
                if outcome.id.is_empty() || outcome.list.is_empty() {
                    failed += 1;
                } else if mailbox.task(outcome.task).is_some() {
                    if let Ok(task) = mailbox.task_mut(outcome.task) {
                        task.origin = Origin { account, id: outcome.id.clone(), change_key: outcome.change_key.clone(), ..Default::default() };
                        task.list = outcome.list.clone();
                        task.synced = Some(sent);
                    }
                    merged.added += 1;
                } else if mailbox.task_gone.len() < mailcraft_model::MAX_TASK_GONE {
                    // Deleted here while it was being made: the server is told to delete it.
                    mailbox.task_gone.push(mailcraft_model::TaskGone { account, id: outcome.id.clone(), list: outcome.list.clone() });
                }
            }
            (TaskOpKind::Update, TaskResult::Done) => {
                if let Ok(task) = mailbox.task_mut(outcome.task) {
                    let base = task.synced.take().unwrap_or_default();
                    task.synced = Some(base.after(&outcome.patch));
                    if !outcome.change_key.is_empty() {
                        task.origin.change_key = outcome.change_key.clone();
                    }
                    merged.updated += 1;
                }
            }
            // The server no longer has it: made again, with what was changed here.
            (TaskOpKind::Update, TaskResult::Gone) => {
                if let Ok(task) = mailbox.task_mut(outcome.task) {
                    task.origin = Origin { account, ..Default::default() };
                    task.list.clear();
                    task.synced = None;
                    merged.updated += 1;
                }
            }
            // Refused: the change stays here (a download does not undo it) and is held, not sent
            // over and over; changing the to-do again tries again.
            (TaskOpKind::Update, TaskResult::Refused) => {
                if let Ok(task) = mailbox.task_mut(outcome.task) {
                    task.local_only = true;
                }
                merged
                    .say(format!("Microsoft To Do didn't take a change to a to-do; it is kept on this computer until you change it again{}", why()));
                failed += 1;
            }
            (TaskOpKind::Create, TaskResult::Refused) => {
                if let Ok(task) = mailbox.task_mut(outcome.task) {
                    task.local_only = true;
                }
                merged.say(format!("Microsoft To Do didn't take a new to-do; it stays on this computer until you change it{}", why()));
                failed += 1;
            }
            (_, TaskResult::Denied) => {
                merged.say(format!("This account may not change its to-dos in Microsoft To Do{}", why()));
                failed += 1;
            }
            _ => failed += 1,
        }
    }
    merged.skipped = failed;
    Ok(merged)
}

fn download_of<T: serde::de::DeserializeOwned>(v: &Value, name: &str) -> Result<T, CmdError> {
    let value = v.get(name).cloned().ok_or_else(|| bad(format!("`{name}` is required")))?;
    serde_json::from_value(value)
        .map_err(|e| bad(format!("`{name}`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))
}

/// Take in the to-dos a host downloaded ([`merge_tasks`]). Not an undo step, as `sync.applyEvents`.
pub fn apply_tasks(s: &mut Session, download: &TasksDownload) -> Result<Merged, CmdError> {
    let merged = merge_tasks(&mut s.mailbox, download)?;
    if merged.changed() {
        s.mailbox_changed();
    }
    Ok(merged)
}

/// Take in what a host heard from the server ([`apply_task_outcomes`]).
pub fn apply_task_changes(s: &mut Session, uploaded: &TasksUploaded) -> Result<Merged, CmdError> {
    let merged = apply_task_outcomes(&mut s.mailbox, uploaded)?;
    // What a server was told can't be undone there: going back to a mailbox from before would
    // undo it here only (the events do the same).
    if uploaded.outcomes.iter().any(|o| matches!(o.result, TaskResult::Done | TaskResult::Gone)) {
        s.forget_history();
    }
    s.mailbox_changed();
    Ok(merged)
}

fn apply_tasks_command(s: &mut Session, v: &Value) -> CmdResult {
    let download = download_of::<TasksDownload>(v, "download")?;
    let merged = apply_tasks(s, &download)?;
    Ok(json!({
        "added": merged.added, "updated": merged.updated, "removed": merged.removed, "unchanged": merged.unchanged,
        "skipped": merged.skipped, "complete": download.complete, "messages": merged.messages,
    }))
}

fn apply_task_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let uploaded = download_of::<TasksUploaded>(v, "result")?;
    let merged = apply_task_changes(s, &uploaded)?;
    Ok(json!({
        "done": merged.added + merged.updated + merged.removed, "failed": merged.skipped, "messages": merged.messages,
    }))
}

fn send_local_command(s: &mut Session, v: &Value) -> CmdResult {
    let answer = match v.get("account") {
        None | Some(Value::Null) => LocalTasks::NotNow,
        Some(_) => {
            let account = crate::p::req_id(v, "account")?;
            if !keeps_tasks(&s.mailbox, account) {
                return Err(bad("that account doesn't keep its to-dos with Microsoft"));
            }
            LocalTasks::SendTo(account)
        }
    };
    s.mailbox.local_tasks = answer;
    s.mailbox_changed();
    let waiting = local_task_count(&s.mailbox);
    Ok(match answer {
        LocalTasks::SendTo(_) => json!({"sent": waiting, "keptHere": 0}),
        _ => json!({"sent": 0, "keptHere": waiting}),
    })
}

fn task_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = crate::p::req_id(v, "account")?;
    let upload = task_upload(&s.mailbox, account)?;
    serde_json::to_value(upload).map_err(|e| fail(e.to_string()))
}
