// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An address whose mail exchangers nobody here knows, at a domain whose DNS names a host for
//! Autodiscover: the owner is asked whether it is an Exchange server (a hoster's or the
//! organisation's own) or IMAP, and Microsoft's is Microsoft 365. Only the domain is asked about,
//! nothing is sent to the hosts named, and IMAP is one click away.

use super::*;
use crate::ews_setup::{EwsSetupEvent, EwsSetupJob};

/// A mail exchanger that is nobody's this program knows.
const HOSTED_MX: &str = "mxpool.hoster.example";
/// The host a hoster of Exchange servers names for its customers' Autodiscover.
const HOSTED_AUTODISCOVER: &str = "autodiscover.de2.hoster.example";

/// A host that looks a domain's Autodiscover records up: it answers `answer`, or nothing at all
/// for `None`. The log is what it was asked.
fn autodiscovers(app: &mut MailApp, answer: Option<Result<Vec<&str>, &str>>) -> Log<String> {
    let asked: Log<String> = Default::default();
    let log = asked.clone();
    let answer = answer.map(|a| a.map(|hosts| hosts.into_iter().map(str::to_string).collect::<Vec<_>>()).map_err(str::to_string));
    let silent: Log<mpsc::Sender<Result<Vec<String>, String>>> = Default::default();
    app.services.lookup_autodiscover = Some(Box::new(move |domain| {
        log.lock().unwrap().push(domain);
        let (tx, rx) = mpsc::channel();
        match &answer {
            Some(answer) => tx.send(answer.clone()).unwrap(),
            None => silent.lock().unwrap().push(tx),
        }
        rx
    }));
    asked
}

/// A host that sets Exchange server accounts up and never finishes: the log is each job's
/// address, server address and password.
fn ews_jobs(app: &mut MailApp) -> Log<(String, String, String)> {
    let asked: Log<(String, String, String)> = Default::default();
    let log = asked.clone();
    let running: Log<mpsc::Sender<EwsSetupEvent>> = Default::default();
    app.services.ews_setup = Some(Box::new(move |job: EwsSetupJob| {
        log.lock().unwrap().push((job.email.clone(), job.url.clone(), job.password.clone()));
        let (tx, rx) = mpsc::channel();
        running.lock().unwrap().push(tx);
        rx
    }));
    asked
}

/// The wizard after Connect for `address`, with the exchangers and the Autodiscover hosts the
/// host answers.
fn connected(
    mut a: MailApp,
    mx: Vec<&str>,
    autodiscover: Option<Result<Vec<&str>, &str>>,
    address: &str,
) -> (Harness<'static, MailApp>, Log<String>, Log<String>) {
    let mx = lookups(&mut a, Some(Ok(mx)));
    let ad = autodiscovers(&mut a, autodiscover);
    let mut h = open(a, json!({}));
    connect(&mut h, address);
    (h, mx, ad)
}

/// The owner says the server found is an Exchange server.
fn exchange_it_is(h: &mut Harness<'static, MailApp>) {
    assert_eq!(page(h), Some(Page::ExchangeOrImap));
    click(h, Kind::Exchange.title());
    assert_eq!(page(h), Some(Page::Exchange));
}

#[test]
fn a_hosted_exchange_server_is_found_from_the_domain_s_autodiscover_records() {
    let mut a = app(false);
    let checked = checks(&mut a, None);
    let jobs = ews_jobs(&mut a);
    let (mut h, mx, ad) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
    // Asked first, neither answer the default: a hoster's IMAP mail may publish Autodiscover too.
    assert_eq!(page(&h), Some(Page::ExchangeOrImap));
    assert!(says(&h, "lumen.example names a server for automatic setup (Autodiscover)"), "the page doesn't say what was found");
    assert!(h.query_by_role_and_label(Role::Button, Kind::Other.title()).is_some());
    exchange_it_is(&mut h);
    let w = wizard(&h).unwrap();
    assert!(w.exchange.found && !w.guessed && w.oauth.is_none() && w.email == "noor@lumen.example");
    // Only the domain was asked about, once each, and nothing was sent to any server.
    assert_eq!((mx.lock().unwrap().clone(), ad.lock().unwrap().clone()), (vec!["lumen.example".to_string()], vec!["lumen.example".to_string()]));
    assert!(checked.lock().unwrap().is_empty() && jobs.lock().unwrap().is_empty());
    // The page says what was found, and how to do without it.
    assert!(says(&h, "lumen.example names a server for Exchange's automatic setup"), "the page doesn't say what was found");
    assert!(h.query_by_role_and_label(Role::Button, "Not an Exchange server? Set it up with IMAP").is_some());
    // Connect hands the search to the host, with the address, no server, and the password.
    type_password(&mut h);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::ExchangeChecking));
    assert_eq!(jobs.lock().unwrap().clone(), vec![("noor@lumen.example".to_string(), String::new(), TYPED.to_string())]);
    assert!(checked.lock().unwrap().is_empty());
}

