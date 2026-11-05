// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keyboard-first, compact-layout and light/dark Google sign-in tests.
//! Fake Google Workspace discovery and synthetic credentials only: no real mailbox or browser.

use super::*;

fn google_window(theme: &str, size: (f32, f32)) -> (Harness<'static, MailApp>, SignIns) {
    let mut a = app(false);
    a.execute("ui.theme", json!({"value":theme})).unwrap();
    a.services.google_client_id = "synthetic-desktop.apps.googleusercontent.com".into();
    a.services.google_client_secret = "synthetic-public-desktop-registration".into();
    let requests = sign_ins(&mut a, None);
    lookups(&mut a, Some(Ok(vec!["aspmx.l.google.com"])));
    a.execute("ui.dialog", json!({"name":"account","email":"staff@example.org"})).unwrap();
    let mut h = window(a, size);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).unwrap().incoming.host, "imap.gmail.com");
    (h, requests)
}

#[test]
fn google_primary_button_works_with_keyboard_in_both_themes_and_at_narrow_sizes() {
    for theme in ["light", "dark"] {
        for size in [(760.0, 480.0), (480.0, 320.0), (320.0, 200.0)] {
            let (mut h, requested) = google_window(theme, size);
            assert!(says(&h, "Sign in with Google"), "{theme} {size:?}");
            assert!(says(&h, "Recommended"));
            assert!(!says(&h, "Microsoft's sign-in page"));
            assert!(requested.lock().unwrap().is_empty());

            h.get_by_role_and_label(Role::Button, "Sign in with Google").focus();
            h.run_steps(2);
            press(&mut h, Key::Enter);
            assert_eq!(page(&h), Some(Page::SignIn), "{theme} {size:?}");
            assert!(says(&h, "Google's sign-in page is open"));
            assert!(!says(&h, "If that page says you need admin approval"));
            assert_eq!(requested.lock().unwrap().len(), 1);
            click(&mut h, "Cancel");
            assert_eq!(page(&h), Some(Page::Password));
            assert_eq!(wizard(&h).unwrap().email, "staff@example.org");
        }
    }
}

