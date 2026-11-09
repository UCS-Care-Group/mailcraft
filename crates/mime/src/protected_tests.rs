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
\x20boundary=\"sig-b\"\r\n\
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
    let base64 = b"Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\nLS0tLS1CRUdJTiBQR1AgTUVTU0FHRS0tLS0tCmFiYwotLS0tLUVORCBQR1AgTUVTU0FHRS0tLS0tCg==\r\n";
    assert_eq!(protection(base64), Some(Protection::InlinePgp));
    let qp = b"Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n=2D----BEGIN PGP MESSAGE-----\r\nabc\r\n=2D----END PGP MESSAGE-----\r\n";
    assert_eq!(protection(qp), Some(Protection::InlinePgp));
}

#[test]
fn a_wrapped_body_is_looked_into() {
    // A mailing list puts the signed message first in a multipart/mixed, with its footer after.
    let wrapped = [
        &b"From: list@example.org\r\nContent-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n--outer\r\n"[..],
        b"Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=\"sig-b\"\r\n\r\n--sig-b\r\nContent-Type: text/plain\r\n\r\nx\r\n--sig-b\r\nContent-Type: application/pkcs7-signature\r\n\r\nZHVtbXk=\r\n--sig-b--\r\n",
        b"--outer\r\nContent-Type: text/plain\r\n\r\nList footer\r\n--outer--\r\n",
    ]
    .concat();
    assert_eq!(protection(&wrapped), Some(Protection::Signed));
    // Plain text in an alternative with inline armour.
    let alternative = b"Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\nContent-Type: text/plain\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nx\r\n-----BEGIN PGP SIGNATURE-----\r\nZHVtbXk=\r\n-----END PGP SIGNATURE-----\r\n--a\r\nContent-Type: text/html\r\n\r\n<p>x</p>\r\n--a--\r\n";
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

/// Only a well-formed structure is kept: what merely claims to be one would let any sender
/// double what the mailbox stores of their mail.
#[test]
fn what_is_not_well_formed_is_not_kept() {
    let signed = String::from_utf8_lossy(PGP_SIGNED).into_owned();
    let encrypted = String::from_utf8_lossy(PGP_ENCRYPTED).into_owned();
    let pkcs7 = String::from_utf8_lossy(PKCS7).into_owned();
    let cases = [
        // multipart/signed: no protocol, an unknown one, one the signature part doesn't match,
        // one part only, three parts, no boundary, an empty signature.
        signed.replace(" protocol=\"application/pgp-signature\";", ""),
        signed.replace("protocol=\"application/pgp-signature\"", "protocol=\"application/x-anything\""),
        signed.replace("Content-Type: application/pgp-signature; name", "Content-Type: application/octet-stream; name"),
        signed.replace("--p\r\nContent-Type: application/pgp-signature; name=\"signature.asc\"\r\n\r\n-----BEGIN PGP SIGNATURE-----\r\nZHVtbXk=\r\n-----END PGP SIGNATURE-----\r\n", ""),
        signed.replace("--p--", "--p\r\nContent-Type: text/plain\r\n\r\nmore\r\n--p--"),
        signed.replace("; boundary=\"p\"", ""),
        signed.replace("-----BEGIN PGP SIGNATURE-----\r\nZHVtbXk=\r\n-----END PGP SIGNATURE-----\r\n", "\r\n"),
        // multipart/encrypted: no or another protocol, parts of other types, one part.
        encrypted.replace(" protocol=\"application/pgp-encrypted\";", ""),
        encrypted.replace("protocol=\"application/pgp-encrypted\"", "protocol=\"application/pkcs7-mime\""),
        encrypted.replace("Content-Type: application/octet-stream", "Content-Type: text/plain"),
        encrypted.replace("--e\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n", ""),
        // application/pkcs7-mime: neither an smime-type nor base64.
        pkcs7.replace("; smime-type=enveloped-data", "").replace("Content-Transfer-Encoding: base64\r\n", ""),
        // Inline PGP: a lone line that opens armour, a signed message without its signature, an
        // END before its BEGIN.
        "Subject: x\r\n\r\n-----BEGIN PGP MESSAGE-----\r\nZHVtbXk=\r\n".into(),
        "Subject: x\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nHash: SHA256\r\n\r\nHello.\r\n".into(),
        "Subject: x\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nHello.\r\n-----END PGP SIGNATURE-----\r\n-----BEGIN PGP SIGNATURE-----\r\n".into(),
        "Subject: x\r\n\r\n-----END PGP MESSAGE-----\r\n-----BEGIN PGP MESSAGE-----\r\n".into(),
    ];
    for case in &cases {
        assert_eq!(protection(case.as_bytes()), None, "{case}");
        assert!(parse_fetched(case.as_bytes()).original.is_none(), "{case}");
    }
    // Each of those was one change from a kept message.
    for whole in [PGP_SIGNED, PGP_ENCRYPTED, PKCS7] {
        assert!(parse_fetched(whole).original.is_some());
    }
    // S/MIME's old x- type for the signature, and pkcs7-mime with only one of the two signs.
    let x_sig = String::from_utf8_lossy(SMIME_SIGNED).replace("pkcs7-signature", "x-pkcs7-signature");
    assert_eq!(protection(x_sig.as_bytes()), Some(Protection::Signed));
    assert_eq!(protection(pkcs7.replace("; smime-type=enveloped-data", "").as_bytes()), Some(Protection::Pkcs7));
    assert_eq!(protection(pkcs7.replace("Content-Transfer-Encoding: base64\r\n", "").as_bytes()), Some(Protection::Pkcs7));
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

/// The body of a message: what follows its first empty line.
fn body(bytes: &[u8]) -> &[u8] {
    bytes.windows(4).position(|w| w == b"\r\n\r\n").and_then(|at| bytes.get(at + 4..)).unwrap_or_default()
}

/// The header block of a message, up to its first empty line.
fn head(bytes: &[u8]) -> String {
    let all = String::from_utf8_lossy(bytes);
    all.split("\r\n\r\n").next().unwrap_or_default().to_string()
}

#[test]
fn written_its_body_is_the_original_and_its_header_the_message() {
    let m = parse_fetched(SMIME_SIGNED);
    for include_bcc in [false, true] {
        let out = write(&m, &WriteOptions { include_bcc, offset_minutes: 60 });
        assert_eq!(body(&out), body(SMIME_SIGNED));
        let head = head(&out);
        // The original's own content fields, folds and all, once each.
        assert!(head.contains("\r\nMIME-Version: 1.0\r\n"), "{head}");
        assert!(
            head.ends_with("Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256;\r\n boundary=\"sig-b\""),
            "{head}"
        );
        assert_eq!(head.matches("Content-Type").count(), 1, "{head}");
        assert_eq!(head.to_ascii_lowercase().matches("mime-version").count(), 1, "{head}");
        // The rest written from the message.
        assert!(head.contains("Subject: Signed") && head.contains("Message-ID: <signed-1@example.org>"), "{head}");
        // Read again, it is the same message.
        let again = parse(&out);
        assert_eq!((again.subject.as_str(), again.body.text.trim()), ("Signed", "Signed text."));
    }
    // The copy put on a server carries MailCraft's marker first.
    let placed = write_placed(&m, &WriteOptions::default(), "m4rk3r");
    assert!(placed.starts_with(format!("{PLACE_HEADER}: m4rk3r\r\n").as_bytes()));
    assert_eq!(body(&placed), body(SMIME_SIGNED));
    // That copy, come down again, keeps an original without the marker and the same body.
    let down = parse_fetched(&placed);
    let kept = down.original_bytes().unwrap();
    assert!(!String::from_utf8_lossy(kept).contains(PLACE_HEADER));
    assert_eq!(body(kept), body(SMIME_SIGNED));
    // A draft is the owner's own: it is built from its parts.
    let draft = mailcraft_model::Message { draft: true, ..m };
    assert_ne!(body(&write(&draft, &WriteOptions::default())), body(SMIME_SIGNED));
}

#[test]
fn the_owners_marks_travel_and_forged_or_bent_fields_dont_get_in() {
    let mut m = parse_fetched(SMIME_SIGNED);
    m.categories = vec!["Taxes".into()];
    m.flag = mailcraft_model::FlagStatus::Flagged;
    m.importance = mailcraft_model::Importance::High;
    m.subject = "Signed\r\nBcc: evil@example.org".into();
    let out = write(&m, &WriteOptions::default());
    let head = head(&out);
    assert!(head.contains("Keywords: Taxes"), "{head}");
    assert!(head.contains("Importance: high") && head.contains("X-Status: F"), "{head}");
    assert!(!head.contains("\r\nBcc:"), "{head}");
    assert_eq!(body(&out), body(SMIME_SIGNED));
}

#[test]
fn an_original_of_lf_lines_is_kept_and_written_in_crlf() {
    let lf: Vec<u8> = String::from_utf8_lossy(SMIME_SIGNED).replace("\r\n", "\n").into_bytes();
    let m = parse_fetched(&lf);
    assert_eq!(m.original_bytes(), Some(SMIME_SIGNED));
    let out = write_placed(&m, &WriteOptions::default(), "m");
    // Every line ends in CRLF, and the signed body is the canonical one.
    assert!(out.windows(2).all(|w| w[1] != b'\n' || w[0] == b'\r'));
    assert_eq!(body(&out), body(SMIME_SIGNED));
}

#[test]
fn crlf_and_content_of() {
    assert_eq!(crate::crlf(b"a\nb\r\nc\n\n"), b"a\r\nb\r\nc\r\n\r\n");
    assert_eq!(crate::crlf(b""), b"");
    assert_eq!(crate::crlf(b"\r\r\n"), b"\r\r\n");
    let (fields, body) = crate::protected::content_of(b"Subject: x\nContent-Type: a/b;\n\tc=d\r\nX-A: y\nMIME-Version: 1.0\0\n\nbody\n").unwrap();
    assert_eq!(fields, b"Content-Type: a/b;\r\n\tc=d\r\nMIME-Version: 1.0\r\n");
    assert_eq!(body, b"body\r\n");
    // Without a MIME-Version, one is written.
    let (fields, _) = crate::protected::content_of(b"Content-Type: a/b\r\n\r\nbody\r\n").unwrap();
    assert_eq!(fields, b"MIME-Version: 1.0\r\nContent-Type: a/b\r\n");
    // No header block, no body: only the version, and no panic.
    assert_eq!(crate::protected::content_of(b""), Some((b"MIME-Version: 1.0\r\n".to_vec(), Vec::new())));
}

/// What S/MIME programs write on enveloped and opaque-signed mail (RFC 8551 §3.2.1): the
/// message itself says it is an attachment `smime.p7m`. It is protected all the same.
#[test]
fn smime_that_says_attachment_at_the_top_is_protected() {
    for smime_type in ["enveloped-data", "signed-data"] {
        let bytes = format!(
            "From: ada@example.org\r\nMIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type={smime_type}; name=\"smime.p7m\"\r\n\
             Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\n\r\nZHVtbXk=\r\n"
        );
        assert_eq!(protection(bytes.as_bytes()), Some(Protection::Pkcs7), "{smime_type}");
        let m = parse_fetched(bytes.as_bytes());
        assert_eq!(m.original_bytes(), Some(bytes.as_bytes()));
        // Written, it says so still.
        assert!(head(&write(&m, &WriteOptions::default())).contains("Content-Disposition: attachment; filename=\"smime.p7m\""));
    }
}

/// DER sent as `binary` holds LF bytes that are data: kept and written byte for byte.
#[test]
fn a_binary_part_is_kept_byte_for_byte() {
    let der: &[u8] = b"\x30\x0a\x06\x09\n\x2a\x86\nraw\r\n\n";
    let bytes = [
        &b"From: ada@example.org\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data\r\nContent-Transfer-Encoding: binary\r\n\r\n"[..],
        der,
    ]
    .concat();
    let m = parse_fetched(&bytes);
    assert_eq!(m.original_bytes(), Some(bytes.as_slice()));
    assert_eq!(body(&write(&m, &WriteOptions::default())), der);
    // Inside a signed message, too.
    let signed = String::from_utf8_lossy(PGP_SIGNED).replace(
        "Content-Type: text/plain\r\n\r\nHello.",
        "Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: binary\r\n\r\nHel\nlo.",
    );
    let m = parse_fetched(signed.as_bytes());
    assert_eq!(m.original_bytes(), Some(signed.as_bytes()));
}

/// The signature part's type and `protocol` may differ by S/MIME's old `x-`, in case and in
/// quoting; the signed part may be empty.
#[test]
fn signed_messages_written_by_other_programs_are_told() {
    let smime = String::from_utf8_lossy(SMIME_SIGNED).into_owned();
    let x_part = smime.replace("Content-Type: application/pkcs7-signature; name", "Content-Type: application/x-pkcs7-signature; name");
    assert_eq!(protection(x_part.as_bytes()), Some(Protection::Signed));
    let x_protocol = smime.replace("protocol=\"application/pkcs7-signature\"", "protocol=application/x-pkcs7-signature");
    assert_eq!(protection(x_protocol.as_bytes()), Some(Protection::Signed));
    let upper = smime.replace("protocol=\"application/pkcs7-signature\"", "PROTOCOL=\"Application/PKCS7-Signature\"");
    assert_eq!(protection(upper.as_bytes()), Some(Protection::Signed));
    let empty = smime.replace("Content-Type: text/plain; charset=utf-8\r\n\r\nSigned text.\r\n", "Content-Type: text/plain; charset=utf-8\r\n\r\n");
    assert_eq!(protection(empty.as_bytes()), Some(Protection::Signed), "{empty}");
    // The signature itself must be there.
    let unsigned = smime.replace("ZHVtbXkgc2lnbmF0dXJl\r\n", "");
    assert_eq!(protection(unsigned.as_bytes()), None);
}

/// A content field longer than a header line may be is folded at white space; one that can't be
/// folded leaves the original out of what is written, which is then built from the parts.
#[test]
fn long_content_fields_are_folded_or_left_out() {
    let smime = String::from_utf8_lossy(SMIME_SIGNED).into_owned();
    let long = smime.replace("micalg=sha-256;", &format!("micalg=sha-256; x-a=\"{}\";", "a ".repeat(700)));
    let m = parse_fetched(long.as_bytes());
    assert!(m.has_original());
    let out = write(&m, &WriteOptions::default());
    assert!(String::from_utf8_lossy(&out).split("\r\n").all(|line| line.len() <= 998));
    assert_eq!(body(&out), body(SMIME_SIGNED));
    assert_eq!(parse(&out).body.text.trim(), "Signed text.");
    let unfoldable = smime.replace("micalg=sha-256;", &format!("micalg=sha-256; x-a={};", "a".repeat(1200)));
    let m = parse_fetched(unfoldable.as_bytes());
    let out = write(&m, &WriteOptions::default());
    assert!(String::from_utf8_lossy(&out).split("\r\n").all(|line| line.len() <= 998));
    assert!(!String::from_utf8_lossy(&out).contains(&"a".repeat(1200)));
}

/// A long field folded is the same field: the message still reads as protected, with the
/// same boundary and parameters.
#[test]
fn a_folded_content_type_says_what_it_said() {
    let smime = String::from_utf8_lossy(SMIME_SIGNED).into_owned();
    let long = smime.replace("micalg=sha-256;", &format!("micalg=sha-256; x-a=\"{}\";", "a ".repeat(700)));
    let out = write(&parse_fetched(long.as_bytes()), &WriteOptions::default());
    assert!(head(&out).split("\r\n").all(|line| line.len() <= 998));
    assert_eq!(protection(&out), Some(Protection::Signed));
    let params = |bytes: &[u8]| {
        let (headers, _) = crate::header::split(bytes, true);
        crate::header::parse_params(headers.get("content-type").unwrap())
    };
    let (before, after) = (params(long.as_bytes()), params(&out));
    assert_eq!(after.value, "multipart/signed");
    for name in ["boundary", "protocol", "micalg", "x-a"] {
        assert_eq!(after.get(name), before.get(name), "{name}");
    }
    assert_eq!(after.get("boundary"), Some("sig-b"));
}

/// A fold goes only past the field's colon, and leaves no line of white space alone: where it
/// can't, the message is written from its parts.
#[test]
fn a_fold_is_never_in_a_name_nor_of_white_space_alone() {
    let content = |field: &str| {
        let bytes = format!("{field}\r\nMIME-Version: 1.0\r\n\r\nbody\r\n");
        crate::protected::content_of(bytes.as_bytes()).map(|(fields, _)| String::from_utf8(fields).unwrap())
    };
    let lines_ok = |fields: &str| fields.split("\r\n").filter(|l| !l.is_empty()).all(|l| l.len() <= 998 && !l.trim().is_empty());
    // White space before the value, and white space at the end: folded within the value.
    let spaced = format!("Content-Type:{}text/plain; a=\"{}\"{}", " ".repeat(990), "x ".repeat(600), " ".repeat(40));
    let fields = content(&spaced).unwrap();
    assert!(lines_ok(&fields), "{fields}");
    assert!(fields.starts_with("Content-Type:"));
    // A name the colon comes far after (white space before it): nowhere to fold.
    assert_eq!(content(&format!("Content-Type{}: text/plain", " ".repeat(1200))), None);
    // White space only after the last word: no fold of it.
    assert_eq!(content(&format!("Content-Type: text/{}{}", "p".repeat(1000), " ".repeat(50))), None);
    // A fold of the original's that is long.
    let folded = format!("Content-Type: text/plain;\r\n\t{}", "a=b ".repeat(400));
    let fields = content(&folded).unwrap();
    assert!(lines_ok(&fields), "{fields}");
}

/// A lone `.asc` file sent as the message is no protection of it; an S/MIME message that says
/// `attachment` is.
#[test]
fn a_top_level_attachment_is_protected_only_as_smime() {
    let asc = b"From: ada@example.org\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"key.asc\"\r\n\r\n\
-----BEGIN PGP MESSAGE-----\r\n\r\nhQEMA\r\n-----END PGP MESSAGE-----\r\n";
    assert_eq!(protection(asc), None);
    let inline = String::from_utf8_lossy(asc).replace("Content-Disposition: attachment; filename=\"key.asc\"\r\n", "");
    assert_eq!(protection(inline.as_bytes()), Some(Protection::InlinePgp));
}

/// The sending server's DKIM and a forwarder's ARC seals cover the header as it came, which a
/// copy put on another server doesn't keep: they don't go there.
#[test]
fn a_placed_copy_carries_no_dkim_or_arc_seal() {
    let mut m = parse_fetched(SMIME_SIGNED);
    for (name, value) in [
        ("DKIM-Signature", "v=1; d=example.org; b=x"),
        ("ARC-Seal", "i=1; b=y"),
        ("ARC-Message-Signature", "i=1; b=z"),
        ("List-Id", "<l.example.org>"),
    ] {
        m.headers.push((name.into(), value.into()));
    }
    let placed = String::from_utf8_lossy(&write_placed(&m, &WriteOptions::default(), "m")).to_ascii_lowercase();
    assert!(!placed.contains("dkim-signature") && !placed.contains("arc-seal") && !placed.contains("arc-message-signature"), "{placed}");
    assert!(placed.contains("list-id"));
    // Saved as a file, the message keeps them.
    assert!(String::from_utf8_lossy(&write(&m, &WriteOptions::default())).contains("DKIM-Signature"));
}

proptest! {
    #[test]
    fn content_of_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = crate::protected::content_of(&bytes);
        let _ = crate::crlf(&bytes);
    }
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
