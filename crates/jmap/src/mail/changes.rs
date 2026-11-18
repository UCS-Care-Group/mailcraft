// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of messages made here, taken up to the server with `Email/set` (RFC 8621 §4.6, RFC 8620 §5.3).
//!
//! - **What goes.** The marks a change names, each as a patch of one keyword (`keywords/$seen`,
//!   `keywords/$flagged`, `keywords/$answered`, `keywords/$forwarded`: `true` to set it, `null` to take it
//!   away) and never another keyword; a move as two patches of the mailboxes the email lies in
//!   (`mailboxIds/<from>` to `null`, `mailboxIds/<to>` to `true`); a message deleted for good in `destroy`.
//! - **How many.** [`MAX_CHANGES`] of a request's changes in a run, the server's `maxObjectsInSet` to a call
//!   at most. A call never holds two changes of one email, so every change has an answer of its own and they
//!   reach the server in the order they were made.
//! - **What comes back.** For each change the server's word: done (with the email's new id, when the server
//!   gave it one), gone (`notFound`), or refused, with why. A change the answer says nothing of is looked up
//!   with `Email/get` (the server's `maxObjectsInGet` ids to a call at most, which may be fewer than a `set`
//!   takes): an email the server has not got is gone (some servers pass over an id they do not
//!   know instead of saying `notFound`), and anything else is left unanswered. A change left unanswered is not
//!   reported, and neither is any change of a call that failed as a whole: the engine works those out again.
//!   A change that names an id no server hands out is refused here, without asking.
//! - **Order within a run.** When a move gives an email a new id, the later changes of the run name it by
//!   that one. After a change of an email was refused or went unanswered, the later changes of that email
//!   wait for the next run, and after a call failed as a whole all the rest do.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{JmapChange, JmapChangeDone, JmapMailResult, JmapOutcome};
use serde_json::{Map, Value};

use super::{Outcome, Run, Stop, arguments, get, refusal};
use crate::{HttpExchange, Id, Redactor};

/// Most changes one run takes up; the rest are not answered, and come with the next request.
pub(super) const MAX_CHANGES: usize = 500;
/// The most the answer to one `Email/set` may be, in bytes.
const SET_BYTES: usize = 2 << 20;
/// Most entries of a list in an answer that are looked at.
const MAX_LISTED: usize = 10_000;

/// Why a change is refused here.
const NOT_AN_ID: &str = "the message's id is not one a server hands out";
const NOT_A_MAILBOX: &str = "a mailbox's id is not one a server hands out";

