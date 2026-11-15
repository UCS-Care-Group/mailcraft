// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Discovery against the strict stand-in, one server for each host of the world a test makes:
//! every step, and the rules about where the password goes (nothing here is a real account's).
use super::*;
use crate::standin::{DAV_HEADER, Quirks, StandIn};
use std::sync::Mutex;

const USER: &str = "owner@example.org";
const PASSWORD: &str = "test-only-not-a-secret";

/// One request that reached the world.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    origin: String,
    method: String,
    path: String,
    credential: bool,
}

type Override = Box<dyn Fn(&HttpRequest) -> Option<Result<HttpResponse, TransportError>>>;

/// Hosts that answer as stand-ins do, and hosts that misbehave as a test scripts them.
struct World {
    servers: Vec<StandIn>,
    over: Vec<Override>,
    log: Mutex<Vec<Sent>>,
}

impl World {
    fn new(origins: &[&str]) -> World {
        let servers = origins
            .iter()
            .map(|origin| {
                let s = StandIn::new(origin, USER, PASSWORD).unwrap();
                s.add_calendar("work", "Work", false);
                s.add_calendar("shared", "Shared", true);
                s.add_task_list("tasks", "Tasks");
                s.add_address_book("people", "People", false);
                s
            })
            .collect();
        World { servers, over: Vec::new(), log: Mutex::new(Vec::new()) }
    }

    fn with(mut self, over: impl Fn(&HttpRequest) -> Option<Result<HttpResponse, TransportError>> + 'static) -> World {
        self.over.push(Box::new(over));
        self
    }

    fn sent(&self) -> Vec<Sent> {
        self.log.lock().unwrap().clone()
    }

    /// The requests that carried the password, as (origin, path).
    fn credentialed(&self) -> Vec<(String, String)> {
        self.sent().into_iter().filter(|s| s.credential).map(|s| (s.origin, s.path)).collect()
    }

    fn to(&self, host: &str) -> Vec<Sent> {
        self.sent().into_iter().filter(|s| s.origin.contains(host)).collect()
    }
}

impl HttpExchange for World {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let origin = Origin::parse(request.url()).unwrap();
        let path = origin.path(request.url()).unwrap_or_default();
        self.log.lock().unwrap().push(Sent {
            origin: origin.base(),
            method: request.method().as_str().to_owned(),
            path,
            credential: request.field("authorization").is_some(),
        });
        for over in &self.over {
            if let Some(answer) = over(request) {
                return answer;
            }
        }
        match self.servers.iter().find(|s| *s.origin() == origin) {
            Some(server) => Ok(server.handle(request)),
            None => Err(TransportError::Unreachable),
        }
    }
}

fn secret() -> Secret {
    Secret::basic(USER, PASSWORD).unwrap()
}

fn search(email: &str, calendars: bool, contacts: bool) -> Discovery {
    Discovery::new(email, secret(), calendars, contacts).unwrap()
}

/// No look-up is expected to find anything.
fn no_srv(_: &SrvName) -> Option<SrvAnswer> {
    None
}

/// The test does not expect a question.
fn never_asked(consent: &Consent) -> bool {
    panic!("the owner was asked about {}", consent.host())
}

fn names(located: &Located) -> Vec<String> {
    located.collections().calendars.iter().filter_map(|c| c.display_name.clone()).collect()
}

fn found(outcome: Outcome) -> Discovered {
    match outcome {
        Outcome::Found(found) => found,
        other => panic!("not found: {other:?}"),
    }
}

/// Every credentialed request was preceded, to the same origin, by one without: the password
/// goes only in answer to a host's own challenge.
fn only_after_a_challenge(world: &World) {
    let sent = world.sent();
    for (at, request) in sent.iter().enumerate() {
        if request.credential {
            assert!(
                sent.iter().take(at).any(|before| before.origin == request.origin && !before.credential),
                "a credential went to {} before it was asked for",
                request.origin
            );
        }
    }
}

/// A 401 that offers Basic, with the headers given.
fn challenge(fields: &[(&str, &str)]) -> HttpResponse {
    fields.iter().fold(HttpResponse::new(401, Vec::new()).with_field("WWW-Authenticate", "Basic realm=\"x\""), |r, (n, v)| r.with_field(n, v))
}

fn redirect(to: &str) -> HttpResponse {
    HttpResponse::new(301, Vec::new()).with_field("Location", to)
}

