// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! New Gmail/Google Workspace accounts prefer OAuth, with a deliberate password fallback.
//! All the addresses, tokens, and servers in these tests are synthetic.

use super::*;

fn registered() -> MailApp {
    let mut app = app(false);
    app.services.google_client_id = "mailcraft-example.apps.googleusercontent.com".into();
    app.services.google_client_secret = "synthetic-public-desktop-registration".into();
    app
}

#[test]
fn new_google_workspace_account_offers_browser_sign_in_first_without_a_password_field() {
    let mut app = registered();
    let signed_in = sign_ins(&mut app, None);
    let checked = checks(&mut app, None);
    lookups(&mut app, Some(Ok(vec!["aspmx.l.google.com"])));
    let mut h = open(app, json!({"email":"staff@example.org"}));
    click(&mut h, "Connect");

    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).unwrap().incoming.host, "imap.gmail.com");
    assert!(!wizard(&h).unwrap().google_app_password_selected);
    assert!(says(&h, "Sign in with Google"));
    assert!(says(&h, "Use an app password instead"));
    assert!(says(&h, "Recommended"));
    assert!(!says(&h, "It is never written to disk"));
    assert!(signed_in.lock().unwrap().is_empty());
    assert!(checked.lock().unwrap().is_empty());

    click(&mut h, "Sign in with Google");
    assert_eq!(page(&h), Some(Page::SignIn));
    assert_eq!(signed_in.lock().unwrap().len(), 1);
    assert!(checked.lock().unwrap().is_empty());
}

#[test]
fn password_setup_is_a_deliberate_reversible_choice_and_wipes_what_was_typed() {
    let mut app = registered();
    let signed_in = sign_ins(&mut app, None);
    checks(&mut app, None);
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(signed_in.lock().unwrap().is_empty());
    assert!(says(&h, "Sign in with Google"));
    assert!(!wizard(&h).unwrap().google_app_password_selected);

    click(&mut h, "Use an app password instead");
    assert!(wizard(&h).unwrap().google_app_password_selected);
    assert!(says(&h, "Use a Google app password"));
    assert!(says(&h, "Use Google sign-in instead"));
    h.get_by_role(Role::PasswordInput).focus();
    h.run_steps(2);
    type_text(&mut h, TYPED);
    assert_eq!(wizard(&h).unwrap().password.0, TYPED);

    click(&mut h, "Use Google sign-in instead");
    assert!(!wizard(&h).unwrap().google_app_password_selected);
    assert!(wizard(&h).unwrap().password.0.is_empty());
    assert!(says(&h, "Sign in with Google"));
    assert!(!says(&h, "Use Google sign-in instead"));
    assert!(signed_in.lock().unwrap().is_empty());
}

#[test]
fn missing_google_registration_shows_app_password_and_no_dead_sign_in_button() {
    let mut app = app(false);
    sign_ins(&mut app, None);
    let checked = checks(&mut app, None);
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");

    assert_eq!(page(&h), Some(Page::Password));
    assert!(!h.state().google_registration_ready());
    assert!(says(&h, "Google sign-in is not configured"));
    assert!(!says(&h, "Use an app password instead"));
    assert!(!says(&h, "Sign in with Google"));
    h.get_by_role(Role::PasswordInput).focus();
    h.run_steps(2);
    type_text(&mut h, TYPED);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Checking));
    assert_eq!(checked.lock().unwrap().len(), 1);
    assert!(checked.lock().unwrap()[0].0.oauth.is_none());
}

#[test]
fn non_google_server_still_uses_existing_password_setup() {
    let mut app = registered();
    checks(&mut app, None);
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Other (IMAP)");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(!says(&h, "Sign in with Google"));
    assert!(!says(&h, "Use an app password instead"));
    h.get_by_role(Role::PasswordInput).focus();
}

#[test]
fn a_stale_password_cannot_bypass_google_oauth_first_with_connect() {
    let mut app = registered();
    sign_ins(&mut app, None);
    let checked = checks(&mut app, None);
    let mut wizard = AccountSetup::new(&app, &json!({"email":"staff@example.org"}));
    wizard.page = Page::Choose;
    wizard.pick(&mut app, Kind::Google);
    assert_eq!(wizard.page, Page::Password);
    assert!(wizard.google_oauth_first(&app));

    // Even a stale value must not turn the recommended OAuth action into a
    // password-based connection before the owner chooses that alternative.
    wizard.password.0 = TYPED.into();
    assert!(!wizard.act(&mut app, Action::Go));
    assert_eq!(wizard.page, Page::Password);
    assert!(wizard.error.contains("Choose Google sign-in"));
    assert!(checked.lock().unwrap().is_empty());
    assert!(app.session.mailbox.accounts.iter().all(|a| a.email != "staff@example.org"));
}

