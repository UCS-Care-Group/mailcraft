// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #30 part 7: Encrypt in a message being written, an account's Encrypt when every recipient has a
//! key, the stop when one has none, the sign-then-encrypt choice of form, drafts that stay on this
//! computer, and what an agent may do. Invented people; the keys are made in the S/MIME and
//! OpenPGP crates' tests (`testdata/`) and imported as the owner would.

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::public_keys::{KeySource, PublicKey};
use mailcraft_model::signing::Unsigned;
use mailcraft_model::sync::SyncResult;
use mailcraft_model::{FolderKind, Id, Message, Original};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{self, Access};
use crate::cmd::account::sync_request;
use crate::cmd::encrypting::{Will, addresses_of, plan, will_encrypt};

/// After the keys were made, before they expire (2030).
const NOW: i64 = 1_900_000_000;
const SMIME_MAYA: &[u8] = include_bytes!("../../../smime/testdata/maya.enc.der");
const SMIME_OWNER: &[u8] = include_bytes!("../../../smime/testdata/owner.enc.der");
const SMIME_KIT: &[u8] = include_bytes!("../../../smime/testdata/kit.enc.der");
const PGP_MAYA: &str = include_str!("../../../openpgp/testdata/maya.enc.asc");
const PGP_OWNER: &str = include_str!("../../../openpgp/testdata/owner.enc.asc");
const SIGN_FP: &str = "abababababababababababababababababababababababababababababababab";

fn b64(bytes: &[u8]) -> String {
    mailcraft_charset::base64::STANDARD.encode(bytes)
}

fn imap() -> (Session, Id) {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    (s, a)
}

fn import(s: &mut Session, data: &[u8]) {
    s.run("keys.import", &json!({"data": b64(data)})).unwrap();
}

fn write(s: &mut Session, a: Id, to: &str) {
    s.run("mail.new", &json!({"to": to, "subject": "Rota", "account": a})).unwrap();
}

fn sign_key(s: &mut Session, a: Id, format: &str) {
    s.run("account.setSigning", &json!({"account": a, "key": {"format": format, "fingerprint": SIGN_FP, "label": "Ada"}})).unwrap();
}

fn outbox_message(s: &Session, a: Id, id: Id) -> Message {
    sync_request(s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap()
}

#[test]
fn a_message_sent_with_encrypt_on_carries_every_recipients_key_and_the_owners() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    import(&mut s, SMIME_OWNER);
    write(&mut s, a, "maya@lumen.example");
    let on = s.run("compose.encrypt", &json!({})).unwrap();
    assert_eq!(on["encrypt"], json!(true), "toggled on from the account's default, off");
    assert_eq!(on["form"], json!("smime"));
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = outbox_message(&s, a, id);
    let to = m.encrypt_with.expect("encrypted at Send");
    assert_eq!(to.format, SignatureFormat::Smime);
    let named: Vec<(&str, bool)> = to.recipients.iter().map(|r| (r.address.as_str(), r.own)).collect();
    assert_eq!(named, [("maya@lumen.example", false), ("ada@mail.example", true)]);
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_MAYA);
    assert!(!to.own_wanted(), "the store had the owner's own key");
    assert!(m.sign_with.is_none());
}

#[test]
fn without_the_owners_own_key_in_the_store_the_host_is_told_to_find_it() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let to = outbox_message(&s, a, id).encrypt_with.unwrap();
    assert!(to.own_wanted());
    assert_eq!(to.own_address, "ada@mail.example");
}

#[test]
fn a_recipient_without_a_key_stops_the_send_and_is_named() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example; kit@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let e = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(e.contains("kit@lumen.example") && !e.contains("maya@lumen.example"), "{e}");
    assert!(e.contains("send it unencrypted"), "{e}");
    assert!(s.compose.is_some(), "the message is still being written");
    assert!(sync_request(&s, a).unwrap().outbox.is_empty(), "nothing was queued");
    // The answer of Encrypt itself names them too, for whoever is asking.
    let on = s.run("compose.encrypt", &json!({"value": true})).unwrap();
    assert_eq!(on["missing"], json!(["kit@lumen.example"]));
    // Remove takes them off, and the message then goes.
    assert_eq!(s.run("compose.removeRecipients", &json!({"addresses": ["KIT@lumen.example"]})).unwrap()["removed"], json!(1));
    assert_eq!(s.compose.as_ref().unwrap().to, "maya@lumen.example");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(outbox_message(&s, a, id).encrypt_with.is_some());
}

