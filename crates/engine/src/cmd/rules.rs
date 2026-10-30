// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules: what to do with arriving mail.
//!
//! A rule is for one account's mail (#566); one from before rules had accounts is for every
//! account's, on this computer. A rule of a Microsoft account is kept on its server when the
//! server can do all of it ([`server_cannot`]): it is then sent there (`rules_remote`) and runs
//! there, whether MailCraft is open or not. The rules of an account are in two lists, as they
//! run: those of its server first, then those of this computer; Move Up and Move Down move a rule
//! within its list.

use mailcraft_model::{Action, Condition, Id, Importance, Keep, Mailbox, Rule, RuleGone, clean_name};
use serde_json::{Value, json};

use super::{bad, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

use mailcraft_model::MAX_RULES;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.add", "Create Rule", "Home › Move", add).params(
            r#"{"name": string, "conditions": [{"type": "fromContains"|"toContains"|"subjectContains"|"bodyContains", "value": string} | {"type": "hasAttachment"} | {"type": "importance", "value": "high"|"normal"|"low"}], "actions": [{"type": "moveTo", "value": folder id} | {"type": "categorize", "value": string} | {"type": "flag"} | {"type": "markRead"} | {"type": "setImportance", "value": string} | {"type": "delete"}], "stop"?: bool, "account"?: number (whose arriving mail it is for; without it, every account's, and kept on this computer), "server"?: bool (false keeps it on this computer; true refuses a rule the server can't keep; default: on the server when it can)} — on a Microsoft account the rule is kept on its server when the server can do all of it (it then runs there, whether MailCraft is open or not; the sender's and recipients' words are looked for in their addresses alone, as the server does); answers {"id", "keep": "server"|"here"}"#,
        ),
        CommandSpec::new("rules.remove", "Delete Rule", "Home › Move", remove).params(r#"{"id": number} — a rule its server has is deleted there too"#),
        CommandSpec::new("rules.enable", "Enable Rule", "Home › Move", enable).params(r#"{"id": number, "value"?: bool (default: toggle)}"#),
        CommandSpec::new("rules.list", "Manage Rules", "Home › Move", list).pure().params(
            r#"{"account"?: number} — the rules (of that account and those for every account, in the order they run); each says where it is kept: "server", "waiting" (to go to the server), "here" (with why when it can't go there) or "everyAccount""#,
        ),
        CommandSpec::new("rules.run", "Run Rules Now", "Folder › Clean Up", run).params(r#"{"folder"?: number} (default: the folder in view)"#),
        CommandSpec::new("rules.fromMessage", "Always Move Messages From", "Home › Move", from_message)
            .params(r#"{"folder": number, "id"?: number, "server"?: bool (as for `rules.add`)} — a rule moving this sender's mail to the folder, run at once; the window opens the New rule builder filled in instead (`ui.dialog` "rules" with "folder")"#)
            .selection(needs_message),
        CommandSpec::new("rules.move", "Move Rule", "Home › Move", move_rule).params(
            r#"{"id": number, "direction": "up"|"down", "account"?: number (the list it is moved in, for a rule for every account; default: the account of the folder in view)} — within its list: the rules its account's server keeps, or those of this computer"#,
        ),
        CommandSpec::new("rules.keepOnServer", "Keep Rule on Server", "Home › Move", keep_on_server).params(
            r#"{"id": number, "account"?: number (for a rule for every account: the Microsoft account whose server is to keep it, which it is then for alone)} — a rule kept on this computer goes to its Microsoft account's server at the next look, when the server can do all of it"#,
        ),
    ]
}

/// Why a rule can't be kept on an account's server (#566). Graph's rules know no flag; move mail
/// only to a folder of the same mailbox, and to one at most; and take several texts of one kind
/// as "any of them", where MailCraft's conditions all have to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotOnServer {
    /// The account's server doesn't keep rules.
    NoServer,
    Flag,
    /// The folder the rule moves mail to is on this computer.
    FolderHere,
    /// The folder is another account's.
    FolderElsewhere,
    /// The folder isn't on the account's server yet, or is gone.
    FolderNotThere,
    TwoMoves,
    TwoOfAKind,
    TwoImportances,
}

impl NotOnServer {
    /// Why, in words.
    pub fn said(self) -> &'static str {
        match self {
            NotOnServer::NoServer => "this account's server doesn't keep rules",
            NotOnServer::Flag => "the server can't flag mail",
            NotOnServer::FolderHere => "the folder it moves mail to is on this computer",
            NotOnServer::FolderElsewhere => "the folder it moves mail to is in another account",
            NotOnServer::FolderNotThere => "the folder it moves mail to isn't on the server",
            NotOnServer::TwoMoves => "it moves mail to more than one folder",
            NotOnServer::TwoOfAKind => "it has two conditions of one kind, and the server would take either of them rather than both",
            NotOnServer::TwoImportances => "it sets more than one importance",
        }
    }
}

