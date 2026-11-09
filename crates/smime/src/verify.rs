// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Checking CMS SignedData (RFC 5652 §5) as S/MIME carries it (RFC 8551 §3.5): the signer's
//! digest of what was signed, their signature over it, and whether their certificate is trusted
//! and for the message's sender.

use cms::signed_data::{EncapsulatedContentInfo, SignerIdentifier, SignerInfo};
use const_oid::ObjectIdentifier;
use der::asn1::OctetString;
use der::{Decode, Encode};
use mailcraft_model::digital_signature::{DigitalSignature, SignatureFormat, Signer, Verdict, Why};
use mailcraft_time::Timestamp;

use crate::algorithms::{self, Fault, Hash};
use crate::ber::{self, Tlv};
use crate::cert::Cert;
use crate::chain::{self, Anchors};

/// The most certificates read from one message.
pub const MAX_CERTS: usize = 32;
/// The most signers of one message that are checked.
pub const MAX_SIGNERS: usize = 8;

const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");

/// What a check is made against.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The certificates the host trusts.
    pub anchors: &'a Anchors,
    /// The message's From address.
    pub from: &'a str,
    /// The time of the check, seconds since 1970.
    pub now: Timestamp,
    /// Certificates kept for the sender already, in DER: a message that doesn't carry its
    /// signer's certificate may be checked with one of them.
    pub known: &'a [&'a [u8]],
}

/// A Digitally signed message's two halves.
#[derive(Clone, Copy, Debug)]
pub enum Signed<'a> {
    /// `multipart/signed`: the signed part as it came (its MIME header and body) and the
    /// signature part's bytes (the CMS SignedData, decoded from base64).
    Detached { content: &'a [u8], signature: &'a [u8] },
    /// `application/pkcs7-mime; smime-type=signed-data`: the CMS SignedData holding what was
    /// signed (decoded from base64).
    Opaque(&'a [u8]),
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub signature: DigitalSignature,
    /// The signer's certificate, in DER, when the signature over the message is the signer's
    /// and the certificate is for the From address: worth keeping for that address (#30 R9).
    pub certificate: Option<Vec<u8>>,
}

/// Check `signed` against `context`. `None` when the CMS isn't SignedData at all (an Encrypted
/// message's enveloped data, say): not a Digital signature. Never fails otherwise: what can't be
/// read is [`Verdict::Unchecked`].
pub fn check(signed: Signed<'_>, context: &Context<'_>) -> Option<Checked> {
    let blob = match signed {
        Signed::Detached { signature, .. } => signature,
        Signed::Opaque(blob) => blob,
    };
    let unreadable = || Some(unchecked(Why::Unreadable));
    let Some(der) = ber::to_der(blob) else { return unreadable() };
    let Some(parts) = content_info(&der) else { return unreadable() };
    if parts.content_type != SIGNED_DATA {
        return None;
    }
    let Some(data) = signed_data(parts.content) else { return unreadable() };
    let Ok(eci) = EncapsulatedContentInfo::from_der(data.encap.whole) else { return unreadable() };
    let content: &[u8] = match signed {
        Signed::Detached { content, .. } => content,
        Signed::Opaque(_) => match econtent(&data.encap) {
            Some(c) => c,
            None => return unreadable(),
        },
    };
    let mut pool: Vec<Cert> = data.certificates.iter().take(MAX_CERTS).filter_map(|c| Cert::from_der(c.whole)).collect();
    pool.extend(context.known.iter().take(MAX_CERTS).filter_map(|c| Cert::from_der(c)));
    let mut best: Option<Checked> = None;
    for signer in data.signers.iter().take(MAX_SIGNERS) {
        let found = one_signer(signer, &eci, content, &pool, context);
        best = Some(match best {
            Some(b) if b.signature.verdict <= found.signature.verdict => b,
            _ => found,
        });
    }
    best.or_else(unreadable)
}

fn unchecked(why: Why) -> Checked {
    Checked { signature: DigitalSignature::bare(SignatureFormat::Smime, Verdict::Unchecked, why), certificate: None }
}

struct ContentInfoParts<'a> {
    content_type: ObjectIdentifier,
    content: &'a [u8],
}