#[test]
fn google_app_password_alternative_is_keyboard_reachable_even_in_a_small_window() {
    for theme in ["light", "dark"] {
        for size in [(760.0, 480.0), (480.0, 320.0), (320.0, 200.0)] {
            let (mut h, requested) = google_window(theme, size);
            // The fallback belongs to the scrollable body, so keyboard focus must still
            // reach it when explanatory text does not fit vertically.
            h.get_by_role_and_label(Role::Button, "Use an app password instead").focus();
            h.run_steps(2);
            press(&mut h, Key::Enter);
            assert_eq!(page(&h), Some(Page::Password));
            assert!(wizard(&h).unwrap().google_app_password_selected, "{theme} {size:?}");
            assert!(says(&h, "Use Google sign-in instead"));
            assert!(requested.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn google_account_settings_have_a_compact_footer_and_offer_explicit_sign_in_actions() {
    for theme in ["light", "dark"] {
        for size in [(760.0, 480.0), (480.0, 320.0)] {
            let mut a = app(false);
            a.execute("ui.theme", json!({"value":theme})).unwrap();
            a.services.google_client_id = "synthetic-desktop.apps.googleusercontent.com".into();
            a.services.google_client_secret = "synthetic-public-desktop-registration".into();
            sign_ins(&mut a, None);
            let id = a
                .execute(
                    "account.add",
                    json!({
                        "email":"staff@example.org", "auth":"password",
                        "username":"staff@example.org",
                        "incoming":{"host":"imap.gmail.com","port":993,"security":"tls"},
                        "outgoing":{"host":"smtp.gmail.com","port":465,"security":"tls"}
                    }),
                )
                .unwrap()["id"]
                .as_u64()
                .unwrap();
            a.execute("ui.dialog", json!({"name":"account","id":id})).unwrap();
            let mut h = window(a, size);
            assert_eq!(page(&h), Some(Page::Settings));
            assert!(says(&h, "Switch to Google sign-in"));
            assert!(says(&h, "Save"));
            assert!(says(&h, "Cancel"));
            click(&mut h, "Switch to Google sign-in");
            assert_eq!(page(&h), Some(Page::SignIn), "{theme} {size:?}");
            click(&mut h, "Cancel");
            assert_eq!(page(&h), Some(Page::Settings));
            assert!(h.state().session.mailbox.account(id).unwrap().oauth.is_none());

            // Reconnecting an already OAuth-enabled account is a distinct, keyboard-accessible
            // action, and a canceled browser request must leave it OAuth-enabled.
            // Close the password-account settings before replacing our fake fixture with
            // an OAuth-enabled account, as the real owner would between sessions.
            click(&mut h, "Cancel");
            assert_eq!(page(&h), None);
            h.state_mut().execute("account.edit", json!({"id":id,"auth":"oauth2","provider":"google"})).unwrap();
            h.state_mut().execute("ui.dialog", json!({"name":"account","id":id})).unwrap();
            h.run_steps(5);
            assert_eq!(page(&h), Some(Page::Settings));
            assert!(says(&h, "Reconnect with Google"));
            click(&mut h, "Reconnect with Google");
            assert_eq!(page(&h), Some(Page::SignIn), "{theme} {size:?}");
            click(&mut h, "Cancel");
            assert_eq!(page(&h), Some(Page::Settings));
            assert_eq!(h.state().session.mailbox.account(id).unwrap().oauth.as_ref().map(|o| o.provider), Some(OAuthProvider::Google));
        }
    }
}

#[test]
fn google_wrong_account_recovery_actions_are_usable_in_both_themes_at_compact_widths() {
    for theme in ["light", "dark"] {
        for size in [(760.0, 480.0), (480.0, 320.0), (320.0, 200.0)] {
            let (mut h, requests) = google_window(theme, size);
            {
                let w = wizard_mut(&mut h);
                w.page = Page::SomeoneElse;
                w.oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
                w.someone_else = Some(SignedIn { email: "other@example.org".into(), ..signed_in("Other") });
            }
            h.run_steps(5);
            assert_eq!(page(&h), Some(Page::SomeoneElse));
            assert!(says(&h, "Sign in again"));
            assert!(says(&h, "Use an app password"));
            assert!(says(&h, "Change email address"));
            assert!(!says(&h, "admin approval"));
            assert!(requests.lock().unwrap().is_empty());

            h.get_by_role_and_label(Role::Button, "Change email address").focus();
            h.run_steps(2);
            press(&mut h, Key::Enter);
            assert_eq!(page(&h), Some(Page::Address), "{theme} {size:?}");
        }
    }
}

#[test]
fn google_timeout_recovery_is_keyboard_usable_in_both_themes_at_compact_widths() {
    for theme in ["light", "dark"] {
        for size in [(760.0, 480.0), (480.0, 320.0), (320.0, 200.0)] {
            let (mut h, requests) = google_window(theme, size);
            {
                let w = wizard_mut(&mut h);
                w.page = Page::SignIn;
                w.oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
                w.waiting = false;
                w.ended = Some(SignInEnd::TimedOut);
            }
            h.run_steps(5);
            assert!(says(&h, "Google sign-in did not finish"));
            assert!(says(&h, "Error 403"));
            assert!(!says(&h, "My organisation has to approve MailCraft first"));
            assert!(says(&h, "Try again"));
            assert!(says(&h, "Use an app password"));

            h.get_by_role_and_label(Role::Button, "Use an app password").focus();
            h.run_steps(2);
            press(&mut h, Key::Enter);
            assert_eq!(page(&h), Some(Page::Password), "{theme} {size:?}");
            assert!(wizard(&h).unwrap().google_app_password_selected);
            assert!(requests.lock().unwrap().is_empty());
        }
    }
}
