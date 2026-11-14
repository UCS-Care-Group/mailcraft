// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Existing-account Google OAuth conversion uses only generated sample mail and fake credentials.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::SecretStore;
use mailcraft_model::sync::Tokens as SignedIn;

use super::*;

#[derive(Clone, Default)]
struct Stored(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Stored {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn existing_google_password_account() -> (MailApp, Id, Stored) {
    // Generated mail and folders must survive changing the existing account's sign-in method.
    let mut a = app(true);
    let id = a.session.mailbox.default_account().unwrap().id;
    assert!(!a.session.mailbox.messages.is_empty());
    a.execute(
        "account.edit",
        json!({
            "id": id,
            "email": "owner@example.org",
            "displayName": "Original Name",
            "username": "owner@example.org",
            "auth": "password",
            "kind": "imap",
            "incoming": {"host":"imap.gmail.com","port":993,"security":"tls"},
            "outgoing": {"host":"smtp.gmail.com","port":465,"security":"tls"}
        }),
    )
    .unwrap();
    a.services.google_client_id = "synthetic-native.apps.googleusercontent.com".into();
    a.services.google_client_secret = "synthetic-public-client-registration".into();
    let store = Stored::default();
    let owner = a.session.mailbox.account(id).unwrap();
    store.set(&MailApp::secret_key("password", owner), "saved-old-app-password").unwrap();
    a.passwords.insert(id, "memory-old-app-password".into());
    a.services.secrets = Some(Arc::new(store.clone()));
    (a, id, store)
}

fn mail_before(a: &MailApp) -> (serde_json::Value, serde_json::Value) {
    (serde_json::to_value(&a.session.mailbox.messages).unwrap(), serde_json::to_value(&a.session.mailbox.folders).unwrap())
}

fn tokens(email: &str) -> SignedIn {
    SignedIn {
        email: email.into(),
        access: "synthetic-access-token".into(),
        refresh: "synthetic-refresh-token".into(),
        ..signed_in("Synthetic Person")
    }
}

#[test]
fn opening_saving_or_cancelling_existing_app_password_settings_preserves_mail_and_credentials() {
    let (mut a, id, store) = existing_google_password_account();
    let original = mail_before(&a);
    let before = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert_eq!(w.page, Page::Settings);
    assert!(w.oauth.is_none() && w.edit_command().is_none());
    assert!(w.act(&mut a, Action::Back));
    assert_eq!(mail_before(&a), original);
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), before);
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(w.act(&mut a, Action::Go));
    assert_eq!(mail_before(&a), original);
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), before);
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some("memory-old-app-password"));
}

#[test]
fn same_identity_conversion_changes_only_auth_and_preserves_messages_folders_and_other_accounts() {
    let (mut a, id, store) = existing_google_password_account();
    let before = a.session.mailbox.account(id).unwrap().clone();
    let password_key = MailApp::secret_key("password", &before);
    let refresh_key = MailApp::secret_key("refresh", &before);
    let other = a
        .execute(
            "account.add",
            json!({
                "email":"other@example.org","auth":"oauth2","provider":"google",
                "incoming":{"host":"imap.gmail.com","port":993,"security":"tls"},
                "outgoing":{"host":"smtp.gmail.com","port":465,"security":"tls"}
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let other_key = MailApp::secret_key("refresh", a.session.mailbox.account(other).unwrap());
    store.set(&other_key, "unrelated-refresh").unwrap();
    // The second account itself adds folders; snapshot after adding it so conversion alone
    // is responsible for any subsequent changes.
    let original = mail_before(&a);
    let requested = sign_ins(&mut a, Some(Ok(tokens("owner@example.org"))));
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(w.oauth.is_none() && requested.lock().unwrap().is_empty());
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert_eq!(w.page, Page::SignIn);
    assert!(w.waiting && a.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(store.get(&password_key).as_deref(), Some("saved-old-app-password"));
    assert_eq!(mail_before(&a), original);
    a.poll_sign_in();
    w.poll_sign_in(&mut a);
    assert!(w.done);
    let after = a.session.mailbox.account(id).unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.email, before.email);
    assert_eq!(after.display_name, before.display_name);
    assert_eq!(after.incoming, before.incoming);
    assert_eq!(after.outgoing, before.outgoing);
    assert_eq!(after.username, before.username);
    assert_eq!(after.oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Google));
    assert_eq!(mail_before(&a), original);
    assert!(!a.passwords.contains_key(&id));
    assert!(store.get(&password_key).is_none());
    assert_eq!(store.get(&refresh_key).as_deref(), Some("synthetic-refresh-token"));
    assert_eq!(store.get(&other_key).as_deref(), Some("unrelated-refresh"));
}

#[test]
fn wrong_google_identity_cannot_replace_existing_account_or_credentials() {
    let (mut a, id, store) = existing_google_password_account();
    let original_mail = mail_before(&a);
    let before = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    sign_ins(&mut a, Some(Ok(tokens("stranger@example.org"))));
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    a.poll_sign_in();
    w.poll_sign_in(&mut a);
    assert_eq!(w.page, Page::Settings);
    assert!(w.error.contains("was not changed"));
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), before);
    assert_eq!(mail_before(&a), original_mail);
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some("memory-old-app-password"));
    assert!(!a.tokens.contains_key(&id));
}

