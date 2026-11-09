// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key holders' logic, with synthetic example.org certificates made here (their signatures
//! are filler: nothing here checks one). The systems' stores themselves are reached only on the
//! systems; PKCS#11 has an opt-in test against a module the tester names.

use std::str::FromStr;

use x509_cert::der::asn1::{BitString, Ia5String, ObjectIdentifier, OctetString};
use x509_cert::der::{Any, Decode, Encode, Reader, SliceReader, Tag};
use x509_cert::ext::Extension;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{ExtendedKeyUsage, KeyUsage, KeyUsages, SubjectAltName};
use x509_cert::name::Name;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};

use super::certificate::{clean_name, hex, sha256, without_leading_zeros};
use super::encoding::{digest_info, ecdsa_raw_to_der};
use super::{
    DigestAlgorithm, HeldCertificate, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, MAX_CERTIFICATES, MIN_RSA_BITS, Pin, RsaPadding, SignatureScheme,
    TokenRef, UnwrappedKey, check_for_decrypting, check_for_signing, check_p12, check_wrapped, now, usable,
};

const RSA: &str = "1.2.840.113549.1.1.1";
const EC: &str = "1.2.840.10045.2.1";
const P256: &str = "1.2.840.10045.3.1.7";
const P384: &str = "1.3.132.0.34";
const EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
const SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";
const NOT_BEFORE: u64 = 1_767_225_600; // 2026-01-01
const NOT_AFTER: u64 = 1_798_761_600; // 2027-01-01
/// A moment the test certificates are valid at.
const IN_DATE: Option<u64> = Some(1_780_000_000);

fn oid(text: &str) -> ObjectIdentifier {
    ObjectIdentifier::new(text).unwrap()
}

/// An RSA modulus of `bits` bits (top bit set), as the certificate's INTEGER holds it.
fn modulus(bits: usize) -> Vec<u8> {
    let mut n = vec![0x00, 0x80];
    n.resize(bits / 8 + 1, 0x01);
    n
}

/// An `RSAPublicKey` with a modulus of `bits` bits and e = 65537.
fn rsa_public_key(bits: usize) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(der_tlv(0x02, &modulus(bits)));
    body.extend(der_tlv(0x02, &[0x01, 0x00, 0x01]));
    der_tlv(0x30, &body)
}

fn der_tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = value.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend([0x81, len as u8]);
    } else {
        out.extend([0x82, (len >> 8) as u8, len as u8]);
    }
    out.extend_from_slice(value);
    out
}

struct Spec {
    subject: String,
    key: SubjectPublicKeyInfoOwned,
    extensions: Vec<Extension>,
    validity: (&'static str, &'static str),
}

impl Spec {
    fn new(subject: &str, key: SubjectPublicKeyInfoOwned, extensions: Vec<Extension>) -> Self {
        Self { subject: subject.to_string(), key, extensions, validity: ("260101000000Z", "270101000000Z") }
    }
}

fn rsa_key_of(bits: usize) -> SubjectPublicKeyInfoOwned {
    SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned { oid: oid(RSA), parameters: Some(Any::new(Tag::Null, Vec::new()).unwrap()) },
        subject_public_key: BitString::from_bytes(&rsa_public_key(bits)).unwrap(),
    }
}

fn rsa_key() -> SubjectPublicKeyInfoOwned {
    rsa_key_of(2048)
}

fn ec_point() -> Vec<u8> {
    let mut point = vec![0x04];
    point.extend([0x11; 64]);
    point
}

fn ec_key(curve: &str) -> SubjectPublicKeyInfoOwned {
    SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned { oid: oid(EC), parameters: Some(Any::encode_from(&oid(curve)).unwrap()) },
        subject_public_key: BitString::from_bytes(&ec_point()).unwrap(),
    }
}

fn extension<T: Encode>(id: &str, critical: bool, value: &T) -> Extension {
    Extension { extn_id: oid(id), critical, extn_value: OctetString::new(value.to_der().unwrap()).unwrap() }
}

fn san(addresses: &[&str]) -> Extension {
    let names = addresses.iter().map(|a| GeneralName::Rfc822Name(Ia5String::new(a).unwrap())).collect();
    extension("2.5.29.17", false, &SubjectAltName(names))
}