/// ContentInfo: its type, and the value inside its `[0] EXPLICIT`.
fn content_info(der: &[u8]) -> Option<ContentInfoParts<'_>> {
    let (outer, _) = ber::read(der)?;
    if outer.tag != 0x30 {
        return None;
    }
    let fields = ber::children(outer.value)?;
    let oid = fields.first().filter(|f| f.tag == 0x06)?;
    let content_type = ObjectIdentifier::from_der(oid.whole).ok()?;
    let explicit = fields.get(1).filter(|f| f.tag == 0xa0)?;
    Some(ContentInfoParts { content_type, content: explicit.value })
}

/// SignedData's parts as they came, so that signatures are checked over the bytes the signer
/// wrote: what is inside, the certificates, and the signers.
struct SignedDataParts<'a> {
    encap: Tlv<'a>,
    certificates: Vec<Tlv<'a>>,
    signers: Vec<Tlv<'a>>,
}

fn signed_data(content: &[u8]) -> Option<SignedDataParts<'_>> {
    let (outer, _) = ber::read(content)?;
    if outer.tag != 0x30 {
        return None;
    }
    let fields = ber::children(outer.value)?;
    // version, digestAlgorithms, encapContentInfo, [0] certificates, [1] crls, signerInfos.
    let encap = *fields.get(2).filter(|f| f.tag == 0x30)?;
    let mut certificates = Vec::new();
    let mut signers = None;
    for field in fields.iter().skip(3) {
        match field.tag {
            0xa0 => certificates = ber::children(field.value)?.into_iter().filter(|c| c.tag == 0x30).collect(),
            0x31 => signers = Some(ber::children(field.value)?),
            _ => {}
        }
    }
    Some(SignedDataParts { encap, certificates, signers: signers? })
}

/// The OCTET STRING inside encapContentInfo's `[0] EXPLICIT eContent`, when there is one.
fn econtent<'a>(encap: &Tlv<'a>) -> Option<&'a [u8]> {
    let fields = ber::children(encap.value)?;
    let explicit = fields.get(1).filter(|f| f.tag == 0xa0)?;
    let (octets, _) = ber::read(explicit.value)?;
    (octets.tag == 0x04).then_some(octets.value)
}

/// The value of the signed attribute `oid`, in DER (its one value), when there is one.
fn attribute(info: &SignerInfo, oid: ObjectIdentifier) -> Option<Vec<u8>> {
    let attrs = info.signed_attrs.as_ref()?;
    let attr = attrs.iter().find(|a| a.oid == oid)?;
    if attr.values.len() != 1 {
        return None;
    }
    attr.values.iter().next()?.to_der().ok()
}

/// The bytes a signer with signed attributes signed (RFC 5652 §5.4): the attributes as they came,
/// with their `[0] IMPLICIT` tag made the SET OF's.
fn signed_attributes(raw: &Tlv<'_>) -> Option<Vec<u8>> {
    let fields = ber::children(raw.value)?;
    let attrs = fields.iter().find(|f| f.tag == 0xa0)?;
    let mut out = attrs.whole.to_vec();
    *out.first_mut()? = 0x31;
    Some(out)
}

/// `content` with every lone LF made CRLF (RFC 8551 §3.1.1's canonical form), when it has one.
fn canonical(content: &[u8]) -> Option<Vec<u8>> {
    let mut lone = false;
    let mut out = Vec::with_capacity(content.len().saturating_add(content.len() / 32));
    let mut previous = 0u8;
    for &b in content {
        if b == b'\n' && previous != b'\r' {
            out.push(b'\r');
            lone = true;
        }
        out.push(b);
        previous = b;
    }
    lone.then_some(out)
}

