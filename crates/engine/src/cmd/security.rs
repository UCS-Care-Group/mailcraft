//! Certificates and keys for signed and encrypted mail (File › Options › Trust Center), and what
//! a message's signature and encryption come to.
//!
//! Only public keys travel through these commands' answers. A key file given to
//! `security.import` may hold a private key (a `.p12` file, under its passphrase): the private
//! key stays in the session's memory, and the host keeps it in the system's credential store when
//! the owner wants that. A passphrase is never a parameter: the window sets it for the next import
//! only (`Session::set_passphrase`).

use mailcraft_model::security::{KeyScheme, StoredKey};
use serde_json::{Value, json};

use super::{bad, fail, needs_message, source};
use crate::{CmdResult, CommandSpec, Session, p, security};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("security.keys", "Certificates and Keys", "File › Options › Trust Center", keys)
            .pure()
            .params(r#"{"email"?: string (only the keys for this address)} — the certificates and OpenPGP public keys kept here: whose, for which addresses, until when, whether the owner trusts it and whether it is their own (a private key is kept for it)"#),
        CommandSpec::new("security.import", "Import Certificate or Key", "File › Options › Trust Center", import)
            .params(r#"{"data": base64, "name"?: string} | {"path": string}, "trust"?: bool — a certificate (.cer, .crt, .pem, .p7b), a digital ID with its private key (.p12, .pfx: the window asks for its passphrase, which is never a parameter) or OpenPGP keys (.asc). "trust": the owner vouches for it (a root certificate: everybody it issued to is verified)"#),
        CommandSpec::new("security.export", "Export Certificate or Key", "File › Options › Trust Center", export)
            .pure()
            .params(r#"{"fingerprint": string, "format"?: "pem"|"der", "path"?: string} — the public certificate or OpenPGP key, to give to others (never the private key)"#),
        CommandSpec::new("security.remove", "Remove Certificate or Key", "File › Options › Trust Center", remove).params(r#"{"fingerprint": string} — and its private key, if the owner has one here"#),
        CommandSpec::new("security.trust", "Trust Certificate or Key", "File › Options › Trust Center", trust)
            .params(r#"{"fingerprint": string, "value"?: bool (default: toggle)} — vouch for it: a root certificate verifies the signers it issued to; an OpenPGP key whose fingerprint was checked with its holder verifies their signatures"#),
        CommandSpec::new("security.check", "Message Security", "Message › Security", check)
            .pure()
            .params(r#"{"id"?: number} (default: the selected message) — whether the message is signed (valid, invalid, by whom, verified or not, whether the signer is the sender it names) and encrypted (opened or why not). Says nothing of what the message says"#)
            .selection(needs_message),
        CommandSpec::new("security.collect", "Add Sender's Certificate", "Message › Security", collect)
            .params(r#"{"id"?: number} (default: the selected message) — keep the certificates and keys its signature carries, so that mail to the sender can be encrypted"#)
            .selection(needs_message),
        CommandSpec::new("security.options", "Email Security Options", "File › Options › Trust Center", options)
            .pure()
            .params(r#"{"collectKeys"?: bool, "signByDefault"?: bool, "encryptByDefault"?: bool, "format"?: "auto"|"smime"|"openpgp"}"#),
    ]
}

fn key_json(s: &Session, k: &StoredKey) -> Value {
    json!({
        "scheme": k.scheme.code(),
        "fingerprint": k.fingerprint,
        "name": k.name,
        "emails": k.emails,
        "issuer": k.issuer,
        "notBefore": k.not_before.map(mailcraft_time::format_iso_utc),
        "notAfter": k.not_after.map(mailcraft_time::format_iso_utc),
        "inForce": k.in_force(s.now()),
        "ca": k.is_ca,
        "trusted": k.trusted,
        "own": k.own,
        "privateKeyHere": s.security.has_secret(k.scheme, &k.fingerprint),
        "collected": k.collected,
    })
}

fn keys(s: &mut Session, v: &Value) -> CmdResult {
    let email = p::str(v, "email").unwrap_or("");
    let list: Vec<Value> = s.mailbox.keys.iter().filter(|k| email.is_empty() || k.is_for(email)).map(|k| key_json(s, k)).collect();
    Ok(json!({"keys": list}))
}

fn import(s: &mut Session, v: &Value) -> CmdResult {
    let (_, bytes) = source(v)?;
    if bytes.len() > mailcraft_crypto::MAX_INPUT {
        return Err(fail("the file is too large to be a key file"));
    }
    let trust = p::bool(v, "trust").unwrap_or(false);
    let got = security::import(s, &bytes, trust)?;
    let keys: Vec<Value> = got.keys.iter().map(|k| key_json(s, k)).collect();
    Ok(json!({"imported": keys, "privateKeys": got.secrets}))
}

fn find<'a>(s: &'a Session, v: &Value) -> Result<&'a StoredKey, crate::CmdError> {
    let fp = p::req_str(v, "fingerprint")?.replace(' ', "");
    s.mailbox.keys.iter().find(|k| k.fingerprint.eq_ignore_ascii_case(&fp)).ok_or_else(|| fail("no such certificate or key"))
}

fn export(s: &mut Session, v: &Value) -> CmdResult {
    let k = find(s, v)?;
    let pem = !matches!(p::str(v, "format"), Some("der"));
    let bytes = mailcraft_crypto::export_public(&security::to_crypto(k), pem).map_err(|e| fail(e.to_string()))?;
    let stem = if k.name.is_empty() { k.fingerprint.clone() } else { k.name.clone() };
    let ext = match (k.scheme, pem) {
        (KeyScheme::OpenPgp, _) => "asc",
        (KeyScheme::Smime, true) => "pem",
        (KeyScheme::Smime, false) => "cer",
    };
    super::deliver(v, &super::file_name(&stem, ext), bytes)
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let fp = find(s, v)?.fingerprint.clone();
    let k = security::remove(s, &fp)?;
    Ok(json!({"removed": k.fingerprint, "privateKeyRemoved": k.own}))
}

fn trust(s: &mut Session, v: &Value) -> CmdResult {
    let fp = find(s, v)?.fingerprint.clone();
    let k = s.mailbox.keys.iter_mut().find(|k| k.fingerprint == fp).ok_or_else(|| fail("no such certificate or key"))?;
    k.trusted = p::bool(v, "value").unwrap_or(!k.trusted);
    let now = k.trusted;
    security::keys_changed(s);
    Ok(json!({"trusted": now}))
}

fn message_id(s: &Session, v: &Value) -> Result<u64, crate::CmdError> {
    let id = p::id(v, "id").or_else(|| s.sel.messages.first().copied()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(fail(format!("no message with id {id}")));
    }
    Ok(id)
}

fn check(s: &mut Session, v: &Value) -> CmdResult {
    let id = message_id(s, v)?;
    Ok(match s.security_view(id) {
        Some(view) => view.to_json(),
        None => json!({"level": "none", "summary": "The message is neither signed nor encrypted."}),
    })
}

fn collect(s: &mut Session, v: &Value) -> CmdResult {
    let id = message_id(s, v)?;
    let new: Vec<StoredKey> = s.security_view(id).map(|view| view.collectable.clone()).unwrap_or_default();
    let mut added = Vec::new();
    for k in new {
        if s.mailbox.keys.len() >= mailcraft_model::security::MAX_KEYS {
            break;
        }
        if !s.mailbox.keys.iter().any(|x| x.scheme == k.scheme && x.fingerprint == k.fingerprint) {
            added.push(k.fingerprint.clone());
            s.mailbox.keys.push(k);
        }
    }
    if !added.is_empty() {
        security::keys_changed(s);
    }
    Ok(json!({"added": added}))
}

fn options(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(f) = p::str(v, "format") {
        s.prefs.security_format = match f {
            "auto" | "" => String::new(),
            other => KeyScheme::parse(other).ok_or_else(|| bad("`format` is auto, smime or openpgp"))?.code().to_string(),
        };
        security::keys_changed(s);
    }
    for (key, field) in [
        ("collectKeys", &mut s.prefs.collect_keys),
        ("signByDefault", &mut s.prefs.sign_by_default),
        ("encryptByDefault", &mut s.prefs.encrypt_by_default),
    ] {
        if let Some(b) = p::bool(v, key) {
            *field = b;
        }
    }
    Ok(json!({
        "collectKeys": s.prefs.collect_keys,
        "signByDefault": s.prefs.sign_by_default,
        "encryptByDefault": s.prefs.encrypt_by_default,
        "format": if s.prefs.security_format.is_empty() { "auto" } else { s.prefs.security_format.as_str() },
    }))
}
