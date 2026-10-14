//! Signed and encrypted mail through the session: import, send, read, refuse, forge.

use mailcraft_crypto::testing;
use mailcraft_model::{Address, FolderKind, Mailbox, Message, Protection};
use serde_json::{Value, json};

use crate::security::{Level, SigState};
use crate::*;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn b64(bytes: &[u8]) -> String {
    crate::cmd::base64_encode(bytes)
}

/// Ada's session, on the system clock (the test certificates run from now for a year).
fn ada_session() -> Session {
    Session::new(Mailbox::with_local_account("Ada Lovelace", "ada@example.org"))
}

fn import_p12(s: &mut Session, who: &testing::Identity, pass: &str) {
    s.set_passphrase(pass);
    let file = testing::p12_file(who, pass).unwrap();
    let out = run(s, "security.import", json!({"data": b64(&file)}));
    assert_eq!(out["privateKeys"], 1, "{out}");
}

fn import_public(s: &mut Session, who: &testing::Identity) {
    let file = mailcraft_crypto::export_public(&who.certificate, true).unwrap();
    run(s, "security.import", json!({"data": b64(&file)}));
}

fn inbox(s: &Session) -> u64 {
    s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap()
}

fn deliver(s: &mut Session, bytes: &[u8]) -> u64 {
    let mut m = mailcraft_mime::parse(bytes);
    m.folder = inbox(s);
    m.account = s.mailbox.accounts[0].id;
    s.mailbox.add_message(m).unwrap()
}

/// A message from `from` to Ada, as its sender's program writes it, signed by `signer`.
fn signed_by(signer: &testing::Identity, from: &str, text: &str) -> Vec<u8> {
    let content = Message { body: mailcraft_model::Body { text: text.into(), html: None }, ..Default::default() };
    let entity = mailcraft_mime::secure::content_entity(&content);
    let secured = match signer.certificate.scheme {
        mailcraft_crypto::Scheme::Smime => {
            let sig = mailcraft_crypto::smime::sign_detached(&entity, &signer.as_smime(), &[], mailcraft_time::now()).unwrap();
            mailcraft_mime::secure::wrap_smime_signed(&entity, &sig)
        }
        mailcraft_crypto::Scheme::OpenPgp => {
            let sig = mailcraft_crypto::openpgp::sign_detached(&entity, &signer.secret).unwrap();
            mailcraft_mime::secure::wrap_pgp_signed(&entity, &sig)
        }
    };
    let m = Message {
        from: Address { name: String::new(), email: from.into() },
        to: vec![Address { name: "Ada".into(), email: "ada@example.org".into() }],
        subject: "Quarterly figures".into(),
        message_id: "q@example.org".into(),
        secured: Some(secured),
        ..Default::default()
    };
    mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())
}

