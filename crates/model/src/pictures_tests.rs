// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The checks a picture in a message passes.

use std::collections::HashSet;

use crate::pictures::*;

fn png(extra: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.resize(8 + extra, 0);
    v
}

#[test]
fn the_kind_is_read_from_the_bytes_and_webp_is_one() {
    assert_eq!(picture_mime(&png(4)), Some("image/png"));
    assert_eq!(picture_mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
    assert_eq!(picture_mime(b"GIF89a...."), Some("image/gif"));
    assert_eq!(picture_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
    assert_eq!(picture_mime(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
    assert_eq!(picture_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
    assert_eq!(picture_mime(b""), None);
}

#[test]
fn ids_are_not_a_signatures_and_never_repeat() {
    let (mut list, mut seq, held) = (Vec::new(), 0, HashSet::new());
    let a = add(&mut list, &mut seq, &held, "a.png", png(10), None, None).unwrap();
    let b = add(&mut list, &mut seq, &held, "b.png", png(10), Some("B"), Some(9999)).unwrap();
    assert_eq!((a, b), (FIRST_ID, FIRST_ID + 1));
    assert_eq!(list[1].width, Some(MAX_SHOWN_WIDTH));
    // Letting go of the first and adding another does not give its id again.
    list.retain(|p| p.id == b);
    assert_eq!(add(&mut list, &mut seq, &held, "c.png", png(10), None, None).unwrap(), FIRST_ID + 2);
}

#[test]
fn what_is_not_a_picture_or_is_too_big_or_too_many_is_refused() {
    let (mut list, mut seq, mut held) = (Vec::new(), 0, HashSet::new());
    assert!(add(&mut list, &mut seq, &held, "x.png", b"not a picture".to_vec(), None, None).is_err());
    assert!(add(&mut list, &mut seq, &held, "big.png", png(MAX_PICTURE_BYTES), None, None).is_err());
    for _ in 0..MAX_PICTURES {
        let id = add(&mut list, &mut seq, &held, "p.png", png(8), None, None).unwrap();
        held.insert(id);
    }
    assert!(add(&mut list, &mut seq, &held, "p.png", png(8), None, None).is_err(), "the count is capped");
    assert!(list.len() == MAX_PICTURES && list.iter().all(|p| p.id >= FIRST_ID));
    // Together too: four at the size cap fill the total, a fifth does not fit.
    let (mut list, mut seq, mut held) = (Vec::new(), 0, HashSet::new());
    for _ in 0..4 {
        let id = add(&mut list, &mut seq, &held, "a.png", png(MAX_PICTURE_BYTES - 8), None, None).unwrap();
        held.insert(id);
    }
    assert_eq!(list.iter().map(|p| p.data.len()).sum::<usize>(), MAX_PICTURES_BYTES);
    assert!(add(&mut list, &mut seq, &held, "c.png", png(100), None, None).is_err(), "the total is capped");
}

#[test]
fn pictures_the_text_let_go_make_room_but_are_kept_until_then() {
    let (mut list, mut seq, mut held) = (Vec::new(), 0, HashSet::new());
    for _ in 0..MAX_PICTURES {
        let id = add(&mut list, &mut seq, &held, "p.png", png(8), None, None).unwrap();
        held.insert(id);
    }
    // One is deleted from the text: it stays in the list (undo) ...
    let gone = FIRST_ID;
    held.remove(&gone);
    assert!(list.iter().any(|p| p.id == gone));
    // ... until a new one needs its place.
    let fresh = add(&mut list, &mut seq, &held, "q.png", png(8), None, None).unwrap();
    assert!(list.iter().all(|p| p.id != gone) && list.iter().any(|p| p.id == fresh));
}

#[test]
fn names_and_descriptions_are_cleaned() {
    assert_eq!(clean_name("C:\\Users\\me\\a\r\nb.png"), "ab.png");
    assert_eq!(clean_name("/home/me/../x\0.png"), "x.png");
    assert_eq!(clean_alt("  a\r\nb\u{0} "), "ab");
    assert_eq!(clean_alt(&"x".repeat(1000)).chars().count(), crate::signature::MAX_ALT);
}

#[test]
fn a_picture_handed_back_is_checked_as_a_new_one() {
    let none = vec![];
    assert!(take(&none, FIRST_ID, "a.png", png(8), "", None).is_some());
    assert!(take(&none, 3, "a.png", png(8), "", None).is_none(), "not a message picture's id");
    assert!(take(&none, FIRST_ID, "a.png", b"junk".to_vec(), "", None).is_none());
    let kept = vec![take(&none, FIRST_ID, "a.png", png(8), "", None).unwrap()];
    assert!(take(&kept, FIRST_ID, "b.png", png(8), "", None).is_none(), "ids are unique");
}

#[test]
fn a_message_has_its_own_caps_and_a_signatures_are_unchanged() {
    assert_eq!((MAX_PICTURE_BYTES, MAX_PICTURES_BYTES, MAX_PICTURES), (5 << 20, 20 << 20, 30));
    assert_eq!(
        (crate::signature::MAX_PICTURE_BYTES, crate::signature::MAX_PICTURES_BYTES, crate::signature::MAX_PICTURES, crate::signature::MAX_WIDTH),
        (256 * 1024, 512 * 1024, 10, 600)
    );
}
