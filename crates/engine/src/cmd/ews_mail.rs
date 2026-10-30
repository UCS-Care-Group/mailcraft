// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account kept in step through Exchange Web Services (#252), E1: the skeleton.
//!
//! As for IMAP, Microsoft Graph and JMAP, the engine never talks to a server: `sync.ewsRequest`
//! will say what a run should read, a host that can reach the server will do it, and
//! `sync.applyEws` will take in what it brought back, piece by piece. The same goes for messages
//! fetched whole (`sync.ewsFetchRequest`). All three are the host's side of a synchronisation,
//! never an agent's.
//!
//! **Nothing is built behind them yet.** The commands are registered so that the pieces can be
//! built against them one at a time. While the feature's switch is off ([`Session::ews`], set by
//! the host from `MAILCRAFT_EWS=1`, never by a command's params) they refuse with
//! [`SWITCHED_OFF`]; with it on they check what they are given as the real ones will and then
//! refuse with [`NOT_BUILT`]. None of them changes the mailbox.

use mailcraft_model::sync::EwsMailResult;
use mailcraft_model::{Account, AccountKind, Id, Mailbox};
use serde_json::Value;

use super::fail;
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What a build says while Exchange server accounts are off ([`Session::ews`]).
pub const SWITCHED_OFF: &str = "Exchange server accounts are being tested: start MailCraft with MAILCRAFT_EWS=1 to try them";

/// What a build says of an Exchange server account's Send/Receive until it is built.
pub const NOT_BUILT: &str = "Exchange server accounts can't be sent and received yet: that part is not built";

/// What a build says of sending from an Exchange server account until sending is built.
pub const SENDING_NOT_BUILT: &str = "Exchange server accounts can't send mail yet: that part is not built";

/// Whether sending mail through Exchange Web Services is built (step 5 of #252 flips this): until
/// it is, nothing is queued in an Exchange server account's Outbox, switch on or off.
const SENDING_BUILT: bool = false;

/// Whether a message may be queued in the Outbox of an account of this kind, to be sent by the
/// next Send/Receive. The one place that says so, for a message being written, an answer to a
/// meeting and a forwarded meeting alike: an account whose kind sends no mail, or whose sending is
/// off or not built, has nowhere for the message, and an Outbox would keep it for ever.
pub(crate) fn can_queue_mail(s: &Session, kind: AccountKind) -> bool {
    kind.sends_mail() && (!kind.needs_switch() || (s.ews && SENDING_BUILT))
}

/// Why a message can't be queued for an account of this kind ([`can_queue_mail`]), in words that
/// follow "…: send it from another account". For a kind that has a server; the same words as ever
/// for a JMAP account.
pub(crate) fn not_sending_text(s: &Session, kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Exchange if !s.ews => SWITCHED_OFF,
        AccountKind::Exchange => SENDING_NOT_BUILT,
        _ => mailcraft_model::NOT_SENDING_YET,
    }
}

/// Why an Exchange server account is not kept in step now, in words for a log or a command's error.
pub fn refusal_text(s: &Session) -> &'static str {
    if s.ews { NOT_BUILT } else { SWITCHED_OFF }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.ewsRequest", "Send/Receive with Exchange Web Services", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "light"?: bool, "background"?: bool} — for an Exchange server account: answers what a host should read there. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1); not built yet"#)
            .when(available),
        CommandSpec::new("sync.applyEws", "Apply Send/Receive with Exchange Web Services", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": EwsMailResult} — takes in what a host read from an Exchange server account. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1); not built yet"#),
        CommandSpec::new("sync.ewsFetchRequest", "Download Exchange Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number, "most"?: number} — answers which messages of an Exchange server account a host should fetch whole. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1); not built yet"#)
            .when(available),
    ]
}

/// Why the commands are out of reach, when they are: the switch is off, or no account is one.
fn available(s: &Session) -> Option<&'static str> {
    if !s.ews {
        return Some(SWITCHED_OFF);
    }
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Exchange) { None } else { Some("no account is an Exchange server account") }
}

/// The Exchange server account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Exchange).map(|a| a.id))
        .ok_or_else(|| fail("no account is an Exchange server account"))
}

/// The account `account` when it is an Exchange server account whose server settings may be used
/// ([`mailcraft_model::EwsServer::problem`]: a mailbox file can say anything).
fn ews_account(mb: &Mailbox, account: Id) -> Result<&Account, CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Exchange {
        return Err(failed!("{name} isn't an Exchange server account", name = a.name));
    }
    if a.ews.as_ref().is_none_or(|server| server.problem().is_some()) {
        return Err(failed!("{name} has no Exchange server address that can be used", name = a.name));
    }
    Ok(a)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    ews_account(&s.mailbox, account)?;
    Err(fail(NOT_BUILT))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    // What comes back is hostile: read as the type says, or refused, whatever it holds.
    let result = v.get("result").cloned().ok_or_else(|| fail("`result` is missing"))?;
    let result: EwsMailResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't an Exchange Send/Receive result"))?;
    // Held to sizes before any of it is looked at: a result may be anything.
    result.check().map_err(|why| failed!("the result {why}", why = mailcraft_i18n::Arg::words(why)))?;
    let a = ews_account(&s.mailbox, result.account)?;
    if !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(fail("the result is for another account"));
    }
    Err(fail(NOT_BUILT))
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    ews_account(&s.mailbox, account)?;
    Err(fail(NOT_BUILT))
}
