// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The roots NSS's own list trusts for mail (#30): which are chosen from what the module says, in
//! either form of its trust objects, and that only NSS's module, at the system's places, is read.

use crate::anchors_nss::*;

fn root(name: u8, serial: u8, der: &[u8]) -> Root {
    Root { issuer: vec![0x30, name], serial: vec![serial], der: der.to_vec(), email_distrust_after: false }
}

fn trust(of: &Root, email: Email) -> Trust {
    Trust { issuer: of.issuer.clone(), serial: of.serial.clone(), email }
}

#[test]
fn a_root_counts_when_a_trust_object_of_either_form_makes_it_an_anchor_for_mail() {
    let (a, b, c) = (root(1, 1, b"cert-a"), root(2, 1, b"cert-b"), root(3, 7, b"cert-c"));
    // a: an anchor in one form; b: in both (once); c: nothing said of mail (a web site's root).
    let trusts = [trust(&a, Email::Anchor), trust(&b, Email::Anchor), trust(&b, Email::Anchor)];
    assert_eq!(chosen(&[a.clone(), b.clone(), c], &trusts), [a.der.clone(), b.der.clone()]);
    // The same certificate twice is given once.
    assert_eq!(chosen(&[a.clone(), a.clone()], &[trust(&a, Email::Anchor)]), [a.der]);
}

#[test]
fn a_root_distrusted_for_mail_by_any_trust_object_or_after_a_date_does_not_count() {
    let a = root(1, 1, b"cert-a");
    // One form says anchor, the other distrusted: distrust wins.
    assert!(chosen(std::slice::from_ref(&a), &[trust(&a, Email::Anchor), trust(&a, Email::Distrusted)]).is_empty());
    // Mozilla distrusts what it issues for mail after a date: left out whole.
    let late = Root { email_distrust_after: true, ..a.clone() };
    assert!(chosen(&[late], &[trust(&a, Email::Anchor)]).is_empty());
    // `CK_FALSE` (one byte of 0, or nothing) names no date; anything else does.
    assert!(!distrusts_after(&[]) && !distrusts_after(&[0]));
    assert!(distrusts_after(b"260415000000Z") && distrusts_after(&[1]));
}

#[test]
fn a_trust_object_is_for_the_one_root_of_its_issuer_and_serial_number() {
    let a = root(1, 1, b"cert-a");
    // Another issuer, or another serial number: not this root's.
    let elsewhere = Trust { issuer: vec![0x30, 9], ..trust(&a, Email::Anchor) };
    let other_serial = Trust { serial: vec![2], ..trust(&a, Email::Anchor) };
    assert!(chosen(std::slice::from_ref(&a), &[elsewhere, other_serial]).is_empty());
    // Two certificates of the module with one issuer and serial number: which of them a trust
    // object is for can't be told, so neither counts (and nothing beside them is touched).
    let twin = root(1, 1, b"cert-a-twin");
    let b = root(2, 1, b"cert-b");
    let trusts = [trust(&a, Email::Anchor), trust(&b, Email::Anchor)];
    assert_eq!(chosen(&[a.clone(), twin, b.clone()], &trusts), [b.der]);
    // The same certificate twice is no such pair.
    assert_eq!(chosen(&[a.clone(), a.clone()], &[trust(&a, Email::Anchor)]), [a.der]);
}

#[test]
fn the_searches_ask_for_mail_s_trust_in_both_forms_by_their_published_numbers() {
    // NSS's own (pkcs11n.h): its vendor number 0x4E534350 in the vendor-defined ranges.
    assert_eq!(CKO_NSS_TRUST, 0xCE53_4353);
    assert_eq!(CKA_NSS_TRUST_EMAIL_PROTECTION, 0xCE53_635B);
    assert_eq!(CKA_NSS_EMAIL_DISTRUST_AFTER, 0xCE53_4374);
    assert_eq!((CKT_NSS_TRUSTED_DELEGATOR, CKT_NSS_NOT_TRUSTED), (0xCE53_4352, 0xCE53_435A));
    // PKCS #11 3.2's (pkcs11t.h).
    assert_eq!((CKO_TRUST, CKA_TRUST_EMAIL_PROTECTION, CKT_TRUST_ANCHOR, CKT_NOT_TRUSTED), (11, 1_583, 2, 3));
    // Each form is asked for its anchors and its distrusted, and for nothing but e-mail protection.
    for (class, purpose) in [(CKO_NSS_TRUST, CKA_NSS_TRUST_EMAIL_PROTECTION), (CKO_TRUST, CKA_TRUST_EMAIL_PROTECTION)] {
        let mine: Vec<_> = SEARCHES.iter().filter(|s| s.0 == class).collect();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|s| s.1 == purpose));
        assert!(mine.iter().any(|s| s.3 == Email::Anchor) && mine.iter().any(|s| s.3 == Email::Distrusted));
    }
}

