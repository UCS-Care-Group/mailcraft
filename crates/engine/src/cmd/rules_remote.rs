// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Microsoft account, kept on its server (#566, Graph's `messageRules`).
//!
//! The engine does no I/O: a host with sockets (`mailcraft_net::graph::rules`) asks what is to
//! be told to the server (`sync.ruleChanges`, [`rule_upload`]), says what the server made of it
//! (`sync.applyRuleChanges`, [`apply_rule_outcomes`]) and brings the server's rules
//! (`sync.applyRules`, [`merge_rules`]).
//!
//! **What a rule remembers.** A rule the server has keeps its id there and, beside whether it is
//! switched on and where it stands, the same two as the server last had them
//! ([`mailcraft_model::ServerRule`]). What was changed here since is the difference, and only that
//! is sent: a rule is never rewritten on the server, so one with parts this version doesn't know
//! stays as it was made. A rule to go to the server and not there yet is sent whole, after those
//! there; one the server refuses stays on this computer, and says why.
//!
//! **A lost answer.** A rule whose creation went out and got no answer (the connection dropped,
//! or MailCraft was closed) may be on the server already: it is marked
//! ([`mailcraft_model::Rule::unanswered`], from when its job starts, [`mark_sent`]) and not sent
//! again until the server's rules have been read whole. A rule there with the same name, conditions
//! and actions is then taken as it; when there is none, it is sent again. Only such a rule is
//! taken for one of the server's that looks the same: a rule never sent is never taken for one
//! made elsewhere.
//!
//! **Conflicts.** A download is the server's word on everything but what was changed here and not
//! sent yet. A rule the server no longer has goes from here too, unless it was changed here
//! since: then it is made again. A rule deleted here is remembered ([`mailcraft_model::RuleGone`])
//! until the server has been told, and is not brought back by a download that still shows it.
//!
//! Everything from outside is hostile: ids and text are cut by [`RemoteRule::tidy`], counts are
//! capped, and a download or an answer for another account is refused whole.

use std::collections::{HashMap, HashSet};

use mailcraft_model::remote_rules::{
    MAX_RULE_OPS, MAX_SERVER_RULES, RemoteRule, RuleOp, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUpload, RulesUploaded,
};
use mailcraft_model::{Action, Condition, Id, Keep, MAX_RULE_GONE, MAX_RULES, Mailbox, Rule, RuleGone, ServerRule};
use serde_json::{Value, json};

use super::remote::{Merged, owner};
use super::rules::server_cannot;
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

