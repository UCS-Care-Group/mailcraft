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

use mailcraft_model::{Action, Condition, FollowUp, Id, Importance, Keep, MAX_WORDS_EACH, Mailbox, Rule, RuleGone, Sensitivity, clean_name};
use serde_json::{Value, json};

use super::{bad, fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

use mailcraft_model::MAX_RULES;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.add", "Create Rule", "Home › Move", add).params(
            r#"{"name": string, "conditions": [condition] (at most 20, as exceptions and actions), "exceptions"?: [condition] (any one of them holding leaves the message alone), "actions": [{"type": "moveTo"|"copyTo", "value": folder id} | {"type": "categorize", "value": string} | {"type": "flag"} | {"type": "flagFor", "value": "today"|"tomorrow"|"thisWeek"|"nextWeek"} | {"type": "clearFlag"} | {"type": "clearCategories"} | {"type": "markRead"} | {"type": "setImportance", "value": string} | {"type": "delete"} | {"type": "deleteForGood"}], "stop"?: bool, "alert"?: bool (a desktop alert for the mail it acts on as it arrives), "account"?: number (whose arriving mail it is for; default: the account of the folder in view, a shared mailbox's owner's), "server"?: bool (false keeps it on this computer; true refuses a rule the server can't keep; default: on the server when it can)}, where a condition is {"type": "fromContains"|"toContains"|"subjectContains"|"bodyContains"|"subjectOrBodyContains"|"headerContains", "value": string or [string] (any one of the words)} | {"type": "from"|"sentTo", "value": address or [address]} | {"type": "category", "value": string or [string]} | {"type": "sentOnlyToMe"|"toMe"|"ccMe"|"toOrCcMe"|"notToMe"|"hasAttachment"|"meetingRequest"|"automaticReply"|"flagged"} | {"type": "importance", "value": "high"|"normal"|"low"} | {"type": "sensitivity", "value": "normal"|"personal"|"private"|"confidential"} | {"type": "sizeBetween", "value": {"min": KB, "max": KB}} | {"type": "receivedBetween", "value": {"from"?: "YYYY-MM-DD", "to"?: "YYYY-MM-DD"} (the days in the calendar's time zone, both included)} — on a Microsoft account the rule is kept on its server when the server can do all of it (it then runs there, whether MailCraft is open or not; the sender's and recipients' words are looked for in their addresses alone, as the server does); answers {"id", "keep": "server"|"here"}"#,
        ),
        CommandSpec::new("rules.remove", "Delete Rule", "Home › Move", remove).params(r#"{"id": number} — a rule its server has is deleted there too"#),
        CommandSpec::new("rules.enable", "Enable Rule", "Home › Move", enable).params(r#"{"id": number, "value"?: bool (default: toggle)}"#),
        CommandSpec::new("rules.list", "Manage Rules", "Home › Move", list).pure().params(
            r#"{"account"?: number} — the rules (of that account and those for every account, in the order they run); each says where it is kept: "server", "waiting" (to go to the server), "here" (with why when it can't go there) or "everyAccount""#,
        ),
        CommandSpec::new("rules.run", "Run Rules Now", "Folder › Clean Up", run)
            .params(r#"{"folder"?: number (default: the folder in view), "id"?: number (that rule alone)} — the enabled rules for the folder's account, those its server keeps too, over the mail in it (not a shared mailbox's folder: the rules leave its mail alone); answers {"matched": how many messages the rules changed}"#),
        CommandSpec::new("rules.fromMessage", "Always Move Messages From", "Home › Move", from_message)
            .params(r#"{"folder": number, "id"?: number, "server"?: bool (as for `rules.add`)} — a rule moving this sender's mail to the folder, run at once over the mail in the message's folder (not for a shared mailbox's message: its rules are its own); the window opens the New rule builder filled in instead (`ui.dialog` "rules" with "folder")"#)
            .selection(needs_message),
        CommandSpec::new("rules.move", "Move Rule", "Home › Move", move_rule).params(
            r#"{"id": number, "direction": "up"|"down", "account"?: number (the list it is moved in, for a rule for every account; default: the account of the folder in view)} — within its list: the rules its account's server keeps, or those of this computer"#,
        ),
        CommandSpec::new("rules.keepOnServer", "Keep Rule on Server", "Home › Move", keep_on_server).params(
            r#"{"id": number, "account"?: number (for a rule for every account: the Microsoft account whose server is to keep it, which it is then for alone)} — a rule kept on this computer goes to its Microsoft account's server at the next look, when the server can do all of it"#,
        ),
    ]
}

/// Why a rule can't be kept on an account's server (#566). Graph's rules know no flag, no
/// clearing of categories, no day mail came on and no desktop alert, and aren't asked to delete
/// for good; move and copy mail only to a folder of the same mailbox, and to one at most; and
/// take several texts of one kind as "any of them", where MailCraft's conditions all have to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotOnServer {
    /// MailCraft keeps no rules on the account's server (an account of this computer, IMAP).
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
    /// It looks for a name in the sender or the recipients (words with a space, say), and the
    /// server looks at their addresses alone.
    Names,
    /// A Microsoft account kept in step by IMAP: its rules on the server are reached through
    /// Microsoft Graph.
    NotInStep,
    /// The folder it copies mail to is on this computer, another account's, or not on the server.
    CopyHere,
    CopyElsewhere,
    CopyNotThere,
    TwoCopies,
    ClearCategories,
    /// It clears a flag, and doesn't set one.
    ClearFlag,
    /// It looks at the day mail came.
    Days,
    /// It shows a desktop alert.
    Alert,
    /// Two exceptions of one kind that look for other things (two importances, say): the server
    /// takes one of each kind.
    TwoExceptionsOfAKind,
    /// It deletes mail for good, which MailCraft asks of no server (R3): here, as Shift+Delete
    /// does, it goes where the server keeps what can still be recovered.
    DeleteForGood,
}

impl NotOnServer {
    /// Why, in words.
    pub fn said(self) -> &'static str {
        match self {
            NotOnServer::NoServer => "MailCraft can't keep this account's rules on its server",
            NotOnServer::Flag => "the server can't flag mail",
            NotOnServer::FolderHere => "the folder it moves mail to is on this computer",
            NotOnServer::FolderElsewhere => "the folder it moves mail to is in another account",
            NotOnServer::FolderNotThere => "the folder it moves mail to isn't on the server",
            NotOnServer::TwoMoves => "it moves mail to more than one folder",
            NotOnServer::TwoOfAKind => "it has two conditions of one kind, and the server would take either of them rather than both",
            NotOnServer::TwoImportances => "it sets more than one importance",
            NotOnServer::Names => "it looks for a name, and the server looks at addresses only",
            NotOnServer::NotInStep => "MailCraft reaches this account's rules on the server once it switches to Microsoft Graph",
            NotOnServer::CopyHere => "the folder it copies mail to is on this computer",
            NotOnServer::CopyElsewhere => "the folder it copies mail to is in another account",
            NotOnServer::CopyNotThere => "the folder it copies mail to isn't on the server",
            NotOnServer::TwoCopies => "it copies mail to more than one folder",
            NotOnServer::ClearCategories => "the server can't clear categories",
            NotOnServer::ClearFlag => "the server can't clear a flag",
            NotOnServer::Days => "the server can't look at the day mail came",
            NotOnServer::Alert => "the server can't show a desktop alert",
            NotOnServer::TwoExceptionsOfAKind => "it has two exceptions of one kind that look for different things, and the server takes one",
            NotOnServer::DeleteForGood => "it deletes mail for good, which MailCraft doesn't ask a server to do",
        }
    }
}

