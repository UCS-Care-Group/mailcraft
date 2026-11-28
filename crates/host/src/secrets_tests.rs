// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::{Account, AccountKind, Server};

use crate::{SecretError, legacy_secret_key, microsoft_secret_key, secret_key};

#[test]
fn errors_say_nothing_of_the_secret() {
    let e = SecretError::Other("could not keep it");
    assert_eq!(e.to_string(), "could not keep it");
    assert_eq!(format!("{e:?}"), "Other(..)");
    assert_eq!(format!("{:?}", SecretError::Refused), "Refused");
    assert!(SecretError::TooLong.to_string().contains("too long"));
}

fn zimbra_account() -> Account {
    Account {
        kind: AccountKind::Zimbra,
        email: "ada@example.org".into(),
        zimbra: Some(mailcraft_model::ZimbraServer { url: "https://mail.example.org".into(), username: "ada".into(), ..Default::default() }),
        ..Default::default()
    }
}

#[test]
fn an_old_ambiguous_zimbra_entry_is_not_read_by_a_pinned_account() {
    let mut account = zimbra_account();
    let pin = "ab".repeat(32);
    account.zimbra.as_mut().unwrap().trusted = pin.clone();
    let old_unpinned_user = format!("ada|pin={pin}");
    let old_key = format!("password:ada@example.org|zimbra=https://mail.example.org|user={old_unpinned_user}");
    let entries = std::collections::BTreeMap::from([(old_key, "synthetic-value")]);
    assert_eq!(entries.get(&secret_key("password", &account)), None);
    assert_eq!(entries.len(), 1);
}

#[test]
fn native_server_accounts_never_adopt_unbound_legacy_entries() {
    let mut account = zimbra_account();
    for kind in [AccountKind::Zimbra, AccountKind::Exchange, AccountKind::Jmap, AccountKind::Microsoft] {
        account.kind = kind;
        for secret in ["password", "refresh"] {
            let expected = match kind {
                AccountKind::Zimbra | AccountKind::Exchange => None,
                _ => Some(legacy_secret_key(secret, &account.email)),
            };
            assert_eq!(crate::secrets::legacy_secret_key_for(secret, &account), expected);
        }
    }
}

#[test]
fn versioned_zimbra_keys_remain_bound_to_server_login_and_pin() {
    let account = zimbra_account();
    let key = secret_key("password", &account);
    assert!(key.starts_with("password:ada@example.org|zimbra-v2="));
    let mut keys = std::collections::BTreeSet::from([key.clone()]);
    for (url, user, pin) in [
        ("https://other.example.org", "ada", String::new()),
        ("https://mail.example.org", "grace", String::new()),
        ("https://mail.example.org", "ada", "ab".repeat(32)),
        ("https://mail.example.org", "ada|pin=ab", String::new()),
        ("https://mail.example.org", "ada%7Cpin=ab", String::new()),
    ] {
        let mut changed = account.clone();
        let server = changed.zimbra.as_mut().unwrap();
        server.url = url.into();
        server.username = user.into();
        server.trusted = pin;
        assert!(keys.insert(secret_key("password", &changed)));
    }
    assert_eq!(secret_key("password", &account), key);
}

#[test]
fn a_microsoft_account_is_named_by_its_address_alone() {
    let account = Account { kind: AccountKind::Microsoft, email: " Ada@Lumen.Example ".to_string(), ..Default::default() };
    assert_eq!(secret_key("refresh", &account), "refresh:ada@lumen.example|graph");
    assert_eq!(secret_key("refresh", &account), microsoft_secret_key("refresh", "ada@lumen.example"));
}

