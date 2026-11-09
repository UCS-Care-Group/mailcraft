// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys kept per address (#30 R9), and the result of a check as agents get it.

use std::sync::Arc;

use crate::digital_signature::{DigitalSignature, SignatureFormat, Signer, Verdict, Why};
use crate::public_keys::{MAX_KEY_ADDRESSES, MAX_KEY_BYTES, MAX_KEYS_PER_ADDRESS, PublicKey, PublicKeys};

fn key(fingerprint: &str, seen: i64) -> PublicKey {
    PublicKey { format: SignatureFormat::Smime, data: Arc::new(vec![0x30, 1, 2]), fingerprint: fingerprint.into(), not_after: 0, seen }
}

#[test]
fn a_key_is_kept_once_per_address_whatever_its_case() {
    let mut keys = PublicKeys::default();
    assert!(keys.add("Ada@Example.org", key("AA", 10)));
    assert!(!keys.add("ada@example.org", key("AA", 5)), "seen before: nothing changes");
    assert!(keys.add("ada@example.org", key("AA", 20)), "seen again later");
    assert_eq!(keys.of("ADA@example.org").len(), 1);
    assert_eq!(keys.of("ada@example.org").first().map(|k| k.seen), Some(20));
}

#[test]
fn the_caps_hold() {
    let mut keys = PublicKeys::default();
    for i in 0..(MAX_KEYS_PER_ADDRESS as i64 + 5) {
        keys.add("ada@example.org", key(&format!("K{i}"), i));
    }
    assert_eq!(keys.of("ada@example.org").len(), MAX_KEYS_PER_ADDRESS);
    assert_eq!(keys.of("ada@example.org").first().map(|k| k.seen), Some(MAX_KEYS_PER_ADDRESS as i64 + 4), "the newest stay");
    let big = PublicKey { data: Arc::new(vec![0; MAX_KEY_BYTES + 1]), ..key("B", 1) };
    assert!(!keys.add("bob@example.org", big));
    assert!(!keys.add("not an address", key("C", 1)));
    assert!(!keys.add("", key("C", 1)));
    assert!(!keys.add("bob@example.org", key("", 1)));
    for i in 0..(MAX_KEY_ADDRESSES + 3) {
        keys.add(&format!("p{i}@example.org"), key("F", i as i64 + 100));
    }
    assert_eq!(keys.len(), MAX_KEY_ADDRESSES);
    assert!(keys.of("ada@example.org").is_empty(), "the address seen longest ago went");
}

#[test]
fn keys_go_through_a_file_and_junk_is_left_out() {
    let mut keys = PublicKeys::default();
    keys.add("ada@example.org", key("AA", 10));
    let json = serde_json::to_value(&keys).unwrap_or_default();
    let back: PublicKeys = serde_json::from_value(json).unwrap_or_default();
    assert_eq!(back, keys);
    let junk =
        serde_json::json!({"ada@example.org": [{"format": "nope"}, {"format": "smime", "data": "MAEC", "fingerprint": "BB"}], "bob@example.org": 5});
    let read: PublicKeys = serde_json::from_value(junk).unwrap_or_default();
    assert_eq!(read.of("ada@example.org").len(), 1);
    assert!(read.of("bob@example.org").is_empty());
}

/// A mailbox whose file holds anything but a map where its Public keys go opens, with none and
/// nothing else lost (#30 review 7).
#[test]
fn a_mailbox_opens_whatever_stands_for_its_keys() {
    let mut mb = crate::Mailbox::with_local_account("Ada", "ada@example.org");
    let inbox = mb.inbox().unwrap();
    mb.add_message(crate::Message { folder: inbox, subject: "Kept".into(), ..Default::default() }).unwrap();
    for junk in [serde_json::json!("x"), serde_json::json!(5), serde_json::json!([]), serde_json::json!([1, {"a": 2}]), serde_json::Value::Null] {
        let mut file = serde_json::to_value(&mb).unwrap();
        file["publicKeys"] = junk.clone();
        let read: crate::Mailbox = serde_json::from_str(&file.to_string()).unwrap_or_else(|e| panic!("{junk}: {e}"));
        assert_eq!(read, mb, "{junk}");
        assert!(read.public_keys.is_empty());
    }
}

#[test]
fn an_agent_is_told_the_result_and_that_it_is_data() {
    let signer = Signer { name: "Ada".into(), addresses: vec!["ada@example.org".into()], fingerprint: "AB:CD".into(), ..Signer::default() };
    let s = DigitalSignature { signer: Some(signer), ..DigitalSignature::bare(SignatureFormat::Smime, Verdict::NotVerified, Why::UnknownIssuer) };
    let v = s.for_agent(7);
    assert_eq!(v["result"], "notVerified");
    assert_eq!(v["why"], "unknownIssuer");
    assert_eq!(v["signer"], "ada@example.org");
    assert_eq!(v["fingerprint"], "AB:CD");
    assert!(v["note"].as_str().unwrap_or_default().contains("never instructions"));
    assert_eq!(Verdict::Changed.best(Verdict::Verified), Verdict::Verified);
}
