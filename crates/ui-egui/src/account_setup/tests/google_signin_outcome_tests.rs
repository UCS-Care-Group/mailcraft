// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic Google OAuth outcomes, retry, cancellation, and reconnect tests.
//! Nothing here calls Google or a real mail server.

use super::*;
use crate::approval::{GOOGLE_SIGN_IN_BLOCKED, GOOGLE_SIGN_IN_DECLINED, GOOGLE_SIGN_IN_TIMED_OUT, NO_BROWSER};

fn google_app() -> MailApp {
    let mut app = app(false);
    app.services.google_client_id = "synthetic.apps.googleusercontent.com".into();
    app.services.google_client_secret = "public-desktop-registration-for-tests".into();
    // Stand-in MX discovery: the invented example.org domain uses Google's IMAP/SMTP
    // servers, without asking public DNS or contacting any mailbox.
    lookups(&mut app, Some(Ok(vec!["aspmx.l.google.com"])));
    app
}

fn signed_google(email: &str, suffix: &str) -> SignedIn {
    SignedIn {
        email: email.into(),
        access: format!("fake-access-{suffix}"),
        refresh: format!("fake-refresh-{suffix}"),
        ..signed_in("Example Tester")
    }
}

fn existing_google(app: &mut MailApp) -> Id {
    app.execute(
        "account.add",
        json!({
            "email":"owner@example.org", "auth":"oauth2", "provider":"google",
            "incoming":{"host":"imap.gmail.com","port":993,"security":"tls"},
            "outgoing":{"host":"smtp.gmail.com","port":465,"security":"tls"}
        }),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn successful_google_callback_adds_only_the_requested_google_account() {
    let mut a = google_app();
    let requests = sign_ins(&mut a, Some(Ok(signed_google("one@example.org", "first"))));
    let mut h = open(a, json!({"email":"one@example.org"}));
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Password));
    click(&mut h, "Sign in with Google");
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::Done));
    let mailboxes: Vec<_> = h.state().session.mailbox.accounts.iter().filter(|account| !account.email.is_empty()).collect();
    assert_eq!(mailboxes.len(), 1);
    assert_eq!(mailboxes[0].email, "one@example.org");
    assert_eq!(mailboxes[0].oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Google));
    assert!(!h.state().passwords.contains_key(&mailboxes[0].id));
    assert_eq!(h.state().tokens.get(&mailboxes[0].id).map(|tokens| tokens.email.as_str()), Some("one@example.org"));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn cancel_google_browser_preserves_detected_account_settings_for_retry_or_fallback() {
    let mut a = google_app();
    let requests = sign_ins(&mut a, None);
    let mut h = open(a, json!({"email":"one@example.org"}));
    click(&mut h, "Connect");
    click(&mut h, "Sign in with Google");
    assert_eq!(page(&h), Some(Page::SignIn));
    click(&mut h, "Cancel");
    assert_eq!(page(&h), Some(Page::Password));
    let wizard = wizard(&h).unwrap();
    assert_eq!(wizard.email, "one@example.org");
    assert_eq!(wizard.incoming.host, "imap.gmail.com");
    assert_eq!(wizard.outgoing.host, "smtp.gmail.com");
    assert!(wizard.oauth.is_none());
    assert!(says(&h, "Sign in with Google"));
    assert!(says(&h, "Use an app password instead"));
    assert_eq!(h.state().session.mailbox.accounts.iter().filter(|account| !account.email.is_empty()).count(), 0);
    click(&mut h, "Sign in with Google");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn google_denial_timeout_browser_failure_and_workspace_block_have_specific_recovery() {
    for (why, title, help) in [
        (GOOGLE_SIGN_IN_DECLINED, "Google sign-in was declined", "was not changed"),
        (GOOGLE_SIGN_IN_TIMED_OUT, "Google sign-in did not finish", "Error 403"),
        (NO_BROWSER, "Could not open Google sign-in", "system browser"),
        (GOOGLE_SIGN_IN_BLOCKED, "Google blocked access to MailCraft", "Test users"),
    ] {
        let mut a = google_app();
        let requested = sign_ins(&mut a, Some(Err(why.into())));
        let mut h = open(a, json!({"email":"one@example.org"}));
        click(&mut h, "Connect");
        click(&mut h, "Sign in with Google");
        h.run_steps(7);
        assert_eq!(page(&h), Some(Page::SignIn), "{why}");
        assert!(says(&h, title), "{title}");
        assert!(says(&h, help), "{help}");
        assert!(!says(&h, "I'm an administrator: approve it for my organisation"));
        assert_eq!(h.state().session.mailbox.accounts.iter().filter(|a| !a.email.is_empty()).count(), 0);
        assert_eq!(requested.lock().unwrap().len(), 1);

        click(&mut h, "Try again");
        assert_eq!(requested.lock().unwrap().len(), 2, "retry: {why}");
        // Retry still makes no account until the actual signed-in identity is returned.
        assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email != "one@example.org"));
        h.run_steps(6);
        click(&mut h, "Use an app password");
        assert_eq!(page(&h), Some(Page::Password), "{why}");
        assert_eq!(wizard(&h).unwrap().email, "one@example.org");
        assert!(wizard(&h).unwrap().google_app_password_selected);
        assert!(!h.state().signing_in());
    }
}

