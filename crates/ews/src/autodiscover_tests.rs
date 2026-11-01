// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Autodiscover state machine against the strict stand-in: each step, the redirects and the
//! owner's say, Exchange Online, and answers that are hostile.

use proptest::prelude::*;

use super::*;
use crate::standin::block_on;
use crate::standin_autodiscover::{AutodiscoverStandIn, BASIC, Route, error_answer, redirect_addr, redirect_url, settings};

const INTERNAL: &str = "https://exch01.corp.example.org/EWS/Exchange.asmx";
const EXTERNAL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn good() -> Route {
    Route::Document(settings(INTERNAL, EXTERNAL, "exch01.corp.example.org"))
}

fn search(email: &str) -> Autodiscover {
    Autodiscover::new(email, Secret::new(BASIC)).unwrap()
}

/// A whole search: no SRV record, and the owner's answers given in order (then no).
struct Run {
    outcome: Outcome,
    asked: Vec<(String, ConsentReason)>,
    srv: Vec<String>,
}

fn run_with(standin: &AutodiscoverStandIn, mut search: Autodiscover, srv: Option<Vec<&str>>, yes: &[bool]) -> Run {
    let (mut asked, mut looked, mut at) = (Vec::new(), Vec::new(), 0usize);
    let srv = srv.map(|hosts| hosts.into_iter().map(|host| (host.to_owned(), 443)).collect::<Vec<_>>());
    let outcome = block_on(search.run(
        standin,
        |name| {
            looked.push(name.to_owned());
            srv.clone()
        },
        |consent| {
            asked.push((consent.host().to_owned(), consent.reason()));
            at += 1;
            yes.get(at - 1).copied().unwrap_or(false)
        },
    ))
    .unwrap();
    Run { outcome, asked, srv: looked }
}

fn go(standin: &AutodiscoverStandIn, srv: Option<Vec<&str>>, yes: &[bool]) -> Run {
    run_with(standin, search("ada@example.org"), srv, yes)
}

fn found(run: &Run) -> &Discovered {
    let Outcome::Found(found) = &run.outcome else { panic!("not found: {:?}", run.outcome) };
    found
}

/// Every sign-in went to the owner's domain or to a host in `also`.
fn only_to(standin: &AutodiscoverStandIn, also: &[&str]) {
    for seen in &standin.state().seen {
        if seen.credentials {
            assert!(seen.authorized, "{}", seen.host);
            assert!(is_within(&seen.host, "example.org") || also.contains(&seen.host.as_str()), "the sign-in went to {}", seen.host);
        }
    }
}

// ---- the three places, in order ----

#[test]
fn the_domain_itself_is_asked_first() {
    let standin = AutodiscoverStandIn::new().route("example.org", good());
    let run = go(&standin, None, &[]);
    let found = found(&run);
    assert_eq!(found.urls(), [EXTERNAL, INTERNAL]);
    assert_eq!((found.external_url(), found.internal_url(), found.server()), (Some(EXTERNAL), Some(INTERNAL), Some("exch01.corp.example.org")));
    assert_eq!(standin.hosts_asked(), ["example.org"]);
    let state = standin.state();
    assert_eq!((state.seen[0].url.as_str(), state.seen[0].email.as_str()), ("https://example.org/autodiscover/autodiscover.xml", "ada@example.org"));
    assert!(run.asked.is_empty() && run.srv.is_empty());
}

#[test]
fn the_autodiscover_host_is_asked_second() {
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Status(404)).route("autodiscover.example.org", good());
    let run = go(&standin, None, &[]);
    assert_eq!(found(&run).external_url(), Some(EXTERNAL));
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org"]);
    assert!(run.srv.is_empty());
    only_to(&standin, &[]);
}

#[test]
fn a_host_that_is_down_or_has_no_certificate_goes_on_to_the_next() {
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Unreachable).route("autodiscover.example.org", Route::TlsFails);
    let run = go(&standin, Some(vec!["ad.example.org"]), &[]);
    // (Nothing at ad.example.org either: the stand-in has no route for it.)
    assert_eq!(run.outcome, Outcome::Failed(Failure::SecureConnection));
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org", "ad.example.org"]);
}

#[test]
fn the_srv_record_is_third_and_a_host_inside_the_domain_needs_no_question() {
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Status(404))
        .route("autodiscover.example.org", Route::Unreachable)
        .route("ad.example.org", good());
    let run = go(&standin, Some(vec!["AD.example.org."]), &[]);
    found(&run);
    assert_eq!(run.srv, ["_autodiscover._tcp.example.org"]);
    assert!(run.asked.is_empty());
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org", "ad.example.org"]);
}

#[test]
fn the_host_an_srv_record_names_outside_the_domain_is_asked_about_first() {
    let make = || AutodiscoverStandIn::new().route("hosted.example.net", good());
    let standin = make();
    let run = go(&standin, Some(vec!["hosted.example.net"]), &[true]);
    found(&run);
    assert_eq!(run.asked, [("hosted.example.net".to_owned(), ConsentReason::Redirect)]);
    only_to(&standin, &["hosted.example.net"]);

    // Declined: no sign-in goes there, and the search ends with that said.
    let standin = make();
    let run = run_with(&standin, search("ada@example.org"), Some(vec!["hosted.example.net"]), &[false]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::Declined));
    assert!(standin.hosts_asked().iter().all(|host| host != "hosted.example.net"), "{:?}", standin.hosts_asked());
}

#[test]
fn what_an_srv_lookup_gives_is_checked() {
    let standin = AutodiscoverStandIn::new();
    let hostile = vec![
        "",
        ".",
        "localhost",
        "a b.example.org",
        "ex@mple.org",
        "evil.example.org/path",
        "https://evil.example.org",
        "-x.example.org",
        "x_y.example.org",
        "e\r\nvil.example.org",
    ];
    let run = go(&standin, Some(hostile), &[]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::Unreachable));
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org"]);

    // At most three, each once.
    let standin = AutodiscoverStandIn::new();
    let many = vec!["a.example.org", "A.example.org.", "b.example.org", "c.example.org", "d.example.org", "e.example.org"];
    run_with(&standin, search("ada@example.org"), Some(many), &[]);
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org", "a.example.org", "b.example.org", "c.example.org"]);
    // A lookup that failed is the same as one that found nothing.
    let standin = AutodiscoverStandIn::new();
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::Unreachable));
}

