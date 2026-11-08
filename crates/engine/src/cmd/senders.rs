// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's other sender addresses (#797): the aliases and role addresses its outgoing
//! server lets it send from. The model is `mailcraft_model::senders`; a message's From is
//! `compose.set`'s `from`, and a reply or forward is from the address the original was sent to.

use mailcraft_model::{Account, AccountKind, MAX_SENDER_ADDRESSES, SenderAddress, SenderProblem, clean_sender_addresses};
use serde_json::{Value, json};

use super::bad;
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Longest a name or signature name in the params may be before it is cut.
const MAX_FIELD: usize = 1_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.senderAddresses", "Sender Addresses", "File › Account Settings", list)
            .pure()
            .params(r#"{"id": number} — the account's own address and its other sender addresses: {"email", "displayName", "addresses": [{"email", "displayName"?, "signature"?}], "takes": bool (whether it sends from them: an IMAP account; not a Microsoft, Exchange or JMAP account, nor a shared mailbox)}"#),
        CommandSpec::new("account.setSenderAddresses", "Set Sender Addresses", "File › Account Settings", set).params(
            r#"{"id": number, "addresses": [{"email": string, "displayName"?: string (default: the account's), "signature"?: string (a signature's name, for its new messages, replies and forwards; default: the account's)}]} — the other addresses an IMAP account sends from over the same sign-in (aliases its server lets it use), in place of those it had; at most 50, each an address once, never the account's own. A new message is from the account's own address unless From says otherwise; a reply or forward is from the one the original was sent to (its To, then Cc, then its X-Original-To and Delivered-To headers). Answers as account.senderAddresses"#,
        ),
    ]
}

fn account_of(s: &Session, v: &Value) -> Result<Account, CmdError> {
    let id = p::req_id(v, "id")?;
    s.mailbox.account(id).cloned().ok_or_else(|| failed!("no account with id {id}", id))
}

fn shown(a: &Account) -> Value {
    json!({
        "id": a.id,
        "email": a.email,
        "displayName": a.display_name,
        "addresses": a.sender_addresses,
        "takes": a.takes_sender_addresses(),
    })
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    Ok(shown(&account_of(s, v)?))
}

/// One entry of `addresses`: an object with a string `email`, and strings for the rest.
fn entry(v: &Value) -> Result<SenderAddress, CmdError> {
    let Value::Object(o) = v else { return Err(bad("each of `addresses` is {\"email\": string, \"displayName\"?: string, \"signature\"?: string}")) };
    let text = |key: &str| -> Result<String, CmdError> {
        match o.get(key) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(t)) => Ok(t.chars().take(MAX_FIELD).collect()),
            Some(_) => Err(bad(format!("`{key}` of a sender address is a string"))),
        }
    };
    Ok(SenderAddress { email: text("email")?, display_name: text("displayName")?, signature: text("signature")? })
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let a = account_of(s, v)?;
    let Some(Value::Array(given)) = v.get("addresses") else { return Err(bad("`addresses` (a list) is required")) };
    if given.len() > MAX_SENDER_ADDRESSES.saturating_mul(4) {
        return Err(failed!("an account has {max} sender addresses at most", max = MAX_SENDER_ADDRESSES));
    }
    let typed = given.iter().map(entry).collect::<Result<Vec<_>, _>>()?;
    // Only an account that sends from them; any may be left with none.
    if !typed.is_empty() && !a.takes_sender_addresses() {
        return Err(match a.kind {
            AccountKind::Microsoft | AccountKind::Exchange => {
                failed!("a Microsoft or Exchange account sends from other addresses only as its server allows, which MailCraft doesn't ask for yet")
            }
            _ => failed!("only an IMAP account sends from other addresses"),
        });
    }
    let mut kept = clean_sender_addresses(&a.email, &typed).map_err(|problem| match problem {
        SenderProblem::NotAnAddress(address) => failed!("\"{address}\" isn't an e-mail address", address),
        SenderProblem::TooMany => failed!("an account has {max} sender addresses at most", max = MAX_SENDER_ADDRESSES),
    })?;
    for x in kept.iter_mut().filter(|x| !x.signature.is_empty()) {
        let name = x.signature.clone();
        x.signature = s.mailbox.signature(&name).map(|sig| sig.name.clone()).ok_or_else(|| failed!("no signature named \"{name}\"", name))?;
    }
    let account = s.mailbox.account_mut(a.id)?;
    account.sender_addresses = kept;
    // (A message being written from an address the account no longer has isn't sent from it:
    // compose.send asks for another From.)
    Ok(shown(account))
}
