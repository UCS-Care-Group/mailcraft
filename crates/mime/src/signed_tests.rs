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

/// The second part of `multipart/signed` is its signature only when it is of the type the
/// `protocol` names: a `text/html` part carrying CMS is a part of the message, shown as one.
#[test]
fn a_second_part_of_another_type_is_no_signature() {
    let html = String::from_utf8_lossy(DETACHED).replacen("Content-Type: application/pkcs7-signature", "Content-Type: text/html", 1);
    assert!(html.contains("Content-Type: text/html"), "the fixture's signature part was renamed");
    assert!(signed_parts(html.as_bytes()).is_none());
    let pgp = b"Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhi\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>other</p>\r\n--b--\r\n";
    assert!(signed_parts(pgp).is_none());
}

#[test]
fn several_senders_are_told() {
    let both = String::from_utf8_lossy(DETACHED).replacen("From: alice@smime.example", "From: alice@smime.example, mallory@example.org", 1);
    assert!(!signed_parts(both.as_bytes()).unwrap_or_else(|| panic!("signed")).one_from);
    let twice = String::from_utf8_lossy(DETACHED).replacen("From: alice@smime.example", "From: alice@smime.example\r\nFrom: mallory@example.org", 1);
    assert!(!signed_parts(twice.as_bytes()).unwrap_or_else(|| panic!("signed")).one_from);
}

const OPAQUE_FILES: &[u8] = include_bytes!("../../smime/testdata/opaque-files.eml");
const OPAQUE_BER: &[u8] = include_bytes!("../../smime/testdata/opaque-ber.eml");

/// An opaque signed message is read as what it holds: its text the body, its files after the
/// `smime.p7m` it came as (#30).
#[test]
fn an_opaque_signed_message_is_read_as_what_it_holds() {
    let m = crate::parse(OPAQUE_FILES);
    assert!(m.body.text.contains("The signed minutes are attached."), "{:?}", m.body.text);
    let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["smime.p7m", "minutes.pdf"]);
    assert!(crate::parse(OPAQUE_BER).body.text.contains("Hello Bob"), "BER, in pieces");
}

/// A signed message sent along as a file isn't opened into the message carrying it, and an
/// Encrypted one, or what isn't CMS at all, is a file as it was.
#[test]
fn a_signed_message_sent_as_a_file_or_enveloped_data_isnt_read_into_the_message() {
    let opaque = String::from_utf8_lossy(OPAQUE).into_owned();
    let (head, body) = opaque.split_once("\r\n\r\n").unwrap_or_default();
    let part_head: String = head.lines().filter(|l| l.starts_with("Content-")).map(|l| format!("{l}\r\n")).collect();
    let carried = format!(
        "From: ada@example.org\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee the file.\r\n--b\r\n{part_head}\r\n{body}\r\n--b--\r\n"
    );
    let m = crate::parse(carried.as_bytes());
    assert_eq!(m.body.text, "See the file.");
    assert_eq!(m.attachments.len(), 1);
    let enveloped = "From: ada@example.org\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data; name=smime.p7m\r\nContent-Transfer-Encoding: base64\r\n\r\nMA8GCSqGSIb3DQEHA6ACMAA=\r\n";
    let m = crate::parse(enveloped.as_bytes());
    assert!(m.body.text.is_empty());
    assert_eq!(m.attachments.len(), 1);
}

/// A DER element of `tag` holding `content`.
fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    match n {
        0..0x80 => out.push(n as u8),
        0x80..0x100 => out.extend([0x81, n as u8]),
        0x100..0x10000 => out.extend([0x82, (n >> 8) as u8, n as u8]),
        _ => out.extend([0x83, (n >> 16) as u8, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(content);
    out
}

/// Signed data holding `content` (no signer: nothing here checks).
fn signed_data(content: &[u8]) -> Vec<u8> {
    let data = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
    let signed = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    let encap = tlv(0x30, &[&data[..], &tlv(0xa0, &tlv(0x04, content))].concat());
    let sd = tlv(0x30, &[&tlv(0x02, &[1])[..], &tlv(0x31, &[]), &encap, &tlv(0x31, &[])].concat());
    tlv(0x30, &[&signed[..], &tlv(0xa0, &sd)].concat())
}

/// Signed data inside signed data opens once: the rest stays one file, so that nesting can't make
/// a message many times the size it came as (#30 review 3).
#[test]
fn signed_data_inside_signed_data_opens_once() {
    let mut entity = [&b"Content-Type: text/plain\r\n\r\n"[..], &vec![b'x'; 200_000], b"\r\n"].concat();
    for _ in 0..30 {
        entity = [
            &b"Content-Type: application/pkcs7-mime; smime-type=signed-data\r\nContent-Transfer-Encoding: binary\r\n\r\n"[..],
            &signed_data(&entity),
        ]
        .concat();
    }
    let m = crate::parse(&entity);
    let kept: usize = m.attachments.iter().map(|a| a.data.len()).sum();
    assert!(kept <= 2 * entity.len(), "{kept} bytes kept of {} that came", entity.len());
    assert_eq!(m.attachments.len(), 2, "the outer file, and the one inside it");
    let shallow = [&b"Content-Type: application/pkcs7-mime\r\n\r\n"[..], &signed_data(b"Content-Type: text/plain\r\n\r\nnear\r\n")].concat();
    assert_eq!(crate::parse(&shallow).body.text.trim_end(), "near");
}

/// An opaque signed part larger than the bytes opened from one message stays a file, unopened
/// (#30 review 4; the real limit is 64 MB).
#[test]
fn signed_data_past_the_byte_limit_stays_a_file() {
    let entity = [&b"Content-Type: application/pkcs7-mime\r\n\r\n"[..], &signed_data(b"Content-Type: text/plain\r\n\r\nnear\r\n")].concat();
    let opened = crate::parse::parse_opening(&entity, 1024);
    assert_eq!(opened.body.text.trim_end(), "near");
    let shut = crate::parse::parse_opening(&entity, 16);
    assert_eq!(shut.body.text.trim(), "");
    assert_eq!(shut.attachments.len(), 1);
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
