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
            .params(r#"{"value"?: bool} (default: the other way round) — whether the message being written is Encrypted at Send, to every recipient's Public key and to the owner's own; until this is set the account's Encrypt when every recipient has a key holds, and a change of that setting applies to it. A recipient with no key stops the send and is named. Kept with its draft, which then stays on this computer. Agents may turn it on, never off. Answers {"encrypt": bool, "form"?: "smime"|"openpgp", "missing"?: [address], "conflicts"?: [address]} (conflicts: addresses several keys that arrived with mail were found for, none the owner's: the owner chooses in the window)"#)
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
/// Why a message with so many Bcc recipients can't be: each gets a message of their own.
pub const TOO_MANY_BLIND: &str = "there are too many Bcc recipients to encrypt for, each in a message of their own: remove some, or turn Encrypt off";

/// A key that arrived for an address, offered to the owner to choose from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// As the key store has it.
    pub fingerprint: String,
    /// When the mail that brought it arrived here.
    pub seen: i64,
}

/// An address several different keys arrived for, none of which the owner vouched for: nothing
/// says which is theirs, and the owner is asked (the window's stop) before mail is encrypted to one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyConflict {
    pub address: String,
    pub candidates: Vec<Candidate>,
}

/// What the message will be at Send, as far as Encrypt goes (R16a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Will {
    /// It goes as it is: Encrypt isn't on for it (the owner turned it off, or the account doesn't
    /// ask for it).
    Clear,
    /// The account's Encrypt when every recipient has a key finds a recipient with none (or
    /// several keys for one, none the owner's): it goes as it is, and the window says so and why.
    ClearBecause { addresses: Vec<String> },
    /// It is Encrypted, to these keys. `unverified` names the recipients whose key the owner
    /// neither imported nor marked Verified: it was collected from their mail. `blind` names the
    /// Bcc recipients, each given a message of their own so that nobody's key names them.
    Encrypted { to: Encryption, unverified: Vec<String>, blind: Vec<String> },
    /// Encrypt was asked for and these recipients have no key (in the form the message would be
    /// Encrypted in): the send stops and names them (R18).
    Missing { format: SignatureFormat, addresses: Vec<String> },
    /// Encrypt was asked for and these addresses have several keys that arrived with mail and none
    /// the owner vouched for: the send stops until the owner says which (the window's, never an
    /// agent's).
    Conflict { format: SignatureFormat, conflicts: Vec<KeyConflict> },
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
    let (to, cc, bcc) = (Address::parse_list(&c.to), Address::parse_list(&c.cc), Address::parse_list(&c.bcc));
    let recipients = addresses_of(&[&to, &cc, &bcc]);
    let visible = addresses_of(&[&to, &cc]);
    let blind: Vec<String> = recipients.iter().filter(|a| !visible.contains(a)).cloned().collect();
    let sign = super::signing::will_sign(s, c).ok().flatten();
    plan_with(&s.mailbox, account, &recipients, &blind, sign.as_ref(), c.encrypt, s.now())
}

/// The addresses a message being written goes to.
pub fn recipients_of(c: &Compose) -> Vec<String> {
    addresses_of(&[&Address::parse_list(&c.to), &Address::parse_list(&c.cc), &Address::parse_list(&c.bcc)])
}

/// [`plan_with`] for a message with no Bcc recipients.
pub fn plan(mb: &Mailbox, account: &Account, recipients: &[String], sign: Option<&SigningKey>, choice: Option<bool>, now: i64) -> Will {
    plan_with(mb, account, recipients, &[], sign, choice, now)
}

