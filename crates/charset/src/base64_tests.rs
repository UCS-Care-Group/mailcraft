// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Base64: the RFC 4648 test vectors, the strict refusals, and a comparison with the `base64`
//! crate (a dev-dependency of this test alone) on every config.

use super::*;
use ::base64::Engine as _;
use proptest::prelude::*;

#[test]
fn the_rfc_vectors_round_trip_in_every_config() {
    let vectors = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")];
    for (plain, coded) in vectors {
        assert_eq!(STANDARD.encode(plain), coded);
        assert_eq!(URL_SAFE.encode(plain), coded);
        assert_eq!(STANDARD_NO_PAD.encode(plain), coded.trim_end_matches('='));
        assert_eq!(URL_SAFE_NO_PAD.encode(plain), coded.trim_end_matches('='));
        assert_eq!(STANDARD.decode(coded).unwrap(), plain.as_bytes());
        assert_eq!(URL_SAFE_NO_PAD.decode(coded.trim_end_matches('=')).unwrap(), plain.as_bytes());
    }
}

#[test]
fn the_two_alphabets_differ_in_the_last_two_symbols() {
    assert_eq!(STANDARD.encode([0xfb, 0xff]), "+/8=");
    assert_eq!(URL_SAFE.encode([0xfb, 0xff]), "-_8=");
    assert!(URL_SAFE.decode("+/8=").is_err());
    assert!(STANDARD.decode("-_8=").is_err());
}

#[test]
fn what_is_not_exactly_base64_is_refused() {
    for bad in ["Zg", "Zg=", "Zg===", "Z", "Z===", "====", "Zm9v\n", " Zm9v", "Zm 9v", "Zh==", "Zm9=", "Zm=v", "Zm9v=", "é", "Zm9\u{0}"] {
        assert!(STANDARD.decode(bad).is_err(), "{bad:?}");
    }
    for bad in ["Zg==", "Z", "Zh", "Zm9v\n", "Zm 9v", "Zg=", "Zm9="] {
        assert!(STANDARD_NO_PAD.decode(bad).is_err(), "{bad:?}");
    }
    assert!(STANDARD_NO_PAD.decode("Zm8").is_ok());
}

#[test]
fn hostile_inputs_do_not_panic() {
    let big = vec![b'A'; 1 << 20];
    for cfg in [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD] {
        assert_eq!(cfg.decode(&big).map(|b| b.len()).unwrap_or(0), 786_432);
        for bytes in [&b""[..], b"=", b"==", b"===", b"\xff\xfe", b"=A=A", b"AA=A"] {
            let _ = cfg.decode(bytes);
        }
    }
}

fn engines() -> [(Config, &'static ::base64::engine::GeneralPurpose); 4] {
    use ::base64::engine::general_purpose as g;
    [(STANDARD, &g::STANDARD), (STANDARD_NO_PAD, &g::STANDARD_NO_PAD), (URL_SAFE, &g::URL_SAFE), (URL_SAFE_NO_PAD, &g::URL_SAFE_NO_PAD)]
}

proptest! {
    #[test]
    fn encoding_is_the_crates(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        for (ours, theirs) in engines() {
            prop_assert_eq!(ours.encode(&data), theirs.encode(&data));
        }
    }

    #[test]
    fn decoding_agrees_with_the_crate_on_valid_and_on_arbitrary_text(
        data in proptest::collection::vec(any::<u8>(), 0..400),
        junk in proptest::collection::vec(any::<u8>(), 0..64),
        text in "[A-Za-z0-9+/=_ -]{0,40}",
    ) {
        for (ours, theirs) in engines() {
            let coded = theirs.encode(&data);
            prop_assert_eq!(ours.decode(&coded).ok(), Some(data.clone()));
            // Any bytes, and any text near base64: the same answer, error or bytes.
            prop_assert_eq!(ours.decode(&junk).ok(), theirs.decode(&junk).ok());
            prop_assert_eq!(ours.decode(&text).ok(), theirs.decode(&text).ok());
            // A valid text with its end cut, and with its first byte made a pad.
            let mut cut = coded.clone().into_bytes();
            cut.pop();
            prop_assert_eq!(ours.decode(&cut).ok(), theirs.decode(&cut).ok());
            if let Some(first) = cut.first_mut() {
                *first = b'=';
            }
            prop_assert_eq!(ours.decode(&cut).ok(), theirs.decode(&cut).ok());
        }
    }
}
