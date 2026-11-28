// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key an account signs with, as the window names it (#30).

use crate::digital_signature::SignatureFormat;
use crate::signing::SigningKey;

#[test]
fn a_key_is_shown_by_its_name_or_the_start_of_its_fingerprint() {
    let named = SigningKey::new(SignatureFormat::Smime, &"ab".repeat(32), "Ada").unwrap();
    assert_eq!(named.shown(), "Ada");
    let unnamed = SigningKey::new(SignatureFormat::OpenPgp, &"cd".repeat(20), "").unwrap();
    assert_eq!(unnamed.shown(), "CDCDCDCDCDCDCDCD");
}