fn typed(kind: Kind, url: &str) -> Discovery {
    search(USER, kind == Kind::Calendar, kind == Kind::AddressBook).typed(kind, url).unwrap()
}

#[test]
fn the_typed_address_is_the_only_place_asked_and_both_kinds_come_back() {
    let world = World::new(&["https://dav.example.net", "https://example.org"]);
    let mut disc = search(USER, true, true)
        .typed(Kind::Calendar, "https://dav.example.net/dav/")
        .unwrap()
        .typed(Kind::AddressBook, "https://dav.example.net/dav/")
        .unwrap();
    let done = found(disc.run(&world, no_srv, never_asked));
    assert_eq!(names(done.calendars().unwrap()), ["Work", "Shared"]);
    assert_eq!(done.calendars().unwrap().home(), "https://dav.example.net/dav/cal/owner/");
    assert_eq!(done.contacts().unwrap().collections().address_books.len(), 1);
    // The task list in the home is passed over, and so is the home itself.
    assert_eq!(done.calendars().unwrap().collections().passed_over, 2);
    // Nothing went anywhere but the typed host, and the password only after its own 401.
    assert!(world.sent().iter().all(|s| s.origin == "https://dav.example.net"));
    only_after_a_challenge(&world);
    let first = &world.sent()[0];
    assert_eq!((first.method.as_str(), first.credential), ("PROPFIND", false));
    // The second kind did not ask the host to challenge again.
    assert_eq!(world.sent().iter().filter(|s| !s.credential).count(), 1);
    assert!(world.servers.iter().all(|s| s.violations().is_empty()));
}

#[test]
fn only_the_kinds_typed_are_searched_for() {
    let world = World::new(&["https://dav.example.net"]);
    let mut disc = search(USER, true, true).typed(Kind::AddressBook, "https://dav.example.net/dav/").unwrap();
    let done = found(disc.run(&world, no_srv, never_asked));
    assert!(done.calendars().is_none() && done.contacts().is_some());
}

#[test]
fn a_typed_address_is_checked_as_an_address() {
    for bad in ["http://dav.example.net/", "ftp://dav.example.net", "https://user:pw@dav.example.net/", "dav.example.net", "", "https://a b/"] {
        assert!(search(USER, true, false).typed(Kind::Calendar, bad).is_err(), "{bad:?}");
    }
    // Plain http only to this machine, for tests.
    assert!(search(USER, true, false).typed(Kind::Calendar, "http://127.0.0.1:8080/dav/").is_ok());
    assert_eq!(Discovery::new("not an address", secret(), true, true).map(|_| ()), Err(AddressError));
    assert!(Discovery::new("a@b", secret(), true, true).is_err());
}

#[test]
fn the_srv_record_and_its_txt_path_say_where_and_the_host_it_names_is_the_owner_s() {
    let world = World::new(&["https://dav.example.net"]);
    let mut disc = search(USER, true, false);
    let mut asked = Vec::new();
    let outcome = disc.run(
        &world,
        |name| {
            asked.push(name.as_str().to_owned());
            Some(SrvAnswer { targets: vec![("DAV.Example.NET.".into(), 443)], path: Some("/dav/".into()) })
        },
        never_asked,
    );
    assert_eq!(names(found(outcome).calendars().unwrap()), ["Work", "Shared"]);
    // Only the domain went out, in the name of the secure service; the plain one is never asked.
    assert_eq!(asked, ["_caldavs._tcp.example.org"]);
    // The domain's own well-known address came before the SRV target, and was asked without the password.
    let sent = world.sent();
    assert_eq!(sent.iter().find(|s| s.origin.contains("example.org")).map(|s| s.credential), Some(false));
    only_after_a_challenge(&world);
}

#[test]
fn contacts_ask_for_the_carddavs_service_and_its_well_known_address() {
    let world = World::new(&["https://carddav.example.net:8443"]);
    let mut disc = search(USER, false, true);
    let mut asked = Vec::new();
    let outcome = disc.run(
        &world,
        |name| {
            asked.push(name.as_str().to_owned());
            Some(SrvAnswer { targets: vec![("carddav.example.net".into(), 8443)], path: None })
        },
        never_asked,
    );
    assert_eq!(found(outcome).contacts().unwrap().collections().address_books.len(), 1);
    assert_eq!(asked, ["_carddavs._tcp.example.org"]);
    // No TXT path: the well-known address of CardDAV, which the host redirected.
    assert!(world.to("carddav.example.net:8443").iter().any(|s| s.path == "/.well-known/carddav"));
}

