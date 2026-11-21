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

/// Re-review P3 1: the policy and name-constraint extensions bind whoever checks the chain, so a
/// critical one doesn't stop the owner's certificate from being listed.
#[test]
fn critical_policy_extensions_are_known() {
    // certificatePolicies: one policy, anyPolicy.
    let policies = Extension {
        extn_id: oid("2.5.29.32"),
        critical: true,
        extn_value: OctetString::new(vec![0x30, 0x06, 0x30, 0x04, 0x06, 0x02, 0x55, 0x1d]).unwrap(),
    };
    let mut extensions = mail_extensions();
    extensions.push(policies);
    assert_eq!(usable([certificate(Spec::new("CN=Judy", rsa_key(), extensions))], IN_DATE).len(), 1);
    for id in ["2.5.29.36", "2.5.29.54", "2.5.29.30"] {
        let critical = Extension { extn_id: oid(id), critical: true, extn_value: OctetString::new(vec![0x30, 0x00]).unwrap() };
        assert!(HeldCertificate::read(certificate(Spec::new("CN=Judy", rsa_key(), vec![critical])), IN_DATE).is_ok(), "{id}");
    }
}

/// `source` with Windows line endings made plain ones: a checkout on Windows may have `\r\n`.
fn lf(source: &str) -> String {
    source.replace("\r\n", "\n")
}

/// The lines of the body of `pub fn name` in `source`, to its closing brace at the start of a
/// line, without comment lines.
fn body_of(source: &str, name: &str) -> Vec<String> {
    let start = source.find(&format!("pub fn {name}(")).unwrap_or_else(|| panic!("no pub fn {name}"));
    let rest = source.get(start..).unwrap();
    let body = rest.get(..rest.find("\n}\n").unwrap()).unwrap();
    body.lines().skip(1).filter(|line| !line.trim_start().starts_with("//")).map(str::to_string).collect()
}

/// Where `statement` is in `body` as a statement of the function's own (four spaces in: not in
/// a closure or a block), before the first line that has `use_of`.
fn checked_before(body: &[String], statement: &str, use_of: &str) -> Result<(), String> {
    let at = body.iter().position(|line| line == &format!("    {statement}")).ok_or(format!("no `{statement}` of its own"))?;
    let used = body.iter().position(|line| line.contains(use_of)).ok_or(format!("no `{use_of}`"))?;
    if at < used { Ok(()) } else { Err(format!("`{statement}` comes after `{use_of}`")) }
}

/// Whether each use in a system store's source checks first: the check a statement of the
/// function's own, before the key is opened.
fn store_checks(source: &str, opens_key: &str) -> Result<(), String> {
    let sign = body_of(source, "sign_digest");
    checked_before(&sign, "check_for_signing(&held, scheme)?;", opens_key).map_err(|e| format!("sign_digest: {e}"))?;
    let decrypt = body_of(source, "decrypt_key");
    checked_before(&decrypt, "let wrapped = check_wrapped(wrapped)?;", opens_key).map_err(|e| format!("decrypt_key: {e}"))?;
    checked_before(&decrypt, "check_for_decrypting(&held)?;", opens_key).map_err(|e| format!("decrypt_key: {e}"))
}

