// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypted messages opened (#30 part 6): the one place that decides which key holder opens
//! what, on a thread of its own, for the window ([`DecryptHost`]).
//!
//! **No RSA private-key operation is ever done in this process** (decided 2026-11-09):
//!
//! - S/MIME: the system's store (the Windows certificate store, the macOS keychain, a PKCS#11
//!   token on Linux and FreeBSD) is handed the wrapped content-encryption key and gives the key
//!   back (`key_holders`); the content is decrypted here by `mailcraft_smime::decrypt`, which
//!   has no private key to use. ECDH certificates aren't read (nothing lists them).
//! - OpenPGP: gpg-agent is handed the encrypted session key (RSA) or the ephemeral point (ECDH,
//!   X25519) and gives back the session key or the shared secret, from which
//!   `mailcraft_openpgp::session_key` derives and unwraps the session key. The public keys come
//!   from `gpg --export`, run as a program and never through a shell, and are matched to the
//!   agent's keys by their key material.
//!
//! What is opened is given to the window as [`Plain`], which overwrites its bytes when dropped,
//! and goes nowhere else: no file, no log, no command's params. A PIN is asked and held as
//! signing's is ([`crate::signing::Host`]). Every failure after a key was found is one
//! [`Failure::Failed`]: nothing here tells a bad padding from a wrong key.

use std::sync::mpsc::Receiver;

use mailcraft_assuan::{ErrorKind, Sexp};
use mailcraft_engine::decrypt::{DecryptJob, Failure, Plain};
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_openpgp::AgentRequest;
use mailcraft_smime::{KeyTransport, Unwrapped};
use mailcraft_ui_egui::decrypting::DecryptHost;

use crate::key_holders::{KeyHolderError, KeyKind, RsaPadding};
use crate::signing::{self, Held, Host};

/// What gpg-agent's pinentry says when it asks for a passphrase or a PIN to open a message.
const PINENTRY_TEXT: &str = "MailCraft is opening an Encrypted message you received.";
/// How many of a message's recipients are asked of the agent.
const MAX_AGENT_TRIES: usize = 8;

impl DecryptHost for Host {
    fn open(&self, job: DecryptJob) -> Receiver<Result<Plain, Failure>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("decrypting".into()).spawn(move || {
            let host = signing::host();
            let _ = tx.send(open(&host, &job));
        });
        if let Err(e) = spawned {
            log::warn!("an Encrypted message couldn't be opened: {e}");
        }
        rx
    }
}

/// Open `job`'s message with the keys this computer holds.
pub(crate) fn open(host: &Host, job: &DecryptJob) -> Result<Plain, Failure> {
    match job.format {
        SignatureFormat::Smime => smime(host, &job.original),
        SignatureFormat::OpenPgp => openpgp(&job.original),
    }
}

// ---- S/MIME ----

pub(crate) fn padding(transport: KeyTransport) -> RsaPadding {
    match transport {
        KeyTransport::Pkcs1v15 => RsaPadding::Pkcs1v15,
        KeyTransport::OaepSha1 => RsaPadding::OaepSha1,
        KeyTransport::OaepSha256 => RsaPadding::OaepSha256,
    }
}

