// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, OAuth, OAuthProvider};

use super::credentials::*;

fn ids() -> ClientIds<'static> {
    ClientIds { microsoft_chosen: "", microsoft_built_in: "built-in-ms", google_chosen: "my-google", google_built_in: "built-in-g" }
}

#[test]
fn the_application_id_is_the_accounts_then_the_owners_then_the_builds() {
    let ms = OAuth { provider: OAuthProvider::Microsoft, ..Default::default() };
    assert_eq!(effective_oauth(ms.clone(), &ids()).client_id, "built-in-ms");
    assert_eq!(effective_oauth(ms.clone(), &ClientIds { microsoft_chosen: "mine", ..ids() }).client_id, "mine");
    assert_eq!(effective_oauth(OAuth { client_id: "its-own".into(), ..ms }, &ids()).client_id, "its-own");
    assert_eq!(effective_oauth(OAuth { provider: OAuthProvider::Google, ..Default::default() }, &ids()).client_id, "my-google");
}

#[test]
fn somebody_elses_sign_in_is_told_apart_and_a_nameless_one_is_the_accounts() {
    let a = Account { email: "me@example.org".into(), username: "me".into(), ..Default::default() };
    let tokens = |email: &str| Tokens { email: email.into(), ..Default::default() };
    assert!(!signed_in_as_other(&a, &tokens("")));
    assert!(!signed_in_as_other(&a, &tokens("ME@example.org")));
    assert!(signed_in_as_other(&a, &tokens("you@example.org")));
    // A Google identity matches the address alone, not the editable user name.
    let g = Account {
        oauth: Some(OAuth { provider: OAuthProvider::Google, ..Default::default() }),
        email: "me@example.org".into(),
        username: "you@example.org".into(),
        ..Default::default()
    };
    assert!(signed_in_as_other(&g, &tokens("you@example.org")));
    assert!(!signed_in_as_other(&g, &tokens("me@example.org")));
}

#[test]
fn a_job_starts_with_the_kept_refresh_token_else_the_remembered_one_else_none() {
    assert_eq!(refresh_token(Some("kept"), Some("stored".into())), "kept");
    assert_eq!(refresh_token(Some(""), Some("stored".into())), "stored");
    assert_eq!(refresh_token(None, None), "");
}

#[test]
fn an_access_token_is_kept_a_day_at_most_and_never_from_the_past() {
    let t = |at| Tokens { expires_at: at, ..Default::default() };
    assert_eq!(kept_for_session(t(0), 1_000).expires_at, 1_000);
    assert_eq!(kept_for_session(t(i64::MAX), 1_000).expires_at, 1_000 + KEEP_ACCESS_S);
    assert_eq!(kept_for_session(t(5_000), i64::MAX).expires_at, i64::MAX);
}
