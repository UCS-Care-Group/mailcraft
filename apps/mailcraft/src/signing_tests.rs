// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing at Send (#30 part 5): what can't be signed is taken out of the run and said, never
//! sent; what is signed goes as its signed bytes. Every name and message is invented.

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use mailcraft_model::sync::{SendOutcome, SendUp};
use mailcraft_model::{Address, Body, Message};
use mailcraft_ui_egui::digital_signatures::SigningHost;

use crate::key_holders::TokenRef;
use crate::signing::*;

fn message(id: u64, sign: bool) -> Message {
    Message {
        id,
        from: Address::new("Owner", "owner@example.org"),
        to: vec![Address::new("Kit", "kit@example.net")],
        subject: "Rota".into(),
        date: 1_790_000_000,
        message_id: format!("rota.{id}@example.org"),
        body: Body { text: "Hello Kit,\n".into(), html: None },
        sign_with: sign.then(|| SigningKey::new(SignatureFormat::Smime, &"AB".repeat(32), "Owner").unwrap()),
        ..Message::default()
    }
}

/// A stand-in signer: signs message 1, can't sign the rest.
fn stand_in(m: &mut Message) -> Result<(), String> {
    if m.sign_with.is_none() {
        return Ok(());
    }
    if m.id != 1 {
        return Err("the smartcard Kit needs its PIN".into());
    }
    let entity = mailcraft_mime_model::signable(m);
    finish(m, &entity, mailcraft_mime::SignatureKind::Smime, "sha-256", &[0x30, 0x03, 0x02, 0x01, 0x01])
}

#[test]
fn what_cant_be_signed_is_taken_out_and_said() {
    let mut outbox = vec![message(1, true), message(2, true), message(3, false)];
    let unsigned = sign_outbox_with(&mut outbox, &stand_in);
    assert_eq!(outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(unsigned.len(), 1);
    assert_eq!(unsigned[0].message, 2);
    assert!(unsigned[0].why.contains("PIN"));
    assert!(!mailcraft_mime_model::signature_missing(&outbox[0]), "signed");
    let wire = mailcraft_mime_model::write(&outbox[0], &Default::default());
    let parts = mailcraft_mime::signed_parts(&wire).unwrap();
    assert_eq!(parts.content, mailcraft_mime_model::signable(&message(1, true)));
}

#[test]
fn a_send_that_cant_be_signed_is_held() {
    let up = |id| SendUp { message: id, draft: None, mime: Some(Box::new(message(id, true))), discard: String::new() };
    let mut sends = vec![up(1), up(2)];
    let held = held_sends(sign_sends_with(&mut sends, &stand_in));
    assert_eq!(sends.len(), 1);
    assert!(
        matches!(&held[..], [d] if d.message == 2 && matches!(&d.outcome, SendOutcome::Held { lost: false, why } if why.contains("Digitally signed")))
    );
}

#[test]
fn a_pin_goes_to_the_token_it_was_asked_for() {
    let host = Host::default();
    assert_eq!(host.pin_wanted(), None);
    host.give_pin("1234".into());
    let token = TokenRef { slot: 1, serial: "0001".into(), label: "Owner's card".into() };
    assert!(host.pin_for(&token).is_none(), "a PIN nobody asked for is let go of");
    host.want_pin(token.clone());
    assert_eq!(host.pin_wanted().as_deref(), Some("Owner's card"));
    assert_eq!(host.pin_wanted(), None, "asked once");
    host.give_pin("1234".into());
    assert_eq!(host.pin_for(&token).map(|p| p.expose().to_string()).as_deref(), Some("1234"));
    host.forget_pin(&token);
    assert!(host.pin_for(&token).is_none());
}

#[test]
fn agent_answers_are_read() {
    let rsa = mailcraft_assuan::sexp::parse(b"(10:public-key(3:rsa(1:n3:\x01\x02\x03)(1:e1:\x03)))").unwrap();
    assert!(matches!(agent_key(&rsa), Some(mailcraft_openpgp::AgentKey::Rsa { n: [1, 2, 3], e: [3] })));
    let ecc = mailcraft_assuan::sexp::parse(b"(10:public-key(3:ecc(5:curve10:NIST P-384)(1:q2:\x04\x01)))").unwrap();
    let key = agent_key(&ecc).unwrap();
    assert_eq!(hash_for(&key), mailcraft_openpgp::HashAlgorithm::Sha384);
    assert!(agent_key(&mailcraft_assuan::sexp::parse(b"(10:public-key(3:dsa))").unwrap()).is_none());
}

#[test]
fn certificates_are_named_by_their_sha256() {
    assert_eq!(sha256_named(&"0A".repeat(32)), Some([0x0A; 32]));
    assert_eq!(sha256_named("0A"), None);
    assert_eq!(sha256_named(&"ZZ".repeat(32)), None);
}

#[test]
fn the_owners_own_keys_are_read_from_gpgs_listing() {
    let listing = b"sec:u:255:22:AAAAAAAAAAAAAAAA:1700000000:::u:::scESC:::+::ed25519:::0:\n\
fpr:::::::::0123456789ABCDEF0123456789ABCDEF01234567:\n\
grp:::::::::0000000000000000000000000000000000000000:\n\
uid:u::::1700000000::X::Owner <owner@example.org>::::::::::0:\n\
ssb:u:255:18:BBBBBBBBBBBBBBBB:1700000000::::::e:::+::cv25519::\n\
fpr:::::::::FEDCBA9876543210FEDCBA9876543210FEDCBA98:\n\
sec:u:2048:1:CCCCCCCCCCCCCCCC:1700000000:::u:::scESC:::+:::23::0:\n\
fpr:::::::::not hex at all:\n\
fpr:::::::::1111111111111111111111111111111111111111:\n";
    // The keys signing lists are those File › Options › Security lists (`own_keys::gnupg`).
    let hex = |l: &[u8]| -> Vec<String> {
        crate::own_keys::gnupg::secret_in(l, 0).into_iter().map(|k| k.fingerprint.chars().filter(char::is_ascii_hexdigit).collect()).collect()
    };
    assert_eq!(hex(listing), ["0123456789ABCDEF0123456789ABCDEF01234567"], "primary keys only; a subkey's and a broken one aren't");
    assert!(hex(&[0xFF; 64]).is_empty());
}