#[cfg(target_os = "windows")]
fn decrypt_key(_host: &Host, held: &Held, padding: RsaPadding, wrapped: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyHolderError> {
    crate::key_holders::windows_store::decrypt_key(&held.certificate.fingerprint, padding, wrapped)
}

#[cfg(target_os = "macos")]
fn decrypt_key(_host: &Host, held: &Held, padding: RsaPadding, wrapped: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyHolderError> {
    crate::key_holders::macos_keychain::decrypt_key(&held.certificate.fingerprint, padding, wrapped)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn decrypt_key(host: &Host, held: &Held, padding: RsaPadding, wrapped: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyHolderError> {
    let holder = crate::key_holders::pkcs11::open_default()?;
    let pin = held.token.as_ref().and_then(|t| host.pin_for(t).map(|p| (t.clone(), p)));
    holder.decrypt_key(&held.certificate.fingerprint, padding, wrapped, pin.as_ref().map(|(t, p)| (t, p)))
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")))]
fn decrypt_key(_host: &Host, _held: &Held, _padding: RsaPadding, _wrapped: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, KeyHolderError> {
    Err(KeyHolderError::Unavailable("no certificate store is used on this system".into()))
}

/// What the key holder's answer means to the decryption. A refusal the holder gives for a key
/// that doesn't open this message (a bad padding comes as a system error on some stores) is
/// `Refused`, alike for all: the next recipient is tried. Only what the owner can fix, or an
/// unreachable holder, stops it.
fn unwrapped(host: &Host, held: &Held, result: Result<zeroize::Zeroizing<Vec<u8>>, KeyHolderError>) -> Unwrapped<Failure> {
    match result {
        Ok(key) => Unwrapped::Key(key),
        Err(KeyHolderError::PinNeeded(token)) => {
            let name = if token.label.is_empty() { token.serial.clone() } else { token.label.clone() };
            host.want_pin(token);
            Unwrapped::Stop(Failure::PinNeeded(name))
        }
        Err(KeyHolderError::PinIncorrect) => match &held.token {
            Some(token) => {
                host.forget_pin(token);
                host.want_pin(token.clone());
                let name = if token.label.is_empty() { token.serial.clone() } else { token.label.clone() };
                Unwrapped::Stop(Failure::PinNeeded(name))
            }
            None => Unwrapped::Stop(Failure::Failed),
        },
        Err(KeyHolderError::Cancelled) => Unwrapped::Stop(Failure::Cancelled),
        Err(KeyHolderError::Unavailable(_) | KeyHolderError::TokenChanged | KeyHolderError::PinLocked | KeyHolderError::PinExpired) => {
            Unwrapped::Stop(Failure::Unavailable)
        }
        Err(_) => Unwrapped::Refused,
    }
}

fn smime(host: &Host, original: &[u8]) -> Result<Plain, Failure> {
    let parts = mailcraft_mime::signed_parts(original).filter(|p| p.kind == mailcraft_mime::SignedKind::SmimeOpaque).ok_or(Failure::Failed)?;
    let all = signing::held().map_err(|_| Failure::Unavailable)?;
    // Only RSA certificates that may unwrap keys: nothing here lists an ECDH one.
    let mine: Vec<&Held> = all.iter().filter(|h| h.certificate.can_decrypt && matches!(h.certificate.key, KeyKind::Rsa { .. })).collect();
    let ders: Vec<&[u8]> = mine.iter().map(|h| h.certificate.der.as_slice()).collect();
    let opened = mailcraft_smime::decrypt(&parts.signature, &ders, |at, transport, wrapped| match mine.get(at) {
        Some(held) => unwrapped(host, held, decrypt_key(host, held, padding(transport), wrapped)),
        None => Unwrapped::Refused,
    });
    match opened {
        Ok(d) => Ok(Plain { data: d.content, inline: false, signatures: Vec::new(), authenticated: d.authenticated }),
        Err(mailcraft_smime::DecryptError::NoKey) => Err(Failure::NoKey),
        Err(mailcraft_smime::DecryptError::Unsupported) => Err(Failure::Unsupported),
        Err(mailcraft_smime::DecryptError::Holder(why)) => Err(why),
        Err(_) => Err(Failure::Failed),
    }
}

// ---- OpenPGP ----

/// `(enc-val (rsa (a …)))` or `(enc-val (ecdh (s …) (e …)))`.
pub(crate) fn enc_val(request: AgentRequest) -> Sexp {
    let pair = |name: &str, value: Vec<u8>| Sexp::List(vec![Sexp::token(name), Sexp::Atom(value)]);
    let (algorithm, parts) = match request {
        // (A leading zero keeps a value with its top bit set from being read as negative.)
        AgentRequest::Rsa { a } => ("rsa", vec![pair("a", signed(a))]),
        AgentRequest::Ecdh { s, e } => ("ecdh", vec![pair("s", s), pair("e", e)]),
    };
    let mut inner = vec![Sexp::token(algorithm)];
    inner.extend(parts);
    Sexp::List(vec![Sexp::token("enc-val"), Sexp::List(inner)])
}

pub(crate) fn signed(mut value: Vec<u8>) -> Vec<u8> {
    if value.first().is_some_and(|b| b & 0x80 != 0) {
        value.insert(0, 0);
    }
    value
}

fn openpgp(original: &[u8]) -> Result<Plain, Failure> {
    let message = mailcraft_openpgp::encrypted_message(original).ok_or(Failure::Failed)?;
    let gpg = signing::gpg().map_err(|_| Failure::Unavailable)?;
    let fingerprints = signing::own_keys(&gpg).map_err(|_| Failure::Unavailable)?;
    let export = signing::exported(&gpg, &fingerprints).map_err(|_| Failure::Unavailable)?;
    let keys = mailcraft_openpgp::decryption_keys(&export);
    let mut agent = crate::gpg_agent::connect().map_err(|_| Failure::Unavailable)?;
    // The agent's keys that are one of the owner's decryption keys: (index into `keys`, keygrip).
    let mut held = Vec::new();
    for info in agent.list_keys().map_err(|_| Failure::Unavailable)? {
        let Ok(public) = agent.read_key(&info.keygrip) else { continue };
        let Some(found) = signing::agent_key(&public) else { continue };
        if let Some(at) = keys.iter().position(|k| k.matches(&found)) {
            held.push((at, info.keygrip));
        }
    }
    let mut tried = 0usize;
    let mut any = false;
    // Why nothing opened, when the agent said so: what the owner can fix, said as that.
    let mut problem: Option<Failure> = None;
    let mut unsupported = false;
    for recipient in message.recipients() {
        let Some(request) = mailcraft_openpgp::agent_request(recipient) else { continue };
        for (at, keygrip) in &held {
            let Some(key) = keys.get(*at) else { continue };
            if !key.is_recipient(recipient) {
                continue;
            }
            any = true;
            // ECDH on a version 6 key isn't opened (its KDF names a 32-octet fingerprint): said so,
            // and the agent isn't asked for it.
            if key.is_v6() && matches!(recipient.esk, mailcraft_openpgp::Esk::Ecdh { .. }) {
                unsupported = true;
                continue;
            }
            tried += 1;
            if tried > MAX_AGENT_TRIES {
                return Err(Failure::Failed);
            }
            let answer = match agent.decrypt(keygrip, &enc_val(request.clone()), Some(PINENTRY_TEXT)) {
                Ok(answer) => answer,
                Err(e) => match refusal(e.server().map(mailcraft_assuan::ServerError::kind)) {
                    Step::Stop(failure) => return Err(failure),
                    Step::Next(why) => {
                        problem = problem.or(why);
                        continue;
                    }
                },
            };
            let Some(session) = mailcraft_openpgp::session_key(recipient, key, answer.bytes(), answer.padding()) else { continue };
            // (OpenPGP's data packets are integrity-protected: rPGP refuses the old kind, and checks
            // the check before it gives anything.)
            return match message.decrypt(&session) {
                Ok(plain) => Ok(Plain { data: plain.data, inline: message.inline, signatures: plain.signatures, authenticated: true }),
                Err(_) => Err(Failure::Failed),
            };
        }
    }
    Err(problem.or(unsupported.then_some(Failure::Unsupported)).unwrap_or(if any { Failure::Failed } else { Failure::NoKey }))
}

/// What to do about gpg-agent refusing (`ERR`) to open a session key.
pub(crate) enum Step {
    /// Nothing more is asked: why.
    Stop(Failure),
    /// The next key is tried; if none opens it, why that is, when the agent said.
    Next(Option<Failure>),
}

/// What gpg-agent's refusal means. These come from the owner's own agent, not from the message,
/// so naming them tells a sender nothing: a wrong passphrase (the agent has asked three times
/// already) and a key it hasn't a secret half of are the owner's to deal with.
pub(crate) fn refusal(kind: Option<ErrorKind>) -> Step {
    match kind {
        Some(ErrorKind::Canceled) => Step::Stop(Failure::Cancelled),
        Some(ErrorKind::NoPinentry | ErrorKind::Timeout) | None => Step::Stop(Failure::Unavailable),
        Some(ErrorKind::BadPassphrase | ErrorKind::BadPin) => Step::Stop(Failure::WrongPassphrase),
        Some(ErrorKind::NoSecretKey) => Step::Next(Some(Failure::NoKey)),
        Some(_) => Step::Next(None),
    }
}
