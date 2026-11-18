// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! OpenPGP Digital signatures checked where a message is read (#30 R6): against the keys kept for
//! the sender's address, a contact's card, the message's own and the owner's GnuPG keyring; the
//! signer's key kept, and nothing else. Synthetic `example.org` people's keys and messages, made
//! with rPGP for these tests.

use mailcraft_model::digital_signature::{SignatureFormat, Verdict, Why};
use mailcraft_model::{Contact, ContactKey, KeyKind, Labeled, Mailbox};
use serde_json::json;

use crate::Session;

/// After the keys and signatures below were made.
const NOW: i64 = 1_794_400_000;
/// Alice's key's fingerprint, as GnuPG writes it.
pub(crate) const ALICE_FPR: &str = "48DEFF8644B890EAB3DD5D7589B580192293D2E2";
const ALICE: &str = "alice@example.org";

fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

fn session_with(messages: &[&str]) -> (Session, Vec<u64>) {
    let mut mb = Mailbox::with_local_account("Bob", "bob@example.org");
    let inbox = mb.inbox().unwrap();
    let mut ids = Vec::new();
    for text in messages {
        let mut m = mailcraft_mime_model::parse_fetched(&crlf(text));
        m.folder = inbox;
        ids.push(mb.add_message(m).unwrap());
    }
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    (s, ids)
}

fn said(s: &mut Session, id: u64) -> (Verdict, Option<Why>) {
    let d = s.digital_signature(id).unwrap();
    assert_eq!(d.format, SignatureFormat::OpenPgp);
    (d.verdict, d.why)
}

/// Runs a command that changes nothing, so that what the checks found to keep is kept.
fn settle(s: &mut Session) {
    s.run("keys.list", &json!({})).unwrap();
}

#[test]
fn a_signature_without_its_key_cant_be_checked_until_the_key_is_kept() {
    let (mut s, ids) = session_with(&[SIGNED]);
    assert_eq!(said(&mut s, ids[0]), (Verdict::Unchecked, Some(Why::NoPublicKey)));
    s.run("keys.import", &json!({"data": mailcraft_charset::base64::STANDARD.encode(ALICE_ASC)})).unwrap();
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
    // The owner's word in the window, and only that, makes it Verified; taken back, it isn't.
    s.set_key_verified(ALICE, ALICE_FPR, true).unwrap();
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!(d.verdict, Verdict::Verified, "{d:?}");
    let signer = d.signer.as_ref().unwrap();
    assert_eq!(signer.addresses, vec![ALICE.to_string()]);
    assert_eq!(signer.fingerprint.replace(' ', ""), ALICE_FPR);
    s.set_key_verified(ALICE, ALICE_FPR, false).unwrap();
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
}

#[test]
fn the_owners_gnupg_keyring_verifies_a_key_for_its_address() {
    let (mut s, ids) = session_with(&[WITH_KEY]);
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
    s.set_gnupg_valid([(ALICE_FPR.to_string(), "Alice@Example.org".to_string())]);
    assert_eq!(said(&mut s, ids[0]).0, Verdict::Verified);
    // For another address it verifies nothing.
    s.set_gnupg_valid([(ALICE_FPR.to_string(), "carol@example.org".to_string()), ("junk".to_string(), ALICE.to_string())]);
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
}

#[test]
fn a_key_on_a_contacts_card_checks_the_signature() {
    let (mut s, ids) = session_with(&[SIGNED]);
    assert_eq!(said(&mut s, ids[0]).1, Some(Why::NoPublicKey));
    s.mailbox.add_contact(Contact {
        display_name: "Alice".into(),
        emails: vec![Labeled { label: String::new(), value: ALICE.into() }],
        keys: vec![ContactKey { kind: KeyKind::OpenPgp, data: ALICE_ASC.as_bytes().to_vec(), ..Default::default() }],
        ..Default::default()
    });
    s.mailbox_changed();
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
}

#[test]
fn another_from_address_or_a_changed_message_is_said() {
    let forged = WITH_KEY.replace("From: Alice <alice@example.org>", "From: mallory@example.org");
    let changed = WITH_KEY.replace("my key is attached.", "my key is attached. Pay Mallory.");
    let (mut s, ids) = session_with(&[&forged, &changed]);
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::AddressMismatch)));
    assert_eq!(said(&mut s, ids[1]).0, Verdict::Changed);
    settle(&mut s);
    assert!(s.mailbox.public_keys.of("mallory@example.org").is_empty(), "not for an address the key doesn't name");
    assert!(s.mailbox.public_keys.of(ALICE).is_empty(), "nothing from a changed message");
}

