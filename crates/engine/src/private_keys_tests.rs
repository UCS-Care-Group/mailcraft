// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules for the owner's own private keys (#30 R13, the owner's decisions of 2026-11-11),
//! and a contact card's keys: with synthetic `example.org` keys made for these tests alone.

use mailcraft_charset::base64::STANDARD;
use mailcraft_model::Mailbox;
use serde_json::json;

use crate::Session;
use crate::cmd::keys::card_key_from;
use crate::private_keys::{openpgp_import, smime_import};
use crate::private_keys_fixtures_tests::{CAROL_PUBLIC, CAROL_SECRET, DAN_SECRET, EC_CRT, EC_P12, ERIN_SECRET, FAY_SECRET, PASS, PLAIN_P12, RSA_P12};

fn p12(b64: &str) -> Vec<u8> {
    STANDARD.decode(b64).unwrap()
}

#[test]
fn an_openpgp_key_without_a_passphrase_is_refused() {
    let err = openpgp_import(DAN_SECRET.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("no passphrase"), "{err}");
}

#[test]
fn an_rsa_openpgp_key_is_refused_and_ed25519_is_taken() {
    let err = openpgp_import(ERIN_SECRET.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("RSA"), "{err}");
    let carol = openpgp_import(CAROL_SECRET.as_bytes()).unwrap();
    assert!(carol.protected && carol.can_sign);
    assert!(carol.addresses.iter().any(|a| a == "carol@example.org"));
    assert!(openpgp_import(CAROL_PUBLIC.as_bytes()).is_err());
    assert!(openpgp_import(b"not a key").is_err());
}

/// An Ed25519 key that signs, with an RSA subkey for encryption (as GnuPG makes one on asking):
/// an RSA part all the same, never used in this process.
#[test]
fn an_ed25519_key_with_an_rsa_subkey_is_refused() {
    let err = openpgp_import(FAY_SECRET.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("RSA"), "{err}");
}

/// The whole file is kept, so it holds the one key checked and nothing else.
#[test]
fn a_file_of_two_keys_is_refused() {
    let two = format!("{CAROL_SECRET}\n{DAN_SECRET}");
    let err = openpgp_import(two.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("more than one key"), "{err}");
    let err = openpgp_import(format!("{CAROL_SECRET}\n{CAROL_PUBLIC}").as_bytes()).unwrap_err().to_string();
    assert!(err.contains("more than one key"), "{err}");
}

/// A `.p12` whose private key is in clear in the file (vouched for by its MAC alone) isn't kept,
/// whatever the window would say of it.
#[test]
fn a_p12_with_its_key_in_clear_is_refused() {
    let opened = mailcraft_smime::pkcs12::open(&p12(PLAIN_P12), PASS).unwrap();
    assert!(!opened.key_encrypted);
    let err = smime_import(&p12(PLAIN_P12), PASS).unwrap_err().to_string();
    assert!(err.contains("isn't encrypted"), "{err}");
}

#[test]
fn an_rsa_p12_is_refused_and_p256_is_taken() {
    let err = smime_import(&p12(RSA_P12), PASS).unwrap_err().to_string();
    assert!(err.contains("RSA"), "{err}");
    assert!(smime_import(&p12(EC_P12), PASS).is_ok());
    let err = smime_import(&p12(EC_P12), "wrong").unwrap_err().to_string();
    assert!(err.contains("passphrase is wrong"), "{err}");
    assert!(smime_import(b"junk", PASS).is_err());
}

#[test]
fn a_card_takes_public_keys_and_never_a_private_one() {
    assert!(card_key_from(EC_CRT.as_bytes()).is_ok());
    assert!(card_key_from(CAROL_PUBLIC.as_bytes()).is_ok());
    for private in [CAROL_SECRET.as_bytes(), &p12(EC_P12)] {
        assert!(card_key_from(private).is_err());
    }
}

#[test]
fn people_edit_keeps_adds_and_drops_a_cards_keys() {
    let mut s = Session::new(Mailbox::with_local_account("Bob", "bob@example.org"));
    let c = s.run("people.new", &json!({"first": "Carol", "email": "carol@example.org"})).unwrap()["id"].as_u64().unwrap();
    let data = |t: &str| json!({"data": STANDARD.encode(t.as_bytes())});
    s.run("people.edit", &json!({"id": c, "keys": [data(EC_CRT), data(CAROL_PUBLIC)]})).unwrap();
    let got = s.run("people.get", &json!({"id": c})).unwrap();
    assert_eq!(got["keys"].as_array().unwrap().len(), 2);
    // Keep the second, drop the first.
    s.run("people.edit", &json!({"id": c, "keys": [{"keep": 1}]})).unwrap();
    let got = s.run("people.get", &json!({"id": c})).unwrap();
    assert_eq!(got["keys"].as_array().unwrap().len(), 1);
    assert!(s.run("people.edit", &json!({"id": c, "keys": [data(CAROL_SECRET)]})).is_err());
    assert!(s.run("people.edit", &json!({"id": c, "keys": [{"keep": 9}]})).is_err());
    assert_eq!(s.run("people.get", &json!({"id": c})).unwrap()["keys"].as_array().unwrap().len(), 1);
}