// ---- redirects ----

fn hop(from: &str, to: &str) -> (String, Route) {
    (from.to_owned(), Route::Redirect(302, format!("https://{to}/autodiscover/autodiscover.xml")))
}

fn chain(hosts: &[&str], settled: Route) -> AutodiscoverStandIn {
    let mut standin = AutodiscoverStandIn::new();
    for pair in hosts.windows(2) {
        let (from, route) = hop(pair[0], pair[1]);
        standin = standin.route(&from, route);
    }
    standin.route(hosts[hosts.len() - 1], settled)
}

#[test]
fn a_redirect_inside_the_domain_is_followed_without_asking() {
    let standin = chain(&["example.org", "mail.example.org"], good());
    let run = go(&standin, None, &[]);
    found(&run);
    assert!(run.asked.is_empty());
    assert_eq!(standin.hosts_asked(), ["example.org", "mail.example.org"]);
    only_to(&standin, &[]);
    for status in [301, 302, 303, 307, 308] {
        let standin = AutodiscoverStandIn::new()
            .route("example.org", Route::Redirect(status, "/autodiscover/autodiscover.xml".into()))
            .route("autodiscover.example.org", good());
        // (A path on the same host is the same host again: a loop, ended by the cap.)
        let run = run_with(&standin, search("ada@example.org"), None, &[]);
        assert_eq!(run.outcome, Outcome::Failed(Failure::TooManyRedirects), "{status}");
    }
    let standin =
        AutodiscoverStandIn::new().route("example.org", Route::Redirect(301, "https://example.org:8443/autodiscover/autodiscover.xml".into()));
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::TooManyRedirects));
}

#[test]
fn a_redirect_outside_the_domain_stops_and_asks() {
    let standin = chain(&["example.org", "login.example.net"], good());
    let mut machine = search("ada@example.org");
    let Step::Request(request) = machine.step() else { panic!("no request") };
    machine.response(block_on(standin.exchange(&request)).unwrap());
    // Stopped, naming the host; the question is the same however often it is asked; nothing was sent there.
    for _ in 0..3 {
        let Step::NeedsYes(consent) = machine.step() else { panic!("not asked") };
        assert_eq!((consent.host(), consent.reason()), ("login.example.net", ConsentReason::Redirect));
    }
    assert_eq!(standin.hosts_asked(), ["example.org"]);
    machine.agree();
    let Step::Request(request) = machine.step() else { panic!("not sent on") };
    assert_eq!(request.url(), "https://login.example.net/autodiscover/autodiscover.xml");
    machine.response(block_on(standin.exchange(&request)).unwrap());
    // It asks for the sign-in, and only then does the sign-in go.
    let Step::Request(again) = machine.step() else { panic!("no sign-in asked for") };
    assert_eq!(again.field("authorization"), Some(BASIC));
    machine.response(block_on(standin.exchange(&again)).unwrap());
    assert!(matches!(machine.step(), Step::Done(Outcome::Found(_))));
    assert_eq!(standin.hosts_asked(), ["example.org", "login.example.net"]);

    // Said no: it is never sent there, and the next place in the domain is tried.
    let standin = chain(&["example.org", "login.example.net"], good()).route("autodiscover.example.org", good());
    let run = go(&standin, None, &[false]);
    found(&run);
    assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org"]);
}

#[test]
fn an_agreement_is_for_that_host_only() {
    // Two hosts outside the domain in a row: the owner is asked about each.
    let standin = chain(&["example.org", "a.example.net", "b.example.net"], good());
    let run = go(&standin, None, &[true, true]);
    found(&run);
    assert_eq!(run.asked.iter().map(|(host, _)| host.as_str()).collect::<Vec<_>>(), ["a.example.net", "b.example.net"]);
    // A look-alike of the domain is not in it.
    let standin = chain(&["example.org", "evil-example.org"], good());
    let run = go(&standin, None, &[false]);
    assert_eq!(run.asked, [("evil-example.org".to_owned(), ConsentReason::Redirect)]);
    assert!(!standin.hosts_asked().contains(&"evil-example.org".to_owned()));
}

#[test]
fn five_redirects_are_followed_and_a_sixth_ends_the_search() {
    let five = ["example.org", "a1.example.org", "a2.example.org", "a3.example.org", "a4.example.org", "a5.example.org"];
    let standin = chain(&five, good());
    found(&go(&standin, None, &[]));
    assert_eq!(standin.hosts_asked().len(), 6);

    let six = ["example.org", "a1.example.org", "a2.example.org", "a3.example.org", "a4.example.org", "a5.example.org", "a6.example.org"];
    let standin = chain(&six, good());
    let run = go(&standin, None, &[]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::TooManyRedirects));
    // The sixth is not even asked, and the other places are not tried: this is a loop, not an absence.
    assert_eq!(standin.hosts_asked().len(), 6);
    assert!(!standin.hosts_asked().contains(&"a6.example.org".to_owned()));

    // A loop between two hosts ends the same way.
    let standin = AutodiscoverStandIn::new()
        .route("example.org", hop("x", "autodiscover.example.org").1)
        .route("autodiscover.example.org", hop("x", "example.org").1);
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::TooManyRedirects));
    assert_eq!(standin.hosts_asked().len(), 6);
}