fn one_signer(raw: &Tlv<'_>, eci: &EncapsulatedContentInfo, content: &[u8], pool: &[Cert], context: &Context<'_>) -> Checked {
    let Ok(info) = SignerInfo::from_der(raw.whole) else { return unchecked(Why::Unreadable) };
    let hash = match Hash::of(&info.digest_alg) {
        Ok(hash) => hash,
        Err(fault) => return unchecked(why_not(fault)),
    };
    let Some(cert) = signer_cert(&info.sid, pool) else { return unchecked(Why::NoPublicKey) };
    let result = |verdict: Verdict, why: Option<Why>| Checked {
        signature: DigitalSignature {
            format: SignatureFormat::Smime,
            verdict,
            why,
            signer: Some(signer_of(cert)),
            signed_at: signing_time(&info),
            revocation_checked: false,
        },
        certificate: None,
    };
    let signed: Vec<u8> = if info.signed_attrs.is_some() {
        let Some(said_type) = attribute(&info, CONTENT_TYPE).and_then(|d| ObjectIdentifier::from_der(&d).ok()) else {
            return unchecked(Why::Unreadable);
        };
        let Some(said_digest) = attribute(&info, MESSAGE_DIGEST).and_then(|d| OctetString::from_der(&d).ok()) else {
            return unchecked(Why::Unreadable);
        };
        if said_type != eci.econtent_type {
            return result(Verdict::Changed, Some(Why::Altered));
        }
        let matches = hash.digest(content) == said_digest.as_bytes() || canonical(content).is_some_and(|c| hash.digest(&c) == said_digest.as_bytes());
        if !matches {
            return result(Verdict::Changed, Some(Why::Altered));
        }
        match signed_attributes(raw) {
            Some(bytes) => bytes,
            None => return unchecked(Why::Unreadable),
        }
    } else {
        if eci.econtent_type != DATA {
            return unchecked(Why::Unreadable);
        }
        content.to_vec()
    };
    let signature = info.signature.as_bytes();
    let mut verified = algorithms::verify(cert.public_key(), &info.signature_algorithm, Some(hash), &signed, signature);
    // Without signed attributes the content itself is signed: in its canonical form, perhaps.
    if verified == Err(Fault::Invalid)
        && info.signed_attrs.is_none()
        && let Some(c) = canonical(content)
    {
        verified = algorithms::verify(cert.public_key(), &info.signature_algorithm, Some(hash), &c, signature);
    }
    match verified {
        Ok(()) => {}
        Err(Fault::Invalid) => return result(Verdict::Changed, Some(Why::Altered)),
        Err(fault) => return result(Verdict::Unchecked, Some(why_not(fault))),
    }
    let worth_keeping = cert.is_for(context.from) && cert.for_email();
    let trust = chain::trust(cert, pool, context.anchors, context.now)
        .and_then(|()| if cert.for_email() { Ok(()) } else { Err(Why::NotForEmail) })
        .and_then(|()| if cert.is_for(context.from) { Ok(()) } else { Err(Why::AddressMismatch) });
    let mut r = match trust {
        Ok(()) => result(Verdict::Verified, None),
        // (An algorithm that's no longer safe on the way to a trust anchor: the certificate can't
        // be relied on, but the message is as it was signed.)
        Err(why) => result(Verdict::NotVerified, Some(why)),
    };
    if worth_keeping {
        r.certificate = Some(cert.der().to_vec());
    }
    r
}

fn why_not(fault: Fault) -> Why {
    match fault {
        Fault::Weak => Why::WeakAlgorithm,
        Fault::Unknown => Why::UnknownAlgorithm,
        Fault::Invalid => Why::Altered,
    }
}

/// The certificate in `pool` that `sid` names.
fn signer_cert<'a>(sid: &SignerIdentifier, pool: &'a [Cert]) -> Option<&'a Cert> {
    match sid {
        SignerIdentifier::IssuerAndSerialNumber(isn) => {
            let issuer = isn.issuer.to_der().ok()?;
            let serial = isn.serial_number.as_bytes();
            pool.iter().find(|c| c.issuer() == issuer.as_slice() && c.serial() == serial)
        }
        SignerIdentifier::SubjectKeyIdentifier(ski) => {
            let id = ski.0.as_bytes();
            pool.iter().find(|c| c.key_id().as_deref() == Some(id))
        }
    }
}

fn signer_of(cert: &Cert) -> Signer {
    let (not_before, not_after) = cert.validity();
    Signer { name: cert.name(), addresses: cert.addresses(), fingerprint: cert.fingerprint(), issuer: cert.issuer_name(), not_before, not_after }
}

/// The signing time the signer gives (RFC 5652 §11.3): their word, unchecked.
fn signing_time(info: &SignerInfo) -> Option<Timestamp> {
    let der = attribute(info, SIGNING_TIME)?;
    let time = x509_cert::time::Time::from_der(&der).ok()?;
    Timestamp::try_from(time.to_unix_duration().as_secs()).ok()
}