#[test]
fn back_from_a_found_exchange_server_is_the_question_and_imap_is_the_guess_as_it_was() {
    let mut a = app(false);
    checks(&mut a, None);
    let (mut h, _, _) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
    exchange_it_is(&mut h);
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::ExchangeOrImap));
    assert_eq!(wizard(&h).map(|w| w.exchange.hosts.clone()), Some(vec![HOSTED_AUTODISCOVER.to_string()]), "what was found is kept");
    exchange_it_is(&mut h);
    assert!(wizard(&h).is_some_and(|w| w.exchange.found && w.exchange.hosts == [HOSTED_AUTODISCOVER]));
    click(&mut h, "Back");
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Address));

    // IMAP from the question, and from the Exchange page: the guess, as it was.
    for from_exchange in [false, true] {
        let mut a = app(false);
        checks(&mut a, None);
        let (mut h, _, _) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
        if from_exchange {
            exchange_it_is(&mut h);
            type_password(&mut h);
            click(&mut h, "Not an Exchange server? Set it up with IMAP");
        } else {
            click(&mut h, Kind::Other.title());
        }
        assert_eq!(page(&h), Some(Page::Password), "{from_exchange}");
        let w = wizard(&h).unwrap();
        assert!(w.guessed && w.settings_open && !w.exchange.found && w.password.0.is_empty());
        assert_eq!((w.incoming.host.as_str(), w.outgoing.host.as_str()), ("imap.lumen.example", "smtp.lumen.example"));
        assert!(says(&h, "only a guess"));
        // The Exchange server is still one tile away.
        assert!(h.query_by_role_and_label(Role::Button, Kind::Exchange.title()).is_some());
        click(&mut h, "Back");
        assert_eq!(page(&h), Some(Page::Address));
    }
}