#[test]
fn smime_send_signed_and_encrypted_then_read_it_back() {
    let ada = testing::smime_self_signed("Ada Lovelace", "ada@example.org").unwrap();
    let bea = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    let mut s = ada_session();
    import_p12(&mut s, &ada, "pw");
    import_public(&mut s, &bea);
    let keys = run(&mut s, "security.keys", json!({}));
    assert_eq!(keys["keys"].as_array().unwrap().len(), 2);
    assert_eq!(keys["keys"][0]["own"], true);
    assert_eq!(keys["keys"][0]["privateKeyHere"], true);

    run(&mut s, "mail.new", json!({"to": "bea@example.org", "subject": "Secret plans", "body": "Meet by the lake."}));
    assert_eq!(run(&mut s, "compose.sign", json!({}))["value"], true);
    assert_eq!(run(&mut s, "compose.encrypt", json!({"value": true}))["value"], true);
    let sent = run(&mut s, "compose.send", json!({}));
    let id = sent["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    let secured = m.secured.as_ref().unwrap();
    assert_eq!(secured.kind, Protection::SmimeEncrypted);
    // What it says is not in the mailbox.
    assert!(m.body.text.is_empty());
    let json = serde_json::to_string(&s.mailbox).unwrap();
    assert!(!json.contains("Meet by the lake"));
    // As it goes over the wire: S/MIME, encrypted.
    let wire = String::from_utf8(mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())).unwrap();
    assert!(wire.contains("application/pkcs7-mime; smime-type=enveloped-data"), "{wire}");
    assert!(!wire.contains("Meet by the lake"));

    // Ada reads her own copy: decrypted, signed by her, but her self-signed certificate is
    // vouched for by nobody yet.
    let view = s.security_view(id).unwrap();
    assert_eq!(view.encrypted, Some(mailcraft_model::security::KeyScheme::Smime));
    let (body, _) = view.opened.clone().unwrap();
    assert!(body.text.contains("Meet by the lake"), "{body:?}");
    let sig = view.signature.clone().unwrap();
    assert_eq!(sig.validity, SigState::Valid);
    assert!(sig.from_matches && !sig.verified);
    assert_eq!(view.level(), Level::Notice);
    assert!(view.summary().contains("not verified"), "{}", view.summary());
    // Once she trusts her own certificate, it is verified.
    run(&mut s, "security.trust", json!({"fingerprint": ada.certificate.fingerprint}));
    let view = s.security_view(id).unwrap();
    assert_eq!(view.level(), Level::Good, "{}", view.summary());
    let checked = run(&mut s, "security.check", json!({"id": id}));
    assert_eq!(checked["level"], "good");
    assert!(!checked.to_string().contains("Meet by the lake"));

    // Bea, with her own key, reads what went over the wire.
    let mut b = Session::new(Mailbox::with_local_account("Bea", "bea@example.org"));
    import_p12(&mut b, &bea, "x");
    let got = deliver(&mut b, wire.as_bytes());
    let view = b.security_view(got).unwrap();
    assert!(view.opened.as_ref().unwrap().0.text.contains("Meet by the lake"));
    let sig = view.signature.clone().unwrap();
    assert_eq!(sig.validity, SigState::Valid);
    assert_eq!(sig.signer_emails, ["ada@example.org"]);
    // She has Ada's certificate from the signature, if she wants it.
    assert_eq!(view.collectable.len(), 1);
    assert_eq!(run(&mut b, "security.collect", json!({"id": got}))["added"].as_array().unwrap().len(), 1);
    assert!(b.security_view(got).unwrap().collectable.is_empty());

    // Somebody without a key sees that it is locked, and why.
    let mut c = Session::new(Mailbox::with_local_account("Cyd", "cyd@example.org"));
    let got = deliver(&mut c, wire.as_bytes());
    let view = c.security_view(got).unwrap();
    assert_eq!(view.level(), Level::Locked);
    assert!(view.opened.is_none() && view.locked.is_some());
}

#[test]
fn encryption_is_refused_without_every_recipients_key() {
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let bea = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    let mut s = ada_session();
    import_p12(&mut s, &ada, "pw");
    import_public(&mut s, &bea);
    run(&mut s, "mail.new", json!({"to": "bea@example.org; cyd@example.org", "cc": "dee@example.org", "subject": "x", "body": "y"}));
    run(&mut s, "compose.encrypt", json!({}));
    let before = s.mailbox.messages.len();
    let err = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(err.contains("cyd@example.org") && err.contains("dee@example.org") && !err.contains("bea@example.org"), "{err}");
    assert_eq!(s.mailbox.messages.len(), before, "nothing was sent");
    assert!(s.compose.is_some(), "the message is still being written");

    // Signing without a key of one's own is refused too.
    let mut t = ada_session();
    run(&mut t, "mail.new", json!({"to": "bea@example.org", "subject": "x", "body": "y"}));
    run(&mut t, "compose.sign", json!({}));
    let err = t.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(err.contains("Trust Center"), "{err}");
}

#[test]
fn signed_mail_tampered_forged_and_wrapped() {
    let bea = testing::smime_self_signed("Bea", "bea@example.org").unwrap();
    let mallory = testing::smime_self_signed("Mallory", "mallory@example.org").unwrap();
    let mut s = ada_session();

    // A good signature from the sender the message names.
    let wire = signed_by(&bea, "bea@example.org", "The figures are final.");
    let id = deliver(&mut s, &wire);
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.secured.as_ref().unwrap().kind, Protection::SmimeSigned);
    assert!(m.body.text.contains("The figures are final."));
    assert!(m.attachments.is_empty(), "the signature isn't a file: {:?}", m.attachments);
    let view = s.security_view(id).unwrap();
    let sig = view.signature.clone().unwrap();
    assert_eq!(sig.validity, SigState::Valid);
    assert!(sig.from_matches && !sig.verified);
    // Saved and read back, it still checks.
    let json = serde_json::to_string(&s.mailbox).unwrap();
    let mut again = Session::new(serde_json::from_str(&json).unwrap());
    assert_eq!(again.security_view(id).unwrap().signature.clone().unwrap().validity, SigState::Valid);

    // The text changed on the way.
    let text = String::from_utf8(wire.clone()).unwrap();
    let tampered = text.replacen("The figures are final.", "The figures are wrong.", 1);
    let id = deliver(&mut s, tampered.as_bytes());
    let view = s.security_view(id).unwrap();
    assert_eq!(view.signature.as_ref().unwrap().validity, SigState::Invalid);
    assert_eq!(view.level(), Level::Bad);

    // Mallory signs with his own certificate but writes Bea's address in From.
    let forged = signed_by(&mallory, "bea@example.org", "Wire the money today.");
    let id = deliver(&mut s, &forged);
    let view = s.security_view(id).unwrap();
    let sig = view.signature.clone().unwrap();
    assert_eq!(sig.validity, SigState::Valid);
    assert!(!sig.from_matches);
    assert_ne!(view.level(), Level::Good);
    assert!(view.summary().contains("not the sender"), "{}", view.summary());

    // Bea's genuine signed message wrapped inside an unsigned one with words of Mallory's: the
    // message as a whole is not signed, and isn't shown as signed.
    let inner = String::from_utf8(wire).unwrap();
    let (_, signed_entity) = inner.split_once("MIME-Version: 1.0\r\n").unwrap();
    let wrapped = format!(
        "From: bea@example.org\r\nTo: ada@example.org\r\nSubject: Re\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"zz\"\r\n\r\n--zz\r\nContent-Type: text/plain\r\n\r\nWire the money today.\r\n--zz\r\n{signed_entity}\r\n--zz--\r\n"
    );
    let id = deliver(&mut s, wrapped.as_bytes());
    assert!(s.mailbox.message(id).unwrap().secured.is_none());
    assert!(s.security_view(id).is_none());
}