/// Why `rule` can't be kept on the server of `account`, or `None` when it can.
pub fn server_cannot(mb: &Mailbox, rule: &Rule, account: Id) -> Option<NotOnServer> {
    if !mb.keeps_rules(account) {
        let imap = mb.account(account).is_some_and(|a| a.on_imap_with_microsoft() && !a.is_shared());
        return Some(if imap { NotOnServer::NotInStep } else { NotOnServer::NoServer });
    }
    let kind = |c: &Condition| std::mem::discriminant(c);
    if rule.conditions.iter().enumerate().any(|(i, c)| rule.conditions.iter().skip(i + 1).any(|d| kind(c) == kind(d))) {
        return Some(NotOnServer::TwoOfAKind);
    }
    // Exceptions of one kind are one list of words or people there; others must be the same.
    let exceptions = &rule.exceptions;
    if exceptions.iter().enumerate().any(|(i, c)| exceptions.iter().skip(i + 1).any(|d| kind(c) == kind(d) && c.words().is_none() && c != d)) {
        return Some(NotOnServer::TwoExceptionsOfAKind);
    }
    let parts = || rule.conditions.iter().chain(exceptions);
    if parts().any(|c| matches!(c, Condition::ReceivedBetween { .. })) {
        return Some(NotOnServer::Days);
    }
    if parts().any(|c| matches!(c, Condition::FromContains(w) | Condition::ToContains(w) if w.iter().any(|t| !in_an_address(t)))) {
        return Some(NotOnServer::Names);
    }
    if rule.alert {
        return Some(NotOnServer::Alert);
    }
    let mut moves = 0usize;
    let mut copies = 0usize;
    let mut importance: Option<Importance> = None;
    for action in &rule.actions {
        match action {
            Action::Flag | Action::FlagFor(_) => return Some(NotOnServer::Flag),
            Action::ClearFlag => return Some(NotOnServer::ClearFlag),
            Action::ClearCategories => return Some(NotOnServer::ClearCategories),
            Action::MoveTo(folder) | Action::CopyTo(folder) => {
                let copy = matches!(action, Action::CopyTo(_));
                if copy {
                    copies += 1;
                } else {
                    moves += 1;
                }
                let not_there = if copy { NotOnServer::CopyNotThere } else { NotOnServer::FolderNotThere };
                let Some(f) = mb.folder(*folder) else { return Some(not_there) };
                if f.account != account {
                    let local = mb.account(f.account).is_some_and(|a| !a.kind.has_server());
                    return Some(match (local, copy) {
                        (true, false) => NotOnServer::FolderHere,
                        (false, false) => NotOnServer::FolderElsewhere,
                        (true, true) => NotOnServer::CopyHere,
                        (false, true) => NotOnServer::CopyElsewhere,
                    });
                }
                if f.server.as_ref().is_none_or(|s| s.id.is_empty()) {
                    return Some(not_there);
                }
            }
            Action::SetImportance(i) => {
                if importance.is_some_and(|had| had != *i) {
                    return Some(NotOnServer::TwoImportances);
                }
                importance = Some(*i);
            }
            Action::DeleteForGood => return Some(NotOnServer::DeleteForGood),
            Action::Categorize(_) | Action::MarkRead | Action::Delete => {}
        }
    }
    match (moves > 1, copies > 1) {
        (true, _) => Some(NotOnServer::TwoMoves),
        (_, true) => Some(NotOnServer::TwoCopies),
        _ => None,
    }
}

