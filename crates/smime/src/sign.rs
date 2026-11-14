// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Making an S/MIME Digital signature (#30): the CMS SignedData of a detached signature
//! (RFC 5652 §5, RFC 8551 §3.5.3) over a MIME entity, with the signer's certificate and the
//! authorities' between it and a trust anchor carried in it.
//!
//! No key is ever seen here. What is to be signed is assembled (the signed attributes of
//! RFC 5652 §5.3 and RFC 8551 §2.5) and its digest handed to the key's holder (the operating
//! system's certificate store, a smart card, a file the owner opened), which gives back the
//! signature. That signature is then checked against the certificate's public key with the
//! same code a received message is checked with, so that a holder which signed with another
//! key, or gave back anything else, never makes a message that reads as changed after signing.
//!
//! The algorithms are SHA-256, SHA-384 and SHA-512 (RFC 5754) with RSA PKCS #1 v1.5, RSASSA-PSS
//! (RFC 4055, RFC 4056) on keys of 2048 bits or more, and ECDSA on P-256 and P-384 (RFC 5753,
//! RFC 5758). MD5 and SHA-1 can't be asked for.

use std::fmt;

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos};
use const_oid::ObjectIdentifier;
use der::asn1::{GeneralizedTime, OctetString, SetOfVec, UtcTime};
use der::{Any, Encode};
use spki::AlgorithmIdentifierOwned;
use x509_cert::attr::Attribute;
use x509_cert::time::Time;

use crate::algorithms::{self, Fault, Hash, SigningKind};
use crate::cert::{Cert, MAX_CERT_BYTES};

/// The largest content signed: a message bigger than any server takes.
pub const MAX_SIGNED_BYTES: usize = 150 * 1024 * 1024;
/// The most certificates of a chain carried besides the signer's.
pub const MAX_CHAIN: usize = 8;
/// The longest signature a holder may give back: an RSA key of 8192 bits makes 1024 bytes.
const MAX_SIGNATURE_BYTES: usize = 1024;

const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
const SMIME_CAPABILITIES: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.15");
const AES256_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.42");
const AES128_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.2");

/// The digest a signature is made over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlgorithm {
    /// Its name in the `micalg` parameter of `multipart/signed` (RFC 8551 §3.5.3.2).
    pub fn micalg(self) -> &'static str {
        match self {
            DigestAlgorithm::Sha256 => "sha-256",
            DigestAlgorithm::Sha384 => "sha-384",
            DigestAlgorithm::Sha512 => "sha-512",
        }
    }

    /// The digest of `data`.
    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        self.hash().digest(data)
    }

    fn hash(self) -> Hash {
        match self {
            DigestAlgorithm::Sha256 => Hash::Sha256,
            DigestAlgorithm::Sha384 => Hash::Sha384,
            DigestAlgorithm::Sha512 => Hash::Sha512,
        }
    }

    fn oid(self) -> ObjectIdentifier {
        match self {
            DigestAlgorithm::Sha256 => algorithms::SHA256,
            DigestAlgorithm::Sha384 => algorithms::SHA384,
            DigestAlgorithm::Sha512 => algorithms::SHA512,
        }
    }

    /// Its identifier, with the parameters absent (RFC 5754 §2).
    fn identifier(self) -> AlgorithmIdentifierOwned {
        AlgorithmIdentifierOwned { oid: self.oid(), parameters: None }
    }

    /// Its length in bytes.
    fn len(self) -> u8 {
        match self {
            DigestAlgorithm::Sha256 => 32,
            DigestAlgorithm::Sha384 => 48,
            DigestAlgorithm::Sha512 => 64,
        }
    }
}

/// How the key's holder makes the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureScheme {
    /// RSA with PKCS #1 v1.5 padding: the digest in a DigestInfo (RFC 8017 §9.2).
    RsaPkcs1v15,
    /// RSASSA-PSS, MGF1 with the same digest and a salt as long as the digest (RFC 4056 §3).
    RsaPss,
    /// ECDSA, the signature as the DER of an `Ecdsa-Sig-Value` (RFC 5753 §7.2).
    Ecdsa,
}

/// What is to be signed, and with what.
#[derive(Clone, Copy, Debug)]
pub struct SignRequest<'a> {
    /// The MIME entity exactly as it is signed: its header fields, an empty line and its body,
    /// in CRLF lines (RFC 8551 §3.1.1).
    pub content: &'a [u8],
    /// The signer's certificate, in DER.
    pub certificate: &'a [u8],
    /// The certificates of the authorities between the signer's and a trust anchor, in DER, to
    /// be carried in the message (at most [`MAX_CHAIN`]).
    pub chain: &'a [Vec<u8>],
    pub digest: DigestAlgorithm,
    pub scheme: SignatureScheme,
    /// When it is signed, seconds since 1970.
    pub signing_time: i64,
}