#[test]
fn cancellation_and_denied_consent_preserve_existing_mail_and_passwords() {
    for denied in [false, true] {
        let (mut a, id, store) = existing_google_password_account();
        let before = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
        let original_mail = mail_before(&a);
        let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
        if denied {
            sign_ins(&mut a, Some(Err(crate::approval::SIGN_IN_DECLINED.into())));
        } else {
            sign_ins(&mut a, None);
        }
        let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
        assert!(!w.act(&mut a, Action::ConvertToGoogle));
        if denied {
            a.poll_sign_in();
            w.poll_sign_in(&mut a);
            assert_eq!(w.page, Page::SignIn);
            assert!(!w.act(&mut a, Action::GooglePassword));
        } else {
            assert!(!w.act(&mut a, Action::Back));
        }
        assert_eq!(w.page, Page::Settings);
        assert!(w.oauth.is_none());
        assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), before);
        assert_eq!(mail_before(&a), original_mail);
        assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
        assert_eq!(a.passwords.get(&id).map(String::as_str), Some("memory-old-app-password"));
    }
}

#[test]
fn conversion_rejects_unsaved_edits_missing_registration_and_different_saved_servers() {
    let (mut a, id, store) = existing_google_password_account();
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    let requested = sign_ins(&mut a, None);
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();

    w.name = "Unsaved display name".into();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert_eq!(w.page, Page::Settings);
    assert!(w.error.contains("Save or discard"));
    assert!(requested.lock().unwrap().is_empty());
    assert!(a.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));

    w.name = "Original Name".into();
    a.services.google_client_id.clear();
    a.services.google_client_secret.clear();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert!(w.error.contains("not configured"));
    assert!(requested.lock().unwrap().is_empty());
    a.services.google_client_id = "synthetic-native.apps.googleusercontent.com".into();
    a.services.google_client_secret = "synthetic-public-client-registration".into();

    // Changing unsaved UI server fields does not silently change the saved account.
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    w.outgoing.host = "smtp.other.example".into();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert!(w.error.contains("Save or discard"));
    assert!(requested.lock().unwrap().is_empty());
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));

    // And a saved non-Google SMTP relay is not eligible for token conversion.
    a.execute("account.edit", json!({"id":id,"outgoing":{"host":"smtp.other.example","port":465,"security":"tls"}})).unwrap();
    let after_edit = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert!(w.error.contains("standard IMAP and SMTP"));
    assert!(requested.lock().unwrap().is_empty());
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), after_edit);
}

#[test]
fn saved_settings_change_during_browser_authorization_is_detected_before_conversion() {
    let (mut a, id, _store) = existing_google_password_account();
    let original_mail = mail_before(&a);
    sign_ins(&mut a, Some(Ok(tokens("owner@example.org"))));
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    // A concurrent settings edit is not part of the configuration the owner approved.
    a.execute("account.edit", json!({"id":id,"outgoing":{"host":"smtp.gmail.com","port":587,"security":"starttls"}})).unwrap();
    let after_edit = serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap();
    a.poll_sign_in();
    w.poll_sign_in(&mut a);
    assert_eq!(w.page, Page::Settings);
    assert!(w.error.contains("was not changed"));
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), after_edit);
    assert!(a.session.mailbox.account(id).unwrap().oauth.is_none());
    assert!(!a.tokens.contains_key(&id));
    assert_eq!(mail_before(&a), original_mail);
}