#[test]
fn a_redirect_that_is_not_https_is_refused() {
    for location in [
        "http://mail.example.org/autodiscover/autodiscover.xml",
        "HTTP://mail.example.org/autodiscover/autodiscover.xml",
        "http://127.0.0.1/autodiscover/autodiscover.xml",
        "ftp://mail.example.org/",
        "//mail.example.org/autodiscover/autodiscover.xml",
        "autodiscover/autodiscover.xml",
        "https://user:pw@mail.example.org/autodiscover/autodiscover.xml",
        "https://mail.example.org/a b",
        "javascript:alert(1)",
        "",
    ] {
        let standin = AutodiscoverStandIn::new().route("example.org", Route::Redirect(302, location.into())).route("mail.example.org", good());
        let run = run_with(&standin, search("ada@example.org"), None, &[]);
        assert_eq!(run.outcome, Outcome::Failed(Failure::InsecureRedirect), "{location:?}");
        // (The stand-in fails the test by itself if anything is asked over plain HTTP.)
        assert_eq!(standin.hosts_asked(), ["example.org", "autodiscover.example.org"], "{location:?}");
    }
    // The same in an answer's redirectUrl.
    let standin =
        AutodiscoverStandIn::new().route("example.org", Route::Document(redirect_url("http://mail.example.org/autodiscover/autodiscover.xml")));
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::InsecureRedirect));
}

#[test]
fn redirect_url_in_an_answer_is_a_redirect_like_any() {
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Document(redirect_url("https://exch01.example.org/Autodiscover/Autodiscover.xml")))
        .route("exch01.example.org", good());
    let run = go(&standin, None, &[]);
    found(&run);
    assert!(run.asked.is_empty());
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Document(redirect_url("https://exch01.example.net/autodiscover/autodiscover.xml")))
        .route("exch01.example.net", good());
    let run = go(&standin, None, &[true]);
    found(&run);
    assert_eq!(run.asked, [("exch01.example.net".to_owned(), ConsentReason::Redirect)]);
}

#[test]
fn redirect_addr_starts_again_with_the_new_address_and_may_ask_about_its_domain() {
    // Inside the domain: no question.
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Document(redirect_addr("Ada@Mail.Example.org")))
        .route("autodiscover.mail.example.org", good());
    let run = go(&standin, None, &[]);
    found(&run);
    assert!(run.asked.is_empty());
    assert_eq!(standin.state().seen.last().map(|seen| seen.email.clone()), Some("Ada@mail.example.org".to_owned()));

    // Another domain: asked once for the domain, then its places are asked without more questions.
    let make = || {
        AutodiscoverStandIn::new()
            .route("example.org", Route::Document(redirect_addr("ada@example.net")))
            .route("example.net", Route::Status(404))
            .route("autodiscover.example.net", good())
    };
    let standin = make();
    let run = go(&standin, None, &[true]);
    found(&run);
    assert_eq!(run.asked, [("example.net".to_owned(), ConsentReason::OtherDomain)]);
    assert_eq!(standin.hosts_asked(), ["example.org", "example.net", "autodiscover.example.net"]);

    // Declined: nothing goes to that domain, and nothing else is tried in it.
    let standin = make();
    let run = run_with(&standin, search("ada@example.org"), Some(vec!["srv.example.net"]), &[false]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::Declined));
    assert_eq!(standin.hosts_asked(), ["example.org"]);

    // Garbage in place of an address.
    for bad in ["", "nobody", "a@b", "a b@example.net", "<ada@example.net>", "a@@example.net"] {
        let standin = AutodiscoverStandIn::new().route("example.org", Route::Document(redirect_addr(bad)));
        assert!(matches!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(_)), "{bad:?}");
        assert_eq!(standin.hosts_asked().len(), 2, "{bad:?}");
    }
}

// ---- Exchange Online ----

#[test]
fn a_mailbox_in_exchange_online_is_said_so_and_nothing_is_sent_there() {
    // The settings name a Microsoft 365 host.
    let online = Route::Document(settings(
        "https://outlook.office365.com/EWS/Exchange.asmx",
        "https://outlook.office365.com/EWS/Exchange.asmx",
        "outlook.office365.com",
    ));
    let standin = AutodiscoverStandIn::new().route("example.org", online);
    let run = go(&standin, None, &[]);
    assert_eq!(run.outcome, Outcome::ExchangeOnline);
    assert!(run.asked.is_empty());

    // Only the server says so.
    let route = Route::Document(settings(INTERNAL, EXTERNAL, "mail.eur01.outlook.com"));
    let standin = AutodiscoverStandIn::new().route("example.org", route);
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::ExchangeOnline);

    // A redirect to Microsoft's Autodiscover: never asked, never sent the sign-in.
    for target in ["https://autodiscover-s.outlook.com/autodiscover/autodiscover.xml", "https://outlook.office365.com/autodiscover/autodiscover.xml"]
    {
        let standin = AutodiscoverStandIn::new()
            .route("example.org", Route::Redirect(302, target.into()))
            .route("autodiscover-s.outlook.com", good())
            .route("outlook.office365.com", good());
        let run = run_with(&standin, search("ada@example.org"), None, &[true, true]);
        assert_eq!(run.outcome, Outcome::ExchangeOnline, "{target}");
        assert!(run.asked.is_empty(), "{target}");
        assert_eq!(standin.hosts_asked(), ["example.org"], "{target}");
    }
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Document(redirect_url("https://autodiscover-s.outlook.com/autodiscover/autodiscover.xml")));
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[true]).outcome, Outcome::ExchangeOnline);

    // A redirect to an address in a tenant.
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Document(redirect_addr("ada@example.onmicrosoft.com")));
    let run = run_with(&standin, search("ada@example.org"), None, &[true]);
    assert_eq!((run.outcome, run.asked.len(), standin.hosts_asked().len()), (Outcome::ExchangeOnline, 0, 1));

    // The owner's own address is a Microsoft one: no request at all.
    for email in ["ada@outlook.com", "ada@Hotmail.OUTLOOK.com", "ada@contoso.onmicrosoft.com"] {
        let standin = AutodiscoverStandIn::new();
        assert_eq!(run_with(&standin, search(email), None, &[]).outcome, Outcome::ExchangeOnline, "{email}");
        assert!(standin.hosts_asked().is_empty(), "{email}");
    }
}

