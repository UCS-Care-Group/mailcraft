// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use mailcraft_model::sync::Tokens;

use crate::signin::{SignIns, forget, hint_of, keep_refresh, sensible_email, signed_in_address, tenant_of};
use crate::{Platform, SecretError, SecretStore};

#[derive(Default)]
struct Kept(Mutex<HashMap<String, String>>);

impl SecretStore for Kept {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
    }
}

struct Phone(Option<Arc<Kept>>);

impl Platform for Phone {
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        self.0.clone().map(|k| k as Arc<dyn SecretStore>)
    }
}

#[test]
fn only_a_shape_of_address_is_one() {
    assert!(sensible_email("ada@lumen.example"));
    for bad in ["", "ada", "@lumen.example", "ada@", "a@b@c", "ada @lumen.example", "ada@lumen.example\u{7}"] {
        assert!(!sensible_email(bad), "{bad:?}");
    }
    assert!(!sensible_email(&format!("{}@lumen.example", "a".repeat(300))));
    assert_eq!(hint_of("  ada@lumen.example "), "ada@lumen.example");
    assert_eq!(hint_of("not an address"), "");
}

#[test]
fn the_directory_named_for_an_administrator_is_a_plain_domain_or_nothing() {
    assert_eq!(tenant_of("Ada@Lumen.Example"), "lumen.example");
    assert_eq!(tenant_of("ada@lumen.example/../x"), "");
    assert_eq!(tenant_of("nobody"), "");
}

#[test]
fn who_signed_in_is_what_microsoft_names_and_nothing_else() {
    let tokens = |email: &str| Tokens { email: email.to_string(), ..Default::default() };
    assert_eq!(signed_in_address(&tokens(" ada@lumen.example ")).as_deref(), Some("ada@lumen.example"));
    assert_eq!(signed_in_address(&tokens("")), None);
    assert_eq!(signed_in_address(&tokens("not an address")), None);
}

#[test]
fn one_sign_in_at_a_time_and_the_first_address_only() {
    let signins = SignIns::default();
    // Nobody waits: the address finds nothing to go to.
    assert!(!signins.deliver("x:/y".to_string()));
    let place = signins.begin().unwrap();
    assert!(signins.begin().is_none());
    let rx = signins.arm();
    assert!(!signins.deliver("x".repeat(9 * 1024)), "too long");
    assert!(signins.deliver("x:/first".to_string()));
    assert!(!signins.deliver("x:/second".to_string()));
    assert_eq!(rx.try_recv().unwrap(), "x:/first");
    signins.cancel();
    assert!(place.cancel_flag().load(std::sync::atomic::Ordering::Acquire));
    drop(place);
    assert!(signins.begin().is_some());
}

#[test]
fn a_refresh_token_is_kept_under_the_graph_name_and_forgotten_with_it() {
    let kept = Arc::new(Kept::default());
    let phone = Phone(Some(kept.clone()));
    keep_refresh(&phone, "Ada@Lumen.Example", "r1").unwrap();
    assert_eq!(kept.get("refresh:ada@lumen.example|graph").as_deref(), Some("r1"));
    kept.set("password:ada@lumen.example|graph", "p").unwrap();
    forget(&phone, "ada@lumen.example");
    assert!(kept.0.lock().unwrap().is_empty());
}

#[test]
fn nothing_is_kept_without_a_token_or_a_store() {
    let phone = Phone(Some(Arc::new(Kept::default())));
    assert!(keep_refresh(&phone, "ada@lumen.example", "").is_err());
    assert!(keep_refresh(&Phone(None), "ada@lumen.example", "r1").is_err());
}