#[test]
fn sending_unencrypted_is_encrypt_turned_off_for_that_message() {
    let (mut s, a) = imap();
    write(&mut s, a, "kit@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    assert!(s.run("compose.send", &json!({})).is_err());
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = outbox_message(&s, a, id);
    assert!(m.encrypt_with.is_none());
    assert_eq!(m.encrypt, Some(false));
}

#[test]
fn encrypt_not_asked_for_changes_nothing() {
    let (mut s, a) = imap();
    write(&mut s, a, "kit@lumen.example");
    assert_eq!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::Clear);
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = outbox_message(&s, a, id);
    assert!(m.encrypt_with.is_none() && m.encrypt.is_none());
}

#[test]
fn the_account_that_encrypts_when_it_can_encrypts_only_when_it_can() {
    let (mut s, a) = imap();
    assert!(!s.mailbox.account(a).unwrap().encryption.when_possible, "off by default");
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).unwrap();
    import(&mut s, SMIME_MAYA);
    // Every recipient has a key: encrypted.
    write(&mut s, a, "maya@lumen.example");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(outbox_message(&s, a, id).encrypt_with.is_some());
    // One has none: it goes as it is, and isn't stopped (that is what the setting says).
    write(&mut s, a, "maya@lumen.example; kit@lumen.example");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(outbox_message(&s, a, id).encrypt_with.is_none());
    // The owner turned Encrypt off for a message: not encrypted whatever the keys.
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(outbox_message(&s, a, id).encrypt_with.is_none());
    // And the open message follows a change of the setting meanwhile (R16a).
    write(&mut s, a, "maya@lumen.example");
    assert!(matches!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::Encrypted { .. }));
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": false})).unwrap();
    assert_eq!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::Clear);
    // The toggle goes the other way round from what it would be: off, when the account encrypts.
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).unwrap();
    assert_eq!(s.run("compose.encrypt", &json!({})).unwrap()["encrypt"], json!(false));
}

#[test]
fn the_form_follows_the_key_it_is_signed_with_and_the_keys_the_recipients_have() {
    let (mut s, a) = imap();
    // Maya has an OpenPGP key only; Kit an S/MIME certificate only.
    import(&mut s, PGP_MAYA.as_bytes());
    import(&mut s, SMIME_KIT);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.format, SignatureFormat::OpenPgp, "the form every recipient has a key for");
    // Signed with an S/MIME key, the signed part goes inside: it is encrypted as S/MIME, and
    // Maya's OpenPGP key isn't one it can use.
    sign_key(&mut s, a, "smime");
    s.run("compose.sign", &json!({"value": true})).unwrap();
    assert_eq!(
        will_encrypt(&s, s.compose.as_ref().unwrap()),
        Will::Missing { format: SignatureFormat::Smime, addresses: vec!["maya@lumen.example".to_string()] }
    );
    // Both recipients: whichever form the signature is in is the form it is encrypted in.
    s.run("compose.set", &json!({"to": "kit@lumen.example"})).unwrap();
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.format, SignatureFormat::Smime);
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = outbox_message(&s, a, id);
    assert!(m.sign_with.is_some() && m.encrypt_with.is_some(), "sign, then encrypt");
}

#[test]
fn openpgp_keys_are_encrypted_to_with_the_owners_own() {
    let (mut s, a) = imap();
    import(&mut s, PGP_MAYA.as_bytes());
    import(&mut s, PGP_OWNER.as_bytes());
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let to = outbox_message(&s, a, id).encrypt_with.unwrap();
    assert_eq!(to.format, SignatureFormat::OpenPgp);
    assert_eq!(
        to.recipients.iter().map(|r| (r.address.as_str(), r.own)).collect::<Vec<_>>(),
        [("maya@lumen.example", false), ("ada@mail.example", true)]
    );
}

#[test]
fn a_key_the_owner_imported_is_used_before_one_that_only_came_with_mail() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    // Another certificate for Maya's address that only arrived with a message, newer.
    let collected = PublicKey {
        format: SignatureFormat::Smime,
        data: std::sync::Arc::new(SMIME_KIT.to_vec()),
        fingerprint: "AA".repeat(32),
        not_after: 0,
        seen: NOW,
        verified: false,
        source: KeySource::Collected,
    };
    assert!(s.mailbox.public_keys.add("maya@lumen.example", collected));
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { to, unverified, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_MAYA, "the imported one");
    assert!(unverified.is_empty());
    // With only the collected one, it is used and said not to be the owner's.
    s.run("keys.remove", &json!({"address": "maya@lumen.example", "fingerprint": s.mailbox.public_keys.of("maya@lumen.example").iter().find(|k| k.is_owners()).unwrap().fingerprint})).unwrap();
    let Will::Encrypted { to, unverified, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_KIT);
    assert_eq!(unverified, ["maya@lumen.example"]);
}

