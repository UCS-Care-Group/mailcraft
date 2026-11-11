// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting at Send (#30 part 7): the one place that turns a message to be Encrypted into the
//! bytes it goes as, after it has been Digitally signed when it is to be ([`crate::signing`]
//! calls this once its signature is made, so that the signed part is what goes inside).
//!
//! A message the owner sent with Encrypt on carries what it is encrypted to
//! ([`Message::encrypt_with`]): a Public key for each recipient, copied from the mailbox's key
//! store when Send was pressed, and the owner's own when the store had one. Here the owner's own
//! key is found when it didn't (R17: Sent Items stays readable): an S/MIME certificate of theirs in
//! the system's store, or an OpenPGP key GnuPG has for their address. Only Public keys are used,
//! nothing here opens a private key, and the random numbers come from the operating system.
//!
//! The encrypted body is kept as the bytes the message goes as ([`Message::original`]), so SMTP,
//! the copy in Sent Items, Graph's `sendMail` and EWS's `MimeContent` all carry the same bytes.
//! One that can't be encrypted is taken out of the run and held in the Outbox with why: it is
//! never sent in the clear (and every path that sends refuses one that isn't encrypted,
//! `mailcraft_mime::encryption_missing`).

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::{Message, Original};

/// Fill `buf` with random bytes from the operating system.
fn random(buf: &mut [u8]) -> bool {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf).is_ok()
}

/// Encrypt `m` when it is to be: its body becomes the encrypted message it goes as, made now from
/// the message as it is (the signed body when it was signed first). Why not, in words, when it
/// can't be.
pub(crate) fn encrypt(m: &mut Message, now: i64) -> Result<(), String> {
    encrypt_with(m, now, &mut random, &own_key)
}

/// [`encrypt`] with its random numbers and the finding of the owner's own key handed in.
pub(crate) fn encrypt_with(
    m: &mut Message,
    now: i64,
    random: &mut dyn FnMut(&mut [u8]) -> bool,
    own: &dyn Fn(SignatureFormat, &str, Option<&str>, i64) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let Some(plan) = m.encrypt_with.clone() else {
        // Asked for and nothing decided what it is encrypted to: never sent in the clear.
        return if mailcraft_mime::encryption_missing(m) { Err("it wasn't decided whom it is encrypted to".into()) } else { Ok(()) };
    };
    let plan = plan.checked().ok_or_else(|| "what it is to be encrypted to can't be read".to_string())?;
    if m.sign_with.is_none() {
        // (Whatever bytes it held before are let go of: made again from the message as it is.)
        m.original = None;
    } else if mailcraft_mime::signature_missing(m) {
        // Signed first and then encrypted: without its signature it goes nowhere.
        return Err("it wasn't Digitally signed, which comes first".into());
    }
    let entity = mailcraft_mime::entity_to_encrypt(m);
    let mut keys: Vec<Vec<u8>> = plan.recipients.iter().map(|r| r.key.to_vec()).collect();
    if plan.own_wanted() {
        let hint = m.sign_with.as_ref().map(|k| k.fingerprint.as_str());
        keys.push(own(plan.format, &plan.own_address, hint, now)?);
    }
    let body = match plan.format {
        SignatureFormat::Smime => {
            let request = mailcraft_smime::EncryptRequest { content: &entity, recipients: &keys, now };
            let der = mailcraft_smime::encrypt(&request, random).map_err(|e| e.to_string())?;
            mailcraft_mime::smime_enveloped(&der).map_err(|e| e.to_string())?
        }
        SignatureFormat::OpenPgp => {
            let armoured = mailcraft_openpgp::encrypt(&entity, &keys, u64::try_from(now).unwrap_or(0), random).map_err(|e| e.to_string())?;
            mailcraft_mime::multipart_encrypted(armoured.as_bytes()).map_err(|e| e.to_string())?
        }
    };
    m.original = Original::new(body);
    if mailcraft_mime::encryption_missing(m) {
        m.original = None;
        return Err("the encrypted message is too large".into());
    }
    Ok(())
}

/// The owner's own Public key in `format` for `address`: the one for the key they sign with when
/// that can be encrypted to, else any of theirs for the address.
fn own_key(format: SignatureFormat, address: &str, signing: Option<&str>, now: i64) -> Result<Vec<u8>, String> {
    let found = match format {
        SignatureFormat::Smime => own_smime(address, signing, now),
        SignatureFormat::OpenPgp => own_openpgp(address, signing, now),
    };
    found.ok_or_else(|| {
        "none of your own keys for this account could be found to encrypt to as well, so Sent Items couldn't be read: import your certificate or key as a Public key (File › Options › Security), or turn Encrypt off".to_string()
    })
}

fn own_smime(address: &str, signing: Option<&str>, now: i64) -> Option<Vec<u8>> {
    let held = crate::signing::held().ok()?;
    let signing = signing.and_then(crate::signing::sha256_named);
    held.iter()
        .filter(|h| {
            h.certificate.emails.iter().any(|e| e.eq_ignore_ascii_case(address)) && mailcraft_smime::can_encrypt_to(&h.certificate.der, now).is_ok()
        })
        .min_by_key(|h| Some(h.certificate.fingerprint) != signing)
        .map(|h| h.certificate.der.clone())
}

fn own_openpgp(address: &str, signing: Option<&str>, now: i64) -> Option<Vec<u8>> {
    let gpg = crate::signing::gpg().ok()?;
    let export = crate::signing::exported(&gpg, &crate::signing::own_keys(&gpg).ok()?).ok()?;
    let found = mailcraft_openpgp::encryption_certificates(&export, u64::try_from(now).unwrap_or(0));
    found
        .into_iter()
        .filter(|c| c.addresses.iter().any(|a| a.eq_ignore_ascii_case(address)))
        .min_by_key(|c| !signing.is_some_and(|s| s.eq_ignore_ascii_case(&c.fingerprint)))
        .map(|c| c.bytes)
}
