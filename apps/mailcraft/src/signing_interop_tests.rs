// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures made at Send (#30 part 5), checked by verifiers that aren't MailCraft's,
//! on the bytes that go out: OpenPGP by rPGP's own verify, S/MIME by RustCrypto's CMS reader and
//! ECDSA. The signed part is taken out of what was sent as RFC 1847 and RFC 2046 say (not by
//! MailCraft's reader), both as it went and as Microsoft 365 writes it again on its way, which
//! gives a part of a `multipart/signed` that doesn't end with a line end one: a signed part that
//! didn't was changed under its signature, and every PGP/MIME signature sent through it checked
//! BAD in GnuPG. Two tests that are ignored check the same with GnuPG and OpenSSL themselves.
//!
//! Every key, certificate, name and message is made here; nothing leaves this machine but for
//! the stand-in for Microsoft Graph on loopback.

use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::asn1::OctetString;
use der::{Decode, Encode};
use mailcraft_mime::SignatureKind;
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use mailcraft_model::{Address, Attachment, Body, Message};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::signature::hazmat::PrehashSigner;
use pgp::composed::{Deserializable, DetachedSignature, KeyType, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey};
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, Password, SignatureBytes, SigningKey as _};
use rand::SeedableRng;
use sha2::Digest;

use crate::signing::{Host, sign, sign_by};

const NOW: i64 = 1_794_441_600;
/// The messageDigest attribute (RFC 5652 §11.2).
const MESSAGE_DIGEST: der::oid::ObjectIdentifier = der::oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");

/// The forms of message signed: text and HTML, as the owner's was (`multipart/alternative`);
/// with a file besides (`multipart/mixed`); and text alone (`text/plain` is the signed part),
/// ending with a line end, without one, and with a blank line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Alternative,
    Mixed,
    TextEndingWithALineEnd,
    TextWithout,
    TextEndingWithABlankLine,
}

impl Shape {
    const ALL: [Shape; 5] = [Shape::Alternative, Shape::Mixed, Shape::TextEndingWithALineEnd, Shape::TextWithout, Shape::TextEndingWithABlankLine];
}

fn message(format: SignatureFormat, fingerprint: &str, shape: Shape) -> Message {
    let text = "\n\nRegards,\nOwner\nMobile: +44 0000 000 000\n\nNot always at my desk.";
    let (text, html) = match shape {
        Shape::Alternative | Shape::Mixed => (text.to_string(), Some("<div><br></div><div>Regards,</div><div>Owner</div>".to_string())),
        Shape::TextEndingWithALineEnd => (format!("{text}\n"), None),
        Shape::TextWithout => (text.to_string(), None),
        Shape::TextEndingWithABlankLine => (format!("{text}\n\n"), None),
    };
    let attachments = if shape == Shape::Mixed {
        vec![Attachment {
            name: "rota.txt".into(),
            mime: "text/plain".into(),
            data: std::sync::Arc::new(b"Mon: Kit\n".to_vec()),
            ..Default::default()
        }]
    } else {
        Vec::new()
    };
    Message {
        id: 9,
        from: Address::new("Owner", "owner@example.org"),
        to: vec![Address::new("Kit", "kit@example.net")],
        subject: "Rota".into(),
        date: NOW,
        message_id: "rota.9@example.org".into(),
        body: Body { text, html },
        attachments,
        sign_with: SigningKey::new(format, fingerprint, "owner@example.org"),
        ..Message::default()
    }
}

// ---- what went, read as the RFCs say ----

/// The value of `name` in the top `Content-Type` of `wire`.
fn top_param(wire: &[u8], name: &str) -> String {
    let text = String::from_utf8_lossy(wire);
    let header = text.split("\r\n\r\n").next().unwrap_or_default().replace("\r\n ", " ").replace("\r\n\t", " ");
    let content_type =
        header.lines().find(|l| l.to_ascii_lowercase().starts_with("content-type:")).unwrap_or_else(|| panic!("no Content-Type: {header}"));
    let at = content_type.find(&format!("{name}=")).unwrap_or_else(|| panic!("no {name}: {content_type}")) + name.len() + 1;
    content_type[at..].trim_start_matches('"').split(['"', ';']).next().unwrap_or_default().to_string()
}