#[test]
fn a_key_that_cant_be_encrypted_to_is_no_key() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    // After the certificate expired (2060).
    s.clock.now = Some(3_000_000_000);
    assert!(matches!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::Missing { .. }));
    // A key kept that isn't a key at all.
    let junk = PublicKey {
        format: SignatureFormat::OpenPgp,
        data: std::sync::Arc::new(b"not a key".to_vec()),
        fingerprint: "CC".repeat(20),
        not_after: 0,
        seen: NOW,
        verified: false,
        source: KeySource::Imported,
    };
    s.clock.now = Some(NOW);
    s.mailbox.public_keys.add("maya@lumen.example", junk);
    s.run("keys.remove", &json!({"address": "maya@lumen.example", "fingerprint": s.mailbox.public_keys.of("maya@lumen.example").iter().find(|k| k.format == SignatureFormat::Smime).unwrap().fingerprint})).unwrap();
    assert!(matches!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::Missing { .. }));
}

#[test]
fn too_many_recipients_are_refused_when_asked_and_ignored_when_not() {
    let (s, a) = imap();
    let account = s.mailbox.account(a).unwrap();
    let many: Vec<String> = (0..250).map(|i| format!("p{i}@lumen.example")).collect();
    assert!(matches!(plan(&s.mailbox, account, &many, None, Some(true), NOW), Will::Refused(_)));
    assert_eq!(plan(&s.mailbox, account, &many, None, None, NOW), Will::Clear);
}

#[test]
fn an_account_that_sends_nothing_from_here_cant_encrypt() {
    let mut s = Session::empty();
    let a = s
        .run("account.add", &json!({"email": "ada@mail.example", "kind": "jmap", "incoming": {"host": "jmap.mail.example"}}))
        .ok()
        .and_then(|v| v["id"].as_u64());
    if let Some(a) = a {
        assert!(s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).is_err());
    }
    let (mut s, _) = imap();
    assert!(s.run("account.setEncryption", &json!({"account": 999, "whenPossible": true})).is_err());
    assert!(s.run("account.setEncryption", &json!({"whenPossible": true})).is_err());
    assert!(s.run("account.setEncryption", &json!({"account": 1, "whenPossible": "yes"})).is_err());
}

#[test]
fn hostile_params_are_refused() {
    let (mut s, a) = imap();
    write(&mut s, a, "kit@lumen.example");
    for junk in [json!({"value": "yes"}), json!({"value": 1}), json!({"extra": true}), json!([1]), json!("x")] {
        assert!(s.run("compose.encrypt", &junk).is_err(), "{junk}");
    }
    for junk in [
        json!({}),
        json!({"addresses": 3}),
        json!({"addresses": [1]}),
        json!({"addresses": "kit@lumen.example"}),
        json!({"addresses": vec!["a"; 1001]}),
    ] {
        assert!(s.run("compose.removeRecipients", &junk).is_err(), "{junk}");
    }
    assert_eq!(s.compose.as_ref().unwrap().to, "kit@lumen.example");
    // Without a message being written, nothing.
    s.run("compose.discard", &json!({})).ok();
    s.compose = None;
    assert!(s.run("compose.encrypt", &json!({})).is_err());
}