#[test]
fn a_redirect_to_a_microsoft_consumer_address_is_exchange_online_and_nothing_is_sent() {
    for target in ["ada@hotmail.co.uk", "ada@outlook.com", "ada@live.com", "ada@Hotmail.Co.UK"] {
        let standin = AutodiscoverStandIn::new()
            .route("example.org", Route::Document(redirect_addr(target)))
            .route("hotmail.co.uk", good())
            .route("autodiscover.hotmail.co.uk", good());
        let run = run_with(&standin, search("ada@example.org"), None, &[true, true]);
        assert_eq!(run.outcome, Outcome::ExchangeOnline, "{target}");
        assert!(run.asked.is_empty(), "{target}");
        assert_eq!(standin.hosts_asked(), ["example.org"], "{target}: nothing went to the new address's domain");
    }
}

#[test]
fn look_alikes_of_microsoft_hosts_are_not_exchange_online() {
    for yes in [
        "outlook.com",
        "mail.outlook.com",
        "outlook.office365.com",
        "x.y.onmicrosoft.com",
        "partner.outlook.cn",
        "hotmail.com",
        "autodiscover.hotmail.com",
        "live.com",
        "msn.com",
    ] {
        assert!(is_exchange_online(yes), "{yes}");
    }
    for no in [
        "outlook.abc.com",
        "mail.outlook.abc.com",
        "live.bbc.com",
        "msn.xy.org",
        "live.co.uk",
        "hotmail.co.uk",
        "outlook.de",
        "live.somecompany.example",
        "hotmail.com.evil.example",
        "hotmail",
        "notoutlook.com",
        "outlook.com.example.org",
        "office365.com.evil.example",
        "example.org",
        "outlook",
        "",
        "xoffice.com",
    ] {
        assert!(!is_exchange_online(no), "{no}");
    }
}

// ---- the server's own address ----

#[test]
fn an_address_on_another_host_asks_before_the_password_goes_there() {
    let route = || {
        Route::Document(settings(
            "https://exch.hoster.example.net/EWS/Exchange.asmx",
            "https://exch.hoster.example.net/EWS/Exchange.asmx",
            "exch.hoster.example.net",
        ))
    };
    let standin = AutodiscoverStandIn::new().route("example.org", route());
    let run = go(&standin, None, &[true]);
    assert_eq!(found(&run).urls(), ["https://exch.hoster.example.net/EWS/Exchange.asmx"]);
    assert_eq!(run.asked, [("exch.hoster.example.net".to_owned(), ConsentReason::MailServer)]);

    let standin = AutodiscoverStandIn::new().route("example.org", route());
    let run = run_with(&standin, search("ada@example.org"), None, &[false]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::Declined));

    // A host agreed to for a redirect is not asked about again for the address.
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Redirect(302, "https://exch.hoster.example.net/autodiscover/autodiscover.xml".into()))
        .route("exch.hoster.example.net", route());
    let run = run_with(&standin, search("ada@example.org"), None, &[true]);
    found(&run);
    assert_eq!(run.asked.len(), 1);
}

// ---- failures ----

#[test]
fn a_refused_sign_in_ends_the_search_at_the_first_host() {
    let standin = AutodiscoverStandIn::new().route("example.org", good()).route("autodiscover.example.org", good());
    let machine = Autodiscover::new("ada@example.org", Secret::new("Basic d3Jvbmc6d3Jvbmc=")).unwrap();
    let run = run_with(&standin, machine, Some(vec!["ad.example.org"]), &[]);
    assert_eq!(run.outcome, Outcome::Failed(Failure::SignInRefused));
    assert_eq!(standin.hosts_asked(), ["example.org"]);
    assert!(run.srv.is_empty());
}

#[test]
fn each_failure_has_its_reason() {
    let case = |route: Route| {
        let standin = AutodiscoverStandIn::new().route("example.org", route.clone()).route("autodiscover.example.org", route);
        run_with(&standin, search("ada@example.org"), None, &[]).outcome
    };
    assert_eq!(case(Route::Status(404)), Outcome::Failed(Failure::NoAutodiscover));
    assert_eq!(case(Route::Status(500)), Outcome::Failed(Failure::NoAutodiscover));
    assert_eq!(case(Route::Status(403)), Outcome::Failed(Failure::ServerRefused));
    assert_eq!(case(Route::Document(error_answer())), Outcome::Failed(Failure::ServerRefused));
    assert_eq!(case(Route::TlsFails), Outcome::Failed(Failure::SecureConnection));
    assert_eq!(case(Route::Unreachable), Outcome::Failed(Failure::Unreachable));
    assert_eq!(case(Route::Bytes(b"<html><body>Sign in</body></html>".to_vec())), Outcome::Failed(Failure::Unreadable));
    assert_eq!(case(Route::Document(settings("", "", "mail.example.org"))), Outcome::Failed(Failure::NoExchangeAddress));
    assert_eq!(case(Route::Status(204)), Outcome::Failed(Failure::NoAutodiscover));
    // The most telling reason is kept when the places differ.
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Unreachable).route("autodiscover.example.org", Route::Status(403));
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::ServerRefused));
}

#[test]
fn every_failure_offers_server_settings_and_holds_nothing_a_server_wrote() {
    for failure in [
        Failure::SignInNotSupported,
        Failure::SignInNtlmOnly,
        Failure::Unreachable,
        Failure::NoAutodiscover,
        Failure::SecureConnection,
        Failure::Unreadable,
        Failure::InsecureRedirect,
        Failure::ServerRefused,
        Failure::NoExchangeAddress,
        Failure::Declined,
        Failure::SignInRefused,
        Failure::TooManyRedirects,
    ] {
        let text = failure.to_string();
        // (Kerberos only: typing the address would not help, and the words don't say it would.)
        assert!(text.contains("Server settings") != (failure == Failure::SignInNotSupported), "{text}");
        assert!(!text.contains("ada") && !text.contains("exch") && !text.contains("https:"), "{text}");
    }
    assert!(AddressError.to_string().contains("Server settings"));
}