/// The header block of `wire` and the body parts of its top `multipart/signed`, each as the
/// bytes between its boundary line and the CRLF before the next delimiter (RFC 2046 §5.1.1,
/// RFC 1847 §2.1), and its boundary.
fn parts(wire: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>, String) {
    let boundary = top_param(wire, "boundary");
    let split = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no header end")) + 2;
    let (header, body) = wire.split_at(split);
    // The first delimiter may open the body: it, too, is preceded by a line end, the header's.
    let body = [b"\r\n".as_slice(), body.get(2..).unwrap_or_default()].concat();
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut at = Vec::new();
    let mut i = 0;
    while let Some(found) = body.get(i..).and_then(|rest| rest.windows(delimiter.len()).position(|w| w == delimiter.as_slice())) {
        at.push(i + found);
        i += found + delimiter.len();
    }
    let mut out = Vec::new();
    for pair in at.windows(2) {
        let line = &body[pair[0] + delimiter.len()..pair[1]];
        let start = line.windows(2).position(|w| w == b"\r\n").unwrap_or_else(|| panic!("no boundary line end")) + 2;
        out.push(line[start..].to_vec());
    }
    assert!(body[*at.last().unwrap_or(&0) + delimiter.len()..].starts_with(b"--"), "the close delimiter is last");
    (header.to_vec(), out, boundary)
}

/// `wire` as Microsoft 365 writes a `multipart/signed` message again on its way (seen on a
/// message sent through it, #30): the preamble left out, and a line end given to each part that
/// doesn't end with one. The header is left as it was (Microsoft folds it again too, which no
/// signature covers).
fn as_microsoft_writes_it(wire: &[u8]) -> Vec<u8> {
    written_again(wire, |part| {
        if !part.ends_with(b"\r\n") {
            part.extend_from_slice(b"\r\n");
        }
    })
}

/// `wire` as a server might write it again that trims the line ends at a part's end to one (a
/// model, not seen: it is what a part with one line end at its end is safe from).
fn as_a_trimming_server_writes_it(wire: &[u8]) -> Vec<u8> {
    written_again(wire, |part| {
        while part.ends_with(b"\r\n") {
            part.truncate(part.len() - 2);
        }
        part.extend_from_slice(b"\r\n");
    })
}