/// What a message from `account` to `recipients` is, as far as Encrypt goes: `blind` are those of
/// them who are written to blind (each is given a message of their own), `choice` the owner's for
/// the message (`None`: they didn't, and the account's setting holds), `sign` the key it is
/// Digitally signed with, whose form it is then encrypted in too (the signed part goes inside).
/// `now` is seconds since 1970.
pub fn plan_with(
    mb: &Mailbox,
    account: &Account,
    recipients: &[String],
    blind: &[String],
    sign: Option<&SigningKey>,
    choice: Option<bool>,
    now: i64,
) -> Will {
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
    if blind.len() > mailcraft_model::encrypting::MAX_BLIND {
        return if asked { Will::Refused(TOO_MANY_BLIND) } else { Will::Clear };
    }
    let own = account.email.trim().to_lowercase();
    // The form it is Encrypted in: the one it is signed in; else the one the account signs with,
    // else S/MIME, and the other one when that is the one every recipient has a key for.
    let first = sign.map(|k| k.format).or(account.signing.key.as_ref().map(|k| k.format)).unwrap_or(SignatureFormat::Smime);
    let second = if first == SignatureFormat::Smime { SignatureFormat::OpenPgp } else { SignatureFormat::Smime };
    let formats: Vec<SignatureFormat> = if sign.is_some() { vec![first] } else { vec![first, second] };
    let mut best: Option<Attempt> = None;
    for format in formats {
        let mut attempt = Attempt { format, keys: Vec::new(), missing: Vec::new(), conflicts: Vec::new(), unverified: Vec::new() };
        for address in recipients {
            match pick(mb, address, format, now) {
                Pick::Key(k) => {
                    if !k.is_owners() {
                        attempt.unverified.push(address.clone());
                    }
                    attempt.keys.push(EncryptTo {
                        address: address.clone(),
                        fingerprint: k.fingerprint.clone(),
                        key: k.data.clone(),
                        own: *address == own,
                    });
                }
                // (The sending account's own address, with no key of the owner's kept: the host
                // finds the owner's own key, as it does when the owner isn't a recipient.)
                Pick::None if *address == own => {}
                Pick::None => attempt.missing.push(address.clone()),
                Pick::Several(keys) => attempt.conflicts.push(KeyConflict {
                    address: address.clone(),
                    candidates: keys.iter().map(|k| Candidate { fingerprint: k.fingerprint.clone(), seen: k.seen }).collect(),
                }),
            }
        }
        // The owner's own key, when the keys kept have one for their address (the host finds it
        // otherwise): Sent Items stays readable (R17).
        if !recipients.contains(&own)
            && let Pick::Key(k) = pick(mb, &own, format, now)
        {
            attempt.keys.push(EncryptTo { address: own.clone(), fingerprint: k.fingerprint.clone(), key: k.data.clone(), own: true });
        }
        if best.as_ref().is_none_or(|b| attempt.lacking() < b.lacking()) {
            best = Some(attempt);
        }
    }
    let Some(best) = best else { return Will::Clear };
    if !best.missing.is_empty() {
        return if asked {
            Will::Missing { format: best.format, addresses: best.missing }
        } else {
            Will::ClearBecause { addresses: best.missing.into_iter().chain(best.conflicts.into_iter().map(|c| c.address)).collect() }
        };
    }
    if !best.conflicts.is_empty() {
        return if asked {
            Will::Conflict { format: best.format, conflicts: best.conflicts }
        } else {
            Will::ClearBecause { addresses: best.conflicts.into_iter().map(|c| c.address).collect() }
        };
    }
    let blind = blind.iter().filter(|b| **b != own).cloned().collect();
    Will::Encrypted { to: Encryption { format: best.format, recipients: best.keys, own_address: own }, unverified: best.unverified, blind }
}

/// One form's try at finding every recipient a key.
struct Attempt {
    format: SignatureFormat,
    keys: Vec<EncryptTo>,
    missing: Vec<String>,
    conflicts: Vec<KeyConflict>,
    unverified: Vec<String>,
}

impl Attempt {
    fn lacking(&self) -> usize {
        self.missing.len().saturating_add(self.conflicts.len())
    }
}

