// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Making S/MIME Digital signatures (#30): with keys and example.org certificates made here,
//! each signature read back by the crate's own check; and never a panic on any certificate.

use std::cell::Cell;
use std::sync::OnceLock;

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use const_oid::ObjectIdentifier;
use der::{Decode, Encode};
use mailcraft_model::digital_signature::{Coverage, Verdict, Why};
use p256::ecdsa::signature::hazmat::PrehashSigner;
use proptest::prelude::*;
use rsa::pkcs1::EncodeRsaPublicKey;

use crate::{Anchors, Context, DigestAlgorithm, MAX_CHAIN, MAX_SIGNED_BYTES, SignError, SignRequest, SignatureScheme, Signed, check, sign};

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;
const CONTENT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe minutes are attached.\r\n";
const SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
const SMIME_CAPABILITIES: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.15");
const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const PSS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10");
const ECDSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");

// ---- randomness and keys ----

/// The system's randomness, as the RSA library asks for it.
pub(crate) struct Random(ring::rand::SystemRandom);

impl rsa::rand_core::TryRng for Random {
    type Error = std::convert::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0u8; 4];
        self.try_fill_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0u8; 8];
        self.try_fill_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        ring::rand::SecureRandom::fill(&self.0, dst).unwrap();
        Ok(())
    }
}

impl rsa::rand_core::TryCryptoRng for Random {}

pub(crate) fn random() -> Random {
    Random(ring::rand::SystemRandom::new())
}

/// An RSA key of `bits`, made once per test run.
pub(crate) fn rsa_key(bits: usize) -> &'static rsa::PrivateKey {
    static K2048: OnceLock<rsa::PrivateKey> = OnceLock::new();
    static K1024: OnceLock<rsa::PrivateKey> = OnceLock::new();
    let cell = if bits == 1024 { &K1024 } else { &K2048 };
    cell.get_or_init(|| rsa::PrivateKey::new(&mut random(), bits).unwrap())
}

fn p256_key() -> p256::ecdsa::SigningKey {
    loop {
        let mut b = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
        if let Ok(k) = p256::ecdsa::SigningKey::from_slice(&b) {
            return k;
        }
    }
}

fn p384_key() -> p384::ecdsa::SigningKey {
    loop {
        let mut b = [0u8; 48];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
        if let Ok(k) = p384::ecdsa::SigningKey::from_slice(&b) {
            return k;
        }
    }
}

/// A public key as rcgen puts it in a certificate.
pub(crate) struct Public {
    der: Vec<u8>,
    alg: &'static rcgen::SignatureAlgorithm,
}

impl rcgen::PublicKeyData for Public {
    fn der_bytes(&self) -> &[u8] {
        &self.der
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        self.alg
    }
}

pub(crate) fn rsa_public(key: &rsa::PrivateKey) -> Public {
    Public { der: key.to_public_key().to_pkcs1_der().unwrap().as_bytes().to_vec(), alg: &rcgen::PKCS_RSA_SHA256 }
}

// ---- certificates ----

pub(crate) struct Authority {
    der: Vec<u8>,
    key: rcgen::KeyPair,
    params: rcgen::CertificateParams,
}

fn params(name: &str) -> rcgen::CertificateParams {
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name.push(rcgen::DnType::CommonName, name);
    p.not_before = rcgen::date_time_ymd(2020, 1, 1);
    p.not_after = rcgen::date_time_ymd(2060, 1, 1);
    p
}

fn ca_params(name: &str) -> rcgen::CertificateParams {
    let mut p = params(name);
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    p
}

pub(crate) fn root(name: &str) -> Authority {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let params = ca_params(name);
    let der = params.self_signed(&key).unwrap().der().to_vec();
    Authority { der, key, params }
}

fn intermediate(name: &str, by: &Authority) -> Authority {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let params = ca_params(name);
    let der = params.signed_by(&key, &rcgen::Issuer::new(by.params.clone(), &by.key)).unwrap().der().to_vec();
    Authority { der, key, params }
}

/// A mail signer's certificate for `address` and `key`, issued by `by`.
pub(crate) fn signer_cert(address: &str, key: &impl rcgen::PublicKeyData, by: &Authority) -> Vec<u8> {
    signer_cert_with(address, key, by, &[])
}

