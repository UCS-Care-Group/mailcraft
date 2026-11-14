// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn imap_account(a: &mut MailApp) -> Id {
    let account = json!({"email": "noor@lumen.example", "displayName": "Noor", "username": "noor",
        "incoming": {"host": "imap.lumen.example", "port": 993}, "outgoing": {"host": "smtp.lumen.example", "port": 587, "security": "starttls"}});
    a.execute("account.add", account).unwrap()["id"].as_u64().unwrap()
}

/// Account Settings names the account in the folder pane: the field is empty while it goes by its
/// address, a name typed there is saved as `name`, and clearing it gives the address back.
#[test]
fn account_settings_name_the_account() {
    let mut a = app(true);
    let id = imap_account(&mut a);
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    assert_eq!(w.account_name, "");
    assert_eq!(w.edit_command(), None);

    w.account_name = "  Work ".into();
    assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": id, "name": "Work"}))));
    assert!(w.act(&mut a, Action::Go));
    let account = a.session.mailbox.account(id).unwrap();
    assert_eq!((account.name.as_str(), account.email.as_str(), account.display_name.as_str()), ("Work", "noor@lumen.example", "Noor"));
    // The folder pane heads the account with it.
    assert_eq!(crate::nav::heading_name(&a, account), "Work");

    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    assert_eq!(w.account_name, "Work");
    assert_eq!(w.edit_command(), None);
    w.account_name.clear();
    assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": id, "name": ""}))));
    assert!(w.act(&mut a, Action::Go));
    assert_eq!(a.session.mailbox.account(id).unwrap().name, "noor@lumen.example");
    assert_eq!(AccountSetup::open(&a, &json!({"id": id})).unwrap().account_name, "");
}

/// The field is on the page, with the address as its hint, in both themes.
#[test]
fn the_account_name_field_is_on_the_settings_page() {
    for theme in ["light", "dark"] {
        let mut a = app(true);
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        let id = imap_account(&mut a);
        let mut h = open(a, json!({"id": id}));
        h.run_steps(2);
        assert!(says(&h, "Account name"));
        assert!(says(&h, "Shown in the folder pane, such as Work or Home. Leave it empty to show the address."));
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("account-name-{theme}.png"))).unwrap();
        }
    }
}

/// A shared mailbox opened under a Microsoft account (#70).
fn shared_mailbox(a: &mut MailApp) -> (Id, Id) {
    let owner = a.execute("account.add", json!({"email": "ada@lumen.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = a.execute("account.addSharedMailbox", json!({"account": owner, "address": "team@lumen.example"})).unwrap()["id"].as_u64().unwrap();
    (owner, shared)
}

/// A shared mailbox can be named too (#1385): its settings are its name and colour, nothing of its
/// owner's sign-in, and the folder pane heads it with the name. Clearing it gives the address back.
#[test]
fn a_shared_mailbox_can_be_named() {
    let mut a = app(true);
    let (owner, shared) = shared_mailbox(&mut a);
    let mut w = AccountSetup::open(&a, &json!({"id": shared})).unwrap();
    assert_eq!((w.page, w.account_name.as_str()), (Page::Settings, ""));
    assert_eq!(w.edit_command(), None);
    w.account_name = " Team ".into();
    assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": shared, "name": "Team"}))));
    assert!(w.act(&mut a, Action::Go));
    let account = a.session.mailbox.account(shared).unwrap();
    assert_eq!((account.name.as_str(), account.email.as_str(), account.shared_of), ("Team", "team@lumen.example", Some(owner)));
    assert_eq!(account.kind, AccountKind::Microsoft);
    assert_eq!(crate::nav::heading_name(&a, account), "Team");
    // The owner's account is as it was.
    assert_eq!(a.session.mailbox.account(owner).unwrap().name, "ada@lumen.example");

    let mut w = AccountSetup::open(&a, &json!({"id": shared})).unwrap();
    assert_eq!(w.account_name, "Team");
    w.account_name.clear();
    assert!(w.act(&mut a, Action::Go));
    assert_eq!(a.session.mailbox.account(shared).unwrap().name, "team@lumen.example");
}

/// The shared mailbox's page has the name and the colour, says whose sign-in reads it, and
/// shows nothing of a sign-in, servers or mail to keep offline; Save names it, in both themes.
#[test]
fn the_shared_mailbox_page_has_the_name_and_nothing_of_a_sign_in() {
    for theme in ["light", "dark"] {
        let mut a = app(true);
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        let (_, shared) = shared_mailbox(&mut a);
        let mut h = open(a, json!({"id": shared}));
        h.run_steps(2);
        assert_eq!(page(&h), Some(Page::Settings));
        assert!(says(&h, "A shared mailbox, read with the sign-in of ada@lumen.example"));
        assert!(says(&h, "Account name") && says(&h, "Kept on this computer only") && says(&h, "Colour"));
        assert!(!says(&h, "Your name") && !says(&h, "Application (client) ID") && !says(&h, "Mail to keep offline"));
        assert!(!says(&h, "User name") && !says(&h, "Ask before deleting a conversation"));
        assert!(h.query_by_role(Role::PasswordInput).is_none());
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("shared-mailbox-name-{theme}.png"))).unwrap();
        }
        wizard_mut(&mut h).account_name = "Support".into();
        click(&mut h, "Save");
        assert_eq!(page(&h), None);
        assert_eq!(h.state().session.mailbox.account(shared).unwrap().name, "Support");
    }
}