// ---- hostile answers ----

fn answer_of(account: &str) -> Vec<u8> {
    format!("<Autodiscover xmlns=\"{ANSWER_SCHEMA}\"><Response xmlns=\"{RESPONSE_SCHEMA}\"><Account>{account}</Account></Response></Autodiscover>")
        .into_bytes()
}

fn protocol(kind: &str, extra: &str) -> String {
    format!("<Protocol><Type>{kind}</Type>{extra}</Protocol>")
}

#[test]
fn only_the_address_and_the_server_are_taken() {
    let account = format!(
        "<Action>settings</Action>{}{}{}",
        protocol(
            "EXCH",
            "<Server>Mail.Example.org.</Server><EwsUrl>https://mail.example.org/EWS/A.asmx</EwsUrl><ASUrl>https://evil.example.net/AS</ASUrl><OOFUrl>https://evil.example.net/OOF</OOFUrl><EmwsUrl>https://evil.example.net/EMWS</EmwsUrl><EwsPartnerUrl>https://evil.example.net/P</EwsPartnerUrl>"
        ),
        protocol("EXCH", "<EwsUrl>https://evil.example.net/second</EwsUrl>"),
        protocol("WEB", "<EwsUrl>https://evil.example.net/web</EwsUrl>")
    );
    let Answer::Settings(found) = read_answer(&answer_of(&account)) else { panic!() };
    assert_eq!(found.urls(), ["https://mail.example.org/EWS/A.asmx"]);
    assert_eq!(found.server(), Some("mail.example.org"));
    assert!(found.internal_url().is_some() && found.external_url().is_none());
    // Debug shows only what is there.
    assert!(!format!("{found:?}").contains("mail"), "{found:?}");
}

#[test]
fn addresses_that_are_not_https_are_not_taken() {
    for bad in [
        "http://mail.example.org/EWS/Exchange.asmx",
        "HTTP://mail.example.org/EWS/Exchange.asmx",
        "http://127.0.0.1/EWS/Exchange.asmx",
        "ftp://mail.example.org/",
        "file:///etc/passwd",
        "//mail.example.org/EWS",
        "mail.example.org/EWS",
        "https://user:pw@mail.example.org/EWS",
        "https://",
        "https:///EWS",
        "https://mail example.org/EWS",
        "https://mail.example.org/\u{202e}EWS",
        "",
    ] {
        let account = format!("<Action>settings</Action>{}", protocol("EXCH", &format!("<EwsUrl>{}</EwsUrl>", xml::escape(bad))));
        assert_eq!(read_answer(&answer_of(&account)), Answer::Settings(Discovered { external: None, internal: None, server: None }), "{bad:?}");
    }
    let long = format!("https://mail.example.org/{}", "a".repeat(3_000));
    let account = format!("<Action>settings</Action>{}", protocol("EXCH", &format!("<EwsUrl>{long}</EwsUrl>")));
    assert!(matches!(read_answer(&answer_of(&account)), Answer::Settings(found) if found.urls().is_empty()));
}

#[test]
fn hostile_documents_are_unreadable_not_trusted() {
    let honest = String::from_utf8(answer_of(&format!(
        "<Action>settings</Action>{}",
        protocol("EXCH", "<EwsUrl>https://mail.example.org/EWS/Exchange.asmx</EwsUrl>")
    )))
    .unwrap();
    assert!(matches!(read_answer(honest.as_bytes()), Answer::Settings(_)));
    let bomb = "<!DOCTYPE a [<!ENTITY x \"aaaaaaaaaa\"><!ENTITY y \"&x;&x;&x;&x;&x;&x;&x;&x;\"><!ENTITY z \"&y;&y;&y;&y;&y;&y;&y;&y;\">]>";
    let external = "<!DOCTYPE a [<!ENTITY x SYSTEM \"file:///etc/passwd\">]>";
    for hostile in [
        format!("{bomb}{honest}"),
        format!("{external}{honest}"),
        honest.replace("<Action>settings</Action>", "<Action>settings</Action><!ENTITY x>"),
        format!("{}{honest}{}", "<a>".repeat(5_000), "</a>".repeat(5_000)),
        honest.replace(ANSWER_SCHEMA, "http://example.org/other"),
        honest.replace(RESPONSE_SCHEMA, "http://example.org/other"),
        honest.replace("Autodiscover", "Autodiscovery"),
        honest.replace("<Account>", "<Acount>").replace("</Account>", "</Acount>"),
        honest.replace("settings", "somethingElse"),
        honest.replace("</Autodiscover>", ""),
        String::new(),
        "\u{0}\u{1}\u{2}".to_owned(),
        "not xml".to_owned(),
    ] {
        let read = read_answer(hostile.as_bytes());
        assert!(!matches!(&read, Answer::Settings(found) if found.urls().iter().any(|url| url.contains("passwd"))), "{read:?}");
        assert!(matches!(read, Answer::Unreadable), "{hostile:.120}");
    }
    // An entity that is not one of XML's five spoils the text it is in, and so the address.
    let spoiled = honest.replace("mail.example.org/EWS", "mail.example.org/&bogus;EWS");
    assert!(matches!(read_answer(spoiled.as_bytes()), Answer::Settings(found) if found.urls().is_empty()));
    // Megabytes of address are never read.
    let huge = honest.replace("mail.example.org", &"a".repeat(MAX_ANSWER + 10));
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Bytes(huge.into_bytes()));
    assert_eq!(run_with(&standin, search("ada@example.org"), None, &[]).outcome, Outcome::Failed(Failure::Unreadable));
    // And the request asks the transport to stop reading at the same limit.
    let Step::Request(request) = search("ada@example.org").step() else { panic!() };
    assert_eq!(request.max_response_bytes(), MAX_ANSWER);
}