/// [`signer_cert`] with a subject key identifier, when `key_id` is not empty.
pub(crate) fn signer_cert_with(address: &str, key: &impl rcgen::PublicKeyData, by: &Authority, key_id: &[u8]) -> Vec<u8> {
    let mut p = params("Ada");
    if !key_id.is_empty() {
        // SubjectKeyIdentifier (2.5.29.14): an OCTET STRING of the identifier, in the extension's.
        let mut inner = vec![0x04, key_id.len() as u8];
        inner.extend_from_slice(key_id);
        p.custom_extensions.push(rcgen::CustomExtension::from_oid_content(&[2, 5, 29, 14], inner));
    }
    p.subject_alt_names = vec![rcgen::SanType::Rfc822Name(address.try_into().unwrap())];
    p.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::EmailProtection];
    p.signed_by(key, &rcgen::Issuer::new(by.params.clone(), &by.key)).unwrap().der().to_vec()
}

// ---- signing ----

fn request<'a>(certificate: &'a [u8], chain: &'a [Vec<u8>], digest: DigestAlgorithm, scheme: SignatureScheme) -> SignRequest<'a> {
    SignRequest { content: CONTENT, certificate, chain, digest, scheme, signing_time: NOW }
}

fn rsa_pkcs1(key: &rsa::PrivateKey, digest: DigestAlgorithm, hashed: &[u8]) -> Vec<u8> {
    let padding = match digest {
        DigestAlgorithm::Sha256 => rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
        DigestAlgorithm::Sha384 => rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
        DigestAlgorithm::Sha512 => rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
    };
    key.sign(padding, hashed).unwrap()
}

fn rsa_pss(key: &rsa::PrivateKey, digest: DigestAlgorithm, hashed: &[u8]) -> Vec<u8> {
    let mut rng = random();
    match digest {
        DigestAlgorithm::Sha256 => key.sign_with_rng(&mut rng, rsa::Pss::<sha2::Sha256>::new_with_salt(32), hashed),
        DigestAlgorithm::Sha384 => key.sign_with_rng(&mut rng, rsa::Pss::<sha2::Sha384>::new_with_salt(48), hashed),
        DigestAlgorithm::Sha512 => key.sign_with_rng(&mut rng, rsa::Pss::<sha2::Sha512>::new_with_salt(64), hashed),
    }
    .unwrap()
}

/// What `check` says of `cms` over `content` for `from`, with `anchors` trusted.
fn verdict(cms: &[u8], content: &[u8], anchors: &Anchors, from: &str) -> (Verdict, Option<Why>) {
    let c = check(Signed::Detached { content, signature: cms }, &Context { anchors, from, now: NOW, known: &[], coverage: Coverage::Whole }).unwrap();
    (c.signature.verdict, c.signature.why)
}

fn signed_data(cms: &[u8]) -> SignedData {
    let info = ContentInfo::from_der(cms).unwrap();
    assert_eq!(info.content_type, ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2"));
    info.content.decode_as::<SignedData>().unwrap()
}

type NoError = SignError<&'static str>;

#[test]
fn micalg_names() {
    assert_eq!(DigestAlgorithm::Sha256.micalg(), "sha-256");
    assert_eq!(DigestAlgorithm::Sha384.micalg(), "sha-384");
    assert_eq!(DigestAlgorithm::Sha512.micalg(), "sha-512");
}

#[test]
fn rsa_pkcs1_signatures_are_verified() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    let cert = signer_cert("ada@example.org", &rsa_public(key), &ca);
    let anchors = Anchors::new([ca.der.as_slice()]);
    for digest in [DigestAlgorithm::Sha256, DigestAlgorithm::Sha512] {
        let handed = Cell::new(Vec::new());
        let cms = sign(&request(&cert, &[], digest, SignatureScheme::RsaPkcs1v15), |hashed| {
            handed.set(hashed.to_vec());
            Ok::<_, &str>(rsa_pkcs1(key, digest, hashed))
        })
        .unwrap();
        assert_eq!(verdict(&cms, CONTENT, &anchors, "ada@example.org"), (Verdict::Verified, None), "{digest:?}");
        let data = signed_data(&cms);
        assert!(data.encap_content_info.econtent.is_none(), "detached");
        let info = data.signer_infos.0.iter().next().unwrap();
        assert_eq!(info.signature_algorithm.oid, RSA_ENCRYPTION);
        assert_eq!(info.signature_algorithm.parameters.as_ref().map(|p| p.to_der().unwrap()), Some(vec![0x05, 0x00]));
        // The holder was handed the digest of the signed attributes as a SET OF, in DER order.
        let attrs = info.signed_attrs.as_ref().unwrap();
        assert_eq!(handed.take(), digest.digest(&attrs.to_der().unwrap()));
        let encoded: Vec<Vec<u8>> = attrs.iter().map(|a| a.to_der().unwrap()).collect();
        let mut sorted = encoded.clone();
        sorted.sort();
        assert_eq!(encoded, sorted);
        assert_eq!(attrs.len(), 4);
        // sMIMECapabilities: AES-256-CBC, then AES-128-CBC.
        let caps = attrs.iter().find(|a| a.oid == SMIME_CAPABILITIES).unwrap();
        let caps = caps.values.iter().next().unwrap().decode_as::<Vec<spki::AlgorithmIdentifierOwned>>().unwrap();
        let oids: Vec<String> = caps.iter().map(|c| c.oid.to_string()).collect();
        assert_eq!(oids, ["2.16.840.1.101.3.4.1.42", "2.16.840.1.101.3.4.1.2"]);
    }
}

