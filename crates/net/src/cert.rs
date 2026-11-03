// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a person is shown about a server's certificate before they choose to trust it: its
//! SHA-256 fingerprint, who it was issued to and by, and its dates (#252, E5).
//!
//! The certificate is hostile input: it comes from a server that the system does not trust. The
//! reader here takes DER with every length checked against what is left, no recursion past the
//! four levels a certificate has, and no allocation beyond the few short names it keeps. It
//! reads only these fields and never decides anything about trust: that is the verifier's
//! (`crate::tls`), which also checks the dates and the name for itself, so a certificate this
//! cannot read is simply one nobody can be offered.

use mailcraft_time::{Date, DateTime};
use ring::digest;

/// What a person is shown about a certificate.
#[derive(Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// The SHA-256 fingerprint of the certificate: 64 lower-case hexadecimal digits.
    pub fingerprint: String,
    /// Who it was issued to (its subject): common name and organisation, as far as it has them.
    pub subject: String,
    /// Who issued it: the same.
    pub issuer: String,
    /// The first moment it is valid, in seconds since 1970-01-01 UTC.
    pub not_before: i64,
    /// The last moment it is valid, in seconds since 1970-01-01 UTC.
    pub not_after: i64,
}

/// Why a server's certificate is not accepted, and what a person may be shown of it.
///
/// Carried by [`NetError::Certificate`](crate::NetError::Certificate) when the system does not
/// trust the certificate of an HTTPS server and the owner's say could change that, or when the
/// certificate is one nobody may be asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertificateProblem {
    /// The system does not trust the certificate, and it is sound otherwise (in date, for this
    /// server's name): setup may show it and offer to trust exactly this one for the account.
    NotTrusted(CertificateInfo),
    /// The account trusts one certificate (by its fingerprint) and the server showed another,
    /// which is sound otherwise and not trusted by the system: the owner is asked again, naming
    /// both the old fingerprint's account and this new certificate.
    Changed(CertificateInfo),
    /// The certificate is out of date or for another name. It is never offered for trust, and no
    /// fingerprint makes it acceptable.
    Unacceptable(CertificateFault, CertificateInfo),
}

/// Why a certificate can never be trusted by its fingerprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificateFault {
    /// Its last day is past.
    Expired,
    /// Its first day has not come.
    NotYetValid,
    /// It is not for the name of the server that was asked.
    WrongName,
}

impl CertificateProblem {
    /// What can be shown of the certificate.
    pub fn info(&self) -> &CertificateInfo {
        match self {
            Self::NotTrusted(info) | Self::Changed(info) | Self::Unacceptable(_, info) => info,
        }
    }

    /// Whether the owner may be offered to trust this certificate: only one that is sound.
    pub fn can_be_trusted(&self) -> bool {
        !matches!(self, Self::Unacceptable(..))
    }
}

impl std::fmt::Display for CertificateProblem {
    /// A sentence with no word from the certificate in it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotTrusted(_) => "the server's certificate is not one this computer trusts",
            Self::Changed(_) => "the server's certificate is not the one trusted for this account; it has changed",
            Self::Unacceptable(CertificateFault::Expired, _) => "the server's certificate has expired",
            Self::Unacceptable(CertificateFault::NotYetValid, _) => "the server's certificate is not valid yet",
            Self::Unacceptable(CertificateFault::WrongName, _) => "the server's certificate is for another server",
        })
    }
}

/// The fingerprint, subject and issuer are an organisation's own: not for a log.
impl std::fmt::Debug for CertificateInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateInfo").finish_non_exhaustive()
    }
}

/// Longest name kept of a subject or an issuer, in characters.
const MAX_NAME: usize = 120;

/// The SHA-256 fingerprint of `der` as 64 lower-case hexadecimal digits.
pub fn fingerprint(der: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, der).as_ref())
}

/// The SHA-256 of `der`.
pub(crate) fn sha256(der: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, der).as_ref());
    out
}