/// `wire` with its preamble left out and each part of its top `multipart/signed` as `each`
/// makes it.
fn written_again(wire: &[u8], each: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let (header, parts, boundary) = parts(wire);
    let mut out = header;
    out.extend_from_slice(b"\r\n");
    for mut part in parts {
        each(&mut part);
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(&part);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// The body of a part, after its header block.
fn body_of(part: &[u8]) -> &[u8] {
    let at = part.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no part header end"));
    &part[at + 4..]
}

/// What goes over SMTP (and in base64 to Graph's `sendMail`, and in EWS's `MimeContent`), and
/// what Graph's stand-in was handed.
fn what_goes(m: &Message) -> Vec<Vec<u8>> {
    let smtp = mailcraft_mime::write(m, &mailcraft_mime::WriteOptions { include_bcc: false, offset_minutes: 0 });
    let server = mailcraft_net::graph::standin::StandIn::start().unwrap();
    let _route = mailcraft_net::graph::standin::route(&server.url());
    mailcraft_net::graph::send(mailcraft_net::graph::standin::TOKEN, m, &mailcraft_net::graph::SendOptions { offset_minutes: Some(0) }).unwrap();
    let graph = server.state().sent.first().map(|s| s.mime.clone()).unwrap_or_default();
    assert!(!graph.is_empty(), "Graph was sent the message");
    vec![smtp, graph]
}

/// Every form `m` may arrive in: as each path sends it, that as Microsoft 365 writes it, and
/// that as a server that trims line ends would.
fn as_it_arrives(m: &Message) -> Vec<Vec<u8>> {
    what_goes(m).into_iter().flat_map(|w| [as_microsoft_writes_it(&w), as_a_trimming_server_writes_it(&w), w]).collect()
}

// ---- OpenPGP, checked by rPGP ----

fn pgp_key() -> SignedSecretKey {
    let mut params = SecretKeyParamsBuilder::default();
    params.key_type(KeyType::Ed25519Legacy).can_certify(true).can_sign(true).primary_user_id("Owner <owner@example.org>".into());
    params.build().unwrap().generate(rand::rngs::StdRng::seed_from_u64(30)).unwrap()
}

/// Signs as Send does with `secret` in place of gpg-agent: the holder is handed the digest and
/// gives back what `PKSIGN` would.
fn pgp_signed(secret: &SignedSecretKey, shape: Shape) -> Message {
    let public = SignedPublicKey::from(secret.clone());
    let keys = mailcraft_openpgp::signing_keys(&public.to_bytes().unwrap(), 4_102_444_800);
    let key = keys.first().unwrap_or_else(|| panic!("the key signs"));
    let mut m = message(SignatureFormat::OpenPgp, &key.primary().to_hex(), shape);
    let created = secret.primary_key.created_at().as_secs() + 60;
    sign_by(&mut m, &|_, entity| {
        let armoured = mailcraft_openpgp::sign_detached(entity, key, created, mailcraft_openpgp::HashAlgorithm::Sha256, |digest| {
            Ok::<_, String>(match secret.primary_key.sign(&Password::empty(), pgp::crypto::hash::HashAlgorithm::Sha256, digest).unwrap() {
                SignatureBytes::Mpis(v) => mailcraft_openpgp::SignatureValue::Ecc { r: v[0].as_ref().to_vec(), s: v[1].as_ref().to_vec() },
                SignatureBytes::Native(b) => mailcraft_openpgp::SignatureValue::Ecc { r: b[..32].to_vec(), s: b[32..].to_vec() },
            })
        })
        .map_err(|e| e.to_string())?;
        Ok((SignatureKind::OpenPgp, "pgp-sha256", armoured.into_bytes()))
    })
    .unwrap();
    m
}

#[test]
fn an_openpgp_signature_checks_in_rpgp_on_what_goes_and_what_arrives() {
    let secret = pgp_key();
    let public = SignedPublicKey::from(secret.clone());
    for shape in Shape::ALL {
        let m = pgp_signed(&secret, shape);
        for wire in as_it_arrives(&m) {
            assert_eq!(top_param(&wire, "protocol"), "application/pgp-signature");
            let (_, parts, _) = parts(&wire);
            let [signed, signature] = parts.as_slice() else { panic!("two parts: {}", String::from_utf8_lossy(&wire)) };
            let armour = String::from_utf8(body_of(signature).to_vec()).unwrap();
            let (detached, _) = DetachedSignature::from_string(&armour).unwrap();
            detached.verify(&public.primary_key, signed).unwrap_or_else(|e| panic!("BAD ({e}) over {:?}", String::from_utf8_lossy(signed)));
            assert_eq!(detached.signature.typ(), Some(pgp::packet::SignatureType::Binary));
            assert_eq!(detached.signature.issuer_fingerprint().first().map(|f| f.to_string()), Some(public.primary_key.fingerprint().to_string()));
        }
    }
}

// ---- S/MIME, checked by RustCrypto ----

/// A P-256 key from the system's randomness.
fn p256_key() -> p256::ecdsa::SigningKey {
    loop {
        let mut b = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
        if let Ok(k) = p256::ecdsa::SigningKey::from_slice(&b) {
            return k;
        }
    }
}

/// A public key as rcgen puts it in a certificate.
struct Public(Vec<u8>);

impl rcgen::PublicKeyData for Public {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

/// A mail signer's certificate for owner@example.org and `key`, issued by an example authority.
fn certificate(key: &p256::ecdsa::SigningKey) -> Vec<u8> {
    let dated = |name: &str| {
        let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name = rcgen::DistinguishedName::new();
        p.distinguished_name.push(rcgen::DnType::CommonName, name);
        p.not_before = rcgen::date_time_ymd(2020, 1, 1);
        p.not_after = rcgen::date_time_ymd(2060, 1, 1);
        p
    };
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca = dated("Example Root");
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let mut p = dated("Owner");
    p.subject_alt_names = vec![rcgen::SanType::Rfc822Name("owner@example.org".try_into().unwrap())];
    p.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::EmailProtection];
    let public = Public(key.verifying_key().to_sec1_bytes().to_vec());
    p.signed_by(&public, &rcgen::Issuer::new(ca, &ca_key)).unwrap().der().to_vec()
}

fn smime_signed(key: &p256::ecdsa::SigningKey, shape: Shape) -> Message {
    let cert = certificate(key);
    let mut m = message(SignatureFormat::Smime, &"AB".repeat(32), shape);
    sign_by(&mut m, &|_, entity| {
        let request = mailcraft_smime::SignRequest {
            content: entity,
            certificate: &cert,
            chain: &[],
            digest: mailcraft_smime::DigestAlgorithm::Sha256,
            scheme: mailcraft_smime::SignatureScheme::Ecdsa,
            signing_time: NOW,
        };
        let der = mailcraft_smime::sign(&request, |digest| {
            let signature: p256::ecdsa::Signature = key.sign_prehash(digest).map_err(|e| e.to_string())?;
            Ok::<_, String>(signature.to_der().as_bytes().to_vec())
        })
        .map_err(|e| e.to_string())?;
        Ok((SignatureKind::Smime, "sha-256", der))
    })
    .unwrap();
    m
}

/// Whether `cms` (a detached SignedData) is `key`'s signature over `content`: its messageDigest
/// is the SHA-256 of `content`, and its signature checks over its signed attributes.
fn smime_checks(cms: &[u8], content: &[u8], key: &p256::ecdsa::VerifyingKey) -> Result<(), String> {
    let info = ContentInfo::from_der(cms).map_err(|e| e.to_string())?;
    let data = info.content.decode_as::<SignedData>().map_err(|e| e.to_string())?;
    let signer = data.signer_infos.0.iter().next().ok_or("no signer")?;
    let attrs = signer.signed_attrs.as_ref().ok_or("no signed attributes")?;
    let digest = attrs.iter().find(|a| a.oid == MESSAGE_DIGEST).and_then(|a| a.values.iter().next()).ok_or("no messageDigest")?;
    let digest = digest.decode_as::<OctetString>().map_err(|e| e.to_string())?;
    if digest.as_bytes() != sha2::Sha256::digest(content).as_slice() {
        return Err("the messageDigest isn't that of what was sent".into());
    }
    let signature = p256::ecdsa::Signature::from_der(signer.signature.as_bytes()).map_err(|e| e.to_string())?;
    key.verify(&attrs.to_der().map_err(|e| e.to_string())?, &signature).map_err(|e| e.to_string())
}

#[test]
fn an_smime_signature_checks_in_rustcrypto_on_what_goes_and_what_arrives() {
    let key = p256_key();
    for shape in Shape::ALL {
        let m = smime_signed(&key, shape);
        for wire in as_it_arrives(&m) {
            assert_eq!(top_param(&wire, "protocol"), "application/pkcs7-signature");
            let (_, parts, _) = parts(&wire);
            let [signed, signature] = parts.as_slice() else { panic!("two parts") };
            let base64: String = String::from_utf8_lossy(body_of(signature)).split_whitespace().collect();
            let cms = base64_decoded(&base64);
            smime_checks(&cms, signed, key.verifying_key()).unwrap_or_else(|e| panic!("{e} over {:?}", String::from_utf8_lossy(signed)));
        }
    }
}

/// Base64 (RFC 4648 §4) read here, so that nothing of MailCraft's reads what is checked.
fn base64_decoded(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let mut out = Vec::new();
    for chunk in text.trim_end_matches('=').as_bytes().chunks(4) {
        let v: Vec<u32> = chunk.iter().map(|c| u32::from(value(*c))).collect();
        let n = v.iter().enumerate().fold(0u32, |n, (i, x)| n | (x << (18 - 6 * i)));
        out.extend(n.to_be_bytes()[1..].iter().take(chunk.len().saturating_sub(1)));
    }
    out
}

// ---- GnuPG and OpenSSL themselves ----

/// A directory of the test's own, for the files GnuPG and OpenSSL read: taken away when the test
/// ends, a failed one too.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn join(&self, name: impl AsRef<std::path::Path>) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("mailcraft-sign-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// GnuPG checks a message signed at Send by gpg-agent, as it went, as Microsoft 365 writes it and as a server that trims line ends would.
/// Run with `GNUPGHOME` a throwaway keyring whose agent can sign without asking (a key without a
/// passphrase, or one preset) and `MAILCRAFT_GNUPG_TEST_KEY` its primary key's fingerprint:
/// `cargo test -p mailcraft gnupg -- --ignored`.
#[test]
#[ignore = "needs GnuPG, a throwaway GNUPGHOME and MAILCRAFT_GNUPG_TEST_KEY"]
fn gnupg_checks_what_gpg_agent_signed_at_send() {
    let fingerprint = std::env::var("MAILCRAFT_GNUPG_TEST_KEY").expect("MAILCRAFT_GNUPG_TEST_KEY");
    assert!(std::env::var_os("GNUPGHOME").is_some(), "a throwaway GNUPGHOME, never the owner's own");
    let dir = scratch("gnupg");
    for shape in Shape::ALL {
        let mut m = message(SignatureFormat::OpenPgp, &fingerprint, shape);
        sign(&Host::default(), &mut m, mailcraft_time::now()).unwrap();
        for (i, wire) in as_it_arrives(&m).into_iter().enumerate() {
            let (_, parts, _) = parts(&wire);
            let [signed, signature] = parts.as_slice() else { panic!("two parts") };
            let (part, sig) = (dir.join(format!("part-{shape:?}-{i}")), dir.join(format!("sig-{shape:?}-{i}.asc")));
            std::fs::write(&part, signed).unwrap();
            std::fs::write(&sig, body_of(signature)).unwrap();
            let out = std::process::Command::new("gpg").args(["--batch", "--status-fd", "1", "--verify"]).arg(&sig).arg(&part).output().unwrap();
            let status = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && status.contains("[GNUPG:] GOODSIG"), "{status}{}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

/// OpenSSL reads a message signed at Send whole (its own MIME reader) and checks it, as it went
/// and as Microsoft 365 writes it: `cargo test -p mailcraft openssl -- --ignored`.
#[test]
#[ignore = "needs the openssl program"]
fn openssl_checks_an_smime_signature_made_at_send() {
    let key = p256_key();
    let dir = scratch("openssl");
    for shape in Shape::ALL {
        let m = smime_signed(&key, shape);
        for (i, wire) in as_it_arrives(&m).into_iter().enumerate() {
            let path = dir.join(format!("m-{shape:?}-{i}.eml"));
            std::fs::write(&path, &wire).unwrap();
            let out = std::process::Command::new("openssl")
                .args(["cms", "-verify", "-noverify", "-inform", "SMIME", "-out"])
                .arg(dir.join("out"))
                .arg("-in")
                .arg(&path)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
    }
}