#[test]
fn a_draft_with_encrypt_on_stays_on_this_computer() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let a = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    // A draft as it always was: put on the server by the next run.
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": "Plain", "account": a})).unwrap();
    let plain = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert!(!s.mailbox.message(plain).unwrap().local_only);
    assert!(s.mailbox.message(plain).unwrap().server_draft.as_ref().is_some_and(|d| d.pending));
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 1);
    s.run("compose.close", &json!({})).unwrap();
    // One with Encrypt on never is.
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": "Secret", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let secret = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let m = s.mailbox.message(secret).unwrap();
    assert!(m.local_only && m.server_draft.is_none() && m.encrypt == Some(true));
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 1, "only the other one waits");
    let (ups, count) = crate::cmd::graph_drafts::draft_changes(&s, a, 10);
    assert_eq!((ups.iter().map(|u| u.message).collect::<Vec<_>>(), count), (vec![plain], 1));
    // Saved again after turning Encrypt off: it may go up.
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    let again = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_eq!(again, secret);
    assert!(!s.mailbox.message(again).unwrap().local_only);
    assert!(s.mailbox.message(again).unwrap().server_draft.as_ref().is_some_and(|d| d.pending));
    // Reopened, a draft is still to be Encrypted.
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    // (The clear copy the server may have is taken away, and the draft made again here: its id is new.)
    let secret = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    s.run("compose.close", &json!({})).unwrap();
    s.run("mail.editDraft", &json!({"id": secret})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, Some(true));
}

#[test]
fn an_account_that_encrypts_when_it_can_keeps_the_drafts_it_would_encrypt_here_too() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let a = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).unwrap();
    import(&mut s, SMIME_MAYA);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Quiet", "account": a})).unwrap();
    let id = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().local_only, "it would be encrypted at Send");
    // One that wouldn't be (a recipient with no key) isn't kept back.
    s.run("compose.set", &json!({"to": "kit@lumen.example"})).unwrap();
    s.run("compose.saveDraft", &json!({})).unwrap();
    assert!(!s.mailbox.message(id).unwrap().local_only);
}

#[test]
fn a_reply_to_an_encrypted_message_is_encrypted_by_default() {
    let (mut s, a) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let body = mailcraft_mime::multipart_encrypted(b"-----BEGIN PGP MESSAGE-----\n\nwcBMA0hQ\n=abcd\n-----END PGP MESSAGE-----\n").unwrap();
    let mut m = Message {
        account: a,
        folder: inbox,
        from: mailcraft_model::Address::new("Kit", "kit@lumen.example"),
        to: vec![mailcraft_model::Address::new("Ada", "ada@mail.example")],
        subject: "Secret".into(),
        message_id: "secret@lumen.example".into(),
        original: Original::new(body),
        ..Default::default()
    };
    m.body.text = "(encrypted)".into();
    let id = s.mailbox.add_message(m).unwrap();
    s.run("mail.select", &json!({"id": id})).unwrap();
    s.run("mail.reply", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, Some(true));
    // The owner may turn it off. A forward of it begins with it on too.
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, Some(false));
    s.run("compose.discard", &json!({})).ok();
    s.compose = None;
    s.run("mail.forward", &json!({"id": id})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, Some(true));
    // A reply to a plain message doesn't.
    s.compose = None;
    let plain = s
        .mailbox
        .add_message(Message { account: a, folder: inbox, subject: "Hi".into(), message_id: "hi@lumen.example".into(), ..Default::default() })
        .unwrap();
    s.run("mail.reply", &json!({"id": plain})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, None);
}

#[test]
fn a_message_that_arrives_in_the_outbox_otherwise_is_still_encrypted_or_held_back() {
    let (mut s, a) = imap();
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).unwrap();
    import(&mut s, SMIME_MAYA);
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let queued = |s: &mut Session, to: &str, encrypt: Option<bool>| {
        let m = Message {
            account: a,
            folder: outbox,
            subject: "Again".into(),
            to: vec![mailcraft_model::Address::new("", to)],
            message_id: format!("{to}@mail.example"),
            encrypt,
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap()
    };
    // Account default, all keys: encrypted.
    let one = queued(&mut s, "maya@lumen.example", None);
    assert!(outbox_message(&s, a, one).encrypt_with.is_some());
    // Account default, a key missing: as it is.
    let two = queued(&mut s, "kit@lumen.example", None);
    let m = outbox_message(&s, a, two);
    assert!(m.encrypt_with.is_none() && !mailcraft_mime::encryption_missing(&m));
    // Asked for, a key missing: nothing decides what it is encrypted to, and no path sends it.
    let three = queued(&mut s, "kit@lumen.example", Some(true));
    let m = outbox_message(&s, a, three);
    assert!(m.encrypt_with.is_none() && mailcraft_mime::encryption_missing(&m));
}