fn eku(usages: &[&str]) -> Extension {
    extension("2.5.29.37", false, &ExtendedKeyUsage(usages.iter().map(|u| oid(u)).collect()))
}

fn key_usage(usages: x509_cert::der::flagset::FlagSet<KeyUsages>) -> Extension {
    extension("2.5.29.15", true, &KeyUsage(usages))
}

/// A certificate as RFC 5280 §4.1 lays it out, put together by hand (x509-cert keeps its fields
/// to itself); its signature is filler.
fn certificate(spec: Spec) -> Vec<u8> {
    let algorithm = AlgorithmIdentifierOwned { oid: oid("1.2.840.113549.1.1.11"), parameters: None }.to_der().unwrap();
    let mut validity = Vec::new();
    validity.extend(der_tlv(0x17, spec.validity.0.as_bytes()));
    validity.extend(der_tlv(0x17, spec.validity.1.as_bytes()));
    let mut tbs = Vec::new();
    tbs.extend([0xa0, 0x03, 0x02, 0x01, 0x02]); // version v3
    tbs.extend([0x02, 0x02, 0x01, 0x02]); // serial number
    tbs.extend(&algorithm);
    tbs.extend(Name::from_str("CN=Example Test CA,O=Example").unwrap().to_der().unwrap());
    tbs.extend(der_tlv(0x30, &validity));
    tbs.extend(Name::from_str(&spec.subject).unwrap().to_der().unwrap());
    tbs.extend(spec.key.to_der().unwrap());
    if !spec.extensions.is_empty() {
        tbs.extend(der_tlv(0xa3, &spec.extensions.to_der().unwrap()));
    }
    let mut cert = der_tlv(0x30, &tbs);
    cert.extend(&algorithm);
    cert.extend(BitString::from_bytes(&[0x5a; 16]).unwrap().to_der().unwrap());
    der_tlv(0x30, &cert)
}

fn mail_extensions() -> Vec<Extension> {
    vec![san(&["Alice@Example.org"]), eku(&[EMAIL_PROTECTION]), key_usage(KeyUsages::DigitalSignature | KeyUsages::KeyEncipherment)]
}

fn alice_rsa() -> Vec<u8> {
    certificate(Spec::new("CN=Alice Example,O=Example", rsa_key(), mail_extensions()))
}

fn read(der: Vec<u8>) -> HeldCertificate {
    HeldCertificate::read(der, IN_DATE).unwrap()
}

#[test]
fn an_rsa_certificate_for_mail_is_read_whole() {
    let der = alice_rsa();
    let held = read(der.clone());
    assert_eq!(held.fingerprint, sha256(&der));
    assert_eq!(held.fingerprint_hex().len(), 64);
    assert!(held.subject.contains("CN=Alice Example"), "{}", held.subject);
    assert!(held.issuer.contains("CN=Example Test CA"), "{}", held.issuer);
    assert_eq!(held.emails, vec!["alice@example.org".to_string()]);
    assert_eq!((held.not_before, held.not_after), (NOT_BEFORE, NOT_AFTER));
    assert!(held.in_date);
    assert_eq!(held.key, KeyKind::Rsa { bits: 2048 });
    assert_eq!(held.public_key, without_leading_zeros(&modulus(2048)));
    assert!(held.for_mail && held.can_sign && held.can_decrypt);
    assert_eq!(held.der, der);
}

#[test]
fn an_ec_certificate_with_no_extensions_takes_the_subjects_address() {
    let held = read(certificate(Spec::new("CN=Bob Example,emailAddress=bob@example.org", ec_key(P256), Vec::new())));
    assert_eq!(held.emails, vec!["bob@example.org".to_string()]);
    assert_eq!(held.key, KeyKind::EcP256);
    assert_eq!(held.public_key, ec_point());
    // No usages named: any purpose, but an EC key unwraps nothing here (ECDH is not RSA's job).
    assert!(held.for_mail && held.can_sign && !held.can_decrypt);

    let der = certificate(Spec::new("CN=Carol Example", ec_key(P384), vec![san(&["carol@example.org"])]));
    assert_eq!(read(der).key, KeyKind::EcP384);
}