/// What the keys kept give for one address.
enum Pick<'a> {
    Key(&'a PublicKey),
    None,
    /// Several different keys that only arrived with mail (none vouched for by the owner), each
    /// usable.
    Several(Vec<&'a PublicKey>),
}

/// The key of `address` to encrypt to, in `format`:
/// - one the owner imported, chose or marked Verified, when there is one (a key that only arrived
///   with mail never takes the place of one the owner vouched for, and when there is one that
///   can't be encrypted to now, there is no key);
/// - never one that only arrived with mail for one of the owner's own addresses: whoever writes
///   in their name could have made it, and their own key comes from them alone;
/// - else the one key that arrived with mail, and when several different ones did, the owner is
///   to say which: [`Pick::Several`], ordered by when their mail arrived here (never by what its
///   `Date:` header says).
// WKD hook (#30, not built): a Web Key Directory look-up would go here, for an address with no key in
// the store; it needs a host that fetches (the engine does no I/O), so it would arrive as a key the
// host adds with source Collected before Send. Listed in docs/gaps.md, gap 16.
fn pick<'a>(mb: &'a Mailbox, address: &str, format: SignatureFormat, now: i64) -> Pick<'a> {
    let all: Vec<&PublicKey> = mb.public_keys.of(address).iter().filter(|k| k.format == format).collect();
    // (`of` gives the newest seen first.)
    if all.iter().any(|k| k.is_owners()) {
        return all.iter().copied().filter(|k| k.is_owners()).find(|k| usable(k, now)).map_or(Pick::None, Pick::Key);
    }
    if mb.is_own_address(address) {
        return Pick::None;
    }
    let mut found: Vec<&PublicKey> = Vec::new();
    for k in all.into_iter().filter(|k| usable(k, now)) {
        if !found.iter().any(|f| f.fingerprint == k.fingerprint) {
            found.push(k);
        }
    }
    match found.len() {
        0 => Pick::None,
        1 => Pick::Key(found[0]),
        _ => Pick::Several(found),
    }
}