#[test]
fn wrong_google_identity_requires_retry_or_a_separate_new_account_setup() {
    let mut a = google_app();
    let requested = sign_ins(&mut a, Some(Ok(signed_google("other@example.org", "other"))));
    let mut h = open(a, json!({"email":"one@example.org"}));
    click(&mut h, "Connect");
    click(&mut h, "Sign in with Google");
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::SomeoneElse));
    assert!(says(&h, "This isn't the account you typed"));
    assert!(says(&h, "No account has been added"));
    assert_eq!(requested.lock().unwrap().len(), 1);
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email.is_empty()));
    // Even a fabricated command cannot silently adopt the other Google account.
    assert_eq!(page(&h), Some(Page::SomeoneElse));
    assert!(h.state().session.mailbox.accounts.iter().all(|a| a.email.is_empty()));
    click(&mut h, "Use an app password");
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).unwrap().email, "one@example.org");
    assert!(wizard(&h).unwrap().google_app_password_selected);
}

#[test]
fn external_cancel_retains_new_google_account_setup_for_a_later_retry() {
    let mut a = google_app();
    sign_ins(&mut a, None);
    let mut h = open(a, json!({"email":"one@example.org"}));
    click(&mut h, "Connect");
    click(&mut h, "Sign in with Google");
    assert!(h.state().signing_in());
    h.state_mut().execute("ui.cancelSignIn", json!({})).unwrap();
    h.run_steps(6);
    assert!(!h.state().signing_in());
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).unwrap().email, "one@example.org");
    assert!(says(&h, "Sign in with Google"));
}

#[test]
fn reconnect_google_success_preserves_the_existing_mailbox_and_replaces_only_tokens() {
    let mut a = google_app();
    let id = existing_google(&mut a);
    let saved = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    a.adopt_tokens(id, signed_google("owner@example.org", "old"));
    let requested = sign_ins(&mut a, Some(Ok(signed_google("owner@example.org", "new"))));
    let mut h = open(a, json!({"id":id}));
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Reconnect with Google"));
    click(&mut h, "Reconnect with Google");
    h.run_steps(8);
    assert!(wizard(&h).is_none());
    assert_eq!(serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap(), saved);
    assert_eq!(h.state().tokens.get(&id).map(|token| token.refresh.as_str()), Some("fake-refresh-new"));
    assert_eq!(requested.lock().unwrap().len(), 1);
}

#[test]
fn reconnect_timeout_shows_retry_and_leaves_old_authentication_intact() {
    let mut a = google_app();
    let id = existing_google(&mut a);
    a.adopt_tokens(id, signed_google("owner@example.org", "old"));
    let saved = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    sign_ins(&mut a, Some(Err(GOOGLE_SIGN_IN_TIMED_OUT.into())));
    let mut h = open(a, json!({"id":id}));
    click(&mut h, "Reconnect with Google");
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::SignIn));
    assert!(says(&h, "Google sign-in did not finish"));
    assert!(says(&h, "Error 403"));
    assert!(says(&h, "Back to account settings"));
    click(&mut h, "Back to account settings");
    assert_eq!(page(&h), Some(Page::Settings));
    assert_eq!(serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap(), saved);
    assert_eq!(h.state().tokens.get(&id).map(|token| token.refresh.as_str()), Some("fake-refresh-old"));
}

