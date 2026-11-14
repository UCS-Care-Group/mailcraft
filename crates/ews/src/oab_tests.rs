// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Downloading the organisation's Offline Address Book (#1393) from the strict stand-in.

use mailcraft_model::sync::DirectoryKind;

use crate::oab::{FullFile, OabError, fetch, full_file, oab_url, origin};
use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::standin_directory::Person;
use crate::standin_oab::{FILE, PATH};
use crate::{Error, Secret};

const URL: &str = "https://exchange.example/EWS/Exchange.asmx";
const ME: &str = "me@example.org";

fn organisation() -> StandIn {
    let standin = StandIn::new();
    standin.state().directory = vec![
        Person {
            company: "Example Ltd".into(),
            department: "Accounts".into(),
            title: "Controller".into(),
            phone: "+1 555 0100".into(),
            mobile: "+1 555 0199".into(),
            office: "Building 2, Room 14".into(),
            home: "+1 555 0123".into(),
            ..Person::new("Ada Lovelace", "ada.lovelace@example.org")
        },
        Person { kind: "PublicDL".into(), ..Person::new("All Staff", "staff@example.org") },
        Person { kind: "Contact".into(), company: "Partner Co".into(), ..Person::new("Grace Hopper", "grace@partner.example") },
        Person::new("bob", "bob@example.org"),
        // Nobody mail can go to.
        Person::new("Broken", "not an address"),
    ];
    standin
}

fn operations(standin: &StandIn) -> Vec<String> {
    standin.state().requests.iter().map(|asked| asked.operation.clone()).collect()
}

#[test]
fn the_whole_list_comes_from_the_accounts_own_server_in_three_reads() {
    let standin = organisation();
    let list = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION))).unwrap().unwrap();
    assert_eq!(list.sequence, 7);
    assert_eq!(list.name, "\\Global Address List");
    assert!(!list.more);
    let names: Vec<&str> = list.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "All Staff", "bob", "Grace Hopper"], "by name, without regard to case");
    let ada = &list.people[0];
    assert_eq!(
        (ada.email.as_str(), ada.company.as_str(), ada.department.as_str(), ada.title.as_str()),
        ("ada.lovelace@example.org", "Example Ltd", "Accounts", "Controller")
    );
    assert_eq!((ada.phone.as_str(), ada.mobile.as_str(), ada.office.as_str()), ("+1 555 0100", "+1 555 0199", "Building 2, Room 14"));
    assert_eq!(list.people[1].kind, DirectoryKind::List);
    assert_eq!(list.people[3].kind, DirectoryKind::Contact);
    assert_eq!(list.people[2].kind, DirectoryKind::Mailbox);
    // The home number is in the file and is not kept.
    assert!(list.people.iter().all(|p| !format!("{}{}", p.phone, p.mobile).contains("0123")));
    assert_eq!(operations(&standin), ["Autodiscover", "oab.xml", "OAB file"]);
    assert!(standin.state().writes.is_empty());
}

#[test]
fn an_address_book_published_elsewhere_is_not_asked_for() {
    let standin = organisation();
    standin.state().oab.elsewhere = Some("https://elsewhere.example".into());
    assert!(matches!(block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION))).unwrap(), Err(OabError::NotPublished)));
    assert_eq!(operations(&standin), ["Autodiscover"]);
}

#[test]
fn a_server_that_publishes_none_says_so() {
    let standin = organisation();
    standin.state().oab.published = false;
    assert!(matches!(block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION))).unwrap(), Err(OabError::NotPublished)));
}

#[test]
fn a_password_turned_down_is_said() {
    let standin = organisation();
    let result = block_on(fetch(&standin, URL, ME, &Secret::new("Basic d3Jvbmc6d3Jvbmc="))).unwrap();
    assert!(matches!(result, Err(OabError::Ews(Error::Unauthorized))), "{result:?}");
}

