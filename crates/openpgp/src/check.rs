// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Checking one Digital signature over bytes, and what came of it (R4, R6).

use pgp::composed::{Deserializable, DetachedSignature};
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::packet::{Signature, SignatureType};
use pgp::types::KeyVersion;

use crate::keys::{self, Fingerprint, KeyId, KeyRing, PublicKey, Signing};

/// How many signatures over one text are looked at.
pub(crate) const MAX_SIGNATURES: usize = 8;
/// How many bytes a block of signatures may hold, decoded.
pub(crate) const MAX_SIGNATURE_BYTES: usize = 64 * 1024;

/// Who made a Digital signature and when, for the details (R4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signer {
    /// The primary key's fingerprint: what the owner compares and marks Verified.
    pub fingerprint: Fingerprint,
    /// The fingerprint of the key that signed, when it is a subkey.
    pub subkey: Option<Fingerprint>,
    /// The key id of the key that signed.
    pub key_id: KeyId,
    /// The key's addresses (from its own User IDs), lower case.
    pub addresses: Vec<String>,
    /// When the signature says it was made, in seconds since 1970.
    pub signed: Option<u64>,
    /// When the primary key was made, in seconds since 1970.
    pub key_created: u64,
    /// The signing key's algorithm ("RSA", "Ed25519", …).
    pub algorithm: &'static str,
    /// The signature's hash ("SHA-256", …).
    pub hash: &'static str,
}

/// Who a signature that couldn't be checked names as its signer, as far as it says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Issuer {
    pub fingerprint: Option<Fingerprint>,
    pub key_id: Option<KeyId>,
}

/// Why a valid Digital signature isn't Verified (R4): either or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotVerified {
    /// The owner hasn't marked the key Verified, nor does their GnuPG keyring trust it fully.
    pub key_unverified: bool,
    /// The From address isn't one of the key's own.
    pub address_differs: bool,
}

/// Why a Digital signature can't be checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CantCheck {
    /// No Public key here for the key it names.
    NoPublicKey,
    /// Made with a hash that is no longer safe (MD5, SHA-1, RIPEMD-160): "uses an algorithm
    /// that's no longer safe". The hash's name.
    WeakAlgorithm(&'static str),
    /// An algorithm or signature version this program doesn't know.
    UnknownAlgorithm,
    /// The signing key was revoked by its owner.
    KeyRevoked,
    /// The signing key had expired when the signature was made.
    KeyExpired,
    /// The key that matches isn't one that may sign (or its subkey didn't sign back that it
    /// belongs to the key).
    NotASigningKey,
    /// The signature itself has expired.
    SignatureExpired,
    /// The signature, or what it is over, can't be read, or is larger than any real one.
    Unreadable,
}

/// What came of checking a message's OpenPGP Digital signature: the four results of R4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Valid, the key Verified, and the From address one of the key's.
    Verified(Signer),
    /// Valid, but the key isn't Verified or the From address isn't the key's.
    Valid { signer: Signer, why: NotVerified },
    /// The message was changed after it was signed: a Public key here for the key it names
    /// doesn't match it.
    Changed(Signer),
    /// It can't be checked, and why; who it names as its signer.
    CantCheck { why: CantCheck, issuer: Issuer },
}

impl Outcome {
    /// Which outcome to show of several signatures over one text: the best.
    fn rank(&self) -> u8 {
        match self {
            Outcome::Verified(_) => 3,
            Outcome::Valid { .. } => 2,
            Outcome::Changed(_) => 1,
            Outcome::CantCheck { .. } => 0,
        }
    }
}

/// What the caller hands in to check against.
pub(crate) struct Context<'a> {
    pub(crate) from: &'a str,
    pub(crate) keys: &'a KeyRing,
    pub(crate) now: u64,
    pub(crate) verified: &'a dyn Fn(&Fingerprint) -> bool,
}

/// Checks the signature packets `packets` (binary) over `data`; the best outcome of them.
pub(crate) fn check(packets: &[u8], data: &[u8], cx: &Context<'_>) -> Outcome {
    let unreadable = Outcome::CantCheck { why: CantCheck::Unreadable, issuer: Issuer::default() };
    if packets.len() > MAX_SIGNATURE_BYTES {
        return unreadable;
    }
    let parsed = crate::guard(|| DetachedSignature::from_bytes_many(packets).map(|it| it.take(MAX_SIGNATURES).collect::<Vec<_>>()));
    let Some(Ok(signatures)) = parsed else { return unreadable };
    let mut best: Option<Outcome> = None;
    for signature in signatures.into_iter().filter_map(Result::ok) {
        let outcome = crate::guard(|| one(&signature.signature, data, cx)).unwrap_or_else(|| unreadable.clone());
        if best.as_ref().is_none_or(|b| outcome.rank() > b.rank()) {
            best = Some(outcome);
        }
    }
    best.unwrap_or(unreadable)
}

