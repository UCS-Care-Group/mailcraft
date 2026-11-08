// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bytes a Digitally signed or Encrypted message came as (#30): written and read back with
//! the message, capped, and told apart from a message without them; and telling such a message
//! by what was read of it.

use std::sync::Arc;

use crate::protected::{has_armour, has_armour_within, is_protection_part};
use crate::{Attachment, Body, ContentRef, MAX_ORIGINAL, Message, Original};

#[test]
fn the_original_round_trips_and_none_is_not_written() {
    let bytes = b"Content-Type: multipart/signed; boundary=b\r\n\r\n--b--\r\n".to_vec();
    let m = Message { id: 3, subject: "Signed".into(), original: Original::new(bytes.clone()), ..Default::default() };
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"original\":{\"data\":\""), "{json}");
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back.original_bytes(), Some(bytes.as_slice()));
    assert_eq!(back, m);
    // Where it is kept outside is never written: the mailbox file names it in its own way.
    let kept = Message { original: Some(Original { data: Arc::default(), kept: Some(ContentRef { sha256: [7; 32], len: 9 }) }), ..m.clone() };
    let back: Message = serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
    assert!(back.original.as_ref().is_some_and(|o| o.kept.is_none()) && !back.has_original());
    // A message without one writes nothing of it, and a file from before reads as none.
    let plain = Message { id: 4, ..Default::default() };
    assert!(!serde_json::to_string(&plain).unwrap().contains("original"));
    let old: Message = serde_json::from_str(r#"{"id":4,"subject":"Before"}"#).unwrap();
    assert!(old.original.is_none());
    // Debug says how large it is, not what it holds.
    assert_eq!(format!("{:?}", m.original.unwrap()), format!("Original({} bytes)", bytes.len()));
}

#[test]
fn no_bytes_or_too_many_are_no_original() {
    assert!(Original::new(Vec::new()).is_none());
    assert!(Original::new(vec![b'x'; MAX_ORIGINAL + 1]).is_none());
    assert!(Original::new(vec![b'x'; 10]).is_some_and(|o| o.size() == 10 && o.is_loaded() && o.is_there()));
    let kept = Original { data: Arc::default(), kept: Some(ContentRef { sha256: [1; 32], len: 500 }) };
    assert!(!kept.is_loaded() && kept.is_there() && kept.size() == 500);
    let m = Message { original: Some(kept), ..Default::default() };
    assert!(!m.is_loaded() && m.has_original() && m.original_bytes().is_none());
}

#[test]
fn a_message_to_fetch_again_lets_its_original_go() {
    let mut m = Message { original: Original::new(b"x".to_vec()), ..Default::default() };
    m.to_fetch_again();
    assert!(m.original.is_none());
}

