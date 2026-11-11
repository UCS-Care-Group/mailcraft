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
    let Will::Encrypted { to, unverified } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
    assert_eq!(to.recipients[0].key.as_slice(), SMIME_MAYA, "the imported one");
    assert!(unverified.is_empty());
    // With only the collected one, it is used and said not to be the owner's.
    s.run("keys.remove", &json!({"address": "maya@lumen.example", "fingerprint": s.mailbox.public_keys.of("maya@lumen.example").iter().find(|k| k.is_owners()).unwrap().fingerprint})).unwrap();
    let Will::Encrypted { to, unverified } = will_encrypt(&s, s.compose.as_ref().unwrap()) else { panic!("encrypted") };
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
    s.run("compose.saveDraft", &json!({})).unwrap();
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
    // The owner may turn it off, and a forward doesn't begin with it.
    s.run("compose.encrypt", &json!({"value": false})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, Some(false));
    s.run("compose.discard", &json!({})).ok();
    s.compose = None;
    s.run("mail.forward", &json!({"id": id})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().encrypt, None);
    // Nor does a reply to a plain message.
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
