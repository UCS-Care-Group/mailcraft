// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Junk E-mail Options (#504): the Safe Senders and Blocked Senders lists, and Home › Delete ›
//! Junk's Block Sender, Never Block Sender and Never Block Sender's Domain.
//!
//! The lists are the mailbox's ([`mailcraft_model::JunkLists`]), so they are saved with it and a
//! change to them is undone as any other. They sort mail as it arrives
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
            r#"{"list": "safe"|"blocked", "entry": string} — an address (`ada@example.org`) or a domain (`@example.org`, which matches that domain only, not its subdomains); it leaves the other list"#,
        ),
        CommandSpec::new("junk.remove", "Remove from Safe or Blocked Senders", "Home › Delete › Junk", remove)
            .params(r#"{"list": "safe"|"blocked", "entry": string}"#),
        CommandSpec::new("junk.list", "Junk E-mail Options", "Home › Delete › Junk", list).pure(),
    ]
}

fn which(v: &Value) -> Result<JunkList, CmdError> {
    JunkList::parse(p::req_str(v, "list")?).ok_or_else(|| bad("`list` is \"safe\" or \"blocked\""))
}

/// Add `entry` to `list`; answers it as kept.
fn add_entry(s: &mut Session, list: JunkList, entry: &str) -> Result<String, CmdError> {
    // The owner's own mail (from another device, or a copy sent to themselves) is never junk.
    if list == JunkList::Blocked
        && let Some(key) = junk_entry(entry).map(|e| junk_key(&e))
        && s.mailbox.accounts.iter().any(|a| !a.email.trim().is_empty() && junk_key(&a.email) == key)
    {
        return Err(failed!("{address} is one of your own addresses: your own mail isn't blocked", address = key));
    }
    match s.mailbox.junk.add(list, entry) {
        Added::Added(e) | Added::There(e) => Ok(e),
        Added::Invalid => Err(fail("type an address, such as someone@example.org, or a domain, such as @example.org")),
        Added::Full => Err(failed!("a list holds {n} entries at most", n = mailcraft_model::MAX_JUNK_ENTRIES)),
    }
}

/// The senders of the target messages, as each would be kept on a list (an address, or its
/// domain), each once.
fn senders(s: &Session, ids: &[Id], domain: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        let Some(m) = s.mailbox.message(*id) else { continue };
        let address = m.from.email.trim();
        let text = if domain { address.rsplit_once('@').map(|(_, d)| format!("@{d}")) } else { Some(address.to_string()) };
        if let Some(e) = text.as_deref().and_then(junk_entry).filter(|e| domain || !e.starts_with('@'))
            && !out.contains(&e)
        {
            out.push(e);
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
    for e in &entries {
        add_entry(s, list, e)?;
    }
    Ok((ids, entries))
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
    let entry = add_entry(s, list, p::req_str(v, "entry")?)?;
    Ok(json!({"list": list.as_str(), "entry": entry}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let list = which(v)?;
    let entry = p::req_str(v, "entry")?;
    if !s.mailbox.junk.remove(list, entry) {
        return Err(fail("that isn't on the list"));
    }
    Ok(json!({"list": list.as_str(), "removed": true}))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"safe": s.mailbox.junk.safe, "blocked": s.mailbox.junk.blocked}))
}
