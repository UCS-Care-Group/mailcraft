// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digitally signed and Encrypted messages told by their MIME, and kept as they came (#30).
//! Synthetic messages with dummy signature bytes: nothing here is real cryptography.

use proptest::prelude::*;

use crate::{PLACE_HEADER, Protection, WriteOptions, is_protected, parse, parse_fetched, parse_mbox, protection, write, write_placed};

/// An S/MIME detached signature, as RFC 8551 lays it out, with a dummy `smime.p7s`.
pub(crate) const SMIME_SIGNED: &[u8] = b"From: Ada <ada@example.org>\r\n\
To: Bea <bea@example.org>\r\n\
Subject: Signed\r\n\
Message-ID: <signed-1@example.org>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256;\r\n\
 boundary=\"sig-b\"\r\n\
\r\n\
This is a cryptographically signed message in MIME format.\r\n\
\r\n\
--sig-b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Signed text.\r\n\
--sig-b\r\n\
Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"smime.p7s\"\r\n\
\r\n\
ZHVtbXkgc2lnbmF0dXJl\r\n\
--sig-b--\r\n";

const PGP_SIGNED: &[u8] = b"From: ada@example.org\r\n\
Content-Type: multipart/signed; micalg=pgp-sha256; protocol=\"application/pgp-signature\"; boundary=\"p\"\r\n\
\r\n\
--p\r\n\
Content-Type: text/plain\r\n\
\r\n\
Hello.\r\n\
--p\r\n\
Content-Type: application/pgp-signature; name=\"signature.asc\"\r\n\
\r\n\
-----BEGIN PGP SIGNATURE-----\r\n\
ZHVtbXk=\r\n\
-----END PGP SIGNATURE-----\r\n\
--p--\r\n";

const PGP_ENCRYPTED: &[u8] = b"From: ada@example.org\r\n\
Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"e\"\r\n\
\r\n\
--e\r\n\
Content-Type: application/pgp-encrypted\r\n\
\r\n\
Version: 1\r\n\
--e\r\n\
Content-Type: application/octet-stream\r\n\
\r\n\
-----BEGIN PGP MESSAGE-----\r\n\
ZHVtbXk=\r\n\
-----END PGP MESSAGE-----\r\n\
--e--\r\n";

const PKCS7: &[u8] = b"From: ada@example.org\r\n\
Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
ZHVtbXkgZW52ZWxvcGVk\r\n";

const INLINE_SIGNED: &[u8] = b"From: ada@example.org\r\n\
Subject: Inline\r\n\
\r\n\
-----BEGIN PGP SIGNED MESSAGE-----\r\n\
Hash: SHA256\r\n\
\r\n\
Hello.\r\n\
-----BEGIN PGP SIGNATURE-----\r\n\
ZHVtbXk=\r\n\
-----END PGP SIGNATURE-----\r\n";

#[test]
fn each_kind_is_told() {
    assert_eq!(protection(SMIME_SIGNED), Some(Protection::Signed));
    assert_eq!(protection(PGP_SIGNED), Some(Protection::Signed));
    assert_eq!(protection(PGP_ENCRYPTED), Some(Protection::Encrypted));
    assert_eq!(protection(PKCS7), Some(Protection::Pkcs7));
    let x_pkcs7 = String::from_utf8_lossy(PKCS7).replace("application/pkcs7-mime", "application/x-pkcs7-mime");
    assert_eq!(protection(x_pkcs7.as_bytes()), Some(Protection::Pkcs7));
    assert_eq!(protection(INLINE_SIGNED), Some(Protection::InlinePgp));
    let inline_encrypted = b"Subject: x\r\n\r\n-----BEGIN PGP MESSAGE-----\r\nZHVtbXk=\r\n-----END PGP MESSAGE-----\r\n";
    assert_eq!(protection(inline_encrypted), Some(Protection::InlinePgp));
    // Armour in a body sent as base64 or quoted-printable is read once decoded.
    let base64 = b"Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\nLS0tLS1CRUdJTiBQR1AgTUVTU0FHRS0tLS0tCmFiYwo=\r\n";
    assert_eq!(protection(base64), Some(Protection::InlinePgp));
    let qp = b"Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n=2D----BEGIN PGP MESSAGE-----\r\nabc\r\n";
    assert_eq!(protection(qp), Some(Protection::InlinePgp));
}

#[test]
fn a_wrapped_body_is_looked_into() {
    // A mailing list puts the signed message first in a multipart/mixed, with its footer after.
    let wrapped = [
        &b"From: list@example.org\r\nContent-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n--outer\r\n"[..],
        b"Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=\"sig-b\"\r\n\r\n--sig-b\r\nContent-Type: text/plain\r\n\r\nx\r\n--sig-b--\r\n",
        b"--outer\r\nContent-Type: text/plain\r\n\r\nList footer\r\n--outer--\r\n",
    ]
    .concat();
    assert_eq!(protection(&wrapped), Some(Protection::Signed));
    // Plain text in an alternative with inline armour.
    let alternative = b"Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\nContent-Type: text/plain\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nx\r\n--a\r\nContent-Type: text/html\r\n\r\n<p>x</p>\r\n--a--\r\n";
    assert_eq!(protection(alternative), Some(Protection::InlinePgp));
}