// ---- the request, the secrets, misuse ----

#[test]
fn the_request_is_the_documented_one_and_the_address_is_escaped() {
    let mut machine = search("ada@example.org");
    let Step::Request(request) = machine.step() else { panic!() };
    assert_eq!(request.method(), Method::Post);
    assert_eq!(request.field("content-type"), Some("text/xml; charset=utf-8"));
    // The first request to any host carries no sign-in.
    assert_eq!(request.field("authorization"), None);
    let body = String::from_utf8(request.body().to_vec()).unwrap();
    assert!(
        body.contains(&format!("<AcceptableResponseSchema>{RESPONSE_SCHEMA}</AcceptableResponseSchema>"))
            && body.contains("<EMailAddress>ada@example.org</EMailAddress>"),
        "{body}"
    );
    // The debug output of the request and the machine holds neither address nor secret.
    for shown in [
        format!("{request:?}"),
        format!("{machine:?}"),
        format!("{:?}", Consent { host: "secret.example.net".into(), reason: ConsentReason::Redirect }),
    ] {
        assert!(!shown.contains("example") && !shown.contains("Basic") && !shown.contains("c3Rhbm"), "{shown}");
    }
    // An address with markup in it cannot be made.
    for bad in [
        "a<b>@example.org",
        "ada@exa mple.org",
        "ada@localhost",
        "@example.org",
        "ada",
        "ada@",
        "ada@-x.org",
        "ada@example..org",
        "ada@exämple.org",
        "a\r\nb@example.org",
    ] {
        assert!(Autodiscover::new(bad, Secret::new(BASIC)).is_err(), "{bad:?}");
    }
    assert!(Autodiscover::new(" Ada.Lovelace+tag@Example.ORG. ", Secret::new(BASIC)).is_ok());
}

#[test]
fn calls_out_of_turn_are_ignored() {
    let mut machine = search("ada@example.org");
    machine.response(Ok(HttpResponse::new(200, Vec::new())));
    machine.srv(Some(vec![("x.example.org".into(), 443)]));
    machine.agree();
    machine.decline();
    assert!(matches!(machine.step(), Step::Request(request) if request.url() == "https://example.org/autodiscover/autodiscover.xml"));
    machine.agree();
    machine.srv(None);
    assert!(matches!(machine.step(), Step::Request(_)));
    // A finished search stays finished.
    let standin = AutodiscoverStandIn::new().route("example.org", good());
    let mut machine = search("ada@example.org");
    let _ = block_on(machine.run(&standin, |_| None, |_| false));
    let before = standin.state().seen.len();
    machine.response(Ok(HttpResponse::new(404, Vec::new())));
    assert!(matches!(machine.step(), Step::Done(Outcome::Found(_))));
    assert_eq!(standin.state().seen.len(), before);
}

#[test]
fn the_domain_tests() {
    assert!(is_within("example.org", "example.org") && is_within("a.b.example.org", "example.org"));
    assert!(
        !is_within("evil-example.org", "example.org")
            && !is_within("example.org", "a.example.org")
            && !is_within("example.org.evil.net", "example.org")
    );
    assert_eq!(host_of("https://Mail.Example.org.:8443/x?y#z").as_deref(), Some("mail.example.org"));
    assert_eq!(host_of("https://[::1]:8443/x").as_deref(), Some("::1"));
    assert_eq!(host_of("http://example.org/"), None);
    assert_eq!(host_of("https:///x"), None);
    // A host that parsers read differently is no host.
    for odd in [
        "https://evil.net\\.example.org/x",
        "https://evil.net%2eexample.org/x",
        "https://exa mple.org/",
        "https://evil.net;.example.org/",
        "https://evil.net,.example.org/",
    ] {
        assert_eq!(host_of(odd), None, "{odd}");
    }
    assert!(check_endpoint("https://evil.net\\.example.org/x").is_err());
}

// ---- any answer, in any order ----

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_answers_are_never_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = read_answer(&bytes);
    }

    #[test]
    fn xml_shaped_text_is_never_a_panic(text in "(<[A-Za-z:]{0,10}( xmlns=\"[a-z:/.0-9]{0,20}\")?/?>|</[A-Za-z:]{0,10}>|[a-z &;#0-9]{0,8}|<!DOCTYPE|&[a-z]{0,5};){0,60}") {
        let _ = read_answer(text.as_bytes());
    }

    #[test]
    fn any_run_of_responses_ends_with_nothing_sent_outside_the_domain(
        responses in proptest::collection::vec((any::<u16>(), "[ -~]{0,60}", proptest::collection::vec(any::<u8>(), 0..200), 0u8..4), 0..40),
        srv in proptest::collection::vec("[ -~]{0,30}", 0..5),
    ) {
        let mut machine = search("ada@example.org");
        let mut feed = responses.into_iter();
        let mut done = false;
        for _ in 0..200 {
            match machine.step() {
                Step::Request(request) => {
                    // Only ever HTTPS, and only to the owner's domain: nobody has agreed to anything.
                    prop_assert!(request.url().starts_with("https://"));
                    let host = host_of(request.url()).unwrap_or_default();
                    prop_assert!(is_within(&host, "example.org"), "{host}");
                    match feed.next() {
                        Some((status, location, body, kind)) => {
                            let response = HttpResponse::new(status, body).with_field("Location", &location);
                            machine.response(if kind == 0 { Err(TransportError::Other) } else { Ok(response) });
                        }
                        None => machine.response(Err(TransportError::TimedOut)),
                    }
                }
                Step::LookupSrv(_) => machine.srv(Some(srv.iter().map(|host| (host.clone(), 443)).collect())),
                Step::NeedsYes(_) => machine.decline(),
                Step::Done(_) => { done = true; break; }
            }
        }
        prop_assert!(done);
    }
}