#[test]
fn microsoft_s_autodiscover_behind_another_mail_exchanger_is_microsoft_365() {
    let mut a = app(false);
    a.services.microsoft_client_id = APP_ID.into();
    let signed = sign_ins(&mut a, None);
    let jobs = ews_jobs(&mut a);
    let (h, _, _) = connected(a, vec!["filter.spamguard.example"], Some(Ok(vec!["autodiscover.outlook.com"])), "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::SignIn));
    let w = wizard(&h).unwrap();
    assert_eq!((w.service.as_str(), w.oauth.as_ref().map(|o| o.provider)), ("Microsoft 365", Some(OAuthProvider::Microsoft)));
    assert_eq!(signed.lock().unwrap().first().map(|(email, _, _)| email.clone()), Some("noor@lumen.example".to_string()));
    assert!(jobs.lock().unwrap().is_empty());
}

#[test]
fn no_autodiscover_or_imap_autodiscover_or_no_answer_keeps_the_guess() {
    for answer in [Ok(vec![]), Ok(vec!["cpanelemaildiscovery.cpanel.net"]), Err("the name server couldn't be asked")] {
        let mut a = app(false);
        checks(&mut a, None);
        let (h, _, _) = connected(a, vec![HOSTED_MX], Some(answer.clone()), "noor@lumen.example");
        assert_eq!(page(&h), Some(Page::Password), "{answer:?}");
        assert!(wizard(&h).is_some_and(|w| w.guessed && !w.exchange.found), "{answer:?}");
    }
}

#[test]
fn a_provider_the_exchangers_name_wins_over_autodiscover() {
    let mut a = app(false);
    checks(&mut a, None);
    let (h, _, _) = connected(a, vec!["aspmx.l.google.com"], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    let w = wizard(&h).unwrap();
    assert!(!w.exchange.found && w.service == "Google Workspace", "{}", w.service);
}

#[test]
fn the_wizard_waits_for_both_answers_and_back_drops_them() {
    let mut a = app(false);
    checks(&mut a, None);
    let (mut h, _, ad) = connected(a, vec![HOSTED_MX], None, "noor@lumen.example");
    // The exchangers are in; the Autodiscover records aren't yet.
    assert_eq!((page(&h), ad.lock().unwrap().len()), (Some(Page::Detecting), 1));
    click(&mut h, "Cancel");
    assert_eq!(page(&h), Some(Page::Address));
    assert!(wizard(&h).is_some_and(|w| w.lookup.is_none() && w.autodiscover.is_none()));
}

#[test]
fn working_offline_nobody_is_asked() {
    let mut a = app(false);
    a.session.prefs.offline = true;
    checks(&mut a, None);
    let (h, mx, ad) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(mx.lock().unwrap().is_empty() && ad.lock().unwrap().is_empty());
}

#[test]
fn a_known_domain_asks_nobody() {
    let mut a = app(false);
    checks(&mut a, None);
    let (h, mx, ad) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@gmail.com");
    assert_ne!(page(&h), Some(Page::Exchange));
    assert!(mx.lock().unwrap().is_empty() && ad.lock().unwrap().is_empty());
}

#[test]
fn server_settings_show_where_automatic_setup_was_found_and_imap_is_offered_below_them() {
    let mut a = app(false);
    checks(&mut a, None);
    let (mut h, _, _) = connected(a, vec![HOSTED_MX], Some(Ok(vec![HOSTED_AUTODISCOVER])), "noor@lumen.example");
    exchange_it_is(&mut h);
    assert_eq!(wizard(&h).map(|w| w.exchange.hosts.clone()), Some(vec![HOSTED_AUTODISCOVER.to_string()]));
    assert!(!says(&h, "https://autodiscover.de2.hoster.example/autodiscover/autodiscover.xml"));
    click(&mut h, "Server settings");
    assert!(says(&h, "https://autodiscover.de2.hoster.example/autodiscover/autodiscover.xml"));
    // The way to IMAP comes after the settings.
    let settings = h.get_by_role_and_label(Role::Button, "Search again").rect().min.y;
    let imap = h.get_by_role_and_label(Role::Button, "Not an Exchange server? Set it up with IMAP").rect().min.y;
    assert!(imap > settings, "{imap} <= {settings}");
}

#[test]
fn search_again_asks_the_dns_once_more_and_shows_what_it_says() {
    let mut a = app(false);
    checks(&mut a, None);
    lookups(&mut a, Some(Ok(vec![HOSTED_MX])));
    // The first look finds the hoster; the second finds nothing.
    let answers: Log<Vec<String>> = Arc::new(Mutex::new(vec![vec![], vec![HOSTED_AUTODISCOVER.to_string()]]));
    let asked: Log<String> = Default::default();
    let log = asked.clone();
    a.services.lookup_autodiscover = Some(Box::new(move |domain| {
        log.lock().unwrap().push(domain);
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(answers.lock().unwrap().pop().unwrap_or_default())).unwrap();
        rx
    }));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    exchange_it_is(&mut h);
    click(&mut h, "Server settings");
    click(&mut h, "Search again");
    h.run_steps(3);
    assert_eq!(page(&h), Some(Page::Exchange));
    assert_eq!(asked.lock().unwrap().clone(), vec!["lumen.example".to_string(), "lumen.example".to_string()]);
    let w = wizard(&h).unwrap();
    assert!(w.exchange.hosts.is_empty() && w.autodiscover.is_none());
    assert!(says(&h, "names no server for automatic setup"));
    // What the page was opened for no longer said, and IMAP still one click away.
    assert!(!says(&h, "so this looks like an Exchange server account"));
    assert!(says(&h, "Not an Exchange server? Set it up with IMAP"));
    // Back: not to the question, which would say the domain names a server.
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Address));
}

