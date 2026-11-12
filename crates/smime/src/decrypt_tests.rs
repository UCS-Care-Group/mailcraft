// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening S/MIME Encrypted messages (#30 part 6). Messages are made here from keys and
//! example.org certificates made here; the RSA private key is a stand-in for the key holder,
//! which in the program is the system's store or a token (no RSA private-key operation runs in
//! the crate itself: the last test reads its source to see that).

use std::cell::Cell;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyIvInit};
use aes_gcm::aead::{AeadInPlace, KeyInit, consts::U12, consts::U16, generic_array::GenericArray};
use const_oid::ObjectIdentifier;
use der::{Decode, Encode};
use proptest::prelude::*;
use zeroize::Zeroizing;

use crate::sign_tests::{random, root, rsa_key, rsa_public, signer_cert, signer_cert_with};
use crate::{DecryptError, KeyTransport, MAX_UNWRAPS, Unwrapped, decrypt, is_enveloped};

const CONTENT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe board meets on Friday.\r\n";

// ---- DER, written by hand ----

fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match value.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n if n < 0x10000 => out.extend([0x82, (n >> 8) as u8, n as u8]),
        n => out.extend([0x83, (n >> 16) as u8, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(value);
    out
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

fn oid(text: &str) -> Vec<u8> {
    ObjectIdentifier::new_unwrap(text).to_der().unwrap()
}

fn octets(bytes: &[u8]) -> Vec<u8> {
    tlv(0x04, bytes)
}

fn int(n: u8) -> Vec<u8> {
    tlv(0x02, &[n])
}

fn alg(oid_text: &str, parameters: Option<Vec<u8>>) -> Vec<u8> {
    let mut parts = vec![oid(oid_text)];
    parts.extend(parameters);
    seq(&parts)
}

fn content_info(oid_text: &str, inner: &[u8]) -> Vec<u8> {
    seq(&[oid(oid_text), tlv(0xa0, inner)])
}

const RSA: &str = "1.2.840.113549.1.1.1";
const OAEP: &str = "1.2.840.113549.1.1.7";
const SHA1: &str = "1.3.14.3.2.26";
const SHA256: &str = "2.16.840.1.101.3.4.2.1";
const AES128_CBC: &str = "2.16.840.1.101.3.4.1.2";
const AES192_CBC: &str = "2.16.840.1.101.3.4.1.22";
const AES256_CBC: &str = "2.16.840.1.101.3.4.1.42";
const AES128_GCM: &str = "2.16.840.1.101.3.4.1.6";
const AES256_GCM: &str = "2.16.840.1.101.3.4.1.46";
const TDES: &str = "1.2.840.113549.3.7";
const ENVELOPED: &str = "1.2.840.113549.1.7.3";
const AUTH_ENVELOPED: &str = "1.2.840.113549.1.9.16.1.23";
const DATA: &str = "1.2.840.113549.1.7.1";

fn rid_issuer_serial(cert: &[u8]) -> Vec<u8> {
    let cert = x509_cert::Certificate::from_der(cert).unwrap();
    let issuer = cert.tbs_certificate().issuer().to_der().unwrap();
    let serial = cert.tbs_certificate().serial_number().as_bytes();
    seq(&[issuer, tlv(0x02, serial)])
}

fn rid_key_id(cert: &[u8]) -> Vec<u8> {
    tlv(0x80, &crate::cert::Cert::from_der(cert).unwrap().key_id().expect("a subject key identifier"))
}

fn transport_alg(transport: KeyTransport) -> Vec<u8> {
    match transport {
        KeyTransport::Pkcs1v15 => alg(RSA, Some(tlv(0x05, &[]))),
        KeyTransport::OaepSha1 => alg(OAEP, Some(oaep_params(SHA1))),
        KeyTransport::OaepSha256 => alg(OAEP, Some(oaep_params(SHA256))),
    }
}

fn oaep_params(hash: &str) -> Vec<u8> {
    let mgf = alg("1.2.840.113549.1.1.8", Some(alg(hash, Some(tlv(0x05, &[])))));
    let empty = alg("1.2.840.113549.1.1.9", Some(octets(&[])));
    seq(&[tlv(0xa0, &alg(hash, Some(tlv(0x05, &[])))), tlv(0xa1, &mgf), tlv(0xa2, &empty)])
}

fn wrap(key: &rsa::PrivateKey, transport: KeyTransport, cek: &[u8]) -> Vec<u8> {
    let public = key.to_public_key();
    let mut rng = random();
    match transport {
        KeyTransport::Pkcs1v15 => public.encrypt(&mut rng, rsa::Pkcs1v15Encrypt, cek).unwrap(),
        KeyTransport::OaepSha1 => public.encrypt(&mut rng, rsa::Oaep::<sha1::Sha1>::new(), cek).unwrap(),
        KeyTransport::OaepSha256 => public.encrypt(&mut rng, rsa::Oaep::<sha2::Sha256>::new(), cek).unwrap(),
    }
}

fn ktri(rid: Vec<u8>, transport: KeyTransport, wrapped: &[u8]) -> Vec<u8> {
    let version = if rid.first() == Some(&0x80) { 2 } else { 0 };
    seq(&[int(version), rid, transport_alg(transport), octets(wrapped)])
}

fn cbc_encrypt(cipher: &str, key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    fn run<E: BlockModeEncrypt>(encryptor: E, data: &[u8]) -> Vec<u8> {
        let mut buffer = data.to_vec();
        buffer.resize(data.len() + 16, 0);
        encryptor.encrypt_padded::<Pkcs7>(&mut buffer, data.len()).unwrap().to_vec()
    }
    match cipher {
        AES128_CBC => run(cbc::Encryptor::<aes::Aes128>::new_from_slices(key, iv).unwrap(), data),
        AES192_CBC => run(cbc::Encryptor::<aes::Aes192>::new_from_slices(key, iv).unwrap(), data),
        AES256_CBC => run(cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv).unwrap(), data),
        _ => run(cbc::Encryptor::<des::TdesEde3>::new_from_slices(key, iv).unwrap(), data),
    }
}

fn enveloped_with(recipients: &[Vec<u8>], cipher: &str, iv: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let info = seq(&[oid(DATA), alg(cipher, Some(octets(iv))), tlv(0x80, ciphertext)]);
    let body = seq(&[int(0), tlv(0x31, &recipients.concat()), info]);
    content_info(ENVELOPED, &body)
}

/// A message encrypted with `cipher` under `cek`, with the recipients given.
fn enveloped(recipients: &[Vec<u8>], cipher: &str, cek: &[u8]) -> Vec<u8> {
    let iv = if cipher == TDES { [7u8; 8].to_vec() } else { [9u8; 16].to_vec() };
    enveloped_with(recipients, cipher, &iv, &cbc_encrypt(cipher, cek, &iv, CONTENT))
}

/// AuthEnvelopedData with AES-GCM, with or without authenticated attributes.
fn auth_enveloped(recipients: &[Vec<u8>], cek: &[u8], tag_len: usize, attributes: Option<&[u8]>, break_tag: bool) -> Vec<u8> {
    let nonce = [3u8; 12];
    let mut aad_set = Vec::new();
    let mut implicit = Vec::new();
    if let Some(attributes) = attributes {
        aad_set = tlv(0x31, attributes);
        implicit = tlv(0xa1, attributes);
    }
    let mut buffer = CONTENT.to_vec();
    let mut tag: Vec<u8> = match (cek.len(), tag_len) {
        (16, 12) => aes_gcm::AesGcm::<aes_gcm::aes::Aes128, U12, U12>::new_from_slice(cek)
            .unwrap()
            .encrypt_in_place_detached(GenericArray::from_slice(&nonce), &aad_set, &mut buffer)
            .unwrap()
            .to_vec(),
        (32, 16) => aes_gcm::AesGcm::<aes_gcm::aes::Aes256, U12, U16>::new_from_slice(cek)
            .unwrap()
            .encrypt_in_place_detached(GenericArray::from_slice(&nonce), &aad_set, &mut buffer)
            .unwrap()
            .to_vec(),
        _ => panic!("a combination the test doesn't make"),
    };
    if break_tag && let Some(first) = tag.first_mut() {
        *first ^= 1;
    }
    let cipher = if cek.len() == 16 { AES128_GCM } else { AES256_GCM };
    let mut params = vec![octets(&nonce)];
    if tag_len != 12 {
        params.push(tlv(0x02, &[tag_len as u8]));
    }
    let info = seq(&[oid(DATA), alg(cipher, Some(seq(&params))), tlv(0x80, &buffer)]);
    let mut parts = vec![int(0), tlv(0x31, &recipients.concat()), info];
    if !implicit.is_empty() {
        parts.push(implicit);
    }
    parts.push(octets(&tag));
    content_info(AUTH_ENVELOPED, &seq(&parts))
}

// ---- the owner's certificates and a stand-in for the key holder ----

struct Owner {
    cert: Vec<u8>,
    key: &'static rsa::PrivateKey,
}

fn owner(address: &str) -> Owner {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    Owner { cert: signer_cert_with(address, &rsa_public(key), &ca, &[0xAB; 20]), key }
}

/// The stand-in key holder: unwraps with the key of one certificate, as the system's store would.
fn holder<'a>(owner: &'a Owner, asked: &'a Cell<usize>) -> impl FnMut(usize, KeyTransport, &[u8]) -> Unwrapped<&'static str> + 'a {
    move |_, transport, wrapped| {
        asked.set(asked.get() + 1);
        let opened = match transport {
            KeyTransport::Pkcs1v15 => owner.key.decrypt(rsa::Pkcs1v15Encrypt, wrapped),
            KeyTransport::OaepSha1 => owner.key.decrypt(rsa::Oaep::<sha1::Sha1>::new(), wrapped),
            KeyTransport::OaepSha256 => owner.key.decrypt(rsa::Oaep::<sha2::Sha256>::new(), wrapped),
        };
        match opened {
            Ok(key) => Unwrapped::Key(Zeroizing::new(key)),
            Err(_) => Unwrapped::Refused,
        }
    }
}