#[test]
fn openpgp_send_and_read_and_inline() {
    let _ada = testing::openpgp("Ada Lovelace", "ada@example.org").unwrap();
    let bea = testing::openpgp("Bea", "bea@example.org").unwrap();
    let mut s = ada_session();
    s.set_passphrase("pw");
    let file = testing::openpgp_protected_file("Ada Lovelace", "ada@example.org", "pw").unwrap();
    assert_eq!(run(&mut s, "security.import", json!({"data": b64(&file)}))["privateKeys"], 1);
    let bea_pub = mailcraft_crypto::export_public(&bea.certificate, true).unwrap();
    run(&mut s, "security.import", json!({"data": b64(&bea_pub)}));
    run(&mut s, "security.options", json!({"format": "openpgp"}));

    run(&mut s, "mail.new", json!({"to": "bea@example.org", "subject": "Plans", "body": "Tea at four."}));
    run(&mut s, "compose.sign", json!({}));
    run(&mut s, "compose.encrypt", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap().clone();
    assert_eq!(m.secured.as_ref().unwrap().kind, Protection::PgpEncrypted);
    let wire = String::from_utf8(mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())).unwrap();
    assert!(wire.contains("multipart/encrypted; protocol=\"application/pgp-encrypted\""));
    assert!(wire.contains("-----BEGIN PGP MESSAGE-----") && !wire.contains("Tea at four"));
    let view = s.security_view(id).unwrap();
    assert!(view.opened.as_ref().unwrap().0.text.contains("Tea at four"));
    let sig = view.signature.clone().unwrap();
    assert_eq!(sig.validity, SigState::Valid);
    assert!(sig.verified && sig.from_matches, "her own key: {sig:?}");

    // Bea reads it with her key; Ada's key isn't hers yet: the signer is unknown until she
    // has it.
    let mut b = Session::new(Mailbox::with_local_account("Bea", "bea@example.org"));
    assert!(b.restore_secret(&format!("openpgp:{}:{}", bea.secret.fingerprint, b64(&bea.secret.data))));
    let got = deliver(&mut b, wire.as_bytes());
    let view = b.security_view(got).unwrap();
    assert!(view.opened.as_ref().unwrap().0.text.contains("Tea at four"));
    assert_eq!(view.signature.clone().unwrap().validity, SigState::UnknownSigner);

    // Inline signed text.
    use pgp::composed::{CleartextSignedMessage, Deserializable as _, SignedSecretKey};
    let (key, _) = SignedSecretKey::from_reader_single(bea.secret.data.as_slice()).unwrap();
    let signed = CleartextSignedMessage::sign(rand::rngs::OsRng, "See you at four.\n", &key.secret_subkeys[0].key, &"".into()).unwrap();
    let text = signed.to_armored_string(Default::default()).unwrap();
    let raw = format!("From: bea@example.org\r\nTo: ada@example.org\r\nSubject: Inline\r\n\r\n{}", text.replace('\n', "\r\n"));
    let id = deliver(&mut s, raw.as_bytes());
    let view = s.security_view(id).unwrap();
    let sig = view.signature.clone().unwrap();
    assert!(sig.inline);
    assert_eq!(sig.validity, SigState::Valid);
    assert!(sig.from_matches && !sig.verified);
    run(&mut s, "security.trust", json!({"fingerprint": bea.certificate.fingerprint}));
    assert!(s.security_view(id).unwrap().signature.clone().unwrap().verified);
}