/// The sender shown (the message's From as the mailbox has it) and the From field of its bytes,
/// which the check goes by, aren't one address: Verified is nobody's word for the one shown.
#[test]
fn a_from_shown_that_isnt_the_one_checked_is_a_mismatch() {
    let mut mb = Mailbox::with_local_account("Bob", "bob@example.org");
    let inbox = mb.inbox().unwrap();
    let mut ids = Vec::new();
    for shown in ["mallory@example.org", " Alice@Example.org "] {
        let mut m = mailcraft_mime_model::parse_fetched(&crlf(WITH_KEY));
        m.folder = inbox;
        m.from.email = shown.to_string();
        ids.push(mb.add_message(m).unwrap());
    }
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    s.run("keys.import", &json!({"data": mailcraft_charset::base64::STANDARD.encode(ALICE_ASC)})).unwrap();
    s.set_key_verified(ALICE, ALICE_FPR, true).unwrap();
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::AddressMismatch)));
    assert_eq!(said(&mut s, ids[1]).0, Verdict::Verified, "the same address, in another case and with spaces");
}

#[test]
fn the_signers_key_in_the_message_is_kept_and_nobody_elses() {
    let (mut s, ids) = session_with(&[WITH_TWO]);
    assert_eq!(said(&mut s, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
    settle(&mut s);
    let kept = s.mailbox.public_keys.of(ALICE);
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].verified);
    assert!(s.mailbox.public_keys.of("carol@example.org").is_empty(), "a key that only rides along is never kept");
    // An Autocrypt header's key checks and is kept too.
    let (mut a, ids) = session_with(&[AUTOCRYPT]);
    assert_eq!(said(&mut a, ids[0]), (Verdict::NotVerified, Some(Why::KeyNotVerified)));
    settle(&mut a);
    assert_eq!(a.mailbox.public_keys.of(ALICE).len(), 1);
}

#[test]
fn a_draft_is_never_checked_and_keeps_nothing() {
    let mut mb = Mailbox::with_local_account("Bob", "bob@example.org");
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime_model::parse_fetched(&crlf(WITH_KEY));
    m.folder = inbox;
    m.draft = true;
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    assert!(s.digital_signature(id).is_none());
    settle(&mut s);
    assert!(s.mailbox.public_keys.of(ALICE).is_empty());
}

#[test]
fn a_revocation_in_a_message_reaches_the_kept_key() {
    let (mut s, ids) = session_with(&[SIGNED, WITH_REVOKED]);
    s.run("keys.import", &json!({"data": mailcraft_charset::base64::STANDARD.encode(ALICE_ASC)})).unwrap();
    s.set_key_verified(ALICE, ALICE_FPR, true).unwrap();
    assert_eq!(said(&mut s, ids[0]).0, Verdict::Verified);
    assert_eq!(said(&mut s, ids[1]), (Verdict::Unchecked, Some(Why::KeyRevoked)));
    settle(&mut s);
    // Kept with the revocation, and the mark: the message without it now says so too.
    let kept = s.mailbox.public_keys.of(ALICE);
    assert_eq!(kept.len(), 1);
    assert!(kept[0].verified);
    assert_eq!(said(&mut s, ids[0]), (Verdict::Unchecked, Some(Why::KeyRevoked)));
    // An undo of the import that came before never takes the revocation away.
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(said(&mut s, ids[0]), (Verdict::Unchecked, Some(Why::KeyRevoked)));
}

#[test]
fn an_imported_revocation_stays_through_an_undo() {
    let (mut s, ids) = session_with(&[SIGNED]);
    let encode = |t: &str| mailcraft_charset::base64::STANDARD.encode(t);
    s.run("keys.import", &json!({"data": encode(ALICE_ASC)})).unwrap();
    s.run("keys.import", &json!({"data": encode(ALICE_REVOKED_ASC)})).unwrap();
    assert_eq!(said(&mut s, ids[0]), (Verdict::Unchecked, Some(Why::KeyRevoked)));
    // Undoing the import of the revocation, or importing the old copy again, takes nothing away.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(said(&mut s, ids[0]).1, Some(Why::KeyRevoked));
    s.run("keys.import", &json!({"data": encode(ALICE_ASC)})).unwrap();
    assert_eq!(said(&mut s, ids[0]).1, Some(Why::KeyRevoked));
}

