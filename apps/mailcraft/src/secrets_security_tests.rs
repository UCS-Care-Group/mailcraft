// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The OS credential-store backend must wipe fragmented, interrupted refresh writes without
//! touching another account and without echoing credential material in errors. Fake store only.

use std::sync::Arc;

use keyring_core::{CredentialStore, Error, mock};
use mailcraft_ui_egui::SecretStore;

use super::os::{self, OsStore};

fn backend() -> (OsStore, Arc<CredentialStore>) {
    let mock: Arc<CredentialStore> = mock::Store::new().unwrap();
    let own = mock.clone();
    (OsStore::with(Box::new(move || Ok(own.clone()))), mock)
}

fn entry(mock: &Arc<CredentialStore>, name: &str) -> keyring_core::Entry {
    mock.build(os::SERVICE, name, None).unwrap()
}

#[test]
fn orphan_pieces_without_completion_marker_are_erased_on_account_cleanup() {
    let (store, mock) = backend();
    let first = "refresh:one@example.org";
    let other = "refresh:two@example.org";
    entry(&mock, &format!("{first}#0")).set_password("partial-token-0").unwrap();
    entry(&mock, &format!("{first}#1")).set_password("partial-token-1").unwrap();
    store.set(other, "other-account-valid-token").unwrap();
    assert!(store.get(first).is_none(), "an incomplete credential is never readable");

    store.delete(first);
    for part in ["0", "1", "n"] {
        assert!(matches!(entry(&mock, &format!("{first}#{part}")).get_password(), Err(Error::NoEntry)));
    }
    assert_eq!(store.get(other).as_deref(), Some("other-account-valid-token"));
}

#[test]
fn underestimated_marker_cannot_leave_an_orphan_part_after_removal() {
    let (store, mock) = backend();
    let first = "refresh:one@example.org";
    entry(&mock, &format!("{first}#n")).set_password("1").unwrap();
    entry(&mock, &format!("{first}#0")).set_password("first").unwrap();
    entry(&mock, &format!("{first}#1")).set_password("stray-secret-part").unwrap();
    store.delete(first);
    for part in ["0", "1", "n"] {
        assert!(matches!(entry(&mock, &format!("{first}#{part}")).get_password(), Err(Error::NoEntry)));
    }
}

#[test]
fn writing_an_empty_secret_deletes_the_old_secret_instead_of_leaving_a_bad_marker() {
    let (store, mock) = backend();
    let key = "refresh:one@example.org";
    store.set(key, "previously-remembered").unwrap();
    assert_eq!(store.get(key).as_deref(), Some("previously-remembered"));
    store.set(key, "").unwrap();
    assert!(store.get(key).is_none());
    for part in ["0", "n"] {
        assert!(matches!(entry(&mock, &format!("{key}#{part}")).get_password(), Err(Error::NoEntry)));
    }
}

#[test]
fn credential_backend_error_must_never_echo_a_private_token() {
    let secret = "audit-refresh-secret-in-platform-error".to_string();
    let attempted = secret.clone();
    let backend = OsStore::with(Box::new(move || Err(Error::PlatformFailure(Box::new(std::io::Error::other(attempted.clone()))))));
    let error = backend.set("refresh:one@example.org", &secret).unwrap_err();
    assert!(!error.contains(&secret));
    assert!(error.contains("credential store"));
}