#[test]
fn oauth_unavailable_google_app_password_fallback_completes_without_oauth_tokens() {
    let mut app = app(false);
    // A provider without a configured desktop client must never start browser OAuth.
    let sign_ins = sign_ins(&mut app, None);
    let checks = checks(&mut app, Some(Ok(())));
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "Google sign-in is not configured"));
    assert!(!says(&h, "Sign in with Google"));

    type_password(&mut h);
    click(&mut h, "Connect");
    h.run_steps(6);
    assert_eq!(page(&h), Some(Page::Done));
    assert_eq!(sign_ins.lock().unwrap().len(), 0);
    let asked = checks.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, TYPED);
    assert!(asked[0].0.oauth.is_none(), "fallback must use a password, not an OAuth token");
    drop(asked);

    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "staff@example.org").unwrap();
    assert!(account.oauth.is_none());
    assert_eq!(h.state().passwords.get(&account.id).map(String::as_str), Some(TYPED));
    assert!(!h.state().tokens.contains_key(&account.id));
}

#[test]
fn rejected_app_password_then_google_oauth_succeeds_without_credential_crossover() {
    let mut app = registered();
    let sign_ins = sign_ins(
        &mut app,
        Some(Ok(SignedIn {
            email: "staff@example.org".into(),
            access: "synthetic-google-access".into(),
            refresh: "synthetic-google-refresh".into(),
            ..signed_in("Staff")
        })),
    );
    let checks = checks(&mut app, Some(Err("sign-in failed: invalid password")));
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    assert_eq!(page(&h), Some(Page::Password));
    click(&mut h, "Use an app password instead");
    type_password(&mut h);
    click(&mut h, "Connect");
    h.run_steps(6);

    // Rejected app password cannot produce an account or a token, and the user can retry OAuth.
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "Google didn't accept the password"));
    assert_eq!(checks.lock().unwrap().len(), 1);
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != "staff@example.org"));
    assert!(h.state().passwords.is_empty());
    assert!(sign_ins.lock().unwrap().is_empty());

    click(&mut h, "Use Google sign-in instead");
    assert!(wizard(&h).unwrap().password.0.is_empty());
    assert!(!wizard(&h).unwrap().google_app_password_selected);
    click(&mut h, "Sign in with Google");
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::Done));
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    assert_eq!(checks.lock().unwrap().len(), 1);
    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "staff@example.org").unwrap();
    assert_eq!(account.oauth.as_ref().map(|oauth| oauth.provider), Some(OAuthProvider::Google));
    assert!(!h.state().passwords.contains_key(&account.id));
    assert_eq!(h.state().tokens.get(&account.id).map(|t| t.refresh.as_str()), Some("synthetic-google-refresh"));
    assert!(!serde_json::to_string(&h.state().session.mailbox).unwrap().contains(TYPED));
}

#[test]
fn configured_google_oauth_still_allows_an_explicit_successful_app_password_account() {
    let mut app = registered();
    let sign_ins = sign_ins(&mut app, None);
    let checks = checks(&mut app, Some(Ok(())));
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    assert!(says(&h, "Sign in with Google"));
    assert!(says(&h, "Use an app password instead"));
    click(&mut h, "Use an app password instead");
    assert!(!says(&h, "Sign in with Google"));
    type_password(&mut h);
    click(&mut h, "Connect");
    h.run_steps(6);

    assert_eq!(page(&h), Some(Page::Done));
    assert_eq!(checks.lock().unwrap().len(), 1);
    assert!(checks.lock().unwrap()[0].0.oauth.is_none());
    assert_eq!(checks.lock().unwrap()[0].1, TYPED);
    assert!(sign_ins.lock().unwrap().is_empty(), "explicit password fallback must not launch a Google browser");
    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "staff@example.org").unwrap();
    assert!(account.oauth.is_none(), "explicitly selected password account must not silently migrate");
    assert_eq!(h.state().passwords.get(&account.id).map(String::as_str), Some(TYPED));
    assert!(!h.state().tokens.contains_key(&account.id));
}
