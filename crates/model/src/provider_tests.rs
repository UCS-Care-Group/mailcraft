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
