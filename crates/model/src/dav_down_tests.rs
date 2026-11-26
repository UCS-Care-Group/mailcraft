// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::DavServer;

fn server(separate: bool) -> DavServer {
    DavServer {
        calendar_homes: vec!["https://dav.example.org/cal/ada/".into()],
        contact_homes: vec!["https://cards.example.org:8443/c/".into()],
        separate_password: separate,
        // (A collection a server's answer named, on another origin: no home of the settings.)
        collections: vec![crate::DavCollectionState { href: "https://other.example.net/cal/x/".into(), ..Default::default() }],
        ..Default::default()
    }
}

#[test]
fn the_mail_password_goes_only_to_an_origin_the_settings_name_and_only_when_it_is_the_servers_too() {
    let s = server(false);
    assert!(s.may_use_mail_password("https://dav.example.org"));
    assert!(s.may_use_mail_password("https://cards.example.org:8443"));
    assert!(!s.may_use_mail_password("https://cards.example.org"), "another port is another origin");
    assert!(!s.may_use_mail_password("https://other.example.net"), "a collection's origin is a server's word, not a setting");
    assert!(!s.may_use_mail_password("https://evil.example.org"));
    assert!(!s.may_use_mail_password(""));
    // The owner gave these servers a password of their own: the mail one never goes.
    let own = server(true);
    assert!(!own.may_use_mail_password("https://dav.example.org"));
}

#[test]
fn a_hostile_home_names_no_origin_for_the_mail_password() {
    let mut s = DavServer { calendar_homes: vec!["https://outlook.office365.com/".into(), "http://dav.example.org/".into()], ..Default::default() };
    assert!(s.home_origins().is_empty());
    s.calendar_homes.push("https://dav.example.org/cal/".into());
    assert_eq!(s.home_origins(), vec!["https://dav.example.org".to_string()]);
}

#[test]
fn debug_shows_counts_never_addresses_or_documents() {
    let download = DavDownload {
        collections: vec![DavRead {
            href: "https://dav.example.org/secret-calendar/".into(),
            changes: vec![DavChange {
                href: "https://dav.example.org/secret-calendar/a.ics".into(),
                raw: Some("BEGIN:VCALENDAR secret".into()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let shown = format!("{download:?} {:?} {:?}", download.collections[0], download.collections[0].changes[0]);
    assert!(!shown.contains("secret") && !shown.contains("dav.example.org"), "{shown}");
    let ask = DavAsk { homes: vec!["https://dav.example.org/cal/".into()], username: "ada".into(), ..Default::default() };
    assert!(!format!("{ask:?}").contains("ada") && !format!("{ask:?}").contains("dav.example"));
}

#[test]
fn a_download_reads_back_from_json_and_junk_reads_as_nothing() {
    let download = DavDownload { account: 3, signed_in: vec!["https://dav.example.org".into()], ..Default::default() };
    let back: DavDownload = serde_json::from_value(serde_json::to_value(&download).unwrap()).unwrap();
    assert_eq!(back, download);
    let junk: DavDownload =
        serde_json::from_str(r#"{"account": 3, "collections": [{"href": "x", "state": "read"}], "problem": "wrongPassword"}"#).unwrap();
    assert_eq!(junk.problem, DavProblem::WrongPassword);
    assert!(serde_json::from_str::<DavDownload>(r#"{"problem": "nonsense"}"#).is_err());
}