/// Whether `words` can be part of an address: no space, none of the marks that set a name or a
/// comment apart from it. A rule looking for others looks for a name, which the server doesn't.
fn in_an_address(words: &str) -> bool {
    !words.chars().any(|c| c.is_whitespace() || "\"(),:;<>[]\\".contains(c))
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
        Some(NotOnServer::NoServer | NotOnServer::NotInStep) | None => ("here", None),
        Some(why) => ("here", Some(why.said().to_string())),
    }
}

/// The words under `value`: a text, or a list of them, any one of which is enough; at most
/// [`MAX_WORDS_EACH`] of 200 characters each. `what` says what they are, for an error.
fn words(v: &Value, what: &str) -> Result<Vec<String>, CmdError> {
    words_of(v, what, 200, false)
}

/// [`words`], each cut to `cap` characters, and kept once as [`mailcraft_model::same_word`] compares
/// them (`addresses`: as people's addresses).
fn words_of(v: &Value, what: &str, cap: usize, addresses: bool) -> Result<Vec<String>, CmdError> {
    let one = |w: &Value| -> Result<String, CmdError> {
        let t: String = w.as_str().ok_or_else(|| bad(format!("`value` is {what}: a text or a list of texts")))?.trim().chars().take(cap).collect();
        if t.is_empty() { Err(bad(format!("`value` has an empty text among its {what}"))) } else { Ok(t) }
    };
    let list: Vec<String> = match v.get("value") {
        Some(Value::Array(items)) if items.len() > MAX_WORDS_EACH => return Err(bad(format!("`value` has more than {MAX_WORDS_EACH} {what}"))),
        Some(Value::Array(items)) => items.iter().map(one).collect::<Result<_, _>>()?,
        Some(w) => vec![one(w)?],
        None => Vec::new(),
    };
    let mut kept: Vec<String> = Vec::new();
    for w in list {
        if !kept.iter().any(|k| mailcraft_model::same_word(addresses, k, &w)) {
            kept.push(w);
        }
    }
    if kept.is_empty() { Err(bad(format!("a condition needs a `value`: {what}"))) } else { Ok(kept) }
}

