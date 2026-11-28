// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Google password page says how an app password is made, for an owner who has never made
//! one. All the addresses in these tests are synthetic.

use super::*;

fn registered() -> MailApp {
    let mut app = app(false);
    app.services.google_client_id = "mailcraft-example.apps.googleusercontent.com".into();
    app.services.google_client_secret = "synthetic-public-desktop-registration".into();
    app
}

#[test]
fn the_app_password_page_opens_the_steps_for_making_one() {
    let mut app = registered();
    sign_ins(&mut app, None);
    checks(&mut app, None);
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    click(&mut h, "Use an app password instead");
    assert_eq!(page(&h), Some(Page::Password));

    // Closed at first, so the page stays short for an owner who has a password ready.
    assert!(says(&h, "How do I make an app password?"));
    assert!(!says(&h, "2-Step Verification"));

    click(&mut h, "How do I make an app password?");
    assert!(wizard(&h).unwrap().app_password_steps);
    assert!(says(&h, "1. Turn on 2-Step Verification"));
    assert!(says(&h, "2. Open Google's App passwords page"));
    assert!(says(&h, "3. Copy the 16-letter password"));
    assert!(says(&h, "Advanced Protection"));
    assert!(h.query_by_role_and_label(Role::Link, "Open Google's App passwords page").is_some());

    click(&mut h, "How do I make an app password?");
    assert!(!says(&h, "2-Step Verification"));
}

#[test]
fn browser_sign_in_first_shows_no_app_password_steps() {
    let mut app = registered();
    sign_ins(&mut app, None);
    checks(&mut app, None);
    let mut h = open(app, json!({"email":"staff@example.org"}));
    connect_manually(&mut h);
    click(&mut h, "Google");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "Sign in with Google"));
    assert!(!says(&h, "How do I make an app password?"));
}