/// `fingerprint` (64 hexadecimal digits, any case) as its 32 bytes.
pub(crate) fn parse_fingerprint(fingerprint: &str) -> Option<[u8; 32]> {
    let digits = fingerprint.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(digits.chunks(2)) {
        let high = char::from(*pair.first()?).to_digit(16)?;
        let low = char::from(*pair.get(1)?).to_digit(16)?;
        *slot = u8::try_from(high.checked_mul(16)?.checked_add(low)?).ok()?;
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What `der` (a DER X.509 certificate) shows; none when it cannot be read.
pub fn describe(der: &[u8]) -> Option<CertificateInfo> {
    let (not_before, not_after, issuer, subject) = fields(der)?;
    Some(CertificateInfo { fingerprint: fingerprint(der), subject, issuer, not_before, not_after })
}

/// A reader of DER: tag, length, contents; nothing past what the bytes hold.
#[derive(Clone, Copy)]
struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    /// The next element as `(tag, contents)`. Tags of more than one byte and lengths of more than
    /// four bytes are not read (no certificate has them).
    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let (&tag, rest) = self.0.split_first()?;
        if tag & 0x1f == 0x1f {
            return None;
        }
        let (&first, rest) = rest.split_first()?;
        let (length, rest) = if first < 0x80 {
            (usize::from(first), rest)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 || count > 4 {
                return None;
            }
            let (bytes, rest) = rest.split_at_checked(count)?;
            (bytes.iter().try_fold(0usize, |n, b| n.checked_mul(256)?.checked_add(usize::from(*b)))?, rest)
        };
        let (contents, rest) = rest.split_at_checked(length)?;
        self.0 = rest;
        Some((tag, contents))
    }

    /// The next element, which must have `tag`.
    fn expect(&mut self, tag: u8) -> Option<&'a [u8]> {
        let (have, contents) = self.next()?;
        (have == tag).then_some(contents)
    }
}

const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
const OID: u8 = 0x06;
const INTEGER: u8 = 0x02;
const VERSION: u8 = 0xa0;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
/// `commonName` and `organizationName` (2.5.4.3, 2.5.4.10).
const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
const ORGANIZATION: &[u8] = &[0x55, 0x04, 0x0a];

/// The hash that `tls-server-end-point` channel binding (RFC 5929 4.1) carries for the certificate
/// `der`: the hash of its signature algorithm (SHA-256, 384 or 512), with SHA-1 and MD5 replaced by
/// SHA-256, taken over the whole DER. None for a signature whose hash this doesn't know (RSASSA-PSS,
/// Ed25519, SHA-224) or for bytes that are no certificate: the caller then sends no binding.
pub(crate) fn end_point_hash(der: &[u8]) -> Option<Vec<u8>> {
    // sha1/sha256/sha384/sha512 WithRSAEncryption (1.2.840.113549.1.1.5, .11, .12, .13), md5WithRSA (.4),
    // and ecdsa-with-SHA1/256/384/512 (1.2.840.10045.4.1, 4.3.2, 4.3.3, 4.3.4).
    const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01];
    const ECDSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04];
    let certificate = Der(der).expect(SEQUENCE)?;
    let mut parts = Der(certificate);
    parts.expect(SEQUENCE)?;
    let algorithm = Der(parts.expect(SEQUENCE)?).expect(OID)?;
    let algorithm = if let Some(rest) = algorithm.strip_prefix(RSA) {
        match rest {
            [0x04 | 0x05 | 0x0b] => &digest::SHA256,
            [0x0c] => &digest::SHA384,
            [0x0d] => &digest::SHA512,
            _ => return None,
        }
    } else {
        match algorithm.strip_prefix(ECDSA)? {
            [0x01] | [0x03, 0x02] => &digest::SHA256,
            [0x03, 0x03] => &digest::SHA384,
            [0x03, 0x04] => &digest::SHA512,
            _ => return None,
        }
    };
    Some(digest::digest(algorithm, der).as_ref().to_vec())
}