#[test]
fn an_account_that_names_servers_is_named_by_them() {
    let server = |host: &str| Some(Server { host: host.to_string(), port: 993, ..Default::default() });
    let one = Account { email: "ada@lumen.example".to_string(), incoming: server("Imap.Lumen.Example"), ..Default::default() };
    let other = Account { incoming: server("imap.other.example"), ..one.clone() };
    assert!(secret_key("password", &one).contains("imap=imap.lumen.example:993"));
    assert_ne!(secret_key("password", &one), secret_key("password", &other));
    assert_eq!(legacy_secret_key("password", " Ada@Lumen.Example"), "password:ada@lumen.example");
}

fn dav_account() -> Account {
    use mailcraft_model::{DavCollectionState, DavServer};
    Account {
        email: " Ada@Lumen.Example ".to_string(),
        dav: Some(DavServer {
            calendar_homes: vec!["https://dav.lumen.example/cal/".into()],
            contact_homes: vec!["https://cards.lumen.example:8443/c/".into()],
            collections: vec![DavCollectionState { href: "https://p1.lumen.example/x/".into(), ..Default::default() }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn a_dav_password_is_named_by_its_origin_user_and_pin() {
    use crate::{dav_secret_key, dav_secret_keys};
    let a = dav_account();
    let key = dav_secret_key(&a, "https://dav.lumen.example/anything").unwrap();
    assert_eq!(key, "dav-password:ada@lumen.example|dav=https://dav.lumen.example|user=ada@lumen.example|pin=");
    // One key per origin the settings name, never the account's mail password's name.
    let keys = dav_secret_keys(&a);
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|k| k.starts_with("dav-password:") && k != &secret_key("password", &a)));
    assert!(keys.iter().collect::<std::collections::HashSet<_>>().len() == 3);
    // The same origin by another spelling is the same key; another origin, user or pin is not.
    assert_eq!(dav_secret_key(&a, "HTTPS://Dav.Lumen.Example:443/x"), Some(key.clone()));
    assert_ne!(dav_secret_key(&a, "https://dav.lumen.example:444/"), Some(key.clone()));
    let named = |f: &dyn Fn(&mut mailcraft_model::DavServer)| {
        let mut b = a.clone();
        f(b.dav.as_mut().unwrap());
        dav_secret_key(&b, "https://dav.lumen.example/")
    };
    assert_ne!(named(&|d| d.username = "grace".into()), Some(key.clone()));
    assert_ne!(named(&|d| d.trusted = "AB".repeat(32)), Some(key.clone()));
    assert_eq!(named(&|d| d.trusted = "AB".repeat(32)), named(&|d| d.trusted = "ab".repeat(32)));
    // No settings, or an origin that is none, names nothing.
    assert_eq!(dav_secret_key(&Account::default(), "https://dav.lumen.example/"), None);
    assert_eq!(dav_secret_key(&a, "https://user:pw@dav.lumen.example/"), None);
    assert!(dav_secret_keys(&Account::default()).is_empty());
}

#[test]
fn a_dav_user_name_keeps_its_case_in_the_key_and_the_host_and_origin_do_not() {
    use crate::dav_secret_key;
    let a = dav_account();
    let with = |name: &str| {
        let mut b = a.clone();
        b.dav.as_mut().unwrap().username = name.into();
        dav_secret_key(&b, "https://DAV.lumen.example/").unwrap()
    };
    assert_ne!(with("Ada"), with("ada"));
    assert!(with("Ada").contains("|user=Ada|"));
    assert_eq!(dav_secret_key(&a, "https://dav.lumen.example/"), dav_secret_key(&a, "https://DAV.Lumen.Example/"));
    // No key for a host nobody's password goes to.
    let mut g = a.clone();
    g.dav.as_mut().unwrap().calendar_homes = vec!["https://www.googleapis.com/x/".into()];
    assert_eq!(dav_secret_key(&g, "https://www.googleapis.com/x/"), None);
    assert_eq!(dav_secret_key(&g, "https://outlook.office365.com/"), None);
    assert!(crate::dav_secret_keys(&g).iter().all(|k| !k.contains("googleapis")));
}
