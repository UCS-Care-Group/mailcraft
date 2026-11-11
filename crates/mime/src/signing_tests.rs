// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message as it is sent (#30): the signed part goes out byte for byte, and
//! the message keeps its body whatever path writes it.

use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, Message, Original};
use proptest::prelude::*;

use crate::{LayoutError, SignatureKind, SignedKind, WriteOptions, multipart_signed, signable, signed_parts, write};

const ARMOURED: &str = "-----BEGIN PGP SIGNATURE-----\n\niQEzBAABCAAdFiEE\n=abcd\n-----END PGP SIGNATURE-----\n";

fn message() -> Message {
    Message {
        from: Address::new("Ada", "ada@example.org"),
        to: vec![Address::new("Bob", "bob@example.org")],
        bcc: vec![Address::new("", "carol@example.org")],
        subject: "Plans".into(),
        message_id: "m1@example.org".into(),
        body: Body { text: "Hello Bob,\n\nSee you at ten.\n".into(), html: Some("<p>Hello Bob,</p>".into()) },
        attachments: vec![Attachment {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            data: Arc::new(b"notes".to_vec()),
            ..Attachment::default()
        }],
        ..Message::default()
    }
}

/// `message` with its body the signed layout of itself, as the host makes it.
fn signed(message: &Message, kind: SignatureKind) -> (Vec<u8>, Message) {
    let entity = signable(message);
    let (micalg, signature) = match kind {
        SignatureKind::Smime => ("sha-256", vec![0x30, 0x82, 0x01, 0x00, 0xAA, 0xBB]),
        SignatureKind::OpenPgp => ("pgp-sha256", ARMOURED.as_bytes().to_vec()),
    };
    let body = multipart_signed(&entity, kind, micalg, &signature).unwrap_or_else(|e| panic!("{e}"));
    let going = Message { original: Original::new(body), ..message.clone() };
    (entity, going)
}

#[test]
fn the_signed_part_goes_out_byte_for_byte_with_its_signature() {
    for kind in [SignatureKind::Smime, SignatureKind::OpenPgp] {
        let (entity, going) = signed(&message(), kind);
        let sent = write(&going, &WriteOptions::default());
        let parts = signed_parts(&sent).unwrap_or_else(|| panic!("signed"));
        assert_eq!(parts.content, entity, "{kind:?}");
        assert!(parts.whole && parts.one_from);
        match kind {
            SignatureKind::Smime => {
                assert_eq!(parts.kind, SignedKind::SmimeDetached);
                assert_eq!(parts.signature, vec![0x30, 0x82, 0x01, 0x00, 0xAA, 0xBB]);
            }
            SignatureKind::OpenPgp => {
                assert_eq!(parts.kind, SignedKind::PgpDetached);
                let text = String::from_utf8_lossy(&parts.signature);
                assert!(text.starts_with("-----BEGIN PGP SIGNATURE-----\r\n") && text.contains("=abcd"), "{text}");
            }
        }
        // The headers are written from the message, the Bcc only in the sender's own copy; the
        // body is the same in both.
        let copy = write(&going, &WriteOptions { include_bcc: true, offset_minutes: 0 });
        let body_of = |b: &[u8]| {
            let at = b.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0);
            b[at..].to_vec()
        };
        assert_eq!(body_of(&sent), body_of(&copy));
        assert!(String::from_utf8_lossy(&copy).contains("Bcc: carol@example.org"));
        assert!(!String::from_utf8_lossy(&sent).contains("carol@example.org"));
    }
}