/// Whether a sender's `email` is one whole address that a rule can look for as it is (`from`); a
/// rule for any other has the sender's words looked for instead (`fromContains`).
pub fn by_address(email: &str) -> bool {
    email.chars().count() <= mailcraft_model::MAX_ADDRESS && mailcraft_model::search::is_address(email)
}

/// The people under `value`, by their whole addresses (`name@domain`), each read whole: one cut
/// short would be somebody else's, and one longer than an address can be is refused.
fn people(v: &Value) -> Result<Vec<String>, CmdError> {
    // (Read one past the longest an address can be, so that a longer one is refused, not cut.)
    let list = words_of(v, "addresses", mailcraft_model::MAX_ADDRESS + 1, true)?;
    match list.iter().find(|a| !by_address(a)) {
        Some(a) => Err(bad(format!("\"{a}\" isn't an address"))),
        None => Ok(list),
    }
}

/// A size in KB under `key` of `value`, at most what a Microsoft account's server keeps (a
/// 32-bit number, Graph's `sizeRange`).
fn kb(v: &Value, key: &str) -> Result<u64, CmdError> {
    v.get("value")
        .and_then(|x| p::u64(x, key))
        .filter(|n| i32::try_from(*n).is_ok())
        .ok_or_else(|| bad(format!("sizeBetween needs `{key}`: a size in KB")))
}

fn condition(v: &Value) -> Result<Condition, CmdError> {
    // A part named wrongly is refused, not left out: a rule without it acts on more mail (#566).
    p::only(v, &["type", "value"])?;
    let value = || p::str(v, "value").unwrap_or("");
    let kind = p::req_str(v, "type")?;
    Ok(match kind {
        "fromContains" => Condition::FromContains(words(v, "words")?),
        "toContains" => Condition::ToContains(words(v, "words")?),
        "subjectContains" => Condition::SubjectContains(words(v, "words")?),
        "bodyContains" => Condition::BodyContains(words(v, "words")?),
        "subjectOrBodyContains" => Condition::SubjectOrBodyContains(words(v, "words")?),
        "headerContains" => Condition::HeaderContains(words(v, "words")?),
        "from" => Condition::From(people(v)?),
        "sentTo" => Condition::SentTo(people(v)?),
        "category" => {
            let names: Vec<String> = words(v, "categories")?.iter().filter_map(|c| clean_name(c)).collect();
            if names.is_empty() {
                return Err(bad("category needs a category name"));
            }
            Condition::Category(names)
        }
        "hasAttachment" => alone(v, kind, Condition::HasAttachment)?,
        "sentOnlyToMe" => alone(v, kind, Condition::SentOnlyToMe)?,
        "toMe" => alone(v, kind, Condition::ToMe)?,
        "ccMe" => alone(v, kind, Condition::CcMe)?,
        "toOrCcMe" => alone(v, kind, Condition::ToOrCcMe)?,
        "notToMe" => alone(v, kind, Condition::NotToMe)?,
        "meetingRequest" => alone(v, kind, Condition::MeetingRequest)?,
        "automaticReply" => alone(v, kind, Condition::AutomaticReply)?,
        "flagged" => alone(v, kind, Condition::Flagged)?,
        "importance" => Condition::Importance(Importance::parse(value()).ok_or_else(|| bad("importance is low, normal or high"))?),
        "sensitivity" => {
            Condition::Sensitivity(Sensitivity::parse(value()).ok_or_else(|| bad("sensitivity is normal, personal, private or confidential"))?)
        }
        "sizeBetween" => {
            let range = v.get("value").filter(|r| r.is_object()).ok_or_else(|| bad("sizeBetween needs `value`: {\"min\": KB, \"max\": KB}"))?;
            p::only(range, &["min", "max"])?;
            let (min, max) = (kb(v, "min")?, kb(v, "max")?);
            if min > max {
                return Err(bad("sizeBetween's `min` is above its `max`"));
            }
            Condition::SizeBetween { min, max }
        }
        "receivedBetween" => {
            let days =
                v.get("value").filter(|d| d.is_object()).ok_or_else(|| bad("receivedBetween needs `value`: {\"from\"?: date, \"to\"?: date}"))?;
            p::only(days, &["from", "to"])?;
            let (from, to) = (p::date(days, "from")?, p::date(days, "to")?);
            match (from, to) {
                (None, None) => return Err(bad("receivedBetween needs `from`, `to` or both")),
                (Some(f), Some(t)) if f > t => return Err(bad("receivedBetween's `from` is after its `to`")),
                _ => Condition::ReceivedBetween { from, to },
            }
        }
        other => return Err(bad(format!("unknown condition type \"{other}\""))),
    })
}

