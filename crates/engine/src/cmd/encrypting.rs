// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting what the owner sends (#30 part 7: R16, R16a, R17, R18, R20, R21).
//!
//! An account says whether its mail is encrypted whenever every recipient has a Public key
//! (`account.setEncryption`, the window's only: an agent never changes how an account handles
//! mail). A message being written follows that until the owner turns Encrypt on or off for it
//! (`compose.encrypt`); what it will be is worked out from both at Send and not before
//! ([`will_encrypt`]), so an open message follows a change of the setting meanwhile.
//!
//! Encrypting needs only Public keys, which are the mailbox's own ([`mailcraft_model::public_keys`]):
//! at Send the message waiting in the Outbox is given what it is encrypted to
//! ([`mailcraft_model::Message::encrypt_with`]), a key for each recipient and the owner's own
//! (R17); the host encrypts it on its way out and nothing sends it in the clear. A recipient with
//! no key, when Encrypt was asked for, stops the send and is named (R18): the window then offers
//! Send unencrypted, Remove and Cancel. Sending unencrypted is the owner's choice alone: an agent
//! may turn Encrypt on and may remove recipients, never turn it off. A draft with Encrypt on stays
//! on this computer ([`Message::local_only`], R20).
//!
//! The engine holds no private key and makes no encryption: that is the host's
//! (`apps/mailcraft`), which the browser build has none of.

use std::collections::HashSet;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::encrypting::{EncryptTo, Encryption, MAX_ENCRYPT_TO};
use mailcraft_model::public_keys::PublicKey;
use mailcraft_model::signing::SigningKey;
use mailcraft_model::{Account, Address, Mailbox, Message, format_list};
use serde_json::{Value, json};