#[test]
fn search_again_that_finds_microsoft_says_it_is_a_microsoft_account() {
    let mut a = app(false);
    checks(&mut a, None);
    lookups(&mut a, Some(Ok(vec![HOSTED_MX])));
    let answers: Log<Vec<String>> = Arc::new(Mutex::new(vec![vec!["autodiscover.outlook.com".to_string()], vec![HOSTED_AUTODISCOVER.to_string()]]));
    a.services.lookup_autodiscover = Some(Box::new(move |_| {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(answers.lock().unwrap().pop().unwrap_or_default())).unwrap();
        rx
    }));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    exchange_it_is(&mut h);
    type_password(&mut h);
    click(&mut h, "Server settings");
    click(&mut h, "Search again");
    h.run_steps(3);
    assert_eq!(page(&h), Some(Page::ExchangeOnline));
    assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()));
    // Back: the Exchange page no longer says what the first look found.
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(wizard(&h).is_some_and(|w| !w.exchange.found));
    assert!(!says(&h, "so this looks like an Exchange server account"));
}

#[test]
fn a_search_again_still_out_is_dropped_when_the_exchange_page_is_left() {
    let mut a = app(false);
    checks(&mut a, None);
    lookups(&mut a, Some(Ok(vec![HOSTED_MX])));
    // The first look answers at once; the second never does while the page is open.
    let held: Log<mpsc::Sender<Result<Vec<String>, String>>> = Default::default();
    let (keep, looks) = (held.clone(), Arc::new(Mutex::new(0)));
    a.services.lookup_autodiscover = Some(Box::new(move |_| {
        let (tx, rx) = mpsc::channel();
        let mut n = looks.lock().unwrap();
        *n += 1;
        if *n == 1 {
            tx.send(Ok(vec![HOSTED_AUTODISCOVER.to_string()])).unwrap();
        }
        keep.lock().unwrap().push(tx);
        rx
    }));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    exchange_it_is(&mut h);
    click(&mut h, "Server settings");
    click(&mut h, "Search again");
    assert!(wizard(&h).is_some_and(|w| w.autodiscover.is_some()));
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::ExchangeOrImap));
    assert!(wizard(&h).is_some_and(|w| w.autodiscover.is_none() && !w.busy()), "nothing left waiting");
    // Its answer, coming late, has nowhere to go.
    for tx in held.lock().unwrap().iter() {
        let _ = tx.send(Ok(vec!["autodiscover.outlook.com".to_string()]));
    }
    h.run_steps(3);
    assert_eq!(page(&h), Some(Page::ExchangeOrImap));
}

#[test]
fn imap_after_the_question_takes_the_servers_the_domain_s_own_records_name() {
    use mailcraft_model::provider::{FoundServer, Via};
    let mut a = app(false);
    checks(&mut a, None);
    // The domain names an Autodiscover host and, in its RFC 6186 records, its IMAP and SMTP servers.
    let server = |host: &str, port, security| Some(FoundServer { server: Server { host: host.into(), port, security }, via: Via::Srv });
    let discovered = Discovered {
        exchangers: vec![HOSTED_MX.into()],
        incoming: server("imap.lumen.example", 993, Security::Tls),
        outgoing: server("smtp.lumen.example", 465, Security::Tls),
        username: String::new(),
    };
    discoveries(&mut a, Some(Ok(discovered)));
    autodiscovers(&mut a, Some(Ok(vec![HOSTED_AUTODISCOVER])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    // Asked first: the Autodiscover host may be an Exchange server's.
    assert_eq!(page(&h), Some(Page::ExchangeOrImap));
    click(&mut h, Kind::Other.title());
    assert_eq!(page(&h), Some(Page::Password));
    let w = wizard(&h).unwrap();
    // The records' servers, said to be found, and not the guess.
    assert_eq!((w.found, w.guessed), (Some(Via::Srv), false));
    assert_eq!((w.incoming.host.as_str(), w.outgoing.host.as_str()), ("imap.lumen.example", "smtp.lumen.example"));
    // Without an Autodiscover host the records' servers are taken as before, with no question.
    let mut a = app(false);
    checks(&mut a, None);
    let discovered =
        Discovered { exchangers: vec![HOSTED_MX.into()], incoming: server("imap.lumen.example", 993, Security::Tls), ..Default::default() };
    discoveries(&mut a, Some(Ok(discovered)));
    autodiscovers(&mut a, Some(Ok(vec![])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(wizard(&h).and_then(|w| w.found), Some(Via::Srv));
}
