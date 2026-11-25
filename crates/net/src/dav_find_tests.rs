// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search for an account's calendar and contacts servers, as the host drives it, against the
//! strict stand-in of `mailcraft-dav` (nothing here is a real account's).

use std::sync::Mutex;

use mailcraft_dav::href::Origin;
use mailcraft_dav::http::{HttpExchange, HttpRequest, HttpResponse, TransportError};
use mailcraft_dav::standin::StandIn;
use mailcraft_model::{DavFindFailure, DavFindQuestion};

use super::*;
use crate::dns_srv::SrvTarget;

const USER: &str = "owner@example.org";
const PASSWORD: &str = "test-only-not-a-secret";
const DAV: &str = "https://dav.example.net";

/// One request that reached the world: its origin, and whether it carried the password.
type Sent = (String, bool);

/// The stand-in at `DAV`, and the domain's own name, whose well-known address sends the search
/// there.
struct World {
    server: StandIn,
    log: Mutex<Vec<Sent>>,
}

impl World {
    fn new() -> World {
        let server = StandIn::new(DAV, USER, PASSWORD).unwrap();
        server.add_calendar("work", "Work", false);
        server.add_calendar("shared", "Shared", true);
        server.add_task_list("tasks", "Tasks");
        server.add_address_book("people", "People", false);
        World { server, log: Mutex::new(Vec::new()) }
    }

    fn to(&self, origin: &str) -> Vec<Sent> {
        self.log.lock().unwrap().iter().filter(|(o, _)| o == origin).cloned().collect()
    }
}

impl HttpExchange for World {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let origin = Origin::parse(request.url()).unwrap().base();
        self.log.lock().unwrap().push((origin.clone(), request.field("authorization").is_some()));
        if request.url().starts_with("https://example.org/.well-known/") {
            return Ok(HttpResponse::new(301, Vec::new()).with_field("Location", &format!("{DAV}/dav/")));
        }
        if origin == DAV { Ok(self.server.handle(request)) } else { Err(TransportError::Unreachable) }
    }
}

fn ask<'a>(calendars: &'a str, contacts: &'a str, password: &'a str) -> DavFindAsk<'a> {
    DavFindAsk { email: USER, mail_host: "", username: "", password, calendars, contacts, trusted: "" }
}

fn no_srv(_: DavService, _: &str) -> Option<DavRecords> {
    None
}

#[test]
fn a_typed_address_is_the_only_place_asked_and_its_homes_and_counts_come_back() {
    let world = World::new();
    let typed = format!("{DAV}/dav/");
    let found = find_with(&ask(&typed, &typed, PASSWORD), &world, &mut no_srv, &mut |host, _| panic!("asked about {host}"), &|| false).unwrap();
    assert_eq!(found.calendar_homes, vec![format!("{DAV}/dav/cal/owner/")]);
    assert_eq!(found.contact_homes.len(), 1);
    assert_eq!((found.calendars, found.address_books), (2, 1));
    assert!(found.passed_over >= 1, "the task list is not read");
    let sent = world.log.lock().unwrap().clone();
    assert!(sent.iter().all(|(origin, _)| origin == DAV), "nowhere but the typed host");
    assert!(!sent[0].1, "the first request carries no password");
    assert!(world.server.violations().is_empty());
}

#[test]
fn the_host_the_domain_sends_the_search_to_is_asked_about_and_a_no_sends_it_nothing() {
    for yes in [false, true] {
        let world = World::new();
        let mut asked = Vec::new();
        let found = find_with(
            &ask("", "", PASSWORD),
            &world,
            &mut no_srv,
            &mut |host, why| {
                asked.push((host.to_string(), why));
                // (Asked before anything went there.)
                assert!(world.to(DAV).is_empty());
                Some(yes)
            },
            &|| false,
        );
        assert_eq!(asked, vec![("dav.example.net".to_string(), DavFindQuestion::Elsewhere)], "one question for both kinds");
        if yes {
            let found = found.unwrap();
            assert_eq!((found.calendars, found.address_books), (2, 1));
            assert!(world.to("https://example.org").iter().all(|(_, password)| !password), "the domain's own name gets no password");
        } else {
            assert_eq!(found, Err(DavFindFailure::Declined));
            assert!(world.to(DAV).is_empty());
        }
    }
}

#[test]
fn an_owner_who_gave_up_ends_the_search_and_nothing_more_is_sent() {
    let world = World::new();
    let found = find_with(&ask("", "", PASSWORD), &world, &mut no_srv, &mut |_, _| None, &|| false);
    assert_eq!(found, Err(DavFindFailure::Declined));
    assert!(world.to(DAV).is_empty());
}

#[test]
fn a_refused_password_ends_the_search_at_once() {
    let world = World::new();
    let typed = format!("{DAV}/dav/");
    let found = find_with(&ask(&typed, "", "wrong"), &world, &mut no_srv, &mut |_, _| Some(true), &|| false);
    assert_eq!(found, Err(DavFindFailure::SignInRefused));
    assert_eq!(world.to(DAV).iter().filter(|(_, password)| *password).count(), 1, "tried once, not again");
}

#[test]
fn microsoft_and_google_addresses_end_the_search_and_nothing_is_sent() {
    for (email, failure) in [("ada@outlook.com", DavFindFailure::Microsoft), ("ada@gmail.com", DavFindFailure::Google)] {
        let world = World::new();
        let found = find_with(&DavFindAsk { email, ..ask("", "", PASSWORD) }, &world, &mut no_srv, &mut |_, _| Some(true), &|| false);
        assert_eq!(found, Err(failure));
        assert!(world.log.lock().unwrap().is_empty());
    }
}

#[test]
fn a_typed_address_that_is_not_https_is_refused_before_anything_is_sent() {
    let world = World::new();
    let found = find_with(&ask("http://dav.example.net/dav/", "", PASSWORD), &world, &mut no_srv, &mut |_, _| Some(true), &|| false);
    assert_eq!(found, Err(DavFindFailure::BadSettings));
    assert!(world.log.lock().unwrap().is_empty());
}

#[test]
fn the_look_up_is_asked_by_the_fixed_service_and_the_domain_only() {
    let world = World::new();
    let mut looked = Vec::new();
    let mut srv = |service: DavService, domain: &str| {
        looked.push((service, domain.to_string()));
        (service == DavService::CalDav)
            .then(|| DavRecords { targets: vec![SrvTarget { host: "dav.example.net".into(), port: 443 }], path: Some("/dav/".into()) })
    };
    let found = find_with(&ask("", "", PASSWORD), &world, &mut srv, &mut |_, _| Some(true), &|| false).unwrap();
    assert!(looked.contains(&(DavService::CalDav, "example.org".to_string())));
    assert!(looked.contains(&(DavService::CardDav, "example.org".to_string())));
    assert_eq!(found.calendars, 2);
}

#[test]
fn a_search_that_was_stopped_sends_nothing_more() {
    let world = World::new();
    let typed = format!("{DAV}/dav/");
    // Stopped once the first request went out: no second, and so no password.
    let found = find_with(&ask(&typed, &typed, PASSWORD), &world, &mut no_srv, &mut |_, _| Some(true), &|| !world.log.lock().unwrap().is_empty());
    assert_eq!(found, Err(DavFindFailure::Declined));
    let sent = world.log.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].1);
}
