// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Downloading the organisation's Offline Address Book (#1393) from the strict stand-in.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::DirectoryKind;

use crate::http::HttpResponse;
use crate::oab::{
    FullFile, MAX_FILE, MAX_KEPT_BYTES, MAX_UNPACKED, MAX_VALUES, OabError, address_list, address_list_within, fetch, file_failure, full_file,
    judged, oab_url, origin,
};
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
    let list = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), None)).unwrap().unwrap().unwrap();
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

/// A file that says it unpacks to more than a full file of the address book can need is refused
/// from its header, before anything is unpacked or held.
#[test]
fn a_file_that_unpacks_past_the_cap_is_refused_from_its_header() {
    let mut file = Vec::new();
    for word in [3u32, 1, 0x40000, MAX_UNPACKED as u32 + 1] {
        file.extend_from_slice(&word.to_le_bytes());
    }
    assert_eq!(address_list(&file).err(), Some(mailcraft_oab::Error::Cap(mailcraft_oab::Cap::OutputSize)));
}

#[test]
fn once_nobody_waits_for_the_list_nothing_more_is_asked() {
    let standin = organisation();
    let result = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(true), None)).unwrap();
    assert!(matches!(result, Err(OabError::Cancelled)), "{result:?}");
    assert_eq!(operations(&standin), ["Autodiscover"], "the request on its way finishes, and nothing after it is asked");
}

#[test]
fn an_answer_longer_than_its_cap_is_refused_whatever_the_transport_read() {
    let answer = HttpResponse::new(200, vec![b' '; 11]);
    assert!(matches!(judged(&answer, 10), Err(Error::TooLarge)));
    assert_eq!(judged(&answer, 11).unwrap().len(), 11);
}

/// A file past the cap that the table of contents gave no size for is found out by the transport
/// or the check of the answer: it is as much `TooLarge` as one whose size the table named, which
/// is held to the day's schedule, and any other failure of the request is left as it is.
#[test]
fn a_file_found_too_large_after_the_request_is_too_large() {
    assert!(matches!(file_failure(Error::TooLarge), OabError::TooLarge));
    assert!(matches!(file_failure(Error::Unauthorized), OabError::Ews(Error::Unauthorized)));
    let answer = HttpResponse::new(200, vec![b' '; MAX_FILE + 1]);
    assert!(matches!(judged(&answer, MAX_FILE).map_err(file_failure), Err(OabError::TooLarge)));
}

#[test]
fn an_address_book_published_elsewhere_is_not_asked_for() {
    let standin = organisation();
    standin.state().oab.elsewhere = Some("https://elsewhere.example".into());
    assert!(matches!(
        block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), None)).unwrap(),
        Err(OabError::NotPublished)
    ));
    assert_eq!(operations(&standin), ["Autodiscover"]);
}

#[test]
fn a_server_that_publishes_none_says_so() {
    let standin = organisation();
    standin.state().oab.published = false;
    assert!(matches!(
        block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), None)).unwrap(),
        Err(OabError::NotPublished)
    ));
}

#[test]
fn a_password_turned_down_is_said() {
    let standin = organisation();
    let result = block_on(fetch(&standin, URL, ME, &Secret::new("Basic d3Jvbmc6d3Jvbmc="), &AtomicBool::new(false), None)).unwrap();
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

/// The signature part of a signed message of the S/MIME crate's tests: a CMS SignedData that
/// carries the signer's certificate, as a directory's `userSMIMECertificate` does.
fn signed_data() -> Vec<u8> {
    let message = String::from_utf8_lossy(include_bytes!("../../smime/testdata/detached.eml")).into_owned();
    let part = message.split("filename=\"smime.p7s\"").nth(1).unwrap();
    let text: String = part.lines().skip_while(|l| !l.trim().is_empty()).skip(1).take_while(|l| !l.starts_with("--")).collect();
    mailcraft_charset::base64::STANDARD.decode(text.trim()).unwrap()
}

/// A person's own certificates come with the list: a certificate of the directory's, and those
/// inside the SignedData of `userSMIMECertificate`, each once; an authority's, and what can't be
/// read, are left out, and whoever has none has none.
#[test]
fn certificates_come_with_whoever_has_them() {
    let alice = include_bytes!("../../smime/testdata/alice.sign.der").to_vec();
    let authority = include_bytes!("../../smime/testdata/ca.rsa.der").to_vec();
    let standin = organisation();
    standin.state().directory[0].certificates =
        (vec![signed_data(), b"not one".to_vec()], vec![alice.clone(), authority, vec![0x30, 0x03, 0x02, 0x01, 0x01]]);
    let list = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), None)).unwrap().unwrap().unwrap();
    let ada = list.people.iter().find(|p| p.email == "ada.lovelace@example.org").unwrap();
    let held: Vec<Vec<u8>> = ada.certificates.iter().map(|c| c.to_vec()).collect();
    assert_eq!(held, [alice]);
    assert!(list.people.iter().filter(|p| p.email != ada.email).all(|p| p.certificates.is_empty()));
    // A log line says how many, never what.
    assert!(format!("{ada:?}").contains("certificates: 1"));
}

/// A list longer than the people kept, or whose values pass the cap on what is kept, is cut there
/// and says it goes on: the people before are shown, and the file is not refused for it.
#[test]
fn a_list_past_its_caps_is_cut_and_says_so() {
    let file = organisation().state().oab_file();
    let whole = address_list(&file).unwrap();
    assert_eq!(whole.people.len(), 4);
    assert!(!whole.more);
    // The first two entries of the file: Ada Lovelace and All Staff.
    let two = address_list_within(&file, 2, MAX_VALUES, MAX_KEPT_BYTES).unwrap();
    assert!(two.more);
    let names: Vec<&str> = two.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "All Staff"]);
    // So few values that only Ada's fit: the header's four and her eleven (two proxy addresses),
    // where All Staff's five more would make twenty.
    let one = address_list_within(&file, 100, 16, MAX_KEPT_BYTES).unwrap();
    assert!(one.more);
    assert_eq!(one.people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Ada Lovelace"]);
    // A header past the cap still refuses the file: it is not an entry to stop before.
    assert_eq!(address_list_within(&file, 100, MAX_VALUES, 1).err(), Some(mailcraft_oab::Error::Cap(mailcraft_oab::Cap::KeptBytes)));
}

/// The generation held already is not downloaded again: the table of contents is read, and the
/// file only when the server publishes another.
#[test]
fn the_generation_held_is_not_downloaded_again() {
    let standin = organisation();
    let same = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), Some(7))).unwrap().unwrap();
    assert!(same.is_none());
    assert_eq!(operations(&standin), ["Autodiscover", "oab.xml"]);
    let other = block_on(fetch(&standin, URL, ME, &Secret::new(AUTHORIZATION), &AtomicBool::new(false), Some(6))).unwrap().unwrap();
    assert_eq!(other.map(|list| list.sequence), Some(7));
    assert_eq!(operations(&standin), ["Autodiscover", "oab.xml", "Autodiscover", "oab.xml", "OAB file"]);
}
