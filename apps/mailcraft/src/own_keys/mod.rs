// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own keys, for File › Options › Security (#30 R13): the host side of
//! `mailcraft_ui_egui::own_keys`. Each job runs on a thread of its own and answers public facts.
//!
//! - Listing: S/MIME certificates with a private key where the system keeps them (the Windows
//!   certificate store, the macOS keychain, PKCS#11 tokens on Linux and FreeBSD; `key_holders`)
//!   and the secret keys of the owner's GnuPG ([`gnupg`]).
//! - GnuPG's fully or ultimately valid User IDs, as (fingerprint, address) pairs (R6).
//! - Importing: a `.p12` goes into the Windows store or the macOS keychain (RSA too: the system
//!   signs with it); elsewhere, and for an OpenPGP secret key everywhere, the file is kept as it
//!   came (still protected by its passphrase) in the system's credential store under
//!   `key:smime:<sha-256>` or `key:openpgp:<fingerprint>`, after the rules of
//!   `mailcraft_engine::private_keys` (a passphrase; no RSA used in-process). Nothing is kept
//!   when there is no credential store. The passphrase is used for the one call and let go of;
//!   no private key opened here outlives the check.

mod gnupg;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../../crates/engine/src/private_keys_fixtures_tests.rs"]
mod fixtures_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use mailcraft_engine::cmd::keys::key_facts;
use mailcraft_engine::model::digital_signature::SignatureFormat;
use mailcraft_engine::private_keys;
use mailcraft_ui_egui::SecretStore;
use mailcraft_ui_egui::own_keys::{
    Gnupg, Imported, ImportedKey, KeyPlace, Listing, MAX_LISTED, OwnKey, OwnKeysAnswer, OwnKeysJob, Passphrase, SmimePlace, store_name,
};
use zeroize::Zeroizing;

/// The most characters of base64 the credential store keeps under one name (16 pieces of 1000).
const MAX_KEPT: usize = 16 * 1000;

#[cfg(target_os = "windows")]
const SMIME_PLACE: SmimePlace = SmimePlace::WindowsStore;
#[cfg(target_os = "macos")]
const SMIME_PLACE: SmimePlace = SmimePlace::Keychain;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const SMIME_PLACE: SmimePlace = SmimePlace::Tokens;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")))]
const SMIME_PLACE: SmimePlace = SmimePlace::None;

/// Do `job`, keeping an imported key in `store`.
pub fn run(job: OwnKeysJob, store: Option<Arc<dyn SecretStore>>) -> OwnKeysAnswer {
    match job {
        OwnKeysJob::List => OwnKeysAnswer::Listed(list()),
        OwnKeysJob::GnupgValid => OwnKeysAnswer::GnupgValid(gnupg::valid()),
        OwnKeysJob::Import { format, file, passphrase } => OwnKeysAnswer::Imported(import(format, &file.0, passphrase.as_ref(), store.as_deref())),
    }
}

fn list() -> Listing {
    let mut listing = Listing { smime: SMIME_PLACE, ..Listing::default() };
    system_certificates(&mut listing);
    match gnupg::find() {
        Ok(gpg) => match gpg.secret_keys() {
            Ok(keys) => {
                listing.gnupg = Gnupg::Found;
                listing.keys.extend(keys);
            }
            Err(why) => listing.gnupg = Gnupg::NotWorking(why),
        },
        Err(state) => listing.gnupg = state,
    }
    listing.keys.truncate(MAX_LISTED);
    listing
}

/// A certificate the system holds a private key for, as the window lists it.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd")), allow(dead_code))]
fn held(h: &crate::key_holders::HeldCertificate, place: KeyPlace) -> OwnKey {
    OwnKey {
        format: SignatureFormat::Smime,
        name: h.subject.clone(),
        addresses: h.emails.clone(),
        fingerprint: key_facts(SignatureFormat::Smime, &h.der).fingerprint,
        not_after: i64::try_from(h.not_after).unwrap_or(i64::MAX),
        expired: !h.in_date,
        place,
        can_sign: h.can_sign,
    }
}