#[test]
fn a_message_that_couldnt_be_encrypted_is_held_with_why() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let unsigned = vec![Unsigned { message: id, why: "none of your own keys could be found".into(), encrypting: true }];
    let email = s.mailbox.account(a).unwrap().email.clone();
    s.run("sync.apply", &json!({"result": SyncResult { account: a, email, unsigned, ..Default::default() }})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Outbox));
    assert!(m.held.starts_with("It couldn't be Encrypted: none of your own keys"), "{}", m.held);
    assert!(sync_request(&s, a).unwrap().outbox.iter().all(|m| m.id != id), "not sent again by itself");
}

#[test]
fn what_was_sent_is_no_longer_to_be_encrypted() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().encrypt_with.is_some());
    let email = s.mailbox.account(a).unwrap().email.clone();
    s.run("sync.apply", &json!({"result": SyncResult { account: a, email, sent: vec![id], ..Default::default() }})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Sent));
    assert!(m.encrypt_with.is_none(), "its encrypted copy is the server's");
}

#[test]
fn recipients_are_each_counted_once_and_only_when_they_are_addresses() {
    use mailcraft_model::Address;
    let to = vec![Address::new("", "Kit@Lumen.Example"), Address::new("", "not an address")];
    let cc = vec![Address::new("", "kit@lumen.example"), Address::new("", "maya@lumen.example")];
    assert_eq!(addresses_of(&[&to, &cc, &[]]), ["kit@lumen.example", "maya@lumen.example"]);
}

#[test]
fn an_agent_may_turn_encrypt_on_and_never_off() {
    let (mut s, a) = imap();
    write(&mut s, a, "kit@lumen.example");
    let class = |s: &Session, id: &str, params: Value| agent::classify(s, id, &params);
    assert_eq!(class(&s, "compose.encrypt", json!({"value": true})), Access::Safe);
    // Off, and the toggle (which may turn it off): the owner's choice alone.
    assert_eq!(class(&s, "compose.encrypt", json!({"value": false})), Access::Never);
    assert_eq!(class(&s, "compose.encrypt", json!({})), Access::Never);
    assert_eq!(class(&s, "compose.encrypt", json!({"value": "true"})), Access::Never);
    // Taking recipients off is as writing the To line; an account's setting is the owner's.
    assert_eq!(class(&s, "compose.removeRecipients", json!({"addresses": ["kit@lumen.example"]})), Access::Safe);
    assert_eq!(class(&s, "account.setEncryption", json!({"account": a, "whenPossible": true})), Access::Never);
}

/// A key as a message that arrived brought it, for `address`: collected, never vouched for.
fn collected(s: &mut Session, address: &str, der: &[u8], fingerprint: &str, seen: i64) {
    let key = PublicKey {
        format: SignatureFormat::Smime,
        data: std::sync::Arc::new(der.to_vec()),
        fingerprint: fingerprint.to_string(),
        not_after: 0,
        seen,
        verified: false,
        source: KeySource::Collected,
    };
    assert!(s.mailbox.public_keys.add(address, key));
}

#[test]
fn a_key_that_only_arrived_with_mail_is_never_the_owners_own() {
    // A forged message for the owner's own address brought a key; it is never encrypted to as the
    // owner's, and the host is told to find the real one.
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    collected(&mut s, "ada@mail.example", SMIME_KIT, &"AB".repeat(32), NOW);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert!(to.own_wanted(), "the host finds the owner's own key: {to:?}");
    assert!(to.recipients.iter().all(|r| r.key.as_slice() != SMIME_KIT));
    // Writing to oneself is no different: the collected key isn't the recipient's either.
    s.run("compose.discard", &json!({})).ok();
    s.compose = None;
    write(&mut s, a, "ada@mail.example; maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert!(to.own_wanted());
    // One the owner imported for it is theirs.
    import(&mut s, SMIME_OWNER);
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert!(!to.own_wanted());
    assert!(to.recipients.iter().any(|r| r.own && r.key.as_slice() == SMIME_OWNER));
}

#[test]
fn several_keys_that_came_with_mail_stop_the_send_until_the_owner_says_which() {
    let (mut s, a) = imap();
    collected(&mut s, "maya@lumen.example", SMIME_MAYA, &"11".repeat(32), NOW - 100);
    // A second, different key, and the newer one: that never decides.
    collected(&mut s, "maya@lumen.example", SMIME_KIT, &"22".repeat(32), NOW);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Conflict { conflicts, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("a conflict") };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].address, "maya@lumen.example");
    assert_eq!(conflicts[0].candidates.len(), 2);
    // Send is refused and says so; an agent has nothing to answer it with.
    let e = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(e.contains("maya@lumen.example") && e.contains("several different keys"), "{e}");
    assert!(s.compose.is_some());
    // The answer is the owner's, in the window: that key is theirs from then on.
    s.set_key_chosen("maya@lumen.example", SignatureFormat::Smime, &"11".repeat(32)).unwrap();
    let Will::Encrypted { to, unverified, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_MAYA);
    assert!(unverified.is_empty(), "chosen keys are the owner's");
    // A third key that arrives later doesn't take its place.
    collected(&mut s, "maya@lumen.example", SMIME_OWNER, &"33".repeat(32), NOW + 1);
    let Will::Encrypted { to, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_MAYA);
    // And no command chooses.
    assert!(s.run("keys.choose", &json!({"address": "maya@lumen.example", "fingerprint": "33"})).is_err());
}