#[test]
fn rsa_pss_signatures_are_verified() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    let cert = signer_cert("ada@example.org", &rsa_public(key), &ca);
    let anchors = Anchors::new([ca.der.as_slice()]);
    for digest in [DigestAlgorithm::Sha256, DigestAlgorithm::Sha384] {
        let cms = sign(&request(&cert, &[], digest, SignatureScheme::RsaPss), |hashed| Ok::<_, &str>(rsa_pss(key, digest, hashed))).unwrap();
        assert_eq!(verdict(&cms, CONTENT, &anchors, "ada@example.org"), (Verdict::Verified, None), "{digest:?}");
        let data = signed_data(&cms);
        let info = data.signer_infos.0.iter().next().unwrap();
        assert_eq!(info.signature_algorithm.oid, PSS);
        let params = info.signature_algorithm.parameters.as_ref().unwrap().to_der().unwrap();
        let params = rsa::pkcs1::RsaPssParams::<der::Any>::from_der(&params).unwrap();
        assert_eq!(params.salt_len, if digest == DigestAlgorithm::Sha256 { 32 } else { 48 });
        assert_eq!(params.hash.oid, params.mask_gen.parameters.as_ref().unwrap().oid, "MGF1 with the same hash");
    }
}

#[test]
fn ecdsa_signatures_are_verified() {
    let ca = root("Example Root");
    let anchors = Anchors::new([ca.der.as_slice()]);
    let p256 = p256_key();
    let public = Public { der: p256.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 };
    let cert = signer_cert("ada@example.org", &public, &ca);
    let cms = sign(&request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa), |hashed| {
        let sig: p256::ecdsa::Signature = p256.sign_prehash(hashed).unwrap();
        Ok::<_, &str>(sig.to_der().as_bytes().to_vec())
    })
    .unwrap();
    assert_eq!(verdict(&cms, CONTENT, &anchors, "ada@example.org"), (Verdict::Verified, None));

    let p384 = p384_key();
    let public = Public { der: p384.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P384_SHA384 };
    let cert = signer_cert("ada@example.org", &public, &ca);
    let cms = sign(&request(&cert, &[], DigestAlgorithm::Sha384, SignatureScheme::Ecdsa), |hashed| {
        let sig: p384::ecdsa::Signature = p384.sign_prehash(hashed).unwrap();
        Ok::<_, &str>(sig.to_der().as_bytes().to_vec())
    })
    .unwrap();
    assert_eq!(verdict(&cms, CONTENT, &anchors, "ada@example.org"), (Verdict::Verified, None));
    let info = signed_data(&cms).signer_infos.0.iter().next().cloned().unwrap();
    assert_eq!((info.signature_algorithm.oid, info.signature_algorithm.parameters), (ECDSA_SHA384, None));
    // Another sender, and a change after signing.
    assert_eq!(verdict(&cms, CONTENT, &anchors, "grace@example.org"), (Verdict::NotVerified, Some(Why::AddressMismatch)));
    let mut changed = CONTENT.to_vec();
    changed[50] ^= 1;
    assert_eq!(verdict(&cms, &changed, &anchors, "ada@example.org"), (Verdict::Changed, Some(Why::Altered)));
}