#[test]
fn the_order_is_the_srv_look_up_then_the_domain_then_the_mail_server_then_the_srv_target() {
    let world = World::new(&[]);
    let mut disc = search(USER, true, false).mail_host("imap.example.org");
    let outcome = disc.run(&world, |_| Some(SrvAnswer { targets: vec![("dav.example.net".into(), 443)], path: None }), never_asked);
    assert_eq!(outcome, Outcome::Failed(Failure::Unreachable));
    let urls: Vec<String> = world.sent().into_iter().map(|s| format!("{}{}", s.origin, s.path)).collect();
    assert_eq!(
        urls,
        ["https://example.org/.well-known/caldav", "https://imap.example.org/.well-known/caldav", "https://dav.example.net/.well-known/caldav"]
    );
}

#[test]
fn the_mail_server_is_the_owner_s_and_gets_the_password_in_answer_to_its_challenge() {
    let world = World::new(&["https://imap.example.org"]);
    let mut disc = search(USER, true, false).mail_host("IMAP.example.org.");
    let outcome = disc.run(&world, no_srv, never_asked);
    assert_eq!(names(found(outcome).calendars().unwrap()), ["Work", "Shared"]);
    // (The well-known address redirected to the root, as RFC 6764 §5 has it.)
    assert!(world.sent().iter().any(|s| s.path == "/.well-known/caldav" && !s.credential));
    only_after_a_challenge(&world);
}

#[test]
fn the_domain_s_own_name_and_other_hosts_in_it_ask_before_the_password_goes() {
    let world = World::new(&["https://example.org"]);
    let mut disc = search(USER, true, false);
    let mut questions = Vec::new();
    let outcome = disc.run(&world, no_srv, |consent| {
        questions.push((consent.host().to_owned(), consent.reason()));
        false
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Declined));
    assert_eq!(questions, [("example.org".to_owned(), ConsentReason::OwnDomain)]);
    assert!(world.credentialed().is_empty(), "a no sends nothing");
    // And a yes is a yes for that host.
    let world = World::new(&["https://example.org"]);
    let outcome = search(USER, true, false).run(&world, no_srv, |_| true);
    assert_eq!(names(found(outcome).calendars().unwrap()), ["Work", "Shared"]);
    only_after_a_challenge(&world);
}

fn off_domain() -> World {
    World::new(&["https://dav.example.net"])
        .with(|r| (r.url().starts_with("https://example.org/.well-known/")).then(|| Ok(redirect("https://dav.example.net/dav/"))))
}

#[test]
fn an_off_domain_redirect_asks_and_a_no_sends_the_password_nowhere() {
    let world = off_domain();
    let mut disc = search(USER, true, false);
    // Step by step: the question names the host whole, and nothing has gone with the password.
    let mut asked = None;
    loop {
        match disc.step() {
            Step::Request(request) => {
                let result = world.exchange(&request);
                disc.response(result);
            }
            Step::LookupSrv(_) => disc.srv(None),
            Step::NeedsYes(consent) => {
                assert_eq!((consent.host(), consent.reason()), ("dav.example.net", ConsentReason::Elsewhere));
                assert!(world.credentialed().is_empty());
                asked = Some(consent.host().to_owned());
                disc.decline();
            }
            Step::Done(outcome) => {
                assert_eq!(outcome, Outcome::Failed(Failure::Declined));
                break;
            }
        }
    }
    assert!(asked.is_some());
    // The host was asked what it is, without the password; it was not asked again.
    assert!(world.credentialed().is_empty());
    assert_eq!(world.to("dav.example.net").len(), 1);
}

#[test]
fn an_off_domain_redirect_that_is_agreed_to_gets_the_password_after_its_own_challenge() {
    let world = off_domain();
    let mut questions = 0;
    let outcome = search(USER, true, true).run(&world, no_srv, |consent| {
        questions += 1;
        assert_eq!(consent.host(), "dav.example.net");
        true
    });
    let done = found(outcome);
    assert!(done.calendars().is_some() && done.contacts().is_some());
    // One question for both kinds, and every credential went to the host that was agreed to.
    assert_eq!(questions, 1);
    assert!(world.credentialed().iter().all(|(origin, _)| origin == "https://dav.example.net"));
    only_after_a_challenge(&world);
    // Nothing went to the domain's own name with the password.
    assert!(world.to("example.org").iter().all(|s| !s.credential));
}

