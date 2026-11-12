// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures made at Send (#30 part 5): the one place that decides which key holder
//! signs a message and how.
//!
//! A message the owner sent with Sign on carries the key it is to be signed with
//! ([`Message::sign_with`]). Before a run sends anything (IMAP and SMTP, Microsoft Graph, an
//! Exchange server), on the run's own thread, each such message is signed here: its signed part
//! is written once ([`mailcraft_mime::signable`]), signed, and the `multipart/signed` body that
//! results is kept as the bytes the message goes as ([`Message::original`]), so SMTP, the copy in
//! Sent Items, Graph's `sendMail` and EWS's `MimeContent` all carry the same signed bytes. One
//! that can't be signed is taken out of the run and held in the Outbox with why: it is never
//! sent unsigned (and every path that sends refuses one that isn't, `signature_missing`).
//!
//! Who signs (decided 2026-11-11, and kept here alone so that it can change in one place):
//!
//! - S/MIME: the system's store holds the private key (the Windows certificate store, the
//!   macOS keychain, a PKCS#11 token on Linux and FreeBSD); it is handed a digest and gives back
//!   the signature (`key_holders`). RSA signs with PKCS#1 v1.5 and SHA-256, ECDSA with SHA-256
//!   on P-256 and SHA-384 on P-384.
//! - OpenPGP: gpg-agent holds the private key (on disk or on a smartcard); it is handed a digest
//!   (`mailcraft-assuan`). The public key comes from `gpg --export`, run as a program and never
//!   through a shell, and is matched to the agent's key by its key material.
//! - An RSA private key is never used in this process; no key MailCraft holds itself signs
//!   anything yet (an Ed25519 or ECDSA key imported in File › Options › Security may later,
//!   [`imported`]; the import rules are `mailcraft_engine::private_keys`).
//!
//! A PKCS#11 token without a PIN pad needs its PIN: the first message that wants it is held, the
//! window asks ([`SigningHost::pin_wanted`]), and the PIN is held here in memory for the
//! session (`key_holders::Pin`, wiped when dropped), never written or logged. Send Again sends it.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::{SigningKey, Unsigned};
use mailcraft_model::sync::{SendDone, SendOutcome, SendUp};
use mailcraft_model::{Message, Original};
use mailcraft_ui_egui::digital_signatures::{SigningChoice, SigningHost};
use mailcraft_ui_egui::own_keys::KeyPlace;

use crate::key_holders::{self, DigestAlgorithm, HeldCertificate, KeyHolderError, KeyKind, Pin, SignatureScheme, TokenRef};

/// The most bytes `gpg --export` is read for: more is not an owner's keyring.
const MAX_EXPORT: usize = 32 * 1024 * 1024;
/// How long `gpg --export` may take.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(30);
/// What gpg-agent's pinentry says when it asks for a passphrase or a PIN to sign.
const PINENTRY_TEXT: &str = "MailCraft is adding a Digital signature to a message you are sending.";

/// The host of signing for the whole program (the window's keys and PIN questions).
pub(crate) fn host() -> Arc<Host> {
    static HOST: OnceLock<Arc<Host>> = OnceLock::new();
    HOST.get_or_init(|| Arc::new(Host::default())).clone()
}

/// The PINs the owner typed this session, and the token one is wanted for.
#[derive(Default)]
pub(crate) struct Host {
    pins: Mutex<Vec<(TokenRef, Pin)>>,
    /// A token a signature wanted the PIN of, not asked about yet.
    wanted: Mutex<Option<TokenRef>>,
    /// The token the window is asking about now.
    asking: Mutex<Option<TokenRef>>,
}

impl Host {
    pub(crate) fn pin_for(&self, token: &TokenRef) -> Option<Pin> {
        let pins = self.pins.lock().ok()?;
        pins.iter().find(|(t, _)| t == token).map(|(_, p)| Pin::new(p.expose().to_string()))
    }

    pub(crate) fn forget_pin(&self, token: &TokenRef) {
        if let Ok(mut pins) = self.pins.lock() {
            pins.retain(|(t, _)| t != token);
        }
    }

    pub(crate) fn want_pin(&self, token: TokenRef) {
        if let Ok(mut wanted) = self.wanted.lock() {
            *wanted = Some(token);
        }
    }
}