#[test]
fn a_holder_that_gives_back_no_signature_of_the_key_makes_nothing() {
    let ca = root("Example Root");
    let key = p256_key();
    let public = Public { der: key.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 };
    let cert = signer_cert("ada@example.org", &public, &ca);
    let req = request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa);
    assert_eq!(sign(&req, |_| Ok::<_, &str>(vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01])), Err(SignError::BadSignature));
    assert_eq!(sign(&req, |_| Ok::<_, &str>(Vec::new())), Err(SignError::BadSignature));
    assert_eq!(sign(&req, |_| Ok::<_, &str>(vec![0xff; 4096])), Err(SignError::BadSignature));
    // Signed by another key, or over something else.
    let other = p256_key();
    assert_eq!(
        sign(&req, |hashed| Ok::<_, &str>(
            PrehashSigner::<p256::ecdsa::Signature>::sign_prehash(&other, hashed).unwrap().to_der().as_bytes().to_vec()
        )),
        Err(SignError::BadSignature)
    );
    assert_eq!(
        sign(&req, |_| Ok::<_, &str>(PrehashSigner::<p256::ecdsa::Signature>::sign_prehash(&key, &[7; 32]).unwrap().to_der().as_bytes().to_vec())),
        Err(SignError::BadSignature)
    );
    // An RSA holder that padded for another digest.
    let rsa = rsa_key(2048);
    let cert = signer_cert("ada@example.org", &rsa_public(rsa), &ca);
    let req = request(&cert, &[], DigestAlgorithm::Sha512, SignatureScheme::RsaPkcs1v15);
    assert_eq!(sign(&req, |hashed| Ok::<_, &str>(rsa_pkcs1(rsa, DigestAlgorithm::Sha256, &hashed[..32]))), Err(SignError::BadSignature));
    // PSS asked for, PKCS #1 v1.5 given.
    let req = request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::RsaPss);
    assert_eq!(sign(&req, |hashed| Ok::<_, &str>(rsa_pkcs1(rsa, DigestAlgorithm::Sha256, hashed))), Err(SignError::BadSignature));
}

#[test]
fn the_holders_error_is_passed_on() {
    let ca = root("Example Root");
    let key = p256_key();
    let public = Public { der: key.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 };
    let cert = signer_cert("ada@example.org", &public, &ca);
    let r = sign(&request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa), |_| Err("the card was taken out"));
    assert_eq!(r, Err(SignError::Holder("the card was taken out")));
    assert_eq!(r.unwrap_err().to_string(), "the key couldn't sign: the card was taken out");
}

/// `sign` of `req`, with a holder that must never be asked.
fn refused(req: &SignRequest<'_>) -> NoError {
    let asked = Cell::new(false);
    let r = sign(req, |_| {
        asked.set(true);
        Err("asked")
    });
    assert!(!asked.get(), "the holder isn't asked");
    r.unwrap_err()
}

#[test]
fn a_scheme_that_doesnt_fit_the_key_and_a_short_rsa_key_are_refused_before_signing() {
    let ca = root("Example Root");
    let rsa_cert = signer_cert("ada@example.org", &rsa_public(rsa_key(2048)), &ca);
    let p256 = p256_key();
    let ec_cert =
        signer_cert("ada@example.org", &Public { der: p256.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 }, &ca);
    assert!(matches!(refused(&request(&rsa_cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa)), SignError::Unsupported(_)));
    for scheme in [SignatureScheme::RsaPkcs1v15, SignatureScheme::RsaPss] {
        assert!(matches!(refused(&request(&ec_cert, &[], DigestAlgorithm::Sha256, scheme)), SignError::Unsupported(_)));
    }
    let short = signer_cert("ada@example.org", &rsa_public(rsa_key(1024)), &ca);
    assert_eq!(
        refused(&request(&short, &[], DigestAlgorithm::Sha256, SignatureScheme::RsaPkcs1v15)),
        SignError::Certificate("its RSA key is shorter than 2048 bits")
    );
    // An authority's certificate, and one not valid when signing.
    assert!(matches!(refused(&request(&ca.der, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa)), SignError::Certificate(_)));
    let mut late = request(&ec_cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa);
    late.signing_time = 2_840_140_801; // a second past its end, 2060-01-01
    assert!(matches!(refused(&late), SignError::Certificate(_)));
    late.signing_time = -1;
    assert!(matches!(refused(&late), SignError::Certificate(_) | SignError::Unsupported(_)));
}

#[test]
fn too_much_content_is_refused() {
    let ca = root("Example Root");
    let p256 = p256_key();
    let cert =
        signer_cert("ada@example.org", &Public { der: p256.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 }, &ca);
    let big = vec![0u8; MAX_SIGNED_BYTES + 1];
    let mut req = request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa);
    req.content = &big;
    assert_eq!(refused(&req), SignError::TooLarge);
}

