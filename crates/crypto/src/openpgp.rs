//! OpenPGP (RFC 9580, 4880) for mail: PGP/MIME (RFC 3156) and inline.
//!
//! * [`verify_detached`]: the `application/pgp-signature` of a `multipart/signed`.
//! * [`decrypt`]: the `application/octet-stream` of a `multipart/encrypted`, or an inline
//!   `-----BEGIN PGP MESSAGE-----`; a signature inside is checked too.
//! * [`verify_cleartext`]: an inline `-----BEGIN PGP SIGNED MESSAGE-----`.
//! * [`sign_detached`], [`encrypt`]: the owner's side.
//!
//! OpenPGP has no certificate authorities: whether a key is the sender's is the owner's call
//! (they compare its fingerprint with the sender's own word), so a check here is never
//! [`Trust::Verified`]; the caller says so when the owner has vouched for the key.

use std::io::{BufReader, Read as _};

use pgp::composed::{
    ArmorOptions, CleartextSignedMessage, Deserializable, DetachedSignature, Message, MessageBuilder, SignedPublicKey, SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::Signature;
use pgp::types::{KeyDetails as _, Password};

use crate::keys::openpgp_secret;
use crate::{CryptoError, MAX_INPUT, PublicKey, Result, Scheme, SecretKey, SignatureCheck, Trust, Validity, hex};

/// Most keys a signature is tried against.
const MAX_TRIED: usize = 256;
/// Most layers (compression, signing) unwrapped from a decrypted message.
const MAX_LAYERS: usize = 4;

fn not_vouched() -> Trust {
    Trust::NotVerified("an OpenPGP key is vouched for by you alone: compare its fingerprint with the sender".into())
}

fn public_keys(known: &[PublicKey]) -> Vec<(PublicKey, SignedPublicKey)> {
    known
        .iter()
        .filter(|k| k.scheme == Scheme::OpenPgp)
        .take(MAX_TRIED)
        .filter_map(|k| SignedPublicKey::from_reader_single(k.data.as_slice()).ok().map(|(s, _)| (k.clone(), s)))
        .collect()
}

/// Whether the signature names this key (its primary key or a subkey) as the one that made it.
/// A signature that names nobody could be from anyone: every key is tried.
fn names(sig: &Signature, key: &SignedPublicKey) -> bool {
    let ids = sig.issuer_key_id();
    let fps = sig.issuer_fingerprint();
    if ids.is_empty() && fps.is_empty() {
        return true;
    }
    let mut ours = vec![(key.primary_key.legacy_key_id(), key.primary_key.fingerprint())];
    ours.extend(key.public_subkeys.iter().map(|s| (s.key.legacy_key_id(), s.key.fingerprint())));
    ours.iter().any(|(id, fp)| ids.contains(&id) || fps.contains(&fp))
}

/// Try a check against the primary key and every subkey of `key`: `$check` is an expression in
/// `$vk`, a key that verifies.
macro_rules! verifies {
    ($key:expr, |$vk:ident| $check:expr) => {{
        let key: &SignedPublicKey = $key;
        ({
            let $vk = &key.primary_key;
            $check
        }) || key.public_subkeys.iter().any(|s| {
            let $vk = &s.key;
            $check
        })
    }};
}

fn signed_at(sig: &Signature) -> Option<i64> {
    sig.created().map(|t| i64::from(t.as_secs()))
}

/// Check signatures against the known keys: the first that names a key we have decides.
fn judge(sigs: &[&Signature], known: &[(PublicKey, SignedPublicKey)], check: impl Fn(&Signature, &SignedPublicKey) -> bool) -> SignatureCheck {
    let mut result = SignatureCheck::failed(Scheme::OpenPgp, Validity::UnknownSigner);
    result.trust = not_vouched();
    for sig in sigs.iter().take(16) {
        result.signed_at = signed_at(sig);
        for (info, key) in known.iter().filter(|(_, k)| names(sig, k)) {
            if check(sig, key) {
                result.validity = Validity::Valid;
                result.signer = Some(info.clone());
                return result;
            }
            result.validity = Validity::Invalid("the message was changed after it was signed, or the signature is another key's".into());
            result.signer = Some(info.clone());
        }
    }
    result
}

fn read_signature(signature: &[u8]) -> Result<DetachedSignature> {
    if signature.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("the signature is too large".into()));
    }
    DetachedSignature::from_reader_single(signature).map(|(s, _)| s).map_err(|_| CryptoError::Malformed("the signature can't be read".into()))
}