const MAX_OUTCOMES: usize = 1_000;
const MAX_SHOWN: usize = 200;
/// What a move to a folder is called among the parts of a server's rule, when it goes to a folder
/// this mailbox doesn't have.
const MOVE_PART: &str = "moveToFolder";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.ruleChanges", "Rule Changes to Send", "Send / Receive", rule_changes_command)
            .pure()
            .params(r#"{"account": number} — what that Microsoft account's server is to be told of rules made, switched on or off, moved or deleted here: {"account", "email", "ops": [{"kind": "create"|"update"|"delete", "rule": number, "id"?: string, "enabled"?: bool, "sequence"?: number, "body"?: RemoteRule}]}"#),
        CommandSpec::new("sync.applyRuleChanges", "Apply Sent Rule Changes", "Send / Receive", apply_rule_changes_command)
            .pure()
            .params(r#"{"result": RulesUploaded} — what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"kind", "rule": number, "id"?: string, "result": "done"|"gone"|"denied"|"failed"|"refused", "enabled"?: bool, "sequence"?: number, "error"?: string}]} — answers {"done", "failed", "messages": [string]}"#),
        CommandSpec::new("sync.applyRules", "Apply Downloaded Rules", "Send / Receive", apply_rules_command)
            .pure()
            .params(r#"{"download": RulesDownload} — what a host brought from the account's server: {"account": number, "email"?: string, "complete": bool, "rules": [{"id", "name", "sequence", "enabled", "readOnly", "error", "conditions", "actions", "moveTo", "stop", "unknown": [string]}]} — answers {"added", "updated", "removed", "unchanged", "skipped", "complete"}"#),
    ]
}

/// The accounts whose rules are kept on their server.
pub fn rule_accounts(mailbox: &Mailbox) -> Vec<Id> {
    mailbox.accounts.iter().map(|a| a.id).filter(|id| mailbox.keeps_rules(*id)).collect()
}

/// How many changes wait to be told to `account`'s server.
pub fn rules_waiting(mailbox: &Mailbox, account: Id) -> usize {
    upload_of(mailbox, account).map_or(0, |ops| ops.len())
}

/// What the server calls the folder with the id `folder`, when it is one of `account`'s.
fn server_folder(mailbox: &Mailbox, account: Id, folder: Id) -> Option<String> {
    let f = mailbox.folder(folder).filter(|f| f.account == account)?;
    f.server.as_ref().map(|s| s.id.clone()).filter(|id| !id.is_empty())
}

/// The rule as it is sent to the server to be made there.
fn remote_of(mailbox: &Mailbox, account: Id, rule: &Rule, sequence: i32) -> RemoteRule {
    let mut move_to = String::new();
    let mut actions = Vec::new();
    for action in &rule.actions {
        match action {
            Action::MoveTo(folder) => move_to = server_folder(mailbox, account, *folder).unwrap_or_default(),
            other => actions.push(other.clone()),
        }
    }
    RemoteRule {
        id: String::new(),
        name: rule.name.clone(),
        sequence,
        enabled: rule.enabled,
        conditions: rule.conditions.clone(),
        actions,
        move_to,
        stop: rule.stop,
        ..Default::default()
    }
}

fn upload_of(mailbox: &Mailbox, account: Id) -> Result<Vec<RuleOp>, CmdError> {
    if !mailbox.keeps_rules(account) {
        return Err(bad("that account doesn't keep its rules on its server"));
    }
    let mut ops: Vec<RuleOp> = Vec::new();
    for gone in mailbox.rule_gone.iter().filter(|gone| gone.account == account) {
        ops.push(RuleOp { kind: RuleOpKind::Delete, id: gone.id.clone(), ..Default::default() });
    }
    let mine = || mailbox.rules.iter().filter(move |r| r.account == account);
    for rule in mine().filter(|r| r.changed_here() && !r.read_only()) {
        let Some(server) = rule.server.as_ref() else { continue };
        ops.push(RuleOp {
            kind: RuleOpKind::Update,
            rule: rule.id,
            id: server.id.clone(),
            enabled: (rule.enabled != server.synced_enabled).then_some(rule.enabled),
            sequence: (server.sequence != server.synced_sequence).then_some(server.sequence),
            body: None,
        });
    }
    // After every rule the server has.
    let last = mine().filter_map(|r| r.server.as_ref()).filter(|s| !s.id.is_empty()).map(|s| s.sequence.max(s.synced_sequence)).max().unwrap_or(0);
    let mut next = last.saturating_add(1);
    for rule in mine().filter(|r| r.waiting() && !r.unanswered && server_cannot(mailbox, r, account).is_none()) {
        ops.push(RuleOp { kind: RuleOpKind::Create, rule: rule.id, body: Some(remote_of(mailbox, account, rule, next)), ..Default::default() });
        next = next.saturating_add(1);
    }
    ops.truncate(MAX_RULE_OPS);
    Ok(ops)
}

/// Mark the rules whose creation goes out with `upload` (the window calls this as their job
/// starts): until an answer says what became of them, or the server's rules have been read
/// whole, they aren't sent again. Not an undo step.
pub fn mark_sent(s: &mut Session, upload: &RulesUpload) {
    let mut marked = false;
    for op in upload.ops.iter().filter(|op| op.kind == RuleOpKind::Create) {
        if let Some(rule) = s.mailbox.rules.iter_mut().find(|r| r.id == op.rule && r.account == upload.account && r.waiting()) {
            marked |= !rule.unanswered;
            rule.unanswered = true;
        }
    }
    if marked {
        s.mailbox_changed();
    }
}

/// Unmark the rules [`mark_sent`] marked for `upload` that went nowhere: every one when nothing
/// of it went out (the sign-in didn't cover changing the rules, or failed), and else those the
/// job stopped before, of which `told` says nothing (a sign-in refused, a busy server or a lost
/// connection ended it). They go with the next job.
pub fn not_sent(s: &mut Session, upload: &RulesUpload, told: &[RuleOutcome]) {
    let mut unmarked = false;
    let reached = |op: &RuleOp| told.iter().any(|o| o.kind == RuleOpKind::Create && o.rule == op.rule);
    for op in upload.ops.iter().filter(|op| op.kind == RuleOpKind::Create && !reached(op)) {
        if let Some(rule) = s.mailbox.rules.iter_mut().find(|r| r.id == op.rule && r.account == upload.account && r.waiting()) {
            unmarked |= rule.unanswered;
            rule.unanswered = false;
        }
    }
    if unmarked {
        s.mailbox_changed();
    }
}

/// What `account`'s server is to be told.
pub fn rule_upload(mailbox: &Mailbox, account: Id) -> Result<RulesUpload, CmdError> {
    let ops = upload_of(mailbox, account)?;
    let a = mailbox.account(account).ok_or_else(|| fail("no such account"))?;
    Ok(RulesUpload { account, email: a.email.trim().to_string(), ops })
}

/// A server's rule as the mailbox keeps it: its conditions, its actions with the move to a folder
/// of the mailbox, and the parts that aren't known here.
fn local_of(mailbox: &Mailbox, account: Id, remote: &RemoteRule) -> (Vec<Action>, Vec<String>) {
    let mut actions = remote.actions.clone();
    let mut unknown = remote.unknown.clone();
    if !remote.move_to.is_empty() {
        let folder = mailbox.folders.iter().find(|f| f.account == account && f.server.as_ref().is_some_and(|s| s.id == remote.move_to));
        match folder {
            Some(f) => actions.insert(0, Action::MoveTo(f.id)),
            None if !unknown.iter().any(|u| u == MOVE_PART) => unknown.push(MOVE_PART.to_string()),
            None => {}
        }
    }
    (actions, unknown)
}

/// Whether `here` and `there` hold the same parts, in any order (a server gives them in its
/// own), by `same`.
fn same_parts<T>(here: &[T], there: &[T], same: impl Fn(&T, &T) -> bool) -> bool {
    let mut left: Vec<&T> = there.iter().collect();
    here.len() == there.len()
        && here.iter().all(|a| match left.iter().position(|b| same(a, b)) {
            Some(i) => {
                left.swap_remove(i);
                true
            }
            None => false,
        })
}

/// The words of two conditions are the same but for their letters' case: the server keeps the
/// text of a condition in capitals.
fn same_words(here: &[Condition], there: &[Condition]) -> bool {
    let words = |c: &Condition| match c {
        Condition::FromContains(t) | Condition::ToContains(t) | Condition::SubjectContains(t) | Condition::BodyContains(t) => Some(t.to_lowercase()),
        _ => None,
    };
    same_parts(here, there, |a, b| std::mem::discriminant(a) == std::mem::discriminant(b) && words(a) == words(b) && (words(a).is_some() || a == b))
}

/// The same with the categories it gives.
fn same_actions(here: &[Action], there: &[Action]) -> bool {
    same_parts(here, there, |a, b| match (a, b) {
        (Action::Categorize(x), Action::Categorize(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    })
}

/// Merge the rules a server has into the mailbox (see the module). Nothing is changed when this
/// returns an error. Doing it twice changes nothing the second time.
pub fn merge_rules(mailbox: &mut Mailbox, download: &RulesDownload) -> Result<Merged, CmdError> {
    let account = owner(mailbox, download.account, &download.email)?.id;
    if !mailbox.keeps_rules(account) {
        return Err(bad("that account doesn't keep its rules on its server"));
    }
    let mut merged = Merged::default();
    let told: HashSet<String> = mailbox.rule_gone.iter().filter(|g| g.account == account).map(|g| g.id.clone()).collect();
    let mut known: HashMap<String, Id> = HashMap::new();
    for rule in mailbox.rules.iter().filter(|r| r.account == account && r.on_server()) {
        if let Some(server) = rule.server.as_ref() {
            known.entry(server.id.clone()).or_insert(rule.id);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for remote in download.rules.iter().take(MAX_SERVER_RULES) {
        let mut remote = remote.clone();
        remote.tidy();
        if remote.id.is_empty() || !seen.insert(remote.id.clone()) || told.contains(&remote.id) {
            merged.skipped += 1;
            continue;
        }
        let (actions, unknown) = local_of(mailbox, account, &remote);
        let fresh = |sequence: i32| ServerRule {
            id: remote.id.clone(),
            sequence,
            synced_enabled: remote.enabled,
            synced_sequence: remote.sequence,
            unknown: unknown.clone(),
            read_only: remote.read_only,
            error: remote.error,
        };
        let at = known.get(&remote.id).and_then(|id| mailbox.rules.iter().position(|r| r.id == *id));
        if let Some(i) = at {
            let Some(old) = mailbox.rules.get(i) else { continue };
            let mut rule = old.clone();
            let had = old.server.clone().unwrap_or_default();
            // What was changed here and not sent yet stays; the rest is the server's.
            let enabled = if old.enabled != had.synced_enabled { old.enabled } else { remote.enabled };
            let sequence = if had.sequence != had.synced_sequence { had.sequence } else { remote.sequence };
            rule.enabled = enabled;
            rule.server = Some(fresh(sequence));
            rule.name = if rule.name.eq_ignore_ascii_case(&remote.name) { rule.name } else { remote.name.clone() };
            if !same_words(&rule.conditions, &remote.conditions) {
                rule.conditions = remote.conditions.clone();
            }
            if !same_actions(&rule.actions, &actions) {
                rule.actions = actions;
            }
            rule.stop = remote.stop;
            rule.keep = Keep::Server;
            rule.refused.clear();
            if rule == *old {
                merged.unchanged += 1;
            } else if let Some(slot) = mailbox.rules.get_mut(i) {
                *slot = rule;
                merged.updated += 1;
            }
            continue;
        }
        // A rule made here whose answer was lost (the server made it, the word didn't come) is the
        // same one: taken in, not made twice. Only one that went out without an answer is.
        let twin = mailbox.rules.iter().position(|r| {
            r.account == account
                && r.waiting()
                && r.unanswered
                && r.name.eq_ignore_ascii_case(&remote.name)
                && same_words(&r.conditions, &remote.conditions)
                && same_actions(&r.actions, &actions)
                && r.stop == remote.stop
        });
        if let Some(i) = twin
            && let Some(rule) = mailbox.rules.get_mut(i)
        {
            rule.enabled = remote.enabled;
            rule.server = Some(fresh(remote.sequence));
            rule.unanswered = false;
            merged.updated += 1;
            continue;
        }
        if mailbox.rules.len() >= MAX_RULES {
            merged.skipped += 1;
            continue;
        }
        let server = fresh(remote.sequence);
        let id = mailbox.alloc_id();
        mailbox.rules.push(Rule {
            id,
            name: remote.name.clone(),
            enabled: remote.enabled,
            conditions: remote.conditions.clone(),
            actions,
            stop: remote.stop,
            account,
            keep: Keep::Server,
            server: Some(server),
            ..Default::default()
        });
        merged.added += 1;
    }
    // Read whole, the server doesn't have the rules whose answer was lost: they are sent again.
    if download.complete && download.rules.len() <= MAX_SERVER_RULES {
        for rule in mailbox.rules.iter_mut().filter(|r| r.account == account && r.unanswered && r.waiting()) {
            rule.unanswered = false;
            merged.updated += 1;
        }
    }
    // What the server no longer has: only when the download says it showed every rule there is.
    if download.complete && download.rules.len() <= MAX_SERVER_RULES {
        let missing: Vec<Id> = known.iter().filter(|(remote, _)| !seen.contains(*remote)).map(|(_, id)| *id).collect();
        for id in missing {
            let Some(i) = mailbox.rules.iter().position(|r| r.id == id) else { continue };
            let again = mailbox.rules.get(i).is_some_and(|r| r.changed_here() && r.known());
            if again {
                // Changed here since: made again rather than lost.
                if let Some(rule) = mailbox.rules.get_mut(i) {
                    rule.server = None;
                    merged.updated += 1;
                }
            } else {
                mailbox.rules.remove(i);
                merged.removed += 1;
            }
        }
    }
    in_server_order(mailbox, account);
    Ok(merged)
}

/// Put the rules the server of `account` has in the order they stand there, in the places they
/// take among the mailbox's rules.
fn in_server_order(mailbox: &mut Mailbox, account: Id) {
    let slots: Vec<usize> = mailbox.rules.iter().enumerate().filter(|(_, r)| r.account == account && r.on_server()).map(|(i, _)| i).collect();
    let mut rules: Vec<Rule> = slots.iter().filter_map(|i| mailbox.rules.get(*i).cloned()).collect();
    rules.sort_by_key(|r| r.server.as_ref().map_or(0, |s| s.sequence));
    for (slot, rule) in slots.into_iter().zip(rules) {
        if let Some(place) = mailbox.rules.get_mut(slot) {
            *place = rule;
        }
    }
}

/// Take in what a server made of the changes it was told. Not an undo step.
pub fn apply_rule_outcomes(mailbox: &mut Mailbox, uploaded: &RulesUploaded) -> Result<Merged, CmdError> {
    let account = owner(mailbox, uploaded.account, &uploaded.email)?.id;
    if !mailbox.keeps_rules(account) {
        return Err(bad("that account doesn't keep its rules on its server"));
    }
    let mut merged = Merged::default();
    let mut failed = 0usize;
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let why = if outcome.error.is_empty() { String::new() } else { format!(": {}", outcome.error) };
        let at = mailbox.rules.iter().position(|r| r.id == outcome.rule && r.account == account);
        let name = at.and_then(|i| mailbox.rules.get(i)).map(|r| r.name.clone()).unwrap_or_default();
        match (outcome.kind, outcome.result) {
            (RuleOpKind::Delete, RuleResult::Done | RuleResult::Gone | RuleResult::Refused) => {
                mailbox.rule_gone.retain(|g| !(g.account == account && g.id == outcome.id));
                if outcome.result == RuleResult::Refused {
                    merged.say(format!("A deleted rule couldn't be deleted from the server{why}"));
                }
                merged.removed += 1;
            }
            (RuleOpKind::Create, RuleResult::Done) => {
                let (enabled, sequence) = (outcome.enabled.unwrap_or(true), outcome.sequence.unwrap_or(0));
                // An id another rule here has already (a server that says so is not believed:
                // deleting the one would delete the other there) is no answer.
                let taken = mailbox.rules.iter().any(|r| r.account == account && r.server.as_ref().is_some_and(|s| s.id == outcome.id));
                if outcome.id.is_empty() || taken {
                    if let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i)) {
                        rule.unanswered = true;
                    }
                    failed += 1;
                } else if let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i)) {
                    rule.server = Some(ServerRule {
                        id: outcome.id.clone(),
                        sequence,
                        synced_enabled: enabled,
                        synced_sequence: sequence,
                        ..Default::default()
                    });
                    rule.refused.clear();
                    rule.unanswered = false;
                    merged.added += 1;
                } else if mailbox.rule_gone.len() < MAX_RULE_GONE {
                    // Deleted here while it was being made: the server is told to delete it.
                    mailbox.rule_gone.push(RuleGone { account, id: outcome.id.clone() });
                }
            }
            (RuleOpKind::Update, RuleResult::Done) => {
                if let Some(server) = at.and_then(|i| mailbox.rules.get_mut(i)).and_then(|r| r.server.as_mut()) {
                    if let Some(enabled) = outcome.enabled {
                        server.synced_enabled = enabled;
                    }
                    if let Some(sequence) = outcome.sequence {
                        server.synced_sequence = sequence;
                    }
                    merged.updated += 1;
                }
            }
            // The server no longer has it: made again, as it is here, when all of it is known.
            (RuleOpKind::Update, RuleResult::Gone) => {
                if let Some(i) = at {
                    if mailbox.rules.get(i).is_some_and(Rule::known) {
                        if let Some(rule) = mailbox.rules.get_mut(i) {
                            rule.server = None;
                        }
                    } else {
                        mailbox.rules.remove(i);
                    }
                    merged.updated += 1;
                }
            }
            // Refused: the rule is as the server has it again, and that is said.
            (RuleOpKind::Update, RuleResult::Refused) => {
                if let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i))
                    && let Some(server) = rule.server.as_mut()
                {
                    rule.enabled = server.synced_enabled;
                    server.sequence = server.synced_sequence;
                }
                merged.say(format!("The server didn't take the change to the rule \"{name}\"{why}"));
                failed += 1;
            }
            (RuleOpKind::Create, RuleResult::Refused) => {
                if let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i)) {
                    rule.keep = Keep::Here;
                    rule.unanswered = false;
                    rule.refused = if outcome.error.is_empty() { "it was refused".to_string() } else { outcome.error.clone() };
                }
                merged.say(format!("The server didn't take the rule \"{name}\"; it is kept on this computer{why}"));
                failed += 1;
            }
            (kind, RuleResult::Denied) => {
                // Refused before anything was made: it goes again once the account may.
                if kind == RuleOpKind::Create
                    && let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i))
                {
                    rule.unanswered = false;
                }
                merged.say(format!("This account may not change its rules on the server{why}"));
                failed += 1;
            }
            // No answer: the server may have made it.
            (RuleOpKind::Create, _) => {
                if let Some(rule) = at.and_then(|i| mailbox.rules.get_mut(i)) {
                    rule.unanswered = true;
                }
                failed += 1;
            }
            _ => failed += 1,
        }
    }
    // The rules just made stand where the server put them, after the others.
    in_server_order(mailbox, account);
    merged.skipped = failed;
    Ok(merged)
}

