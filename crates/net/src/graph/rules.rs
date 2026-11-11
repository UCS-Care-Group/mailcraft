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
//! ([`MAX_FOLDER_LOOKUPS`] at most). A rule whose folder gets no answer waits for the next read.
//!
//! What comes back is hostile JSON like everything else from a server: the list is capped, an
//! entry that is no rule is skipped, and [`RemoteRule::tidy`] cuts the rest to size.

use std::collections::HashMap;

use mailcraft_model::remote_rules::{
    MAX_RULE_OPS, MAX_SERVER_RULES, RemoteRule, RuleOp, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUpload, RulesUploaded,
};
use mailcraft_model::{Action, Condition, Importance, Sensitivity};
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
/// And its copy.
const COPY: &str = "copyToFolder";

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
    for folder in download.rules.iter().flat_map(|r| [&r.move_to, &r.copy_to]) {
        if !folder.is_empty() && !folders.contains(folder) && !named.contains_key(folder) {
            if named.len() < MAX_FOLDER_LOOKUPS {
                named.insert(folder.clone(), None);
            } else {
                // Not looked up, so nothing is known of this rule's folder: the read isn't whole.
                download.complete = false;
            }
        }
    }
    let mut no_answer: Vec<String> = Vec::new();
    for (asked, found) in &mut named {
        match folder_id(&graph, asked)? {
            Looked::Is(id) => *found = Some(id),
            Looked::NotKnown => {}
            Looked::NoAnswer => no_answer.push(asked.clone()),
        }
    }
    // Nothing can be told of such a rule now, not even that it isn't one made here: it waits for
    // the next read, which is not whole without it.
    if !no_answer.is_empty() {
        download.rules.retain(|rule| !no_answer.contains(&rule.move_to) && !no_answer.contains(&rule.copy_to));
        download.complete = false;
    }
    for rule in &mut download.rules {
        for folder in [&mut rule.move_to, &mut rule.copy_to] {
            if let Some(Some(id)) = named.get(folder.as_str()) {
                *folder = id.clone();
            }
        }
    }
    log::debug!("rules: {} of them, {unread} that couldn't be read, complete: {}", download.rules.len(), download.complete);
    Ok(download)
}

/// What a look-up of a rule's folder came to.
enum Looked {
    /// The immutable id it goes by.
    Is(String),
    /// The server won't say: the rule names a folder that isn't known.
    NotKnown,
    /// Not to be had now (a time-out, the server busy or failing).
    NoAnswer,
}

