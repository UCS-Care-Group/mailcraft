// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules: what to do with arriving mail.

use mailcraft_model::{Action, Condition, Id, Importance, Rule, clean_name};
use serde_json::{Value, json};

use super::{bad, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

const MAX_RULES: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.add", "Create Rule", "Home › Move", add).params(
            r#"{"name": string, "conditions": [{"type": "fromContains"|"toContains"|"subjectContains"|"bodyContains", "value": string} | {"type": "hasAttachment"} | {"type": "importance", "value": "high"|"normal"|"low"}], "actions": [{"type": "moveTo", "value": folder id} | {"type": "categorize", "value": string} | {"type": "flag"} | {"type": "markRead"} | {"type": "setImportance", "value": string} | {"type": "delete"}], "stop"?: bool}"#,
        ),
        CommandSpec::new("rules.remove", "Delete Rule", "Home › Move", remove).params(r#"{"id": number}"#),
        CommandSpec::new("rules.enable", "Enable Rule", "Home › Move", enable).params(r#"{"id": number, "value"?: bool (default: toggle)}"#),
        CommandSpec::new("rules.list", "Manage Rules", "Home › Move", list).pure(),
        CommandSpec::new("rules.run", "Run Rules Now", "Folder › Clean Up", run).params(r#"{"folder"?: number} (default: the folder in view)"#),
        CommandSpec::new("rules.fromMessage", "Always Move Messages From", "Home › Move", from_message)
            .params(r#"{"folder": number, "id"?: number} — a rule moving this sender's mail to the folder, run at once"#)
            .selection(needs_message),
    ]
}

fn text(v: &Value) -> Result<String, CmdError> {
    let t: String = p::str(v, "value").unwrap_or("").trim().chars().take(200).collect();
    if t.is_empty() { Err(bad("a condition needs a `value`")) } else { Ok(t) }
}

fn condition(v: &Value) -> Result<Condition, CmdError> {
    Ok(match p::req_str(v, "type")? {
        "fromContains" => Condition::FromContains(text(v)?),
        "toContains" => Condition::ToContains(text(v)?),
        "subjectContains" => Condition::SubjectContains(text(v)?),
        "bodyContains" => Condition::BodyContains(text(v)?),
        "hasAttachment" => Condition::HasAttachment,
        "importance" => {
            Condition::Importance(Importance::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("importance is low, normal or high"))?)
        }
        other => return Err(bad(format!("unknown condition type \"{other}\""))),
    })
}

fn action(s: &Session, v: &Value) -> Result<Action, CmdError> {
    Ok(match p::req_str(v, "type")? {
        "moveTo" => {
            let folder = p::req_id(v, "value")?;
            Action::MoveTo(s.mailbox.folder(folder).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id = folder))?)
        }
        "categorize" => Action::Categorize(clean_name(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("categorize needs a category name"))?),
        "flag" => Action::Flag,
        "markRead" => Action::MarkRead,
        "setImportance" => {
            Action::SetImportance(Importance::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("importance is low, normal or high"))?)
        }
        "delete" => Action::Delete,
        other => return Err(bad(format!("unknown action type \"{other}\""))),
    })
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let conditions = v
        .get("conditions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`conditions` (a list) is required"))?
        .iter()
        .take(20)
        .map(condition)
        .collect::<Result<Vec<_>, _>>()?;
    let actions = v
        .get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`actions` (a list) is required"))?
        .iter()
        .take(20)
        .map(|a| action(s, a))
        .collect::<Result<Vec<_>, _>>()?;
    if conditions.is_empty() || actions.is_empty() {
        return Err(bad("a rule needs at least one condition and one action"));
    }
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    s.mailbox.rules.push(Rule { id, name, enabled: true, conditions, actions, stop: p::bool(v, "stop").unwrap_or(false) });
    Ok(json!({"id": id}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let before = s.mailbox.rules.len();
    s.mailbox.rules.retain(|r| r.id != id);
    if s.mailbox.rules.len() == before {
        return Err(failed!("no rule with id {id}", id));
    }
    Ok(json!({"deleted": id}))
}

fn enable(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let r = s.mailbox.rules.iter_mut().find(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    r.enabled = p::bool(v, "value").unwrap_or(!r.enabled);
    Ok(json!({"id": id, "enabled": r.enabled}))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let rules: Vec<Value> = s
        .mailbox
        .rules
        .iter()
        .map(|r| json!({"id": r.id, "name": r.name, "enabled": r.enabled, "stop": r.stop, "text": r.describe(), "rule": r}))
        .collect();
    Ok(json!({"rules": rules}))
}

/// Run every enabled rule on the messages of `folder`; answers how many messages a rule acted on.
fn run_on(s: &mut Session, folder: Id) -> usize {
    let ids: Vec<Id> = s.mailbox.messages_in(folder).map(|m| m.id).collect();
    // A rule that reads the body reads it whole: one kept on disk is brought in first (one that
    // can't be read says so in its place, and is matched as that).
    let reads_bodies = s.mailbox.rules.iter().any(|r| r.enabled && r.conditions.iter().any(|c| matches!(c, Condition::BodyContains(_))));
    ids.into_iter()
        .filter(|id| {
            if reads_bodies {
                let _ = s.load_where(*id, |_| false);
            }
            !s.mailbox.apply_rules(*id).is_empty()
        })
        .count()
}

fn run(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::id(v, "folder").unwrap_or(s.view.folder);
    if s.mailbox.folder(folder).is_none() {
        return Err(failed!("no folder with id {id}", id = folder));
    }
    let matched = run_on(s, folder);
    Ok(json!({"matched": matched}))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::req_id(v, "folder")?;
    let target = s.mailbox.folder(folder).map(|f| f.name.clone()).ok_or_else(|| failed!("no folder with id {id}", id = folder))?;
    let mid = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(mid).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    let (from, source) = (m.from.clone(), m.folder);
    if from.email.trim().is_empty() {
        return Err(fail("the sender has no address to match"));
    }
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    let name = format!("{} to {target}", from.display());
    s.mailbox.rules.push(Rule {
        id,
        name: name.chars().take(mailcraft_model::MAX_NAME).collect(),
        enabled: true,
        conditions: vec![Condition::FromContains(from.email)],
        actions: vec![Action::MoveTo(folder)],
        stop: false,
    });
    let matched = run_on(s, source);
    Ok(json!({"id": id, "matched": matched}))
}