fn open(message: &[u8], owner: &Owner) -> Result<Vec<u8>, DecryptError<&'static str>> {
    let asked = Cell::new(0);
    decrypt(message, &[&owner.cert], holder(owner, &asked)).map(|d| d.content.to_vec())
}

// ---- tests ----

#[test]
fn aes_256_cbc_with_rsa_pkcs1_is_opened_by_issuer_and_serial() {
    let ada = owner("ada@example.org");
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let message = enveloped(&[ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)], AES256_CBC, &cek);
    assert!(is_enveloped(&message));
    assert_eq!(open(&message, &ada).unwrap(), CONTENT);
}

#[test]
fn a_recipient_named_by_its_subject_key_identifier_is_found() {
    let ada = owner("ada@example.org");
    let cek = [0x11u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let message = enveloped(&[ktri(rid_key_id(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)], AES256_CBC, &cek);
    assert_eq!(open(&message, &ada).unwrap(), CONTENT);
}

#[test]
fn every_content_cipher_and_key_wrapping_is_opened() {
    let ada = owner("ada@example.org");
    for (cipher, key_len) in [(AES128_CBC, 16), (AES192_CBC, 24), (AES256_CBC, 32), (TDES, 24)] {
        for transport in [KeyTransport::Pkcs1v15, KeyTransport::OaepSha1, KeyTransport::OaepSha256] {
            let cek = vec![0x5au8; key_len];
            let wrapped = wrap(ada.key, transport, &cek);
            let message = enveloped(&[ktri(rid_issuer_serial(&ada.cert), transport, &wrapped)], cipher, &cek);
            assert_eq!(open(&message, &ada).unwrap(), CONTENT, "{cipher} {transport:?}");
        }
    }
}

#[test]
fn aes_gcm_in_auth_enveloped_data_is_opened_with_and_without_authenticated_attributes() {
    let ada = owner("ada@example.org");
    let attributes = seq(&[oid("1.2.840.113549.1.9.3"), tlv(0x31, &oid(DATA))]);
    for (cek_len, tag_len) in [(16usize, 12usize), (32, 16)] {
        for attrs in [None, Some(attributes.as_slice())] {
            let cek = vec![0x33u8; cek_len];
            let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
            let recipient = ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped);
            let message = auth_enveloped(std::slice::from_ref(&recipient), &cek, tag_len, attrs, false);
            assert!(is_enveloped(&message));
            assert_eq!(open(&message, &ada).unwrap(), CONTENT, "{cek_len} {tag_len} {}", attrs.is_some());
            // A changed tag is not opened.
            assert_eq!(open(&auth_enveloped(&[recipient], &cek, tag_len, attrs, true), &ada), Err(DecryptError::Failed));
        }
    }
}

#[test]
fn the_message_may_have_other_recipients_first_and_its_encrypted_content_in_pieces() {
    let ada = owner("ada@example.org");
    let cek = [0x77u8; 32];
    let other = signer_cert("grace@example.org", &rsa_public(ada.key), &root("Another Root"));
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let recipients =
        [ktri(rid_issuer_serial(&other), KeyTransport::Pkcs1v15, &[1; 256]), ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)];
    let iv = [9u8; 16];
    let ciphertext = cbc_encrypt(AES256_CBC, &cek, &iv, CONTENT);
    let (a, b) = ciphertext.split_at(16);
    let pieces = tlv(0xa0, &[octets(a), octets(b)].concat());
    let info = seq(&[oid(DATA), alg(AES256_CBC, Some(octets(&iv))), pieces]);
    let message = content_info(ENVELOPED, &seq(&[int(0), tlv(0x31, &recipients.concat()), info]));
    assert_eq!(open(&message, &ada).unwrap(), CONTENT);
}