#[test]
fn existing_google_oauth_account_can_edit_display_name_without_a_client_override_or_tenant() {
    let (mut a, id, store) = existing_google_password_account();
    a.execute("account.edit", json!({"id":id,"auth":"oauth2","provider":"google"})).unwrap();
    let saved = a.session.mailbox.account(id).unwrap().clone();
    assert_eq!(saved.oauth.as_ref().map(|oauth| oauth.provider), Some(OAuthProvider::Google));
    assert!(saved.oauth.as_ref().unwrap().client_id.is_empty());
    assert!(saved.oauth.as_ref().unwrap().tenant.is_empty());
    let mail = mail_before(&a);
    a.adopt_tokens(id, tokens("owner@example.org"));
    let refresh_key = MailApp::secret_key("refresh", &saved);
    assert_eq!(store.get(&refresh_key).as_deref(), Some("synthetic-refresh-token"));

    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert_eq!(w.page, Page::Settings);
    assert!(w.settings_problem(false).is_none());
    w.name = "Updated display name".into();
    assert_eq!(w.edit_command().unwrap().1, json!({"id":id,"displayName":"Updated display name"}));
    assert!(w.act(&mut a, Action::Go));
    let now = a.session.mailbox.account(id).unwrap();
    assert_eq!(now.display_name, "Updated display name");
    assert_eq!(now.oauth.as_ref().map(|oauth| oauth.provider), Some(OAuthProvider::Google));
    assert_eq!(now.incoming, saved.incoming);
    assert_eq!(now.outgoing, saved.outgoing);
    assert_eq!(mail_before(&a), mail);
    assert_eq!(store.get(&refresh_key).as_deref(), Some("synthetic-refresh-token"));
    assert_eq!(a.tokens.get(&id).map(|tokens| tokens.email.as_str()), Some("owner@example.org"));
}

#[test]
fn gmail_account_with_alternate_imap_username_must_be_fixed_before_conversion() {
    let (mut a, id, store) = existing_google_password_account();
    a.execute("account.edit", json!({"id":id,"username":"not-owner@example.org"})).unwrap();
    let account = a.session.mailbox.account(id).unwrap().clone();
    assert!(!google_conversion_ready(&account));
    let before = serde_json::to_value(&account).unwrap();
    let key = MailApp::secret_key("password", &account);
    let requested = sign_ins(&mut a, None);
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert!(w.error.contains("matching email user name"));
    assert!(requested.lock().unwrap().is_empty());
    assert_eq!(serde_json::to_value(a.session.mailbox.account(id).unwrap()).unwrap(), before);
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
}

#[test]
fn existing_gmail_account_settings_show_explicit_switch_and_cancel_retains_mail() {
    let (mut a, id, store) = existing_google_password_account();
    let before = mail_before(&a);
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    let requested = sign_ins(&mut a, None);
    let mut h = open(a, json!({"id":id}));

    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "keeps your downloaded messages and folders"));
    assert!(requested.lock().unwrap().is_empty());
    click(&mut h, "Switch to Google sign-in");
    assert_eq!(page(&h), Some(Page::SignIn));
    assert_eq!(requested.lock().unwrap().len(), 1);
    assert_eq!(mail_before(h.state()), before);
    click(&mut h, "Cancel");
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(h.state().session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(mail_before(h.state()), before);
    assert_eq!(store.get(&key).as_deref(), Some("saved-old-app-password"));
}

#[test]
fn existing_google_oauth_settings_do_not_offer_app_password_conversion() {
    let (mut a, id, _store) = existing_google_password_account();
    a.execute("account.edit", json!({"id":id,"auth":"oauth2","provider":"google"})).unwrap();
    let mut h = open(a, json!({"id":id}));
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Signs in on the Google page"));
    assert!(!says(&h, "Switch to Google sign-in"));
    click(&mut h, "Cancel");
    assert_eq!(page(&h), None);
}