use super::{bad, fail, needs_compose};
use crate::{CmdResult, CommandSpec, Compose, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.encrypt", "Encrypt", "Options › Encrypt", encrypt)
            .pure()
            .params(r#"{"value"?: bool} (default: the other way round) — whether the message being written is Encrypted at Send, to every recipient's Public key and to the owner's own; until this is set the account's Encrypt when every recipient has a key holds, and a change of that setting applies to it. A recipient with no key stops the send and is named. Kept with its draft, which then stays on this computer. Agents may turn it on, never off. Answers {"encrypt": bool, "form"?: "smime"|"openpgp", "missing"?: [address]}"#)
            .when_writing(needs_compose),
        CommandSpec::new("compose.removeRecipients", "Remove Recipients", "Options › Encrypt", remove_recipients)
            .pure()
            .params(r#"{"addresses": [string]} — takes these addresses off the To, Cc and Bcc lines of the message being written (the ones a send named as having no key, say). Answers {"removed": number}"#)
            .when_writing(needs_compose),
        CommandSpec::new("account.setEncryption", "Encryption", "File › Account Settings", set_encryption)
            .params(r#"{"account": number, "whenPossible"?: bool} — whether every message from the account is Encrypted when every recipient has a Public key (it goes as it is when some recipient has none). The window's only"#),
    ]
}

/// Whether `account` can send an Encrypted message at all: those whose mail goes from here (as
/// [`super::signing::can_sign`]).
pub fn can_encrypt(account: &Account) -> bool {
    super::signing::can_sign(account)
}

/// Why a message to be Encrypted can't be sent as it is.
pub const NOT_HERE: &str = "this account doesn't send mail from here, so its messages can't be Encrypted: turn Encrypt off";
/// Why a message to be Encrypted (and signed) can't be.
pub const TOO_MANY: &str = "there are too many recipients to encrypt to: remove some, or turn Encrypt off";

/// What the message will be at Send, as far as Encrypt goes (R16a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Will {
    /// It goes as it is: Encrypt isn't on for it, or the account's Encrypt when every recipient has
    /// a key finds a recipient with none.
    Clear,
    /// It is Encrypted, to these keys. `unverified` names the recipients whose key the owner
    /// neither imported nor marked Verified: it was collected from their mail.
    Encrypted { to: Encryption, unverified: Vec<String> },
    /// Encrypt was asked for and these recipients have no key (in the form the message would be
    /// Encrypted in): the send stops and names them (R18).
    Missing { format: SignatureFormat, addresses: Vec<String> },
    /// It can't be Encrypted, and why, in words.
    Refused(&'static str),
}

/// The addresses a message goes to, in lower case, each once.
pub fn addresses_of(lists: &[&[Address]]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in lists.iter().flat_map(|l| l.iter()) {
        let email = a.email.trim().to_lowercase();
        if !email.is_empty() && a.is_valid() && seen.insert(email.clone()) {
            out.push(email);
        }
    }
    out
}

/// What the message being written will be at Send, as far as Encrypt goes.
pub fn will_encrypt(s: &Session, c: &Compose) -> Will {
    let Some(account) = s.mailbox.account(c.account).or(s.mailbox.default_account()) else { return Will::Clear };
    let recipients = recipients_of(c);
    let sign = super::signing::will_sign(s, c).ok().flatten();
    plan(&s.mailbox, account, &recipients, sign.as_ref(), c.encrypt, s.now())
}

/// The addresses a message being written goes to.
pub fn recipients_of(c: &Compose) -> Vec<String> {
    addresses_of(&[&Address::parse_list(&c.to), &Address::parse_list(&c.cc), &Address::parse_list(&c.bcc)])
}

/// What a message from `account` to `recipients` is, as far as Encrypt goes: `choice` is the
/// owner's for the message (`None`: they didn't, and the account's setting holds), `sign` the key
/// it is Digitally signed with, whose form it is then encrypted in too (the signed part goes
/// inside). `now` is seconds since 1970.
pub fn plan(mb: &Mailbox, account: &Account, recipients: &[String], sign: Option<&SigningKey>, choice: Option<bool>, now: i64) -> Will {
    let asked = choice == Some(true);
    if !asked && !(choice.is_none() && account.encryption.when_possible) {
        return Will::Clear;
    }
    if !can_encrypt(account) {
        return if asked { Will::Refused(NOT_HERE) } else { Will::Clear };
    }
    if recipients.is_empty() {
        return Will::Clear;
    }
    if recipients.len() >= MAX_ENCRYPT_TO {
        return if asked { Will::Refused(TOO_MANY) } else { Will::Clear };
    }
    let own = account.email.trim().to_lowercase();
    // The form it is Encrypted in: the one it is signed in; else the one the account signs with,
    // else S/MIME, and the other one when that is the one every recipient has a key for.
    let first = sign.map(|k| k.format).or(account.signing.key.as_ref().map(|k| k.format)).unwrap_or(SignatureFormat::Smime);
    let second = if first == SignatureFormat::Smime { SignatureFormat::OpenPgp } else { SignatureFormat::Smime };
    let formats: Vec<SignatureFormat> = if sign.is_some() { vec![first] } else { vec![first, second] };
    let mut best: Option<(SignatureFormat, Vec<String>, Vec<EncryptTo>, Vec<String>)> = None;
    for format in formats {
        let mut keys = Vec::new();
        let mut missing = Vec::new();
        let mut unverified = Vec::new();
        for address in recipients {
            match key_for(mb, address, format, now) {
                Some(k) => {
                    if !k.is_owners() {
                        unverified.push(address.clone());
                    }
                    keys.push(EncryptTo { address: address.clone(), fingerprint: k.fingerprint.clone(), key: k.data.clone(), own: *address == own });
                }
                None => missing.push(address.clone()),
            }
        }
        // The owner's own key, when the keys kept have one for their address (the host finds it
        // otherwise): Sent Items stays readable (R17).
        if !recipients.contains(&own)
            && let Some(k) = key_for(mb, &own, format, now)
        {
            keys.push(EncryptTo { address: own.clone(), fingerprint: k.fingerprint.clone(), key: k.data.clone(), own: true });
        }
        let better = best.as_ref().is_none_or(|(_, m, _, _)| missing.len() < m.len());
        if better {
            best = Some((format, missing, keys, unverified));
        }
    }
    let Some((format, missing, recipients_keys, unverified)) = best else { return Will::Clear };
    if !missing.is_empty() {
        return if asked { Will::Missing { format, addresses: missing } } else { Will::Clear };
    }
    Will::Encrypted { to: Encryption { format, recipients: recipients_keys, own_address: own }, unverified }
}

/// The key of `address` to encrypt to, in `format`: one the owner imported or marked Verified,
/// when there is one (a key that only arrived with mail never takes the place of one the owner
/// vouched for), else the newest collected; one that can't be encrypted to now is no key.
fn key_for<'a>(mb: &'a Mailbox, address: &str, format: SignatureFormat, now: i64) -> Option<&'a PublicKey> {
    let usable = |k: &&PublicKey| match format {
        SignatureFormat::Smime => mailcraft_smime::can_encrypt_to(&k.data, now).is_ok(),
        SignatureFormat::OpenPgp => mailcraft_openpgp::can_encrypt_to(&k.data, u64::try_from(now).unwrap_or(0)).is_ok(),
    };
    let mut of_format = mb.public_keys.of(address).iter().filter(|k| k.format == format);
    let all: Vec<&PublicKey> = of_format.by_ref().collect();
    let mut owners = all.iter().copied().filter(|k| k.is_owners());
    // (`of` gives the newest seen first.)
    owners.find(usable).or_else(|| if all.iter().any(|k| k.is_owners()) { None } else { all.iter().copied().find(usable) })
}