fn action(s: &Session, v: &Value) -> Result<Action, CmdError> {
    p::only(v, &["type", "value"])?;
    let folder = || -> Result<Id, CmdError> {
        let folder = p::req_id(v, "value")?;
        refuse_send_queue(s, folder)?;
        s.mailbox.folder(folder).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id = folder))
    };
    let kind = p::req_str(v, "type")?;
    Ok(match kind {
        "moveTo" => Action::MoveTo(folder()?),
        "copyTo" => Action::CopyTo(folder()?),
        "categorize" => Action::Categorize(clean_name(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("categorize needs a category name"))?),
        "flag" => alone(v, kind, Action::Flag)?,
        "flagFor" => {
            Action::FlagFor(FollowUp::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("flagFor is today, tomorrow, thisWeek or nextWeek"))?)
        }
        "clearFlag" => alone(v, kind, Action::ClearFlag)?,
        "clearCategories" => alone(v, kind, Action::ClearCategories)?,
        "markRead" => alone(v, kind, Action::MarkRead)?,
        "setImportance" => {
            Action::SetImportance(Importance::parse(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("importance is low, normal or high"))?)
        }
        "delete" => alone(v, kind, Action::Delete)?,
        "deleteForGood" => alone(v, kind, Action::DeleteForGood)?,
        other => return Err(bad(format!("unknown action type \"{other}\""))),
    })
}

/// A condition or an action that is a yes or no, which takes no `value`: one given `false` (or a
/// day, for `flag`) is refused, not read as the part itself (#566).
fn alone<T>(v: &Value, kind: &str, part: T) -> Result<T, CmdError> {
    if v.get("value").is_some_and(|x| !x.is_null()) { Err(bad(format!("{kind} takes no `value`"))) } else { Ok(part) }
}

/// A rule never moves or copies mail into an Outbox: the next Send/Receive would send it.
fn refuse_send_queue(s: &Session, folder: Id) -> Result<(), CmdError> {
    if s.mailbox.is_send_queue(folder) { Err(fail("a rule can't move or copy mail to an Outbox: that would send it")) } else { Ok(()) }
}

/// The account whose own mail a folder's is: a shared mailbox's is its owner's (rules leave a
/// shared mailbox's mail alone, and are made for the account it is opened through).
fn own_account(mb: &Mailbox, account: Id) -> Id {
    match mb.account(account) {
        Some(a) if a.is_shared() => mb.owner_of(account).map_or(account, |o| o.id),
        _ => account,
    }
}

/// The account a new rule is for: the one `v` names, or else that of the folder in view (a
/// shared mailbox's owner's), as `rules.move` reads it. A new rule is always for one account's
/// mail (#566): one for every account's is only what a mailbox from before rules had accounts
/// holds.
fn rule_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let account = match p::id_strict(v, "account")? {
        Some(account) => account,
        None => s
            .mailbox
            .folder(s.view.folder)
            .map(|f| own_account(&s.mailbox, f.account))
            .filter(|a| s.mailbox.account(*a).is_some())
            .ok_or_else(|| bad("say whose mail the rule is for (`account`): the folder in view is no account's"))?,
    };
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.is_shared() {
        return Err(fail("a rule is for an account's own mail, not a shared mailbox's"));
    }
    Ok(account)
}

/// The most conditions, exceptions or actions a rule has.
const MOST_PARTS: usize = 20;

/// The conditions under `key` of `v`: a list, required or not.
fn condition_list(v: &Value, key: &str, required: bool) -> Result<Vec<Condition>, CmdError> {
    match v.get(key) {
        None if !required => Ok(Vec::new()),
        // Not cut short: a rule without one of its conditions acts on more mail than it was to,
        // and one without an exception lets through mail it was to leave alone.
        Some(Value::Array(list)) if list.len() > MOST_PARTS => Err(bad(format!("`{key}` has more than {MOST_PARTS}"))),
        Some(Value::Array(list)) => list.iter().map(condition).collect(),
        _ => Err(bad(format!("`{key}` (a list) is {}", if required { "required" } else { "a list" }))),
    }
}