/// What is to be done about a change before the server is asked.
enum Plan {
    /// It goes to the server, the email named by this id.
    Ask(String),
    /// It names no mark: there is nothing to do, and it is done.
    Nothing,
    /// It cannot be asked.
    Refuse(&'static str),
}

/// The plan for `change`, an email that got a new id in this run named by that one.
fn plan(change: &JmapChange, renamed: &HashMap<String, String>) -> Plan {
    if Id::new(change.id()).is_err() {
        return Plan::Refuse(NOT_AN_ID);
    }
    match change {
        JmapChange::Move { from, to, .. } if Id::new(from).is_err() || Id::new(to).is_err() => return Plan::Refuse(NOT_A_MAILBOX),
        JmapChange::Keywords { read: None, flagged: None, answered: None, forwarded: None, .. } => return Plan::Nothing,
        _ => {}
    }
    Plan::Ask(renamed.get(change.id()).cloned().unwrap_or_else(|| change.id().to_string()))
}

/// The patch a change of marks or a move is (RFC 8620 §5.3): `None` for a change that is no update.
fn patch(change: &JmapChange) -> Option<Map<String, Value>> {
    let mut patch = Map::new();
    match change {
        JmapChange::Keywords { read, flagged, answered, forwarded, .. } => {
            for (keyword, mark) in [("$seen", read), ("$flagged", flagged), ("$answered", answered), ("$forwarded", forwarded)] {
                if let Some(on) = mark {
                    patch.insert(format!("keywords/{keyword}"), if *on { Value::Bool(true) } else { Value::Null });
                }
            }
        }
        JmapChange::Move { from, to, .. } => {
            patch.insert(format!("mailboxIds/{from}"), Value::Null);
            patch.insert(format!("mailboxIds/{to}"), Value::Bool(true));
        }
        JmapChange::Destroy { .. } => return None,
    }
    Some(patch)
}

/// The arguments of the `Email/set` that takes `chunk` up: each change with the id its email goes by.
pub(super) fn set_arguments(account: &str, chunk: &[(&JmapChange, String)]) -> Map<String, Value> {
    let mut update = Map::new();
    let mut destroy = Vec::new();
    for (change, id) in chunk {
        match patch(change) {
            Some(patch) => {
                update.insert(id.clone(), Value::Object(patch));
            }
            None => destroy.push(Value::String(id.clone())),
        }
    }
    let mut args = Map::new();
    args.insert("accountId".into(), Value::String(account.to_string()));
    if !update.is_empty() {
        args.insert("update".into(), Value::Object(update));
    }
    if !destroy.is_empty() {
        args.insert("destroy".into(), Value::Array(destroy));
    }
    args
}

/// What the answer to an `Email/set` says of `change`, whose email went by `id`: `None` when it says nothing
/// of it. Whatever the answer is, this reads it without trusting it: only this id is looked up.
pub(super) fn outcome_of(answer: &Value, change: &JmapChange, id: &str, secrets: &Redactor) -> Option<JmapOutcome> {
    let destroys = matches!(change, JmapChange::Destroy { .. });
    let done = if destroys {
        let destroyed = answer.get("destroyed").and_then(Value::as_array);
        destroyed.is_some_and(|ids| ids.iter().take(MAX_LISTED).any(|gone| gone.as_str() == Some(id))).then(String::new)
    } else {
        // (The entry is what the server changed besides: a new id, when it gave the email one.)
        answer.get("updated").and_then(Value::as_object).and_then(|updated| updated.get(id)).map(|changed| {
            let new = changed.get("id").and_then(Value::as_str).filter(|new| *new != id && Id::new(new).is_ok());
            new.unwrap_or_default().to_string()
        })
    };
    if let Some(id) = done {
        return Some(JmapOutcome::Done { id });
    }
    let failed = answer.get(if destroys { "notDestroyed" } else { "notUpdated" }).and_then(Value::as_object)?.get(id)?;
    Some(match failed.get("type").and_then(Value::as_str) {
        Some("notFound") => JmapOutcome::Gone,
        _ => JmapOutcome::Refused { why: refusal(secrets, failed) },
    })
}

impl<H: HttpExchange> Run<'_, H> {
    /// Take the request's changes up, a piece for the answers of each call. An error is what ends the run
    /// (stopped, the sign-in refused, the server busy); a call that failed otherwise is a line, and the run
    /// goes on to read.
    pub(super) async fn changes(&self, out: &mut Outcome, emit: &mut dyn FnMut(JmapMailResult)) -> Result<(), Stop> {
        let changes = &self.request.changes;
        if changes.is_empty() {
            return Ok(());
        }
        let at_once = usize::try_from(self.connection.client().limits().max_objects_in_set()).unwrap_or(usize::MAX).clamp(1, MAX_CHANGES);
        let get_at_once = usize::try_from(self.connection.client().limits().max_objects_in_get()).unwrap_or(usize::MAX).clamp(1, MAX_CHANGES);
        // Emails a move gave a new id in this run, and emails whose later changes wait for the next.
        let mut renamed: HashMap<String, String> = HashMap::new();
        let mut waiting: HashSet<String> = HashSet::new();
        let mut rest = changes.iter().take(MAX_CHANGES).peekable();
        let (mut answered, mut asked) = (0usize, 0usize);
        loop {
            // The next call's changes, and what is known of the others without asking.
            let mut known: Vec<JmapChangeDone> = Vec::new();
            let mut chunk: Vec<(&JmapChange, String)> = Vec::new();
            let mut named: HashSet<String> = HashSet::new();
            while chunk.len() < at_once {
                let Some(change) = rest.peek().copied() else { break };
                match plan(change, &renamed) {
                    Plan::Ask(id) if named.contains(&id) => break,
                    Plan::Ask(id) if waiting.contains(&id) || waiting.contains(change.id()) => {}
                    Plan::Ask(id) => {
                        named.insert(id.clone());
                        chunk.push((change, id));
                    }
                    Plan::Nothing => known.push(JmapChangeDone { change: change.clone(), outcome: JmapOutcome::Done { id: String::new() } }),
                    Plan::Refuse(why) => {
                        known.push(JmapChangeDone { change: change.clone(), outcome: JmapOutcome::Refused { why: why.to_string() } })
                    }
                }
                rest.next();
            }
            let over = rest.peek().is_none();
            let mut hand_on = |done: Vec<JmapChangeDone>| {
                if !done.is_empty() {
                    emit(self.piece(JmapMailResult { changes: done, partial: true, ..Default::default() }));
                }
            };
            if chunk.is_empty() {
                hand_on(known);
                if over {
                    break;
                }
                continue;
            }
            asked = asked.saturating_add(chunk.len());
            let got = match self.check() {
                Ok(()) => self.one_call("Email/set", set_arguments(self.account(), &chunk), SET_BYTES).await,
                Err(stop) => Err(stop),
            };
            let answer = match got {
                Ok(answer) => answer,
                Err(stop) => {
                    hand_on(known);
                    return match stop {
                        // This step's trouble: none of the call's changes is answered, and the rest wait.
                        Stop::Failed(error) | Stop::Down(error) => {
                            log::info!("jmap mail: a call that takes changes up failed; they are worked out again for the next run");
                            out.changes.get_or_insert_with(|| self.said(&error));
                            Ok(())
                        }
                        stop => Err(stop),
                    };
                }
            };
            // Changes the answer says nothing of. A server that keeps to the specification names every
            // id it was asked about; one that passes over an email it does not have would be asked the
            // same again at every run, so those are looked up, and what the server has not got is gone.
            let mut silent: Vec<(&JmapChange, String)> = Vec::new();
            for (change, id) in chunk {
                match outcome_of(&answer, change, &id, &self.secrets) {
                    Some(outcome) => {
                        match &outcome {
                            JmapOutcome::Done { id: new } if !new.is_empty() => {
                                renamed.insert(change.id().to_string(), new.clone());
                            }
                            JmapOutcome::Refused { .. } => {
                                waiting.insert(id);
                            }
                            _ => {}
                        }
                        answered = answered.saturating_add(1);
                        known.push(JmapChangeDone { change: change.clone(), outcome });
                    }
                    None => {
                        waiting.insert(id.clone());
                        silent.push((change, id));
                    }
                }
            }
            // (As many ids to a look as the server answers in one `get`, which may be fewer than it
            // takes in a `set`: a look for more would be refused as too large, at every run.)
            for looked_for in silent.chunks(get_at_once) {
                let ids: Vec<&str> = looked_for.iter().map(|(_, id)| id.as_str()).collect();
                let looked = match self.check().and_then(|()| Ok(arguments(&get(self.account(), Some(&ids), &["id"]))?)) {
                    Ok(args) => self.one_call("Email/get", args, SET_BYTES).await,
                    Err(stop) => Err(stop),
                };
                match looked {
                    Ok(found) => {
                        let gone: HashSet<&str> = found
                            .get("notFound")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .take(MAX_LISTED)
                            .filter_map(Value::as_str)
                            .collect();
                        for (change, _) in looked_for.iter().filter(|(_, id)| gone.contains(id.as_str())) {
                            answered = answered.saturating_add(1);
                            known.push(JmapChangeDone { change: (*change).clone(), outcome: JmapOutcome::Gone });
                        }
                    }
                    // The look alone failed: they are asked again with the next run, as before, and so
                    // are those of the looks that would have followed.
                    Err(Stop::Failed(_) | Stop::Down(_)) => break,
                    Err(stop) => {
                        hand_on(known);
                        return Err(stop);
                    }
                }
            }
            hand_on(known);
            if over {
                break;
            }
        }
        log::debug!("jmap mail: {answered} of {asked} changes asked were answered ({} came with the request)", changes.len());
        Ok(())
    }
}
