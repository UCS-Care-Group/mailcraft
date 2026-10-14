use mailcraft_model::{Address, Attachment, Body};
use proptest::prelude::*;

use super::*;

fn message() -> Message {
    Message {
        from: Address { name: "Ada".into(), email: "ada@example.org".into() },
        to: vec![Address { name: String::new(), email: "bea@example.org".into() }],
        subject: "Figures".into(),
        body: Body { text: "Line one\nLine two  \n".into(), html: None },
        attachments: vec![Attachment::new("a.bin", "application/octet-stream", vec![0, 1, 2, 255])],
        ..Default::default()
    }
}

#[test]
fn a_signed_message_round_trips_and_splits_exactly() {
    let m = message();
    let entity = content_entity(&m);
    assert!(!entity.windows(2).any(|w| w == b"\n\n"), "CRLF throughout");
    let secured = wrap_pgp_signed(&entity, "-----BEGIN PGP SIGNATURE-----\nAAAA\n-----END PGP SIGNATURE-----\n");
    let parts = signed_parts(&secured.raw).unwrap();
    assert_eq!(parts.content, entity, "what is split off is exactly what was signed");
    assert!(parts.signature.starts_with(b"-----BEGIN PGP SIGNATURE-----"));
    let sent = Message { secured: Some(secured.clone()), ..m.clone() };
    let wire = crate::write(&sent, &crate::WriteOptions::default());
    let back = crate::parse(&wire);
    assert_eq!(back.secured.as_ref().map(|s| s.kind), Some(Protection::PgpSigned));
    assert_eq!(back.body.text, "Line one\nLine two  \n");
    // The signature is not a file; the real attachment is.
    assert_eq!(back.attachments.len(), 1);
    assert_eq!(back.attachments[0].name, "a.bin");
    assert_eq!(signed_parts(&back.secured.unwrap().raw).unwrap().content, entity);

    let smime = wrap_smime_signed(&entity, &[0x30, 0x03, 0x02, 0x01, 0x01]);
    let parts = signed_parts(&smime.raw).unwrap();
    assert_eq!(parts.content, entity);
    assert_eq!(parts.signature, [0x30, 0x03, 0x02, 0x01, 0x01]);
}

#[test]
fn encrypted_messages_keep_their_payload_and_show_nothing() {
    let env = wrap_smime_encrypted(&[1, 2, 3, 4, 5]);
    let m = Message { secured: Some(env), ..message() };
    let back = crate::parse(&crate::write(&m, &crate::WriteOptions::default()));
    assert_eq!(back.secured.as_ref().map(|s| s.kind), Some(Protection::SmimeEncrypted));
    assert!(back.body.text.is_empty() && back.attachments.is_empty());
    assert_eq!(encrypted_payload(&back.secured.unwrap().raw).unwrap(), [1, 2, 3, 4, 5]);

    let pgp = wrap_pgp_encrypted("-----BEGIN PGP MESSAGE-----\nAAAA\n-----END PGP MESSAGE-----\n");
    let m = Message { secured: Some(pgp), ..message() };
    let back = crate::parse(&crate::write(&m, &crate::WriteOptions::default()));
    assert_eq!(back.secured.as_ref().map(|s| s.kind), Some(Protection::PgpEncrypted));
    let payload = encrypted_payload(&back.secured.unwrap().raw).unwrap();
    assert!(payload.starts_with(b"-----BEGIN PGP MESSAGE-----"));
}

#[test]
fn what_is_inside_can_not_say_who_it_is_from() {
    let inner = b"From: mallory@example.org\r\nSubject: Not this\r\nContent-Type: text/plain\r\n\r\nHello";
    let m = read_entity(inner);
    assert_eq!(m.body.text, "Hello");
    assert!(m.from.email.is_empty() && m.subject.is_empty());
}

#[test]
fn inline_blocks_are_found_at_the_start_of_a_line() {
    let text = "Hi\n-----BEGIN PGP MESSAGE-----\nabc\n-----END PGP MESSAGE-----\nbye";
    let (kind, block) = inline_pgp(text).unwrap();
    assert_eq!(kind, InlinePgp::Encrypted);
    assert!(block.starts_with("-----BEGIN") && block.ends_with("-----END PGP MESSAGE-----"));
    assert!(inline_pgp("quoted: -----BEGIN PGP MESSAGE-----").is_none());
    assert!(inline_pgp("plain").is_none());
}

#[test]
fn protection_is_read_from_the_top_level_only() {
    for (ct, want) in [
        ("multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=b", Some(Protection::SmimeSigned)),
        ("multipart/signed; protocol=\"application/x-pkcs7-signature\"; boundary=b", Some(Protection::SmimeSigned)),
        ("multipart/signed; protocol=\"application/pgp-signature\"; boundary=b", Some(Protection::PgpSigned)),
        ("multipart/signed; protocol=\"application/unknown\"; boundary=b", None),
        ("application/pkcs7-mime; smime-type=signed-data", Some(Protection::SmimeSignedData)),
        ("application/x-pkcs7-mime; name=smime.p7m", Some(Protection::SmimeEncrypted)),
        ("application/pkcs7-mime; smime-type=certs-only", None),
        ("multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b", Some(Protection::PgpEncrypted)),
        ("text/plain", None),
    ] {
        let m = crate::parse(format!("Content-Type: {ct}\r\n\r\nbody").as_bytes());
        assert_eq!(m.secured.map(|s| s.kind), want, "{ct}");
    }
    // Inside a multipart/mixed, a signed part doesn't make the message signed.
    let nested = "Content-Type: multipart/mixed; boundary=o\r\n\r\n--o\r\nContent-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=b\r\n\r\n--b\r\nhi\r\n--b--\r\n--o--\r\n";
    assert!(crate::parse(nested.as_bytes()).secured.is_none());
}

proptest! {
    #[test]
    fn arbitrary_entities_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1500)) {
        let _ = signed_parts(&bytes);
        let _ = encrypted_payload(&bytes);
        let _ = read_entity(&bytes);
        let _ = inline_pgp(&String::from_utf8_lossy(&bytes));
        let mut top = b"Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=x\r\n\r\n--x\r\n".to_vec();
        top.extend_from_slice(&bytes);
        let m = crate::parse(&top);
        if let Some(s) = &m.secured {
            let _ = signed_parts(&s.raw);
        }
    }
}