#[test]
fn a_signed_message_read_before_its_bytes_were_kept_says_check_again() {
    let mut mb = Mailbox::with_local_account("Bob", "bob@example.org");
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime_model::parse(&crlf(SIGNED));
    m.folder = inbox;
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    assert_eq!(said(&mut s, id), (Verdict::Unchecked, Some(Why::NoOriginal)));
}

#[test]
fn an_inline_signed_message_is_checked() {
    let (mut s, ids) = session_with(&[INLINE]);
    assert_eq!(said(&mut s, ids[0]).1, Some(Why::NoPublicKey));
    s.run("keys.import", &json!({"data": mailcraft_charset::base64::STANDARD.encode(ALICE_ASC)})).unwrap();
    assert_eq!(said(&mut s, ids[0]).0, Verdict::NotVerified);
}

#[test]
fn an_agent_is_told_the_format() {
    let (mut s, ids) = session_with(&[WITH_KEY]);
    let v = s.run("mail.digitalSignature", &json!({"id": ids[0]})).unwrap();
    assert_eq!(v["digitalSignature"]["format"], "openpgp", "{v}");
    assert_eq!(v["digitalSignature"]["result"], "notVerified", "{v}");
    let got = s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(got["digitalSignature"]["format"], "openpgp");
}

// The keys and messages (written with LF; read with CRLF).
pub(crate) const ALICE_ASC: &str = r#"-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+
hi2KLmzNGUFsaWNlIDxhbGljZUBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYh
BEje/4ZEuJDqs91ddYm1gBkik9LiAhsDAh4BAQsBFQEWAScCGQEACgkQibWAGSKT
0uJA1QEArSq4srq6vxlE/s3VXdNGQB48hwNTRvCEUTeZyyvsS1gA/An3EutbKTbq
Xy81RDcXTRLLnKvv750ByO+v26rqWJAC
=QLXe
-----END PGP PUBLIC KEY BLOCK-----
"#;
pub(crate) const ALICE_REVOKED_ASC: &str = r#"-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+
hi2KLmzCawQgFggAHQUCavOxixYhBEje/4ZEuJDqs91ddYm1gBkik9LiAADDMwEA
s9z0x8mxOWZJpzHjrs/pW+AJ41wa3afw/1jYCRbjNO0BAImQRGSuf5SF6+4JBgzX
BfsSidPI8J+vlOlhGvmTP78FzRlBbGljZSA8YWxpY2VAZXhhbXBsZS5vcmc+woYE
ExYIAC4FAmrzsYEWIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gIbAwIeAQELARUBFgEn
AhkBAAoJEIm1gBkik9LiQNUBAK0quLK6ur8ZRP7N1V3TRkAePIcDU0bwhFE3mcsr
7EtYAPwJ9xLrWyk26l8vNUQ3F00Sy5yr7++dAcjvr9uq6liQAg==
=09Ud
-----END PGP PUBLIC KEY BLOCK-----
"#;
pub(crate) const CAROL_ASC: &str = r#"-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAfiZwJIw1zxqCygcJM8x5Va9m5sCieow804uT
Trd8lKrNGUNhcm9sIDxjYXJvbEBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYh
BDvCaM8LVfsbPRfI4yn3+l+AgZPEAhsDAh4BAQsBFQEWAScCGQEACgkQKff6X4CB
k8TizQEAy+r3wkQA+jn9vasKan/E9ULarr8XHWuZaYOJkjgixHwA/3Z7HKrOAuEB
WpNDHeXMlMn8IrlrKu+JYgriu2Z3e7wD
=XM5C
-----END PGP PUBLIC KEY BLOCK-----
"#;
pub(crate) const SIGNED: &str = r#"From: Alice <alice@example.org>
To: bob@example.org
Subject: Minutes
MIME-Version: 1.0
Content-Type: multipart/signed; micalg=pgp-sha256;
 protocol="application/pgp-signature"; boundary="=-b1"

This is an OpenPGP/MIME signed message.
--=-b1
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: 7bit

Hello Bob,

the minutes are attached.
-- 
Alice

--=-b1
Content-Type: application/pgp-signature; name="signature.asc"
Content-Disposition: attachment; filename="signature.asc"

-----BEGIN PGP SIGNATURE-----

wnUEABYIAB0WIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gUCavOxgQAKCRCJtYAZIpPS
4gkyAP9689KK0kvBamIlon+JAxe0szjy7Kf/wmm2GiwvNFJdtwEAuxXOlix8qK89
Wm7MLSShwWqYKZ++z33rMqoJFry81wg=
=J5To
-----END PGP SIGNATURE-----

