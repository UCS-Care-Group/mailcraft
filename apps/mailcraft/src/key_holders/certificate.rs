// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a certificate in the system's store says about itself, read from its DER (RFC 5280)
//! with RustCrypto's `x509-cert`, the same on every system. What the store hands over is read
//! as hostile: sizes capped, names shortened and cleared of control characters.

use x509_cert::Certificate;
use x509_cert::der::asn1::{Ia5StringRef, ObjectIdentifier, UintRef};
use x509_cert::der::{Decode, Reader, SliceReader};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{ExtendedKeyUsage, KeyUsage, SubjectAltName};

use super::{KeyHolderError, MAX_CERTIFICATE_LEN};

/// The longest subject or issuer kept, in characters.
const MAX_NAME_CHARS: usize = 512;
/// The most addresses kept from one certificate.
const MAX_EMAILS: usize = 16;
/// The longest address kept, in bytes (RFC 5321's path limit).
const MAX_EMAIL_LEN: usize = 254;

const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const SECP256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const SECP384R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const EMAIL_ADDRESS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.1");
const EMAIL_PROTECTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.4");
const ANY_EXTENDED_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37.0");
const ID_CE_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.15");
const ID_CE_SUBJECT_ALT_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.17");
const ID_CE_EXT_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37");

/// The kind of key a certificate is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// RSA, with its modulus's length in bits.
    Rsa { bits: u32 },
    /// ECDSA on NIST P-256.
    EcP256,
    /// ECDSA on NIST P-384.
    EcP384,
    /// Anything else (another curve, Ed25519, DSA): listed, not used.
    Other,
}

/// One of the owner's certificates with a private key, as a holder lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldCertificate {
    /// The certificate itself, as the store holds it.
    pub der: Vec<u8>,
    /// SHA-256 of `der`: how a certificate is named to a holder.
    pub fingerprint: [u8; 32],
    /// The subject, RFC 4514 text.
    pub subject: String,
    /// The issuer, RFC 4514 text.
    pub issuer: String,
    /// The addresses it is for: the subject alternative name's, else the subject's
    /// `emailAddress`; lower-cased.
    pub emails: Vec<String>,
    /// Valid from, seconds since 1970.
    pub not_before: u64,
    /// Valid until, seconds since 1970.
    pub not_after: u64,
    /// The key's kind.
    pub key: KeyKind,
    /// Whether its extended key usage allows mail (email protection, any, or none named).
    pub for_mail: bool,
    /// Whether its key usage allows signing (digital signature or non-repudiation, or none named).
    pub can_sign: bool,
    /// Whether its key may unwrap content-encryption keys (RSA with key encipherment, or none named).
    pub can_decrypt: bool,
}

