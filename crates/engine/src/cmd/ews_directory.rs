// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account's calendar, contacts and to-dos (#252): the request a host reads,
//! and taking in what it brought.
//!
//! The engine does no I/O. [`ews_directory_request`] says what a run should read after its mail
//! (the account's calendars, address books and to-do lists, the calendars and books it opened by
//! address, and what was changed here), and [`apply_ews_directory`] merges what came back with the
//! same code a Microsoft account's calendar, contacts and to-dos use.
//!
//! **Nothing here trusts the answer.** The model holds a result to its sizes
//! (`EwsDirectoryResult::check`), and every download is merged through
//! [`super::remote::merge_events`], [`merge_contacts`](super::remote::merge_contacts) and
//! [`super::tasks_remote::merge_tasks`], which tidy each item first.

use mailcraft_model::sync::{EwsDirectoryRequest, EwsDirectoryResult, EwsSharedMailbox};
use mailcraft_model::{Account, AccountKind, EwsServer, Id, Mailbox};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most shared mailboxes one directory request names.
const MAX_SHARED: usize = 50;
/// The days a calendar read covers beyond the calendar in view, either side.
const MARGIN_DAYS: i64 = 8;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.ewsDirectoryRequest", "Exchange Calendar, Contacts and To-dos to Read", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "from"?: seconds, "to"?: seconds, "background"?: bool} — for an Exchange server account: answers what a host should read of its calendars, address books and to-do lists (and those shared with it), after its mail."#)
            .when(available),
        CommandSpec::new("sync.applyEwsDirectory", "Apply Read Exchange Calendar, Contacts and To-dos", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": EwsDirectoryResult} — takes in what a host read from an Exchange server account's calendars, address books and to-do lists."#),
    ]
}

fn available(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Exchange) { None } else { Some("no account is an Exchange server account") }
}

/// The Exchange server account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Exchange).map(|a| a.id))
        .ok_or_else(|| fail("no account is an Exchange server account"))
}

/// The account and the server its directory is read through: its own, or for a shared mailbox the
/// account it is opened through (whose EWS address and user name are used; the shared mailbox's
/// own address is the `mailbox`).
fn ews_account(mb: &Mailbox, account: Id) -> Result<(&Account, &Account, &EwsServer), CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Exchange {
        return Err(failed!("{name} isn't an Exchange server account", name = a.name));
    }
    let signer = if a.is_shared() { mb.owner_of(account).filter(|owner| owner.kind == AccountKind::Exchange) } else { Some(a) };
    let signer = signer.ok_or_else(|| failed!("{name} has no Exchange server account it is opened through", name = a.name))?;
    let server = signer.ews.as_ref().filter(|server| server.problem().is_none());
    let server = server.ok_or_else(|| failed!("{name} has no Exchange server address that can be used", name = a.name))?;
    Ok((a, signer, server))
}

/// What the directory of `account` should read: its calendars in `from..to`, its address books and
/// to-do lists whole, and what was changed here since the server last said.
pub fn ews_directory_request(
    s: &Session,
    account: Id,
    from: Option<i64>,
    to: Option<i64>,
    background: bool,
) -> Result<EwsDirectoryRequest, CmdError> {
    let (a, _signer, server) = ews_account(&s.mailbox, account)?;
    let (from, to) = match (from, to) {
        (Some(from), Some(to)) if to > from => (from, to),
        _ => {
            let (first, last) = super::view::calendar_range(s);
            let instant = |day: mailcraft_time::Date| {
                let midnight = mailcraft_time::DateTime::at_midnight(day);
                midnight.to_timestamp(s.offset_at(midnight.to_timestamp(0)))
            };
            (instant(first.add_days(-MARGIN_DAYS)), instant(last.add_days(MARGIN_DAYS + 1)))
        }
    };
    let mut shared: Vec<EwsSharedMailbox> = Vec::new();
    let mut add = |address: String, calendar: bool, contacts: bool, tasks: bool| {
        let address = address.trim().to_owned();
        if address.is_empty() || shared.len() >= MAX_SHARED {
            return;
        }
        if let Some(one) = shared.iter_mut().find(|one| one.address.eq_ignore_ascii_case(&address)) {
            one.calendar |= calendar;
            one.contacts |= contacts;
            one.tasks |= tasks;
        } else {
            shared.push(EwsSharedMailbox { address, calendar, contacts, tasks });
        }
    };
    if !a.is_shared() {
        for opened in a.opened_calendars.iter().take(mailcraft_model::MAX_OPENED_CALENDARS) {
            add(opened.address.clone(), true, false, false);
        }
        for opened in a.opened_address_books.iter().take(mailcraft_model::MAX_OPENED_ADDRESS_BOOKS) {
            add(opened.address.clone(), false, true, false);
        }
    }
    let changes = super::remote::event_upload(s, account).ok().filter(|upload| !upload.ops.is_empty());
    let contact_changes = super::remote::contact_upload(s, account).filter(|upload| !upload.ops.is_empty());
    let task_changes = super::tasks_remote::task_upload(&s.mailbox, account).ok().filter(|upload| !upload.ops.is_empty());
    Ok(EwsDirectoryRequest {
        account,
        email: a.email.clone(),
        url: server.url.clone(),
        username: server.username.clone(),
        mailbox: if a.is_shared() { a.email.clone() } else { String::new() },
        trusted: server.trusted.clone(),
        calendars: true,
        contacts: true,
        tasks: a.keeps_tasks(),
        from,
        to,
        shared,
        changes,
        contact_changes,
        task_changes,
        background,
    })
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = named_account(s, v)?;
    let from = v.get("from").and_then(Value::as_i64);
    let to = v.get("to").and_then(Value::as_i64);
    let request = ews_directory_request(s, account, from, to, p::bool(v, "background").unwrap_or(false))?;
    serde_json::to_value(request).map_err(|e| fail(e.to_string()))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let result: EwsDirectoryResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't an Exchange directory result"))?;
    apply_ews_directory(s, &result)
}