/// The rule `v` asks for, with no id yet (shared by the command and the agent's question).
pub(crate) fn rule_from_params(s: &Session, v: &Value) -> Result<Rule, CmdError> {
    // (A misspelt `exceptions` would make a rule that leaves nothing alone.)
    p::only(v, &["name", "conditions", "exceptions", "actions", "stop", "alert", "account", "server"])?;
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let conditions = condition_list(v, "conditions", true)?;
    // Kept as the server keeps them, so that a rule sent there is known again when it is read
    // back.
    let exceptions = mailcraft_model::fold_exceptions(condition_list(v, "exceptions", false)?).map_err(|e| bad(e.to_string()))?;
    let actions = v.get("actions").and_then(Value::as_array).ok_or_else(|| bad("`actions` (a list) is required"))?;
    if actions.len() > MOST_PARTS {
        return Err(bad(format!("`actions` has more than {MOST_PARTS}")));
    }
    let mut actions = actions.iter().map(|a| action(s, a)).collect::<Result<Vec<_>, _>>()?;
    // An action given twice is done once: two copies into one folder are one, as Outlook's rule
    // has each of its actions once.
    let mut seen: Vec<Action> = Vec::new();
    actions.retain(|a| {
        let again = seen.contains(a);
        if !again {
            seen.push(a.clone());
        }
        !again
    });
    let alert = p::opt_bool(v, "alert")?.unwrap_or(false);
    if conditions.is_empty() || (actions.is_empty() && !alert) {
        return Err(bad("a rule needs at least one condition, and an action or a desktop alert (`alert`)"));
    }
    let account = rule_account(s, v)?;
    let stop = p::opt_bool(v, "stop")?.unwrap_or(false);
    let mut rule = Rule { id: 0, name, enabled: true, conditions, exceptions, actions, stop, alert, account, ..Default::default() };
    place(s, &mut rule, v)?;
    Ok(rule)
}