/// Why `rule` can't be kept on the server of `account`, or `None` when it can.
pub fn server_cannot(mb: &Mailbox, rule: &Rule, account: Id) -> Option<NotOnServer> {
    if !mb.keeps_rules(account) {
        return Some(NotOnServer::NoServer);
    }
    let kind = |c: &Condition| std::mem::discriminant(c);
    if rule.conditions.iter().enumerate().any(|(i, c)| rule.conditions.iter().skip(i + 1).any(|d| kind(c) == kind(d))) {
        return Some(NotOnServer::TwoOfAKind);
    }
    let mut moves = 0usize;
    let mut importance: Option<Importance> = None;
    for action in &rule.actions {
        match action {
            Action::Flag => return Some(NotOnServer::Flag),
            Action::MoveTo(folder) => {
                moves += 1;
                let Some(f) = mb.folder(*folder) else { return Some(NotOnServer::FolderNotThere) };
                if f.account != account {
                    let local = mb.account(f.account).is_some_and(|a| !a.kind.has_server());
                    return Some(if local { NotOnServer::FolderHere } else { NotOnServer::FolderElsewhere });
                }
                if f.server.as_ref().is_none_or(|s| s.id.is_empty()) {
                    return Some(NotOnServer::FolderNotThere);
                }
            }
            Action::SetImportance(i) => {
                if importance.is_some_and(|had| had != *i) {
                    return Some(NotOnServer::TwoImportances);
                }
                importance = Some(*i);
            }
            Action::Categorize(_) | Action::MarkRead | Action::Delete => {}
        }
    }
    if moves > 1 { Some(NotOnServer::TwoMoves) } else { None }
}

/// Where a rule is kept, as `rules.list` says it, and why it is kept here when it could be
/// expected on the server.
pub fn kept(mb: &Mailbox, r: &Rule) -> (&'static str, Option<String>) {
    if r.account == 0 {
        return ("everyAccount", None);
    }
    if r.on_server() {
        return ("server", None);
    }
    let why = server_cannot(mb, r, r.account);
    if r.keep == Keep::Server {
        return ("waiting", why.map(|w| w.said().to_string()));
    }
    if !r.refused.is_empty() {
        return ("here", Some(format!("the server didn't take it: {}", r.refused)));
    }
    match why {
        Some(NotOnServer::NoServer) | None => ("here", None),
        Some(why) => ("here", Some(why.said().to_string())),
    }
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
            refuse_send_queue(s, folder)?;
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

/// A rule never moves mail into an Outbox: the next Send/Receive would send it.
fn refuse_send_queue(s: &Session, folder: Id) -> Result<(), CmdError> {
    if s.mailbox.is_send_queue(folder) { Err(fail("a rule can't move mail to an Outbox: that would send it")) } else { Ok(()) }
}

/// The account whose own mail a folder's is: a shared mailbox's is its owner's (rules leave a
/// shared mailbox's mail alone, and are made for the account it is opened through).
fn own_account(mb: &Mailbox, account: Id) -> Id {
    match mb.account(account) {
        Some(a) if a.is_shared() => mb.owner_of(account).map_or(account, |o| o.id),
        _ => account,
    }
}

/// The account a new rule is for: the one `v` names, or none (for every account's mail, kept on
/// this computer, as rules were before they had accounts: what a script that names none asked
/// for then).
fn rule_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let Some(account) = p::id(v, "account") else { return Ok(0) };
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.is_shared() {
        return Err(fail("a rule is for an account's own mail, not a shared mailbox's"));
    }
    Ok(account)
}

/// The rule `v` asks for, with no id yet (shared by the command and the agent's question).
pub(crate) fn rule_from_params(s: &Session, v: &Value) -> Result<Rule, CmdError> {
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
    let account = rule_account(s, v)?;
    let mut rule = Rule { id: 0, name, enabled: true, conditions, actions, stop: p::bool(v, "stop").unwrap_or(false), account, ..Default::default() };
    place(s, &mut rule, v)?;
    Ok(rule)
}

/// Where a new rule is kept, by `v`'s `server`: on a server that can do all of it, as Outlook
/// keeps an Exchange account's rules, unless the owner keeps it on this computer.
fn place(s: &Session, rule: &mut Rule, v: &Value) -> Result<(), CmdError> {
    let account = rule.account;
    let cannot = if account == 0 { None } else { server_cannot(&s.mailbox, rule, account) };
    match (p::bool(v, "server"), cannot) {
        (Some(false), _) => {}
        (Some(true), _) if account == 0 => return Err(bad("say which account's server is to keep it (`account`)")),
        (Some(true), Some(why)) => return Err(CmdError::Failed(why.said().to_string())),
        (_, None) if account != 0 => rule.keep = Keep::Server,
        _ => {}
    }
    Ok(())
}