// ---- the sign-in goes only in answer to a challenge ----

#[test]
fn no_sign_in_goes_to_a_host_that_does_not_ask_for_one() {
    // The bare domain is a web host that sends visitors on to www; www is a site that answers
    // what it likes; only the autodiscover host is Exchange and asks for a sign-in.
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Redirect(301, "https://www.example.org/autodiscover/autodiscover.xml".into()))
        .route("www.example.org", Route::Bytes(b"<html>Welcome</html>".to_vec()))
        .route("autodiscover.example.org", good());
    let run = go(&standin, None, &[]);
    found(&run);
    assert_eq!(standin.hosts_asked(), ["example.org", "www.example.org", "autodiscover.example.org"]);
    // The password went to the one host that asked for it, and only after it asked.
    assert_eq!(standin.hosts_sent_credentials(), ["autodiscover.example.org"]);
    let state = standin.state();
    let first = state.seen.iter().find(|seen| seen.host == "autodiscover.example.org").unwrap();
    assert!(!first.credentials && first.status == 401);
    assert!(state.seen.last().unwrap().credentials);
}

#[test]
fn a_challenge_without_basic_sends_nothing_and_the_next_place_is_tried() {
    // NTLM only on the bare domain: a typed reason, no password, and autodiscover. still found.
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Challenge("NTLM".into())).route("autodiscover.example.org", good());
    found(&go(&standin, None, &[]));
    assert_eq!(standin.hosts_sent_credentials(), ["autodiscover.example.org"]);
    // NTLM or Negotiate everywhere: the search says that is the trouble.
    for (schemes, failure) in [
        ("NTLM", Failure::SignInNtlmOnly),
        ("Negotiate", Failure::SignInNotSupported),
        ("Negotiate, NTLM", Failure::SignInNtlmOnly),
        ("ntlm", Failure::SignInNtlmOnly),
    ] {
        let standin = AutodiscoverStandIn::new()
            .route("example.org", Route::Challenge(schemes.into()))
            .route("autodiscover.example.org", Route::Challenge(schemes.into()));
        let run = go(&standin, None, &[]);
        assert_eq!(run.outcome, Outcome::Failed(failure), "{schemes}");
        assert!(standin.hosts_sent_credentials().is_empty(), "{schemes}");
    }
    // Another scheme (a site behind a bearer token) is just not Autodiscover.
    let standin = AutodiscoverStandIn::new().route("example.org", Route::Challenge("Bearer".into())).route("autodiscover.example.org", good());
    found(&go(&standin, None, &[]));
    // Several offered, Basic among them: Basic is taken.
    let standin = AutodiscoverStandIn::new().route("example.org", good());
    let mut machine = search("ada@example.org");
    let Step::Request(request) = machine.step() else { panic!() };
    machine.response(Ok(HttpResponse::new(401, Vec::new())
        .with_field("WWW-Authenticate", "Negotiate")
        .with_field("WWW-Authenticate", "Basic realm=\"x\"")));
    let Step::Request(again) = machine.step() else { panic!("no second request") };
    assert_eq!((again.url(), again.field("authorization")), (request.url(), Some(BASIC)));
    let _ = standin;
}

#[test]
fn only_credentials_that_were_sent_and_refused_are_a_refused_sign_in() {
    // A staging site behind Basic on the bare domain is asked, sends the challenge, gets the sign-in
    // and refuses it: the search ends there rather than ask others with a password just refused.
    let standin =
        AutodiscoverStandIn::new().route("example.org", Route::Challenge("Basic realm=\"staging\"".into())).route("autodiscover.example.org", good());
    assert_eq!(go(&standin, None, &[]).outcome, Outcome::Failed(Failure::SignInRefused));
    assert_eq!(standin.hosts_asked(), ["example.org"]);
    // A 401 that wants no Basic never counted as a refusal (above); and a 401 to a request that
    // carried no sign-in is never one either.
    let mut machine = search("ada@example.org");
    let Step::Request(_) = machine.step() else { panic!() };
    machine.response(Ok(HttpResponse::new(401, Vec::new())));
    assert!(matches!(machine.step(), Step::Request(request) if request.url().contains("autodiscover.example.org")));
}

// ---- Microsoft's own mail ----

#[test]
fn microsofts_consumer_mail_is_exchange_online_and_nothing_is_sent() {
    for email in [
        "ada@hotmail.com",
        "ada@live.com",
        "ada@msn.com",
        "ada@outlook.de",
        "ada@hotmail.co.uk",
        "ada@live.com.au",
        "ada@outlook.com.br",
        "ada@hotmail.fr",
        "ada@Live.IT",
    ] {
        let standin = AutodiscoverStandIn::new().route("hotmail.com", good()).route("live.com", good());
        let mut machine = search(email);
        assert_eq!(machine.step(), Step::Done(Outcome::ExchangeOnline), "{email}");
        assert_eq!(go_with(&standin, search(email)).outcome, Outcome::ExchangeOnline, "{email}");
        assert!(standin.state().seen.is_empty(), "{email}");
    }
    // Not Microsoft's: companies whose names happen to start so.
    for email in [
        "ada@live.somecompany.example",
        "ada@live.abc.com",
        "ada@outlook.abc.com",
        "ada@outlook.ucs.de",
        "ada@live.bbc.com",
        "ada@msn.xy.org",
        "ada@mail.outlook.abc.com",
        "ada@msn.de",
        "ada@hotmail.org",
    ] {
        assert!(matches!(search(email).step(), Step::Request(_)), "{email}");
    }
    // Nor are their servers: an organisation's own server may be called outlook.something.
    let route =
        Route::Document(settings("https://outlook.abc.com/EWS/Exchange.asmx", "https://outlook.abc.com/EWS/Exchange.asmx", "outlook.abc.com"));
    let standin = AutodiscoverStandIn::new().route("abc.com", route);
    let run = run_with(&standin, search("ada@abc.com"), None, &[true]);
    assert_eq!(found(&run).urls(), ["https://outlook.abc.com/EWS/Exchange.asmx"]);
}

