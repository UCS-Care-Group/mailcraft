// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra account kept in step through a Zimbra server's own web service: the skeleton (Z1).
//!
//! As for IMAP, Microsoft Graph, JMAP and Exchange Web Services, the engine never talks to a
//! server: `sync.zimbraRequest` will say what a run should do, a host that can reach the server
//! will do it, and `sync.applyZimbra` will take in what it brought back, piece by piece. The same
//! goes for messages fetched whole (`sync.zimbraFetchRequest`). All three are the host's side of a
//! synchronisation, never an agent's.
//!
//! **Nothing is built behind them yet.** The commands are registered so that the pieces can be
//! built against them one at a time. While the feature's switch is off ([`Session::zimbra`], set by
//! the host from `MAILCRAFT_ZIMBRA=1`, never by a command's params) they refuse with
//! [`SWITCHED_OFF`]; with it on they check what they are given as the real ones will and then
//! refuse with [`NOT_BUILT`]. None of them changes the mailbox.

use mailcraft_model::sync::ZimbraMailResult;
use mailcraft_model::{Account, AccountKind, Id, Mailbox};
use serde_json::Value;

use super::fail;
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What a build says while Zimbra accounts are off ([`Session::zimbra`]).
pub const SWITCHED_OFF: &str = "Zimbra accounts are being tested: start MailCraft with MAILCRAFT_ZIMBRA=1 to try them";

/// What a build says of a Zimbra account's Send/Receive until it is built.
pub const NOT_BUILT: &str = "Zimbra accounts can't be sent and received yet: that part is not built";

/// What a build says of sending from a Zimbra account until sending is built.
pub const SENDING_NOT_BUILT: &str = "Zimbra accounts can't send mail yet: that part is not built";

/// Why a Zimbra account is not kept in step now, in words for a log or a command's error.
pub fn refusal_text(s: &Session) -> &'static str {
    if s.zimbra { NOT_BUILT } else { SWITCHED_OFF }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.zimbraRequest", "Send/Receive with Zimbra", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "background"?: bool} — for a Zimbra account: answers what a host should do there. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1); not built yet"#)
            .when(available),
        CommandSpec::new("sync.applyZimbra", "Apply Send/Receive with Zimbra", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": ZimbraMailResult} — takes in what a host read from a Zimbra account. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1); not built yet"#),
        CommandSpec::new("sync.zimbraFetchRequest", "Download Zimbra Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number, "most"?: number} — answers which messages of a Zimbra account a host should fetch whole. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1); not built yet"#)
            .when(available),
    ]
}

/// Why the commands are out of reach, when they are: the switch is off, or no account is one.
fn available(s: &Session) -> Option<&'static str> {
    if !s.zimbra {
        return Some(SWITCHED_OFF);
    }
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Zimbra) { None } else { Some("no account is a Zimbra account") }
}

/// The Zimbra account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Zimbra).map(|a| a.id))
        .ok_or_else(|| fail("no account is a Zimbra account"))
}

/// The account `account` when it is a Zimbra account whose server settings may be used
/// ([`mailcraft_model::ZimbraServer::problem`]: a mailbox file can say anything).
fn zimbra_account(mb: &Mailbox, account: Id) -> Result<&Account, CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Zimbra {
        return Err(failed!("{name} isn't a Zimbra account", name = a.name));
    }
    if a.zimbra.as_ref().is_none_or(|server| server.problem().is_some()) {
        return Err(failed!("{name} has no Zimbra server address that can be used", name = a.name));
    }
    Ok(a)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    zimbra_account(&s.mailbox, account)?;
    Err(fail(NOT_BUILT))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    // What comes back is hostile: read as the type says, or refused, whatever it holds.
    let result = v.get("result").cloned().ok_or_else(|| fail("`result` is missing"))?;
    let result: ZimbraMailResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't a Zimbra Send/Receive result"))?;
    // Held to sizes before any of it is looked at: a result may be anything.
    result.check().map_err(|why| failed!("the result {why}", why = mailcraft_i18n::Arg::words(why)))?;
    let a = zimbra_account(&s.mailbox, result.account)?;
    if !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(fail("the result is for another account"));
    }
    // The server too, when the result names one: a result produced for another host, whose
    // `from` token still matches, would apply its pages and its token here.
    if !result.url.is_empty() && !a.zimbra.as_ref().is_some_and(|server| server.same_server(&result.url)) {
        return Err(fail("the result is for another Zimbra server"));
    }
    Err(fail(NOT_BUILT))
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    zimbra_account(&s.mailbox, account)?;
    Err(fail(NOT_BUILT))
}
