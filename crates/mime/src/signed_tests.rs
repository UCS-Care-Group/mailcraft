// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The halves of a Digitally signed message (#30): the signed part exactly as it came, and the
//! signature, for S/MIME and PGP/MIME, detached and opaque, and never a panic on any bytes.

use proptest::prelude::*;

use crate::{SignedKind, signed_parts};

const DETACHED: &[u8] = include_bytes!("../../smime/testdata/detached.eml");
const OPAQUE: &[u8] = include_bytes!("../../smime/testdata/opaque.eml");

#[test]
fn a_detached_signature_gives_the_signed_part_byte_for_byte() {
    let parts = signed_parts(DETACHED).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.kind, SignedKind::SmimeDetached);
    let content = String::from_utf8_lossy(&parts.content);
    assert!(content.starts_with("Content-Type: text/plain; charset=us-ascii\r\n\r\nHello Bob,"), "{content}");
    assert!(content.ends_with("Alice\r\n"), "the CRLF before the boundary is the boundary's: {content:?}");
    // The CMS, decoded: a SEQUENCE.
    assert_eq!(parts.signature.first(), Some(&0x30));
    assert!(parts.whole, "the message's top level");
    assert!(parts.one_from);
}

#[test]
fn an_opaque_signature_gives_its_cms() {
    let parts = signed_parts(OPAQUE).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.kind, SignedKind::SmimeOpaque);
    assert!(parts.content.is_empty());
    assert_eq!(parts.signature.first(), Some(&0x30));
}

#[test]
fn pgp_mime_is_told_and_other_mail_is_not_signed() {
    let pgp = b"From: a@example.org\r\nContent-Type: multipart/signed; protocol=\"application/pgp-signature\"; micalg=pgp-sha256; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhi\r\n--b\r\nContent-Type: application/pgp-signature\r\n\r\n-----BEGIN PGP SIGNATURE-----\r\nx\r\n-----END PGP SIGNATURE-----\r\n--b--\r\n";
    let parts = signed_parts(pgp).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.kind, SignedKind::PgpDetached);
    assert_eq!(parts.content, b"Content-Type: text/plain\r\n\r\nhi");
    assert!(signed_parts(b"From: a@example.org\r\nContent-Type: text/plain\r\n\r\nhi\r\n").is_none());
    // A signature sent along as a file isn't the message's.
    let attached = b"Content-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\nContent-Type: text/plain\r\n\r\nhi\r\n--m\r\nContent-Type: application/pkcs7-mime; smime-type=signed-data\r\nContent-Disposition: attachment; filename=smime.p7m\r\nContent-Transfer-Encoding: base64\r\n\r\nMAA=\r\n--m--\r\n";
    assert!(signed_parts(attached).is_none());
    // One part only, or three: not RFC 1847's two.
    let one = b"Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=b\r\n\r\n--b\r\n\r\nhi\r\n--b--\r\n";
    assert!(signed_parts(one).is_none());
}

#[test]
fn a_signed_message_inside_a_list_wrapping_is_found() {
    let inner = String::from_utf8_lossy(DETACHED).into_owned();
    let wrapped = format!(
        "From: list@example.org\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{inner}\r\n--outer\r\nContent-Type: text/plain\r\n\r\nfooter\r\n--outer--\r\n"
    );
    let parts = signed_parts(wrapped.as_bytes()).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.kind, SignedKind::SmimeDetached);
    assert!(!parts.whole, "the footer beside it isn't signed");
}

/// A signed first version beside an unsigned other one (an HTML part saying something else) is
/// not looked into: the other version is the one shown as often, and nobody signed it.
#[test]
fn a_signed_part_in_an_alternative_is_not_the_messages() {
    let inner = String::from_utf8_lossy(DETACHED).into_owned();
    let alternative = format!(
        "From: alice@smime.example\r\nContent-Type: multipart/alternative; boundary=alt\r\n\r\n--alt\r\n{inner}\r\n--alt\r\nContent-Type: text/html\r\n\r\n<p>Wire 10000 EUR to Mallory</p>\r\n--alt--\r\n"
    );
    assert!(signed_parts(alternative.as_bytes()).is_none());
    // Nor one inside an alternative inside a wrapper.
    let mixed = format!("Content-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\n{alternative}\r\n--m--\r\n");
    assert!(signed_parts(mixed.as_bytes()).is_none());
}

#[test]
fn several_senders_are_told() {
    let both = String::from_utf8_lossy(DETACHED).replacen("From: alice@smime.example", "From: alice@smime.example, mallory@example.org", 1);
    assert!(!signed_parts(both.as_bytes()).unwrap_or_else(|| panic!("signed")).one_from);
    let twice = String::from_utf8_lossy(DETACHED).replacen("From: alice@smime.example", "From: alice@smime.example\r\nFrom: mallory@example.org", 1);
    assert!(!signed_parts(twice.as_bytes()).unwrap_or_else(|| panic!("signed")).one_from);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = signed_parts(&bytes);
    }

    #[test]
    fn a_damaged_signed_message_never_panics(cut in 0usize..3000, flip in 0usize..3000, byte in any::<u8>()) {
        let mut bytes = DETACHED.to_vec();
        if let Some(b) = bytes.get_mut(flip) {
            *b = byte;
        }
        bytes.truncate(cut);
        let _ = signed_parts(&bytes);
    }
}