#[test]
fn key_usage_decides_signing_and_unwrapping() {
    let held = read(certificate(Spec::new("CN=Dan", rsa_key(), vec![key_usage(KeyUsages::NonRepudiation.into())])));
    assert!(held.can_sign && !held.can_decrypt);
    let held = read(certificate(Spec::new("CN=Dan", rsa_key(), vec![key_usage(KeyUsages::KeyEncipherment.into())])));
    assert!(!held.can_sign && held.can_decrypt);
}

#[test]
fn only_certificates_for_mail_are_listed_each_once() {
    let web = certificate(Spec::new("CN=www.example.org", rsa_key(), vec![eku(&[SERVER_AUTH])]));
    assert!(!read(web.clone()).for_mail);
    let neither = certificate(Spec::new("CN=Eve", rsa_key(), vec![key_usage(KeyUsages::KeyCertSign.into())]));
    let listed = usable([alice_rsa(), web, neither, alice_rsa(), b"not a certificate".to_vec()], IN_DATE);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first().map(|h| h.emails.clone()), Some(vec!["alice@example.org".to_string()]));
}

#[test]
fn addresses_that_cant_be_addresses_are_left_out() {
    let der =
        certificate(Spec::new("CN=Frank", rsa_key(), vec![san(&["no-at-sign", "frank@example.org", "FRANK@example.org", "two words@example.org"])]));
    assert_eq!(read(der).emails, vec!["frank@example.org".to_string()]);
}

/// Review item 13: names lose format characters (a bidi override would turn the text around it).
#[test]
fn names_lose_control_and_format_characters() {
    assert_eq!(clean_name("CN=Alice \u{202e}gro.elpmaxe\u{202c} Example"), "CN=Alice gro.elpmaxe Example");
    assert_eq!(clean_name("Example\u{0007} Card\u{200b}"), "Example Card");
    assert_eq!(clean_name("\u{202e}"), "");
    let held = read(certificate(Spec::new("CN=Mallory \u{202e}Example", rsa_key(), Vec::new())));
    assert!(!held.subject.contains('\u{202e}'), "{:?}", held.subject);
    assert!(held.subject.contains("Mallory"));
}

/// Review item 14: RFC 5280 §4.2.
#[test]
fn a_duplicated_or_unknown_critical_extension_is_refused() {
    let twice = certificate(Spec::new("CN=Grace", rsa_key(), vec![san(&["grace@example.org"]), san(&["other@example.org"])]));
    assert_eq!(HeldCertificate::read(twice, IN_DATE), Err(KeyHolderError::BadInput("an extension is there twice")));

    let unknown_critical = Extension { extn_id: oid("1.3.6.1.4.1.55555.1"), critical: true, extn_value: OctetString::new(vec![0x05, 0x00]).unwrap() };
    let refused = certificate(Spec::new("CN=Grace", rsa_key(), vec![unknown_critical.clone()]));
    assert_eq!(HeldCertificate::read(refused, IN_DATE), Err(KeyHolderError::BadInput("a critical extension this reader doesn't know")));

    let unknown_quiet = Extension { critical: false, ..unknown_critical };
    assert!(HeldCertificate::read(certificate(Spec::new("CN=Grace", rsa_key(), vec![unknown_quiet])), IN_DATE).is_ok());
    // The ones this reader knows may be critical.
    assert!(HeldCertificate::read(certificate(Spec::new("CN=Grace", rsa_key(), mail_extensions())), IN_DATE).is_ok());
}

/// Review item 15: RSA below 2048 bits is neither listed nor used.
#[test]
fn a_weak_rsa_key_is_neither_listed_nor_used() {
    let weak = certificate(Spec::new("CN=Heidi", rsa_key_of(1024), mail_extensions()));
    let held = read(weak.clone());
    assert_eq!(held.key, KeyKind::Rsa { bits: 1024 });
    assert!(usable([weak], IN_DATE).is_empty());
    assert!(matches!(check_for_signing(&held, SignatureScheme::RsaPkcs1v15), Err(KeyHolderError::NotAllowed(_))));
    assert!(matches!(check_for_decrypting(&held), Err(KeyHolderError::NotAllowed(_))));
    let strong = read(certificate(Spec::new("CN=Heidi", rsa_key_of(MIN_RSA_BITS as usize), mail_extensions())));
    assert!(check_for_signing(&strong, SignatureScheme::RsaPkcs1v15).is_ok());
}