/// `(not_before, not_after, issuer, subject)` of a certificate's DER.
fn fields(der: &[u8]) -> Option<(i64, i64, String, String)> {
    let mut certificate = Der(der);
    let mut body = Der(certificate.expect(SEQUENCE)?);
    let mut tbs = Der(body.expect(SEQUENCE)?);
    // `[0] version` (absent in v1), serial number, signature algorithm.
    let (tag, _) = tbs.next()?;
    if tag == VERSION {
        tbs.expect(INTEGER)?;
    } else if tag != INTEGER {
        return None;
    }
    tbs.expect(SEQUENCE)?;
    let issuer = name(tbs.expect(SEQUENCE)?);
    let mut validity = Der(tbs.expect(SEQUENCE)?);
    let not_before = time(validity.next()?)?;
    let not_after = time(validity.next()?)?;
    let subject = name(tbs.expect(SEQUENCE)?);
    Some((not_before, not_after, issuer, subject))
}

/// A `Name` as a line for a person: the common name, then the organisation in brackets.
fn name(contents: &[u8]) -> String {
    let (mut common, mut organization) = (None, None);
    let mut sets = Der(contents);
    // A name has a handful of parts; a hostile one is not read past a few dozen.
    for _ in 0..32 {
        let Some((SET, set)) = sets.next() else { break };
        let mut set = Der(set);
        let Some(pair) = set.expect(SEQUENCE) else { continue };
        let mut pair = Der(pair);
        let (Some(kind), Some((tag, value))) = (pair.expect(OID), pair.next()) else { continue };
        let slot = if kind == COMMON_NAME {
            &mut common
        } else if kind == ORGANIZATION {
            &mut organization
        } else {
            continue;
        };
        if slot.is_none() {
            *slot = text(tag, value);
        }
    }
    match (common, organization) {
        (Some(common), Some(organization)) => format!("{common} ({organization})"),
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => "(no name)".to_owned(),
    }
}

/// A name's string as text a person can read: control characters left out, the length capped.
fn text(tag: u8, value: &[u8]) -> Option<String> {
    let decoded: String = match tag {
        // UTF8String, PrintableString, IA5String
        0x0c | 0x13 | 0x16 => String::from_utf8(value.to_vec()).ok()?,
        // TeletexString (read as Latin-1, as it mostly is)
        0x14 => value.iter().map(|b| char::from(*b)).collect(),
        // BMPString (UTF-16, big-endian)
        0x1e => {
            let (pairs, _) = value.as_chunks::<2>();
            let units: Vec<u16> = pairs.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
            String::from_utf16(&units).ok()?
        }
        _ => return None,
    };
    let clean: String =
        decoded.chars().filter(|c| !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).take(MAX_NAME).collect();
    let clean = clean.trim().to_owned();
    (!clean.is_empty()).then_some(clean)
}

/// A `Time` (UTCTime `YYMMDDHHMMSSZ` or GeneralizedTime `YYYYMMDDHHMMSSZ`) as seconds since 1970.
fn time((tag, value): (u8, &[u8])) -> Option<i64> {
    let digits = value.strip_suffix(b"Z")?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |from: usize, len: usize| -> Option<i32> {
        let part = digits.get(from..from.checked_add(len)?)?;
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    let (year, at): (i32, usize) = match (tag, digits.len()) {
        (UTC_TIME, 12) => {
            let yy = number(0, 2)?;
            // RFC 5280: 50 to 99 are 19xx, 00 to 49 are 20xx.
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, 2)
        }
        (GENERALIZED_TIME, 14) => (number(0, 4)?, 4),
        _ => return None,
    };
    let part = |n: usize, len: usize| u8::try_from(number(at.checked_add(n)?, len)?).ok();
    let date = Date::new(year, part(0, 2)?, part(2, 2)?)?;
    Some(DateTime::new(date, part(4, 2)?, part(6, 2)?, part(8, 2)?)?.civil_seconds())
}

#[cfg(test)]
#[path = "cert_tests.rs"]
pub(crate) mod cert_tests;