fn one(sig: &Signature, data: &[u8], cx: &Context<'_>) -> Outcome {
    let fps: Vec<Fingerprint> = sig.issuer_fingerprint().iter().map(|fp| Fingerprint::of(fp)).collect();
    let ids: Vec<KeyId> = sig.issuer_key_id().iter().map(|id| KeyId::of(id)).collect();
    let issuer = Issuer { fingerprint: fps.first().cloned(), key_id: ids.first().copied() };
    let cant = |why: CantCheck| Outcome::CantCheck { why, issuer: issuer.clone() };
    let Some(config) = sig.config() else { return cant(CantCheck::UnknownAlgorithm) };
    if !matches!(config.typ, SignatureType::Binary | SignatureType::Text) {
        return cant(CantCheck::Unreadable);
    }
    if keys::is_weak(config.hash_alg) {
        return cant(CantCheck::WeakAlgorithm(keys::hash_name(config.hash_alg)));
    }
    if config.hash_alg.new_hasher().is_err() || keys::hash_name(config.hash_alg) == "unknown" || !known_signature_algorithm(config.pub_alg) {
        return cant(CantCheck::UnknownAlgorithm);
    }
    // The keys it names; a signature that names none may be by any key of the From address.
    let named = !(fps.is_empty() && ids.is_empty());
    let candidates: Vec<(&PublicKey, &Signing)> = if named {
        cx.keys.signers(&fps, &ids).collect()
    } else {
        cx.keys.for_address(cx.from).flat_map(|k| k.signing.iter().map(move |s| (k, s))).collect()
    };
    let signed = sig.created().map(|t| u64::from(t.as_secs()));
    let mut failed: Option<Signer> = None;
    for (key, signing) in candidates {
        // A version 6 signature is made by a version 6 key and only by one (RFC 9580 §5.2.3).
        if (signing.version == KeyVersion::V6) != (sig.version() == pgp::packet::SignatureVersion::V6) || signing.algorithm != config.pub_alg {
            continue;
        }
        let signer = Signer {
            fingerprint: key.fingerprint().clone(),
            subkey: signing.subkey.map(|_| signing.fingerprint.clone()),
            key_id: signing.key_id,
            addresses: key.addresses().to_vec(),
            signed,
            key_created: key.created(),
            algorithm: keys::algorithm_name(signing.algorithm),
            hash: keys::hash_name(config.hash_alg),
        };
        if !verify_with(sig, key, signing, data) {
            failed.get_or_insert(signer);
            continue;
        }
        return valid(sig, signing, signer, signed, cx).unwrap_or_else(|why| Outcome::CantCheck { why, issuer: issuer.clone() });
    }
    match failed {
        Some(signer) if named => Outcome::Changed(signer),
        _ => cant(CantCheck::NoPublicKey),
    }
}

/// A signature that checks: whether its key could make it then, and whether it is Verified.
fn valid(sig: &Signature, signing: &Signing, signer: Signer, signed: Option<u64>, cx: &Context<'_>) -> Result<Outcome, CantCheck> {
    if signing.revoked {
        return Err(CantCheck::KeyRevoked);
    }
    if !signing.may_sign {
        return Err(CantCheck::NotASigningKey);
    }
    // A signature that says it was made before its key was is not one the key made.
    if signed.is_some_and(|made| made < u64::from(signing.created)) {
        return Err(CantCheck::Unreadable);
    }
    let when = signed.unwrap_or(cx.now);
    if signing.expires.is_some_and(|end| when >= end) {
        return Err(CantCheck::KeyExpired);
    }
    if let (Some(made), Some(after)) = (signed, sig.signature_expiration_time().map(|d| u64::from(d.as_secs())))
        && after > 0
        && cx.now >= made.saturating_add(after)
    {
        return Err(CantCheck::SignatureExpired);
    }
    let why = NotVerified {
        key_unverified: !(cx.verified)(&signer.fingerprint),
        address_differs: !signer.addresses.contains(&crate::mime::address(cx.from)),
    };
    Ok(if why.key_unverified || why.address_differs { Outcome::Valid { signer, why } } else { Outcome::Verified(signer) })
}

fn verify_with(sig: &Signature, key: &PublicKey, signing: &Signing, data: &[u8]) -> bool {
    let inner = key.inner();
    match signing.subkey {
        None => sig.verify(&inner.primary_key, data).is_ok(),
        Some(i) => inner.public_subkeys.get(i).is_some_and(|sub| sig.verify(&sub.key, data).is_ok()),
    }
}

/// Algorithms a Digital signature can be made with that rPGP checks.
fn known_signature_algorithm(algorithm: PublicKeyAlgorithm) -> bool {
    matches!(
        algorithm,
        PublicKeyAlgorithm::RSA
            | PublicKeyAlgorithm::RSASign
            | PublicKeyAlgorithm::DSA
            | PublicKeyAlgorithm::ECDSA
            | PublicKeyAlgorithm::EdDSALegacy
            | PublicKeyAlgorithm::Ed25519
            | PublicKeyAlgorithm::Ed448
    )
}