#[test]
fn the_signing_time_is_utc_time_until_2049_and_generalized_time_after() {
    let ca = root("Example Root");
    let p256 = p256_key();
    let cert =
        signer_cert("ada@example.org", &Public { der: p256.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 }, &ca);
    // 2049-12-31T23:59:59Z and 2050-01-01T00:00:00Z.
    for (time, tag, text) in [(2_524_607_999i64, 0x17u8, &b"491231235959Z"[..]), (2_524_608_000, 0x18, &b"20500101000000Z"[..])] {
        let mut req = request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa);
        req.signing_time = time;
        let cms = sign(&req, |hashed| {
            let sig: p256::ecdsa::Signature = p256.sign_prehash(hashed).unwrap();
            Ok::<_, &str>(sig.to_der().as_bytes().to_vec())
        })
        .unwrap();
        let data = signed_data(&cms);
        let info = data.signer_infos.0.iter().next().unwrap();
        let attr = info.signed_attrs.as_ref().unwrap().iter().find(|a| a.oid == SIGNING_TIME).unwrap();
        let value = attr.values.iter().next().unwrap().to_der().unwrap();
        assert_eq!(value[0], tag, "{time}");
        assert_eq!(&value[2..], text);
        let c = check(
            Signed::Detached { content: CONTENT, signature: &cms },
            &Context { anchors: &Anchors::new([ca.der.as_slice()]), from: "ada@example.org", now: NOW, known: &[], coverage: Coverage::Whole },
        )
        .unwrap();
        assert_eq!(c.signature.signed_at, Some(time));
    }
}

#[test]
fn the_chain_is_carried_and_capped() {
    let root = root("Example Root");
    let mid = intermediate("Example Mail CA", &root);
    let p256 = p256_key();
    let cert =
        signer_cert("ada@example.org", &Public { der: p256.verifying_key().to_sec1_bytes().to_vec(), alg: &rcgen::PKCS_ECDSA_P256_SHA256 }, &mid);
    let anchors = Anchors::new([root.der.as_slice()]);
    let holder = |hashed: &[u8]| {
        let sig: p256::ecdsa::Signature = p256.sign_prehash(hashed).unwrap();
        Ok::<_, &str>(sig.to_der().as_bytes().to_vec())
    };
    // Without the intermediate authority, nothing leads to the root.
    let alone = sign(&request(&cert, &[], DigestAlgorithm::Sha256, SignatureScheme::Ecdsa), holder).unwrap();
    assert_eq!(verdict(&alone, CONTENT, &anchors, "ada@example.org"), (Verdict::NotVerified, Some(Why::UnknownIssuer)));
    // With it (given twice, and the signer's own once more: each is carried once).
    let chain = vec![mid.der.clone(), mid.der.clone(), cert.clone()];
    let cms = sign(&request(&cert, &chain, DigestAlgorithm::Sha256, SignatureScheme::Ecdsa), holder).unwrap();
    assert_eq!(verdict(&cms, CONTENT, &anchors, "ada@example.org"), (Verdict::Verified, None));
    let carried: Vec<Vec<u8>> = signed_data(&cms).certificates.unwrap().0.iter().map(|c| c.to_der().unwrap()).collect();
    assert_eq!(carried.len(), 2);
    assert!(carried.contains(&cert) && carried.contains(&mid.der));
    // More than the cap, a certificate that can't be read, and one too large.
    let many = vec![mid.der.clone(); MAX_CHAIN + 1];
    assert_eq!(refused(&request(&cert, &many, DigestAlgorithm::Sha256, SignatureScheme::Ecdsa)), SignError::TooLarge);
    let junk = vec![b"not a certificate".to_vec()];
    assert!(matches!(refused(&request(&cert, &junk, DigestAlgorithm::Sha256, SignatureScheme::Ecdsa)), SignError::Certificate(_)));
    let huge = vec![vec![0x30; 64 * 1024 + 1]];
    assert_eq!(refused(&request(&cert, &huge, DigestAlgorithm::Sha256, SignatureScheme::Ecdsa)), SignError::TooLarge);
}

#[test]
fn errors_say_nothing_of_the_key() {
    let e: NoError = SignError::BadSignature;
    assert_eq!(e.to_string(), "the key gave back a signature that doesn't match the certificate");
    let e: NoError = SignError::Certificate("it can't be read");
    assert_eq!(e.to_string(), "the certificate can't be used for signing: it can't be read");
    let _: &dyn std::error::Error = &e;
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn any_bytes_as_a_certificate_never_panic(certificate in proptest::collection::vec(any::<u8>(), 0..512), link in proptest::collection::vec(any::<u8>(), 0..256)) {
        let chain = vec![link];
        for scheme in [SignatureScheme::RsaPkcs1v15, SignatureScheme::RsaPss, SignatureScheme::Ecdsa] {
            let req = SignRequest { content: CONTENT, certificate: &certificate, chain: &chain, digest: DigestAlgorithm::Sha256, scheme, signing_time: NOW };
            let asked = Cell::new(false);
            let r = sign(&req, |_| { asked.set(true); Err("asked") });
            prop_assert!(r.is_err());
            prop_assert!(!asked.get());
        }
    }
}