#[test]
fn a_message_for_nobody_here_has_no_key() {
    let ada = owner("ada@example.org");
    let stranger = signer_cert("eve@example.org", &rsa_public(ada.key), &root("Other Root"));
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let message = enveloped(&[ktri(rid_issuer_serial(&stranger), KeyTransport::Pkcs1v15, &wrapped)], AES256_CBC, &cek);
    let asked = Cell::new(0);
    assert_eq!(decrypt(&message, &[&ada.cert], holder(&ada, &asked)).err(), Some(DecryptError::NoKey));
    // The holder was never bothered, and with no certificate at all nothing matches either.
    assert_eq!(asked.get(), 0);
    assert_eq!(decrypt::<&str>(&message, &[], |_, _, _| Unwrapped::Refused).err(), Some(DecryptError::NoKey));
    assert_eq!(decrypt::<&str>(&message, &[b"not a certificate"], |_, _, _| Unwrapped::Refused).err(), Some(DecryptError::NoKey));
}

#[test]
fn every_fault_after_the_recipient_is_found_is_the_same_failure() {
    let ada = owner("ada@example.org");
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let rid = rid_issuer_serial(&ada.cert);
    let good = || ktri(rid.clone(), KeyTransport::Pkcs1v15, &wrapped);
    let mut faults: Vec<(&str, Vec<u8>)> = Vec::new();
    // The key holder refuses (a wrong key, bad padding): wrapped bytes that aren't a wrapped key.
    faults.push(("not a wrapped key", enveloped(&[ktri(rid.clone(), KeyTransport::Pkcs1v15, &[5; 256])], AES256_CBC, &cek)));
    // A key of the wrong size for the cipher.
    let short = wrap(ada.key, KeyTransport::Pkcs1v15, &[1u8; 16]);
    faults.push(("a key of the wrong size", enveloped(&[ktri(rid.clone(), KeyTransport::Pkcs1v15, &short)], AES256_CBC, &cek)));
    // A key that opens, and content that another key made.
    faults.push(("another key's content", enveloped(&[good()], AES256_CBC, &[0x99u8; 32])));
    // A ciphertext that isn't whole blocks, a bad IV, an empty ciphertext, an unknown cipher.
    faults.push(("a short ciphertext", enveloped_with(&[good()], AES256_CBC, &[9; 16], &[1; 17])));
    faults.push(("an IV of the wrong size", enveloped_with(&[good()], AES256_CBC, &[9; 5], &[1; 32])));
    faults.push(("no ciphertext", enveloped_with(&[good()], AES256_CBC, &[9; 16], &[])));
    // Several recipients for the one certificate, all bad.
    faults.push(("many bad recipients", enveloped(&vec![ktri(rid.clone(), KeyTransport::Pkcs1v15, &[5; 256]); 20], AES256_CBC, &cek)));
    for (what, message) in &faults {
        assert_eq!(open(message, &ada), Err(DecryptError::Failed), "{what}");
    }
    // A cipher this doesn't know is a message it can't read at all: the same one answer.
    assert_eq!(open(&enveloped_with(&[good()], "1.2.3.4", &[9; 16], &[1; 32]), &ada), Err(DecryptError::Failed));
}