#[test]
fn a_host_that_was_declined_is_not_asked_again_for_the_other_kind() {
    let world = off_domain();
    let mut questions = 0;
    let outcome = search(USER, true, true).run(&world, no_srv, |_| {
        questions += 1;
        false
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Declined));
    assert_eq!(questions, 1);
    assert!(world.credentialed().is_empty());
}

#[test]
fn a_bare_401_is_not_a_calendar_server_and_the_password_stays_home() {
    let world = World::new(&["https://dav.example.net"]);
    world.servers[0].set_quirks(Quirks { bare_401: true, ..Quirks::default() });
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::NotDavServer));
    assert!(world.credentialed().is_empty());
    // It was asked what it is, once, with an OPTIONS and without the password.
    let methods: Vec<String> = world.sent().into_iter().map(|s| s.method).collect();
    assert_eq!(methods, ["PROPFIND", "OPTIONS"]);
}

#[test]
fn an_options_answer_that_names_calendar_access_is_evidence_enough() {
    let world = World::new(&["https://dav.example.net"])
        .with(|r| {
            let anonymous = r.field("authorization").is_none();
            (anonymous && r.method().as_str() == "PROPFIND" && r.url().ends_with("/dav/")).then(|| Ok(challenge(&[])))
        })
        .with(|r| (r.method().as_str() == "OPTIONS").then(|| Ok(HttpResponse::new(200, Vec::new()).with_field("DAV", DAV_HEADER))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let outcome = disc.run(&world, no_srv, never_asked);
    assert!(found(outcome).calendars().is_some());
    only_after_a_challenge(&world);
}

#[test]
fn the_dav_header_must_name_the_service_that_is_looked_for() {
    for (header, kind) in
        [("1, 2", Kind::Calendar), ("1, addressbook", Kind::Calendar), ("1, calendar-access", Kind::AddressBook), ("", Kind::Calendar)]
    {
        let world = World::new(&[]).with(move |_| Some(Ok(challenge(&[("DAV", header)]))));
        let mut disc = typed(kind, "https://web.example.net/");
        assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::NotDavServer), "{header:?}");
        assert!(world.credentialed().is_empty(), "{header:?}");
    }
    // A site that answers 207 with something that is no multistatus is no evidence either.
    let world = World::new(&[]).with(|_| Some(Ok(HttpResponse::new(207, b"<html>hello</html>".to_vec()))));
    let mut disc = typed(Kind::Calendar, "https://web.example.net/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::NotDavServer));
}

#[test]
fn a_challenge_that_is_not_basic_gets_no_password() {
    for scheme in ["Digest realm=\"x\", nonce=\"abc\"", "Negotiate", "Bearer realm=\"x\""] {
        let world = World::new(&[])
            .with(move |_| Some(Ok(HttpResponse::new(401, Vec::new()).with_field("WWW-Authenticate", scheme).with_field("DAV", DAV_HEADER))));
        let mut disc = typed(Kind::Calendar, "https://web.example.net/");
        assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::SignInNotSupported), "{scheme}");
        assert!(world.credentialed().is_empty());
    }
}

#[test]
fn a_wrong_password_stops_the_search_at_once() {
    let world = World::new(&["https://one.example.net", "https://two.example.net"]);
    let wrong = Secret::basic(USER, "not-the-password").unwrap();
    let mut disc = Discovery::new(USER, wrong, true, true).unwrap();
    let outcome = disc.run(
        &world,
        |_| Some(SrvAnswer { targets: vec![("one.example.net".into(), 443), ("two.example.net".into(), 443)], path: Some("/dav/".into()) }),
        |_| true,
    );
    assert_eq!(outcome, Outcome::Failed(Failure::SignInRefused));
    // One try, one host: nothing else was asked with it (lockouts), and not the other kind either.
    assert_eq!(world.credentialed().len(), 1);
    assert!(world.to("two.example.net").is_empty());
    assert!(!Failure::SignInRefused.to_string().contains("example"));
}