#[test]
fn the_address_is_taken_only_on_the_accounts_own_server() {
    let answer = |protocols: &str| {
        format!(
            "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">\
             <Response xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a\"><Account>{protocols}</Account></Response></Autodiscover>"
        )
    };
    let one = |url: &str| answer(&format!("<Protocol><Type>EXCH</Type><OABUrl>{url}</OABUrl></Protocol>"));
    assert_eq!(oab_url(one("https://EXCHANGE.example/OAB/x").as_bytes(), URL).unwrap().as_deref(), Some("https://EXCHANGE.example/OAB/x/"));
    assert_eq!(oab_url(one("https://exchange.example:444/OAB/x/").as_bytes(), URL).unwrap(), None);
    assert_eq!(oab_url(one("http://exchange.example/OAB/x/").as_bytes(), URL).unwrap(), None);
    assert_eq!(oab_url(one("https://exchange.example.evil.example/OAB/").as_bytes(), URL).unwrap(), None);
    assert_eq!(oab_url(one("https://user@exchange.example/OAB/").as_bytes(), URL).unwrap(), None);
    assert_eq!(oab_url(one("https://exchange.example/OAB/?x=1").as_bytes(), URL).unwrap(), None);
    // The first on this server, whichever protocol names it.
    let two = answer(
        "<Protocol><Type>EXCH</Type><OABUrl>https://internal.example/OAB/a/</OABUrl></Protocol>\
         <Protocol><Type>EXPR</Type><OABUrl>https://exchange.example/OAB/b/</OABUrl></Protocol>",
    );
    assert_eq!(oab_url(two.as_bytes(), URL).unwrap().as_deref(), Some("https://exchange.example/OAB/b/"));
    assert!(matches!(oab_url(b"<html/>", URL), Err(Error::Unexpected)));
    assert!(matches!(oab_url(b"not xml <", URL), Err(Error::Malformed)));
    assert_eq!(origin("https://Exchange.Example:444/EWS/x").as_deref(), Some("https://exchange.example:444"));
}

#[test]
fn the_full_file_of_version_4_is_taken_with_a_plain_name() {
    let toc = |fulls: &str| format!("<OAB><OAL id=\"1\" dn=\"/\" name=\"x\">{fulls}<Template ver=\"7\" seq=\"1\">t.lzx</Template></OAL></OAB>");
    let full = |ver: &str, name: &str| format!("<Full seq=\"9\" ver=\"{ver}\" size=\"100\" uncompressedsize=\"200\">{name}</Full>");
    let taken = full_file(toc(&format!("{}{}", full("3", "old.lzx"), full("32", "new.lzx"))).as_bytes()).unwrap();
    assert_eq!(taken, Some(FullFile { name: "new.lzx".into(), sequence: 9, size: 100 }));
    for name in ["../secret", "a/b.lzx", ".hidden", "a\\b", "x y", ""] {
        assert_eq!(full_file(toc(&full("32", name)).as_bytes()).unwrap(), None, "{name}");
    }
    assert!(matches!(full_file(b"<html/>"), Err(Error::Unexpected)));
}

#[test]
fn a_damaged_file_is_said_and_not_read() {
    let standin = organisation();
    let mut file = standin.state().oab_file();
    let last = file.len() - 1;
    file[last] ^= 1;
    assert!(matches!(crate::oab::address_list(&file), Err(mailcraft_oab::Error::BadChecksum(_))));
    // The paths the stand-in serves are the ones the tests name.
    assert!(PATH.ends_with('/') && FILE.ends_with(".lzx"));
}

/// A person's certificates come with the list, from both of the properties that hold them: each
/// once, only what has the shape of one, and none for whoever has none.
#[test]
fn certificates_come_with_whoever_has_them() {
    let standin = organisation();
    let cert = |n: u8| vec![0x30, 0x03, 0x02, 0x01, n];
    standin.state().directory[0].certificates = (vec![cert(1)], vec![cert(1), cert(2), b"not one".to_vec(), vec![0x30; 17 * 1024]]);
    let list = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION))).unwrap().unwrap();
    let ada = list.people.iter().find(|p| p.email == "ada.lovelace@example.org").unwrap();
    let held: Vec<Vec<u8>> = ada.certificates.iter().map(|c| c.to_vec()).collect();
    assert_eq!(held, [cert(1), cert(2)]);
    assert!(list.people.iter().filter(|p| p.email != ada.email).all(|p| p.certificates.is_empty()));
    // A log line says how many, never what.
    assert!(format!("{ada:?}").contains("certificates: 2"));
}