/// Review items 3 and 4: the listing rule applies again at use; expired certificates are listed
/// on every system, sign nothing, and still unwrap old mail.
#[test]
fn expired_certificates_are_listed_marked_and_only_decrypt() {
    let mut spec = Spec::new("CN=Ivan", rsa_key(), mail_extensions());
    spec.validity = ("200101000000Z", "210101000000Z");
    let expired = certificate(spec);
    let listed = usable([expired.clone()], IN_DATE);
    assert_eq!(listed.len(), 1);
    let held = listed.into_iter().next().unwrap();
    assert!(!held.in_date);
    assert_eq!(check_for_signing(&held, SignatureScheme::RsaPkcs1v15), Err(KeyHolderError::NotAllowed("it is expired or not valid yet")));
    assert_eq!(check_for_decrypting(&held), Ok(()));

    let mut spec = Spec::new("CN=Ivan", rsa_key(), mail_extensions());
    spec.validity = ("400101000000Z", "410101000000Z");
    let early = read(certificate(spec));
    assert!(!early.in_date);
    assert!(check_for_signing(&early, SignatureScheme::RsaPss).is_err());

    // A clock that can't say signs nothing.
    assert!(!HeldCertificate::read(alice_rsa(), None).unwrap().in_date);
    assert!(now().is_some());

    // Not for mail: refused at use as at listing, whatever the dates.
    let web = read(certificate(Spec::new("CN=www.example.org", rsa_key(), vec![eku(&[SERVER_AUTH])])));
    assert_eq!(check_for_signing(&web, SignatureScheme::RsaPkcs1v15), Err(KeyHolderError::NotAllowed("not a certificate for mail")));
    assert_eq!(check_for_decrypting(&web), Err(KeyHolderError::NotAllowed("not a certificate for mail")));

    // Usage and scheme.
    let sign_only = read(certificate(Spec::new("CN=Dan", rsa_key(), vec![key_usage(KeyUsages::DigitalSignature.into())])));
    assert!(matches!(check_for_decrypting(&sign_only), Err(KeyHolderError::NotAllowed(_))));
    assert!(matches!(check_for_signing(&sign_only, SignatureScheme::Ecdsa), Err(KeyHolderError::Unsupported(_))));
    let unwrap_only = read(certificate(Spec::new("CN=Dan", rsa_key(), vec![key_usage(KeyUsages::KeyEncipherment.into())])));
    assert!(matches!(check_for_signing(&unwrap_only, SignatureScheme::RsaPkcs1v15), Err(KeyHolderError::NotAllowed(_))));
}

/// Review item 16: the cap applies to what is listed, after the filter.
#[test]
fn a_store_full_of_other_certificates_still_shows_the_mail_ones() {
    let web = certificate(Spec::new("CN=www.example.org", rsa_key(), vec![eku(&[SERVER_AUTH])]));
    let mut found = vec![web; 300];
    found.push(alice_rsa());
    assert_eq!(usable(found, IN_DATE).len(), 1);

    let many: Vec<Vec<u8>> = (0..300).map(|i| certificate(Spec::new(&format!("CN=Person {i}"), rsa_key(), mail_extensions()))).collect();
    assert_eq!(usable(many, IN_DATE).len(), MAX_CERTIFICATES);
}

#[test]
fn hostile_certificates_never_panic() {
    let der = alice_rsa();
    // Every truncation, and every byte flipped in turn.
    for end in 0..der.len() {
        let _ = HeldCertificate::read(der.get(..end).unwrap().to_vec(), IN_DATE);
    }
    for i in 0..der.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut bad = der.clone();
            if let Some(b) = bad.get_mut(i) {
                *b ^= flip;
            }
            let _ = HeldCertificate::read(bad, IN_DATE);
        }
    }
    let _ = usable([Vec::new(), vec![0x30], vec![0x30, 0x84, 0xff, 0xff, 0xff, 0xff]], IN_DATE);
}