#[test]
fn changed_bytes_never_panic_and_fail_in_one_way() {
    let ada = owner("ada@example.org");
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let message = enveloped(&[ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)], AES256_CBC, &cek);
    assert_eq!(open(&message, &ada).unwrap(), CONTENT);
    // CBC has no authentication: a change either breaks the padding or alters the text, and the
    // message is a fault in the shapes it can have.
    for at in 0..message.len() {
        let mut changed = message.clone();
        if let Some(b) = changed.get_mut(at) {
            *b ^= 0x55;
        }
        match open(&changed, &ada) {
            Ok(_) | Err(DecryptError::Failed | DecryptError::NotEnveloped | DecryptError::NoKey | DecryptError::Unsupported) => {}
            Err(other) => panic!("byte {at}: {other:?}"),
        }
    }
}

#[test]
fn the_key_holder_stopping_stops_everything_and_says_why() {
    let ada = owner("ada@example.org");
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let rid = rid_issuer_serial(&ada.cert);
    let message = enveloped(&vec![ktri(rid, KeyTransport::Pkcs1v15, &wrapped); 3], AES256_CBC, &cek);
    let asked = Cell::new(0);
    let result = decrypt(&message, &[&ada.cert], |_, _, _| {
        asked.set(asked.get() + 1);
        Unwrapped::Stop("PIN needed")
    });
    assert_eq!(result.err(), Some(DecryptError::Holder("PIN needed")));
    assert_eq!(asked.get(), 1);
}