impl HeldCertificate {
    /// What `der` says, or an error when it isn't a certificate.
    pub fn from_der(der: Vec<u8>) -> Result<Self, KeyHolderError> {
        if der.len() > MAX_CERTIFICATE_LEN {
            return Err(KeyHolderError::BadInput("the certificate is too large"));
        }
        let cert = Certificate::from_der(&der).map_err(|_| KeyHolderError::BadInput("not a certificate"))?;
        let tbs = cert.tbs_certificate();
        let spki = tbs.subject_public_key_info();
        let key = key_kind(&spki.algorithm.oid, spki.algorithm.parameters.as_ref(), spki.subject_public_key.raw_bytes());

        let mut emails = Vec::new();
        let mut key_usage: Option<KeyUsage> = None;
        let mut for_mail = true;
        for ext in tbs.extensions().into_iter().flatten() {
            let value = ext.extn_value.as_bytes();
            if ext.extn_id == ID_CE_SUBJECT_ALT_NAME {
                if let Ok(names) = SubjectAltName::from_der(value) {
                    for name in names.0 {
                        if let GeneralName::Rfc822Name(address) = name {
                            push_email(&mut emails, address.as_str());
                        }
                    }
                }
            } else if ext.extn_id == ID_CE_KEY_USAGE {
                key_usage = KeyUsage::from_der(value).ok();
                // A key usage that can't be read allows nothing.
                if key_usage.is_none() {
                    key_usage = Some(KeyUsage(Default::default()));
                }
            } else if ext.extn_id == ID_CE_EXT_KEY_USAGE {
                for_mail = match ExtendedKeyUsage::from_der(value) {
                    Ok(usages) => usages.0.iter().any(|u| *u == EMAIL_PROTECTION || *u == ANY_EXTENDED_KEY_USAGE),
                    Err(_) => false,
                };
            }
        }
        if emails.is_empty() {
            for atv in tbs.subject().iter() {
                if atv.oid == EMAIL_ADDRESS
                    && let Ok(address) = atv.value.decode_as::<Ia5StringRef<'_>>()
                {
                    push_email(&mut emails, address.as_str());
                }
            }
        }

        let (can_sign, can_decrypt) = match &key_usage {
            None => (true, matches!(key, KeyKind::Rsa { .. })),
            Some(ku) => (ku.digital_signature() || ku.non_repudiation(), matches!(key, KeyKind::Rsa { .. }) && ku.key_encipherment()),
        };
        let fingerprint = sha256(&der);
        Ok(Self {
            subject: clean_name(&tbs.subject().to_string()),
            issuer: clean_name(&tbs.issuer().to_string()),
            emails,
            not_before: tbs.validity().not_before.to_unix_duration().as_secs(),
            not_after: tbs.validity().not_after.to_unix_duration().as_secs(),
            key,
            for_mail,
            can_sign,
            can_decrypt,
            fingerprint,
            der,
        })
    }

    /// The fingerprint as lower-case hex, as the window shows it.
    pub fn fingerprint_hex(&self) -> String {
        hex(&self.fingerprint)
    }
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = [0u8; 32];
    for (o, b) in out.iter_mut().zip(digest.as_ref()) {
        *o = *b;
    }
    out
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        out.push(char::from(DIGITS.get(usize::from(b >> 4)).copied().unwrap_or(b'0')));
        out.push(char::from(DIGITS.get(usize::from(b & 0x0f)).copied().unwrap_or(b'0')));
    }
    out
}

fn key_kind(algorithm: &ObjectIdentifier, parameters: Option<&x509_cert::der::Any>, key: &[u8]) -> KeyKind {
    if *algorithm == RSA_ENCRYPTION {
        return rsa_bits(key).map_or(KeyKind::Other, |bits| KeyKind::Rsa { bits });
    }
    if *algorithm == EC_PUBLIC_KEY {
        let curve = parameters.and_then(|p| p.decode_as::<ObjectIdentifier>().ok());
        return match curve {
            Some(c) if c == SECP256R1 => KeyKind::EcP256,
            Some(c) if c == SECP384R1 => KeyKind::EcP384,
            _ => KeyKind::Other,
        };
    }
    KeyKind::Other
}

/// The modulus length of an `RSAPublicKey` (RFC 8017 A.1.1), in bits.
fn rsa_bits(key: &[u8]) -> Option<u32> {
    let mut reader = SliceReader::new(key).ok()?;
    let modulus = reader
        .sequence(|seq| {
            let n = UintRef::decode(seq)?;
            let _e = UintRef::decode(seq)?;
            Ok::<_, x509_cert::der::Error>(n.as_bytes().to_vec())
        })
        .ok()?;
    let first = *modulus.first()?;
    let len = u32::try_from(modulus.len()).ok()?;
    Some(len.saturating_sub(1).saturating_mul(8).saturating_add(8 - first.leading_zeros()))
}

fn push_email(emails: &mut Vec<String>, address: &str) {
    if emails.len() >= MAX_EMAILS
        || address.len() > MAX_EMAIL_LEN
        || !address.contains('@')
        || address.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return;
    }
    let address = address.to_ascii_lowercase();
    if !emails.contains(&address) {
        emails.push(address);
    }
}

fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect()
}