#[test]
fn what_only_mentions_protection_is_not_protected() {
    // A reply that quotes armour, or a sentence that names it.
    let quoted = b"Subject: Re: x\r\n\r\nSee below.\r\n\r\n> -----BEGIN PGP MESSAGE-----\r\n> abc\r\n";
    assert_eq!(protection(quoted), None);
    let sentence = b"Subject: x\r\n\r\nPaste it after -----BEGIN PGP MESSAGE----- and you're done.\r\n";
    assert_eq!(protection(sentence), None);
    // A file `smime.p7m` sent along with ordinary mail.
    let attached = b"Content-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\nContent-Type: text/plain\r\n\r\nHere's the file.\r\n--m\r\nContent-Type: application/pkcs7-mime; name=\"smime.p7m\"\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\n\r\nZHVtbXk=\r\n--m--\r\n";
    assert_eq!(protection(attached), None);
    // Only the header of a signed message (too large to download): nothing to keep.
    let header_only = b"From: ada@example.org\r\nContent-Type: multipart/signed; boundary=\"b\"\r\n\r\n";
    assert_eq!(protection(header_only), None);
    // Armour in an HTML part is not inline PGP (nor is HTML read for it).
    let html = b"Content-Type: text/html\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n";
    assert_eq!(protection(html), None);
    assert!(!is_protected(b"Subject: plain\r\n\r\nHello.\r\n"));
    assert!(!is_protected(b""));
}

#[test]
fn a_protected_message_keeps_its_bytes_and_others_keep_none() {
    for bytes in [SMIME_SIGNED, PGP_SIGNED, PGP_ENCRYPTED, PKCS7, INLINE_SIGNED] {
        let m = parse_fetched(bytes);
        assert_eq!(m.original_bytes(), Some(bytes), "{}", String::from_utf8_lossy(bytes));
        // Everything else is read as `parse` reads it.
        assert_eq!(mailcraft_model::Message { original: None, ..m }, parse(bytes));
    }
    let plain = b"From: ada@example.org\r\nSubject: plain\r\n\r\nHello.\r\n";
    assert_eq!(parse_fetched(plain).original, None);
    assert_eq!(parse_fetched(plain), parse(plain));
}

#[test]
fn written_whole_it_is_the_original() {
    let m = parse_fetched(SMIME_SIGNED);
    for include_bcc in [false, true] {
        assert_eq!(write(&m, &WriteOptions { include_bcc, offset_minutes: 60 }), SMIME_SIGNED);
    }
    // The copy put on a server carries MailCraft's marker first, outside what is signed.
    let placed = write_placed(&m, &WriteOptions::default(), "m4rk3r");
    assert_eq!(placed, [format!("{PLACE_HEADER}: m4rk3r\r\n").as_bytes(), SMIME_SIGNED].concat());
    // That copy, come down again, keeps the original without the marker.
    assert_eq!(parse_fetched(&placed).original_bytes(), Some(SMIME_SIGNED));
    // A draft is the owner's own: it is built from its parts.
    let draft = mailcraft_model::Message { draft: true, ..m };
    assert_ne!(write(&draft, &WriteOptions::default()), SMIME_SIGNED);
}

#[test]
fn an_mbox_keeps_the_originals() {
    let mbox = [
        &b"From ada@example.org Tue Jul  1 08:52:37 2003\n"[..],
        SMIME_SIGNED,
        b"\nFrom ada@example.org Tue Jul  1 08:52:38 2003\nSubject: plain\n\nHello.\n",
    ]
    .concat();
    let got = parse_mbox(&mbox);
    assert_eq!(got.len(), 2);
    assert_eq!(got.first().and_then(|m| m.original_bytes()), Some(SMIME_SIGNED));
    assert_eq!(got.get(1).and_then(|m| m.original.as_ref()), None);
}

proptest! {
    #[test]
    fn telling_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2000)) {
        let _ = protection(&bytes);
        let m = parse_fetched(&bytes);
        if let Some(original) = m.original_bytes() {
            prop_assert!(is_protected(&bytes));
            prop_assert!(original.len() <= bytes.len());
        }
    }

    #[test]
    fn nested_text_never_panics(text in "(Content-Type: multipart/(mixed|signed|encrypted); boundary=b\r\n\r\n--b\r\n|-----BEGIN PGP (SIGNED )?MESSAGE-----\r\n|--b--\r\n|> |[a-z ]{0,6}|\r\n){0,40}") {
        let _ = protection(text.as_bytes());
        let _ = parse_fetched(text.as_bytes());
    }
}