/// Encrypt: on or off for the message being written, or the other way round from what it would be.
fn encrypt(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["value"])?;
    let wanted = match v.get("value") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad("`value` is true or false")),
    };
    let c = s.compose.as_ref().ok_or_else(|| fail("no message is being written"))?;
    let account = s.mailbox.account(c.account).or(s.mailbox.default_account()).cloned();
    let now = c.encrypt.unwrap_or_else(|| account.as_ref().is_some_and(|a| a.encryption.when_possible));
    let on = wanted.unwrap_or(!now);
    if on && account.as_ref().is_some_and(|a| !can_encrypt(a)) {
        return Err(fail(NOT_HERE));
    }
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    if c.encrypt != Some(on) {
        c.encrypt = Some(on);
        c.dirty = true;
    }
    let c = s.compose.clone().ok_or_else(|| fail("no message is being written"))?;
    let (form, missing) = match will_encrypt(s, &c) {
        Will::Encrypted { to, .. } => (Some(to.format.as_str()), Vec::new()),
        Will::Missing { format, addresses } => (Some(format.as_str()), addresses),
        _ => (None, Vec::new()),
    };
    Ok(json!({"encrypt": on, "form": form, "missing": missing}))
}

/// Take recipients off the lines of the message being written.
fn remove_recipients(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["addresses"])?;
    let Some(Value::Array(list)) = v.get("addresses") else { return Err(bad("`addresses` is a list of addresses")) };
    if list.len() > 1_000 {
        return Err(bad("`addresses` names too many"));
    }
    let mut gone: HashSet<String> = HashSet::new();
    for a in list {
        let Value::String(a) = a else { return Err(bad("`addresses` is a list of addresses")) };
        gone.insert(a.trim().to_lowercase());
    }
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    let mut removed = 0usize;
    for line in [&mut c.to, &mut c.cc, &mut c.bcc] {
        let before = Address::parse_list(line);
        let kept: Vec<Address> = before.iter().filter(|a| !gone.contains(&a.email.trim().to_lowercase())).cloned().collect();
        if kept.len() != before.len() {
            removed = removed.saturating_add(before.len().saturating_sub(kept.len()));
            *line = format_list(&kept);
        }
    }
    if removed > 0 {
        c.dirty = true;
    }
    Ok(json!({"removed": removed}))
}

/// Whether Encrypt, when every recipient has a key, is on for the account.
fn set_encryption(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account", "whenPossible"])?;
    let id = p::req_id(v, "account")?;
    let when = p::opt_bool(v, "whenPossible")?;
    let account = s.mailbox.account(id).ok_or_else(|| failed!("no account with id {id}", id = id))?;
    if when == Some(true) && !can_encrypt(account) {
        return Err(fail(NOT_HERE));
    }
    let account = s.mailbox.account_mut(id)?;
    if let Some(when) = when {
        account.encryption.when_possible = when;
    }
    Ok(json!({"account": id, "whenPossible": account.encryption.when_possible}))
}

/// Whether `m`, a message that arrived, was Encrypted: its bytes say so, or, when they aren't here,
/// the part it carries does.
pub fn was_encrypted(m: &Message) -> bool {
    match m.original_bytes() {
        Some(bytes) => mailcraft_mime::is_encrypted(bytes),
        None => m.attachments.iter().any(|a| a.mime.eq_ignore_ascii_case("application/pgp-encrypted") || a.name.eq_ignore_ascii_case("smime.p7m")),
    }
}

/// Whether a message with this `choice` stays on this computer when saved as a draft (R20): it is
/// to be Encrypted, so what a server would be given of it is its clear text.
pub fn keeps_draft_here(s: &Session, c: &Compose) -> bool {
    match c.encrypt {
        Some(on) => on,
        None => {
            s.mailbox.account(c.account).or(s.mailbox.default_account()).is_some_and(|a| a.encryption.when_possible)
                && matches!(will_encrypt(s, c), Will::Encrypted { .. })
        }
    }
}

/// A message on its way out of `mb`'s Outbox, with what it is encrypted to when it is to be
/// Encrypted and wasn't given that at Send: whatever queued it (Send, a meeting's answer, Send
/// Again), it isn't sent in the clear while the account encrypts what it can, or the owner asked
/// for Encrypt. One that was asked for and can't be (no key now) is left without, and every path
/// that sends refuses it ([`mailcraft_mime::encryption_missing`]).
pub fn decided(mb: &Mailbox, mut m: Message, now: i64) -> Message {
    if m.encrypt_with.is_none()
        && let Some(account) = mb.account(m.account)
    {
        let recipients = addresses_of(&[&m.to, &m.cc, &m.bcc]);
        if let Will::Encrypted { to, .. } = plan(mb, account, &recipients, m.sign_with.as_ref(), m.encrypt, now) {
            m.encrypt_with = Some(to);
        }
    }
    m
}

/// The error a Send gets when the message can't go as [`will_encrypt`] says it must.
pub(crate) fn send_refused(will: &Will) -> Option<crate::CmdError> {
    match will {
        Will::Missing { addresses, .. } => {
            let shown: Vec<String> = addresses.iter().take(10).map(|a| a.chars().filter(|c| !c.is_control()).take(80).collect()).collect();
            let more = addresses.len().saturating_sub(shown.len());
            let mut list = shown.join(", ");
            if more > 0 {
                list = format!("{list} and {more} more");
            }
            Some(failed!(
                "there is no Public key for {recipients}, so the message can't be Encrypted: remove them, import their keys, or send it unencrypted",
                recipients = list
            ))
        }
        Will::Refused(why) => Some(fail(*why)),
        _ => None,
    }
}
