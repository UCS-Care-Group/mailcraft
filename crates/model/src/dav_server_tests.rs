// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A DAV server's addresses: both services on one listener, https only, and no addresses in
//! debug output.

use super::*;

#[test]
fn a_dav_server_has_both_services_on_one_listener() {
    let dav = DavServer::for_host("Mail.Example.org.").unwrap();
    assert_eq!(dav.caldav, "https://mail.example.org/caldav/");
    assert_eq!(dav.carddav, "https://mail.example.org/carddav/");
    assert_eq!(dav.problem(), None);
    assert_eq!(DavServer::host(&dav.caldav), Some("mail.example.org"));
    assert!(DavServer::for_host("not a host").is_none());
}

#[test]
fn only_an_https_service_root_is_used() {
    for good in [
        "https://mail.example.org/caldav/",
        "HTTPS://Mail.Example.org:8443/caldav/",
        "https://192.0.2.7:8080/caldav/",
        "https://[2001:db8::1]/caldav/",
        "http://localhost:8080/caldav/",
    ] {
        assert_eq!(DavServer::check_url(good), Ok(()), "{good}");
    }
    for (bad, why) in [
        ("", "is empty"),
        ("mail.example.org/caldav/", "isn't an https:// address"),
        ("ftp://mail.example.org/caldav/", "isn't an https:// address"),
        ("http://mail.example.org/caldav/", "is a plain http:// address to a server that isn't on this computer: use https://"),
        ("https://ada@mail.example.org/caldav/", "carries a user name or a password: the secret is given apart from the address"),
        ("https://mail.example.org/cald av/", "has a control character or a space in it"),
        ("https://mail.example.org:0/caldav/", "has a port that isn't a number from 1 to 65535"),
    ] {
        assert_eq!(DavServer::check_url(bad), Err(why), "{bad:?}");
    }
}

#[test]
fn the_settings_keep_their_addresses_out_of_debug_output() {
    let dav = DavServer::for_host("mail.example.org").unwrap();
    let json = serde_json::to_string(&dav).unwrap();
    assert!(json.contains("\"caldav\":\"https://mail.example.org/caldav/\""));
    assert!(!json.contains("username"), "the account's own name is used unless one is given");
    assert_eq!(serde_json::from_str::<DavServer>(&json).unwrap(), dav);
    let shown = format!("{dav:?}");
    assert!(!shown.contains("example.org") && shown.contains("caldav: true"), "{shown}");
}

#[test]
fn the_calendars_may_know_the_owner_by_another_name() {
    let dav = DavServer { username: "ada-cloud".into(), ..DavServer::for_host("cloud.example.org").unwrap() };
    assert_eq!(dav.problem(), None);
    let json = serde_json::to_string(&dav).unwrap();
    assert!(json.contains("\"username\":\"ada-cloud\""), "{json}");
    assert_eq!(serde_json::from_str::<DavServer>(&json).unwrap(), dav);
    assert!(format!("{dav:?}").contains("username: true"), "whether a name is given, never the name");

    for (bad, why) in [("\u{0}", "has a control character in it"), ("ada\ncloud", "has a control character in it")] {
        assert_eq!(DavServer::check_username(bad), Err(why), "{bad:?}");
    }
    let long = "a".repeat(DavServer::MAX_USERNAME + 1);
    assert_eq!(DavServer::check_username(&long), Err("is longer than 254 characters"));
    assert_eq!(DavServer::check_username(""), Ok(()), "empty means the account's own login");
    let mut broken = dav;
    broken.username = "ada\ncloud".into();
    assert_eq!(broken.problem(), Some("has a control character in it"));
}
