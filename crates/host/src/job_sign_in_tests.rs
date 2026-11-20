// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose sign-in a job's answer came with: the one rule for the calendar's and the to-dos' jobs.
//! Every address is invented.

use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, OAuth, OAuthProvider};

use crate::job_sign_in::foreign_sign_in;

fn account(provider: OAuthProvider) -> Account {
    Account {
        email: "ada@example.org".into(),
        username: "ada.l@example.org".into(),
        oauth: Some(OAuth { provider, ..Default::default() }),
        ..Default::default()
    }
}

fn naming(email: &str) -> Tokens {
    Tokens { refresh: "r".into(), email: email.into(), ..Default::default() }
}

#[test]
fn the_accounts_own_sign_in_and_one_that_names_nobody_are_its_own() {
    let a = account(OAuthProvider::Microsoft);
    assert!(foreign_sign_in(&a, None).is_none());
    assert!(foreign_sign_in(&a, Some(&naming(""))).is_none());
    assert!(foreign_sign_in(&a, Some(&naming(" Ada@Example.org "))).is_none());
    assert!(foreign_sign_in(&a, Some(&naming("ada.l@example.org"))).is_none(), "its user name");
}

#[test]
fn somebody_elses_is_refused_and_named_for_the_screen_alone() {
    let a = account(OAuthProvider::Microsoft);
    let foreign = foreign_sign_in(&a, Some(&naming(" mallory@example.net "))).unwrap();
    assert_eq!(foreign.signed_in_as, "mallory@example.net");
    assert!(!format!("{foreign:?}").contains("mallory"), "never in a Debug output");
    // A Google identity must be the address itself, not a user name.
    let g = account(OAuthProvider::Google);
    assert!(foreign_sign_in(&g, Some(&naming("ada.l@example.org"))).is_some());
    // What is handed on to be shown is cut to size.
    let long = format!("{}@example.net", "m".repeat(2_000));
    assert_eq!(foreign_sign_in(&a, Some(&naming(&long))).unwrap().signed_in_as.chars().count(), 320);
}