--=-b1--
"#;
pub(crate) const WITH_KEY: &str = r#"From: Alice <alice@example.org>
To: bob@example.org
Subject: Minutes
MIME-Version: 1.0
Content-Type: multipart/signed; micalg=pgp-sha256;
 protocol="application/pgp-signature"; boundary="=-b1"

This is an OpenPGP/MIME signed message.
--=-b1
Content-Type: multipart/mixed; boundary="=-m1"

--=-m1
Content-Type: text/plain; charset=utf-8

Hello Bob,
my key is attached.
--=-m1
Content-Type: application/pgp-keys; name="key0.asc"
Content-Disposition: attachment; filename="key0.asc"

-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+
hi2KLmzNGUFsaWNlIDxhbGljZUBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYh
BEje/4ZEuJDqs91ddYm1gBkik9LiAhsDAh4BAQsBFQEWAScCGQEACgkQibWAGSKT
0uJA1QEArSq4srq6vxlE/s3VXdNGQB48hwNTRvCEUTeZyyvsS1gA/An3EutbKTbq
Xy81RDcXTRLLnKvv750ByO+v26rqWJAC
=QLXe
-----END PGP PUBLIC KEY BLOCK-----
--=-m1--

--=-b1
Content-Type: application/pgp-signature; name="signature.asc"
Content-Disposition: attachment; filename="signature.asc"

-----BEGIN PGP SIGNATURE-----

wnUEABYIAB0WIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gUCavOxgQAKCRCJtYAZIpPS
4iHkAQCJuOvzvOJO4KNdedqVIHrb/iz9tykr0HmhBWog8N88jQD+NPHz73Hx2jid
3xjiQPK3ONjPQ3c6GIHyn9yBbjQOLw4=
=RMB0
-----END PGP SIGNATURE-----

--=-b1--
"#;
pub(crate) const WITH_TWO: &str = r#"From: Alice <alice@example.org>
To: bob@example.org
Subject: Minutes
MIME-Version: 1.0
Content-Type: multipart/signed; micalg=pgp-sha256;
 protocol="application/pgp-signature"; boundary="=-b1"

This is an OpenPGP/MIME signed message.
--=-b1
Content-Type: multipart/mixed; boundary="=-m1"

--=-m1
Content-Type: text/plain; charset=utf-8

Hello Bob,
my key is attached.
--=-m1
Content-Type: application/pgp-keys; name="key0.asc"
Content-Disposition: attachment; filename="key0.asc"

-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+
hi2KLmzNGUFsaWNlIDxhbGljZUBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYh
BEje/4ZEuJDqs91ddYm1gBkik9LiAhsDAh4BAQsBFQEWAScCGQEACgkQibWAGSKT
0uJA1QEArSq4srq6vxlE/s3VXdNGQB48hwNTRvCEUTeZyyvsS1gA/An3EutbKTbq
Xy81RDcXTRLLnKvv750ByO+v26rqWJAC
=QLXe
-----END PGP PUBLIC KEY BLOCK-----
--=-m1
Content-Type: application/pgp-keys; name="key1.asc"
Content-Disposition: attachment; filename="key1.asc"

-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAfiZwJIw1zxqCygcJM8x5Va9m5sCieow804uT
Trd8lKrNGUNhcm9sIDxjYXJvbEBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYh
BDvCaM8LVfsbPRfI4yn3+l+AgZPEAhsDAh4BAQsBFQEWAScCGQEACgkQKff6X4CB
k8TizQEAy+r3wkQA+jn9vasKan/E9ULarr8XHWuZaYOJkjgixHwA/3Z7HKrOAuEB
WpNDHeXMlMn8IrlrKu+JYgriu2Z3e7wD
=XM5C
-----END PGP PUBLIC KEY BLOCK-----
--=-m1--

--=-b1
Content-Type: application/pgp-signature; name="signature.asc"
Content-Disposition: attachment; filename="signature.asc"

-----BEGIN PGP SIGNATURE-----

wnUEABYIAB0WIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gUCavOxgQAKCRCJtYAZIpPS
4mr7AP47eJVriaTmDwJafqklZaQXHlN8a4To5sZS0/uMBlggoAEAo1BJmAhLZCZq
ixBCkxKmlQ6CfkJjTBMv8pGtR50hOQc=
=LPlc
-----END PGP SIGNATURE-----

