// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Provider hints belong only to the explicitly imported address.

use super::*;

#[test]
fn changing_an_imported_address_discards_its_provider_before_connecting() {
    for edited in ["person@outlook.com", "", "not an address"] {
        let mut app = app(false);
        let source = crate::gnome_accounts::Account {
            email: "owner@gmail.com".into(),
            name: "Example Owner".into(),
            provider_name: "Google".into(),
            provider: crate::gnome_accounts::Provider::Google,
            linked: None,
        };
        let mut setup = AccountSetup::from_gnome(&app, &source);
        setup.email = edited.into();
        setup.connect(&mut app);
        assert!(setup.imported_kind.is_none());
        assert!(!setup.incoming.host.contains("gmail") && !setup.outgoing.host.contains("gmail"));
        assert!(setup.oauth.as_ref().is_none_or(|oauth| oauth.provider != OAuthProvider::Google));
    }
}