/// Why the rule `v` describes (the params of `rules.add`, finished or not) couldn't be kept on
/// its account's server, or `None` when it could: what the New rule form says before the rule
/// is made. Parts not filled in yet are left out.
pub fn server_cannot_draft(s: &Session, v: &Value) -> Option<NotOnServer> {
    let account = p::id(v, "account").filter(|a| s.mailbox.account(*a).is_some())?;
    let parts = |key: &str| v.get(key).and_then(Value::as_array).map(|a| a.iter().take(20).collect::<Vec<_>>()).unwrap_or_default();
    let conditions = parts("conditions").into_iter().filter_map(|c| condition(c).ok()).collect();
    let actions = parts("actions").into_iter().filter_map(|a| action(s, a).ok()).collect();
    let rule = Rule { conditions, actions, account, ..Default::default() };
    server_cannot(&s.mailbox, &rule, account)
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let mut rule = rule_from_params(s, v)?;
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    rule.id = id;
    let keep = if rule.keep == Keep::Server { "server" } else { "here" };
    s.mailbox.rules.push(rule);
    Ok(json!({"id": id, "keep": keep}))
}

fn changeable(s: &Session, id: Id) -> Result<usize, CmdError> {
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if s.mailbox.rules.get(i).is_some_and(Rule::read_only) {
        return Err(fail("the server doesn't let this rule be changed"));
    }
    Ok(i)
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let i = changeable(s, id)?;
    let rule = s.mailbox.rules.remove(i);
    // The server is told at the next look; until then, a download that still has it doesn't bring
    // it back.
    if let Some(server) = rule.server.filter(|s| !s.id.is_empty())
        && s.mailbox.rule_gone.len() < mailcraft_model::MAX_RULE_GONE
    {
        s.mailbox.rule_gone.push(RuleGone { account: rule.account, id: server.id });
    }
    Ok(json!({"deleted": id}))
}