/// Whether mail may be encrypted to `k` now. Reading a key costs (an OpenPGP key's signatures are
/// checked), and this is asked at every change to the recipients, so the answer is kept for the
/// hour (a key's validity is by the hour at most), by what the key is, bounded.
fn usable(k: &PublicKey, now: i64) -> bool {
    use std::hash::{Hash, Hasher};
    use std::sync::{LazyLock, Mutex};
    static ANSWERS: LazyLock<Mutex<std::collections::HashMap<(u8, u64, i64), bool>>> = LazyLock::new(Default::default);
    const MAX_ANSWERS: usize = 4_096;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    k.data.hash(&mut hasher);
    let key = (k.format as u8, hasher.finish(), now.div_euclid(3_600));
    if let Ok(answers) = ANSWERS.lock()
        && let Some(known) = answers.get(&key)
    {
        return *known;
    }
    let ok = match k.format {
        SignatureFormat::Smime => mailcraft_smime::can_encrypt_to(&k.data, now).is_ok(),
        SignatureFormat::OpenPgp => mailcraft_openpgp::can_encrypt_to(&k.data, u64::try_from(now).unwrap_or(0)).is_ok(),
    };
    if let Ok(mut answers) = ANSWERS.lock() {
        if answers.len() >= MAX_ANSWERS {
            answers.clear();
        }
        answers.insert(key, ok);
    }
    ok
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
    let (form, missing, conflicts) = match will_encrypt(s, &c) {
        Will::Encrypted { to, .. } => (Some(to.format.as_str()), Vec::new(), Vec::new()),
        Will::Missing { format, addresses } => (Some(format.as_str()), addresses, Vec::new()),
        Will::Conflict { format, conflicts } => (Some(format.as_str()), Vec::new(), conflicts.into_iter().map(|c| c.address).collect()),
        _ => (None, Vec::new(), Vec::new()),
    };
    Ok(json!({"encrypt": on, "form": form, "missing": missing, "conflicts": conflicts}))
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

/// What the host does so that a draft that is to be Encrypted is kept Encrypted (#30 part 7): a
/// draft is Encrypted to the owner's own key alone, so that nothing in the mailbox file, and
/// nothing any server is given, holds what the owner wrote in the clear. The host has the owner's
/// key and the random numbers; the engine has neither.
pub trait Sealer: Send + Sync {
    /// The body of an Encrypted message (content fields and body, as a message is kept) of the MIME
    /// entity `entity`, in `format`, to the owner's own key for `address`; why not, in words.
    fn seal(&self, format: SignatureFormat, address: &str, entity: &[u8], now: i64) -> Result<Vec<u8>, String>;
}

/// Why a draft kept Encrypted can't be opened to edit yet.
pub const SEALED_DRAFT: &str = "this draft is kept Encrypted to your own key, and opening it to edit needs decrypting, which this version of MailCraft can't do yet: what you wrote is safe here, and it can still be deleted";

/// Keep the draft `m` Encrypted (#30 part 7): what it holds of the owner's text and files becomes an
/// Encrypted message to the owner's own key ([`Message::original`]), the body it keeps is a note, and
/// it can't be opened to edit until MailCraft decrypts ([`Message::sealed_draft`]). Why not when
/// it can't be sealed; the draft is then left as it is (and stays on this computer).
pub(crate) fn seal_draft(s: &Session, m: &mut Message) -> Result<(), String> {
    let sealer = s.sealer.clone().ok_or_else(|| "this program can't encrypt a draft here".to_string())?;
    let account = s.mailbox.account(m.account).ok_or_else(|| "the account is gone".to_string())?;
    let entity = mailcraft_mime::signable(m);
    if entity.len() > mailcraft_smime::MAX_ENCRYPTED_BYTES {
        return Err("the draft is too large to keep Encrypted".into());
    }
    let address = account.email.trim().to_lowercase();
    let first = account.signing.key.as_ref().map(|k| k.format).unwrap_or(SignatureFormat::Smime);
    let second = if first == SignatureFormat::Smime { SignatureFormat::OpenPgp } else { SignatureFormat::Smime };
    let mut why = String::new();
    for format in [first, second] {
        match sealer.seal(format, &address, &entity, s.now()) {
            Ok(body) => {
                m.original = Some(mailcraft_model::Original::new(body).ok_or_else(|| "the draft is too large to keep Encrypted".to_string())?);
                m.body = mailcraft_model::Body { text: s.interface_language.t("This draft is kept Encrypted.").to_string(), html: None };
                m.attachments.clear();
                m.hidden_text.clear();
                m.sealed_draft = true;
                return Ok(());
            }
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// Whether a message with this `choice` is saved as a draft Encrypted (R20): it is to be Encrypted,
/// so what a server would be given of it is its clear text, and so it is kept (and goes up) as
/// the Encrypted message alone, or stays on this computer when it can't be.
pub fn keeps_draft_here(s: &Session, c: &Compose) -> bool {
    match c.encrypt {
        Some(on) => on,
        None => {
            s.mailbox.account(c.account).or(s.mailbox.default_account()).is_some_and(|a| a.encryption.when_possible)
                && matches!(will_encrypt(s, c), Will::Encrypted { .. })
        }
    }
}

/// A message on its way out of `mb`'s Outbox, with what it is encrypted to worked out again from
/// the keys kept now (a key removed or replaced since Send, or since a failed try, is no longer
/// encrypted to): whatever queued it (Send, a meeting's answer, Send Again). A message the owner
/// asked to be Encrypted ([`Message::encrypt`], which Send sets when it will be) that can't be now
/// is left without, and every path that sends refuses it
/// ([`mailcraft_mime::encryption_missing`]): it is held, never sent in the clear. One the account's
/// Encrypt when every recipient has a key governs goes as it is when it can't be.
pub fn decided(mb: &Mailbox, mut m: Message, now: i64) -> Message {
    if let Some(account) = mb.account(m.account) {
        let recipients = addresses_of(&[&m.to, &m.cc, &m.bcc]);
        let blind = m.blind_addresses();
        m.encrypt_with = match plan_with(mb, account, &recipients, &blind, m.sign_with.as_ref(), m.encrypt, now) {
            Will::Encrypted { to, .. } => Some(to),
            _ => None,
        };
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
        Will::Conflict { conflicts, .. } => {
            let shown: Vec<String> = conflicts.iter().take(10).map(|c| c.address.chars().filter(|c| !c.is_control()).take(80).collect()).collect();
            Some(failed!(
                "several different keys arrived with mail for {recipients} and none is yours: choose one in the window (the stop that Send opens), or import their key, or send it unencrypted",
                recipients = shown.join(", ")
            ))
        }
        Will::Refused(why) => Some(fail(*why)),
        _ => None,
    }
}