/// Check a detached signature over `content` (the signed MIME entity, canonical CRLF).
pub fn verify_detached(content: &[u8], signature: &[u8], known: &[PublicKey]) -> SignatureCheck {
    let sig = match read_signature(signature) {
        Ok(s) => s,
        Err(e) => return SignatureCheck::failed(Scheme::OpenPgp, Validity::Invalid(e.to_string())),
    };
    let keys = public_keys(known);
    judge(&[&sig.signature], &keys, |s, key| verifies!(key, |vk| s.verify(vk, content).is_ok()))
}

/// What decrypting found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decrypted {
    pub content: Vec<u8>,
    /// A signature inside the encrypted message, checked.
    pub signature: Option<SignatureCheck>,
}

/// Decrypt an OpenPGP message (armored or binary) with one of the owner's secret keys; a
/// signature inside is checked against `known`. At most `MAX_INPUT` bytes come out.
pub fn decrypt(message: &[u8], secrets: &[SecretKey], known: &[PublicKey]) -> Result<Decrypted> {
    if message.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("the message is too large".into()));
    }
    let keys: Vec<SignedSecretKey> = secrets.iter().filter(|s| s.scheme == Scheme::OpenPgp).filter_map(|s| openpgp_secret(s).ok()).collect();
    if keys.is_empty() {
        return Err(CryptoError::NoKey("you have no OpenPGP key here to decrypt it".into()));
    }
    let armored = message.windows(10).take(4096).any(|w| w == b"-----BEGIN");
    let parsed = if armored { Message::from_armor(BufReader::new(message)).map(|(m, _)| m) } else { Message::from_bytes(BufReader::new(message)) }
        .map_err(|_| CryptoError::Malformed("the encrypted message can't be read".into()))?;
    if !parsed.is_encrypted() {
        return Err(CryptoError::Malformed("the message isn't encrypted".into()));
    }
    let passwords: Vec<Password> = keys.iter().map(|_| Password::empty()).collect();
    let mut msg = parsed.decrypt_with_keys(passwords.iter().collect(), keys.iter().collect()).map_err(|e| match e {
        pgp::errors::Error::MissingKey => CryptoError::NoKey("the message was encrypted for keys that aren't here".into()),
        _ => CryptoError::Failed("the message couldn't be decrypted with your key".into()),
    })?;
    for _ in 0..MAX_LAYERS {
        if msg.is_compressed() {
            msg = msg.decompress().map_err(|_| CryptoError::Unsupported("the message is packed in a way this program doesn't read".into()))?;
        } else {
            break;
        }
    }
    let mut content = Vec::new();
    (&mut msg).take(MAX_INPUT as u64 + 1).read_to_end(&mut content).map_err(|_| CryptoError::Failed("the message couldn't be decrypted".into()))?;
    if content.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("the message is too large".into()));
    }
    let signature = if msg.is_signed() {
        let known_keys = public_keys(known);
        let mut result = SignatureCheck::failed(Scheme::OpenPgp, Validity::UnknownSigner);
        result.trust = not_vouched();
        for (info, key) in &known_keys {
            if verifies!(key, |vk| msg.verify(vk).is_ok()) {
                result.validity = Validity::Valid;
                result.signer = Some(info.clone());
                break;
            }
        }
        Some(result)
    } else {
        None
    };
    Ok(Decrypted { content, signature })
}

