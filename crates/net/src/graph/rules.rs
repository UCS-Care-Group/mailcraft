// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules a Microsoft account's server keeps (#566): Graph's `messageRules` of the Inbox,
//! read whole each time (capped: [`MAX_SERVER_RULES`]), and the changes made here sent one by
//! one.
//!
//! Reading them needs `MailboxSettings.Read`, which every sign-in has; changing them needs
//! `MailboxSettings.ReadWrite`, which is asked for by itself: a sign-in that doesn't cover it is
//! answered 403, which is [`RuleResult::Denied`]. Nothing of a rule (its name, the words it looks
//! for, its id) is written to a log.
//!
//! Folders are named by their immutable ids, as the account's mail has them: every request asks
//! for those, and a folder a rule names by another id is looked up by it once
//! ([`MAX_FOLDER_LOOKUPS`] at most).
//!
//! What comes back is hostile JSON like everything else from a server: the list is capped, an
//! entry that is no rule is skipped, and [`RemoteRule::tidy`] cuts the rest to size.

use std::collections::HashMap;

use mailcraft_model::remote_rules::{
    MAX_RULE_OPS, MAX_SERVER_RULES, RemoteRule, RuleOp, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUpload, RulesUploaded,
};
use mailcraft_model::{Action, Condition, Importance};
use serde_json::{Map, Value, json};

use super::tasks::valid_id;
use super::{ASK_TIMEOUT, Doing, Graph, MAX_PAGE, MAX_WRITTEN, WRITE_TIMEOUT, text};
use crate::NetError;
use crate::http;

const RULES: &str = "/v1.0/me/mailFolders/inbox/messageRules";
const FOLDERS: &str = "/v1.0/me/mailFolders";
/// Most folders named by a rule that are looked up by the id the rule gives.
pub const MAX_FOLDER_LOOKUPS: usize = 20;
/// What a rule's move is called among its parts, when the folder isn't one the account has.
const MOVE: &str = "moveToFolder";

/// The account's rules, as its server has them. `folders` are the ids of the account's folders
/// as its mail has them; a rule that moves mail to a folder named otherwise has it looked up.
pub fn rules(token: &str, folders: &[String]) -> Result<RulesDownload, NetError> {
    rules_at(&super::graph_base(), token, folders)
}

/// Tell the account's server what was done here to its rules, one change after the other; what
/// it made of each comes back ([`RuleOutcome`]).
///
/// - made (201), changed (200), deleted (204), or any other 2xx: [`RuleResult::Done`].
/// - 404 or 410: [`RuleResult::Gone`].
/// - 403: [`RuleResult::Denied`]; the rest of the upload is left untried.
/// - 409, 412, 429, 503, 504, any other 5xx, no connection: [`RuleResult::Failed`], to be tried
///   again; the rest is left untried.
/// - any other refusal (400, 413…): [`RuleResult::Refused`], for good.
/// - 401: nothing more is sent, and the upload says the sign-in was refused.
pub fn push_rules(token: &str, upload: &RulesUpload) -> RulesUploaded {
    push_rules_at(&super::graph_base(), token, upload)
}

fn graph_at<'a>(base: &str, token: &'a str) -> Result<Graph<'a>, NetError> {
    let graph = Graph::at(base, token)?;
    Ok(Graph { immutable: true, ..graph })
}

pub(super) fn rules_at(base: &str, token: &str, folders: &[String]) -> Result<RulesDownload, NetError> {
    let graph = graph_at(base, token)?;
    let mut download = RulesDownload::default();
    let mut unread = 0usize;
    let whole = graph.list(Doing::Rules, graph.url(RULES), &[], MAX_SERVER_RULES, &mut |entry| match rule_of(entry) {
        Some(rule) if !download.rules.iter().any(|had| had.id == rule.id) => download.rules.push(rule),
        Some(_) => {}
        None => unread += 1,
    })?;
    download.complete = whole && unread == 0;
    // The folders named otherwise than the account's mail names them, each looked up once.
    let mut named: HashMap<String, Option<String>> = HashMap::new();
    for rule in &download.rules {
        if !rule.move_to.is_empty() && !folders.contains(&rule.move_to) && !named.contains_key(&rule.move_to) && named.len() < MAX_FOLDER_LOOKUPS {
            named.insert(rule.move_to.clone(), None);
        }
    }
    for (asked, found) in &mut named {
        *found = folder_id(&graph, asked)?;
    }
    for rule in &mut download.rules {
        if let Some(Some(id)) = named.get(&rule.move_to) {
            rule.move_to = id.clone();
        }
    }
    log::debug!("rules: {} of them, {unread} that couldn't be read, complete: {}", download.rules.len(), download.complete);
    Ok(download)
}

