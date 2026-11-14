// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #30: a message that was to go Digitally signed is refused by every sending path unless what it
//! would be written as is a whole signed message of its key's form, with a signature in it.

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use mailcraft_model::{Address, Body, Message, Original};

use crate::{signable, signature_missing};

const ARMOURED: &str = "-----BEGIN PGP SIGNATURE-----\n\niQEzBAABCAAdFiEE\n=abcd\n-----END PGP SIGNATURE-----\n";

fn message(format: SignatureFormat) -> Message {
    Message {
        from: Address::new("Ada", "ada@example.org"),
        to: vec![Address::new("Bob", "bob@example.org")],
        subject: "Plans".into(),
        body: Body { text: "Hello Bob,\n".into(), html: None },
        sign_with: SigningKey::new(format, &"AB".repeat(32), "Ada"),
        ..Message::default()
    }
}

fn signed_as(m: &Message, kind: mailcraft_mime::SignatureKind) -> Message {
    let (micalg, signature) = match kind {
        mailcraft_mime::SignatureKind::Smime => ("sha-256", vec![0x30, 0x03, 0x02, 0x01, 0x01]),
        mailcraft_mime::SignatureKind::OpenPgp => ("pgp-sha256", ARMOURED.as_bytes().to_vec()),
    };
    let body = mailcraft_mime::multipart_signed(&signable(m), kind, micalg, &signature).unwrap();
    Message { original: Original::new(body), ..m.clone() }
}

#[test]
fn only_a_whole_signature_of_the_keys_form_lets_it_go() {
    use mailcraft_mime::SignatureKind::{OpenPgp, Smime};
    let smime = message(SignatureFormat::Smime);
    let pgp = message(SignatureFormat::OpenPgp);
    assert!(signature_missing(&smime) && signature_missing(&pgp), "no signed body");
    assert!(!signature_missing(&signed_as(&smime, Smime)));
    assert!(!signature_missing(&signed_as(&pgp, OpenPgp)));
    assert!(signature_missing(&signed_as(&smime, OpenPgp)), "signed in the other form");
    assert!(signature_missing(&signed_as(&pgp, Smime)), "signed in the other form");
    // Not to be signed: never missing.
    assert!(!signature_missing(&Message { sign_with: None, ..smime }));
}
