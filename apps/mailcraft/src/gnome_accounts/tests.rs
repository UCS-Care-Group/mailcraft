// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic ObjectManager replies: no session accounts or provider credentials.

use super::*;
use zbus::zvariant::{OwnedObjectPath, Str};

fn string(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

fn fixture(provider: &str, email: &str) -> zbus::fdo::ManagedObjects {
    let account = HashMap::from([
        ("ProviderType".into(), string(provider)),
        ("ProviderName".into(), string("Example provider")),
        ("MailDisabled".into(), OwnedValue::from(false)),
    ]);
    let mail = HashMap::from([("EmailAddress".into(), string(email)), ("Name".into(), string("Example Owner"))]);
    HashMap::from([(
        OwnedObjectPath::try_from("/org/gnome/OnlineAccounts/Accounts/account_1").unwrap(),
        HashMap::from([(ACCOUNT.try_into().unwrap(), account), (MAIL.try_into().unwrap(), mail)]),
    )])
}

#[test]
fn public_metadata_maps_supported_providers_and_keeps_unknown_ones_visible() {
    for (provider, expected) in [
        ("google", Provider::Google),
        ("ms_graph", Provider::Microsoft365),
        ("windows_live", Provider::OutlookCom),
        ("exchange", Provider::Unsupported),
        ("imap_smtp", Provider::Unsupported),
        ("unknown", Provider::Unsupported),
    ] {
        let accounts = parse(fixture(provider, "owner@example.org")).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].provider, expected);
        assert_eq!(accounts[0].email, "owner@example.org");
        assert_eq!(accounts[0].name, "Example Owner");
    }
}

#[test]
fn disabled_mail_is_respected_even_while_the_mail_interface_is_still_present() {
    let mut objects = fixture("google", "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(ACCOUNT).unwrap().insert("MailDisabled".into(), OwnedValue::from(true));
    }
    assert!(parse(objects).unwrap().is_empty());
}

#[test]
fn calendar_only_accounts_and_malformed_properties_are_not_imported() {
    for key in [ACCOUNT, MAIL] {
        let mut objects = fixture("google", "owner@example.org");
        for interfaces in objects.values_mut() {
            interfaces.remove(key);
        }
        assert!(parse(objects).unwrap().is_empty());
    }
    let mut objects = fixture("google", "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(MAIL).unwrap().insert("EmailAddress".into(), OwnedValue::from(42u32));
    }
    assert!(parse(objects).unwrap().is_empty());
}

#[test]
fn hostile_or_missing_addresses_never_reach_setup() {
    for email in [
        "",
        "not-an-address",
        "@example.org",
        "owner@@example.org",
        "owner@example.org\n",
        "own\u{202e}er@example.org",
        "a b@example.org",
        &format!("{}@example.org", "x".repeat(255)),
    ] {
        assert!(parse(fixture("google", email)).unwrap().is_empty(), "{email:?}");
    }
}

#[test]
fn accounts_are_sorted_deduplicated_and_bounded() {
    let mut objects = fixture("google", "owner@example.org");
    let interfaces = fixture("google", "OWNER@example.org").into_values().next().unwrap();
    objects.insert(OwnedObjectPath::try_from("/org/gnome/OnlineAccounts/Accounts/account_2").unwrap(), interfaces);
    assert_eq!(parse(objects).unwrap().len(), 1);
    let mut objects = zbus::fdo::ManagedObjects::new();
    for i in 0..257 {
        objects.insert(OwnedObjectPath::try_from(format!("/org/gnome/OnlineAccounts/Accounts/account_{i}")).unwrap(), HashMap::new());
    }
    assert_eq!(parse(objects).unwrap_err(), DiscoveryError::TooManyAccounts);
}

struct Fixture;

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Fixture {
    fn get_managed_objects(&self) -> zbus::fdo::ManagedObjects {
        fixture("google", "owner@example.org")
    }
}

/// Run with dbus-run-session and MAILCRAFT_TEST_PRIVATE_BUS=1; never touches a user's session.
#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_discovery_reads_only_public_metadata() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _server = zbus::blocking::connection::Builder::session().unwrap().name(SERVICE).unwrap().serve_at(PATH, Fixture).unwrap().build().unwrap();
    let accounts = discover().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].email, "owner@example.org");
}
