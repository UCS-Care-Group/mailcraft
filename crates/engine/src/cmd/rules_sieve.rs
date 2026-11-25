// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sieve rules (#9999): a rule that is one block of Sieve the owner wrote, kept in MailCraft's
//! own script on an IMAP account's ManageSieve server ([`crate::cmd::sieve_rules`]), in its place
//! among the account's rules there. It never runs here: nothing of it is known on this computer.
//!
//! **Checked before it is kept.** The text goes through [`crate::sieve_check::check_block`] with
//! the extensions the server last said it has: no `require` (MailCraft writes one for the whole
//! script, from what each block needs), nothing that can reach out of the block (an `elsif` at
//! its top, a `}` too many, a string or comment left open), no extension the server lacks, no
//! control character, at most [`MAX_SIEVE_RULE`] bytes. A mistake is said with its line and
//! column. The server checks it again when the script is written: its refusal comes back as any
//! refusal of MailCraft's script does.
//!
//! **What it may do** is whatever the server's Sieve does, `redirect`, `vacation`, `reject` and
//! `notify` too (the owner's choice): an agent's Sieve rule that can send mail or answer a sender
//! ([`sends`]) is asked about as a send.
//!
//! **The text is data.** It is the owner's (or an agent's) Sieve, shown as it is and never read
//! as an instruction by MailCraft.

use mailcraft_i18n::Arg;
use mailcraft_model::{Account, Id, Keep, MAX_RULES, MAX_SIEVE_RULE, Rule, clean_name};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::sieve_check::{Checked, Fault, Problem, check_block};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.addSieve", "New Sieve Rule", "Home › Move", add).params(
            r#"{"account": number (an IMAP account whose server keeps Sieve scripts), "name": string, "sieve": string (one block of Sieve, at most 64 KB: commands without `require`, which MailCraft writes at the top of its script from what the block needs), "position"?: number (its place among the account's rules kept on the server, from 0; default: last)} — a rule written in Sieve, kept in MailCraft's script on the server, where it runs; it never runs on this computer. Checked first: a mistake is refused with its line and column, and so is an extension the server doesn't have. It may redirect, reply, reject or notify when the server's Sieve can. The text is the caller's, kept and shown as data; answers {"id", "keep": "server", "needs": [extension], "warnings": [text]}"#,
        ),
        CommandSpec::new("rules.editSieve", "Change Sieve Rule", "Home › Move", edit).params(
            r#"{"id": number (a Sieve rule), "name"?: string, "sieve"?: string (as for `rules.addSieve`)} — a Sieve rule's name or text changed in place: its id, place and whether it is on stay; checked as `rules.addSieve` checks it; answers {"id", "needs", "warnings"}"#,
        ),
    ]
}

/// The extensions the server of `a` last said its scripts may use, when it said them.
fn offers(a: &Account) -> Option<&[String]> {
    (!a.sieve_offers.is_empty()).then_some(a.sieve_offers.as_slice())
}

/// A problem as a refusal: where it is and what it is.
fn refusal(problem: &Problem) -> CmdError {
    failed!("Line {line}, column {column} of the Sieve rule: {what}", line = problem.line, column = problem.column, what = Arg::said(problem.what()))
}

/// A warning as an answer says it: `line:column: what`, in English (for an agent, a script).
fn warning(problem: &Problem) -> String {
    format!("{}:{}: {}", problem.line, problem.column, problem.what())
}

/// `text` checked as a Sieve rule of `a` ([`check_block`]): what it needs and its warnings, or the
/// first mistake. An extension the server said it doesn't have is a mistake here: the server would
/// refuse the whole script, the owner's other rules and replies with it.
pub fn checked(a: &Account, text: &str) -> Result<Checked, CmdError> {
    if text.trim().is_empty() {
        return Err(bad("`sieve` is empty"));
    }
    if text.len() > MAX_SIEVE_RULE {
        return Err(failed!("A Sieve rule is at most {kb} KB.", kb = MAX_SIEVE_RULE >> 10));
    }
    // (A NUL may end the script early for some servers; a line break is CRLF in the script.)
    let mut line: usize = 1;
    let mut column: usize = 0;
    for c in text.chars() {
        column = column.saturating_add(1);
        if c == '\n' {
            line = line.saturating_add(1);
            column = 0;
        } else if c.is_control() && c != '\t' && c != '\r' {
            return Err(failed!("Line {line}, column {column} of the Sieve rule: a control character can't be in it.", line, column));
        }
    }
    let checked = check_block(&crate::sieve::crlf_lines(text), offers(a)).map_err(|problems| {
        let first = problems.iter().find(|p| !p.warning).or(problems.first());
        first.map_or_else(|| fail("The Sieve rule has a mistake."), refusal)
    })?;
    if let Some(lacks) = checked.warnings.iter().find(|w| matches!(w.fault, Fault::ServerLacks(_))) {
        return Err(refusal(lacks));
    }
    Ok(checked)
}