#[test]
fn microsoft_hosts_stop_the_search_and_get_nothing() {
    // The address itself.
    for email in ["ada@outlook.com", "ada@contoso.onmicrosoft.com", "ada@hotmail.co.uk"] {
        let world = World::new(&[]);
        assert_eq!(search(email, true, true).run(&world, no_srv, never_asked), Outcome::Microsoft, "{email}");
        assert!(world.sent().is_empty(), "{email}");
    }
    // A mail server of Microsoft's.
    let world = World::new(&[]);
    let mut disc = search(USER, true, true).mail_host("outlook.office365.com");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Microsoft);
    // A typed address.
    let mut disc = typed(Kind::Calendar, "https://outlook.office365.com/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Microsoft);
    assert!(world.sent().is_empty());
    // A redirect, or an SRV record, that leads there.
    let world = World::new(&[]).with(|r| r.url().starts_with("https://example.org/").then(|| Ok(redirect("https://outlook.office365.com/dav/"))));
    assert_eq!(search(USER, true, true).run(&world, no_srv, never_asked), Outcome::Microsoft);
    assert!(world.to("office365.com").is_empty());
    let world = World::new(&[]);
    let named = |_: &SrvName| Some(SrvAnswer { targets: vec![("example-org.mail.protection.outlook.com".into(), 443)], path: None });
    assert_eq!(search(USER, true, false).run(&world, named, never_asked), Outcome::Microsoft);
    assert!(world.credentialed().is_empty() && world.to("outlook.com").is_empty());
}

