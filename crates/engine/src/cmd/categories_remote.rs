// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's list of categories, kept in step with its server both ways (#661,
//! Graph's `masterCategories`).
//!
//! The engine does no I/O: a host with sockets (`mailcraft_net::graph::categories`) asks what is
//! to be told to the server (`sync.categoryChanges`, the difference between the account's list and
//! the server's as last synced), says what the server made of it (`sync.applyCategoryChanges`)
//! and brings the server's list (`sync.applyCategories`, [`super::remote::apply_categories`]). It
//! says too whether the account's sign-in covers changing the list (`sync.categoryWrites`): the
//! list of one that doesn't is read-only here.
//!
//! Undo: a change not sent yet is undone like any other. One the server has is undone here and
//! the next job sends the reverse, as for mail: what the server last had stays as it is now
//! ([`crate::undo`]).

use mailcraft_model::remote::{CategoriesUpload, CategoriesUploaded, CategoryResult};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Longest piece of an error that is said back.
const MAX_SHOWN: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.categoryChanges", "Category Changes to Send", "Send / Receive", category_changes_command)
            .reads()
            .params(r##"{"account": number} — what that Microsoft account's server is to be told of its list of categories: {"account", "email", "changes": [{"kind": "create"|"color"|"delete", "name": string, "color": "#RRGGBB"}]}; none while the list is read-only here"##),
        CommandSpec::new("sync.applyCategoryChanges", "Apply Sent Category Changes", "Send / Receive", apply_category_changes_command)
            .pure()
            .params(r##"{"result": CategoriesUploaded} — what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"kind", "name", "color", "result": "done"|"denied"|"failed"|"refused", "error"?: string}]} — answers {"done", "messages": [string]}"##),
        CommandSpec::new("sync.categoryWrites", "Category Changes Allowed", "Send / Receive", category_writes_command)
            .pure()
            .params(r#"{"account": number, "allowed": bool} — whether the sign-in of that Microsoft account covers changing its server's list of categories, as the host found; without it the list is read-only here"#),
    ]
}

/// What `account`'s server is to be told of its list of categories.
pub fn category_upload(s: &Session, account: mailcraft_model::Id) -> Result<CategoriesUpload, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| bad("`account` isn't an account"))?;
    let owner = s.mailbox.junk_owner(a.id);
    let email = s.mailbox.account(owner).map(|o| o.email.trim().to_string()).unwrap_or_default();
    Ok(CategoriesUpload { account: owner, email, changes: s.mailbox.category_upload(owner) })
}

/// How many changes wait to be told to `account`'s server.
pub fn categories_waiting(s: &Session, account: mailcraft_model::Id) -> usize {
    s.mailbox.category_upload(s.mailbox.junk_owner(account)).len()
}

/// Take in what a server made of the changes sent to it. Not an undo step: an undo of the change
/// it took sends the reverse with the next job.
pub fn apply_category_changes(s: &mut Session, uploaded: &CategoriesUploaded) -> Result<Vec<String>, CmdError> {
    let owner = super::remote::owner(&s.mailbox, uploaded.account, &uploaded.email)?.id;
    if !s.mailbox.synced_categories.contains_key(&owner) {
        return Err(fail("this account's categories aren't kept in step with its server"));
    }
    let mut uploaded = uploaded.clone();
    uploaded.tidy();
    let said = s.mailbox.take_category_outcomes(owner, &uploaded.outcomes);
    // The server says the sign-in doesn't cover changing the list: it is read-only from now on.
    if uploaded.outcomes.iter().any(|o| o.result == CategoryResult::Denied) {
        s.mailbox.category_writes.remove(&owner);
    }
    s.mailbox_changed();
    Ok(said)
}

fn category_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let upload = category_upload(s, p::req_id(v, "account")?)?;
    serde_json::to_value(upload).map_err(|e| fail(e.to_string()))
}

fn apply_category_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let value = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let uploaded: CategoriesUploaded = serde_json::from_value(value)
        .map_err(|e| bad(format!("`result`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))?;
    let messages = apply_category_changes(s, &uploaded)?;
    let done = uploaded.outcomes.iter().filter(|o| o.result == CategoryResult::Done).count();
    Ok(json!({"done": done, "messages": messages}))
}

/// Say whether `account`'s sign-in covers changing its server's list of categories. Not an undo
/// step. Whether that changed anything.
pub fn set_category_writes(s: &mut Session, account: mailcraft_model::Id, allowed: bool) -> Result<bool, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| bad("`account` isn't an account"))?;
    if a.kind != mailcraft_model::AccountKind::Microsoft || a.is_shared() {
        return Err(bad("only a Microsoft account's own list of categories is kept in step with its server"));
    }
    let id = a.id;
    let changed = if allowed { s.mailbox.category_writes.insert(id) } else { s.mailbox.category_writes.remove(&id) };
    if changed {
        s.mailbox_changed();
    }
    Ok(changed)
}

fn category_writes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    let allowed = p::bool(v, "allowed").ok_or_else(|| bad("`allowed` is required"))?;
    let changed = set_category_writes(s, account, allowed)?;
    Ok(json!({"changed": changed}))
}