fn go_with(standin: &AutodiscoverStandIn, machine: Autodiscover) -> Run {
    run_with(standin, machine, None, &[])
}

#[test]
fn a_domain_whose_mail_exchangers_are_microsofts_is_exchange_online() {
    for host in [
        "example-org.mail.protection.outlook.com",
        "Example-Org.Mail.Protection.Outlook.com.",
        "mx1.olc.protection.outlook.com",
        "example-org.h-v1.mx.microsoft",
    ] {
        assert!(is_exchange_online_mx(host.trim_end_matches('.')), "{host}");
        let mut machine = search("ada@example.org");
        machine.note_mail_exchangers(&["mx.example.net".into(), host.into()]);
        assert_eq!(machine.step(), Step::Done(Outcome::ExchangeOnline), "{host}");
    }
    for host in ["mx.example.org", "mail.protection.outlook.com.evil.example", "alt1.aspmx.l.google.com", ""] {
        assert!(!is_exchange_online_mx(host), "{host}");
        let mut machine = search("ada@example.org");
        machine.note_mail_exchangers(&[host.into()]);
        assert!(matches!(machine.step(), Step::Request(_)), "{host}");
    }
    // Once the search has begun, a late word changes nothing.
    let mut machine = search("ada@example.org");
    let _ = machine.step();
    machine.note_mail_exchangers(&["x.mail.protection.outlook.com".into()]);
    assert!(matches!(machine.step(), Step::Request(_)));
}

// ---- the SRV port, the server's name, the redacted name ----

#[test]
fn an_srv_target_is_asked_at_its_port() {
    let standin = AutodiscoverStandIn::new().route("ad.example.org", good());
    let mut machine = search("ada@example.org");
    let mut asked = Vec::new();
    for _ in 0..10 {
        match machine.step() {
            Step::Request(request) => {
                asked.push(request.url().to_owned());
                machine.response(block_on(standin.exchange(&request)).unwrap());
            }
            Step::LookupSrv(name) => {
                assert_eq!(name.as_str(), "_autodiscover._tcp.example.org");
                // Ports that are neither 443 nor high are not asked at; 0 is the usual one.
                machine.srv(Some(vec![("ad.example.org".into(), 25), ("ad.example.org".into(), 8443), ("ad.example.org".into(), 0)]));
            }
            Step::NeedsYes(_) => machine.decline(),
            Step::Done(_) => break,
        }
    }
    assert!(asked.contains(&"https://ad.example.org:8443/autodiscover/autodiscover.xml".to_owned()), "{asked:?}");
    assert!(!asked.iter().any(|url| url.contains(":25/")), "{asked:?}");
}

#[test]
fn the_servers_name_is_not_a_place_the_password_goes() {
    // An internal server name outside the domain: no question, since nothing is sent to it.
    let route = Route::Document(settings(INTERNAL, EXTERNAL, "exch01.corp.local"));
    let standin = AutodiscoverStandIn::new().route("example.org", route);
    let run = go(&standin, None, &[]);
    assert_eq!(found(&run).server(), Some("exch01.corp.local"));
    assert!(run.asked.is_empty());
}

#[test]
fn the_srv_name_shows_nothing_in_debug_output() {
    let mut machine = search("ada@example.org");
    let step = loop {
        match machine.step() {
            Step::Request(_) => machine.response(Err(TransportError::Unreachable)),
            other => break other,
        }
    };
    let Step::LookupSrv(name) = &step else { panic!() };
    assert_eq!(name.as_str(), "_autodiscover._tcp.example.org");
    assert!(!format!("{step:?}").contains("example"), "{step:?}");
}

/// The shape Grommunio 2026 answers with (E8, first contact with a real EWS server, made-up
/// names): an `EXHTTP` and a MAPI/HTTP protocol first, then `EXCH` whose `Server` is a mailbox
/// identifier with an `@` in it (not a host name), then an `EXPR` with no `EwsUrl` at all.
#[test]
fn the_answer_of_grommunio_is_read_for_its_ews_address_and_a_server_name() {
    let body = r#"<?xml version="1.0" encoding="UTF-8"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006"><Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a"><User><AutoDiscoverSMTPAddress>ada@example.org</AutoDiscoverSMTPAddress><DisplayName>ada@example.org</DisplayName></User><Account><AccountType>email</AccountType><Action>settings</Action><MicrosoftOnline>False</MicrosoftOnline><ConsumerMailbox>False</ConsumerMailbox><Protocol><OOFUrl>https://mail.example.org/EWS/Exchange.asmx</OOFUrl><Type>EXHTTP</Type><Server>mail.example.org</Server><SSL>On</SSL><AuthPackage>basic</AuthPackage><EwsUrl>https://mail.example.org/EWS/Exchange.asmx</EwsUrl></Protocol><Protocol Type="mapiHttp" Version="1"><MailStore><InternalUrl>https://mail.example.org/mapi/emsmdb/?MailboxId=00000000-0000-0000-0000-000000000001@example.org</InternalUrl></MailStore></Protocol><Protocol><Type>EXCH</Type><Server>00000000-0000-0000-0000-000000000001@example.org</Server><AuthPackage>anonymous</AuthPackage><EwsUrl>https://mail.example.org/EWS/Exchange.asmx</EwsUrl></Protocol><Protocol><Type>EXPR</Type><Server>mail.example.org</Server><SSL>On</SSL><AuthPackage>basic</AuthPackage></Protocol></Account></Response></Autodiscover>"#;
    let Answer::Settings(found) = read_answer(body.as_bytes()) else { panic!("not settings") };
    assert_eq!(found.internal.as_deref(), Some("https://mail.example.org/EWS/Exchange.asmx"));
    assert_eq!(found.external, None);
    assert_eq!(found.server.as_deref(), Some("mail.example.org"));
}