#[test]
fn an_account_that_encrypts_when_it_can_says_why_a_message_goes_unencrypted() {
    let (mut s, a) = imap();
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": true})).unwrap();
    import(&mut s, SMIME_MAYA);
    write(&mut s, a, "maya@lumen.example; kit@lumen.example");
    // (No choice made: the account's setting holds, and a recipient has no key.)
    assert_eq!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::ClearBecause { addresses: vec!["kit@lumen.example".to_string()] });
    // Several keys for a recipient say so too.
    collected(&mut s, "kit@lumen.example", SMIME_KIT, &"44".repeat(32), NOW);
    collected(&mut s, "kit@lumen.example", SMIME_OWNER, &"55".repeat(32), NOW);
    assert_eq!(will_encrypt(&s, s.compose.as_ref().unwrap()), Will::ClearBecause { addresses: vec!["kit@lumen.example".to_string()] });
}

#[test]
fn what_a_waiting_message_is_encrypted_to_is_worked_out_again_from_the_keys_kept_now() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    import(&mut s, SMIME_OWNER);
    write(&mut s, a, "maya@lumen.example");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let before = outbox_message(&s, a, id);
    assert_eq!(before.encrypt, Some(true), "decided at Send, whatever the account says later");
    assert!(before.encrypt_with.is_some());
    // The owner removes Maya's key (a forged one found out, say) and sends it again: the key
    // removed is not the one it is encrypted to.
    let removed = s.mailbox.public_keys.of("maya@lumen.example")[0].fingerprint.clone();
    s.run("keys.remove", &json!({"address": "maya@lumen.example", "fingerprint": removed})).unwrap();
    let after = outbox_message(&s, a, id);
    assert!(after.encrypt_with.is_none(), "no key now");
    assert!(mailcraft_mime::encryption_missing(&after), "so it is held, never sent in the clear");
    // The account's setting turned off meanwhile doesn't make a decided message clear either.
    s.run("account.setEncryption", &json!({"account": a, "whenPossible": false})).unwrap();
    assert!(mailcraft_mime::encryption_missing(&outbox_message(&s, a, id)));
}