fn download_of<T: serde::de::DeserializeOwned>(v: &Value, name: &str) -> Result<T, CmdError> {
    let value = v.get(name).cloned().ok_or_else(|| bad(format!("`{name}` is required")))?;
    serde_json::from_value(value)
        .map_err(|e| bad(format!("`{name}`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))
}

/// Take in the rules a host downloaded ([`merge_rules`]). Not an undo step, as `sync.applyTasks`.
pub fn apply_rules(s: &mut Session, download: &RulesDownload) -> Result<Merged, CmdError> {
    let merged = merge_rules(&mut s.mailbox, download)?;
    if merged.changed() {
        s.mailbox_changed();
    }
    Ok(merged)
}

/// Take in what a host heard from the server ([`apply_rule_outcomes`]).
pub fn apply_rule_changes(s: &mut Session, uploaded: &RulesUploaded) -> Result<Merged, CmdError> {
    let merged = apply_rule_outcomes(&mut s.mailbox, uploaded)?;
    // What a server was told can't be undone there: going back to a mailbox from before would
    // undo it here only (the to-dos do the same).
    if uploaded.outcomes.iter().any(|o| matches!(o.result, RuleResult::Done | RuleResult::Gone)) {
        s.forget_history();
    }
    s.mailbox_changed();
    Ok(merged)
}

fn apply_rules_command(s: &mut Session, v: &Value) -> CmdResult {
    let download = download_of::<RulesDownload>(v, "download")?;
    let merged = apply_rules(s, &download)?;
    Ok(json!({
        "added": merged.added, "updated": merged.updated, "removed": merged.removed, "unchanged": merged.unchanged,
        "skipped": merged.skipped, "complete": download.complete,
    }))
}

fn apply_rule_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let uploaded = download_of::<RulesUploaded>(v, "result")?;
    let merged = apply_rule_changes(s, &uploaded)?;
    Ok(json!({"done": merged.added + merged.updated + merged.removed, "failed": merged.skipped, "messages": merged.messages}))
}

fn rule_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = crate::p::req_id(v, "account")?;
    let upload = rule_upload(&s.mailbox, account)?;
    serde_json::to_value(upload).map_err(|e| fail(e.to_string()))
}