/// Why no signature was made. Nothing in it is of the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignError<E> {
    /// A certificate can't be used: what is wrong with it.
    Certificate(&'static str),
    /// What was asked for can't be made here: what.
    Unsupported(&'static str),
    /// The content, a certificate or the chain is larger than is taken.
    TooLarge,
    /// The key's holder couldn't sign.
    Holder(E),
    /// The key's holder gave back something that isn't a signature by the certificate's key over
    /// what was to be signed.
    BadSignature,
    /// The signature couldn't be written out.
    Encoding,
}

impl<E: fmt::Display> fmt::Display for SignError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignError::Certificate(why) => write!(f, "the certificate can't be used for signing: {why}"),
            SignError::Unsupported(why) => write!(f, "this signature can't be made: {why}"),
            SignError::TooLarge => f.write_str("the message or a certificate is too large to sign"),
            SignError::Holder(e) => write!(f, "the key couldn't sign: {e}"),
            SignError::BadSignature => f.write_str("the key gave back a signature that doesn't match the certificate"),
            SignError::Encoding => f.write_str("the signature couldn't be written"),
        }
    }
}

impl<E: fmt::Display + fmt::Debug> std::error::Error for SignError<E> {}

/// Make the Digital signature of `request`: the DER of a ContentInfo holding a SignedData with
/// no content of its own (detached, for the second part of a `multipart/signed`).
/// `sign_digest` is given the digest (by [`SignRequest::digest`]) of what is signed and gives
/// back the signature as [`SignRequest::scheme`] says; it is called only once everything else
/// has been found good, and what it gives back is checked against the certificate.
pub fn sign<E>(request: &SignRequest<'_>, sign_digest: impl FnOnce(&[u8]) -> Result<Vec<u8>, E>) -> Result<Vec<u8>, SignError<E>> {
    if request.content.len() > MAX_SIGNED_BYTES {
        return Err(SignError::TooLarge);
    }
    let signer = certificate(request.certificate)?;
    if !signer.for_email() || signer.is_authority() {
        return Err(SignError::Certificate("it isn't for signing mail"));
    }
    if !signer.valid_at(request.signing_time) {
        return Err(SignError::Certificate("it isn't valid at the signing time"));
    }
    let signature_algorithm = signature_algorithm(&signer, request.scheme, request.digest)?;
    let certificates = carried(&signer, request.chain)?;

    let digest = request.digest;
    let attributes = signed_attributes(digest.digest(request.content), request.signing_time)?;
    let to_sign = attributes.to_der().map_err(|_| SignError::Encoding)?;
    let signature = sign_digest(&digest.digest(&to_sign)).map_err(SignError::Holder)?;
    if signature.is_empty() || signature.len() > MAX_SIGNATURE_BYTES {
        return Err(SignError::BadSignature);
    }
    algorithms::verify(signer.public_key(), &signature_algorithm, Some(digest.hash()), &to_sign, &signature).map_err(|_| SignError::BadSignature)?;

    let tbs = signer.certificate().tbs_certificate();
    let info = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: tbs.issuer().clone(),
            serial_number: tbs.serial_number().clone(),
        }),
        digest_alg: digest.identifier(),
        signed_attrs: Some(attributes),
        signature_algorithm,
        signature: OctetString::new(signature).map_err(|_| SignError::Encoding)?,
        unsigned_attrs: None,
    };
    let data = SignedData {
        version: CmsVersion::V1,
        digest_algorithms: SetOfVec::try_from(vec![digest.identifier()]).map_err(|_| SignError::Encoding)?,
        encap_content_info: EncapsulatedContentInfo { econtent_type: DATA, econtent: None },
        certificates: Some(CertificateSet(SetOfVec::try_from(certificates).map_err(|_| SignError::Encoding)?)),
        crls: None,
        signer_infos: SignerInfos(SetOfVec::try_from(vec![info]).map_err(|_| SignError::Encoding)?),
    };
    let content = Any::encode_from(&data).map_err(|_| SignError::Encoding)?;
    ContentInfo { content_type: SIGNED_DATA, content }.to_der().map_err(|_| SignError::Encoding)
}

/// The certificate `der` is, in DER exactly (so that what is carried is what was given).
fn certificate<E>(der: &[u8]) -> Result<Cert, SignError<E>> {
    if der.len() > MAX_CERT_BYTES {
        return Err(SignError::TooLarge);
    }
    let cert = Cert::from_der(der).ok_or(SignError::Certificate("it can't be read"))?;
    if cert.certificate().to_der().ok().as_deref() != Some(der) {
        return Err(SignError::Certificate("it isn't in DER"));
    }
    Ok(cert)
}

/// The signer's certificate and the chain's, each once.
fn carried<E>(signer: &Cert, chain: &[Vec<u8>]) -> Result<Vec<CertificateChoices>, SignError<E>> {
    if chain.len() > MAX_CHAIN {
        return Err(SignError::TooLarge);
    }
    let mut seen: Vec<&[u8]> = vec![signer.der()];
    let mut out = vec![CertificateChoices::Certificate(signer.certificate().clone())];
    for der in chain {
        if seen.contains(&der.as_slice()) {
            continue;
        }
        let cert = certificate::<E>(der).map_err(|e| match e {
            SignError::TooLarge => SignError::TooLarge,
            _ => SignError::Certificate("a certificate of the chain can't be read"),
        })?;
        seen.push(der);
        out.push(CertificateChoices::Certificate(cert.certificate().clone()));
    }
    Ok(out)
}