/// The immutable id of the folder Graph knows as `asked`; none when it doesn't say.
fn folder_id(graph: &Graph<'_>, asked: &str) -> Result<Option<String>, NetError> {
    if !valid_id(asked) {
        return Ok(None);
    }
    let url = graph.url(&format!("{FOLDERS}/{}?{}", http::encode(asked), http::form(&[("$select", "id")])));
    match graph.ask(&url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE) {
        Ok(reply) if reply.status == 200 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            Ok(Some(text(&json, "id").to_string()).filter(|id| valid_id(id)))
        }
        Ok(reply) if reply.status == 401 => Err(graph.refusal(Doing::Rules, &reply, "")),
        // Gone, or not to be read now: the rule says it moves mail to a folder that isn't known.
        Ok(_) => Ok(None),
        Err(e) if e.is_refused_sign_in() => Err(e),
        Err(_) => Ok(None),
    }
}

/// Whether a part of a rule says anything: a part that is `null`, `false`, empty or `{}` is one
/// the rule doesn't have.
fn said(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => o.iter().any(|(name, value)| !name.starts_with('@') && said(value)),
        _ => true,
    }
}

/// The one text of a list of words (Graph's `senderContains: ["x"]`). A list of more is one of
/// "any of these", which a rule here can't say.
fn one_word(value: &Value) -> Option<String> {
    match value.as_array()?.as_slice() {
        [Value::String(word)] => Some(word.clone()),
        _ => None,
    }
}

pub(super) fn rule_of(entry: &Value) -> Option<RemoteRule> {
    let id = text(entry, "id");
    if !valid_id(id) {
        return None;
    }
    let flag = |name: &str| entry.get(name).and_then(Value::as_bool).unwrap_or(false);
    let mut rule = RemoteRule {
        id: id.to_string(),
        name: text(entry, "displayName").to_string(),
        sequence: entry.get("sequence").and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()).unwrap_or(0),
        enabled: flag("isEnabled"),
        read_only: flag("isReadOnly"),
        error: flag("hasError"),
        ..Default::default()
    };
    let empty = Map::new();
    let conditions = entry.get("conditions").and_then(Value::as_object).unwrap_or(&empty);
    for (name, value) in conditions.iter().filter(|(name, value)| !name.starts_with('@') && said(value)) {
        let condition = match name.as_str() {
            "senderContains" => one_word(value).map(Condition::FromContains),
            "recipientContains" => one_word(value).map(Condition::ToContains),
            "subjectContains" => one_word(value).map(Condition::SubjectContains),
            "bodyContains" => one_word(value).map(Condition::BodyContains),
            "hasAttachments" if value.as_bool() == Some(true) => Some(Condition::HasAttachment),
            "importance" => value.as_str().and_then(Importance::parse).map(Condition::Importance),
            _ => None,
        };
        match condition {
            Some(c) => rule.conditions.push(c),
            None => rule.unknown.push(name.clone()),
        }
    }
    // A rule of the server without conditions acts on every message: not one a rule here is.
    if rule.conditions.is_empty() && rule.unknown.is_empty() {
        rule.unknown.push("conditions".into());
    }
    if entry.get("exceptions").is_some_and(said) {
        rule.unknown.push("exceptions".into());
    }
    let actions = entry.get("actions").and_then(Value::as_object).unwrap_or(&empty);
    for (name, value) in actions.iter().filter(|(name, value)| !name.starts_with('@') && said(value)) {
        let known = match name.as_str() {
            "assignCategories" => match value.as_array() {
                Some(names) if names.iter().all(Value::is_string) => {
                    rule.actions.extend(names.iter().filter_map(Value::as_str).map(|c| Action::Categorize(c.to_string())));
                    true
                }
                _ => false,
            },
            "markAsRead" => {
                rule.actions.push(Action::MarkRead);
                true
            }
            "markImportance" => match value.as_str().and_then(Importance::parse) {
                Some(i) => {
                    rule.actions.push(Action::SetImportance(i));
                    true
                }
                None => false,
            },
            "delete" => {
                rule.actions.push(Action::Delete);
                true
            }
            MOVE => match value.as_str() {
                Some(folder) if valid_id(folder) => {
                    rule.move_to = folder.to_string();
                    true
                }
                _ => false,
            },
            "stopProcessingRules" => {
                rule.stop = true;
                true
            }
            _ => false,
        };
        if !known {
            rule.unknown.push(name.clone());
        }
    }
    rule.tidy();
    Some(rule)
}

