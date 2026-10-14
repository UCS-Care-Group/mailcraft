//! Round trips, tampering, wrong keys and hostile bytes.

use proptest::prelude::*;

use crate::smime::{self, Identity};
use crate::testing;
use crate::*;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

const CONTENT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe minutes are attached.\r\n";

#[test]
fn smime_sign_verify_tamper_and_trust() {
    let root = testing::smime_root("Example Root").unwrap();
    let ada = testing::smime_leaf(&root, "Ada Lovelace", "ada@example.org").unwrap();
    assert!(!ada.certificate.is_ca && root.certificate.is_ca);
    assert_eq!(ada.certificate.emails, ["ada@example.org"]);
    let sig = smime::sign_detached(CONTENT, &ada.as_smime(), std::slice::from_ref(&root.certificate), now()).unwrap();

    // Without the root among the trusted: right, but nobody vouches for Ada.
    let check = smime::verify_detached(CONTENT, &sig, &[], &[], now());
    assert_eq!(check.validity, Validity::Valid, "{check:?}");
    assert!(matches!(check.trust, Trust::NotVerified(_)));
    assert_eq!(check.signer_emails(), ["ada@example.org"]);
    assert!(check.signed_at.is_some_and(|t| (t - now()).abs() < 120));
    assert_eq!(check.carried.len(), 2);

    // With it: verified.
    let check = smime::verify_detached(CONTENT, &sig, &[], std::slice::from_ref(&root.certificate), now());
    assert_eq!(check.trust, Trust::Verified("Example Root".into()));

    // One byte changed: invalid.
    let mut changed = CONTENT.to_vec();
    changed[50] ^= 1;
    let check = smime::verify_detached(&changed, &sig, &[], std::slice::from_ref(&root.certificate), now());
    assert!(matches!(check.validity, Validity::Invalid(_)), "{check:?}");

    // A year and more later, the certificate has run out.
    let check = smime::verify_detached(CONTENT, &sig, &[], std::slice::from_ref(&root.certificate), now() + 400 * 86_400);
    assert!(matches!(check.trust, Trust::NotVerified(ref why) if why.contains("expired")), "{check:?}");

    // Another root doesn't vouch.
    let other = testing::smime_root("Other Root").unwrap();
    let check = smime::verify_detached(CONTENT, &sig, &[], std::slice::from_ref(&other.certificate), now());
    assert!(matches!(check.trust, Trust::NotVerified(_)));

    // Garbage for a signature.
    let check = smime::verify_detached(CONTENT, b"not a signature", &[], &[], now());
    assert!(matches!(check.validity, Validity::Invalid(_)));
}

#[test]
fn smime_expired_and_self_signed() {
    let old = testing::smime_expired("Old Key", "old@example.org").unwrap();
    let sig = smime::sign_detached(CONTENT, &old.as_smime(), &[], now()).unwrap();
    let check = smime::verify_detached(CONTENT, &sig, &[], std::slice::from_ref(&old.certificate), now());
    assert_eq!(check.validity, Validity::Valid);
    assert!(matches!(check.trust, Trust::NotVerified(ref why) if why.contains("expired")), "{check:?}");

    let me = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    assert!(me.certificate.self_signed && !me.certificate.is_ca);
    let sig = smime::sign_detached(CONTENT, &me.as_smime(), &[], now()).unwrap();
    // Trusting the certificate itself vouches for it.
    let check = smime::verify_detached(CONTENT, &sig, &[], std::slice::from_ref(&me.certificate), now());
    assert_eq!(check.trust, Trust::Verified("Bea".into()));
}

#[test]
fn smime_encrypt_decrypt_and_wrong_key() {
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let bea = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    let cyd = testing::smime_self_signed("Cyd", "cyd@example.org").unwrap();
    let env = smime::encrypt(CONTENT, &[ada.certificate.clone(), bea.certificate.clone()]).unwrap();
    assert_eq!(smime::kind(&env), Some(smime::Kind::Enveloped));
    for who in [&ada, &bea] {
        let opened = smime::open(&env, &[who.as_smime()], &[], &[], now()).unwrap();
        assert!(opened.encrypted && opened.signature.is_none());
        assert_eq!(opened.content, CONTENT);
    }
    assert!(matches!(smime::open(&env, &[cyd.as_smime()], &[], &[], now()), Err(CryptoError::NoKey(_))));
    assert!(matches!(smime::open(&env, &[], &[], &[], now()), Err(CryptoError::NoKey(_))));
    // A cut-short message fails cleanly.
    assert!(smime::open(&env[..env.len() / 2], &[ada.as_smime()], &[], &[], now()).is_err());
    // Somebody else's key under Ada's certificate: decrypting fails, cleanly.
    let wrong = Identity { certificate: &ada.certificate, secret: &cyd.secret };
    assert!(smime::open(&env, &[wrong], &[], &[], now()).is_err());
    assert!(smime::encrypt(CONTENT, &[]).is_err());
}

