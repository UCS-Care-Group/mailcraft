// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Junk E-mail Options (#504): the Safe Senders and Blocked Senders lists, and Home › Delete ›
//! Junk's Block Sender, Never Block Sender and Never Block Sender's Domain.
//!
//! The lists are each account's (#516; [`mailcraft_model::JunkLists`], kept in the mailbox file),
//! so they are saved with it and a change to them is undone as any other. A command on a message
//! changes its account's lists; `junk.add`, `junk.remove` and `junk.list` take an optional
//! `account` (default: the selected message's account, else the first). They sort mail as it arrives
//! ([`mailcraft_model::Mailbox::sort_junk`]); a change to a list leaves the mail that is here
//! where it is, but for Block Sender's own message, which goes to Junk E-mail.

use mailcraft_model::{Added, FolderKind, Id, JunkList, junk_entry, junk_key};
use serde_json::{Value, json};

use super::{bad, fail, needs_message, select_after_removal, targets};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("junk.blockSender", "Block Sender", "Home › Delete › Junk", block_sender)
            .params(r#"{"ids"?: [number]} (default: the selected messages) — adds each one's sender to Blocked Senders and moves the messages to Junk E-mail"#)
            .selection(needs_message),
        CommandSpec::new("junk.neverBlockSender", "Never Block Sender", "Home › Delete › Junk", never_block_sender)
            .params(r#"{"ids"?: [number]} (default: the selected messages) — adds each one's sender to Safe Senders"#)
            .selection(needs_message),
        CommandSpec::new("junk.neverBlockDomain", "Never Block Sender's Domain", "Home › Delete › Junk", never_block_domain)
            .params(r#"{"ids"?: [number]} (default: the selected messages) — adds each one's sender's domain (`@example.org`) to Safe Senders"#)
            .selection(needs_message),
        CommandSpec::new("junk.add", "Add to Safe or Blocked Senders", "Home › Delete › Junk", add).params(
            r#"{"list": "safe"|"blocked", "entry": string, "account"?: number} — on that account's list (default: the selected message's account, else the first); an address (`ada@example.org`) or a domain (`@example.org`, which matches that domain only, not its subdomains); it leaves the other list"#,
        ),
        CommandSpec::new("junk.remove", "Remove from Safe or Blocked Senders", "Home › Delete › Junk", remove)
            .params(r#"{"list": "safe"|"blocked", "entry": string, "account"?: number}"#),
        CommandSpec::new("junk.list", "Junk E-mail Options", "Home › Delete › Junk", list)
            .params(r#"{"account"?: number} — answers {"account", "safe": [string], "blocked": [string]}"#)
            .pure(),
    ]
}

fn which(v: &Value) -> Result<JunkList, CmdError> {
    JunkList::parse(p::req_str(v, "list")?).ok_or_else(|| bad("`list` is \"safe\" or \"blocked\""))
}

/// The account whose lists a call is about: `account` (an account of its own, not a shared
/// mailbox, which has its owner's), else the selected message's, else the first.
pub(crate) fn list_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    if let Some(id) = p::id(v, "account") {
        return match s.mailbox.account(id) {
            Some(a) if a.shared_of.is_none() => Ok(id),
            Some(_) => Err(bad("`account` is a shared mailbox: it has the lists of the account it was opened through")),
            None => Err(bad("`account` isn't an account")),
        };
    }
    let selected = s.sel.messages.last().and_then(|m| s.mailbox.message(*m)).map(|m| s.mailbox.junk_owner(m.account));
    selected.or_else(|| s.mailbox.default_account().map(|a| a.id)).ok_or_else(|| fail("there is no account"))
}

/// Add `entry` to `list` of `account`; answers it as kept.
fn add_entry(s: &mut Session, account: Id, list: JunkList, entry: &str) -> Result<String, CmdError> {
    // The owner's own mail (from another device, or a copy sent to themselves) is never junk.
    if list == JunkList::Blocked
        && let Some(key) = junk_entry(entry).map(|e| junk_key(&e))
        && s.mailbox.accounts.iter().any(|a| !a.email.trim().is_empty() && junk_key(&a.email) == key)
    {
        return Err(failed!("{address} is one of your own addresses: your own mail isn't blocked", address = key));
    }
    match s.mailbox.junk_lists_mut(account).add(list, entry) {
        Added::Added(e) | Added::There(e) => Ok(e),
        Added::Invalid => Err(fail("type an address, such as someone@example.org, or a domain, such as @example.org")),
        Added::Full => Err(failed!("a list holds {n} entries at most", n = mailcraft_model::MAX_JUNK_ENTRIES)),
    }
}

/// The senders of the target messages, as each would be kept on a list (an address, or its
/// domain), each once for the account its message is in.
fn senders(s: &Session, ids: &[Id], domain: bool) -> Vec<(Id, String)> {
    let mut out: Vec<(Id, String)> = Vec::new();
    for id in ids {
        let Some(m) = s.mailbox.message(*id) else { continue };
        let address = m.from.email.trim();
        let text = if domain { address.rsplit_once('@').map(|(_, d)| format!("@{d}")) } else { Some(address.to_string()) };
        if let Some(e) = text.as_deref().and_then(junk_entry).filter(|e| domain || !e.starts_with('@')) {
            let owner = s.mailbox.junk_owner(m.account);
            if !out.iter().any(|(a, x)| *a == owner && *x == e) {
                out.push((owner, e));
            }
        }
    }
    out
}

fn add_senders(s: &mut Session, v: &Value, list: JunkList, domain: bool) -> Result<(Vec<Id>, Vec<String>), CmdError> {
    let ids = targets(s, v)?;
    let entries = senders(s, &ids, domain);
    if entries.is_empty() {
        return Err(fail("the sender has no address to put on the list"));
    }
    let mut shown: Vec<String> = Vec::new();
    for (account, e) in &entries {
        let kept = add_entry(s, *account, list, e)?;
        if !shown.contains(&kept) {
            shown.push(kept);
        }
    }
    Ok((ids, shown))
}

fn block_sender(s: &mut Session, v: &Value) -> CmdResult {
    let (ids, entries) = add_senders(s, v, JunkList::Blocked, false)?;
    // The messages go to Junk E-mail, as Junk does; one its server keeps where it is (a JMAP
    // account's, for now) stays, its sender blocked all the same.
    let before = s.rows();
    let mut moved = Vec::new();
    for id in ids {
        let Some((account, folder)) = s.mailbox.message(id).map(|m| (m.account, m.folder)) else { continue };
        if s.mailbox.stays_on_server(id) {
            continue;
        }
        let Some(junk) = s.mailbox.folder_of_kind(account, FolderKind::Junk) else { continue };
        if folder != junk && s.mailbox.move_message(id, junk).is_ok() {
            moved.push(id);
        }
    }
    select_after_removal(s, &before, &moved);
    Ok(json!({"blocked": entries, "moved": moved.len()}))
}

fn never_block_sender(s: &mut Session, v: &Value) -> CmdResult {
    let (_, entries) = add_senders(s, v, JunkList::Safe, false)?;
    Ok(json!({"safe": entries}))
}

fn never_block_domain(s: &mut Session, v: &Value) -> CmdResult {
    let (_, entries) = add_senders(s, v, JunkList::Safe, true)?;
    Ok(json!({"safe": entries}))
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let list = which(v)?;
    let account = list_account(s, v)?;
    let entry = add_entry(s, account, list, p::req_str(v, "entry")?)?;
    Ok(json!({"account": account, "list": list.as_str(), "entry": entry}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let list = which(v)?;
    let entry = p::req_str(v, "entry")?;
    let account = list_account(s, v)?;
    if !s.mailbox.junk_lists_mut(account).remove(list, entry) {
        return Err(fail("that isn't on the list"));
    }
    Ok(json!({"account": account, "list": list.as_str(), "removed": true}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    let lists = s.mailbox.junk_accounts.get(&account);
    Ok(json!({"account": account, "safe": lists.map(|l| &l.safe), "blocked": lists.map(|l| &l.blocked)}))
}