/// The signatureAlgorithm for `scheme` with `digest`, when the signer's key is of its kind.
fn signature_algorithm<E>(signer: &Cert, scheme: SignatureScheme, digest: DigestAlgorithm) -> Result<AlgorithmIdentifierOwned, SignError<E>> {
    let kind = algorithms::signing_kind(signer.public_key()).map_err(|fault| match fault {
        Fault::Weak => SignError::Certificate("its RSA key is shorter than 2048 bits"),
        Fault::Malformed => SignError::Certificate("its key can't be read"),
        Fault::Unknown | Fault::Invalid => SignError::Unsupported("its key is of a kind that can't sign here"),
    })?;
    match (scheme, kind) {
        // `rsaEncryption`, the hash given by the digest algorithm (RFC 5754 §3.2): what every
        // reader takes.
        (SignatureScheme::RsaPkcs1v15, SigningKind::Rsa) => Ok(AlgorithmIdentifierOwned { oid: algorithms::RSA, parameters: Some(Any::null()) }),
        (SignatureScheme::RsaPss, SigningKind::Rsa) => {
            let hash = spki::AlgorithmIdentifier::<Any> { oid: digest.oid(), parameters: None };
            let params = rsa::pkcs1::RsaPssParams::<Any> {
                hash: hash.clone(),
                mask_gen: spki::AlgorithmIdentifier { oid: algorithms::MGF1, parameters: Some(hash) },
                salt_len: digest.len(),
                trailer_field: rsa::pkcs1::TrailerField::BC,
            };
            let parameters = Any::encode_from(&params).map_err(|_| SignError::Encoding)?;
            Ok(AlgorithmIdentifierOwned { oid: algorithms::PSS, parameters: Some(parameters) })
        }
        (SignatureScheme::Ecdsa, SigningKind::P256 | SigningKind::P384) => {
            let oid = match digest {
                DigestAlgorithm::Sha256 => algorithms::SHA256_ECDSA,
                DigestAlgorithm::Sha384 => algorithms::SHA384_ECDSA,
                DigestAlgorithm::Sha512 => algorithms::SHA512_ECDSA,
            };
            Ok(AlgorithmIdentifierOwned { oid, parameters: None })
        }
        (SignatureScheme::RsaPkcs1v15 | SignatureScheme::RsaPss, _) => Err(SignError::Unsupported("an RSA signature needs an RSA key")),
        (SignatureScheme::Ecdsa, SigningKind::Rsa) => Err(SignError::Unsupported("an ECDSA signature needs an elliptic-curve key")),
    }
}

/// The signed attributes (RFC 5652 §11, RFC 8551 §2.5): the content's type and digest, the
/// signing time and the algorithms mail to the signer may be encrypted with, as a SET OF in
/// DER order.
fn signed_attributes<E>(message_digest: Vec<u8>, signing_time: i64) -> Result<SetOfVec<Attribute>, SignError<E>> {
    let encoding = |_| SignError::Encoding;
    let attribute = |oid: ObjectIdentifier, value: Any| -> Result<Attribute, SignError<E>> {
        Ok(Attribute { oid, values: SetOfVec::try_from(vec![value]).map_err(encoding)? })
    };
    // Strongest first (RFC 8551 §2.5.2).
    let capabilities =
        vec![AlgorithmIdentifierOwned { oid: AES256_CBC, parameters: None }, AlgorithmIdentifierOwned { oid: AES128_CBC, parameters: None }];
    let attributes = vec![
        attribute(CONTENT_TYPE, Any::encode_from(&DATA).map_err(encoding)?)?,
        attribute(SIGNING_TIME, Any::encode_from(&time(signing_time)?).map_err(encoding)?)?,
        attribute(MESSAGE_DIGEST, Any::encode_from(&OctetString::new(message_digest).map_err(encoding)?).map_err(encoding)?)?,
        attribute(SMIME_CAPABILITIES, Any::encode_from(&capabilities).map_err(encoding)?)?,
    ];
    SetOfVec::try_from(attributes).map_err(encoding)
}

/// A signing time as RFC 5652 §11.3 writes it: UTCTime for the years 1950 to 2049, else
/// GeneralizedTime. (Times before 1970 aren't made.)
fn time<E>(seconds: i64) -> Result<Time, SignError<E>> {
    let seconds = u64::try_from(seconds).map_err(|_| SignError::Unsupported("a signing time before 1970"))?;
    let since = std::time::Duration::from_secs(seconds);
    let at = der::DateTime::from_unix_duration(since).map_err(|_| SignError::Unsupported("a signing time past 9999"))?;
    if at.year() <= 2049 {
        UtcTime::from_unix_duration(since).map(Time::UtcTime).map_err(|_| SignError::Encoding)
    } else {
        GeneralizedTime::from_unix_duration(since).map(Time::GeneralTime).map_err(|_| SignError::Encoding)
    }
}
