// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys kept per address (#30 R9), and the result of a check as agents get it.

use std::sync::Arc;

use crate::digital_signature::{DigitalSignature, SignatureFormat, Signer, Verdict, Why};
use crate::public_keys::{KeySource, MAX_KEY_ADDRESSES, MAX_KEY_BYTES, MAX_KEYS_PER_ADDRESS, PublicKey, PublicKeys};

fn key(fingerprint: &str, seen: i64) -> PublicKey {
    PublicKey {
        format: SignatureFormat::Smime,
        data: Arc::new(vec![0x30, 1, 2]),
        fingerprint: fingerprint.into(),
        not_after: 0,
        seen,
        verified: false,
        source: KeySource::Collected,
    }
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

fn pgp(fingerprint: &str, seen: i64) -> PublicKey {
    PublicKey { format: SignatureFormat::OpenPgp, ..key(fingerprint, seen) }
}

/// A key given to `add` never comes Verified, and one kept keeps the owner's mark and its
/// imported source whatever copy of it arrives (#30 R6).
#[test]
fn add_never_marks_a_key_verified_and_keeps_the_owners_mark() {
    let mut keys = PublicKeys::default();
    assert!(keys.add("ada@example.org", PublicKey { verified: true, ..pgp("AA", 1) }));
    assert!(!keys.of("ada@example.org")[0].verified, "a key given to add is never Verified");
    assert!(keys.set_verified("ada@example.org", "AA", true));
    assert!(!keys.set_verified("ada@example.org", "AA", true), "unchanged");
    assert!(keys.add("ada@example.org", PublicKey { source: KeySource::Imported, ..pgp("AA", 2) }));
    assert!(keys.add("ada@example.org", pgp("AA", 3)));
    let kept = &keys.of("ada@example.org")[0];
    assert!(kept.verified, "the mark stays");
    assert_eq!(kept.source, KeySource::Imported, "imported stays imported");
    // S/MIME's trust is the authority's: never marked.
    keys.add("bob@example.org", key("BB", 1));
    assert!(!keys.set_verified("bob@example.org", "BB", true));
    assert!(!keys.set_verified("bob@example.org", "nope", true));
    let before = keys.revision();
    assert!(keys.set_verified("ada@example.org", "AA", false));
    assert_ne!(keys.revision(), before);
}

/// A copy of a key kept already with other bytes (one carrying a revocation, merged by the
/// caller) takes its place even when it was seen no later (#30 part 4).
#[test]
fn a_copy_with_other_bytes_takes_the_kept_ones_place_whenever_it_was_seen() {
    let mut keys = PublicKeys::default();
    keys.add("ada@example.org", pgp("AA", 10));
    let revoked = PublicKey { data: Arc::new(vec![9, 9, 9]), not_after: 77, ..pgp("AA", 5) };
    assert!(keys.add("ada@example.org", revoked));
    let kept = &keys.of("ada@example.org")[0];
    assert_eq!(kept.data.as_slice(), &[9, 9, 9]);
    assert_eq!((kept.not_after, kept.seen), (77, 10));
}

/// What the owner imported or marked never goes to make room before what was only collected.
#[test]
fn the_owners_keys_are_kept_before_collected_ones() {
    let mut keys = PublicKeys::default();
    keys.add("ada@example.org", PublicKey { source: KeySource::Imported, ..key("OLD", 0) });
    keys.add("ada@example.org", pgp("MARKED", 1));
    keys.set_verified("ada@example.org", "MARKED", true);
    for i in 0..(MAX_KEYS_PER_ADDRESS as i64 + 5) {
        keys.add("ada@example.org", key(&format!("K{i}"), i + 10));
    }
    let kept = keys.of("ada@example.org");
    assert_eq!(kept.len(), MAX_KEYS_PER_ADDRESS);
    assert!(kept.iter().any(|k| k.fingerprint == "OLD"));
    assert!(kept.iter().any(|k| k.fingerprint == "MARKED"));
    // A full list of the owner's: a collected one coming is the one that doesn't stay.
    let mut full = PublicKeys::default();
    for i in 0..MAX_KEYS_PER_ADDRESS as i64 {
        full.add("bob@example.org", PublicKey { source: KeySource::Imported, ..key(&format!("I{i}"), i) });
    }
    assert!(!full.add("bob@example.org", key("NEW", 1_000)), "nothing changed");
    assert!(full.of("bob@example.org").iter().all(|k| k.source == KeySource::Imported));
    // An address with something of the owner's stays past the cap of addresses.
    let mut many = PublicKeys::default();
    many.add("owner@example.org", PublicKey { source: KeySource::Imported, ..key("O", 0) });
    for i in 0..(MAX_KEY_ADDRESSES + 3) {
        many.add(&format!("p{i}@example.org"), key("F", i as i64 + 100));
    }
    assert_eq!(many.len(), MAX_KEY_ADDRESSES);
    assert_eq!(many.of("owner@example.org").len(), 1);
}

#[test]
fn a_key_is_removed_and_listed() {
    let mut keys = PublicKeys::default();
    keys.add("ada@example.org", key("AA", 1));
    keys.add("ada@example.org", pgp("BB", 2));
    keys.add("bob@example.org", key("CC", 3));
    assert_eq!(keys.iter().map(|(_, k)| k.len()).sum::<usize>(), 3);
    let before = keys.revision();
    assert!(keys.remove("ADA@example.org", "AA"));
    assert_ne!(keys.revision(), before);
    assert!(!keys.remove("ada@example.org", "AA"));
    assert!(keys.remove("bob@example.org", "CC"));
    assert_eq!(keys.len(), 1, "an address with no key left goes");
    assert_eq!(keys.iter().map(|(a, _)| a).collect::<Vec<_>>(), vec!["ada@example.org"]);
}

/// The marks and sources go through the file; a file's S/MIME key marked Verified isn't.
#[test]
fn the_owners_marks_go_through_a_file() {
    let mut keys = PublicKeys::default();
    keys.add("ada@example.org", PublicKey { source: KeySource::Imported, ..pgp("AA", 1) });
    keys.set_verified("ada@example.org", "AA", true);
    keys.add("ada@example.org", key("BB", 2));
    let json = serde_json::to_value(&keys).unwrap();
    assert_eq!(json["ada@example.org"][1]["verified"], true);
    assert_eq!(json["ada@example.org"][1]["source"], "imported");
    assert!(json["ada@example.org"][0].get("verified").is_none(), "left out when false");
    assert!(json["ada@example.org"][0].get("source").is_none(), "left out when collected");
    let back: PublicKeys = serde_json::from_value(json).unwrap();
    assert_eq!(back, keys);
    let forged = serde_json::json!({"ada@example.org": [{"format": "smime", "data": "MAEC", "fingerprint": "BB", "verified": true}]});
    let read: PublicKeys = serde_json::from_value(forged).unwrap();
    assert!(!read.of("ada@example.org")[0].verified);
}

/// What a step changed of the keys is what an undo puts back: never a revocation taken away,
/// never a key marked Verified, and every other key as it is now.
#[test]
fn an_undo_puts_back_only_what_its_step_changed() {
    let mut before = PublicKeys::default();
    before.add("ada@example.org", pgp("KEEP", 1));
    before.set_verified("ada@example.org", "KEEP", true);
    before.add("ada@example.org", pgp("GONE", 2));
    before.set_verified("ada@example.org", "GONE", true);
    let mut after = before.clone();
    assert!(after.differences(&before).is_empty());
    after.remove("ada@example.org", "GONE");
    after.add("bob@example.org", PublicKey { source: KeySource::Imported, ..key("NEW", 3) });
    after.add("ada@example.org", PublicKey { source: KeySource::Imported, data: Arc::new(vec![7]), ..pgp("KEEP", 4) });
    let step = after.differences(&before);
    assert_eq!(step.len(), 3, "{step:?}");
    // Since: a key collected for somebody else, which the undo leaves.
    let mut now = after.clone();
    now.add("cy@example.org", key("LATER", 9));
    now.put_back(&before, &step);
    assert_eq!(now.of("cy@example.org").len(), 1);
    assert!(now.of("bob@example.org").is_empty());
    let ada = now.of("ada@example.org");
    let gone = ada.iter().find(|k| k.fingerprint == "GONE").unwrap();
    assert!(!gone.verified, "back, but not Verified");
    let keep = ada.iter().find(|k| k.fingerprint == "KEEP").unwrap();
    assert_eq!(keep.data.as_slice(), &[7], "the newer bytes stay");
    assert_eq!(keep.source, KeySource::Collected);
    assert!(keep.verified);
}
