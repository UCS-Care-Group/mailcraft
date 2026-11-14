// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Looking names up in the organisation's address list (#1262): the request, the reader and the
//! client against the strict stand-in.

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::standin_directory::Person;
use crate::*;

const URL: &str = "http://127.0.0.1:1/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn lookup(standin: &StandIn, text: &str) -> Result<Resolved, Error> {
    block_on(client().resolve_names(standin, &ResolveNames::new(text).unwrap())).unwrap()
}

/// The stand-in's organisation: two people called Ada, a list, a mail contact.
fn organisation() -> StandIn {
    let standin = StandIn::new();
    standin.state().directory = vec![
        Person {
            company: "Example Ltd".into(),
            department: "Accounts".into(),
            title: "Controller".into(),
            phone: "+1 555 0100".into(),
            ..Person::new("Ada Lovelace", "ada.lovelace@example.org")
        },
        Person { department: "Engines".into(), ..Person::new("Ada Byron", "ada.byron@example.org") },
        Person { kind: "PublicDL".into(), ..Person::new("All Staff", "staff@example.org") },
        Person { kind: "Contact".into(), company: "Partner Co".into(), ..Person::new("Grace Hopper", "grace@partner.example") },
    ];
    standin
}

/// Nothing that writes anything was asked, and every request was `ResolveNames` alone.
fn only_looked_up(standin: &StandIn) {
    let state = standin.state();
    assert!(state.writes.is_empty(), "{:?}", state.writes);
    assert!(state.requests.iter().all(|asked| asked.operation == "ResolveNames"), "{:?}", state.requests);
}

#[test]
fn one_person_found_is_read_with_what_tells_them_apart_and_nothing_more() {
    let standin = organisation();
    let found = lookup(&standin, "lovelace").unwrap();
    assert!(!found.more);
    let [ada] = found.candidates.as_slice() else { panic!("{found:?}") };
    assert_eq!(
        (ada.name.as_str(), ada.email.as_str(), ada.kind, ada.company.as_str(), ada.department.as_str(), ada.title.as_str()),
        ("Ada Lovelace", "ada.lovelace@example.org", RecipientKind::Mailbox, "Example Ltd", "Accounts", "Controller")
    );
    // The phone number the answer held is kept nowhere.
    assert!(![&ada.name, &ada.email, &ada.company, &ada.department, &ada.title].iter().any(|field| field.contains("555")));
    // Only the directory is asked, with the whole contact data, and the text as typed.
    let body = standin.state().requests[0].body.clone();
    assert!(body.contains("SearchScope=\"ActiveDirectory\"") && body.contains("ReturnFullContactData=\"true\""), "{body}");
    assert!(body.contains("<m:UnresolvedEntry>lovelace</m:UnresolvedEntry>"), "{body}");
    only_looked_up(&standin);
}

#[test]
fn several_found_is_a_warning_read_as_an_answer_and_nobody_found_is_an_empty_one() {
    let standin = organisation();
    let found = lookup(&standin, "ada").unwrap();
    let names: Vec<&str> = found.candidates.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Ada Lovelace", "Ada Byron"]);
    assert_eq!(found.candidates[1].department, "Engines");
    assert_eq!(found.candidates[1].company, "");
    assert_eq!(standin.state().requests[0].code, "ErrorNameResolutionMultipleResults");

    assert_eq!(lookup(&standin, "nobody").unwrap(), Resolved::default());
    assert_eq!(standin.state().requests[1].code, "ErrorNameResolutionNoResults");

    let kinds: Vec<RecipientKind> = ["staff", "grace"].iter().map(|text| lookup(&standin, text).unwrap().candidates[0].kind).collect();
    assert_eq!(kinds, [RecipientKind::List, RecipientKind::Contact]);
    only_looked_up(&standin);
}

#[test]
fn more_people_than_an_answer_holds_are_said_to_be_more() {
    let standin = StandIn::new();
    standin.state().directory = (0..150).map(|i| Person::new(&format!("Sam Number{i}"), &format!("sam{i}@example.org"))).collect();
    let found = lookup(&standin, "sam").unwrap();
    assert_eq!((found.candidates.len(), found.more), (100, true));
    only_looked_up(&standin);
}

#[test]
fn a_busy_server_is_said_to_be_busy() {
    let standin = organisation();
    standin.state().busy = (1, 2_500);
    assert_eq!(lookup(&standin, "ada"), Err(Error::Busy { back_off_ms: 2_500 }));
    assert_eq!(lookup(&standin, "ada").unwrap().candidates.len(), 2);
}