#[test]
fn oversized_inputs_are_refused_unread() {
    assert_eq!(HeldCertificate::read(vec![0x30; MAX_CERTIFICATE_LEN + 1], IN_DATE), Err(KeyHolderError::BadInput("the certificate is too large")));
    assert!(check_wrapped(&[]).is_err());
    assert!(check_wrapped(&[0; super::MAX_WRAPPED_KEY_LEN + 1]).is_err());
    assert!(check_wrapped(&[0; 256]).is_ok());
    assert!(check_p12(&[]).is_err());
    assert!(check_p12(&vec![0; super::MAX_P12_LEN + 1]).is_err());
    assert!(check_p12(&[0x30; 16]).is_ok());
}

#[test]
fn digest_info_is_rfc_8017s() {
    let digest = [0xab; 32];
    let info = digest_info(DigestAlgorithm::Sha256, &digest).unwrap();
    assert_eq!(hex(info.get(..19).unwrap()), "3031300d060960864801650304020105000420");
    assert_eq!(info.get(19..), Some(&digest[..]));
    assert_eq!(hex(digest_info(DigestAlgorithm::Sha384, &[0; 48]).unwrap().get(..19).unwrap()), "3041300d060960864801650304020205000430");
    assert_eq!(hex(digest_info(DigestAlgorithm::Sha512, &[0; 64]).unwrap().get(..19).unwrap()), "3051300d060960864801650304020305000440");
    assert!(digest_info(DigestAlgorithm::Sha256, &[0; 48]).is_err());
    assert_eq!(digest_info(DigestAlgorithm::Sha512, &[0; 64]).unwrap().len(), 83);
}

#[test]
fn digests_must_be_their_hashs_length() {
    assert!(DigestAlgorithm::Sha256.check(&[0; 32]).is_ok());
    assert!(DigestAlgorithm::Sha256.check(&[0; 20]).is_err());
    assert!(DigestAlgorithm::Sha384.check(&[0; 48]).is_ok());
    assert!(DigestAlgorithm::Sha512.check(&[]).is_err());
}

/// The two INTEGERs of a DER `Ecdsa-Sig-Value`, as minimal unsigned bytes.
fn ecdsa_parts(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
    use x509_cert::der::asn1::UintRef;
    let mut reader = SliceReader::new(der).unwrap();
    let parts = reader
        .sequence(|seq| Ok::<_, x509_cert::der::Error>((UintRef::decode(seq)?.as_bytes().to_vec(), UintRef::decode(seq)?.as_bytes().to_vec())))
        .unwrap();
    reader.finish().unwrap();
    parts
}

/// Review item 5: the signature's length must be its curve's.
#[test]
fn ecdsa_r_s_becomes_der_at_its_curves_length() {
    // r = 1, s with its top bit set: a leading zero for s, none for r.
    let mut raw = vec![0u8; 31];
    raw.push(0x01);
    raw.extend([0xff; 32]);
    let der = ecdsa_raw_to_der(&raw, KeyKind::EcP256).unwrap();
    assert_eq!(hex(der.get(..7).unwrap()), "30260201010221");
    assert_eq!(ecdsa_parts(&der), (vec![0x01], vec![0xff; 32]));

    // Zero is one zero byte.
    assert_eq!(hex(&ecdsa_raw_to_der(&[0u8; 64], KeyKind::EcP256).unwrap()), "3006020100020100");

    let der = ecdsa_raw_to_der(&[0x80; 96], KeyKind::EcP384).unwrap();
    assert_eq!(ecdsa_parts(&der), (vec![0x80; 48], vec![0x80; 48]));

    // Any other length, or a key that isn't P-256 or P-384, is refused.
    assert!(ecdsa_raw_to_der(&[0x80; 96], KeyKind::EcP256).is_err());
    assert!(ecdsa_raw_to_der(&[0x80; 64], KeyKind::EcP384).is_err());
    assert!(ecdsa_raw_to_der(&[], KeyKind::EcP256).is_err());
    assert!(ecdsa_raw_to_der(&[1; 63], KeyKind::EcP256).is_err());
    assert!(ecdsa_raw_to_der(&[1; 132], KeyKind::EcP384).is_err());
    assert!(matches!(ecdsa_raw_to_der(&[1; 64], KeyKind::Rsa { bits: 2048 }), Err(KeyHolderError::Unsupported(_))));
    assert!(ecdsa_raw_to_der(&[1; 64], KeyKind::Other).is_err());
}