impl SigningHost for Host {
    fn keys(&self) -> Receiver<Result<Vec<SigningChoice>, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("signing keys".into()).spawn(move || {
            let _ = tx.send(choices());
        });
        if let Err(e) = spawned {
            log::warn!("the signing keys couldn't be looked for: {e}");
        }
        rx
    }

    fn pin_wanted(&self) -> Option<String> {
        let token = self.wanted.lock().ok()?.take()?;
        let label = if token.label.is_empty() { token.serial.clone() } else { token.label.clone() };
        if let Ok(mut asking) = self.asking.lock() {
            *asking = Some(token);
        }
        Some(label)
    }

    fn give_pin(&self, pin: String) {
        let pin = Pin::new(pin);
        let Some(token) = self.asking.lock().ok().and_then(|mut a| a.take()) else { return };
        if let Ok(mut pins) = self.pins.lock() {
            pins.retain(|(t, _)| *t != token);
            pins.push((token, pin));
        }
    }

    fn no_pin(&self) {
        if let Ok(mut asking) = self.asking.lock() {
            *asking = None;
        }
    }
}

/// Seconds since 1970 now.
fn now() -> i64 {
    key_holders::now().and_then(|n| i64::try_from(n).ok()).unwrap_or(0)
}

/// Sign the Outbox messages of an IMAP or a Graph run that are to be signed; those that can't be
/// are taken out of the run and said.
pub(crate) fn sign_outbox(outbox: &mut Vec<Message>) -> Vec<Unsigned> {
    sign_outbox_with(outbox, &|m| sign(&host(), m, now()))
}

/// Sign the messages a Graph or an Exchange run sends as their own bytes; one that can't be is
/// taken out of the run and said.
pub(crate) fn sign_sends(sends: &mut Vec<SendUp>) -> Vec<Unsigned> {
    sign_sends_with(sends, &|m| sign(&host(), m, now()))
}

/// What a run that answers each send says of one that couldn't be signed: held, with why.
pub(crate) fn held_sends(unsigned: Vec<Unsigned>) -> Vec<SendDone> {
    unsigned
        .into_iter()
        .map(|u| SendDone {
            message: u.message,
            outcome: SendOutcome::Held { lost: false, why: mailcraft_model::signing::held_unsigned(&u.why) },
            ..SendDone::default()
        })
        .collect()
}

pub(crate) fn sign_outbox_with(outbox: &mut Vec<Message>, sign: &dyn Fn(&mut Message) -> Result<(), String>) -> Vec<Unsigned> {
    let mut unsigned = Vec::new();
    outbox.retain_mut(|m| match sign(m) {
        Ok(()) => true,
        Err(why) => {
            unsigned.push(Unsigned { message: m.id, why });
            false
        }
    });
    unsigned
}

pub(crate) fn sign_sends_with(sends: &mut Vec<SendUp>, sign: &dyn Fn(&mut Message) -> Result<(), String>) -> Vec<Unsigned> {
    let mut unsigned = Vec::new();
    sends.retain_mut(|send| {
        let Some(m) = send.mime.as_deref_mut() else { return true };
        match sign(m) {
            Ok(()) => true,
            Err(why) => {
                unsigned.push(Unsigned { message: send.message, why });
                false
            }
        }
    });
    unsigned
}

/// Sign `m` when it is to be signed: its body becomes the `multipart/signed` it goes as, made
/// now from the message as it is (whatever bytes it held before, a signature by someone else
/// among them, are let go of). Why not, in words, when it can't be.
pub(crate) fn sign(host: &Host, m: &mut Message, now: i64) -> Result<(), String> {
    sign_by(m, &|key, entity| match key.format {
        SignatureFormat::Smime => {
            let (signature, micalg) = smime(host, entity, key, now)?;
            Ok((mailcraft_mime::SignatureKind::Smime, micalg, signature))
        }
        SignatureFormat::OpenPgp => {
            let (armoured, micalg) = openpgp(entity, key, now)?;
            Ok((mailcraft_mime::SignatureKind::OpenPgp, micalg, armoured.into_bytes()))
        }
    })
}

/// What signs an entity with a key: the kind of signature, its `micalg` and the signature.
pub(crate) type Signer<'a> = dyn Fn(&SigningKey, &[u8]) -> Result<(mailcraft_mime::SignatureKind, &'static str, Vec<u8>), String> + 'a;

