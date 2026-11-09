// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An X.509 certificate (RFC 5280) as a Digital signature is checked with: its bytes, what it
//! says, and the few questions asked of it (who it names, for what, until when, who signed it).

use const_oid::ObjectIdentifier;
use der::{Decode, Encode};
use mailcraft_model::digital_signature::{MAX_SIGNER_ADDRESSES, signer_text};
use mailcraft_time::Timestamp;
use x509_cert::Certificate;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAltName, SubjectKeyIdentifier};

use crate::algorithms::{self, Fault};
use crate::ber;

/// The largest certificate read.
pub const MAX_CERT_BYTES: usize = 64 * 1024;

const EMAIL_PROTECTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.4");
const ANY_EXTENDED_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37.0");
const EMAIL_ADDRESS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.1");

/// A certificate: its DER, the body its issuer signed, and what it says.
#[derive(Clone, Debug)]
pub struct Cert {
    der: Vec<u8>,
    /// Where the to-be-signed body is in `der`.
    tbs: std::ops::Range<usize>,
    parsed: Certificate,
    subject: Vec<u8>,
    issuer: Vec<u8>,
}

impl PartialEq for Cert {
    fn eq(&self, other: &Self) -> bool {
        self.der == other.der
    }
}

impl Cert {
    /// The certificate `der` is, when it is one.
    pub fn from_der(der: &[u8]) -> Option<Cert> {
        if der.len() > MAX_CERT_BYTES {
            return None;
        }
        let (outer, rest) = ber::read(der)?;
        if !rest.is_empty() {
            return None;
        }
        let (tbs, _) = ber::read(outer.value)?;
        let start = outer.whole.len().checked_sub(outer.value.len())?;
        let end = start.checked_add(tbs.whole.len())?;
        let parsed = Certificate::from_der(der).ok()?;
        let subject = parsed.tbs_certificate().subject().to_der().ok()?;
        let issuer = parsed.tbs_certificate().issuer().to_der().ok()?;
        Some(Cert { der: der.to_vec(), tbs: start..end, parsed, subject, issuer })
    }

    pub fn der(&self) -> &[u8] {
        &self.der
    }

    fn tbs(&self) -> &[u8] {
        self.der.get(self.tbs.clone()).unwrap_or_default()
    }

    pub fn subject(&self) -> &[u8] {
        &self.subject
    }

    pub fn issuer(&self) -> &[u8] {
        &self.issuer
    }

    /// The serial number's bytes.
    pub fn serial(&self) -> &[u8] {
        self.parsed.tbs_certificate().serial_number().as_bytes()
    }

    /// The subject key identifier, when it has one.
    pub fn key_id(&self) -> Option<Vec<u8>> {
        let (_, ski) = self.parsed.tbs_certificate().get_extension::<SubjectKeyIdentifier>().ok()??;
        Some(ski.0.as_bytes().to_vec())
    }

    pub fn public_key(&self) -> &spki::SubjectPublicKeyInfoOwned {
        self.parsed.tbs_certificate().subject_public_key_info()
    }

    /// Its SHA-256 fingerprint: upper-case hexadecimal pairs with colons between.
    pub fn fingerprint(&self) -> String {
        algorithms::Hash::Sha256.digest(&self.der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
    }

    /// From and until when it is valid, in seconds since 1970.
    pub fn validity(&self) -> (Timestamp, Timestamp) {
        let v = self.parsed.tbs_certificate().validity();
        let secs = |t: &x509_cert::time::Time| Timestamp::try_from(t.to_unix_duration().as_secs()).unwrap_or(Timestamp::MAX);
        (secs(&v.not_before), secs(&v.not_after))
    }

    /// Whether it is valid at `now`.
    pub fn valid_at(&self, now: Timestamp) -> bool {
        let (from, until) = self.validity();
        from <= now && now <= until
    }

    /// The name it is for: its common name, else its whole name.
    pub fn name(&self) -> String {
        name_of(self.parsed.tbs_certificate().subject())
    }

    /// The name of who issued it.
    pub fn issuer_name(&self) -> String {
        name_of(self.parsed.tbs_certificate().issuer())
    }

    /// The mail addresses it is for: its subject alternative names of that kind, and an
    /// `emailAddress` in its subject (RFC 8551 §3.1), at most [`MAX_SIGNER_ADDRESSES`].
    pub fn addresses(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(Some((_, san))) = self.parsed.tbs_certificate().get_extension::<SubjectAltName>() {
            for name in san.0.iter() {
                if let GeneralName::Rfc822Name(a) = name {
                    out.push(signer_text(a.as_str()));
                }
            }
        }
        for atv in self.parsed.tbs_certificate().subject().iter() {
            if atv.oid == EMAIL_ADDRESS
                && let Ok(a) = atv.value.decode_as::<der::asn1::Ia5String>()
            {
                out.push(signer_text(a.as_str()));
            }
        }
        out.retain(|a| !a.is_empty());
        out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        out.truncate(MAX_SIGNER_ADDRESSES);
        out
    }

    /// Whether it is for `address` (compared without regard to case).
    pub fn is_for(&self, address: &str) -> bool {
        let address = address.trim();
        !address.is_empty() && self.addresses().iter().any(|a| a.eq_ignore_ascii_case(address))
    }

    /// Whether it may be a certification authority's: basic constraints say so, and its key
    /// usage, when it has one, lets it sign certificates.
    pub fn is_ca(&self) -> bool {
        let tbs = self.parsed.tbs_certificate();
        let ca = matches!(tbs.get_extension::<BasicConstraints>(), Ok(Some((_, bc))) if bc.ca);
        let signs = match tbs.get_extension::<KeyUsage>() {
            Ok(Some((_, ku))) => ku.key_cert_sign(),
            Ok(None) => true,
            Err(_) => false,
        };
        ca && signs
    }

    /// Whether it is for signing mail (RFC 8551 §4.4): its key usage, when it has one, allows a
    /// Digital signature or non-repudiation, and its extended key usage, when it has one, names
    /// email protection (or any use).
    pub fn for_email(&self) -> bool {
        let tbs = self.parsed.tbs_certificate();
        let usage = match tbs.get_extension::<KeyUsage>() {
            Ok(Some((_, ku))) => ku.digital_signature() || ku.non_repudiation(),
            Ok(None) => true,
            Err(_) => false,
        };
        let extended = match tbs.get_extension::<ExtendedKeyUsage>() {
            Ok(Some((_, eku))) => eku.0.iter().any(|o| *o == EMAIL_PROTECTION || *o == ANY_EXTENDED_KEY_USAGE),
            Ok(None) => true,
            Err(_) => false,
        };
        usage && extended
    }

    /// Whether it names itself as its issuer.
    pub fn self_issued(&self) -> bool {
        self.subject == self.issuer
    }

    /// Whether `issuer`'s key made its signature.
    pub fn signed_by(&self, issuer: &Cert) -> Result<(), Fault> {
        let signature = self.parsed.signature().as_bytes().ok_or(Fault::Invalid)?;
        algorithms::verify(issuer.public_key(), self.parsed.signature_algorithm(), None, self.tbs(), signature)
    }
}

/// A name for people: its common name, else the whole of it as RFC 4514 writes it.
fn name_of(name: &x509_cert::name::Name) -> String {
    if let Ok(Some(cn)) = name.common_name() {
        let cn = signer_text(&cn.value());
        if !cn.is_empty() {
            return cn;
        }
    }
    signer_text(&name.to_string())
}