#[test]
fn what_is_signed_is_the_body_as_it_would_be_written() {
    let m = message();
    let entity = String::from_utf8(signable(&m)).unwrap_or_default();
    assert!(entity.starts_with("Content-Type: multipart/mixed;"), "{entity}");
    assert!(entity.contains("Hello Bob,\r\n\r\nSee you at ten.\r\n"), "{entity}");
    assert!(!entity.ends_with("\r\n"), "the line end before the boundary is the boundary's");
    // The same message always gives the same bytes.
    assert_eq!(signable(&m), signable(&m));
    // Every byte 7-bit, every line ending in CRLF.
    let bytes = signable(&Message { body: Body { text: "Grüße\n".into(), html: None }, ..m });
    assert!(bytes.iter().all(|b| *b < 0x80));
    assert!(!bytes.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'));
}

#[test]
fn text_a_server_could_change_is_quoted_printable_when_signed() {
    for text in ["trailing space \nnext", "From the start\n", "tab\t\nnext", "one\n.\ntwo"] {
        let m = Message { body: Body { text: text.into(), html: None }, ..Message::default() };
        let entity = String::from_utf8(signable(&m)).unwrap_or_default();
        assert!(entity.contains("Content-Transfer-Encoding: quoted-printable"), "{text:?}: {entity}");
        assert!(
            !entity.contains(" \r\n") && !entity.contains("\t\r\n") && !entity.contains("\r\nFrom ") && !entity.contains("\r\n.\r\n"),
            "{entity:?}"
        );
        // Unsigned, plain ASCII text stays 7bit.
        let plain = String::from_utf8_lossy(&write(&m, &WriteOptions::default())).to_string();
        assert!(plain.contains("Content-Transfer-Encoding: 7bit"), "{plain}");
    }
    // An attached message with white space at a line's end goes as base64 when signed.
    let attached = Message {
        attachments: vec![Attachment {
            name: "m.eml".into(),
            mime: "message/rfc822".into(),
            data: Arc::new(b"Subject: x\r\n\r\nend \r\n".to_vec()),
            ..Attachment::default()
        }],
        ..Message::default()
    };
    let entity = String::from_utf8(signable(&attached)).unwrap_or_default();
    assert!(entity.contains("message/rfc822") && entity.contains("Content-Transfer-Encoding: base64"), "{entity}");
}

#[test]
fn only_sha2_micalgs_and_armoured_signatures_are_written() {
    let entity = signable(&message());
    assert_eq!(multipart_signed(&entity, SignatureKind::Smime, "sha1", b"x"), Err(LayoutError::Micalg));
    assert_eq!(multipart_signed(&entity, SignatureKind::Smime, "md5", b"x"), Err(LayoutError::Micalg));
    assert_eq!(multipart_signed(&entity, SignatureKind::Smime, "pgp-sha256", b"x"), Err(LayoutError::Micalg));
    assert_eq!(multipart_signed(&entity, SignatureKind::OpenPgp, "sha-256", ARMOURED.as_bytes()), Err(LayoutError::Micalg));
    assert_eq!(multipart_signed(&entity, SignatureKind::OpenPgp, "pgp-sha256", b"\x89\x01binary"), Err(LayoutError::NotArmoured));
    assert_eq!(
        multipart_signed(&entity, SignatureKind::OpenPgp, "pgp-sha256", b"-----BEGIN PGP SIGNATURE-----\n\x07\n-----END PGP SIGNATURE-----"),
        Err(LayoutError::NotArmoured)
    );
    assert_eq!(multipart_signed(b"", SignatureKind::Smime, "sha-256", b"x"), Err(LayoutError::Empty));
    assert_eq!(multipart_signed(&entity, SignatureKind::Smime, "sha-256", b""), Err(LayoutError::Empty));
}

#[test]
fn the_boundary_is_in_neither_part() {
    // A signed part that holds the boundary the hash would give first moves it on.
    let first = {
        let body = multipart_signed(b"x", SignatureKind::Smime, "sha-256", b"y").unwrap_or_default();
        let text = String::from_utf8_lossy(&body).to_string();
        let at = text.find("boundary=\"").map(|a| a + 10).unwrap_or(0);
        text[at..].split('"').next().unwrap_or_default().to_string()
    };
    assert!(first.starts_with("=_mc_signed_"), "{first}");
    let tricky = format!("x{first}");
    let body = multipart_signed(tricky.as_bytes(), SignatureKind::Smime, "sha-256", b"y").unwrap_or_default();
    let parts = signed_parts(&body).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.content, tricky.as_bytes());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// Whatever the text and files, what is signed comes back byte for byte from what is sent.
    #[test]
    fn any_message_signed_reads_back_as_signed(text in ".{0,300}", file in proptest::collection::vec(any::<u8>(), 0..300)) {
        let m = Message {
            from: Address::new("", "ada@example.org"),
            body: Body { text, html: None },
            attachments: vec![Attachment { name: "f.bin".into(), mime: "application/octet-stream".into(), data: Arc::new(file), ..Attachment::default() }],
            ..Message::default()
        };
        let (entity, going) = signed(&m, SignatureKind::Smime);
        let sent = write(&going, &WriteOptions::default());
        let parts = signed_parts(&sent);
        prop_assert!(parts.is_some());
        prop_assert_eq!(parts.map(|p| p.content).unwrap_or_default(), entity);
    }
}
