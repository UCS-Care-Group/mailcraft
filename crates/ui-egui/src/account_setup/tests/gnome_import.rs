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

#[test]
fn linked_account_settings_save_local_preferences_together() {
    let mut app = app(false);
    let added = app
        .execute(
            "account.add",
            json!({
                "email": "owner@example.org",
                "kind": "imap",
                "username": "owner",
                "incoming": {"host": "imap.example.org", "port": 993, "security": "tls"},
                "outgoing": {"host": "smtp.example.org", "port": 465, "security": "tls"},
                "goa": {"id": "one", "provider": "imap_smtp"}
            }),
        )
        .unwrap();
    let id = added["id"].as_u64().unwrap();
    let before = app.session.mailbox.account(id).unwrap().clone();
    app.session.mailbox.signatures.push(mailcraft_model::Signature::plain_text("Work", "Example"));
    let mut settings = AccountSetup::open(&app, &json!({"id":id})).unwrap();
    settings.account_name = "Work mail".into();
    settings.name = "My sender name".into();
    settings.color.as_mut().unwrap().1 = Some(Rgb(12, 34, 56));
    settings.signature_changes = [Some("Work".into()), Some("Work".into())];
    assert!(settings.save(&mut app), "{}", settings.error);
    let account = app.session.mailbox.account(id).unwrap();
    assert_eq!(account.name, "Work mail");
    assert_eq!(account.display_name, "My sender name");
    assert_eq!(account.signature, "Work");
    assert_eq!(account.reply_signature(), "Work");
    assert_eq!(account.color, Some(Rgb(12, 34, 56)));
    assert_eq!(account.goa, before.goa);
    assert_eq!(account.oauth, before.oauth);
    assert_eq!(account.username, before.username);
    assert_eq!(account.incoming, before.incoming);
    assert_eq!(account.outgoing, before.outgoing);
}

#[test]
fn linked_account_signature_pickers_work_offline_and_only_apply_on_save() {
    let mut app = app(false);
    let added =
        app.execute("account.add", json!({"email":"owner@example.org", "kind":"microsoft","goa":{"id":"one","provider":"ms_graph"}})).unwrap();
    let id = added["id"].as_u64().unwrap();
    app.session.mailbox.signatures.push(mailcraft_model::Signature::plain_text("Work", "Example"));
    let mut h = open(app, json!({"id": id}));

    let choose = |h: &mut Harness<'static, MailApp>, field: &str, choice: &str| {
        h.get_by_role_and_label(Role::ComboBox, field).click();
        h.run_steps(5);
        h.get_by_label(choice).click();
        h.run_steps(5);
    };
    choose(&mut h, "New messages", "Work");
    choose(&mut h, "Replies and forwards", "Work");
    assert_eq!(h.state().session.mailbox.account(id).unwrap().signature, "");
    click(&mut h, "Cancel");
    assert!(wizard(&h).is_none());
    assert_eq!(h.state().session.mailbox.account(id).unwrap().reply_signature(), "");

    h.state_mut().execute("ui.dialog", json!({"name":"account", "id": id})).unwrap();
    h.run_steps(6);
    choose(&mut h, "New messages", "Work");
    choose(&mut h, "Replies and forwards", "Work");
    choose(&mut h, "Replies and forwards", "(none)");
    click(&mut h, "Save");
    assert!(wizard(&h).is_none());
    let account = h.state().session.mailbox.account(id).unwrap();
    assert_eq!(account.signature, "Work");
    assert_eq!(account.reply_signature(), "");
}