/// What Graph says of the folder it knows as `asked`.
fn folder_id(graph: &Graph<'_>, asked: &str) -> Result<Looked, NetError> {
    if !valid_id(asked) {
        return Ok(Looked::NotKnown);
    }
    let url = graph.url(&format!("{FOLDERS}/{}?{}", http::encode(asked), http::form(&[("$select", "id")])));
    match graph.ask(&url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE) {
        Ok(reply) if reply.status == 200 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            let id = text(&json, "id");
            Ok(if valid_id(id) { Looked::Is(id.to_string()) } else { Looked::NotKnown })
        }
        Ok(reply) if reply.status == 401 => Err(graph.refusal(Doing::Rules, &reply, "")),
        Ok(reply) if reply.status == 429 || reply.status >= 500 => Ok(Looked::NoAnswer),
        Ok(_) => Ok(Looked::NotKnown),
        Err(e) if e.is_refused_sign_in() => Err(e),
        Err(_) => Ok(Looked::NoAnswer),
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

/// The words of a list of them (Graph's `senderContains: ["x", "y"]`), any of which is enough;
/// none when it isn't a list of texts.
fn words_of(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    let words: Vec<String> = list.iter().map(|w| w.as_str().map(str::to_string)).collect::<Option<_>>()?;
    (!words.is_empty()).then_some(words)
}

/// The addresses of a list of recipients (Graph's `fromAddresses`), each by its address alone;
/// none when one has none.
fn addresses_of(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    let addresses: Vec<String> = list
        .iter()
        .map(|r| r.get("emailAddress").and_then(|e| e.get("address")).and_then(Value::as_str).map(|a| a.trim().to_string()).filter(|a| !a.is_empty()))
        .collect::<Option<_>>()?;
    (!addresses.is_empty()).then_some(addresses)
}

/// A rule's condition of the server's name `name`, when this version knows it whole.
fn condition_of(name: &str, value: &Value) -> Option<Condition> {
    let yes = value.as_bool() == Some(true);
    match name {
        "senderContains" => words_of(value).map(Condition::FromContains),
        "recipientContains" => words_of(value).map(Condition::ToContains),
        "subjectContains" => words_of(value).map(Condition::SubjectContains),
        "bodyContains" => words_of(value).map(Condition::BodyContains),
        "bodyOrSubjectContains" => words_of(value).map(Condition::SubjectOrBodyContains),
        "headerContains" => words_of(value).map(Condition::HeaderContains),
        "categories" => words_of(value).map(Condition::Category),
        "fromAddresses" => addresses_of(value).map(Condition::From),
        "sentToAddresses" => addresses_of(value).map(Condition::SentTo),
        "hasAttachments" if yes => Some(Condition::HasAttachment),
        "importance" => value.as_str().and_then(Importance::parse).map(Condition::Importance),
        "sensitivity" => value.as_str().and_then(Sensitivity::parse).map(Condition::Sensitivity),
        "withinSizeRange" => {
            let size = |key: &str| value.get(key).and_then(Value::as_u64);
            match (size("minimumSize"), size("maximumSize")) {
                (Some(min), Some(max)) if min <= max => Some(Condition::SizeBetween { min, max }),
                _ => None,
            }
        }
        "isMeetingRequest" if yes => Some(Condition::MeetingRequest),
        "isAutomaticReply" if yes => Some(Condition::AutomaticReply),
        // Flagged for any action: Outlook's flags for one action (call, reply…) aren't MailCraft's.
        "messageActionFlag" if value.as_str() == Some("any") => Some(Condition::Flagged),
        "sentOnlyToMe" if yes => Some(Condition::SentOnlyToMe),
        "sentToMe" if yes => Some(Condition::ToMe),
        "sentCcMe" if yes => Some(Condition::CcMe),
        "sentToOrCcMe" if yes => Some(Condition::ToOrCcMe),
        "notSentToMe" if yes => Some(Condition::NotToMe),
        _ => None,
    }
}

/// The conditions of a rule's `conditions` or `exceptions`, with the names of those this
/// version doesn't know, each after `prefix`.
fn conditions_of(entry: &Value, key: &str, prefix: &str, unknown: &mut Vec<String>) -> Vec<Condition> {
    let empty = Map::new();
    let given = entry.get(key);
    // Something other than an object can't be read: the rule has a part not known here, so that
    // it isn't run here without it.
    if given.is_some_and(|v| !v.is_object() && said(v)) {
        unknown.push(key.to_string());
    }
    let parts = given.and_then(Value::as_object).unwrap_or(&empty);
    let mut conditions = Vec::new();
    for (name, value) in parts.iter().filter(|(name, value)| !name.starts_with('@') && said(value)) {
        match condition_of(name, value) {
            Some(c) => conditions.push(c),
            None => unknown.push(format!("{prefix}{name}")),
        }
    }
    conditions
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
    rule.conditions = conditions_of(entry, "conditions", "", &mut rule.unknown);
    // A rule of the server without conditions acts on every message: not one a rule here is.
    if rule.conditions.is_empty() && rule.unknown.is_empty() {
        rule.unknown.push("conditions".into());
    }
    rule.exceptions = conditions_of(entry, "exceptions", "exceptions.", &mut rule.unknown);
    let empty = Map::new();
    let actions = entry.get("actions").and_then(Value::as_object).unwrap_or(&empty);
    for (name, value) in actions.iter().filter(|(name, value)| !name.starts_with('@') && said(value)) {
        let mut take = |a: Action| {
            rule.actions.push(a);
            true
        };
        let known = match name.as_str() {
            "assignCategories" => match value.as_array() {
                Some(names) if names.iter().all(Value::is_string) => {
                    rule.actions.extend(names.iter().filter_map(Value::as_str).map(|c| Action::Categorize(c.to_string())));
                    true
                }
                _ => false,
            },
            "markAsRead" => take(Action::MarkRead),
            "markImportance" => value.as_str().and_then(Importance::parse).is_some_and(|i| take(Action::SetImportance(i))),
            "delete" => take(Action::Delete),
            MOVE | COPY => match value.as_str() {
                Some(folder) if valid_id(folder) => {
                    let to = if name == MOVE { &mut rule.move_to } else { &mut rule.copy_to };
                    *to = folder.to_string();
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

/// A condition as the server has it: its name and value. `Err` for one it has no such
/// condition for.
fn predicate(condition: &Condition) -> Result<(&'static str, Value), &'static str> {
    let people = |list: &[String]| Value::Array(list.iter().map(|a| json!({"emailAddress": {"address": a}})).collect());
    Ok(match condition {
        Condition::FromContains(t) => ("senderContains", json!(t)),
        Condition::ToContains(t) => ("recipientContains", json!(t)),
        Condition::SubjectContains(t) => ("subjectContains", json!(t)),
        Condition::BodyContains(t) => ("bodyContains", json!(t)),
        Condition::SubjectOrBodyContains(t) => ("bodyOrSubjectContains", json!(t)),
        Condition::HeaderContains(t) => ("headerContains", json!(t)),
        Condition::Category(t) => ("categories", json!(t)),
        Condition::From(p) => ("fromAddresses", people(p)),
        Condition::SentTo(p) => ("sentToAddresses", people(p)),
        Condition::HasAttachment => ("hasAttachments", json!(true)),
        Condition::Importance(i) => ("importance", json!(i.code())),
        Condition::Sensitivity(s) => ("sensitivity", json!(s.code())),
        Condition::SizeBetween { min, max } => ("withinSizeRange", json!({"minimumSize": min, "maximumSize": max})),
        Condition::MeetingRequest => ("isMeetingRequest", json!(true)),
        Condition::AutomaticReply => ("isAutomaticReply", json!(true)),
        Condition::Flagged => ("messageActionFlag", json!("any")),
        Condition::SentOnlyToMe => ("sentOnlyToMe", json!(true)),
        Condition::ToMe => ("sentToMe", json!(true)),
        Condition::CcMe => ("sentCcMe", json!(true)),
        Condition::ToOrCcMe => ("sentToOrCcMe", json!(true)),
        Condition::NotToMe => ("notSentToMe", json!(true)),
        Condition::ReceivedBetween { .. } => return Err("the server's rules have no condition on the day mail came"),
    })
}

/// The server's `conditions`: one of each kind, all of which must hold.
fn conditions_body(conditions: &[Condition]) -> Result<Map<String, Value>, &'static str> {
    let mut body = Map::new();
    for condition in conditions {
        let (name, value) = predicate(condition)?;
        if body.insert(name.into(), value).is_some() {
            return Err("the server's rules have one condition of each kind");
        }
    }
    Ok(body)
}

/// The server's `exceptions`, any of which leaves a message alone: two of a kind that look for
/// words or people are one list of them; two that are the same are one.
fn exceptions_body(exceptions: &[Condition]) -> Result<Map<String, Value>, &'static str> {
    let mut body = Map::new();
    for exception in exceptions {
        let (name, value) = predicate(exception)?;
        match body.get_mut(name) {
            None => {
                body.insert(name.into(), value);
            }
            Some(had) if *had == value => {}
            Some(Value::Array(had)) if exception.words().is_some() => {
                for v in value.as_array().into_iter().flatten() {
                    if !had.contains(v) {
                        had.push(v.clone());
                    }
                }
            }
            Some(_) => return Err("the server's rules have one exception of each kind"),
        }
    }
    Ok(body)
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
    if rule.actions.is_empty() && rule.move_to.is_empty() && rule.copy_to.is_empty() {
        return Err("a rule that does nothing isn't made on the server");
    }
    let conditions = conditions_body(&rule.conditions)?;
    let exceptions = exceptions_body(&rule.exceptions)?;
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
            // Graph's delete past the recoverable items is asked of no server (R3).
            Action::DeleteForGood => return Err("a rule that deletes mail for good stays on this computer"),
            Action::Flag | Action::FlagFor(_) | Action::ClearFlag => return Err("the server's rules can't flag mail"),
            Action::ClearCategories => return Err("the server's rules can't clear categories"),
            Action::MoveTo(_) | Action::CopyTo(_) => return Err("a folder a rule puts mail in is named by the server's id"),
        }
    }
    if !categories.is_empty() {
        actions.insert("assignCategories".into(), json!(categories));
    }
    for (name, folder) in [(MOVE, &rule.move_to), (COPY, &rule.copy_to)] {
        if !folder.is_empty() {
            if !valid_id(folder) {
                return Err("that isn't a folder the server could have");
            }
            actions.insert(name.into(), json!(folder));
        }
    }
    actions.insert("stopProcessingRules".into(), json!(rule.stop));
    let mut body = json!({
        "displayName": rule.name,
        "sequence": rule.sequence.max(1),
        "isEnabled": rule.enabled,
        "conditions": conditions,
        "actions": actions,
    });
    if !exceptions.is_empty() {
        body["exceptions"] = Value::Object(exceptions);
    }
    Ok(body)
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
                    NetError::Proto(mailcraft_proto::ProtoError::Rejected(why)) => why.to_string(),
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