/// [`sign`] with `signer` making the signature over the entity: everything else is the send's
/// own (the tests hand it a key holder of their own).
pub(crate) fn sign_by(m: &mut Message, signer: &Signer<'_>) -> Result<(), String> {
    let Some(key) = m.sign_with.clone() else { return Ok(()) };
    m.original = None;
    let key = key.checked().ok_or_else(|| "the key it was to be signed with can't be named".to_string())?;
    let entity = mailcraft_mime::signable(m);
    let (kind, micalg, signature) = signer(&key, &entity)?;
    finish(m, &entity, kind, micalg, &signature)
}

/// Keep the signed body as the bytes `m` goes as, and check it now is a signed message.
pub(crate) fn finish(m: &mut Message, entity: &[u8], kind: mailcraft_mime::SignatureKind, micalg: &str, signature: &[u8]) -> Result<(), String> {
    let body = mailcraft_mime::multipart_signed(entity, kind, micalg, signature).map_err(|e| e.to_string())?;
    m.original = Original::new(body);
    if mailcraft_mime::signature_missing(m) {
        m.original = None;
        return Err("the signed message is too large".into());
    }
    Ok(())
}

/// A key imported in File › Options › Security and kept by MailCraft in the credential store
/// (`own_keys`; an RSA one is never kept: `mailcraft_engine::private_keys`, decided 2026-11-11).
/// Such a key doesn't sign yet: Digital Signatures doesn't offer it.
fn imported(_key: &SigningKey) -> Result<(Vec<u8>, &'static str), String> {
    // ponytail: an Ed25519/ECDSA signer for the keys `own_keys` keeps goes here, in a later part.
    Err("its certificate isn't in the system's certificate store or on a smartcard here".into())
}

// ---- S/MIME ----

/// A certificate with its private key, and where it is. [`held`] is the one listing of them:
/// File › Options › Security (`own_keys`) lists the same.
pub(crate) struct Held {
    pub(crate) certificate: HeldCertificate,
    pub(crate) place: KeyPlace,
    /// The PKCS#11 token it is on, when it is on one without a PIN pad that needs a login.
    token: Option<TokenRef>,
}

#[cfg(target_os = "windows")]
pub(crate) fn held() -> Result<Vec<Held>, KeyHolderError> {
    Ok(key_holders::windows_store::certificates()?
        .into_iter()
        .map(|certificate| Held { certificate, place: KeyPlace::WindowsStore, token: None })
        .collect())
}