#[test]
fn bcc_recipients_are_encrypted_for_apart_and_are_not_in_the_others_keys() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    import(&mut s, SMIME_KIT);
    import(&mut s, SMIME_OWNER);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "bcc": "kit@lumen.example", "subject": "Rota", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { to, blind, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(blind, ["kit@lumen.example"]);
    assert!(to.recipients.iter().any(|r| r.address == "kit@lumen.example"), "their key is kept for their own message");
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = outbox_message(&s, a, id);
    assert_eq!(m.blind_addresses(), ["kit@lumen.example"]);
    // Until the host has made their message, it isn't Encrypted: it is held, never sent as one.
    assert!(mailcraft_mime::encryption_missing(&m));
    // Someone on To and Bcc is known to the others and gets no message of their own.
    s.run("mail.new", &json!({"to": "maya@lumen.example", "bcc": "maya@lumen.example", "subject": "Rota", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let Will::Encrypted { blind, .. } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert!(blind.is_empty());
}

#[test]
fn too_many_bcc_recipients_are_refused() {
    let (s, a) = imap();
    let account = s.mailbox.account(a).unwrap();
    let many: Vec<String> = (0..60).map(|i| format!("p{i}@lumen.example")).collect();
    assert!(matches!(crate::cmd::encrypting::plan_with(&s.mailbox, account, &many, &many, None, Some(true), NOW), Will::Refused(_)));
}

/// What the desktop program does for a draft: Encrypts it to the owner's own key (here the
/// certificate of the tests' owner, and a fixed fill for the random numbers).
struct TestSealer;

impl crate::cmd::encrypting::Sealer for TestSealer {
    fn seal(&self, format: SignatureFormat, _: &str, entity: &[u8], now: i64) -> Result<Vec<u8>, String> {
        if format != SignatureFormat::Smime {
            return Err("no key of yours for that".into());
        }
        let request = mailcraft_smime::EncryptRequest { content: entity, recipients: &[SMIME_OWNER.to_vec()], now };
        let der = mailcraft_smime::encrypt(&request, &mut |b: &mut [u8]| {
            b.fill(7);
            true
        })
        .map_err(|e| e.to_string())?;
        mailcraft_mime::smime_enveloped(&der).map_err(|e| e.to_string())
    }
}

#[test]
fn a_draft_with_encrypt_on_is_kept_encrypted_and_waits_for_decrypting_to_be_opened() {
    let (mut s, a) = imap();
    s.sealer = Some(std::sync::Arc::new(TestSealer));
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": "Secret", "body": "The vault code is 4417.", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.sealed_draft && m.local_only && m.draft);
    assert!(m.original_bytes().is_some_and(mailcraft_mime::is_encrypted), "what is kept is the Encrypted message");
    // The clear text is nowhere in what the mailbox file would say of it.
    assert!(!m.body.text.contains("4417") && m.attachments.is_empty());
    assert!(!serde_json::to_string(m).unwrap().contains("4417"));
    assert_eq!(s.compose.as_ref().unwrap().draft_sealed, Some(true));
    // Saved again, it is still one message, and kept the same way.
    let again = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_eq!(again, id);
    // It can't be opened to edit until decrypting is built; it can still be deleted.
    s.run("compose.close", &json!({})).unwrap();
    let e = s.run("mail.editDraft", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("kept Encrypted"), "{e}");
    assert!(s.compose.is_none());
}

#[test]
fn a_draft_that_cant_be_kept_encrypted_stays_here_as_written_and_says_so() {
    // No host that can (the command line, the browser, a test): as written, on this computer.
    let (mut s, a) = imap();
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": "Secret", "body": "The vault code is 4417.", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.local_only && !m.sealed_draft && m.body.text.contains("4417"));
    assert_eq!(s.compose.as_ref().unwrap().draft_sealed, Some(false));
    // And one whose key can't be found: the same.
    struct NoKey;
    impl crate::cmd::encrypting::Sealer for NoKey {
        fn seal(&self, _: SignatureFormat, _: &str, _: &[u8], _: i64) -> Result<Vec<u8>, String> {
            Err("none of your own keys could be found".into())
        }
    }
    s.sealer = Some(std::sync::Arc::new(NoKey));
    s.run("compose.saveDraft", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().draft_sealed, Some(false));
    assert!(s.mailbox.message(id).unwrap().body.text.contains("4417"));
    s.run("compose.close", &json!({})).unwrap();
    s.run("mail.editDraft", &json!({"id": id})).unwrap();
}

#[test]
fn a_clear_copy_of_a_draft_on_a_server_goes_from_it_when_encrypt_is_turned_on() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let a = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": "Plan", "body": "The vault code is 4417.", "account": a})).unwrap();
    let plain = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    // The server has it by now, in the clear.
    {
        let m = s.mailbox.message_mut(plain).unwrap();
        m.server = Some(mailcraft_model::sync::ServerMessage { id: "AAMkAD-clear".into(), folder: "drafts".into(), ..Default::default() });
    }
    // Encrypt is turned on, and it is saved again: the server's copy is taken away, and nothing of the
    // draft there is kept here.
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let kept = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_ne!(kept, plain, "made again as a draft of this computer's alone");
    assert!(s.mailbox.message(plain).is_none());
    let m = s.mailbox.message(kept).unwrap();
    assert!(m.local_only && m.server.is_none() && m.server_draft.is_none());
    assert!(
        s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "AAMkAD-clear")),
        "the server is told to delete its copy: {:?}",
        s.mailbox.queued.len()
    );
    // (And nothing of it goes up again.)
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 0);
    assert_eq!(s.compose.as_ref().unwrap().draft, Some(kept));
}

