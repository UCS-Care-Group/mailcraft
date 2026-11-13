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