--=-b1--
"#;
pub(crate) const WITH_REVOKED: &str = r#"From: Alice <alice@example.org>
To: bob@example.org
Subject: Minutes
MIME-Version: 1.0
Content-Type: multipart/signed; micalg=pgp-sha256;
 protocol="application/pgp-signature"; boundary="=-b1"

This is an OpenPGP/MIME signed message.
--=-b1
Content-Type: multipart/mixed; boundary="=-m1"

--=-m1
Content-Type: text/plain; charset=utf-8

Hello Bob,
my key is attached.
--=-m1
Content-Type: application/pgp-keys; name="key0.asc"
Content-Disposition: attachment; filename="key0.asc"

-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+
hi2KLmzCawQgFggAHQUCavOxixYhBEje/4ZEuJDqs91ddYm1gBkik9LiAADDMwEA
s9z0x8mxOWZJpzHjrs/pW+AJ41wa3afw/1jYCRbjNO0BAImQRGSuf5SF6+4JBgzX
BfsSidPI8J+vlOlhGvmTP78FzRlBbGljZSA8YWxpY2VAZXhhbXBsZS5vcmc+woYE
ExYIAC4FAmrzsYEWIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gIbAwIeAQELARUBFgEn
AhkBAAoJEIm1gBkik9LiQNUBAK0quLK6ur8ZRP7N1V3TRkAePIcDU0bwhFE3mcsr
7EtYAPwJ9xLrWyk26l8vNUQ3F00Sy5yr7++dAcjvr9uq6liQAg==
=09Ud
-----END PGP PUBLIC KEY BLOCK-----
--=-m1--

--=-b1
Content-Type: application/pgp-signature; name="signature.asc"
Content-Disposition: attachment; filename="signature.asc"

-----BEGIN PGP SIGNATURE-----

wnUEABYIAB0WIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gUCavOxgQAKCRCJtYAZIpPS
4rqaAP4mGBIMhjpnW64wVwb4aOa7AapmzS+f+trzzYYcTC+a9wD9EIQpJcIdSZqC
ae7A0JWNrk4yTQiSxEaSPjSCnuIpkws=
=jIQj
-----END PGP SIGNATURE-----

--=-b1--
"#;
pub(crate) const AUTOCRYPT: &str = r#"From: Alice <alice@example.org>
To: bob@example.org
Autocrypt: addr=alice@example.org; keydata=
 xjMEavOxgRYJKwYBBAHaRw8BAQdAsi1XFagmFg/W1f8GxFOmfvdbT81NWFXXKEb+hi2KLmzNGUFs
 aWNlIDxhbGljZUBleGFtcGxlLm9yZz7ChgQTFggALgUCavOxgRYhBEje/4ZEuJDqs91ddYm1gBki
 k9LiAhsDAh4BAQsBFQEWAScCGQEACgkQibWAGSKT0uJA1QEArSq4srq6vxlE/s3VXdNGQB48hwNT
 RvCEUTeZyyvsS1gA/An3EutbKTbqXy81RDcXTRLLnKvv750ByO+v26rqWJAC
Subject: Minutes
MIME-Version: 1.0
Content-Type: multipart/signed; micalg=pgp-sha256;
 protocol="application/pgp-signature"; boundary="=-b1"

This is an OpenPGP/MIME signed message.
--=-b1
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: 7bit

Hello Bob,

the minutes are attached.
-- 
Alice

--=-b1
Content-Type: application/pgp-signature; name="signature.asc"
Content-Disposition: attachment; filename="signature.asc"

-----BEGIN PGP SIGNATURE-----

wnUEABYIAB0WIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gUCavOxgQAKCRCJtYAZIpPS
4gkyAP9689KK0kvBamIlon+JAxe0szjy7Kf/wmm2GiwvNFJdtwEAuxXOlix8qK89
Wm7MLSShwWqYKZ++z33rMqoJFry81wg=
=J5To
-----END PGP SIGNATURE-----

--=-b1--
"#;
pub(crate) const INLINE: &str = r#"From: Alice <alice@example.org>
Subject: Note
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: 7bit

-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA256

Hello Bob,

the minutes are attached.

-----BEGIN PGP SIGNATURE-----

wnUEARYIAB0FAmrzsYEWIQRI3v+GRLiQ6rPdXXWJtYAZIpPS4gAKCRCJtYAZIpPS
4no/AQCtRtCe6mf8K6oDatBzJLg9UoRc/jgSKnr0u2ytdh1zTQD/VpCXg+ABwGSm
bM/Op48fCyoAPJ82K2f2Fd06n08/iw0=
=LVhi
-----END PGP SIGNATURE-----
"#;
