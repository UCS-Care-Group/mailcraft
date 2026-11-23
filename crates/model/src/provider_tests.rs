// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose the Autodiscover a domain's DNS names is.

use proptest::prelude::*;

use super::*;

fn of(hosts: &[&str]) -> Autodiscover {
    by_autodiscover(&hosts.iter().map(|host| host.to_string()).collect::<Vec<_>>())
}

#[test]
fn a_hoster_s_or_the_organisation_s_own_autodiscover_is_an_exchange_server() {
    assert_eq!(of(&["autodiscover.de2.hoster.example"]), Autodiscover::Exchange);
    assert_eq!(of(&["Autodiscover.Example.ORG."]), Autodiscover::Exchange);
    assert_eq!(of(&["cpanelemaildiscovery.cpanel.net", "mail.example.org"]), Autodiscover::Exchange);
}

#[test]
fn microsoft_s_is_microsoft_365_whatever_else_is_named() {
    assert_eq!(of(&["autodiscover.outlook.com"]), Autodiscover::Microsoft);
    assert_eq!(of(&["autodiscover.example.org", "autodiscover.outlook.com."]), Autodiscover::Microsoft);
    // A look-alike is somebody else's.
    assert_eq!(of(&["autodiscover.evil-outlook.com"]), Autodiscover::Exchange);
}

#[test]
fn imap_autodiscover_and_nothing_are_nothing() {
    assert_eq!(of(&[]), Autodiscover::Nothing);
    assert_eq!(of(&["cpanelemaildiscovery.cpanel.net"]), Autodiscover::Nothing);
    assert_eq!(of(&["", " ", "localhost", "not a host", "user@example.org"]), Autodiscover::Nothing);
    // `cpanel.net` itself only, not a name that ends the same way.
    assert_eq!(of(&["autodiscover.notcpanel.net"]), Autodiscover::Exchange);
}

proptest! {
    #[test]
    fn any_hosts_never_panic(hosts in proptest::collection::vec(".{0,80}", 0..40)) {
        let _ = by_autodiscover(&hosts);
    }
}

#[test]
fn icloud_and_fastmail_have_dav_hosts_and_nobody_else_does() {
    let icloud = dav_by_domain("ICloud.com").unwrap();
    assert_eq!((icloud.caldav, icloud.carddav), ("https://caldav.icloud.com/", "https://contacts.icloud.com/"));
    assert_eq!(dav_by_domain("me.com"), Some(icloud));
    assert_eq!(dav_by_imap_host("imap.mail.me.com"), Some(icloud));
    let fastmail = dav_by_domain("fastmail.com").unwrap();
    assert_eq!((fastmail.caldav, fastmail.carddav), ("https://caldav.fastmail.com/", "https://carddav.fastmail.com/"));
    assert_eq!(dav_by_imap_host("imap.fastmail.com."), Some(fastmail));
    for other in ["gmail.com", "example.org", "evilicloud.com", "", "icloud.com.example.org"] {
        assert_eq!(dav_by_domain(other), None, "{other}");
    }
    assert_eq!(dav_by_imap_host("imap.example.org"), None);
}

#[test]
fn a_provider_s_password_goes_to_its_own_dav_hosts_only() {
    let icloud = dav_by_domain("icloud.com").unwrap();
    for host in ["caldav.icloud.com", "contacts.icloud.com", "p07-caldav.icloud.com", "P123-contacts.icloud.com", "p7-caldav.icloud.com."] {
        assert!(icloud.trusts(host), "{host}");
    }
    for host in [
        "icloud.com",
        "www.icloud.com",
        "p-caldav.icloud.com",
        "pxx-caldav.icloud.com",
        "p07-caldav.icloud.com.evil.example",
        "caldav.fastmail.com",
        "p07-carddav.icloud.com",
        "evil-p07-caldav.icloud.com",
    ] {
        assert!(!icloud.trusts(host), "{host}");
    }
    let fastmail = dav_by_domain("fastmail.com").unwrap();
    assert!(fastmail.trusts("caldav.fastmail.com") && fastmail.trusts("carddav.fastmail.com"));
    assert!(!fastmail.trusts("p07-caldav.fastmail.com") && !fastmail.trusts("caldav.icloud.com") && !fastmail.trusts("imap.fastmail.com"));
}
