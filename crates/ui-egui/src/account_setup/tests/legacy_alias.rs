// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

/// Stand in for an Outlook.com sign-in that returns a different address from the one entered.
fn alias_setup(tenant: &str, mail_only: bool, theme: &str) -> Harness<'static, MailApp> {
    let mut a = app(false);
    a.execute("ui.theme", json!({"value": theme})).unwrap();
    a.services.microsoft_client_id = APP_ID.into();
    lookups(&mut a, Some(Ok(vec!["123456789.pamx1.hotmail.com"])));
    sign_ins(&mut a, Some(Ok(SignedIn { tenant: tenant.into(), ..signed_in_as("noor@outlook.com") })));
    syncs(&mut a);
    let mut h = open(a, json!({}));
    wizard_mut(&mut h).mail_only = mail_only;
    connect(&mut h, "noor@example.org");
    h.run_steps(4);
    assert_eq!(page(&h), Some(Page::SomeoneElse));
    h
}

/// Confirming a personal alias keeps the sending address and adopts the unmodified tokens for the login identity.
#[test]
fn a_confirmed_legacy_alias_keeps_the_custom_domain_address() {
    for (mail_only, theme) in [(false, "light"), (true, "dark")] {
        let mut h = alias_setup(Directory::PERSONAL_TENANT, mail_only, theme);
        assert_eq!(addresses(&h), [""]);
        assert!(h.state().tokens.is_empty());
        assert!(says(&h, "Only keep your custom-domain address if both addresses are aliases"));
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("legacy-alias-{theme}.png"))).unwrap();
        }
        click(&mut h, "Keep noor@example.org");
        assert_eq!(page(&h), Some(Page::Done));
        let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "noor@example.org").cloned().unwrap();
        assert_eq!(account.username, "noor@outlook.com");
        assert_eq!(account.kind, if mail_only { AccountKind::Imap } else { AccountKind::Microsoft });
        let tokens = h.state().tokens.get(&account.id).cloned().unwrap();
        assert_eq!((tokens.email.as_str(), tokens.access.as_str()), ("noor@outlook.com", "tok-1"));
        assert!(!h.state_mut().signed_in_as_other(account.id, &tokens));
    }
}

/// Organisational and unidentified sign-ins cannot use the personal-mailbox alias override.
#[test]
fn a_nonpersonal_sign_in_does_not_offer_custom_domain_alias_confirmation() {
    for tenant in ["organizations", ""] {
        let h = alias_setup(tenant, false, "light");
        assert!(!wizard(&h).unwrap().can_keep_alias());
        assert!(h.query_by_role_and_label(Role::Button, "Keep noor@example.org").is_none());
        assert_eq!(addresses(&h), [""]);
        assert!(h.state().tokens.is_empty());
    }
}

/// A failed add keeps the original wizard identity and tokens available for an explicit retry.
#[test]
fn a_confirmed_alias_that_cannot_be_added_restores_the_pending_sign_in() {
    let mut h = alias_setup(Directory::PERSONAL_TENANT, false, "light");
    h.state_mut().execute("account.add", json!({"email": "noor@example.org"})).unwrap();
    click(&mut h, "Keep noor@example.org");
    assert_eq!(page(&h), Some(Page::SomeoneElse));
    let w = wizard(&h).unwrap();
    assert_eq!((w.email.as_str(), w.username.as_str()), ("noor@example.org", "noor@example.org"));
    assert!(w.someone_else.is_some() && w.can_keep_alias());
    assert!(says(&h, "already set up"));
    assert!(h.state().tokens.is_empty());
    let existing = h.state().session.mailbox.accounts.iter().find(|a| a.email == "noor@example.org").map(|a| a.id).unwrap();
    h.state_mut().execute("account.edit", json!({"id": existing, "email": "old@example.org"})).unwrap();
    click(&mut h, "Keep noor@example.org");
    assert_eq!(page(&h), Some(Page::Done));
    let added = h.state().session.mailbox.accounts.iter().find(|a| a.email == "noor@example.org").unwrap();
    assert!(h.state().tokens.contains_key(&added.id));
}