#[test]
fn p12_and_pem_import() {
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let file = testing::p12_file(&ada, "correct horse").unwrap();
    let got = import(&file, Some("correct horse")).unwrap();
    assert_eq!(got.public.len(), 1);
    assert_eq!(got.public[0].fingerprint, ada.certificate.fingerprint);
    assert_eq!(got.secret.len(), 1);
    assert_eq!(got.secret[0].fingerprint, ada.certificate.fingerprint);
    assert_eq!(got.secret[0].data, ada.secret.data);
    assert_eq!(import(&file, Some("wrong")), Err(CryptoError::Passphrase));
    assert_eq!(import(&file, None), Err(CryptoError::Passphrase));

    // A certificate as PEM and as DER, and a PKCS#8 key as PEM.
    let pem = String::from_utf8(export_public(&ada.certificate, true).unwrap()).unwrap();
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
    let key_pem = crate::pem::encode("PRIVATE KEY", &ada.secret.data);
    let got = import(format!("{pem}{key_pem}").as_bytes(), None).unwrap();
    assert_eq!(got.public[0], ada.certificate);
    assert_eq!(got.secret[0].fingerprint, ada.certificate.fingerprint);
    let der = export_public(&ada.certificate, false).unwrap();
    assert_eq!(import(&der, None).unwrap().public[0], ada.certificate);
    assert_eq!(read_public(Scheme::Smime, &ada.certificate.data).unwrap(), ada.certificate);
    assert!(secret_belongs_to(&ada.secret, &ada.certificate));
    let bea = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    assert!(!secret_belongs_to(&bea.secret, &ada.certificate));
}

#[test]
fn openpgp_sign_verify_encrypt_decrypt() {
    let ada = testing::openpgp("Ada Lovelace", "ada@example.org").unwrap();
    let bea = testing::openpgp("Bea", "bea@example.org").unwrap();
    assert_eq!(ada.certificate.emails, ["ada@example.org"]);
    assert!(ada.certificate.can_encrypt && ada.certificate.can_sign);

    let sig = openpgp::sign_detached(CONTENT, &ada.secret).unwrap();
    assert!(sig.starts_with("-----BEGIN PGP SIGNATURE-----"));
    let check = openpgp::verify_detached(CONTENT, sig.as_bytes(), &[ada.certificate.clone(), bea.certificate.clone()]);
    assert_eq!(check.validity, Validity::Valid);
    assert_eq!(check.signer.as_ref().map(|s| s.fingerprint.as_str()), Some(ada.certificate.fingerprint.as_str()));
    assert!(matches!(check.trust, Trust::NotVerified(_)));
    // Changed content.
    let check = openpgp::verify_detached(b"something else", sig.as_bytes(), std::slice::from_ref(&ada.certificate));
    assert!(matches!(check.validity, Validity::Invalid(_)), "{check:?}");
    // A key nobody here has.
    let check = openpgp::verify_detached(CONTENT, sig.as_bytes(), std::slice::from_ref(&bea.certificate));
    assert_eq!(check.validity, Validity::UnknownSigner);

    let armored = openpgp::encrypt(CONTENT, &[ada.certificate.clone(), bea.certificate.clone()]).unwrap();
    assert!(armored.starts_with("-----BEGIN PGP MESSAGE-----"));
    for who in [&ada, &bea] {
        let d = openpgp::decrypt(armored.as_bytes(), std::slice::from_ref(&who.secret), &[]).unwrap();
        assert_eq!(d.content, CONTENT);
        assert!(d.signature.is_none());
    }
    let cyd = testing::openpgp("Cyd", "cyd@example.org").unwrap();
    assert!(matches!(openpgp::decrypt(armored.as_bytes(), std::slice::from_ref(&cyd.secret), &[]), Err(CryptoError::NoKey(_))));
    assert!(matches!(openpgp::decrypt(armored.as_bytes(), &[], &[]), Err(CryptoError::NoKey(_))));
    assert!(openpgp::decrypt(&armored.as_bytes()[..armored.len() / 2], std::slice::from_ref(&ada.secret), &[]).is_err());
}