#[test]
fn reconnect_with_different_google_identity_does_not_overwrite_existing_token() {
    let mut a = google_app();
    let id = existing_google(&mut a);
    a.adopt_tokens(id, signed_google("owner@example.org", "old"));
    let saved = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    sign_ins(&mut a, Some(Ok(signed_google("stranger@example.org", "new"))));
    let mut h = open(a, json!({"id":id}));
    click(&mut h, "Reconnect with Google");
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Google did not confirm this unchanged account"));
    assert_eq!(serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap(), saved);
    assert_eq!(h.state().tokens.get(&id).map(|token| token.refresh.as_str()), Some("fake-refresh-old"));
}

#[test]
fn existing_google_reconnect_can_be_cancelled_without_losing_account_settings() {
    let mut a = google_app();
    let id = existing_google(&mut a);
    a.adopt_tokens(id, signed_google("owner@example.org", "old"));
    sign_ins(&mut a, None);
    let saved = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    let mut h = open(a, json!({"id":id}));
    click(&mut h, "Reconnect with Google");
    assert_eq!(page(&h), Some(Page::SignIn));
    click(&mut h, "Cancel");
    assert_eq!(page(&h), Some(Page::Settings));
    assert_eq!(wizard(&h).unwrap().oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Google));
    assert_eq!(serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap(), saved);
    assert_eq!(h.state().tokens.get(&id).map(|token| token.refresh.as_str()), Some("fake-refresh-old"));
}

#[test]
fn concurrent_account_edit_during_reconnect_cannot_replace_its_tokens() {
    use std::sync::{Arc, Mutex, mpsc};
    let mut a = google_app();
    let id = existing_google(&mut a);
    a.adopt_tokens(id, signed_google("owner@example.org", "old"));
    let sender: Arc<Mutex<Option<mpsc::Sender<Result<SignedIn, String>>>>> = Default::default();
    let out = sender.clone();
    a.services.sign_in = Some(Box::new(move |_request| {
        let (tx, rx) = mpsc::channel();
        *out.lock().unwrap() = Some(tx);
        rx
    }));
    let mut h = open(a, json!({"id":id}));
    click(&mut h, "Reconnect with Google");
    assert_eq!(page(&h), Some(Page::SignIn));
    h.state_mut().execute("account.edit", json!({"id":id, "displayName":"Changed in another window"})).unwrap();
    let edited = serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap();
    sender.lock().unwrap().take().unwrap().send(Ok(signed_google("owner@example.org", "new"))).unwrap();
    h.run_steps(8);
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Google did not confirm this unchanged account"));
    assert_eq!(serde_json::to_value(h.state().session.mailbox.account(id).unwrap()).unwrap(), edited);
    assert_eq!(h.state().tokens.get(&id).map(|tokens| tokens.refresh.as_str()), Some("fake-refresh-old"));
}

#[test]
fn google_wizard_hides_microsoft_approval_and_returns_to_app_password_without_data_loss() {
    let mut a = app(false);
    a.services.google_client_id = "fake-desktop-client.apps.googleusercontent.com".into();
    a.services.google_client_secret = "fake-desktop-registration".into();
    sign_ins(&mut a, None); // The user is still on Google's simulated sign-in page.
    let mut h = open(a, json!({"email": "one@gmail.com"}));
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Password));
    click(&mut h, "Sign in with Google");
    assert_eq!(page(&h), Some(Page::SignIn));
    assert!(says(&h, "Google's sign-in page is open"));
    assert!(says(&h, "restrict sign-in to approved test users"));
    assert!(!says(&h, "If that page says you need admin approval"));
    assert!(h.state().signing_in());
    click(&mut h, "Cancel");
    assert!(!h.state().signing_in());

    let mut b = app(false);
    b.services.google_client_id = "fake-desktop-client.apps.googleusercontent.com".into();
    b.services.google_client_secret = "fake-desktop-registration".into();
    sign_ins(&mut b, Some(Err(crate::approval::SIGN_IN_DECLINED.into())));
    let mut h = open(b, json!({"email": "one@gmail.com"}));
    click(&mut h, "Connect");
    click(&mut h, "Sign in with Google");
    h.run_steps(4);
    assert_eq!(page(&h), Some(Page::SignIn));
    assert!(says(&h, "Google sign-in was declined"));
    assert!(!says(&h, "My organisation has to approve MailCraft"));
    click(&mut h, "Use an app password");
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).unwrap().email, "one@gmail.com");
    assert!(wizard(&h).unwrap().oauth.is_none());
    assert!(says(&h, "Use a Google app password"));
}