/// Check an inline signed message (`-----BEGIN PGP SIGNED MESSAGE-----`). Gives the check and
/// the text that was signed.
pub fn verify_cleartext(text: &str, known: &[PublicKey]) -> Result<(SignatureCheck, String)> {
    if text.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("the message is too large".into()));
    }
    let (msg, _) = CleartextSignedMessage::from_string(text).map_err(|_| CryptoError::Malformed("the signed text can't be read".into()))?;
    let keys = public_keys(known);
    let sigs: Vec<&Signature> = msg.signatures().iter().collect();
    let check = judge(&sigs, &keys, |_, key| verifies!(key, |vk| msg.verify(vk).is_ok()));
    Ok((check, msg.signed_text()))
}

/// A detached, armored signature over `content` (the MIME entity, canonical CRLF), SHA-256: made
/// with the key's signing subkey, else its primary key.
pub fn sign_detached(content: &[u8], secret: &SecretKey) -> Result<String> {
    let key = openpgp_secret(secret)?;
    let fail = |e: pgp::errors::Error| CryptoError::Failed(format!("OpenPGP: {e}"));
    let sub = key.secret_subkeys.iter().find(|sub| sub.key.algorithm().can_sign() && sub.signatures.iter().any(|s| s.key_flags().sign()));
    let sig = match sub {
        Some(sub) => DetachedSignature::sign_binary_data(crate::rng(), &sub.key, &Password::empty(), HashAlgorithm::Sha256, content).map_err(fail)?,
        None if key.primary_key.algorithm().can_sign() => {
            DetachedSignature::sign_binary_data(crate::rng(), &key.primary_key, &Password::empty(), HashAlgorithm::Sha256, content).map_err(fail)?
        }
        None => return Err(CryptoError::Unsupported("your OpenPGP key can't sign".into())),
    };
    sig.to_armored_string(ArmorOptions::default()).map_err(|e| CryptoError::Failed(format!("OpenPGP: {e}")))
}

/// Encrypt `content` to the `recipients`' keys, armored (SEIPD v1, AES-256: what every OpenPGP
/// program reads).
pub fn encrypt(content: &[u8], recipients: &[PublicKey]) -> Result<String> {
    if recipients.is_empty() {
        return Err(CryptoError::NoKey("there is nobody to encrypt to".into()));
    }
    let mut builder = MessageBuilder::from_bytes("", content.to_vec()).seipd_v1(crate::rng(), SymmetricKeyAlgorithm::AES256);
    for recipient in recipients {
        let (key, _) = SignedPublicKey::from_reader_single(recipient.data.as_slice())
            .map_err(|_| CryptoError::Malformed(format!("the key of {} can't be read", recipient.name)))?;
        let mut added = false;
        for sub in &key.public_subkeys {
            let flags = sub.signatures.iter().any(|s| {
                let f = s.key_flags();
                f.encrypt_comms() || f.encrypt_storage()
            });
            if flags && sub.key.algorithm().can_encrypt() {
                builder.encrypt_to_key(crate::rng(), &sub.key).map_err(|e| CryptoError::Failed(format!("OpenPGP: {e}")))?;
                added = true;
                break;
            }
        }
        if !added && key.primary_key.algorithm().can_encrypt() {
            builder.encrypt_to_key(crate::rng(), &key.primary_key).map_err(|e| CryptoError::Failed(format!("OpenPGP: {e}")))?;
            added = true;
        }
        if !added {
            return Err(CryptoError::NoKey(format!("the key of {} can't encrypt", recipient.name)));
        }
    }
    builder.to_armored_string(crate::rng(), ArmorOptions::default()).map_err(|e| CryptoError::Failed(format!("OpenPGP: {e}")))
}

/// The fingerprint of an OpenPGP secret key, as hex.
pub fn fingerprint(secret: &SecretKey) -> Option<String> {
    openpgp_secret(secret).ok().map(|k| hex(k.fingerprint().as_bytes()))
}