#[cfg(target_os = "macos")]
pub(crate) fn held() -> Result<Vec<Held>, KeyHolderError> {
    Ok(key_holders::macos_keychain::certificates()?
        .into_iter()
        .map(|certificate| Held { certificate, place: KeyPlace::Keychain, token: None })
        .collect())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn held() -> Result<Vec<Held>, KeyHolderError> {
    let holder = key_holders::pkcs11::open_default()?;
    Ok(holder
        .certificates()?
        .into_iter()
        .map(|c| {
            let token = (c.login_required && !c.protected_path).then(|| c.token.clone());
            Held { certificate: c.certificate, place: KeyPlace::Token(c.token.label.clone()), token }
        })
        .collect())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")))]
pub(crate) fn held() -> Result<Vec<Held>, KeyHolderError> {
    Err(KeyHolderError::Unavailable("no certificate store is used on this system".into()))
}

#[cfg(target_os = "windows")]
fn sign_digest(_host: &Host, held: &Held, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    key_holders::windows_store::sign_digest(&held.certificate.fingerprint, scheme, algorithm, digest)
}

#[cfg(target_os = "macos")]
fn sign_digest(_host: &Host, held: &Held, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    key_holders::macos_keychain::sign_digest(&held.certificate.fingerprint, scheme, algorithm, digest)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn sign_digest(host: &Host, held: &Held, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let holder = key_holders::pkcs11::open_default()?;
    let pin = held.token.as_ref().and_then(|t| host.pin_for(t).map(|p| (t.clone(), p)));
    let result = holder.sign_digest(&held.certificate.fingerprint, scheme, algorithm, digest, pin.as_ref().map(|(t, p)| (t, p)));
    match &result {
        Err(KeyHolderError::PinNeeded(token)) => host.want_pin(token.clone()),
        Err(KeyHolderError::PinIncorrect) => {
            if let Some(token) = &held.token {
                host.forget_pin(token);
                host.want_pin(token.clone());
            }
        }
        _ => {}
    }
    result
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")))]
fn sign_digest(_host: &Host, _held: &Held, _scheme: SignatureScheme, _algorithm: DigestAlgorithm, _digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    Err(KeyHolderError::Unavailable("no certificate store is used on this system".into()))
}

/// How a key of this kind signs: the store's scheme and hash, and the CMS crate's.
fn smime_scheme(kind: KeyKind) -> Option<(SignatureScheme, DigestAlgorithm, mailcraft_smime::SignatureScheme, mailcraft_smime::DigestAlgorithm)> {
    use mailcraft_smime::{DigestAlgorithm as D, SignatureScheme as S};
    match kind {
        KeyKind::Rsa { .. } => Some((SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256, S::RsaPkcs1v15, D::Sha256)),
        KeyKind::EcP256 => Some((SignatureScheme::Ecdsa, DigestAlgorithm::Sha256, S::Ecdsa, D::Sha256)),
        KeyKind::EcP384 => Some((SignatureScheme::Ecdsa, DigestAlgorithm::Sha384, S::Ecdsa, D::Sha384)),
        KeyKind::Other => None,
    }
}

/// A detached CMS signature of `entity` by the certificate `key` names, and its `micalg`.
fn smime(host: &Host, entity: &[u8], key: &SigningKey, now: i64) -> Result<(Vec<u8>, &'static str), String> {
    let fingerprint = sha256_named(&key.fingerprint).ok_or("the certificate can't be named")?;
    let all = held().map_err(|e| e.to_string())?;
    let Some(found) = all.iter().find(|h| h.certificate.fingerprint == fingerprint) else {
        return imported(key);
    };
    let (scheme, algorithm, cms_scheme, cms_digest) = smime_scheme(found.certificate.key).ok_or("its key isn't one that signs mail")?;
    let request = mailcraft_smime::SignRequest {
        content: entity,
        certificate: &found.certificate.der,
        chain: &[],
        digest: cms_digest,
        scheme: cms_scheme,
        signing_time: now,
    };
    let signature = mailcraft_smime::sign(&request, |digest| sign_digest(host, found, scheme, algorithm, digest)).map_err(|e| match e {
        mailcraft_smime::SignError::Holder(KeyHolderError::PinNeeded(token)) => {
            let name = if token.label.is_empty() { token.serial } else { token.label };
            format!("the smartcard {name} needs its PIN: type it when MailCraft asks, then Send Again")
        }
        other => other.to_string(),
    })?;
    Ok((signature, cms_digest.micalg()))
}

/// The SHA-256 a certificate is named by, from its hex.
pub(crate) fn sha256_named(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(bytes.chunks(2)) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

// ---- OpenPGP ----

/// `gpg`, as File › Options › Security finds it (`own_keys::gnupg`).
fn gpg() -> Result<crate::own_keys::gnupg::Gpg, String> {
    use mailcraft_ui_egui::own_keys::Gnupg;
    crate::own_keys::gnupg::find().map_err(|state| match state {
        Gnupg::NotWorking(why) => why,
        _ => "GnuPG isn't installed".to_string(),
    })
}

/// The public keys GnuPG has for `fingerprints` (`gpg --export`), at most [`MAX_EXPORT`] bytes.
/// Each is a fingerprint in hex, checked, so nothing else reaches gpg's command line.
fn exported(gpg: &crate::own_keys::gnupg::Gpg, fingerprints: &[String]) -> Result<Vec<u8>, String> {
    if fingerprints.is_empty() || fingerprints.iter().any(|f| f.is_empty() || f.len() > 64 || !f.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Ok(Vec::new());
    }
    let mut args = vec!["--batch", "--no-tty", "--export-options", "export-minimal", "--export"];
    args.extend(fingerprints.iter().map(String::as_str));
    crate::own_keys::gnupg::run_within(gpg.program(), &args, EXPORT_TIMEOUT, MAX_EXPORT)
}

/// The fingerprints (hex) of the owner's own keys: those GnuPG has a private key for, as
/// File › Options › Security lists them.
fn own_keys(gpg: &crate::own_keys::gnupg::Gpg) -> Result<Vec<String>, String> {
    Ok(gpg.secret_keys()?.into_iter().map(|k| k.fingerprint.chars().filter(char::is_ascii_hexdigit).collect()).collect())
}

/// The key the agent holds, from its `READKEY` answer: `(public-key (rsa (n …)(e …)))` or
/// `(public-key (ecc (curve …)(q …)))`.
pub(crate) fn agent_key(sexp: &mailcraft_assuan::Sexp) -> Option<mailcraft_openpgp::AgentKey<'_>> {
    if let Some(rsa) = sexp.find("rsa") {
        return Some(mailcraft_openpgp::AgentKey::Rsa { n: rsa.value("n")?, e: rsa.value("e")? });
    }
    let ecc = sexp.find("ecc")?;
    let curve = std::str::from_utf8(ecc.value("curve")?).ok()?;
    Some(mailcraft_openpgp::AgentKey::Ecc { curve, q: ecc.value("q")? })
}

/// The hash a key signs with: as large as its curve, SHA-256 for the rest.
pub(crate) fn hash_for(agent: &mailcraft_openpgp::AgentKey<'_>) -> mailcraft_openpgp::HashAlgorithm {
    use mailcraft_openpgp::HashAlgorithm as H;
    match agent {
        mailcraft_openpgp::AgentKey::Ecc { curve, .. } => match curve.trim_matches('"').to_ascii_lowercase().as_str() {
            "nist p-384" | "nistp384" | "secp384r1" | "1.3.132.0.34" => H::Sha384,
            "nist p-521" | "nistp521" | "secp521r1" | "1.3.132.0.35" => H::Sha512,
            _ => H::Sha256,
        },
        mailcraft_openpgp::AgentKey::Rsa { .. } => H::Sha256,
    }
}

/// What gpg-agent signed, as the OpenPGP packet takes it.
fn signature_value(signature: &mailcraft_assuan::Signature) -> Option<mailcraft_openpgp::SignatureValue> {
    match signature.algorithm.as_str() {
        "rsa" => Some(mailcraft_openpgp::SignatureValue::Rsa { s: signature.part("s")?.to_vec() }),
        "ecdsa" | "eddsa" => Some(mailcraft_openpgp::SignatureValue::Ecc { r: signature.part("r")?.to_vec(), s: signature.part("s")?.to_vec() }),
        _ => None,
    }
}

fn agent_hash(hash: mailcraft_openpgp::HashAlgorithm) -> mailcraft_assuan::HashAlgorithm {
    match hash {
        mailcraft_openpgp::HashAlgorithm::Sha256 => mailcraft_assuan::HashAlgorithm::Sha256,
        mailcraft_openpgp::HashAlgorithm::Sha384 => mailcraft_assuan::HashAlgorithm::Sha384,
        mailcraft_openpgp::HashAlgorithm::Sha512 => mailcraft_assuan::HashAlgorithm::Sha512,
    }
}

/// The keys that may sign for what `key` names: that key itself, else every key of the
/// certificate whose primary key it is, newest first (a primary key kept offline with its
/// signing subkey on a smartcard is the usual case).
pub(crate) fn candidates<'a>(keys: &'a [mailcraft_openpgp::SigningKey], key: &SigningKey) -> Vec<&'a mailcraft_openpgp::SigningKey> {
    let named = |f: &mailcraft_openpgp::Fingerprint| f.to_hex().eq_ignore_ascii_case(&key.fingerprint);
    let mut out: Vec<_> = keys.iter().filter(|k| named(k.fingerprint())).collect();
    if out.is_empty() {
        out = keys.iter().filter(|k| named(k.primary())).collect();
        out.sort_by_key(|k| std::cmp::Reverse(k.created()));
    }
    out
}

/// An armoured detached signature of `entity` by the OpenPGP key `key` names, made by gpg-agent,
/// and its `micalg`.
fn openpgp(entity: &[u8], key: &SigningKey, now: i64) -> Result<(String, &'static str), String> {
    let keys = mailcraft_openpgp::signing_keys(&exported(&gpg()?, std::slice::from_ref(&key.fingerprint))?, u64::try_from(now).unwrap_or(0));
    let candidates = candidates(&keys, key);
    if candidates.is_empty() {
        return Err("the OpenPGP key isn't in GnuPG here, or can't sign now".into());
    }
    let mut agent = crate::gpg_agent::connect().map_err(|e| e.to_string())?;
    // The newest of the certificate's keys that may sign whose private key the agent holds.
    let mut best: Option<(usize, mailcraft_assuan::Keygrip, mailcraft_openpgp::HashAlgorithm)> = None;
    for info in agent.list_keys().map_err(|e| e.to_string())? {
        let Ok(public) = agent.read_key(&info.keygrip) else { continue };
        let Some(found) = agent_key(&public) else { continue };
        if let Some(at) = candidates.iter().position(|c| c.matches(&found))
            && best.as_ref().is_none_or(|(b, _, _)| at < *b)
        {
            best = Some((at, info.keygrip, hash_for(&found)));
        }
    }
    let (at, keygrip, hash) = best.ok_or("gpg-agent doesn't hold the OpenPGP key's private key (insert its smartcard?)")?;
    let signer = *candidates.get(at).ok_or("the OpenPGP key can't be found again")?;
    let created = u32::try_from(now).map_err(|_| "the clock is wrong".to_string())?;
    let armoured = mailcraft_openpgp::sign_detached(entity, signer, created, hash, |digest| {
        let digest = mailcraft_assuan::Digest::new(agent_hash(hash), digest).ok_or_else(|| "a digest of the wrong length".to_string())?;
        let signature = agent.sign(&keygrip, &digest, Some(PINENTRY_TEXT)).map_err(|e| e.to_string())?;
        signature_value(&signature).ok_or_else(|| "gpg-agent's signature can't be read".to_string())
    })
    .map_err(|e| e.to_string())?;
    Ok((armoured, hash.micalg()))
}

// ---- What the window lists ----

/// The owner's keys that can sign mail now, for Digital Signatures.
fn choices() -> Result<Vec<SigningChoice>, String> {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    match held() {
        Ok(all) => out.extend(all.into_iter().filter_map(smime_choice)),
        Err(e) => problems.push(e.to_string()),
    }
    match openpgp_choices() {
        Ok(found) => out.extend(found),
        Err(e) => problems.push(e),
    }
    if out.is_empty() && !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(out)
}

fn smime_choice(h: Held) -> Option<SigningChoice> {
    let c = &h.certificate;
    if !c.in_date || !c.for_mail || !c.can_sign || smime_scheme(c.key).is_none() {
        return None;
    }
    let label = c.emails.first().cloned().unwrap_or_else(|| c.subject.clone());
    let key = SigningKey::new(SignatureFormat::Smime, &c.fingerprint_hex(), &label)?;
    Some(SigningChoice { key, addresses: c.emails.clone(), place: h.place, expires: i64::try_from(c.not_after).unwrap_or(0) })
}

/// The OpenPGP keys GnuPG has whose private key gpg-agent holds.
fn openpgp_choices() -> Result<Vec<SigningChoice>, String> {
    let keys = mailcraft_openpgp::signing_keys(
        &{
            let gpg = gpg()?;
            exported(&gpg, &own_keys(&gpg)?)?
        },
        u64::try_from(now()).unwrap_or(0),
    );
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut agent = crate::gpg_agent::connect().map_err(|e| e.to_string())?;
    let mut out: Vec<SigningChoice> = Vec::new();
    for info in agent.list_keys().map_err(|e| e.to_string())? {
        let Ok(public) = agent.read_key(&info.keygrip) else { continue };
        let Some(held) = agent_key(&public) else { continue };
        for k in keys.iter().filter(|k| k.matches(&held)) {
            let label = k.addresses().first().cloned().unwrap_or_default();
            let Some(key) = SigningKey::new(SignatureFormat::OpenPgp, &k.primary().to_hex(), &label) else { continue };
            if out.iter().any(|c| c.key == key) {
                continue;
            }
            let place = if info.card_serial.is_some() { KeyPlace::GnupgCard } else { KeyPlace::Gnupg };
            let expires = k.expires().and_then(|e| i64::try_from(e).ok()).unwrap_or(0);
            out.push(SigningChoice { key, addresses: k.addresses().to_vec(), place, expires });
        }
    }
    Ok(out)
}