/// Re-review P1 (b) and round 3 P3 1: the Windows and macOS stores can't run here, so their
/// source is read: each use applies the same check as the listing, and an unwrap checks its
/// input, before the key is opened.
#[test]
fn the_system_stores_check_before_each_use() {
    assert_eq!(store_checks(&lf(include_str!("windows_store.rs")), "open_key("), Ok(()), "windows_store.rs");
    assert_eq!(store_checks(&lf(include_str!("macos_keychain.rs")), "private_key()"), Ok(()), "macos_keychain.rs");

    // What the check would have missed before: a check commented out, moved after the key is
    // used, or put in a closure that never runs.
    let windows = lf(include_str!("windows_store.rs"));
    let check = "    check_for_signing(&held, scheme)?;\n";
    let commented = windows.replacen(check, "    // check_for_signing(&held, scheme)?;\n", 1);
    assert!(store_checks(&commented, "open_key(").is_err());
    let used = "    let signature = bytes(&CryptographicEngine::SignHashedData(&key, &data).map_err(system)?)?;\n";
    let moved = windows.replacen(check, "", 1).replacen(used, &format!("{used}{check}"), 1);
    assert_ne!(moved, windows.replacen(check, "", 1), "the test's own edit took");
    assert!(store_checks(&moved, "open_key(").is_err());
    let closure = windows.replacen(
        check,
        "    let _later = || -> Result<(), KeyHolderError> {\n        check_for_signing(&held, scheme)?;\n        Ok(())\n    };\n",
        1,
    );
    assert!(store_checks(&closure, "open_key(").is_err());
    let one_line = windows.replacen(check, "    let _later = || check_for_signing(&held, scheme);\n", 1);
    assert!(store_checks(&one_line, "open_key(").is_err());
    let mac = lf(include_str!("macos_keychain.rs"));
    assert!(store_checks(&mac.replacen("    check_for_decrypting(&held)?;\n", "", 1), "private_key()").is_err());
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod pkcs11 {
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use cryptoki::context::Function;
    use cryptoki::error::{Error, RvError};
    use cryptoki::mechanism::Mechanism;
    use cryptoki::object::ObjectClass;

    use super::super::pkcs11::{
        KeyFacts, KeyKindOnToken, MODULE_VARIABLE, Module, PROXY_PLACES, Pkcs11Holder, Registry, TokenCertificate, TokenFacts, TokenSession,
        check_same_token, choose_module, decrypt_key, first_that_opens, key_matches, list, module_key, open_default, shared, sign_digest,
        token_error,
    };
    use x509_cert::der::Decode;
    use x509_cert::spki::SubjectPublicKeyInfoOwned;

    use super::super::encoding::ecdsa_raw_to_der;
    use super::super::pkcs11::signed_by;
    use super::super::{DigestAlgorithm, HeldCertificate, KeyHolderError, Pin, RsaPadding, SignatureScheme, TokenRef};
    use super::{EC, P256, SERVER_AUTH, Spec, alice_rsa, certificate, ec_key, ec_point, eku, example_token, mail_extensions, modulus, read, rsa_key};

    // ---- A fake token ---------------------------------------------------------------------

    #[derive(Clone, Default)]
    struct FakeKey {
        id: Vec<u8>,
        public: bool,
        facts: KeyFacts,
        always: bool,
    }

    #[derive(Clone)]
    struct FakeToken {
        facts: TokenFacts,
        certificates: Vec<(Vec<u8>, Vec<u8>)>,
        keys: Vec<FakeKey>,
        pin: &'static str,
        /// The token wants a login at every use (a key it doesn't mark as such).
        login_at_every_use: bool,
        /// What the token's key signs anything to.
        signature: Vec<u8>,
    }

    struct FakeModule {
        tokens: Vec<FakeToken>,
        log: Rc<RefCell<Vec<String>>>,
    }

    struct FakeSession {
        token: FakeToken,
        log: Rc<RefCell<Vec<String>>>,
        logged_in: Cell<bool>,
    }

    impl Module for FakeModule {
        type Session = FakeSession;
        fn slots(&self) -> Result<Vec<u64>, KeyHolderError> {
            Ok(self.tokens.iter().map(|t| t.facts.token.slot).collect())
        }
        fn open(&self, slot: u64) -> Result<FakeSession, KeyHolderError> {
            let token = self.tokens.iter().find(|t| t.facts.token.slot == slot).cloned().ok_or(KeyHolderError::TokenChanged)?;
            Ok(FakeSession { token, log: Rc::clone(&self.log), logged_in: Cell::new(false) })
        }
    }

    impl FakeSession {
        fn note(&self, what: String) {
            self.log.borrow_mut().push(what);
        }
        fn use_key(&self, what: &str, key: usize) -> Result<(), Error> {
            self.note(format!("{what} slot {} key {key}", self.token.facts.token.slot));
            let logged_in = self.logged_in.get();
            if (self.token.facts.login_required && !logged_in) || self.token.login_at_every_use {
                return Err(Error::Pkcs11(RvError::UserNotLoggedIn, Function::Sign));
            }
            Ok(())
        }
    }

    impl TokenSession for FakeSession {
        type Key = usize;
        fn facts(&self) -> Result<TokenFacts, KeyHolderError> {
            Ok(self.token.facts.clone())
        }
        fn certificates(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
            self.token.certificates.clone()
        }
        fn keys(&self, class: ObjectClass, id: &[u8]) -> Vec<usize> {
            let public = class == ObjectClass::PUBLIC_KEY;
            self.token.keys.iter().enumerate().filter(|(_, k)| k.public == public && k.id == id).map(|(i, _)| i).collect()
        }
        fn key_facts(&self, key: usize) -> KeyFacts {
            self.token.keys.get(key).map(|k| k.facts.clone()).unwrap_or_default()
        }
        fn always_authenticate(&self, key: usize) -> bool {
            self.token.keys.get(key).is_some_and(|k| k.always)
        }
        fn login(&self, pin: Option<&Pin>) -> Result<(), Error> {
            let slot = self.token.facts.token.slot;
            match pin {
                None if self.token.facts.protected_path => self.note(format!("pad login slot {slot}")),
                None => return Err(Error::Pkcs11(RvError::ArgumentsBad, Function::Login)),
                Some(pin) => {
                    self.note(format!("pin {} slot {slot}", pin.expose()));
                    if pin.expose() != self.token.pin {
                        return Err(Error::Pkcs11(RvError::PinIncorrect, Function::Login));
                    }
                }
            }
            self.logged_in.set(true);
            Ok(())
        }
        fn sign(&self, _: &Mechanism<'_>, key: usize, _: &[u8]) -> Result<Vec<u8>, Error> {
            self.use_key("sign", key)?;
            Ok(self.token.signature.clone())
        }
        fn decrypt(&self, _: &Mechanism<'_>, key: usize, _: &[u8]) -> Result<Vec<u8>, Error> {
            self.use_key("decrypt", key)?;
            Ok(vec![0x07; 32])
        }
    }

    const ID: &[u8] = &[0x01];

    /// A certificate for mail, valid for decades (the fake reads it with the real clock).
    fn long_lived(subject: &str) -> Vec<u8> {
        let mut spec = Spec::new(subject, rsa_key(), mail_extensions());
        spec.validity = ("200101000000Z", "491231235959Z");
        certificate(spec)
    }

    fn rsa_private(modulus_on_token: Option<Vec<u8>>, kind: Option<KeyKindOnToken>) -> FakeKey {
        FakeKey { id: ID.to_vec(), public: false, facts: KeyFacts { kind, modulus: modulus_on_token, ec_point: None }, always: false }
    }

    fn the_key() -> FakeKey {
        rsa_private(Some(modulus(2048)), Some(KeyKindOnToken::Rsa))
    }

    fn other_key() -> FakeKey {
        let mut other = modulus(2048);
        if let Some(last) = other.last_mut() {
            *last ^= 1;
        }
        rsa_private(Some(other), Some(KeyKindOnToken::Rsa))
    }

    fn token(slot: u64, serial: &str, der: &[u8], keys: Vec<FakeKey>) -> FakeToken {
        FakeToken {
            facts: TokenFacts {
                token: TokenRef { slot, serial: serial.into(), label: format!("Card {slot}") },
                protected_path: false,
                login_required: true,
            },
            certificates: vec![(der.to_vec(), ID.to_vec())],
            keys,
            pin: "246813",
            login_at_every_use: false,
            signature: vec![0x5a; 256],
        }
    }

    fn module(tokens: Vec<FakeToken>) -> FakeModule {
        FakeModule { tokens, log: Rc::default() }
    }

    fn log(module: &FakeModule) -> Vec<String> {
        module.log.borrow().clone()
    }

    fn fingerprint(der: &[u8]) -> [u8; 32] {
        super::super::certificate::sha256(der)
    }

    fn sign(module: &FakeModule, der: &[u8], pin: Option<(&TokenRef, &Pin)>) -> Result<Vec<u8>, KeyHolderError> {
        sign_digest(module, &fingerprint(der), SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256, &[0; 32], pin)
    }

    fn pin() -> Pin {
        Pin::new("246813".into())
    }

    // ---- The logic, end to end on the fake --------------------------------------------------

    /// Re-review P1: the first call asks for the token's PIN, the second signs with it.
    #[test]
    fn a_pin_is_asked_for_once_and_then_signs() {
        let der = long_lived("CN=Alice Example");
        let fake = module(vec![token(3, "0123", &der, vec![the_key()])]);
        let asked = sign(&fake, &der, None);
        let Err(KeyHolderError::PinNeeded(named)) = asked else { panic!("{asked:?}") };
        assert_eq!(named.slot, 3);
        assert!(log(&fake).is_empty());
        assert_eq!(sign(&fake, &der, Some((&named, &pin()))), Ok(vec![0x5a; 256]));
        assert_eq!(log(&fake), vec!["pin 246813 slot 3".to_string(), "sign slot 3 key 0".to_string()]);
        let wrong = Pin::new("000000".into());
        assert_eq!(sign(&fake, &der, Some((&named, &wrong))), Err(KeyHolderError::PinIncorrect));
    }

    /// Re-review P1 (find_key): a key whose public half isn't the certificate's is skipped, at
    /// use and at listing.
    #[test]
    fn a_certificate_without_an_id_is_never_listed() {
        let der = long_lived("CN=Alice Example");
        let mut card = token(3, "0123", &der, vec![the_key()]);
        assert_eq!(list(&module(vec![card.clone()]), super::IN_DATE).unwrap().len(), 1);
        // On a token that needs a login, a certificate with an empty id would ask for a PIN it
        // can't use.
        card.certificates = vec![(der, Vec::new())];
        let mut no_id = card.clone();
        no_id.keys = vec![FakeKey { id: Vec::new(), ..the_key() }];
        assert!(list(&module(vec![card.clone()]), super::IN_DATE).unwrap().is_empty());
        assert!(list(&module(vec![no_id]), super::IN_DATE).unwrap().is_empty());
        card.facts.login_required = false;
        assert!(list(&module(vec![card]), super::IN_DATE).unwrap().is_empty());
    }

    #[test]
    fn a_key_that_isnt_the_certificates_is_never_used() {
        let der = long_lived("CN=Alice Example");
        let fake = module(vec![token(3, "0123", &der, vec![other_key(), the_key()])]);
        let named = fake.tokens[0].facts.token.clone();
        assert_eq!(sign(&fake, &der, Some((&named, &pin()))), Ok(vec![0x5a; 256]));
        assert_eq!(log(&fake).last().map(String::as_str), Some("sign slot 3 key 1"));

        let fake = module(vec![token(3, "0123", &der, vec![other_key()])]);
        assert_eq!(sign(&fake, &der, Some((&named, &pin()))), Err(KeyHolderError::NotFound));
        assert!(!log(&fake).iter().any(|l| l.starts_with("sign")));

        // Listing a token that shows its keys without a login: only a certificate with its key.
        let mut open = token(3, "0123", &der, vec![other_key()]);
        open.facts.login_required = false;
        assert!(list(&module(vec![open.clone()]), super::IN_DATE).unwrap().is_empty());
        open.keys.push(the_key());
        let listed: Vec<TokenCertificate> = list(&module(vec![open]), super::IN_DATE).unwrap();
        assert_eq!(listed.len(), 1);
    }

    /// Re-review P1 (key_for) and P3 2: a PIN goes only to the slot it was asked for, only while
    /// the session shows the token with that serial number, and a token without one by its
    /// label.
    #[test]
    fn a_pin_goes_only_to_the_token_it_was_asked_for() {
        let der = long_lived("CN=Alice Example");
        // The same certificate on slots 1 and 3; the PIN was asked for slot 3.
        let fake = module(vec![token(1, "AAAA", &der, vec![the_key()]), token(3, "0123", &der, vec![the_key()])]);
        let slot3 = fake.tokens[1].facts.token.clone();
        assert_eq!(sign(&fake, &der, Some((&slot3, &pin()))), Ok(vec![0x5a; 256]));
        assert!(log(&fake).iter().all(|l| l.ends_with("slot 3") || l.contains("slot 3 ")), "{:?}", log(&fake));

        // Another card in slot 3 now.
        let fake = module(vec![token(3, "9999", &der, vec![the_key()])]);
        assert_eq!(sign(&fake, &der, Some((&slot3, &pin()))), Err(KeyHolderError::TokenChanged));
        assert!(log(&fake).is_empty());

        // The certificate is no longer in slot 3.
        let fake = module(vec![token(3, "0123", &long_lived("CN=Someone Else"), vec![the_key()])]);
        assert_eq!(sign(&fake, &der, Some((&slot3, &pin()))), Err(KeyHolderError::TokenChanged));
        assert!(log(&fake).is_empty());

        // Two tokens without a serial number are not the same token.
        let fake = module(vec![token(3, "", &der, vec![the_key()])]);
        let blank = TokenRef { slot: 3, serial: String::new(), label: "Another Card".into() };
        assert_eq!(sign(&fake, &der, Some((&blank, &pin()))), Err(KeyHolderError::TokenChanged));
        assert!(log(&fake).is_empty());
        let same = fake.tokens[0].facts.token.clone();
        assert_eq!(sign(&fake, &der, Some((&same, &pin()))), Ok(vec![0x5a; 256]));
    }

    /// Re-review P1 (check_for_signing / check_for_decrypting / check_wrapped): the listing's
    /// rule again at use, before any login.
    #[test]
    fn a_certificate_the_rule_refuses_is_refused_before_the_pin() {
        let mut spec = Spec::new("CN=Ivan", rsa_key(), mail_extensions());
        spec.validity = ("200101000000Z", "210101000000Z");
        let expired = certificate(spec);
        let fake = module(vec![token(3, "0123", &expired, vec![the_key()])]);
        let named = fake.tokens[0].facts.token.clone();
        assert_eq!(sign(&fake, &expired, None), Err(KeyHolderError::NotAllowed("it is expired or not valid yet")));
        assert_eq!(sign(&fake, &expired, Some((&named, &pin()))), Err(KeyHolderError::NotAllowed("it is expired or not valid yet")));
        assert!(log(&fake).is_empty());
        // An expired certificate's key still unwraps old mail.
        let unwrapped = decrypt_key(&fake, &fingerprint(&expired), RsaPadding::Pkcs1v15, &[0; 256], Some((&named, &pin())));
        assert_eq!(unwrapped.map(|k| k.to_vec()), Ok(vec![0x07; 32]));

        let mut spec = Spec::new("CN=www.example.org", rsa_key(), vec![eku(&[SERVER_AUTH])]);
        spec.validity = ("200101000000Z", "491231235959Z");
        let web = certificate(spec);
        let fake = module(vec![token(3, "0123", &web, vec![the_key()])]);
        assert_eq!(sign(&fake, &web, Some((&named, &pin()))), Err(KeyHolderError::NotAllowed("not a certificate for mail")));
        let refused = decrypt_key(&fake, &fingerprint(&web), RsaPadding::Pkcs1v15, &[0; 256], Some((&named, &pin())));
        assert_eq!(refused.map(|_| ()), Err(KeyHolderError::NotAllowed("not a certificate for mail")));
        // The wrong scheme for the key.
        let der = long_lived("CN=Alice Example");
        let fake = module(vec![token(3, "0123", &der, vec![the_key()])]);
        let ecdsa = sign_digest(&fake, &fingerprint(&der), SignatureScheme::Ecdsa, DigestAlgorithm::Sha256, &[0; 32], Some((&named, &pin())));
        assert!(matches!(ecdsa, Err(KeyHolderError::Unsupported(_))));
        // A wrapped key that can't be one is refused unread.
        let empty = decrypt_key(&fake, &fingerprint(&der), RsaPadding::Pkcs1v15, &[], Some((&named, &pin())));
        assert!(matches!(empty, Err(KeyHolderError::BadInput(_))));
        assert!(log(&fake).is_empty());
    }

    /// Re-review P3 3: "not logged in" never has the window ask twice, nor a PIN pad's owner at
    /// all.
    #[test]
    fn not_logged_in_asks_for_a_pin_only_when_one_could_help() {
        let der = long_lived("CN=Alice Example");
        // A PIN pad whose key wants a login at every use.
        let mut pad = token(3, "0123", &der, vec![the_key()]);
        pad.facts.protected_path = true;
        pad.login_at_every_use = true;
        let fake = module(vec![pad.clone()]);
        assert!(matches!(sign(&fake, &der, None), Err(KeyHolderError::Unsupported(_))));
        assert_eq!(log(&fake).first().map(String::as_str), Some("pad login slot 3"));
        // A PIN pad that said it needs no login, then wants one.
        pad.facts.login_required = false;
        let fake = module(vec![pad]);
        assert!(matches!(sign(&fake, &der, None), Err(KeyHolderError::Unsupported(_))));
        assert!(!log(&fake).iter().any(|l| l.contains("login")));

        // A PIN given and taken, then "not logged in": over, not asked again.
        let mut card = token(3, "0123", &der, vec![the_key()]);
        card.login_at_every_use = true;
        let fake = module(vec![card.clone()]);
        let named = card.facts.token.clone();
        assert!(matches!(sign(&fake, &der, Some((&named, &pin()))), Err(KeyHolderError::Unsupported(_))));
        // A token that said it needs no login and wants one: asked once, for this token.
        card.facts.login_required = false;
        let fake = module(vec![card]);
        assert_eq!(sign(&fake, &der, None), Err(KeyHolderError::PinNeeded(named)));
        // A key marked as wanting a login at every use is refused before anything is asked.
        let mut marked = token(3, "0123", &der, vec![FakeKey { always: true, ..the_key() }]);
        marked.facts.protected_path = true;
        assert!(matches!(sign(&module(vec![marked]), &der, None), Err(KeyHolderError::Unsupported(_))));
    }

    /// Re-review P3 4: the public half is read from the public-key object when the private key
    /// doesn't say it, for RSA as for EC, and a key with no type is known by what it shows.
    #[test]
    fn a_keys_public_half_may_come_from_its_public_object() {
        let der = long_lived("CN=Alice Example");
        let named = |fake: &FakeModule| fake.tokens[0].facts.token.clone();
        let public = |facts| FakeKey { id: ID.to_vec(), public: true, facts, always: false };

        // RSA: no modulus on the private key; the public key has it. The key is found and used,
        // and what it signs is then checked (the fake's filler isn't the certificate's).
        let not_its = Err(KeyHolderError::NotAllowed("the token signed with a key that isn't the certificate's"));
        let modulus_object = public(KeyFacts { kind: Some(KeyKindOnToken::Rsa), modulus: Some(modulus(2048)), ec_point: None });
        let fake = module(vec![token(3, "0123", &der, vec![rsa_private(None, Some(KeyKindOnToken::Rsa)), modulus_object.clone()])]);
        assert_eq!(sign(&fake, &der, Some((&named(&fake), &pin()))), not_its);
        assert_eq!(log(&fake).last().map(String::as_str), Some("sign slot 3 key 0"));
        // Nor any type.
        let fake = module(vec![token(3, "0123", &der, vec![rsa_private(None, None), modulus_object])]);
        assert_eq!(sign(&fake, &der, Some((&named(&fake), &pin()))), not_its);
        assert_eq!(log(&fake).last().map(String::as_str), Some("sign slot 3 key 0"));
        // No type, its own modulus.
        let fake = module(vec![token(3, "0123", &der, vec![rsa_private(Some(modulus(2048)), None)])]);
        assert_eq!(sign(&fake, &der, Some((&named(&fake), &pin()))), Ok(vec![0x5a; 256]));
        // The public object's modulus is another key's.
        let wrong = public(KeyFacts { kind: Some(KeyKindOnToken::Rsa), modulus: other_key().facts.modulus, ec_point: None });
        let fake = module(vec![token(3, "0123", &der, vec![rsa_private(None, None), wrong])]);
        assert_eq!(sign(&fake, &der, Some((&named(&fake), &pin()))), Err(KeyHolderError::NotFound));

        // EC: the point on the public object.
        let mut spec = Spec::new("CN=Bob", ec_key(P256), mail_extensions());
        spec.validity = ("200101000000Z", "491231235959Z");
        let ec = certificate(spec);
        let private = FakeKey { id: ID.to_vec(), public: false, facts: KeyFacts::default(), always: false };
        let point = public(KeyFacts { kind: None, modulus: None, ec_point: Some(ec_point()) });
        let fake = module(vec![token(3, "0123", &ec, vec![private, point])]);
        let signed = sign_digest(&fake, &fingerprint(&ec), SignatureScheme::Ecdsa, DigestAlgorithm::Sha256, &[0; 32], Some((&named(&fake), &pin())));
        // The fake's 256 bytes aren't the certificate's signature: the key was found and used.
        assert_eq!(signed, not_its);
        assert_eq!(log(&fake).last().map(String::as_str), Some("sign slot 3 key 0"));
        let _ = EC;
    }

    // Round 3 P3 2: synthetic keys made with OpenSSL for these tests, and their signatures of
    // the SHA-256 (and SHA-384) of "MailCraft test vector".
    const DIGEST_256: &str = "173ad5d5c3b26001033e2774e682a79ef77b7e9b200f210c70d406857eb37130";
    const DIGEST_384: &str = "4db194dc7d0dde2aa9183f3f514bdb8c0f4b1456cab3b5b3afb549fd1820ca5883c341d66b5a708febc90ad52e6d2d24";
    const RSA_SPKI: &str = "30820122300d06092a864886f70d01010105000382010f003082010a02820101009923a05fe685f7a3e0bc3c8a79b109d81b364fdedec495c83d9b1f5ee4b00dc8e0e812e4543440e8b521ab853d7be920f42441ca6bb14d4efe627e8d6b1426e9debbf05efcc5b9aced5aa35959d92e00b3490b408158f7bd163f429e79e8eee4a555615b902cd2040266da52599f473063a7f71e06a50d7b32bcfe4fe5ea96e8f90f0e75bc07d2eca8fa2cd6f815afe5cc9cc0c41c7d0c01d8915c7ac433fd9a283d0926f9cd963cae3829ef0cd7104ff346310ad8ed2f9da1d41897b0718a195a88b29684f99244467ddfe8d83011868a342596282efcbd9e1e231a6f97fbb67dbde4914d06c76f731ec65827786569b0df9e250f4dff346b48d70064304a0f0203010001";
    const RSA_PKCS1: &str = "3edf67f9f0edaaa5215c25d2acd03e408a68e32c2860246e4c0f2b402c0b66e343d4ccb525a0973abb7c3660eea9e781c4b2c7b0a571ce0fe203bb5b63af538a03289903c41aeab59ae75b2e6dc6a72a0fa38b260f15e6420f8c6e5ceff24ca3d657aeb1aa4c74ed595acd7c403ee0a8baa5546dbd41da7d2248ccad67343c1f323b8a607b135b680ee67932f4fbcc5510823f320587e5b6a07ed935131ff15f04a12442556e63c8588e2f3e1b4bbc6cfb75cf5ad61c56969427578dfac5eee16a7049f0a7d7bec212d60ea76c0b41004381170412a768ff5bc8a62b346603249a7d8db5f165319d3b917a96d0cd8cf39a987331f761caff2edd90eb8d908ac4";
    const RSA_PSS: &str = "092374646bd5ab25811dd2c957f624e59dc50c421fd08b2ff04583e09823def0ceb815054ed7b39ad82229ac5fb7e2a500ecaa398981ace25c2cef41aa5b8feb51bc0c4395aa23dcffcefe99fc76bd931c12da75e04aee3594ef239aaf6a58d194ec7b1e8eb2bb6d87182ce152e10bff94b4f5ebc9ff9d0dfa8e9f9b6282ade4a47a75cb87a1e27445c85921622cc2f080d14e0c8b48be1de5b385a7f3819c02713311b5c73b02ee1788f5b0efc9cf89e27428878294feb6348dc017806e4cfae11c6ae2b8425c4ee77640a7dce6e54afa3650611ed3675403c97f2dade35e5d311713bdb4e47afa43699ee43bb6d04164e1babe8b38bd776a039b86fce2f932";
    const P256_SPKI: &str = "3059301306072a8648ce3d020106082a8648ce3d03010703420004bfb8dba0cc93640e5fe0577b7857de542026519556d69d8f96e3c778d01ae19ef2076fbbc2330d6ebe043d06642235e01922a85de081715426292fd254b511bd";
    const P256_SIG: &str =
        "9021b9a7a31384e635a4270123a0ebe1cfaeaacbfc142c9d023a025aa82de2afcc86ad3e9354393998d65df3aa61441e102df7efa9c1bb3bc02452be3de44595";
    /// The P-256 key's signature of the SHA-384 digest (the digest cut to the curve's length).
    const P256_SHA384_SIG: &str =
        "77c079d101525ff2213fdc9d7a215b6a32692a52877518c77e5f024639dc6b330434cefd029e67890246b877ca1508dce659e78aaff7102171588a7b7986c392";
    const P384_SPKI: &str = "3076301006072a8648ce3d020106052b8104002203620004aa45843a107d00a658b8135282583c5142e8272aa5c07349511c9e9a002f30f4021a578d4edaaf3816cc4b626cc15a986d585332fe23eaf02b23e0e4e34105b3aeee7c8a1d50c96818cca2b89465f454bcd857e5e0d1ba79cc5f159167d957db";
    const P384_SIG: &str = "75c34ce17d395c9bbbdd280f3ddf60d6478ef04f271628abcef78458f8c4a0b5f0fddcc33ee6c960bb4e03d2c8ce622d30ae3dee915063d47524acb8714e0bf689bafbeb27f4d5922b6cad4e80a678335c9044e55d19050bbffa9294cbd41549";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap(), 16).unwrap()).collect()
    }

    fn flipped(text: &str) -> Vec<u8> {
        let mut bytes = unhex(text);
        if let Some(last) = bytes.last_mut() {
            *last ^= 1;
        }
        bytes
    }

    /// A long-lived certificate for mail with the synthetic key `spki`.
    fn real(spki: &str) -> HeldCertificate {
        let mut spec = Spec::new("CN=Alice Example", SubjectPublicKeyInfoOwned::from_der(&unhex(spki)).unwrap(), mail_extensions());
        spec.validity = ("200101000000Z", "491231235959Z");
        read(certificate(spec))
    }

    #[test]
    fn signatures_are_checked_against_the_certificates_key() {
        let rsa = real(RSA_SPKI);
        let (p256, p384) = (real(P256_SPKI), real(P384_SPKI));
        let (d256, d384) = (unhex(DIGEST_256), unhex(DIGEST_384));
        let (sha256, sha384) = (DigestAlgorithm::Sha256, DigestAlgorithm::Sha384);
        let (pkcs1, pss, ecdsa) = (SignatureScheme::RsaPkcs1v15, SignatureScheme::RsaPss, SignatureScheme::Ecdsa);
        assert!(signed_by(&rsa, pkcs1, sha256, &d256, &unhex(RSA_PKCS1)));
        assert!(signed_by(&rsa, pss, sha256, &d256, &unhex(RSA_PSS)));
        assert!(signed_by(&p256, ecdsa, sha256, &d256, &unhex(P256_SIG)));
        assert!(signed_by(&p256, ecdsa, sha384, &d384, &unhex(P256_SHA384_SIG)));
        assert!(signed_by(&p384, ecdsa, sha384, &d384, &unhex(P384_SIG)));

        // A byte off, another digest, another scheme, another key, the wrong length: no.
        assert!(!signed_by(&rsa, pkcs1, sha256, &d256, &flipped(RSA_PKCS1)));
        assert!(!signed_by(&rsa, pss, sha256, &d256, &flipped(RSA_PSS)));
        assert!(!signed_by(&rsa, pss, sha256, &d256, &unhex(RSA_PKCS1)));
        assert!(!signed_by(&rsa, pkcs1, sha256, &d256, &unhex(RSA_PSS)));
        assert!(!signed_by(&rsa, pkcs1, sha256, &flipped(DIGEST_256), &unhex(RSA_PKCS1)));
        assert!(!signed_by(&rsa, pkcs1, sha256, &d256, &[]));
        assert!(!signed_by(&p256, ecdsa, sha256, &d256, &flipped(P256_SIG)));
        assert!(!signed_by(&p256, ecdsa, sha256, &flipped(DIGEST_256), &unhex(P256_SIG)));
        assert!(!signed_by(&p256, ecdsa, sha256, &d256, &unhex(P256_SHA384_SIG)));
        assert!(!signed_by(&p384, ecdsa, sha384, &d384, &flipped(P384_SIG)));
        assert!(!signed_by(&p384, ecdsa, sha256, &d256, &unhex(P256_SIG)));
        assert!(!signed_by(&p256, ecdsa, sha256, &d256, &[0; 64]));
        assert!(!signed_by(&p256, pkcs1, sha256, &d256, &unhex(P256_SIG)));
        assert!(!signed_by(&rsa, ecdsa, sha256, &d256, &unhex(RSA_PKCS1)));
        // A certificate's point that isn't on the curve.
        let mut off = p256.clone();
        if let Some(last) = off.public_key.last_mut() {
            *last ^= 1;
        }
        assert!(!signed_by(&off, ecdsa, sha256, &d256, &unhex(P256_SIG)));
    }

    /// Round 3 P3 2: a key paired with its certificate through a stale public-key object of
    /// another key signs, and the signature is found not to be the certificate's.
    #[test]
    fn a_key_paired_through_its_public_object_must_sign_as_the_certificate() {
        let public = |facts| FakeKey { id: ID.to_vec(), public: true, facts, always: false };
        let hidden = FakeKey { id: ID.to_vec(), public: false, facts: KeyFacts::default(), always: false };
        let digest = unhex(DIGEST_256);
        let sign_with = |held: &HeldCertificate, object: KeyFacts, scheme, signature: Vec<u8>| {
            let mut card = token(3, "0123", &held.der, vec![hidden.clone(), public(object)]);
            card.signature = signature;
            let named = card.facts.token.clone();
            sign_digest(&module(vec![card]), &held.fingerprint, scheme, DigestAlgorithm::Sha256, &digest, Some((&named, &pin())))
        };
        let not_its = Err(KeyHolderError::NotAllowed("the token signed with a key that isn't the certificate's"));

        let rsa = real(RSA_SPKI);
        let modulus_object = KeyFacts { kind: Some(KeyKindOnToken::Rsa), modulus: Some(rsa.public_key.clone()), ec_point: None };
        assert_eq!(sign_with(&rsa, modulus_object.clone(), SignatureScheme::RsaPkcs1v15, unhex(RSA_PKCS1)), Ok(unhex(RSA_PKCS1)));
        assert_eq!(sign_with(&rsa, modulus_object.clone(), SignatureScheme::RsaPss, unhex(RSA_PSS)), Ok(unhex(RSA_PSS)));
        assert_eq!(sign_with(&rsa, modulus_object, SignatureScheme::RsaPkcs1v15, flipped(RSA_PKCS1)), not_its);

        let p256 = real(P256_SPKI);
        let point_object = KeyFacts { kind: Some(KeyKindOnToken::Ec), modulus: None, ec_point: Some(p256.public_key.clone()) };
        let signed = sign_with(&p256, point_object.clone(), SignatureScheme::Ecdsa, unhex(P256_SIG));
        assert_eq!(signed, ecdsa_raw_to_der(&unhex(P256_SIG), p256.key));
        assert_eq!(sign_with(&p256, point_object, SignatureScheme::Ecdsa, unhex(P256_SHA384_SIG)), not_its);

        // A key that shows its own modulus is the certificate's by that, and isn't checked again.
        let mut card = token(3, "0123", &rsa.der, vec![rsa_private(Some(rsa.public_key.clone()), Some(KeyKindOnToken::Rsa))]);
        card.signature = flipped(RSA_PKCS1);
        let named = card.facts.token.clone();
        let own = sign_digest(
            &module(vec![card]),
            &rsa.fingerprint,
            SignatureScheme::RsaPkcs1v15,
            DigestAlgorithm::Sha256,
            &digest,
            Some((&named, &pin())),
        );
        assert_eq!(own, Ok(flipped(RSA_PKCS1)));
    }

    /// Re-review P1 (open_default) and P3 6: a module the owner named is the only one tried,
    /// and one that isn't an absolute path is said, not passed over.
    #[test]
    fn a_named_module_is_the_only_one_tried() {
        let tried = RefCell::new(Vec::new());
        let open = |p: &Path| -> Result<&'static str, KeyHolderError> {
            tried.borrow_mut().push(p.to_path_buf());
            Err(KeyHolderError::Unavailable("won't load".into()))
        };
        let everything = |_: &Path| true;
        assert_eq!(choose_module(Some(OsString::from("/example/owner.so")), everything, open), Err(KeyHolderError::Unavailable("won't load".into())));
        assert_eq!(*tried.borrow(), vec![PathBuf::from("/example/owner.so")]);

        tried.borrow_mut().clear();
        assert_eq!(
            choose_module(Some(OsString::from("owner.so")), everything, open),
            Err(KeyHolderError::Unavailable(format!("{MODULE_VARIABLE} is not an absolute path")))
        );
        assert!(tried.borrow().is_empty());

        // Nothing named (or named empty): the proxy's places, in turn, those that exist.
        for named in [None, Some(OsString::new())] {
            tried.borrow_mut().clear();
            let _ = choose_module(named, |p: &Path| p.starts_with("/usr/lib64"), open);
            assert_eq!(*tried.borrow(), vec![PathBuf::from("/usr/lib64/p11-kit-proxy.so")]);
        }
        tried.borrow_mut().clear();
        let _ = choose_module(None, everything, open);
        assert_eq!(tried.borrow().len(), PROXY_PLACES.len());
    }

    #[test]
    fn a_module_must_be_named_by_an_absolute_path_that_loads() {
        assert!(matches!(Pkcs11Holder::open(Path::new("p11-kit-proxy.so")), Err(KeyHolderError::Unavailable(_))));
        assert!(matches!(Pkcs11Holder::open(Path::new("/nonexistent/example/module.so")), Err(KeyHolderError::Unavailable(_))));
        let _ = open_default();
    }

    /// Review item 1: one holder per module for the life of the program; a failure isn't kept.
    #[test]
    fn a_module_is_kept_once_and_never_let_go_of() {
        let registry: Registry<u32> = Mutex::new(Vec::new());
        let first = shared(&registry, Path::new("/example/a.so"), 2, |_| Ok(1)).unwrap();
        let again = shared(&registry, Path::new("/example/a.so"), 2, |_| Ok(2)).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*again, 1);

        assert!(shared(&registry, Path::new("/example/b.so"), 2, |_| Err(KeyHolderError::Unavailable("no".into()))).is_err());
        assert_eq!(*shared(&registry, Path::new("/example/b.so"), 2, |_| Ok(3)).unwrap(), 3);
        assert!(shared(&registry, Path::new("/example/c.so"), 2, |_| Ok(4)).is_err());
        drop((first, again));
        // Still held by the registry after every caller let go.
        let counts: Vec<usize> =
            registry.lock().unwrap().iter().map(|(_, place)| place.lock().unwrap().as_ref().map_or(0, Arc::strong_count)).collect();
        assert_eq!(counts, vec![1, 1]);
    }

    /// Round 3 P3 3: a file that fails to load gives its place back; only loaded modules count.
    #[test]
    fn a_module_that_fails_to_load_keeps_no_place() {
        let registry: Registry<u32> = Mutex::new(Vec::new());
        for i in 0..10 {
            let path = PathBuf::from(format!("/example/broken-{i}.so"));
            assert_eq!(shared(&registry, &path, 2, |_| Err(KeyHolderError::Unavailable("no".into()))), Err(KeyHolderError::Unavailable("no".into())));
        }
        assert!(registry.lock().unwrap().is_empty());
        assert_eq!(shared(&registry, Path::new("/example/a.so"), 2, |_| Ok(1)).map(|m| *m), Ok(1));
        assert_eq!(shared(&registry, Path::new("/example/b.so"), 2, |_| Ok(2)).map(|m| *m), Ok(2));
        assert!(shared(&registry, Path::new("/example/broken-0.so"), 2, |_| Ok(3)).is_err());
        assert_eq!(registry.lock().unwrap().len(), 2);
    }

    /// Re-review P3 5: the same file by another path is the same module, and a module that
    /// hangs while loading holds up no other.
    #[test]
    fn a_module_is_known_by_its_file_and_loads_alone() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let roundabout = manifest.join("src").join("..").join("Cargo.toml");
        assert_eq!(module_key(&roundabout), std::fs::canonicalize(manifest.join("Cargo.toml")).unwrap());
        assert_eq!(module_key(Path::new("/nonexistent/example/module.so")), PathBuf::from("/nonexistent/example/module.so"));
        let registry: Registry<u32> = Mutex::new(Vec::new());
        let first = shared(&registry, &roundabout, 4, |_| Ok(1)).unwrap();
        assert!(Arc::ptr_eq(&first, &shared(&registry, &manifest.join("Cargo.toml"), 4, |_| Ok(2)).unwrap()));

        let registry: Arc<Registry<u32>> = Arc::new(Mutex::new(Vec::new()));
        let (loading, started) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let slow = {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || {
                shared(&registry, Path::new("/example/slow.so"), 4, |_| {
                    let _ = loading.send(());
                    let _ = held.recv();
                    Ok(1)
                })
            })
        };
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        let (done, finished) = mpsc::channel();
        let quick = {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || {
                let _ = done.send(shared(&registry, Path::new("/example/quick.so"), 4, |_| Ok(2)).map(|q| *q));
            })
        };
        let quick_result = finished.recv_timeout(Duration::from_secs(10));
        let _ = release.send(());
        assert_eq!(quick_result, Ok(Ok(2)), "another module waited on the one loading");
        assert_eq!(slow.join().unwrap().map(|s| *s), Ok(1));
        quick.join().unwrap();
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
        assert!(!key_matches(&rsa, &other_key().facts));
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

    /// Review items 7, 9 and 10, re-review P3 2 and 3: errors name the token, never loop, and a
    /// PIN goes only to the token it was asked for.
    #[test]
    fn token_errors_are_named_and_never_ask_twice() {
        let token = example_token();
        let card = TokenFacts { token: token.clone(), protected_path: false, login_required: true };
        let pad = TokenFacts { protected_path: true, ..card.clone() };
        let error = |rv| token_error(Error::Pkcs11(rv, Function::Sign), &card, false);
        assert_eq!(error(RvError::PinIncorrect), KeyHolderError::PinIncorrect);
        assert_eq!(error(RvError::PinLocked), KeyHolderError::PinLocked);
        assert_eq!(error(RvError::PinExpired), KeyHolderError::PinExpired);
        assert_eq!(error(RvError::FunctionCanceled), KeyHolderError::Cancelled);
        assert_eq!(error(RvError::TokenNotPresent), KeyHolderError::TokenChanged);
        assert!(matches!(error(RvError::MechanismInvalid), KeyHolderError::Unsupported(_)));
        assert!(matches!(error(RvError::DeviceError), KeyHolderError::System(_)));

        // Not logged in: without a login in this call, the window is asked for this token's PIN;
        // after one, or on a PIN pad, it's over.
        let not_logged_in = |facts: &TokenFacts, logged_in| token_error(Error::Pkcs11(RvError::UserNotLoggedIn, Function::Sign), facts, logged_in);
        assert_eq!(not_logged_in(&card, false), KeyHolderError::PinNeeded(token.clone()));
        assert!(matches!(not_logged_in(&card, true), KeyHolderError::Unsupported(_)));
        assert!(matches!(not_logged_in(&pad, false), KeyHolderError::Unsupported(_)));
        assert!(matches!(not_logged_in(&pad, true), KeyHolderError::Unsupported(_)));

        assert_eq!(check_same_token(None, &token), Ok(()));
        assert_eq!(check_same_token(Some(&token), &token), Ok(()));
        let moved = TokenRef { slot: 4, ..token.clone() };
        assert_eq!(check_same_token(Some(&moved), &token), Err(KeyHolderError::TokenChanged));
        let swapped = TokenRef { serial: "9999".into(), ..token.clone() };
        assert_eq!(check_same_token(Some(&swapped), &token), Err(KeyHolderError::TokenChanged));
        // The label alone may change (it's only shown)...
        let relabelled = TokenRef { label: "Renamed".into(), ..token.clone() };
        assert_eq!(check_same_token(Some(&relabelled), &token), Ok(()));
        // ...except on tokens without a serial number.
        let blank = TokenRef { serial: String::new(), ..token.clone() };
        assert_eq!(check_same_token(Some(&blank), &blank), Ok(()));
        let other_blank = TokenRef { label: "Renamed".into(), ..blank.clone() };
        assert_eq!(check_same_token(Some(&other_blank), &blank), Err(KeyHolderError::TokenChanged));
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
        let gone = TokenRef { slot: u64::MAX, serial: "none".into(), label: "none".into() };
        let refused = holder.sign_digest(&[0; 32], SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256, &[0; 32], Some((&gone, &pin())));
        assert_eq!(refused, Err(KeyHolderError::TokenChanged));
    }
}
