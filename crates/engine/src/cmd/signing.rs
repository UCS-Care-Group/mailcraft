// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures made at send (#30 R16, R16a, R19, R23).
//!
//! An account names the key it signs with and whether it always signs
//! ([`mailcraft_model::signing::AccountSigning`], `account.setSigning`, the window's only: an agent
//! never chooses a key). A message being written follows that setting until the owner (or an
//! agent, R23) turns Sign on or off for it (`compose.sign`); an open message follows a change of
//! the setting meanwhile, because what it will be is worked out from both at Send and not before
//! ([`will_sign`]). At Send the message waiting in the Outbox is given the key it goes signed
//! with ([`mailcraft_model::Message::sign_with`]); the host signs it on its way out and nothing
//! sends it unsigned. A message that couldn't be signed is held in the Outbox with why
//! ([`hold_unsigned`]), never sent in the clear.
//!
//! The engine holds no key and makes no signature: those are the host's (`apps/mailcraft`), which
//! the browser build has none of.

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::{SigningKey, Unsigned};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use super::{bad, fail, needs_compose};
use crate::{CmdError, CmdResult, CommandSpec, Compose, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.sign", "Sign", "Options › Encrypt", sign)
            .pure()
            .params(r#"{"value"?: bool} (default: the other way round) — whether the message being written goes with a Digital signature, made at Send with the key its account signs with (File › Info › Settings › Digital Signatures); until this is set the account's Always add a Digital signature holds, and a change of that setting applies to it. Kept with its draft. Answers {"sign": bool, "key"?: string, "form": "clear-signed"}"#)
            .when_writing(needs_compose),
        CommandSpec::new("account.setSigning", "Digital Signatures", "File › Info › Settings", set_signing).params(
            r#"{"account": number, "key"?: {"format": "smime"|"openpgp", "fingerprint": string, "label"?: string} | null, "always"?: bool} — the certificate or OpenPGP key the account signs with (by its fingerprint; null: none) and whether every message from it is Digitally signed. The window's only"#,
        ),
    ]
}

/// Whether `account` can send a Digital signature at all: an account whose mail goes from here
/// (IMAP and SMTP, Microsoft Graph, Exchange Web Services). A JMAP account doesn't send yet, and a
/// local one sends nothing.
pub fn can_sign(account: &Account) -> bool {
    matches!(account.kind, AccountKind::Imap | AccountKind::Microsoft | AccountKind::Exchange)
}

/// Why a message to be signed can't be sent as it is.
pub const NOT_HERE: &str = "this account doesn't send mail from here, so its messages can't be Digitally signed: turn Sign off";
/// Why Always can't be on.
pub const NO_KEY_FOR_ALWAYS: &str = "choose the certificate or key to sign with before turning Always on";
/// Why a message to be signed can't be sent yet.
pub const NO_KEY: &str = "choose the certificate or key to sign with first (File › Info › Settings › Digital Signatures), or turn Sign off";

/// What the message being written will be at Send (R16a): `Ok(None)` unsigned, `Ok(Some(key))`
/// Digitally signed with that key, `Err` signed but it can't be (no key chosen, an account that
/// can't sign): [`NOT_HERE`] or [`NO_KEY`], in words for the owner.
pub fn will_sign(s: &Session, c: &Compose) -> Result<Option<SigningKey>, &'static str> {
    let Some(account) = s.mailbox.account(c.account).or(s.mailbox.default_account()) else { return Ok(None) };
    if !account.signing.signs(c.sign) {
        return Ok(None);
    }
    if !can_sign(account) {
        return Err(NOT_HERE);
    }
    account.signing.key.as_ref().and_then(SigningKey::checked).map(Some).ok_or(NO_KEY)
}

/// Sign: on or off for the message being written, or the other way round from what it would be.
fn sign(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["value"])?;
    let wanted = match v.get("value") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad("`value` is true or false")),
    };
    let c = s.compose.as_ref().ok_or_else(|| fail("no message is being written"))?;
    let account = s.mailbox.account(c.account).or(s.mailbox.default_account()).cloned();
    let now = account.as_ref().is_some_and(|a| a.signing.signs(c.sign));
    let on = wanted.unwrap_or(!now);
    if on && account.as_ref().is_some_and(|a| !can_sign(a)) {
        return Err(fail(NOT_HERE));
    }
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    if c.sign != Some(on) {
        c.sign = Some(on);
        c.dirty = true;
    }
    let key = account.and_then(|a| a.signing.key).map(|k| k.label);
    Ok(json!({"sign": on, "key": key, "form": "clear-signed"}))
}