fn system_certificates(listing: &mut Listing) {
    #[cfg(target_os = "windows")]
    match crate::key_holders::windows_store::certificates() {
        Ok(found) => listing.keys.extend(found.iter().map(|h| held(h, KeyPlace::WindowsStore))),
        Err(e) => listing.trouble.push(format!("Windows' certificate store couldn't be read: {e}")),
    }
    #[cfg(target_os = "macos")]
    match crate::key_holders::macos_keychain::certificates() {
        Ok(found) => listing.keys.extend(found.iter().map(|h| held(h, KeyPlace::Keychain))),
        Err(e) => listing.trouble.push(format!("The keychain couldn't be read: {e}")),
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    match crate::key_holders::pkcs11::open_default().and_then(|holder| holder.certificates()) {
        Ok(found) => listing.keys.extend(found.iter().map(|t| held(&t.certificate, KeyPlace::Token(t.token.label.clone())))),
        // No PKCS#11 module installed is no trouble: there are simply no tokens.
        Err(crate::key_holders::KeyHolderError::Unavailable(why)) => log::info!("no PKCS#11 tokens: {why}"),
        Err(e) => listing.trouble.push(format!("The smartcards couldn't be read: {e}")),
    }
    let _ = listing;
}

fn import(format: SignatureFormat, bytes: &[u8], passphrase: Option<&Passphrase>, store: Option<&dyn SecretStore>) -> Result<Imported, String> {
    match format {
        SignatureFormat::OpenPgp => {
            let info = private_keys::openpgp_import(bytes).map_err(|e| e.to_string())?;
            let public = info.public.to_bytes();
            keep(store, format, &info.fingerprint.to_hex(), &public, bytes, Some(public.clone()))
        }
        SignatureFormat::Smime => {
            let passphrase = passphrase.ok_or("A certificate file needs its passphrase.")?;
            smime(bytes, passphrase, store)
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn smime(bytes: &[u8], passphrase: &Passphrase, _store: Option<&dyn SecretStore>) -> Result<Imported, String> {
    use crate::key_holders::{Pin, check_p12};
    check_p12(bytes).map_err(|_| "The file is larger than a certificate file can be.".to_string())?;
    let pin = Pin::new(passphrase.expose().to_string());
    #[cfg(target_os = "windows")]
    let done = crate::key_holders::windows_store::import_p12(bytes, &pin, crate::key_holders::windows_store::ImportProtection::Consent);
    #[cfg(target_os = "macos")]
    let done = crate::key_holders::macos_keychain::import_p12(bytes, &pin).map(drop);
    done.map(|()| Imported::System).map_err(|e| format!("The system didn't import the certificate: {e}"))
}

/// Where the system has no store of its own for S/MIME keys: the rules, then the file kept.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn smime(bytes: &[u8], passphrase: &Passphrase, store: Option<&dyn SecretStore>) -> Result<Imported, String> {
    let certificate = {
        // (The private key opened here is wiped as this block ends.)
        let opened = private_keys::smime_import(bytes, passphrase.expose()).map_err(|e| e.to_string())?;
        opened.certificate.clone()
    };
    let usable = crate::key_holders::usable([certificate.clone()], crate::key_holders::now());
    if !usable.first().is_some_and(|h| h.can_sign) {
        return Err("This certificate isn't for signing mail: its key usage or extended key usage is for something else.".into());
    }
    let hex: String = key_facts(SignatureFormat::Smime, &certificate).fingerprint.chars().filter(char::is_ascii_hexdigit).collect();
    keep(store, SignatureFormat::Smime, &hex, &certificate, bytes, None)
}

/// Keep the key file `bytes` in `store`, named by its fingerprint, with the facts of `public`
/// (a certificate in DER, or an OpenPGP key's public half).
fn keep(
    store: Option<&dyn SecretStore>,
    format: SignatureFormat,
    fingerprint_hex: &str,
    public: &[u8],
    bytes: &[u8],
    half: Option<Vec<u8>>,
) -> Result<Imported, String> {
    let store = store.ok_or("This computer has no credential store to keep a key in, so MailCraft can't keep it.")?;
    let facts = key_facts(format, public);
    let key = ImportedKey {
        format,
        fingerprint: facts.fingerprint,
        name: facts.name,
        addresses: facts.addresses,
        not_after: facts.not_after,
        store_key: store_name(format, fingerprint_hex),
        can_sign: true,
    }
    .sane()
    .ok_or("The key's fingerprint can't be read.")?;
    let text = Zeroizing::new(mailcraft_engine::cmd::base64_encode(bytes));
    if text.len() > MAX_KEPT {
        return Err("The file is too large to keep in the system's credential store.".into());
    }
    store.set(&key.store_key, &text).map_err(|_| "The system's credential store didn't keep the key.".to_string())?;
    Ok(Imported::Kept { key, public: half })
}
