// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #438: the password's eye on Add Account's password page and in Account Settings. Hidden at
//! first; a click shows the password and the next hides it; a new page, the end of the dialog
//! or the account being added hides it again; and shown, it is still never spoken. (The field
//! itself, and the Sign In dialog's, are in `secret_field_tests.rs`.)

use super::*;
use crate::secret_field::eye_name;

/// Whether the eye says `shown`, by its name and by the wizard.
fn eye_says(h: &Harness<'static, MailApp>, shown: bool) -> bool {
    h.query_by_role_and_label(Role::Button, eye_name(shown)).is_some() && wizard(h).is_some_and(|w| w.reveal == shown)
}

fn click_eye(h: &mut Harness<'static, MailApp>) {
    let shown = wizard(h).is_some_and(|w| w.reveal);
    click(h, eye_name(shown));
}

/// Whether anything screen readers and AccessKit are given has the password in it.
fn spoken(h: &Harness<'static, MailApp>) -> bool {
    let leaks =
        |n: &egui_kittest::kittest::AccessKitNode<'_>| n.value().is_some_and(|v| v.contains(TYPED)) || n.label().is_some_and(|l| l.contains(TYPED));
    h.query_all(egui_kittest::kittest::By::new().include_labels().predicate(leaks)).next().is_some()
}

/// The password page for a Google Workspace address, with the password typed.
fn password_page(a: MailApp) -> Harness<'static, MailApp> {
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@example.org");
    assert_eq!(page(&h), Some(Page::Password));
    type_password(&mut h);
    h
}

#[test]
fn the_eye_shows_the_password_until_the_next_click_or_page() {
    let mut a = app(false);
    lookups(&mut a, Some(Ok(vec!["aspmx.l.google.com"])));
    checks(&mut a, Some(Err("imap.gmail.com refused the sign-in")));
    syncs(&mut a);
    let mut h = password_page(a);
    // Hidden at first: dots only, and the eye offers to show it.
    assert!(eye_says(&h, false) && !says(&h, TYPED) && !spoken(&h));
    click_eye(&mut h);
    assert!(eye_says(&h, true));
    // Shown, it is still a password's field, and nothing in the tree has it.
    assert!(h.query_by_role(Role::PasswordInput).is_some() && !spoken(&h) && !says(&h, TYPED));
    click_eye(&mut h);
    assert!(eye_says(&h, false));

    // The servers refuse it: back on the page, through Checking, and the password is dots again.
    click_eye(&mut h);
    press(&mut h, Key::Enter);
    assert!(page(&h) == Some(Page::Password) && says(&h, "refused the sign-in"));
    assert!(eye_says(&h, false));

    // Back to the address and on again: hidden.
    click_eye(&mut h);
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Address));
    assert!(wizard(&h).is_some_and(|w| !w.reveal));
    press(&mut h, Key::Enter);
    assert!(page(&h) == Some(Page::Password) && eye_says(&h, false));
}

#[test]
fn the_account_being_added_hides_it() {
    let mut a = app(false);
    lookups(&mut a, Some(Ok(vec!["aspmx.l.google.com"])));
    checks(&mut a, Some(Ok(())));
    syncs(&mut a);
    let mut h = password_page(a);
    click_eye(&mut h);
    assert!(eye_says(&h, true));
    click(&mut h, "Connect");
    // Added: the wizard has gone on, and the password is shown nowhere.
    assert!(page(&h) != Some(Page::Password) && wizard(&h).is_none_or(|w| !w.reveal));
    assert!(h.query_by_role(Role::PasswordInput).is_none() && !spoken(&h));
}

#[test]
fn account_settings_have_the_eye_and_closing_hides_it() {
    let mut a = app(true);
    checks(&mut a, None);
    syncs(&mut a);
    let server = json!({"email": "noor@lumen.example", "incoming": {"host": "imap.lumen.example"}, "outgoing": {"host": "smtp.lumen.example"}});
    let id = a.execute("account.add", server).unwrap()["id"].as_u64().unwrap();
    let mut h = open(a, json!({"id": id}));
    assert_eq!(page(&h), Some(Page::Settings));
    type_password(&mut h);
    assert!(eye_says(&h, false));
    click_eye(&mut h);
    assert!(eye_says(&h, true) && !spoken(&h));
    // Cancel: the dialog goes, and when it opens again the password field starts hidden.
    click(&mut h, "Cancel");
    assert_eq!(page(&h), None);
    h.state_mut().execute("ui.dialog", json!({"name": "account", "id": id})).unwrap();
    h.run_steps(4);
    assert!(page(&h) == Some(Page::Settings) && eye_says(&h, false));
}
