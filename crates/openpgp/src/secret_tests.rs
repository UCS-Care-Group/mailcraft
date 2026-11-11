// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Secret keys read for import (#30 R13), without anything decrypted: synthetic `example.org`
//! keys only.

use pgp::composed::{ArmorOptions, SignedSecretKey};
use pgp::ser::Serialize;
use pgp::types::Password;
use proptest::prelude::*;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::fixtures::{Make, armoured_public, public, secret};
use crate::secret::{SigningKind, holds_secret, read};
use crate::{Error, KeyRing};

fn protect(key: &mut SignedSecretKey, primary: bool, subkeys: bool) {
    let password: Password = "correct horse".into();
    if primary {
        key.primary_key.set_password(StdRng::seed_from_u64(3), &password).unwrap();
    }
    if subkeys {
        for sub in &mut key.secret_subkeys {
            sub.key.set_password(StdRng::seed_from_u64(4), &password).unwrap();
        }
    }
}

#[test]
fn a_secret_key_is_read_armoured_or_binary_without_being_decrypted() {
    let key = secret("Ada <ada@example.org>", Make::default());
    let armoured = key.to_armored_string(ArmorOptions::default()).unwrap();
    for bytes in [armoured.as_bytes().to_vec(), key.to_bytes().unwrap()] {
        let info = read(&bytes).unwrap();
        assert_eq!(info.addresses, vec!["ada@example.org".to_string()]);
        assert_eq!(&info.fingerprint, public(&key).fingerprint());
        assert!(!info.protected, "a key with no passphrase");
        assert!(info.can_sign);
        assert_eq!(info.signing_kind, Some(SigningKind::EdDsa));
        // Its public half goes in a ring and finds its signatures.
        let ring: KeyRing = std::iter::once(info.public).collect();
        assert_eq!(ring.for_address("ada@example.org").count(), 1);
        assert!(holds_secret(&bytes));
    }
}

#[test]
fn whether_every_secret_part_is_protected_is_told() {
    let mut key = secret("Ada <ada@example.org>", Make { subkey: true, ..Make::default() });
    protect(&mut key, true, false);
    let info = read(&key.to_bytes().unwrap()).unwrap();
    assert!(!info.protected, "its subkey isn't");
    assert!(info.can_sign, "its signing subkey is here");
    protect(&mut key, false, true);
    assert!(read(&key.to_bytes().unwrap()).unwrap().protected);
}

#[test]
fn a_key_whose_signing_subkey_isnt_here_cant_sign() {
    let mut key = secret("Ada <ada@example.org>", Make { subkey: true, ..Make::default() });
    key.secret_subkeys.clear();
    let info = read(&key.to_bytes().unwrap()).unwrap();
    assert!(!info.can_sign, "the primary key may only certify");
    assert_eq!(info.signing_kind, None);
}

#[test]
fn an_rsa_key_says_so() {
    // (RSA keys are never used in this process: the window refuses them.)
    let key = secret("Ada <ada@example.org>", Make { rsa: Some(2048), ..Make::default() });
    assert_eq!(read(&key.to_bytes().unwrap()).unwrap().signing_kind, Some(SigningKind::Rsa));
}

#[test]
fn a_public_key_is_no_secret_key() {
    let key = secret("Ada <ada@example.org>", Make::default());
    let public = armoured_public(&key);
    assert_eq!(read(public.as_bytes()).err(), Some(Error::Unreadable));
    assert!(!holds_secret(public.as_bytes()));
    assert_eq!(read(&vec![0x95; crate::keys::MAX_KEY_BYTES + 1]).err(), Some(Error::TooLarge));
    assert!(read(b"").is_err());
}

/// A secret key wearing a Public key's armour is still told for one.
#[test]
fn a_secret_key_in_a_public_keys_armour_is_told() {
    let key = secret("Ada <ada@example.org>", Make::default());
    let armoured = key.to_armored_string(ArmorOptions::default()).unwrap().replace("PRIVATE KEY BLOCK", "PUBLIC KEY BLOCK");
    assert!(holds_secret(armoured.as_bytes()));
}

proptest! {
    #[test]
    fn any_bytes_read_without_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = read(&bytes);
        let _ = holds_secret(&bytes);
    }

    #[test]
    fn a_damaged_secret_key_reads_without_a_panic(at in 0usize..400, byte in any::<u8>()) {
        static KEY: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| secret("Ada <ada@example.org>", Make::default()).to_bytes().unwrap());
        let mut bytes = KEY.clone();
        let len = bytes.len();
        if let Some(b) = bytes.get_mut(at % len) {
            *b = byte;
        }
        let _ = read(&bytes);
    }
}
