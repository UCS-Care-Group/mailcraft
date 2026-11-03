// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::proof::*;

#[test]
fn a_proof_needs_the_key_and_the_nonce() {
    let nonce = new_nonce().unwrap();
    assert_eq!(nonce.len(), 64);
    assert!(valid_nonce(&nonce));
    assert_ne!(nonce, new_nonce().unwrap());
    let p = proof("secret", &nonce);
    assert!(proof_ok("secret", &nonce, &p));
    assert!(!proof_ok("other", &nonce, &p));
    assert!(!proof_ok("secret", &new_nonce().unwrap(), &p));
    assert!(!proof_ok("", &nonce, &proof("", &nonce)), "an empty key proves nothing");
}

#[test]
fn junk_is_no_proof_and_no_nonce() {
    let nonce = "ab".repeat(32);
    for given in ["", "zz", "abc", "é", &"00".repeat(32), &"ff".repeat(33)] {
        assert!(!proof_ok("secret", &nonce, given), "{given}");
    }
    for n in ["", "abc", "zz".repeat(20).as_str(), &"a".repeat(129), "é".repeat(20).as_str()] {
        assert!(!valid_nonce(n), "{n}");
    }
}