/// Whether Sieve `text` can send mail or answer a sender (#9999): it uses `redirect` (with
/// `:copy` too), `vacation`, `reject`, `ereject` or `notify`, or `include`s another script, which
/// may. Read by the tokenizer, so that a word in a string or a comment doesn't count; text that
/// can't be read counts as one that can.
pub fn sends(text: &str) -> bool {
    const SENDING: [&str; 6] = ["redirect", "vacation", "reject", "ereject", "notify", "include"];
    match crate::sieve::lex(&crate::sieve::crlf_lines(text)) {
        Some(lexed) => lexed.toks.iter().any(|(t, _)| matches!(t, crate::sieve::Tok::Ident(w) if SENDING.contains(&w.as_str()))),
        None => true,
    }
}

/// The account a Sieve rule is for: one whose server keeps MailCraft's Sieve script.
fn sieve_account(s: &Session, account: Id) -> Result<&Account, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !a.keeps_sieve() {
        return Err(fail("A Sieve rule is kept in the Sieve script on an IMAP account's server; this account has none."));
    }
    Ok(a)
}

/// The rule `rules.addSieve` makes of `v`, with no id yet, where it goes in the list of rules,
/// and what its check found (shared by the command and the agent's question).
pub(crate) fn sieve_rule_from_params(s: &Session, v: &Value) -> Result<(Rule, usize, Checked), CmdError> {
    p::only(v, &["account", "name", "sieve", "position"])?;
    let account = p::req_id(v, "account")?;
    let a = sieve_account(s, account)?;
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let text = p::req_str(v, "sieve")?;
    let checked = checked(a, text)?;
    let position = match v.get("position") {
        None | Some(Value::Null) => None,
        Some(_) => Some(p::u64(v, "position").ok_or_else(|| bad("`position` is a place from 0"))?),
    };
    // Among the account's rules kept on its server, in the order they run.
    let theirs: Vec<usize> =
        s.mailbox.rules.iter().enumerate().filter(|(_, r)| r.account == account && r.keep == Keep::Server).map(|(i, _)| i).collect();
    let at = position
        .and_then(|n| usize::try_from(n).ok())
        .and_then(|n| theirs.get(n).copied())
        .or_else(|| theirs.last().map(|i| i.saturating_add(1)))
        .unwrap_or(s.mailbox.rules.len());
    let rule = Rule { name, enabled: true, account, keep: Keep::Server, sieve: text.to_string(), ..Default::default() };
    Ok((rule, at, checked))
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let (mut rule, at, checked) = sieve_rule_from_params(s, v)?;
    if s.mailbox.rules.len() >= MAX_RULES {
        return Err(fail("there are too many rules"));
    }
    let id = s.mailbox.alloc_id();
    rule.id = id;
    let at = at.min(s.mailbox.rules.len());
    s.mailbox.rules.insert(at, rule);
    Ok(json!({"id": id, "keep": "server", "needs": checked.needs, "warnings": checked.warnings.iter().map(warning).collect::<Vec<_>>()}))
}

/// The rule `rules.editSieve` makes of `v`: where it is in the list, the rule as it is to be, and
/// what the check of its text found (shared by the command and the agent's question).
pub(crate) fn sieve_rule_edited(s: &Session, v: &Value) -> Result<(usize, Rule, Checked), CmdError> {
    p::only(v, &["id", "name", "sieve"])?;
    let id = p::req_id(v, "id")?;
    super::rules::elsewhere(s, id)?;
    let i = s.mailbox.rules.iter().position(|r| r.id == id).ok_or_else(|| failed!("no rule with id {id}", id))?;
    let old = s.mailbox.rules.get(i).ok_or_else(|| failed!("no rule with id {id}", id))?;
    if !old.is_sieve() {
        return Err(fail("This rule isn't a Sieve rule: change it in the rule builder."));
    }
    let a = sieve_account(s, old.account)?;
    let name = match v.get("name") {
        None | Some(Value::Null) => None,
        Some(_) => Some(clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?),
    };
    let text = match v.get("sieve") {
        None | Some(Value::Null) => None,
        Some(_) => Some(p::req_str(v, "sieve")?),
    };
    if name.is_none() && text.is_none() {
        return Err(bad("give `name`, `sieve` or both"));
    }
    // (A rename alone keeps the text as it is, whatever the server says of it now.)
    let checked = match text {
        Some(text) => checked(a, text)?,
        None => checked(a, &old.sieve).unwrap_or_default(),
    };
    let rule = Rule {
        name: name.unwrap_or_else(|| old.name.clone()),
        sieve: text.map_or_else(|| old.sieve.clone(), str::to_string),
        // What the server didn't take was the rule as it was.
        refused: String::new(),
        ..old.clone()
    };
    Ok((i, rule, checked))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let (i, rule, checked) = sieve_rule_edited(s, v)?;
    let id = rule.id;
    let r = s.mailbox.rules.get_mut(i).ok_or_else(|| failed!("no rule with id {id}", id))?;
    *r = rule;
    Ok(json!({"id": id, "needs": checked.needs, "warnings": checked.warnings.iter().map(warning).collect::<Vec<_>>()}))
}