/// Where a new rule is kept, by `v`'s `server`: on a server that can do all of it, as Outlook
/// keeps an Exchange account's rules, unless the owner keeps it on this computer.
fn place(s: &Session, rule: &mut Rule, v: &Value) -> Result<(), CmdError> {
    let account = rule.account;
    let cannot = if account == 0 { None } else { server_cannot(&s.mailbox, rule, account) };
    match (p::opt_bool(v, "server")?, cannot) {
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
    let parts = |key: &str| v.get(key).and_then(Value::as_array).map(|a| a.iter().take(MOST_PARTS).collect::<Vec<_>>()).unwrap_or_default();
    let conditions = parts("conditions").into_iter().filter_map(|c| condition(c).ok()).collect();
    // As `rules.add` keeps them; exceptions that can't be (more words of a kind than a rule keeps,
    // which `rules.add` refuses and the builder says) are looked at as they are.
    let exceptions: Vec<Condition> = parts("exceptions").into_iter().filter_map(|c| condition(c).ok()).collect();
    let exceptions = mailcraft_model::fold_exceptions(exceptions.clone()).unwrap_or(exceptions);
    let actions = parts("actions").into_iter().filter_map(|a| action(s, a).ok()).collect();
    let alert = p::bool(v, "alert").unwrap_or(false);
    let rule = Rule { conditions, exceptions, actions, alert, account, ..Default::default() };
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
    // it back. One the server was making, with no answer, is looked for by what it is.
    if s.mailbox.rule_gone.len() < mailcraft_model::MAX_RULE_GONE {
        match rule.server.as_ref().filter(|s| !s.id.is_empty()) {
            Some(server) => s.mailbox.rule_gone.push(RuleGone { account: rule.account, id: server.id.clone(), made: None }),
            None if rule.unanswered => s.mailbox.rule_gone.push(RuleGone { account: rule.account, id: String::new(), made: Some(Box::new(rule)) }),
            None => {}
        }
    }
    Ok(json!({"deleted": id}))
}

fn enable(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let value = p::opt_bool(v, "value")?;
    let i = changeable(s, id)?;
    let r = s.mailbox.rules.get_mut(i).ok_or_else(|| failed!("no rule with id {id}", id))?;
    r.enabled = value.unwrap_or(!r.enabled);
    Ok(json!({"id": id, "enabled": r.enabled}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let mb = &s.mailbox;
    let order: Vec<usize> = match p::id_strict(v, "account")? {
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
/// has among them, or the rule `only` alone; answers how many messages a rule acted on.
fn run_on(s: &mut Session, folder: Id, only: Option<Id>) -> usize {
    let ids: Vec<Id> = s.mailbox.messages_in(folder).map(|m| m.id).collect();
    let account = s.mailbox.folder(folder).map_or(0, |f| f.account);
    // A rule that reads the body reads it whole, and a calendar part: one kept on disk is brought
    // in first (one that can't be read says so in its place, and is matched as that). A calendar
    // part too large to be shown isn't brought in for a rule either.
    let reads_bodies = s.mailbox.rules.iter().any(|r| r.enabled && r.is_for(account) && only.is_none_or(|o| o == r.id) && r.reads_body());
    let clock = s.rule_clock();
    // What the rules delete for good goes together at the end (#566).
    let mut going = mailcraft_model::Going::default();
    let matched = ids
        .into_iter()
        .filter(|id| {
            if reads_bodies {
                let _ = s.load_where(*id, crate::session::is_readable_calendar_part);
            }
            match only {
                Some(rule) => s.mailbox.run_rule_batched(*id, rule, &clock, &mut going),
                None => !s.mailbox.run_rules_batched(*id, &clock, &mut going).is_empty(),
            }
        })
        .count();
    s.mailbox.rules_done(going);
    matched
}

fn run(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::id_strict(v, "folder")?.unwrap_or(s.view.folder);
    let only = p::id_strict(v, "id")?;
    let Some(account) = s.mailbox.folder(folder).map(|f| f.account) else {
        return Err(failed!("no folder with id {id}", id = folder));
    };
    if !s.mailbox.rules_reach(account) {
        return Err(failed!("The rules leave a shared mailbox's mail alone: its rules are its own."));
    }
    if let Some(rule) = only
        && !s.mailbox.rules.iter().any(|r| r.id == rule)
    {
        return Err(failed!("no rule with id {id}", id = rule));
    }
    let matched = run_on(s, folder, only);
    Ok(json!({"matched": matched}))
}

/// The rule `rules.fromMessage` makes from `v`, with no id yet, and the folder the message is in
/// (shared by the command and the agent's question).
pub(crate) fn rule_from_message(s: &Session, v: &Value) -> Result<(Rule, Id), CmdError> {
    let folder = p::req_id(v, "folder")?;
    let target = s.mailbox.folder(folder).map(|f| f.name.clone()).ok_or_else(|| failed!("no folder with id {id}", id = folder))?;
    refuse_send_queue(s, folder)?;
    let mid = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(mid).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    // A shared mailbox's mail is left alone by the rules, and one for its owner's account would
    // move the owner's own mail into the folder it names.
    if s.mailbox.account(m.account).is_some_and(|a| a.is_shared()) {
        return Err(fail("a shared mailbox's rules are its own: this version doesn't make them"));
    }
    if m.from.email.trim().is_empty() {
        return Err(fail("the sender has no address to match"));
    }
    let name = format!("{} to {target}", m.from.display());
    let email = m.from.email.trim();
    let from =
        if by_address(email) { Condition::From(vec![email.to_string()]) } else { Condition::FromContains(vec![email.chars().take(200).collect()]) };
    let mut rule = Rule {
        name: name.chars().take(mailcraft_model::MAX_NAME).collect(),
        enabled: true,
        conditions: vec![from],
        actions: vec![Action::MoveTo(folder)],
        account: m.account,
        ..Default::default()
    };
    place(s, &mut rule, v)?;
    Ok((rule, m.folder))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let (mut rule, source) = rule_from_message(s, v)?;
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    rule.id = id;
    s.mailbox.rules.push(rule);
    let matched = run_on(s, source, Some(id));
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
        0 => p::id_strict(v, "account")?.or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account)).map_or(0, |a| own_account(&s.mailbox, a)),
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
    // The one of the two that is in this account's list alone is taken out and put in on the
    // other side of the other, not swapped with it: a rule for every account is in every
    // account's list, and moving one across others would reorder another account's rules.
    let before = server.then(|| s.mailbox.rules.clone());
    if rule.account == 0 && other.account != 0 {
        let neighbour = s.mailbox.rules.remove(j);
        s.mailbox.rules.insert(i, neighbour);
    } else {
        let moving = s.mailbox.rules.remove(i);
        s.mailbox.rules.insert(j, moving);
    }
    // A rule the server doesn't let be changed can leave no number free for the order asked
    // (two rules before it, and it second on the server): the server would put one of them after
    // it, so the move is refused and nothing changes.
    if let Some(before) = before
        && !renumber(&mut s.mailbox, account)
    {
        s.mailbox.rules = before;
        return Err(fail("the server can't keep the rules in that order: a rule it doesn't let be changed leaves no free place for them"));
    }
    Ok(json!({"id": id, "moved": true}))
}

/// Give the rules the server of `account` has the places they now stand in: the same numbers as
/// before, in their new order (all of them afresh when two had the same). One the server doesn't
/// let be changed keeps its number, which no other is given: a new one would never be sent.
/// Whether the numbers hold that order, each above the one before it; they don't when such a rule
/// leaves too few numbers before it (the server's places start at 1).
pub(crate) fn renumber(mb: &mut Mailbox, account: Id) -> bool {
    let on: Vec<usize> = mb.rules.iter().enumerate().filter(|(_, r)| r.account == account && r.on_server()).map(|(i, _)| i).collect();
    let locked = |i: &&usize| mb.rules.get(**i).is_some_and(Rule::read_only);
    let number = |i: &usize| mb.rules.get(*i).and_then(|r| r.server.as_ref()).map(|s| s.sequence);
    let taken: Vec<i32> = on.iter().filter(locked).filter_map(number).collect();
    let mut numbers: Vec<i32> = on.iter().filter(|i| !locked(i)).filter_map(number).collect();
    numbers.sort_unstable();
    // (The server's places start at 1: one below is sent as 1, and would stand with that one.)
    let distinct = numbers.windows(2).all(|w| w.first() < w.get(1)) && numbers.first().is_none_or(|n| *n >= 1);
    if !distinct || numbers.iter().any(|n| taken.contains(n)) {
        // Starting low enough that those listed before the last rule the server won't change stay
        // before it, where two had the same number and the list put this one second.
        let last = on.iter().rposition(|i| locked(&i)).unwrap_or(0);
        let before = on.iter().take(last).filter(|i| !locked(i)).count();
        let low = numbers.first().into_iter().chain(taken.iter()).copied().min().unwrap_or(1);
        let mut next = i32::try_from(before).map_or(1, |b| low.saturating_sub(b)).max(1);
        numbers.clear();
        for i in &on {
            if locked(&i) {
                next = next.max(number(i).unwrap_or(0).saturating_add(1));
                continue;
            }
            while taken.contains(&next) && next < i32::MAX {
                next = next.saturating_add(1);
            }
            numbers.push(next);
            next = next.saturating_add(1);
        }
    }
    let movable: Vec<usize> = on.iter().filter(|i| !locked(i)).copied().collect();
    for (i, number) in movable.into_iter().zip(numbers) {
        if let Some(server) = mb.rules.get_mut(i).and_then(|r| r.server.as_mut()) {
            server.sequence = number;
        }
    }
    let places: Vec<i32> = on.iter().filter_map(|i| mb.rules.get(*i).and_then(|r| r.server.as_ref()).map(|s| s.sequence)).collect();
    places.windows(2).all(|w| w.first() < w.get(1))
}

fn keep_on_server(s: &mut Session, v: &Value) -> CmdResult {
    let (i, rule, account) = keeping_on_server(s, v)?;
    let id = rule.id;
    if rule.keep == Keep::Server {
        return Ok(json!({"id": id, "account": rule.account, "keep": "server"}));
    }
    if let Some(r) = s.mailbox.rules.get_mut(i) {
        r.account = account;
        r.keep = Keep::Server;
        r.refused.clear();
    }
    Ok(json!({"id": id, "account": account, "keep": "server"}))
}

/// The rule `rules.keepOnServer` is asked to keep on a server (where it is in the list, and as it
/// is) and the account whose server is to keep it, or why it can't: for the command, and for the
/// agent's question, which isn't asked of what the command refuses.
pub(crate) fn keeping_on_server(s: &Session, v: &Value) -> Result<(usize, Rule, Id), CmdError> {
    let id = p::req_id(v, "id")?;
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    let rule = s.mailbox.rules.get(i).cloned().ok_or_else(|| failed!("no rule with id {id}", id))?;
    if rule.keep == Keep::Server {
        let account = rule.account;
        return Ok((i, rule, account));
    }
    let account = match (rule.account, p::id_strict(v, "account")?) {
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
    Ok((i, rule, account))
}