#[test]
fn no_more_than_a_few_unwraps_are_asked_of_the_key_holder() {
    let ada = owner("ada@example.org");
    let rid = rid_issuer_serial(&ada.cert);
    let message = enveloped(&vec![ktri(rid, KeyTransport::Pkcs1v15, &[5; 256]); 40], AES256_CBC, &[0u8; 32]);
    let asked = Cell::new(0);
    let result = decrypt(&message, &[&ada.cert], |_, _, _| {
        asked.set(asked.get() + 1);
        Unwrapped::<&str>::Refused
    });
    assert_eq!(result.err(), Some(DecryptError::Failed));
    assert_eq!(asked.get(), MAX_UNWRAPS);
}

#[test]
fn a_message_for_key_agreement_only_is_said_not_to_be_readable_yet() {
    let ada = owner("ada@example.org");
    // A KeyAgreeRecipientInfo ([1]) is all it has.
    let kari = tlv(0xa1, &int(3));
    let info = seq(&[oid(DATA), alg(AES256_CBC, Some(octets(&[9; 16]))), tlv(0x80, &[1; 32])]);
    let message = content_info(ENVELOPED, &seq(&[int(2), tlv(0x31, &kari), info]));
    assert_eq!(open(&message, &ada), Err(DecryptError::Unsupported));
}

#[test]
fn signed_data_is_not_an_encrypted_message() {
    let ada = owner("ada@example.org");
    let signed = content_info("1.2.840.113549.1.7.2", &seq(&[int(1)]));
    assert!(!is_enveloped(&signed));
    assert_eq!(open(&signed, &ada), Err(DecryptError::NotEnveloped));
    assert_eq!(open(b"", &ada), Err(DecryptError::NotEnveloped));
}

#[test]
fn a_message_too_large_is_still_told_for_what_it_is() {
    let mut big = content_info(ENVELOPED, &seq(&[int(0)]));
    big.resize(crate::MAX_CMS + 1, 0);
    assert!(is_enveloped(&big));
    let ada = owner("ada@example.org");
    assert_eq!(open(&big, &ada), Err(DecryptError::Failed));
}

#[test]
fn no_rsa_private_key_operation_is_written_in_the_crate() {
    // Decision 6 (#30): RSA keys unwrap through the system's store or a token alone. The module
    // names no RSA library at all.
    let source = include_str!("decrypt.rs");
    for word in ["rsa::", "PrivateKey", "decrypt_blinded", "Pkcs1v15Encrypt", "Oaep::"] {
        assert!(!source.contains(word), "decrypt.rs mentions {word}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn no_bytes_make_it_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let ada = owner("ada@example.org");
        let _ = is_enveloped(&bytes);
        let _ = open(&bytes, &ada);
    }

    #[test]
    fn a_valid_message_with_bytes_changed_never_panics(at in 0usize..400, with in any::<u8>(), cut in 0usize..400) {
        let ada = owner("ada@example.org");
        let cek = [0x42u8; 32];
        let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
        let mut message = enveloped(&[ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)], AES256_CBC, &cek);
        if let Some(b) = message.get_mut(at) {
            *b = with;
        }
        message.truncate(message.len().saturating_sub(cut % 8));
        let _ = open(&message, &ada);
    }
}

#[test]
fn only_the_encryption_that_detects_changes_says_so() {
    // AES-CBC and 3DES can be changed by someone who knows part of what a message says (EFAIL),
    // and what they open to must not be allowed to fetch anything; AES-GCM can't be changed.
    let ada = owner("ada@example.org");
    let cek = [0x42u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let recipient = ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped);
    let asked = Cell::new(0);
    let cbc = enveloped(std::slice::from_ref(&recipient), AES256_CBC, &cek);
    assert!(!decrypt(&cbc, &[&ada.cert], holder(&ada, &asked)).unwrap().authenticated);
    let cek24 = [0x43u8; 24];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek24);
    let tdes = enveloped(&[ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped)], TDES, &cek24);
    assert!(!decrypt(&tdes, &[&ada.cert], holder(&ada, &asked)).unwrap().authenticated);
    let cek = [0x33u8; 32];
    let wrapped = wrap(ada.key, KeyTransport::Pkcs1v15, &cek);
    let recipient = ktri(rid_issuer_serial(&ada.cert), KeyTransport::Pkcs1v15, &wrapped);
    let gcm = auth_enveloped(&[recipient], &cek, 16, None, false);
    assert!(decrypt(&gcm, &[&ada.cert], holder(&ada, &asked)).unwrap().authenticated);
}
