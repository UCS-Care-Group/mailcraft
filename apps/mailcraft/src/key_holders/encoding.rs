// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The few byte layouts the systems differ on, written from the specifications: the
//! `DigestInfo` a raw RSA PKCS#1 v1.5 signature is made over (RFC 8017 §9.2), and ECDSA
//! signatures as `r || s` (what PKCS#11 and Windows' CNG give back, PKCS#11 v2.40 §2.3.1) turned
//! into DER `Ecdsa-Sig-Value` (RFC 5753 §7.2, RFC 3279 §2.2.3), which CMS carries.

use super::{DigestAlgorithm, KeyHolderError, KeyKind};

/// `DigestInfo` up to the digest's bytes, for each hash (RFC 8017 §9.2, note 1).
fn digest_info_prefix(algorithm: DigestAlgorithm) -> &'static [u8] {
    match algorithm {
        DigestAlgorithm::Sha256 => {
            &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20]
        }
        DigestAlgorithm::Sha384 => {
            &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30]
        }
        DigestAlgorithm::Sha512 => {
            &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40]
        }
    }
}

/// The DER `DigestInfo` for `digest`, which a raw RSA PKCS#1 v1.5 signature (PKCS#11's
/// `CKM_RSA_PKCS`) is made over.
pub fn digest_info(algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let prefix = digest_info_prefix(algorithm);
    let mut out = Vec::with_capacity(prefix.len().saturating_add(digest.len()));
    out.extend_from_slice(prefix);
    out.extend_from_slice(digest);
    Ok(out)
}

/// The DER `Ecdsa-Sig-Value` for an ECDSA signature given as `r || s` with a key of kind `key`:
/// exactly 64 bytes for P-256, 96 for P-384.
pub fn ecdsa_raw_to_der(raw: &[u8], key: KeyKind) -> Result<Vec<u8>, KeyHolderError> {
    let expected = match key {
        KeyKind::EcP256 => 64,
        KeyKind::EcP384 => 96,
        KeyKind::Rsa { .. } | KeyKind::Other => return Err(KeyHolderError::Unsupported("an ECDSA signature for a key that isn't P-256 or P-384")),
    };
    if raw.len() != expected {
        return Err(KeyHolderError::System("the ECDSA signature's length doesn't fit its curve".into()));
    }
    let (r, s) = raw.split_at(raw.len() / 2);
    let mut body = Vec::with_capacity(raw.len().saturating_add(8));
    der_integer(&mut body, r);
    der_integer(&mut body, s);
    let mut out = Vec::with_capacity(body.len().saturating_add(4));
    out.push(0x30);
    der_length(&mut out, body.len());
    out.extend_from_slice(&body);
    Ok(out)
}

/// An unsigned big-endian number as a DER INTEGER.
fn der_integer(out: &mut Vec<u8>, number: &[u8]) {
    let first_non_zero = number.iter().position(|b| *b != 0).unwrap_or(number.len());
    let digits = number.get(first_non_zero..).unwrap_or_default();
    let pad = digits.first().is_none_or(|b| b & 0x80 != 0);
    out.push(0x02);
    der_length(out, digits.len().saturating_add(usize::from(pad)));
    if pad {
        out.push(0x00);
    }
    out.extend_from_slice(digits);
}

/// A DER length.
fn der_length(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(u8::try_from(len).unwrap_or(0x7f));
        return;
    }
    let bytes = len.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len().saturating_sub(1));
    let significant = bytes.get(first..).unwrap_or_default();
    out.push(0x80 | u8::try_from(significant.len()).unwrap_or(0));
    out.extend_from_slice(significant);
}