#[test]
fn a_sent_encrypted_message_keeps_the_encrypted_message_and_never_the_clear_text() {
    let (mut s, a) = imap();
    import(&mut s, SMIME_MAYA);
    import(&mut s, SMIME_OWNER);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Rota", "body": "The vault code is 4417.", "account": a})).unwrap();
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    // What the host does on its way out: encrypts the copy the run is handed and hands back what it went as.
    let run_copy = outbox_message(&s, a, id);
    let body = mailcraft_mime::smime_enveloped(b"0\x03\x02\x01\x01").unwrap();
    run_copy.sealed.put(Original::new(body.clone()).unwrap());
    let email = s.mailbox.account(a).unwrap().email.clone();
    s.run("sync.apply", &json!({"result": SyncResult { account: a, email, sent: vec![id], ..Default::default() }})).unwrap();
    // Sent Items holds the Encrypted message; its text is a note.
    let sent = s.mailbox.message(id).unwrap();
    assert_eq!(sent.original_bytes(), Some(body.as_slice()));
    assert!(!sent.body.text.contains("4417") && sent.attachments.is_empty());
    assert!(sent.sealed.take().is_none(), "handed back once");
    // It can't be sent again as a new message: there is no text to send.
    let e = s.run("mail.resend", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("Encrypted"), "{e}");
}

fn microsoft_draft(s: &mut Session, subject: &str) -> Id {
    let a = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.new", &json!({"to": "kit@lumen.example", "subject": subject, "body": "The vault code is 4417.", "account": a})).unwrap();
    a
}

#[test]
fn an_encrypted_draft_of_a_server_that_keeps_drafts_goes_up_as_the_encrypted_message_alone() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    s.sealer = Some(std::sync::Arc::new(TestSealer));
    let a = microsoft_draft(&mut s, "Plan");
    // Saved in the clear first: the server has it, with its fields.
    let plain = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    s.mailbox.message_mut(plain).unwrap().server =
        Some(mailcraft_model::sync::ServerMessage { id: "AAMkAD-clear".into(), folder: "drafts".into(), ..Default::default() });
    // Encrypt on, saved again: the same draft here, kept Encrypted, waiting to go up.
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_eq!(id, plain);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.sealed_draft && !m.local_only && m.server_draft.as_ref().is_some_and(|d| d.pending));
    assert_eq!(s.compose.as_ref().unwrap().draft_sealed, Some(true));
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 1);
    let (ups, count) = crate::cmd::graph_drafts::draft_changes(&s, a, 10);
    assert_eq!(count, 1);
    let up = &ups[0];
    // The Encrypted message is all that goes up of it, over the clear copy's id (which goes after).
    assert!(up.is_sealed() && mailcraft_mime::is_encrypted(&up.sealed), "{:?}", String::from_utf8_lossy(&up.sealed));
    assert_eq!(up.id, "AAMkAD-clear");
    assert!(!up.has_fields() && up.attachments.is_none() && up.body.is_none());
    assert!(!String::from_utf8_lossy(&up.sealed).contains("4417"), "no clear text goes up");
    // It is never sent as fields.
    assert!(crate::cmd::graph_drafts::draft_up_whole(s.mailbox.message(id).unwrap()).is_none());
}

#[test]
fn an_encrypted_draft_made_clear_again_replaces_the_encrypted_copy_on_the_server() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    s.sealer = Some(std::sync::Arc::new(TestSealer));
    let a = microsoft_draft(&mut s, "Plan");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let sealed = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    s.mailbox.message_mut(sealed).unwrap().server =
        Some(mailcraft_model::sync::ServerMessage { id: "AAMkAD-sealed".into(), folder: "drafts".into(), ..Default::default() });
    // (Encrypt off for it: the server's draft, made from MIME, can't be changed by fields.)
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    let kept = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_ne!(kept, sealed);
    assert!(s.mailbox.message(sealed).is_none());
    let m = s.mailbox.message(kept).unwrap();
    assert!(!m.sealed_draft && !m.local_only && m.server.is_none());
    assert!(
        s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "AAMkAD-sealed")),
        "the Encrypted copy is deleted there"
    );
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 1, "and the clear one is made");
}

#[test]
fn an_encrypted_draft_with_no_key_of_the_owners_is_never_uploaded() {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let a = microsoft_draft(&mut s, "Plan");
    s.run("compose.encrypt", &json!({"value": true})).unwrap();
    let id = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.local_only && !m.sealed_draft);
    assert_eq!(s.compose.as_ref().unwrap().draft_sealed, Some(false));
    assert_eq!(crate::cmd::graph_drafts::pending(&s.mailbox, a), 0);
}