#[test]
fn armour_is_a_line_of_its_own_at_the_start() {
    assert!(has_armour(b"-----BEGIN PGP MESSAGE-----\nabc"));
    assert!(has_armour(b"Hi\r\n-----BEGIN PGP SIGNED MESSAGE-----  \r\nHash: SHA256"));
    assert!(has_armour(b"\xef\xbb\xbf-----BEGIN PGP MESSAGE-----"));
    assert!(!has_armour(b"> -----BEGIN PGP MESSAGE-----"));
    assert!(!has_armour(b"put -----BEGIN PGP MESSAGE----- here"));
    assert!(!has_armour(b"-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    assert!(!has_armour(b"-----BEGIN PGP MESSAGE----- and more"));
    assert!(!has_armour(b""));
    let late = [vec![b'a'; 100], b"\n-----BEGIN PGP MESSAGE-----\n".to_vec()].concat();
    assert!(has_armour(&late) && !has_armour_within(&late, 50));
}

#[test]
fn a_message_read_before_looks_protected_by_its_parts_or_its_text() {
    let file = |name: &str, mime: &str| Attachment::new(name, mime, b"x".to_vec());
    for (name, mime) in [
        ("smime.p7s", "application/pkcs7-signature"),
        ("x.p7s", "application/x-pkcs7-signature"),
        ("smime.p7m", "application/pkcs7-mime; smime-type=enveloped-data"),
        ("signature.asc", "application/pgp-signature"),
        ("", "application/pgp-encrypted"),
        ("SMIME.P7S", "application/octet-stream"),
    ] {
        assert!(is_protection_part(mime, name), "{name} {mime}");
        assert!(Message { attachments: vec![file(name, mime)], ..Default::default() }.looks_protected());
    }
    assert!(!Message { attachments: vec![file("report.pdf", "application/pdf")], ..Default::default() }.looks_protected());
    let inline = Message { body: Body { text: "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\nHi".into(), html: None }, ..Default::default() };
    assert!(inline.looks_protected());
    assert!(!Message { body: Body { text: "Hello.".into(), html: None }, ..Default::default() }.looks_protected());
}

#[test]
fn a_block_of_armour_is_begin_and_end() {
    use crate::protected::has_armour_block;
    assert!(has_armour_block(b"-----BEGIN PGP MESSAGE-----\nabc\n-----END PGP MESSAGE-----\n"));
    assert!(has_armour_block(b"-----BEGIN PGP SIGNED MESSAGE-----\r\nHi\r\n-----BEGIN PGP SIGNATURE-----\r\nx\r\n-----END PGP SIGNATURE-----"));
    assert!(!has_armour_block(b"-----BEGIN PGP MESSAGE-----\nabc\n"));
    assert!(!has_armour_block(b"-----BEGIN PGP SIGNED MESSAGE-----\nHi\n-----END PGP SIGNATURE-----\n"));
    assert!(!has_armour_block(b"-----END PGP MESSAGE-----\n-----BEGIN PGP MESSAGE-----\n"));
    assert!(!has_armour_block(b"> -----BEGIN PGP MESSAGE-----\n> -----END PGP MESSAGE-----\n"));
    assert!(!has_armour_block(b""));
    // Past the scan, an END isn't looked for.
    let late = [&b"-----BEGIN PGP MESSAGE-----\n"[..], &vec![b'a'; crate::protected::MAX_ARMOUR_SCAN], b"\n-----END PGP MESSAGE-----\n"].concat();
    assert!(!has_armour_block(&late));
}

/// The running total of originals is what a fresh sum gives, whichever way the messages change.
#[test]
fn the_running_total_of_originals_follows_every_change() {
    let mut mb = crate::Mailbox::with_local_account("Avery", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let fresh = |mb: &crate::Mailbox| mb.messages.values().filter_map(|m| m.original.as_ref()).map(Original::size).sum::<u64>();
    let check = |mb: &mut crate::Mailbox| {
        let sum = fresh(mb);
        assert_eq!(mb.messages.originals_size(), sum);
        // A copy keeps it, and works it out the same.
        assert_eq!(mb.clone().messages.originals_size(), sum);
    };
    // Adds over several chunks, with and without originals.
    let mut ids = Vec::new();
    for i in 0..3000u64 {
        let original = (i % 3 == 0).then(|| Original::new(vec![b'x'; 1 + (i % 7) as usize])).flatten();
        ids.push(mb.add_message(Message { folder: inbox, original, ..Default::default() }).unwrap());
        if i % 500 == 0 {
            check(&mut mb);
        }
    }
    check(&mut mb);
    // Removed, dropped, put back, made larger, by every way the map is changed.
    for id in ids.iter().step_by(5) {
        mb.messages.remove(id);
    }
    check(&mut mb);
    for id in ids.iter().skip(1).step_by(4) {
        if let Ok(m) = mb.message_mut(*id) {
            m.original = None;
        }
    }
    check(&mut mb);
    for id in ids.iter().skip(2).step_by(6) {
        if let Ok(m) = mb.message_mut(*id) {
            m.original = Original::new(vec![b'y'; 100]);
        }
    }
    check(&mut mb);
    for m in mb.messages.values_mut().take(50) {
        Arc::make_mut(m).original = Original::new(vec![b'z'; 3]);
    }
    check(&mut mb);
    for (_, m) in mb.messages.iter_mut().skip(100).take(50) {
        Arc::make_mut(m).original = None;
    }
    check(&mut mb);
    mb.messages.retain(|id, _| id % 2 == 0);
    check(&mut mb);
    let id = *mb.messages.keys().next().unwrap();
    let mut m = Message::clone(mb.message(id).unwrap());
    m.original = Some(Original { data: Arc::default(), kept: Some(ContentRef { sha256: [2; 32], len: 12_345 }) });
    mb.messages.insert(id, Arc::new(m));
    check(&mut mb);
    while mb.messages.pop_first().is_some() && mb.messages.len() > 10 {}
    check(&mut mb);
    mb.messages.clear();
    check(&mut mb);
    assert_eq!(mb.messages.originals_size(), 0);
    // Read from a file, it is worked out from what the file holds.
    let mut back: crate::Mailbox = serde_json::from_str(
        &serde_json::to_string(&{
            let mut full = crate::Mailbox::with_local_account("Avery", "avery@example.org");
            let inbox = full.inbox().unwrap();
            full.add_message(Message { folder: inbox, original: Original::new(vec![b'q'; 77]), ..Default::default() }).unwrap();
            full
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(back.messages.originals_size(), 77);
}

#[test]
fn a_mailbox_keeps_originals_within_its_cap() {
    let mut mb = crate::Mailbox::with_local_account("Avery", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    assert!(mb.room_for_original(crate::MAX_ORIGINALS));
    assert!(!mb.room_for_original(crate::MAX_ORIGINALS + 1));
    // Originals named on disk count with their length, without their bytes in memory.
    let big = ContentRef { sha256: [1; 32], len: crate::MAX_ORIGINALS - 10 };
    let first = Message { folder: inbox, original: Some(Original { data: Arc::default(), kept: Some(big) }), ..Default::default() };
    let first = mb.add_message(first).unwrap();
    assert!(mb.message(first).unwrap().has_original());
    // One more past the cap is kept without its original; one that fits, with it.
    let over = mb.add_message(Message { folder: inbox, original: Original::new(vec![b'x'; 11]), ..Default::default() }).unwrap();
    assert!(mb.message(over).unwrap().original.is_none());
    let fits = mb.add_message(Message { folder: inbox, original: Original::new(vec![b'x'; 10]), ..Default::default() }).unwrap();
    assert!(mb.message(fits).unwrap().has_original());
}