#[test]
fn only_nss_s_module_at_the_system_s_places_is_read() {
    assert!(NSS_MODULES.iter().all(|p| p.starts_with('/') && p.ends_with("/libnssckbi.so")));
    // Never a program's own copy (a browser's, Adobe AIR's under /opt), nor the home folder.
    assert!(NSS_MODULES.iter().all(|p| p.starts_with("/usr/lib") || *p == "/usr/local/lib/libnssckbi.so"));
    // FreeBSD's `security/nss` port puts it straight in its prefix's `lib`, not in an `nss/` folder.
    assert!(NSS_MODULES.contains(&"/usr/local/lib/libnssckbi.so"));
    assert_eq!(BUILTIN_TOKEN, "Builtin Object Token");
}

/// The module of this machine, when it has NSS's: it trusts some roots for mail, each a
/// certificate. (Ignored by default: what a machine has installed is not the tests' to rely on.
/// `cargo test -p mailcraft-net anchors_nss -- --ignored --nocapture`.)
#[test]
#[ignore]
fn this_machine_s_nss_module_gives_roots_for_mail() {
    let Some(found) = read() else {
        println!("no NSS module of roots here");
        return;
    };
    println!("{} roots trusted for mail", found.len());
    assert!(!found.is_empty() && found.len() <= MAX_OBJECTS);
    assert!(found.iter().all(|der| der.first() == Some(&0x30)));
}

/// Many threads asking at once (as the command line's tests do, each with a session of its own)
/// have one read between them, and the same answer: a regression test for a crash when two loaded,
/// started and let go of the module at the same time. Without NSS's module here it asks nothing.
#[test]
fn many_threads_asking_at_once_share_one_read() {
    let answers: Vec<Option<Vec<Vec<u8>>>> = std::thread::scope(|s| {
        let asked: Vec<_> = (0..16).map(|_| s.spawn(read)).collect();
        asked.into_iter().map(|t| t.join().unwrap()).collect()
    });
    assert!(answers.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn distrust_not_read_in_full_gives_no_roots_but_an_unread_anchor_is_only_left_out() {
    let (a, b) = (root(1, 1, b"cert-a"), root(2, 1, b"cert-b"));
    let all = |of: &Root, email| Some(vec![Some(trust(of, email))]);
    // Read in full: kept as found.
    assert_eq!(kept(Email::Distrusted, all(&a, Email::Distrusted)), Some(vec![trust(&a, Email::Distrusted)]));
    assert_eq!(kept(Email::Distrusted, Some(Vec::new())), Some(Vec::new()));
    // A failed search, an object not read whole, or more than are read: no answer at all.
    assert_eq!(kept(Email::Distrusted, None), None);
    assert_eq!(kept(Email::Distrusted, Some(vec![Some(trust(&a, Email::Distrusted)), None])), None);
    assert_eq!(kept(Email::Distrusted, Some(vec![Some(trust(&a, Email::Distrusted)); MAX_OBJECTS + 1])), None);
    // Anchors: what could be read, and nothing where the search failed.
    assert_eq!(kept(Email::Anchor, Some(vec![None, Some(trust(&b, Email::Anchor))])), Some(vec![trust(&b, Email::Anchor)]));
    assert_eq!(kept(Email::Anchor, None), Some(Vec::new()));
    assert_eq!(kept(Email::Anchor, Some(vec![Some(trust(&b, Email::Anchor)); MAX_OBJECTS + 1])).map(|v| v.len()), Some(MAX_OBJECTS));
}