#[test]
fn what_is_looked_up_is_one_line_of_bounded_length_and_never_nothing() {
    assert!(ResolveNames::new("  \r\n\t ").is_none());
    assert_eq!(ResolveNames::new(" Ada\r\nBcc: x@attacker.example\0 ").unwrap().text(), "Ada Bcc: x@attacker.example");
    assert_eq!(ResolveNames::new(&"a".repeat(10_000)).unwrap().text().chars().count(), resolve::MAX_LOOKUP);
    // What the request writes is escaped by the writer, whatever was typed.
    let standin = organisation();
    assert_eq!(lookup(&standin, "<Ada & \"co\">").unwrap(), Resolved::default());
    assert!(standin.state().requests[0].body.contains("&lt;Ada &amp;"));
}

#[test]
fn a_hostile_answer_is_read_only_so_far() {
    let answer = |set: &str| {
        let body = format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><s:Body>\
             <m:ResolveNamesResponse><m:ResponseMessages><m:ResolveNamesResponseMessage ResponseClass=\"Warning\"><m:ResponseCode>ErrorNameResolutionMultipleResults</m:ResponseCode>\
             <m:ResolutionSet TotalItemsInView=\"2\" IncludesLastItemInRange=\"true\">{set}</m:ResolutionSet></m:ResolveNamesResponseMessage></m:ResponseMessages></m:ResolveNamesResponse></s:Body></s:Envelope>"
        );
        let reply = Reply::judge(&HttpResponse::new(200, body.into_bytes()), 1 << 20).unwrap();
        ResolveNames::new("x").unwrap().answer(&reply).unwrap()
    };
    let mailbox = |name: &str, email: &str, routing: &str| {
        format!(
            "<t:Resolution><t:Mailbox><t:Name>{name}</t:Name><t:EmailAddress>{email}</t:EmailAddress><t:RoutingType>{routing}</t:RoutingType><t:MailboxType>Mailbox</t:MailboxType></t:Mailbox></t:Resolution>"
        )
    };
    let long = "N".repeat(5_000);
    let set = [
        // An Exchange address that is no SMTP address, and addresses that aren't ones.
        mailbox("Legacy", "/o=Example/ou=Group/cn=Recipients/cn=ada", "EX"),
        mailbox("No at", "nobody.example.org", "SMTP"),
        mailbox("Two", "a@b@example.org", "SMTP"),
        mailbox("Header", "ada@example.org&#13;&#10;Bcc: x@attacker.example", "SMTP"),
        // A name of many lines and great length, then the same address twice.
        mailbox(&format!("Ada&#10;Lovelace{long}"), "ada@example.org", "SMTP"),
        mailbox("Ada again", "ADA@example.org", "SMTP"),
        mailbox("", "bea@example.org", "SMTP"),
    ]
    .concat();
    let found = answer(&set);
    let read: Vec<(usize, &str)> = found.candidates.iter().map(|c| (c.name.chars().count(), c.email.as_str())).collect();
    assert_eq!(read, [(256, "ada@example.org"), (0, "bea@example.org")]);
    assert!(found.candidates[0].name.starts_with("Ada Lovelace"));
    assert!(!found.more);
}

#[test]
fn who_was_found_is_never_in_a_debug_line() {
    let standin = organisation();
    let found = lookup(&standin, "lovelace").unwrap();
    let line = format!("{found:?} {:?}", ResolveNames::new("lovelace").unwrap());
    assert!(!line.contains("Ada") && !line.contains("lovelace") && !line.contains("example.org"), "{line}");
}

/// Exchange Server 2019 answers several people with `NoError` and no warning (seen on a hosted
/// one): that is read the same.
#[test]
fn several_found_without_the_warning_are_read_the_same() {
    let resolution = |name: &str, email: &str| {
        format!(
            "<t:Resolution><t:Mailbox><t:Name>{name}</t:Name><t:EmailAddress>{email}</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType>\
             <t:MailboxType>Mailbox</t:MailboxType></t:Mailbox><t:Contact><t:DisplayName>{name}</t:DisplayName><t:CompanyName>Example</t:CompanyName>\
             <t:Department/><t:JobTitle/></t:Contact></t:Resolution>"
        )
    };
    let body = format!(
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><s:Body>\
         <m:ResolveNamesResponse><m:ResponseMessages><m:ResolveNamesResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>\
         <m:ResolutionSet TotalItemsInView=\"2\" IncludesLastItemInRange=\"true\">{}{}</m:ResolutionSet></m:ResolveNamesResponseMessage></m:ResponseMessages></m:ResolveNamesResponse></s:Body></s:Envelope>",
        resolution("Chris Example", "chris@example.org"),
        resolution("Christa Example", "christa@example.org"),
    );
    let reply = Reply::judge(&HttpResponse::new(200, body.into_bytes()), 1 << 20).unwrap();
    let found = ResolveNames::new("chris").unwrap().answer(&reply).unwrap();
    let read: Vec<(&str, &str, &str)> = found.candidates.iter().map(|c| (c.name.as_str(), c.company.as_str(), c.title.as_str())).collect();
    assert_eq!(read, [("Chris Example", "Example", ""), ("Christa Example", "Example", "")]);
    assert!(!found.more);
}