/// The key an account signs with, and whether always.
fn set_signing(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account", "key", "always"])?;
    let id = p::req_id(v, "account")?;
    let key = match v.get("key") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Object(k)) => {
            let format = match k.get("format").and_then(Value::as_str) {
                Some("smime") => SignatureFormat::Smime,
                Some("openpgp") => SignatureFormat::OpenPgp,
                _ => return Err(bad("`key.format` is smime or openpgp")),
            };
            let fingerprint = k.get("fingerprint").and_then(Value::as_str).ok_or_else(|| bad("`key.fingerprint` is required"))?;
            let label = k.get("label").and_then(Value::as_str).unwrap_or_default();
            Some(Some(SigningKey::new(format, fingerprint, label).ok_or_else(|| bad("`key.fingerprint` isn't a fingerprint"))?))
        }
        Some(_) => return Err(bad("`key` is an object or null")),
    };
    let always = p::opt_bool(v, "always")?;
    let account = s.mailbox.account(id).ok_or_else(|| failed!("no account with id {id}", id = id))?;
    if !can_sign(account) && (always == Some(true) || matches!(key, Some(Some(_)))) {
        return Err(fail(NOT_HERE));
    }
    // Always needs a key: a message from the account is never to be signed with none.
    let will_have_key = match &key {
        Some(k) => k.is_some(),
        None => account.signing.key.is_some(),
    };
    if always.unwrap_or(account.signing.always) && !will_have_key {
        return Err(fail(NO_KEY_FOR_ALWAYS));
    }
    let account = s.mailbox.account_mut(id)?;
    if let Some(key) = key {
        account.signing.key = key;
    }
    if let Some(always) = always {
        account.signing.always = always;
    }
    Ok(json!({"account": id, "key": account.signing.key, "always": account.signing.always}))
}

/// Hold in the Outbox of `account` the messages its host couldn't Digitally sign, each with why:
/// none of them went, and none goes again by itself (Send Again sends it). Whether anything
/// changed; a line for the owner for each.
pub fn hold_unsigned(mb: &mut Mailbox, account: Id, unsigned: &[Unsigned], lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let mut changed = false;
    for u in unsigned {
        let Ok(m) = mb.message_mut(u.message) else { continue };
        if m.account != account || Some(m.folder) != outbox {
            continue;
        }
        m.sending = false;
        changed = true;
        if !m.held.is_empty() {
            continue;
        }
        let why: String = u.why.chars().filter(|c| !c.is_control()).take(300).collect();
        m.held = mailcraft_i18n::fmt(lang.t("It couldn't be Digitally signed: {why}. It wasn't sent; Send Again tries again."), &[("why", &why)]);
        let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(60).collect();
        let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
        let why: String = u.why.chars().filter(|c| !c.is_control()).take(300).collect();
        said.push(mailcraft_i18n::fmt(
            lang.t("'{subject}' couldn't be Digitally signed, so it wasn't sent: {why}. It is held in the Outbox."),
            &[("subject", &subject), ("why", &why)],
        ));
    }
    changed
}

/// A message on its way out of `mb`'s Outbox, with the key it goes signed with when its
/// account always signs and it wasn't sent with Sign off: whatever queued it (Send, a meeting's
/// answer, Send Again), it isn't sent unsigned while the account says every message is signed.
pub fn decided(mb: &Mailbox, mut m: mailcraft_model::Message) -> mailcraft_model::Message {
    if m.sign_with.is_none()
        && let Some(account) = mb.account(m.account).filter(|a| can_sign(a) && a.signing.signs(m.sign))
    {
        m.sign_with = account.signing.key.as_ref().and_then(SigningKey::checked);
    }
    m
}

/// The error a Send gets when the message can't go as [`will_sign`] says it must.
pub(crate) fn send_refused(why: &'static str) -> CmdError {
    if why == NOT_HERE { fail(NOT_HERE) } else { fail(NO_KEY) }
}