#[test]
fn private_keys_stay_out_of_the_mailbox_and_come_back_from_the_host() {
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let mut s = ada_session();
    import_p12(&mut s, &ada, "pw");
    let key_b64 = b64(&ada.secret.data);
    let json = serde_json::to_string(&s.mailbox).unwrap();
    assert!(!json.contains(&key_b64[..40]));
    assert!(!format!("{:?}", s.security).contains(&key_b64[..40]));
    // Encrypt something to herself.
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "subject": "note", "body": "to self"}));
    run(&mut s, "compose.encrypt", json!({}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    // The host is handed the key once, to keep.
    let kept = s.take_secrets_to_remember();
    assert_eq!(kept.len(), 1);
    assert!(s.take_secrets_to_remember().is_empty());
    assert_eq!(s.secret_names(), [kept[0].0.clone()]);

    // Next start: the mailbox file, and the key from the credential store.
    let mut next = Session::new(serde_json::from_str(&serde_json::to_string(&s.mailbox).unwrap()).unwrap());
    assert_eq!(next.security_view(id).unwrap().level(), Level::Locked);
    assert!(next.restore_secret(&kept[0].1));
    assert!(next.security_view(id).unwrap().opened.clone().unwrap().0.text.contains("to self"));
    assert!(!next.restore_secret("smime:zz:@@@"));

    // Removing the key: gone from memory, and the host is told to forget it.
    run(&mut next, "security.remove", json!({"fingerprint": ada.certificate.fingerprint}));
    assert_eq!(next.take_secrets_to_forget(), [kept[0].0.clone()]);
    assert_eq!(next.security_view(id).unwrap().level(), Level::Locked);

    // Wrong passphrase, and none.
    let mut t = ada_session();
    let file = testing::p12_file(&ada, "pw").unwrap();
    t.set_passphrase("nope");
    assert!(t.run("security.import", &json!({"data": b64(&file)})).unwrap_err().to_string().contains("passphrase"));
    // The passphrase was used once and is gone.
    assert!(t.run("security.import", &json!({"data": b64(&file)})).unwrap_err().to_string().contains("passphrase"));
}

#[test]
fn drafts_keep_the_choices_and_replies_to_encrypted_mail_are_encrypted() {
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    let mut s = ada_session();
    import_p12(&mut s, &ada, "pw");
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "subject": "draft", "body": "b"}));
    run(&mut s, "compose.sign", json!({}));
    run(&mut s, "compose.encrypt", json!({}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let c = s.compose.clone().unwrap();
    assert!(c.sign && c.encrypt);
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = s.compose.clone().unwrap();
    assert!(c.encrypt, "a reply to encrypted mail is encrypted");
    // Quoted as plain text (`> b`) or under the header block of an HTML reply (`b`).
    assert!(c.body.lines().any(|line| line.trim_start_matches('>').trim() == "b"), "the reply quotes what it said: {:?}", c.body);
}

#[test]
fn hostile_messages_never_panic() {
    let mut s = ada_session();
    let ada = testing::smime_self_signed("Ada", "ada@example.org").unwrap();
    import_p12(&mut s, &ada, "pw");
    for raw in [
        "Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"\r\n\r\nno boundary",
        "Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=b\r\n\r\n--b\r\nx\r\n--b--",
        "Content-Type: application/pkcs7-mime; smime-type=enveloped-data\r\nContent-Transfer-Encoding: base64\r\n\r\n!!!!",
        "Content-Type: application/pkcs7-mime\r\n\r\n",
        "Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b\r\n\r\n--b\r\nVersion: 1\r\n--b\r\n-----BEGIN PGP MESSAGE-----\r\nxx\r\n-----END PGP MESSAGE-----\r\n--b--",
        "Subject: x\r\n\r\n-----BEGIN PGP SIGNED MESSAGE-----\r\nHash: SHA256\r\n\r\nhi\r\n-----BEGIN PGP SIGNATURE-----\r\n\r\nAAAA\r\n-----END PGP SIGNATURE-----",
        "Subject: x\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n\r\nAAAA\r\n-----END PGP MESSAGE-----",
    ] {
        let id = deliver(&mut s, raw.as_bytes());
        let view = s.security_view(id);
        let _ = view.map(|v| (v.level(), v.summary(), v.to_json()));
        let _ = s.run("security.check", &json!({"id": id}));
        let _ = s.run("security.collect", &json!({"id": id}));
        let _ = s.run("mail.reply", &json!({"id": id}));
    }
}