fn enable(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let i = changeable(s, id)?;
    let r = s.mailbox.rules.get_mut(i).ok_or_else(|| failed!("no rule with id {id}", id))?;
    r.enabled = p::bool(v, "value").unwrap_or(!r.enabled);
    Ok(json!({"id": id, "enabled": r.enabled}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let mb = &s.mailbox;
    let order: Vec<usize> = match p::id(v, "account") {
        Some(account) => mb.rules_in_order(account),
        None => (0..mb.rules.len()).collect(),
    };
    let rules: Vec<Value> = order
        .into_iter()
        .filter_map(|i| mb.rules.get(i))
        .map(|r| {
            let (where_kept, why) = kept(mb, r);
            let mut entry = json!({
                "id": r.id, "name": r.name, "enabled": r.enabled, "stop": r.stop, "text": r.describe_as(mb.rule_reads_addresses(r)),
                "account": r.account, "kept": where_kept, "rule": r,
            });
            if let Some(why) = why {
                entry["why"] = json!(why);
            }
            if let Some(server) = r.server.as_ref() {
                if !server.unknown.is_empty() {
                    entry["unknownParts"] = json!(server.unknown);
                }
                if server.read_only {
                    entry["readOnly"] = json!(true);
                }
                if server.error {
                    entry["error"] = json!(true);
                }
            }
            entry
        })
        .collect();
    Ok(json!({"rules": rules}))
}

/// Run every enabled rule for the folder's account on the messages of `folder`, those its server
/// has among them; answers how many messages a rule acted on.
fn run_on(s: &mut Session, folder: Id) -> usize {
    let ids: Vec<Id> = s.mailbox.messages_in(folder).map(|m| m.id).collect();
    let account = s.mailbox.folder(folder).map_or(0, |f| f.account);
    // A rule that reads the body reads it whole: one kept on disk is brought in first (one that
    // can't be read says so in its place, and is matched as that).
    let reads_bodies =
        s.mailbox.rules.iter().any(|r| r.enabled && r.is_for(account) && r.conditions.iter().any(|c| matches!(c, Condition::BodyContains(_))));
    ids.into_iter()
        .filter(|id| {
            if reads_bodies {
                let _ = s.load_where(*id, |_| false);
            }
            !s.mailbox.run_rules(*id, true).is_empty()
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
    refuse_send_queue(s, folder)?;
    let mid = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(mid).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    let (from, source, account) = (m.from.clone(), m.folder, own_account(&s.mailbox, m.account));
    if from.email.trim().is_empty() {
        return Err(fail("the sender has no address to match"));
    }
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    let name = format!("{} to {target}", from.display());
    let mut rule = Rule {
        id,
        name: name.chars().take(mailcraft_model::MAX_NAME).collect(),
        enabled: true,
        conditions: vec![Condition::FromContains(from.email)],
        actions: vec![Action::MoveTo(folder)],
        stop: false,
        account,
        ..Default::default()
    };
    place(s, &mut rule, v)?;
    s.mailbox.rules.push(rule);
    let matched = run_on(s, source);
    Ok(json!({"id": id, "matched": matched}))
}

/// The list a rule is in, in the rules of `account`: those for its server, or those of this
/// computer (with the rules for every account).
fn same_list(r: &Rule, account: Id, server: bool) -> bool {
    let for_server = r.account == account && r.keep == Keep::Server;
    for_server == server && r.is_for(account)
}

fn move_rule(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let up = match p::req_str(v, "direction")? {
        "up" => true,
        "down" => false,
        _ => return Err(bad("`direction` is \"up\" or \"down\"")),
    };
    let i = changeable(s, id)?;
    let rule = s.mailbox.rules.get(i).cloned().ok_or_else(|| failed!("no rule with id {id}", id))?;
    let account = match rule.account {
        0 => p::id(v, "account").or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account)).map_or(0, |a| own_account(&s.mailbox, a)),
        own => own,
    };
    let server = rule.account == account && rule.keep == Keep::Server;
    let rules = &s.mailbox.rules;
    let next = if up {
        rules.iter().enumerate().take(i).rev().find(|(_, r)| same_list(r, account, server)).map(|(j, _)| j)
    } else {
        rules.iter().enumerate().skip(i + 1).find(|(_, r)| same_list(r, account, server)).map(|(j, _)| j)
    };
    let Some(j) = next else {
        return Ok(json!({"id": id, "moved": false}));
    };
    let other = rules.get(j).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if other.read_only() {
        return Err(fail("the server doesn't let the rule next to it be moved"));
    }
    if server && rule.on_server() != other.on_server() {
        return Err(fail("a rule waiting to go to the server goes after those there; it can be moved once it is there"));
    }
    s.mailbox.rules.swap(i, j);
    if server {
        renumber(&mut s.mailbox, account);
    }
    Ok(json!({"id": id, "moved": true}))
}

/// Give the rules the server of `account` has the places they now stand in: the same numbers as
/// before, in their new order (all of them afresh when two had the same).
pub(crate) fn renumber(mb: &mut Mailbox, account: Id) {
    let on: Vec<usize> = mb.rules.iter().enumerate().filter(|(_, r)| r.account == account && r.on_server()).map(|(i, _)| i).collect();
    let mut numbers: Vec<i32> = on.iter().filter_map(|i| mb.rules.get(*i)).filter_map(|r| r.server.as_ref().map(|s| s.sequence)).collect();
    numbers.sort_unstable();
    let distinct = numbers.windows(2).all(|w| w.first() < w.get(1));
    if !distinct {
        let first = numbers.first().copied().unwrap_or(1).max(0);
        numbers = (0..on.len()).map(|n| first.saturating_add(i32::try_from(n).unwrap_or(i32::MAX))).collect();
    }
    for (i, number) in on.into_iter().zip(numbers) {
        if let Some(server) = mb.rules.get_mut(i).and_then(|r| r.server.as_mut()) {
            server.sequence = number;
        }
    }
}

fn keep_on_server(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    let rule = s.mailbox.rules.get(i).cloned().ok_or_else(|| failed!("no rule with id {id}", id))?;
    if rule.keep == Keep::Server {
        return Ok(json!({"id": id, "account": rule.account, "keep": "server"}));
    }
    let account = match (rule.account, p::id(v, "account")) {
        (0, Some(account)) => account,
        (0, None) => {
            let servers: Vec<Id> = s.mailbox.accounts.iter().map(|a| a.id).filter(|a| s.mailbox.keeps_rules(*a)).collect();
            match servers.as_slice() {
                [only] => *only,
                [] => return Err(fail("no account here keeps its rules on its server")),
                _ => return Err(bad("say which account's server is to keep it (`account`)")),
            }
        }
        (own, Some(other)) if other != own => return Err(fail("a rule of one account can't go to another account's server")),
        (own, _) => own,
    };
    if let Some(why) = server_cannot(&s.mailbox, &rule, account) {
        return Err(CmdError::Failed(why.said().to_string()));
    }
    if let Some(r) = s.mailbox.rules.get_mut(i) {
        r.account = account;
        r.keep = Keep::Server;
        r.refused.clear();
    }
    Ok(json!({"id": id, "account": account, "keep": "server"}))
}