#[test]
fn existing_gmail_account_converts_only_after_explicit_same_identity_sign_in() {
    let add = |app: &mut MailApp| {
        app.execute(
            "account.add",
            json!({
                "email":"one@gmail.com", "auth":"password",
                "incoming":{"host":"imap.gmail.com","port":993,"security":"tls"},
                "outgoing":{"host":"smtp.gmail.com","port":465,"security":"tls"}
            }),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap()
    };
    let set_up = || {
        let mut app = app(false);
        app.services.google_client_id = "example-desktop.apps.googleusercontent.com".into();
        app.services.google_client_secret = "example-public-desktop-registration".into();
        app
    };

    let mut app = set_up();
    let id = add(&mut app);
    app.passwords.insert(id, "existing-app-password".into());
    sign_ins(&mut app, Some(Ok(SignedIn { email: "one@gmail.com".into(), ..signed_in("One") })));
    let mut w = AccountSetup::open(&app, &json!({"id":id})).unwrap();
    assert_eq!(w.page, Page::Settings);
    assert!(w.oauth.is_none());
    assert!(!w.act(&mut app, Action::ConvertToGoogle));
    assert!(w.page == Page::SignIn && w.waiting);
    // No changes while Google's page is open, even if the owner has an app password.
    assert!(app.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(app.passwords.get(&id).map(String::as_str), Some("existing-app-password"));
    app.poll_sign_in();
    w.poll_sign_in(&mut app);
    assert!(w.done);
    let owner = app.session.mailbox.account(id).unwrap();
    assert_eq!(owner.kind, AccountKind::Imap);
    assert_eq!(owner.oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Google));
    assert_eq!(app.tokens.get(&id).map(|t| t.email.as_str()), Some("one@gmail.com"));
    assert!(!app.passwords.contains_key(&id));

    // A different Google account's tokens cannot convert this one.
    let mut app = set_up();
    let id = add(&mut app);
    app.passwords.insert(id, "existing-app-password".into());
    sign_ins(&mut app, Some(Ok(SignedIn { email: "two@gmail.com".into(), ..signed_in("Wrong account") })));
    let mut w = AccountSetup::open(&app, &json!({"id": id})).unwrap();
    assert!(!w.act(&mut app, Action::ConvertToGoogle));
    app.poll_sign_in();
    w.poll_sign_in(&mut app);
    assert!(w.page == Page::Settings && w.error.contains("was not changed"));
    assert!(app.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(app.passwords.get(&id).map(String::as_str), Some("existing-app-password"));
    assert!(!app.tokens.contains_key(&id));

    // Cancellation leaves the old account and password untouched too.
    let mut app = set_up();
    let id = add(&mut app);
    app.passwords.insert(id, "existing-app-password".into());
    sign_ins(&mut app, None);
    let mut w = AccountSetup::open(&app, &json!({"id": id})).unwrap();
    assert!(!w.act(&mut app, Action::ConvertToGoogle));
    assert!(!w.act(&mut app, Action::Back));
    assert!(w.page == Page::Settings && w.oauth.is_none());
    assert!(app.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(app.passwords.get(&id).map(String::as_str), Some("existing-app-password"));
}

#[test]
fn conversion_waits_for_an_unsaved_colour() {
    // A switch closes the dialog without saving: a colour picked and not saved would go (#893).
    let (mut a, id, _store) = existing_google_password_account();
    let requested = sign_ins(&mut a, None);
    let mut w = AccountSetup::open(&a, &json!({"id":id})).unwrap();
    let was = a.session.mailbox.account(id).unwrap().color;
    w.color = Some((was, Some(mailcraft_model::Rgb(0x12, 0x34, 0x56))));
    assert!(!w.act(&mut a, Action::ConvertToGoogle));
    assert_eq!(w.page, Page::Settings);
    assert!(w.error.contains("Save or discard"), "{}", w.error);
    assert!(requested.lock().unwrap().is_empty());
    assert!(a.session.mailbox.account(id).unwrap().oauth.is_none());
    assert_eq!(a.session.mailbox.account(id).unwrap().color, was);
}