#[test]
fn schemes_fit_their_keys() {
    let rsa = KeyKind::Rsa { bits: 2048 };
    assert!(SignatureScheme::RsaPkcs1v15.fits(rsa) && SignatureScheme::RsaPss.fits(rsa) && !SignatureScheme::Ecdsa.fits(rsa));
    assert!(SignatureScheme::Ecdsa.fits(KeyKind::EcP256) && SignatureScheme::Ecdsa.fits(KeyKind::EcP384));
    assert!(!SignatureScheme::RsaPss.fits(KeyKind::EcP256) && !SignatureScheme::Ecdsa.fits(KeyKind::Other));
    assert_ne!(RsaPadding::Pkcs1v15, RsaPadding::OaepSha256);
    assert_ne!(RsaPadding::OaepSha1, RsaPadding::OaepSha256);
}

fn example_token() -> TokenRef {
    TokenRef { slot: 3, serial: "0123456789".into(), label: "Example Card".into() }
}

#[test]
fn a_pin_and_an_unwrapped_key_are_never_shown_in_errors() {
    let pin = Pin::new("246813".to_string());
    assert_eq!(format!("{pin:?}"), "Pin(…)");
    assert_eq!(format!("{:?}", Some(&pin)), "Some(Pin(…))");
    assert_eq!(pin.expose(), "246813");
    let errors = [
        KeyHolderError::Unavailable("x".into()),
        KeyHolderError::NotFound,
        KeyHolderError::Unsupported("x"),
        KeyHolderError::NotAllowed("x"),
        KeyHolderError::BadInput("x"),
        KeyHolderError::PinNeeded(example_token()),
        KeyHolderError::TokenChanged,
        KeyHolderError::PinIncorrect,
        KeyHolderError::PinLocked,
        KeyHolderError::PinExpired,
        KeyHolderError::Cancelled,
        KeyHolderError::System("x".into()),
    ];
    for error in errors {
        assert!(!error.to_string().contains("246813"));
        assert!(!format!("{error:?}").contains("246813"));
    }
    assert!(KeyHolderError::PinNeeded(example_token()).to_string().contains("Example Card"));
    let key: UnwrappedKey = UnwrappedKey::new(vec![1, 2, 3]);
    assert_eq!(key.len(), 3);
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod pkcs11 {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use cryptoki::context::Function;
    use cryptoki::error::{Error, RvError};

    use super::super::pkcs11::{
        KeyFacts, KeyKindOnToken, PROXY_PLACES, Pkcs11Holder, TokenCertificate, check_same_token, first_that_opens, key_matches, module_from_env,
        open_default, shared, token_error,
    };
    use super::super::{DigestAlgorithm, KeyHolderError, RsaPadding, SignatureScheme, TokenRef};
    use super::{IN_DATE, P256, Spec, alice_rsa, certificate, ec_key, ec_point, example_token, mail_extensions, modulus, read};

    #[test]
    fn a_module_must_be_named_by_an_absolute_path_that_loads() {
        assert!(matches!(Pkcs11Holder::open(Path::new("p11-kit-proxy.so")), Err(KeyHolderError::Unavailable(_))));
        assert!(matches!(Pkcs11Holder::open(Path::new("/nonexistent/example/module.so")), Err(KeyHolderError::Unavailable(_))));
        // The test's environment names no module (a relative one would be ignored).
        if std::env::var_os("MAILCRAFT_PKCS11_MODULE").is_none() {
            assert_eq!(module_from_env(), None);
        }
        let _ = open_default();
    }

    /// Review item 1: one holder per module for the life of the program; a failure isn't kept.
    #[test]
    fn a_module_is_kept_once_and_never_let_go_of() {
        let registry: Mutex<Vec<(PathBuf, Arc<u32>)>> = Mutex::new(Vec::new());
        let first = shared(&registry, Path::new("/example/a.so"), 2, || Ok(1)).unwrap();
        let again = shared(&registry, Path::new("/example/a.so"), 2, || Ok(2)).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*again, 1);

        assert!(shared(&registry, Path::new("/example/b.so"), 2, || Err(KeyHolderError::Unavailable("no".into()))).is_err());
        assert_eq!(*shared(&registry, Path::new("/example/b.so"), 2, || Ok(3)).unwrap(), 3);
        assert!(shared(&registry, Path::new("/example/c.so"), 2, || Ok(4)).is_err());
        drop((first, again));
        // Still held by the registry after every caller let go.
        assert_eq!(registry.lock().unwrap().iter().map(|(_, h)| Arc::strong_count(h)).collect::<Vec<_>>(), vec![1, 1]);
    }

    /// Review item 6: the 64-bit places first, each candidate tried until one opens.
    #[test]
    fn proxy_places_are_tried_in_turn_64_bit_first() {
        let place = |p: &str| PROXY_PLACES.iter().position(|q| *q == p).unwrap();
        assert!(place("/usr/lib64/p11-kit-proxy.so") < place("/usr/lib/p11-kit-proxy.so"));
        assert!(place("/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so") < place("/usr/lib/p11-kit-proxy.so"));
        for triplet in ["aarch64-linux-gnu", "riscv64-linux-gnu", "powerpc64le-linux-gnu", "arm-linux-gnueabihf", "i386-linux-gnu"] {
            assert!(PROXY_PLACES.contains(&format!("/usr/lib/{triplet}/p11-kit-proxy.so").as_str()), "{triplet}");
        }
        assert_eq!(PROXY_PLACES.last(), Some(&"/usr/local/lib/p11-kit-proxy.so"));

        let mut tried = Vec::new();
        let opened = first_that_opens(["/a.so", "/b.so", "/c.so"].map(PathBuf::from), |p| {
            tried.push(p.to_path_buf());
            if p == Path::new("/b.so") { Ok("b") } else { Err(KeyHolderError::Unavailable("no".into())) }
        });
        assert_eq!(opened, Ok("b"));
        assert_eq!(tried, vec![PathBuf::from("/a.so"), PathBuf::from("/b.so")]);
        let none: Result<(), _> = first_that_opens(Vec::new(), |_| Ok(()));
        assert!(matches!(none, Err(KeyHolderError::Unavailable(_))));
        let last = first_that_opens([PathBuf::from("/a.so")], |_| -> Result<(), _> { Err(KeyHolderError::System("last".into())) });
        assert_eq!(last, Err(KeyHolderError::System("last".into())));
    }

    /// Review item 2: a key is the certificate's only when its public half is.
    #[test]
    fn a_token_key_must_be_the_certificates() {
        let rsa = read(alice_rsa());
        let facts = |kind, modulus: Option<Vec<u8>>, ec_point: Option<Vec<u8>>| KeyFacts { kind, modulus, ec_point };
        // The token's modulus may carry a leading zero.
        assert!(key_matches(&rsa, &facts(Some(KeyKindOnToken::Rsa), Some(modulus(2048)), None)));
        let mut other = modulus(2048);
        if let Some(last) = other.last_mut() {
            *last ^= 1;
        }
        assert!(!key_matches(&rsa, &facts(Some(KeyKindOnToken::Rsa), Some(other), None)));
        assert!(!key_matches(&rsa, &facts(Some(KeyKindOnToken::Rsa), None, None)));
        assert!(!key_matches(&rsa, &facts(Some(KeyKindOnToken::Ec), Some(modulus(2048)), None)));
        assert!(!key_matches(&rsa, &facts(None, Some(modulus(2048)), None)));

        let ec = read(certificate(Spec::new("CN=Bob", ec_key(P256), Vec::new())));
        let point = ec_point();
        let mut wrapped = vec![0x04, u8::try_from(point.len()).unwrap()];
        wrapped.extend(&point);
        assert!(key_matches(&ec, &facts(Some(KeyKindOnToken::Ec), None, Some(point.clone()))));
        assert!(key_matches(&ec, &facts(Some(KeyKindOnToken::Ec), None, Some(wrapped))));
        let mut moved = point.clone();
        if let Some(last) = moved.last_mut() {
            *last ^= 1;
        }
        assert!(!key_matches(&ec, &facts(Some(KeyKindOnToken::Ec), None, Some(moved))));
        assert!(!key_matches(&ec, &facts(Some(KeyKindOnToken::Rsa), None, Some(point))));
        assert!(!key_matches(&ec, &KeyFacts::default()));
    }

    /// Review items 7, 9 and 10: errors name the token, never loop, and a PIN goes only to the
    /// token it was asked for.
    #[test]
    fn token_errors_are_named_and_never_ask_twice() {
        let token = example_token();
        let error = |rv| token_error(Error::Pkcs11(rv, Function::Sign), &token, false);
        assert_eq!(error(RvError::PinIncorrect), KeyHolderError::PinIncorrect);
        assert_eq!(error(RvError::PinLocked), KeyHolderError::PinLocked);
        assert_eq!(error(RvError::PinExpired), KeyHolderError::PinExpired);
        assert_eq!(error(RvError::FunctionCanceled), KeyHolderError::Cancelled);
        assert_eq!(error(RvError::TokenNotPresent), KeyHolderError::TokenChanged);
        assert!(matches!(error(RvError::MechanismInvalid), KeyHolderError::Unsupported(_)));
        assert!(matches!(error(RvError::DeviceError), KeyHolderError::System(_)));

        // Not logged in: without a PIN, the window is asked for this token's; with one, it's over.
        assert_eq!(error(RvError::UserNotLoggedIn), KeyHolderError::PinNeeded(token.clone()));
        assert!(matches!(token_error(Error::Pkcs11(RvError::UserNotLoggedIn, Function::Sign), &token, true), KeyHolderError::Unsupported(_)));

        assert_eq!(check_same_token(None, &token), Ok(()));
        assert_eq!(check_same_token(Some(&token), &token), Ok(()));
        let moved = TokenRef { slot: 4, ..token.clone() };
        assert_eq!(check_same_token(Some(&moved), &token), Err(KeyHolderError::TokenChanged));
        let swapped = TokenRef { serial: "9999".into(), ..token.clone() };
        assert_eq!(check_same_token(Some(&swapped), &token), Err(KeyHolderError::TokenChanged));
        // The label alone may change (it's only shown).
        let relabelled = TokenRef { label: "Renamed".into(), ..token.clone() };
        assert_eq!(check_same_token(Some(&relabelled), &token), Ok(()));
    }

    /// Opt-in: `MAILCRAFT_PKCS11_TEST_MODULE=/usr/lib/softhsm/libsofthsm2.so cargo test -p mailcraft
    /// -- --ignored key_holders` lists what the named module's tokens hold. Use a test token
    /// (SoftHSM2, or p11-kit's trust module, which has no keys), never a card in daily use.
    #[test]
    #[ignore = "needs a PKCS#11 module named in MAILCRAFT_PKCS11_TEST_MODULE"]
    fn a_named_module_lists_its_certificates() {
        let Some(module) = std::env::var_os("MAILCRAFT_PKCS11_TEST_MODULE") else { return };
        let holder = Pkcs11Holder::open(Path::new(&module)).unwrap();
        // The same module twice is the same holder: loaded and initialised once.
        assert!(Arc::ptr_eq(&holder, &Pkcs11Holder::open(Path::new(&module)).unwrap()));
        let listed: Vec<TokenCertificate> = holder.certificates().unwrap();
        for held in &listed {
            assert!(held.certificate.for_mail);
            let _ = (&held.token, held.protected_path, held.login_required);
        }
        // An unknown fingerprint is not found, and asks for no PIN.
        let missing = holder.sign_digest(&[0; 32], SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256, &[0; 32], None);
        assert_eq!(missing, Err(KeyHolderError::NotFound));
        let missing = holder.decrypt_key(&[0; 32], RsaPadding::Pkcs1v15, &[0; 256], None);
        assert_eq!(missing.map(|_| ()), Err(KeyHolderError::NotFound));
        // A PIN for a slot that isn't there goes nowhere.
        let pin = super::super::Pin::new("000000".into());
        let gone = TokenRef { slot: u64::MAX, serial: "none".into(), label: "none".into() };
        let refused = holder.sign_digest(&[0; 32], SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256, &[0; 32], Some((&gone, &pin)));
        assert_eq!(refused, Err(KeyHolderError::TokenChanged));
        let _ = (IN_DATE, mail_extensions());
    }
}