/// Take in what a directory run brought: what the server said of the changes, then the calendars,
/// the events, the contacts and the to-dos. Answers counts and the lines worth showing.
pub fn apply_ews_directory(s: &mut Session, result: &EwsDirectoryResult) -> CmdResult {
    result.check().map_err(|why| failed!("the result {why}", why = mailcraft_i18n::Arg::words(why)))?;
    let (a, _, _) = ews_account(&s.mailbox, result.account)?;
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("this directory was for another account, not for {name}", name = a.name));
    }
    let account = result.account;
    let mut said: Vec<String> = result.errors.clone();
    let mut counts = json!({"added": 0, "updated": 0, "removed": 0, "contacts": 0, "tasks": 0, "calendars": 0, "events": 0});
    let mut uploaded = false;
    if let Some(outcomes) = &result.uploaded {
        match super::remote::apply_event_outcomes(s, outcomes) {
            Ok(applied) => {
                uploaded = !outcomes.outcomes.is_empty();
                said.extend(applied.messages.iter().cloned());
                counts["updated"] = json!(applied.done);
            }
            Err(e) => said.push(e.to_string()),
        }
    }
    if let Some(outcomes) = &result.contacts_uploaded {
        uploaded |= !outcomes.outcomes.is_empty();
        let applied = super::remote::apply_contact_outcomes(s, outcomes)?;
        counts["contacts"] = applied.get("saved").cloned().unwrap_or_else(|| json!(0));
    }
    if let Some(outcomes) = &result.tasks_uploaded {
        uploaded |= !outcomes.outcomes.is_empty();
        let applied = super::tasks_remote::apply_task_outcomes(&mut s.mailbox, outcomes)?;
        counts["tasks"] = json!(applied.added + applied.updated);
    }
    if let Some(list) = &result.calendars {
        let applied = super::remote::apply_calendars(s, list)?;
        counts["calendars"] = applied.get("added").cloned().unwrap_or_else(|| json!(0));
    }
    let mut events = 0usize;
    for download in &result.events {
        events = events.saturating_add(download.events.len());
        match super::remote::take_in_events(s, download) {
            Ok(merged) => {
                said.extend(merged.messages.iter().cloned());
                if let Some(first) = merged.messages.first() {
                    let _ = first;
                }
            }
            Err(e) => said.push(e.to_string()),
        }
    }
    counts["events"] = json!(events);
    if let Some(contacts) = &result.contacts {
        match super::remote::apply_contacts(s, contacts) {
            Ok(applied) => {
                counts["contacts"] =
                    json!(applied.get("added").and_then(Value::as_u64).unwrap_or(0) + applied.get("updated").and_then(Value::as_u64).unwrap_or(0));
            }
            Err(e) => said.push(e.to_string()),
        }
    }
    if let Some(tasks) = &result.tasks {
        match super::tasks_remote::merge_tasks(&mut s.mailbox, tasks) {
            Ok(merged) => {
                counts["tasks"] = json!(merged.added + merged.updated);
                said.extend(merged.messages.iter().cloned());
            }
            Err(e) => said.push(e.to_string()),
        }
    }
    if uploaded || result.calendars.is_some() || !result.events.is_empty() || result.contacts.is_some() || result.tasks.is_some() {
        s.mailbox_changed();
    }
    said.truncate(20);
    Ok(json!({
        "account": account,
        "calendars": counts["calendars"],
        "events": counts["events"],
        "contacts": counts["contacts"],
        "tasks": counts["tasks"],
        "errors": said,
        "busyFor": result.busy_for,
        "authRefused": result.auth_refused,
        "untrusted": result.untrusted,
    }))
}

/// The id the result's run was for, from a command's params: kept for a host that applies a
/// directory result on its own.
pub fn result_account(result: &EwsDirectoryResult) -> Id {
    result.account
}

#[cfg(test)]
#[path = "ews_directory_tests.rs"]
mod tests;