/// The body of a rule to be made on the server, or why it can't be.
pub(super) fn rule_body(rule: &RemoteRule) -> Result<Value, &'static str> {
    if !rule.unknown.is_empty() {
        return Err("a rule with parts this version doesn't know isn't made again");
    }
    if rule.conditions.is_empty() {
        // On the server it would act on every message.
        return Err("a rule without conditions isn't made on the server");
    }
    if rule.actions.is_empty() && rule.move_to.is_empty() {
        return Err("a rule that does nothing isn't made on the server");
    }
    let mut conditions = Map::new();
    for condition in &rule.conditions {
        let (name, value) = match condition {
            Condition::FromContains(t) => ("senderContains", json!([t])),
            Condition::ToContains(t) => ("recipientContains", json!([t])),
            Condition::SubjectContains(t) => ("subjectContains", json!([t])),
            Condition::BodyContains(t) => ("bodyContains", json!([t])),
            Condition::HasAttachment => ("hasAttachments", json!(true)),
            Condition::Importance(i) => ("importance", json!(i.code())),
        };
        if conditions.insert(name.into(), value).is_some() {
            return Err("the server's rules have one condition of each kind");
        }
    }
    let mut actions = Map::new();
    let mut categories: Vec<&str> = Vec::new();
    for action in &rule.actions {
        match action {
            Action::Categorize(c) => {
                if !categories.contains(&c.as_str()) {
                    categories.push(c);
                }
            }
            Action::MarkRead => {
                actions.insert("markAsRead".into(), json!(true));
            }
            Action::SetImportance(i) => {
                if actions.insert("markImportance".into(), json!(i.code())).is_some_and(|had| had != json!(i.code())) {
                    return Err("the server's rules set one importance");
                }
            }
            Action::Delete => {
                actions.insert("delete".into(), json!(true));
            }
            Action::Flag => return Err("the server's rules can't flag mail"),
            Action::MoveTo(_) => return Err("a folder to move mail to is named by the server's id"),
        }
    }
    if !categories.is_empty() {
        actions.insert("assignCategories".into(), json!(categories));
    }
    if !rule.move_to.is_empty() {
        if !valid_id(&rule.move_to) {
            return Err("that isn't a folder the server could have");
        }
        actions.insert(MOVE.into(), json!(rule.move_to));
    }
    actions.insert("stopProcessingRules".into(), json!(rule.stop));
    Ok(json!({
        "displayName": rule.name,
        "sequence": rule.sequence.max(1),
        "isEnabled": rule.enabled,
        "conditions": conditions,
        "actions": actions,
    }))
}

/// The body of a change: only what it changes.
fn change_body(op: &RuleOp) -> Option<Value> {
    let mut body = Map::new();
    if let Some(enabled) = op.enabled {
        body.insert("isEnabled".into(), json!(enabled));
    }
    if let Some(sequence) = op.sequence {
        body.insert("sequence".into(), json!(sequence.max(1)));
    }
    if body.is_empty() { None } else { Some(Value::Object(body)) }
}