#[test]
fn google_hosts_stop_the_search_and_get_nothing() {
    for email in ["ada@gmail.com", "ada@googlemail.com"] {
        let world = World::new(&[]);
        assert_eq!(search(email, true, true).run(&world, no_srv, never_asked), Outcome::Google, "{email}");
        assert!(world.sent().is_empty());
    }
    let world = World::new(&[]);
    assert_eq!(search(USER, true, true).mail_host("imap.gmail.com").run(&world, no_srv, never_asked), Outcome::Google);
    let mut disc = typed(Kind::Calendar, "https://apidata.googleusercontent.com/caldav/v2/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Google);
    assert!(world.sent().is_empty());
    // A redirect to Google's CalDAV from a host that was typed.
    let world = World::new(&["https://dav.example.net"])
        .with(|r| r.url().starts_with("https://dav.example.net/dav/").then(|| Ok(redirect("https://apidata.googleusercontent.com/caldav/v2/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Google);
    assert!(world.credentialed().is_empty() && world.to("google").is_empty());
}

/// Providers answer at the root of their host; the stand-in keeps its tree under `/dav/`.
fn root_is_the_tree(world: World) -> World {
    world.with(|r| r.url().ends_with(".com/").then(|| Ok(redirect("/dav/"))))
}

#[test]
fn a_known_provider_s_addresses_are_used_and_nothing_is_looked_up() {
    let world = root_is_the_tree(World::new(&["https://caldav.icloud.com", "https://contacts.icloud.com"]));
    let mut disc = search("ada@iCloud.com", true, true);
    let done = found(disc.run(&world, |name| panic!("looked up {}", name.as_str()), never_asked));
    assert!(done.calendars().is_some() && done.contacts().is_some());
    // The provider's own addresses, and no domain, no mail server, only its own hosts.
    let origins: Vec<String> = world.sent().into_iter().map(|s| s.origin).collect();
    assert!(origins.iter().all(|o| o == "https://caldav.icloud.com" || o == "https://contacts.icloud.com"), "{origins:?}");
    only_after_a_challenge(&world);
    // Fastmail, and a custom domain at Fastmail by its IMAP host.
    let world = root_is_the_tree(World::new(&["https://caldav.fastmail.com"]));
    let mut disc = search("ada@example.org", true, false).mail_host("imap.fastmail.com");
    assert!(found(disc.run(&world, no_srv, never_asked)).calendars().is_some());
}

#[test]
fn a_provider_s_password_does_not_go_to_another_provider_s_hosts_without_asking() {
    // A Fastmail account sent to iCloud's host by a redirect: iCloud's host is not Fastmail's.
    let world = World::new(&["https://caldav.icloud.com"])
        .with(|r| r.url().starts_with("https://caldav.fastmail.com/").then(|| Ok(redirect("https://caldav.icloud.com/"))));
    let mut asked = Vec::new();
    let outcome = search("ada@fastmail.com", true, false).run(&world, no_srv, |consent| {
        asked.push(consent.host().to_owned());
        false
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Declined));
    assert_eq!(asked, ["caldav.icloud.com"]);
    assert!(world.credentialed().is_empty());
}

/// A server at `provider` that names a principal and, in it, a home on `shard`, as iCloud does;
/// the shard is a stand-in.
fn shard_world(provider: &str, shard: &str) -> World {
    let world = World::new(&[shard]);
    let (provider, shard) = (provider.to_owned(), shard.to_owned());
    let ok = secret();
    world.with(move |r| {
        if !r.url().starts_with(&provider) {
            return None;
        }
        if r.field("authorization") != Some(ok.expose()) {
            return Some(Ok(challenge(&[("DAV", DAV_HEADER)])));
        }
        let wrap = |inner: &str| {
            format!(
                "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>{}</d:href><d:propstat><d:prop>{inner}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
                "/"
            )
        };
        let body = if r.url().ends_with("/principal/") {
            wrap(&format!("<c:calendar-home-set><d:href>{shard}/dav/cal/owner/</d:href></c:calendar-home-set>"))
        } else {
            wrap("<d:current-user-principal><d:href>/principal/</d:href></d:current-user-principal>")
        };
        Some(Ok(HttpResponse::new(207, body.into_bytes()).with_field("Content-Type", "application/xml")))
    })
}

#[test]
fn a_home_on_another_host_is_asked_without_the_password_and_the_provider_s_numbered_host_is_trusted() {
    let world = shard_world("https://caldav.icloud.com", "https://p07-caldav.icloud.com");
    let mut disc = search("ada@icloud.com", true, false);
    let outcome = disc.run(&world, no_srv, never_asked);
    assert_eq!(names(found(outcome).calendars().unwrap()), ["Work", "Shared"]);
    // The shard host was asked without the password first, and had it after its own challenge.
    let shard = world.to("p07-caldav.icloud.com");
    assert!(!shard[0].credential && shard.iter().any(|s| s.credential));
    only_after_a_challenge(&world);
}

#[test]
fn a_home_on_a_host_that_is_nobody_s_is_asked_about_first() {
    let world = shard_world("https://dav.example.net", "https://p07-caldav.example.org");
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let mut asked = Vec::new();
    let outcome = disc.run(&world, no_srv, |consent| {
        asked.push((consent.host().to_owned(), consent.reason()));
        false
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Declined));
    // The typed host has the password; the host it named for the home does not, unless the owner agrees.
    assert_eq!(asked, [("p07-caldav.example.org".to_owned(), ConsentReason::OwnDomain)]);
    assert!(world.to("p07-caldav.example.org").iter().all(|s| !s.credential));
    let world = shard_world("https://dav.example.net", "https://p07-caldav.example.org");
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert!(found(disc.run(&world, no_srv, |_| true)).calendars().is_some());
    only_after_a_challenge(&world);
}

#[test]
fn plain_resources_in_the_home_are_skipped_never_a_failure() {
    let world = World::new(&["https://dav.example.net"]);
    world.servers[0].set_quirks(Quirks { sogo_plain_in_home: true, ..Quirks::default() });
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let done = found(disc.run(&world, no_srv, never_asked));
    let located = done.calendars().unwrap();
    assert_eq!(names(located), ["Work", "Shared"]);
    assert_eq!(located.collections().passed_over, 3, "the home, the task list and personal.ics");
}

#[test]
fn a_server_without_a_principal_or_a_home_fails_with_its_own_reason() {
    let empty = |inner: &'static str| {
        World::new(&[]).with(move |r| {
            if r.field("authorization").is_none() {
                return Some(Ok(challenge(&[("DAV", DAV_HEADER)])));
            }
            let body = format!(
                "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/dav/</d:href><d:propstat><d:prop>{inner}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"
            );
            Some(Ok(HttpResponse::new(207, body.into_bytes())))
        })
    };
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&empty(""), no_srv, never_asked), Outcome::Failed(Failure::NoPrincipal));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let principal = "<d:current-user-principal><d:href>/dav/principals/owner/</d:href></d:current-user-principal>";
    assert_eq!(disc.run(&empty(principal), no_srv, never_asked), Outcome::Failed(Failure::NoHome));
}

#[test]
fn redirects_are_followed_one_at_a_time_to_https_only_and_a_chain_ends() {
    let world = World::new(&["https://dav.example.net"])
        .with(|r| (r.url() == "https://dav.example.net/start/").then(|| Ok(redirect("https://dav.example.net/dav/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/start/");
    assert!(found(disc.run(&world, no_srv, never_asked)).calendars().is_some());
    only_after_a_challenge(&world);
    // Never to plain http, and nothing is sent there.
    let world = World::new(&[]).with(|_| Some(Ok(redirect("http://dav.example.net/dav/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::InsecureRedirect));
    assert_eq!(world.sent().len(), 1);
    // Not to anything that is not an address either.
    for location in ["", "ftp://x.example.net/", "javascript:alert(1)", "//evil.example.net/", "https://u:p@evil.example.net/", "\\\\evil"] {
        let world = World::new(&[]).with(move |_| Some(Ok(redirect(location))));
        let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
        assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::InsecureRedirect), "{location:?}");
        assert_eq!(world.sent().len(), 1, "{location:?}");
    }
    // A host that redirects to itself for ever ends after five.
    let world = World::new(&[]).with(|_| Some(Ok(redirect("/again/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::TooManyRedirects));
    assert_eq!(world.sent().len(), usize::from(MAX_REDIRECTS) + 1);
}

#[test]
fn a_redirect_from_a_host_with_the_password_to_another_host_does_not_take_it_along() {
    // The typed host has been signed in to; its next answer redirects to another host.
    let world = World::new(&["https://dav.example.net", "https://other.example.org"])
        .with(|r| (r.url().starts_with("https://dav.example.net/dav/cal/")).then(|| Ok(redirect("https://other.example.org/dav/cal/owner/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let mut asked = Vec::new();
    let outcome = disc.run(&world, no_srv, |consent| {
        asked.push(consent.host().to_owned());
        false
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Declined));
    assert_eq!(asked, ["other.example.org"]);
    assert!(world.to("other.example.org").iter().all(|s| !s.credential));
}

#[test]
fn srv_targets_are_checked_and_a_hostile_path_is_not_used() {
    let world = World::new(&[]);
    let mut disc = search(USER, true, false);
    let hostile = |_: &SrvName| {
        Some(SrvAnswer {
            targets: vec![
                ("evil host.example.net".into(), 443),
                ("localhost".into(), 443),
                ("10.0.0.1".into(), 443),
                ("169.254.169.254".into(), 443),
                ("a.example.net".into(), 80),
                ("b.example.net/path".into(), 443),
                ("ok.example.net".into(), 0),
                ("ok.example.net".into(), 443),
                ("c.example.net".into(), 8443),
                ("d.example.net".into(), 8443),
                ("e.example.net".into(), 8443),
            ],
            path: Some("//evil.example.net/x".into()),
        })
    };
    assert_eq!(disc.run(&world, hostile, never_asked), Outcome::Failed(Failure::Unreachable));
    let urls: Vec<String> = world.sent().into_iter().map(|s| format!("{}{}", s.origin, s.path)).collect();
    // The domain's own address, then at most three targets, at the well-known path (the TXT path
    // was not a path), at 443 or a high port.
    assert_eq!(
        urls,
        [
            "https://example.org/.well-known/caldav",
            "https://ok.example.net/.well-known/caldav",
            "https://c.example.net:8443/.well-known/caldav",
            "https://d.example.net:8443/.well-known/caldav"
        ]
    );
}

#[test]
fn the_password_goes_only_to_a_secure_host_or_this_machine() {
    // A loopback test server over plain http is the one exception.
    let world = World::new(&["http://127.0.0.1:8080"]);
    let mut disc = typed(Kind::Calendar, "http://127.0.0.1:8080/dav/");
    assert!(found(disc.run(&world, no_srv, never_asked)).calendars().is_some());
    // Anywhere else plain http is not an address that can be typed, found, or followed.
    let world = World::new(&[]).with(|r| r.url().starts_with("https://dav.example.net/").then(|| Ok(redirect("http://127.0.0.1:8080/dav/"))));
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    assert_eq!(disc.run(&world, no_srv, never_asked), Outcome::Failed(Failure::InsecureRedirect));
}

#[test]
fn a_certificate_or_connection_failure_is_a_reason_and_the_search_goes_on() {
    let world =
        World::new(&["https://imap.example.org"]).with(|r| r.url().starts_with("https://example.org/").then_some(Err(TransportError::Certificate)));
    let outcome = search(USER, true, false).mail_host("imap.example.org").run(&world, no_srv, never_asked);
    assert!(found(outcome).calendars().is_some());
    let world = World::new(&[]).with(|_| Some(Err(TransportError::Certificate)));
    assert_eq!(search(USER, true, false).run(&world, no_srv, never_asked), Outcome::Failed(Failure::SecureConnection));
    let world = World::new(&[]).with(|_| Some(Ok(HttpResponse::new(403, Vec::new()))));
    assert_eq!(search(USER, true, false).run(&world, no_srv, never_asked), Outcome::Failed(Failure::ServerRefused));
    let world = World::new(&[]).with(|_| Some(Ok(HttpResponse::new(404, Vec::new()))));
    assert_eq!(search(USER, true, false).run(&world, no_srv, never_asked), Outcome::Failed(Failure::NotFound));
}

#[test]
fn nothing_found_anywhere_is_a_failure_with_a_sentence_that_names_nothing() {
    let world = World::new(&[]);
    let outcome = search(USER, true, true).run(&world, no_srv, never_asked);
    assert_eq!(outcome, Outcome::Failed(Failure::Unreachable));
    for why in [
        Failure::Unreachable,
        Failure::NotFound,
        Failure::NotDavServer,
        Failure::SecureConnection,
        Failure::Unreadable,
        Failure::InsecureRedirect,
        Failure::ServerRefused,
        Failure::NoPrincipal,
        Failure::NoHome,
        Failure::SignInNotSupported,
        Failure::Declined,
        Failure::SignInRefused,
        Failure::TooManyRedirects,
    ] {
        assert!(!why.to_string().contains("example") && why.to_string().ends_with('.'), "{why:?}");
    }
}

#[test]
fn the_machine_ignores_calls_made_out_of_turn_and_says_the_same_until_answered() {
    let mut disc = search(USER, true, false);
    disc.response(Ok(HttpResponse::new(207, Vec::new())));
    disc.srv(None);
    disc.agree();
    disc.decline();
    let first = disc.step();
    assert_eq!(first, disc.step());
    assert!(matches!(first, Step::LookupSrv(_)));
    disc.response(Err(TransportError::Failed));
    assert_eq!(first, disc.step());
}

#[test]
fn nothing_of_the_owner_shows_in_debug_output() {
    let world = World::new(&["https://dav.example.net"]);
    let mut disc = typed(Kind::Calendar, "https://dav.example.net/dav/");
    let step = disc.step();
    let shown = format!("{disc:?} {step:?}");
    let outcome = disc.run(&world, no_srv, never_asked);
    let consent = Consent { host: "dav.example.net".into(), reason: ConsentReason::Elsewhere };
    let all = format!(
        "{shown} {outcome:?} {consent:?} {:?} {:?}",
        SrvName("_caldavs._tcp.example.org".into()),
        SrvAnswer { targets: vec![("dav.example.net".into(), 443)], path: Some("/p".into()) }
    );
    for secret in ["example", "owner", PASSWORD, "Basic", "dav"] {
        assert!(!all.contains(secret), "{secret}: {all}");
    }
}

#[test]
fn a_search_ends_whatever_the_server_says() {
    // Every answer a hostile server can give, over and over: the search always ends, and the
    // requests are bounded.
    let answers: Vec<Box<dyn Fn() -> Result<HttpResponse, TransportError>>> = vec![
        Box::new(|| Ok(redirect("https://a.example.net/x"))),
        Box::new(|| Ok(challenge(&[("DAV", DAV_HEADER)]))),
        Box::new(|| Ok(HttpResponse::new(207, b"<d:multistatus xmlns:d=\"DAV:\"/>".to_vec()))),
        Box::new(|| Ok(HttpResponse::new(500, vec![0xff; 10]))),
        Box::new(|| Err(TransportError::BodyTooLarge)),
    ];
    for answer in answers {
        let world = World::new(&[]).with(move |_| Some(answer()));
        let hosts = |_: &SrvName| Some(SrvAnswer { targets: vec![("a.example.net".into(), 443)], path: None });
        let outcome = search(USER, true, true).run(&world, hosts, |_| true);
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(world.sent().len() <= usize::from(MAX_REQUESTS));
    }
}
