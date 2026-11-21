// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules for a private key the owner imports into MailCraft (#30 R13), each in one place so
//! that it can be changed in one place:
//!
//! - [`passphrase_rule`]: an OpenPGP private key without a passphrase (or with an empty one) is
//!   refused, and nothing of it is kept; the owner is told to add one in GnuPG and import it
//!   again. [`encrypted_key_rule`] is the same for a `.p12` MailCraft keeps: its private key must
//!   be encrypted in the file, not merely vouched for by its MAC.
//! - [`in_process_openpgp`] and [`in_process_smime`]: an RSA private-key operation never runs
//!   inside MailCraft. RSA keys sign and decrypt through gpg-agent or the system's own stores
//!   (the Windows certificate store, the macOS keychain, a PKCS#11 token); a key MailCraft keeps
//!   itself and uses in-process must be Ed25519 or ECDSA (OpenPGP: every part of it, the
//!   subkeys for encryption too), or ECDSA on P-256 or P-384 (S/MIME, by the private key's own
//!   algorithm). An import into the Windows store or the macOS keychain is the system's, and RSA
//!   is fine there.
//! - [`openpgp_import`]: a file holds one secret key and nothing else, since the whole file is
//!   what is kept.
//!
//! No I/O, nothing kept: the host ([`crate`]'s caller, `apps/mailcraft`) reads the file, calls
//! these, and keeps the file, still protected by its passphrase, in the system's credential store.
//! The refusals are words for the owner, in the window's language.

use mailcraft_openpgp::secret::{SecretKeyInfo, SigningKind};
use mailcraft_smime::pkcs12::{self, KeyKind, Opened};

use crate::CmdError;
use crate::cmd::fail;

/// Rule 1 (Martin, 2026-11-11): an OpenPGP private key MailCraft keeps has a passphrase, and
/// not an empty one.
pub fn passphrase_rule(protected: bool) -> Result<(), CmdError> {
    if protected {
        Ok(())
    } else {
        Err(fail(
            "This key has no passphrase, so MailCraft doesn't keep it. Add one in GnuPG (gpg --edit-key <key>, then passwd) and import it again.",
        ))
    }
}

/// Rule 1 for a `.p12` MailCraft keeps: the private key is encrypted in the file (a shrouded
/// key bag, or a bag in an encrypted safe), so that the file kept is protected by its passphrase.
pub fn encrypted_key_rule(encrypted: bool) -> Result<(), CmdError> {
    if encrypted {
        Ok(())
    } else {
        Err(fail(
            "The private key in this file isn't encrypted, so MailCraft doesn't keep it: export it again with its key encrypted (AES), protected by a passphrase.",
        ))
    }
}

/// Rule 2 for OpenPGP: a key MailCraft keeps signs with EdDSA or ECDSA, never RSA, and has no
/// RSA part at all (`any_rsa`: a subkey for encryption too).
pub fn in_process_openpgp(kind: Option<SigningKind>, any_rsa: bool) -> Result<(), CmdError> {
    let rsa = || {
        fail(
            "This is an RSA key, which MailCraft doesn't use by itself: keep it in GnuPG, whose agent MailCraft signs with, and leave it out of here.",
        )
    };
    match kind {
        Some(SigningKind::Rsa) => Err(rsa()),
        _ if any_rsa => Err(fail(
            "A part of this key is RSA (a subkey for encryption, perhaps), which MailCraft doesn't use by itself: keep the key in GnuPG, whose agent MailCraft signs with, and leave it out of here.",
        )),
        Some(SigningKind::EdDsa | SigningKind::Ecdsa) => Ok(()),
        Some(SigningKind::Other) => {
            Err(fail("This key signs with an algorithm MailCraft doesn't use: only Ed25519 and ECDSA keys are imported here."))
        }
        None => Err(fail("This key can't make Digital signatures: none of its keys may sign, or its secret parts aren't in the file.")),
    }
}

/// Rule 2 for S/MIME: a private key MailCraft keeps (where the system has no store of its own
/// for it) is ECDSA on P-256 or P-384, never RSA: its kind as its own PKCS #8 algorithm says.
pub fn in_process_smime(kind: KeyKind) -> Result<(), CmdError> {
    match kind {
        KeyKind::P256 | KeyKind::P384 => Ok(()),
        KeyKind::Rsa { .. } => Err(fail(
            "This is an RSA certificate, which MailCraft doesn't use by itself on this system: put it on a smartcard or token (PKCS#11), where MailCraft finds it.",
        )),
        KeyKind::Other => Err(fail("This certificate's key is of a kind MailCraft doesn't use: only ECDSA on P-256 or P-384 is imported here.")),
    }
}

/// An OpenPGP secret key file to import, read without using anything secret, and every rule it
/// must pass: it reads, it is one key, it can sign, it has a passphrase, it is EdDSA or ECDSA
/// throughout.
pub fn openpgp_import(bytes: &[u8]) -> Result<SecretKeyInfo, CmdError> {
    let info = mailcraft_openpgp::secret::read(bytes).map_err(|e| match e {
        mailcraft_openpgp::Error::TooLarge => fail("The file is larger than an OpenPGP key can be."),
        mailcraft_openpgp::Error::Unreadable => fail("The file holds no OpenPGP secret key that can be read."),
        mailcraft_openpgp::Error::SeveralKeys => {
            fail("The file holds more than one key, or something besides the key: export only the one secret key to import (gpg --armor --export-secret-keys <key>).")
        }
    })?;
    passphrase_rule(info.protected)?;
    if !info.can_sign && !info.any_rsa {
        return Err(fail("This key can't make Digital signatures: none of its keys may sign, or its secret parts aren't in the file."));
    }
    in_process_openpgp(info.signing_kind, info.any_rsa)?;
    Ok(info)
}

/// A `.p12` opened with its passphrase, to be kept by MailCraft: the file's own refusals in
/// words, [`encrypted_key_rule`] and [`in_process_smime`]. The private key in what it gives is
/// wiped when let go of: the caller lets go of it at once.
pub fn smime_import(p12: &[u8], passphrase: &str) -> Result<Opened, CmdError> {
    let opened = pkcs12::open(p12, passphrase).map_err(|e| {
        fail(match e {
            pkcs12::Error::TooLarge => "The file is larger than a certificate file can be.",
            pkcs12::Error::Unreadable => "The file isn't a certificate file (.p12 or .pfx) that can be read.",
            pkcs12::Error::NoMac => "The file isn't protected with a passphrase, so nothing vouches for it.",
            pkcs12::Error::WrongPassphrase => "The passphrase is wrong.",
            pkcs12::Error::WeakEncryption => {
                "The file is encrypted in a way that is no longer safe (RC2, RC4, DES or 40-bit keys): export it again with AES."
            }
            pkcs12::Error::UnknownAlgorithm => "The file uses an algorithm MailCraft doesn't know.",
            pkcs12::Error::Unreasonable => "The file asks for more work than a certificate file needs.",
            pkcs12::Error::NoPrivateKey => "The file holds no private key, or more than one.",
            pkcs12::Error::NoCertificate => "The file holds no certificate for its private key.",
        })
    })?;
    in_process_smime(opened.key_kind)?;
    encrypted_key_rule(opened.key_encrypted)?;
    Ok(opened)
}