pub(super) fn push_rules_at(base: &str, token: &str, upload: &RulesUpload) -> RulesUploaded {
    let mut uploaded = RulesUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let Ok(graph) = graph_at(base, token) else {
        uploaded.token_refused = true;
        return uploaded;
    };
    for op in upload.ops.iter().take(MAX_RULE_OPS) {
        let outcome = |result: RuleResult, error: &str| RuleOutcome {
            kind: op.kind,
            rule: op.rule,
            id: op.id.clone(),
            result,
            enabled: op.enabled,
            sequence: op.sequence,
            error: error.into(),
            ..Default::default()
        };
        if op.kind != RuleOpKind::Create && !valid_id(&op.id) {
            uploaded.outcomes.push(outcome(RuleResult::Refused, "that isn't a rule the server could have"));
            continue;
        }
        let one = format!("{RULES}/{}", http::encode(&op.id));
        let (method, path, body) = match op.kind {
            RuleOpKind::Create => {
                let mut rule = op.body.clone().unwrap_or_default();
                rule.tidy();
                match rule_body(&rule) {
                    Ok(body) => ("POST", RULES.to_string(), Some(body)),
                    Err(why) => {
                        uploaded.outcomes.push(outcome(RuleResult::Refused, why));
                        continue;
                    }
                }
            }
            RuleOpKind::Update => match change_body(op) {
                Some(body) => ("PATCH", one, Some(body)),
                None => {
                    uploaded.outcomes.push(outcome(RuleResult::Refused, "the change says nothing that could be sent"));
                    continue;
                }
            },
            RuleOpKind::Delete => ("DELETE", one, None),
        };
        let body = body.map(|b| b.to_string());
        let headers: &[(&str, &str)] = if body.is_some() { &[("Content-Type", "application/json")] } else { &[] };
        let reply = match graph.ask(&graph.url(&path), method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN) {
            Ok(reply) => reply,
            Err(e) => {
                uploaded.outcomes.push(outcome(RuleResult::Failed, &e.to_string()));
                break;
            }
        };
        let sent = body.as_deref().unwrap_or("");
        let mut done = outcome(RuleResult::Done, "");
        done.status = reply.status;
        let stop = match reply.status {
            401 => {
                uploaded.token_refused = true;
                break;
            }
            200..=299 => {
                if op.kind == RuleOpKind::Create {
                    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                    done.id = text(&json, "id").to_string();
                    let sent = op.body.as_ref();
                    done.enabled = json.get("isEnabled").and_then(Value::as_bool).or(sent.map(|r| r.enabled));
                    done.sequence =
                        json.get("sequence").and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()).or(sent.map(|r| r.sequence.max(1)));
                    // Made, and no word of what it is called: not made again (the next download
                    // shows it); said, not hidden.
                    if !valid_id(&done.id) {
                        done.result = RuleResult::Refused;
                        done.error = "the server made it and didn't say what it is called".into();
                    }
                }
                false
            }
            404 | 410 => {
                done.result = RuleResult::Gone;
                false
            }
            403 => {
                done.result = RuleResult::Denied;
                done.error = "this account may not change its rules".into();
                true
            }
            409 | 412 | 429 | 503 | 504 => {
                done.result = RuleResult::Failed;
                done.error = "Microsoft Graph is busy or the rule changed there meanwhile".into();
                true
            }
            status if status >= 500 => {
                done.result = RuleResult::Failed;
                done.error = format!("Microsoft Graph failed (status {status})");
                true
            }
            status => {
                done.result = RuleResult::Refused;
                done.error = match super::refusal(Doing::Rules, &reply, &[token, sent], mailcraft_time::now()) {
                    NetError::Proto(mailcraft_proto::ProtoError::Rejected(why)) => why,
                    e => e.to_string(),
                };
                if done.error.is_empty() {
                    done.error = format!("Microsoft Graph refused it (status {status})");
                }
                false
            }
        };
        uploaded.outcomes.push(done);
        if stop {
            break;
        }
    }
    for outcome in &mut uploaded.outcomes {
        outcome.tidy();
    }
    log::debug!("rules: {} changes told, sign-in refused: {}", uploaded.outcomes.len(), uploaded.token_refused);
    uploaded
}
