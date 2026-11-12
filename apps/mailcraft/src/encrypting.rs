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
//! One message goes to everybody but the Bcc recipients, encrypted to them and the owner; each
//! Bcc recipient is sent a message of their own, encrypted to them and the owner alone
//! ([`Message::blind_copies`]): one message for everybody would carry each recipient's key
//! (S/MIME names the certificate, OpenPGP the key id), which tells the others who was Bcc'd.
//!
//! The encrypted body is kept as the bytes the message goes as ([`Message::original`]), so SMTP,
//! the copy in Sent Items, Graph's `sendMail` and EWS's `MimeContent` all carry the same bytes; and
//! it is handed back ([`Message::sealed`]) so that the copy kept here in Sent Items is that and
//! never the clear text. One that can't be encrypted is taken out of the run and held in the Outbox
//! with why: it is never sent in the clear (and every path that sends refuses one that isn't,
//! `mailcraft_mime::encryption_missing`).

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::encrypting::BlindCopy;
use mailcraft_model::{Message, Original};

/// The most the encrypted messages of one send take in all (the message itself and one for each Bcc
/// recipient, held at once).
const MAX_TOTAL: usize = 256 * 1024 * 1024;

/// Fill `buf` with random bytes from the operating system.
fn random(buf: &mut [u8]) -> bool {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf).is_ok()
}

/// A random value for this install to keep in its settings (never a secret): 128 bits, in hex. Empty
/// when the system has none to give.
pub(crate) fn new_install_tag() -> String {
    let mut bytes = [0u8; 16];
    if random(&mut bytes) { bytes.iter().map(|b| format!("{b:02x}")).collect() } else { String::new() }
}

/// Encrypt `m` when it is to be: its body becomes the encrypted message it goes as, made now from
/// the message as it is (the signed body when it was signed first), and each Bcc recipient has
/// their own. Why not, in words, when it can't be.
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
    m.blind_copies.clear();
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
    // The owner's own key: the plan's, else the one found. It is in every message.
    let own_keys: Vec<Vec<u8>> = if plan.own_wanted() {
        let hint = m.sign_with.as_ref().map(|k| k.fingerprint.as_str());
        vec![own(plan.format, &plan.own_address, hint, now)?]
    } else {
        plan.recipients.iter().filter(|r| r.own).map(|r| r.key.to_vec()).collect()
    };
    let blind = m.blind_addresses();
    // The message everybody else gets: not the Bcc recipients' keys.
    let mut keys: Vec<Vec<u8>> = plan.recipients.iter().filter(|r| r.own || !blind.contains(&r.address)).map(|r| r.key.to_vec()).collect();
    if plan.own_wanted() {
        keys.extend(own_keys.iter().cloned());
    }
    let main = seal(plan.format, &entity, &keys, now, random)?;
    let mut total = main.len();
    // (One for each Bcc recipient: theirs and the owner's.)
    let mut copies = Vec::new();
    for address in &blind {
        let mut theirs: Vec<Vec<u8>> = if *address == plan.own_address {
            Vec::new()
        } else {
            let entry =
                plan.recipients.iter().find(|r| r.address == *address).ok_or_else(|| "a Bcc recipient has no key to encrypt to".to_string())?;
            vec![entry.key.to_vec()]
        };
        theirs.extend(own_keys.iter().cloned());
        let body = seal(plan.format, &entity, &theirs, now, random)?;
        total = total.saturating_add(body.len());
        if total > MAX_TOTAL {
            return Err("the encrypted messages for the Bcc recipients are too large together".into());
        }
        let original = Original::new(body).ok_or_else(|| "the encrypted message is too large".to_string())?;
        copies.push(BlindCopy { address: address.clone(), original });
    }
    let original = Original::new(main).ok_or_else(|| "the encrypted message is too large".to_string())?;
    m.original = Some(original.clone());
    m.blind_copies = copies;
    if mailcraft_mime::encryption_missing(m) {
        m.original = None;
        m.blind_copies.clear();
        return Err("the encrypted message is too large".into());
    }
    // What the copy kept in Sent Items is (the main message, never the clear text).
    m.sealed.put(original);
    Ok(())
}

/// The body of an Encrypted message of `entity` for `keys`, in `format`.
pub(crate) fn seal(
    format: SignatureFormat,
    entity: &[u8],
    keys: &[Vec<u8>],
    now: i64,
    random: &mut dyn FnMut(&mut [u8]) -> bool,
) -> Result<Vec<u8>, String> {
    match format {
        SignatureFormat::Smime => {
            let request = mailcraft_smime::EncryptRequest { content: entity, recipients: keys, now };
            let der = mailcraft_smime::encrypt(&request, random).map_err(|e| e.to_string())?;
            mailcraft_mime::smime_enveloped(&der).map_err(|e| e.to_string())
        }
        SignatureFormat::OpenPgp => {
            let armoured = mailcraft_openpgp::encrypt(entity, keys, u64::try_from(now).unwrap_or(0), random).map_err(|e| e.to_string())?;
            mailcraft_mime::multipart_encrypted(armoured.as_bytes()).map_err(|e| e.to_string())
        }
    }
}

/// The owner's own Public key in `format` for `address`: the one for the key they sign with when
/// that can be encrypted to, else any of theirs for the address.
pub(crate) fn own_key(format: SignatureFormat, address: &str, signing: Option<&str>, now: i64) -> Result<Vec<u8>, String> {
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

/// How long the owner's own key found for an address is kept (a draft is saved every few seconds,
/// and finding an OpenPGP key asks GnuPG).
const OWN_KEY_KEPT: std::time::Duration = std::time::Duration::from_secs(300);

/// Keeps a draft that is to be Encrypted Encrypted (#30 part 7): to the owner's own key alone, with
/// the operating system's random numbers.
pub(crate) struct DraftSealer {
    found: std::sync::Mutex<Vec<(SignatureFormat, String, std::time::Instant, Vec<u8>)>>,
}

impl DraftSealer {
    pub(crate) fn new() -> DraftSealer {
        DraftSealer { found: std::sync::Mutex::new(Vec::new()) }
    }
}

impl mailcraft_engine::cmd::encrypting::Sealer for DraftSealer {
    fn seal(&self, format: SignatureFormat, address: &str, entity: &[u8], now: i64) -> Result<Vec<u8>, String> {
        let kept = self.found.lock().ok().and_then(|f| {
            f.iter().find(|(fm, a, at, _)| *fm == format && a == address && at.elapsed() < OWN_KEY_KEPT).map(|(_, _, _, key)| key.clone())
        });
        let key = match kept {
            Some(key) => key,
            None => {
                let key = own_key(format, address, None, now)?;
                if let Ok(mut f) = self.found.lock() {
                    f.retain(|(fm, a, _, _)| !(*fm == format && a == address));
                    f.push((format, address.to_string(), std::time::Instant::now(), key.clone()));
                }
                key
            }
        };
        seal(format, entity, &[key], now, &mut random)
    }
}