#[test]
fn openpgp_inline_signed_text() {
    use pgp::composed::{CleartextSignedMessage, Deserializable as _, SignedSecretKey};
    let ada = testing::openpgp("Ada", "ada@example.org").unwrap();
    let (key, _) = SignedSecretKey::from_reader_single(ada.secret.data.as_slice()).unwrap();
    let signed = CleartextSignedMessage::sign(crate::rng(), "Meet at noon.\n", &key.secret_subkeys[0].key, &"".into()).unwrap();
    let text = signed.to_armored_string(Default::default()).unwrap();
    let (check, body) = openpgp::verify_cleartext(&text, std::slice::from_ref(&ada.certificate)).unwrap();
    assert_eq!(check.validity, Validity::Valid);
    assert!(body.contains("Meet at noon."));
    let forged = text.replace("noon", "dawn");
    let (check, _) = openpgp::verify_cleartext(&forged, std::slice::from_ref(&ada.certificate)).unwrap();
    assert!(matches!(check.validity, Validity::Invalid(_)));
}

#[test]
fn openpgp_key_files() {
    let file = testing::openpgp_protected_file("Ada", "ada@example.org", "s3cret").unwrap();
    assert_eq!(import(&file, Some("nope")), Err(CryptoError::Passphrase));
    let got = import(&file, Some("s3cret")).unwrap();
    assert_eq!(got.secret.len(), 1);
    assert_eq!(got.public[0].emails, ["ada@example.org"]);
    // The unlocked key works without the passphrase from now on.
    let sig = openpgp::sign_detached(CONTENT, &got.secret[0]).unwrap();
    assert_eq!(openpgp::verify_detached(CONTENT, sig.as_bytes(), &got.public).validity, Validity::Valid);
    // The public half exports and imports again.
    let asc = export_public(&got.public[0], true).unwrap();
    assert!(asc.starts_with(b"-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    let again = import(&asc, None).unwrap();
    assert_eq!(again.public[0].fingerprint, got.public[0].fingerprint);
    assert!(again.secret.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn readers_never_panic_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = import(&bytes, None);
        let _ = import(&bytes, Some("x"));
        let _ = crate::ber::to_der(&bytes);
        let _ = smime::kind(&bytes);
        let _ = smime::verify_detached(b"x", &bytes, &[], &[], 0);
        let _ = smime::open(&bytes, &[], &[], &[], 0);
        let _ = openpgp::verify_detached(b"x", &bytes, &[]);
        let _ = openpgp::verify_cleartext(&String::from_utf8_lossy(&bytes), &[]);
        let _ = p12::read(&bytes, "");
        let _ = read_public(Scheme::Smime, &bytes);
        let _ = read_public(Scheme::OpenPgp, &bytes);
    }

    #[test]
    fn armored_junk_never_panics(body in "[A-Za-z0-9+/=\n]{0,600}") {
        for label in ["PGP MESSAGE", "PGP SIGNATURE", "PGP PUBLIC KEY BLOCK", "PGP PRIVATE KEY BLOCK", "CERTIFICATE", "PRIVATE KEY", "ENCRYPTED PRIVATE KEY", "PKCS7"] {
            let text = format!("-----BEGIN {label}-----\n\n{body}\n-----END {label}-----\n");
            let _ = import(text.as_bytes(), Some("x"));
            let _ = openpgp::verify_detached(b"x", text.as_bytes(), &[]);
            let _ = openpgp::verify_cleartext(&text, &[]);
        }
    }
}

#[test]
fn hostile_der_around_real_structures() {
    // Real signatures and envelopes with bytes flipped all over: never a panic.
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let sig = smime::sign_detached(CONTENT, &ada.as_smime(), &[], now()).unwrap();
    let env = smime::encrypt(CONTENT, std::slice::from_ref(&ada.certificate)).unwrap();
    for source in [&sig, &env, &ada.certificate.data] {
        for i in (0..source.len()).step_by(7) {
            let mut b = source.clone();
            b[i] ^= 0x5a;
            let _ = smime::verify_detached(CONTENT, &b, &[], &[], now());
            let _ = smime::open(&b, &[ada.as_smime()], &[], &[], now());
            let _ = import(&b, None);
        }
    }
}
