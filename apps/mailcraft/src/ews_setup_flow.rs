// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of Add Account for an Exchange server account (#252, E7): the search through
//! the strict stand-in for Autodiscover, and the test of the account through the host's own
//! transport against a loopback server that answers with the strict stand-in for Exchange Web
//! Services behind Basic, NTLM or Kerberos alone. Nothing leaves the machine; every name, address
//! and fingerprint is invented.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_ews::standin::block_on;
use mailcraft_ews::standin_autodiscover::{AutodiscoverStandIn, Route, settings};
use mailcraft_ews::{HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};
use mailcraft_net::{CertificateFault, CertificateInfo, CertificateProblem};
use mailcraft_ui_egui::ews_setup::{EwsConsentReason, EwsSetupEnd, EwsSetupEvent, EwsSetupFailure, EwsSetupJob};

use crate::ews_setup::{Probed, Who, World, setup_with};
use crate::ews_up_flow::{Mode, PASSWORD, USER, serve};

const ME: &str = "ada@example.org";
const EWS: &str = "https://mail.example.org/EWS/Exchange.asmx";
const INTERNAL: &str = "https://exch01.corp.example.org/EWS/Exchange.asmx";

/// The world of one test: Autodiscover's stand-in, what the DNS says, and how the account's test
/// goes: scripted, or the host's own transport at a loopback server that stands in for `EWS`.
struct Fake {
    autodiscover: AutodiscoverStandIn,
    exchangers: Vec<String>,
    srv: Option<Vec<(String, u16)>>,
    scripted: Mutex<Option<Probed>>,
    /// Where the account is really tested.
    loopback: Option<String>,
    /// What the test of the account was asked: the address, whether a password was in the job, and
    /// the fingerprint trusted.
    probes: Mutex<Vec<(String, bool, String)>>,
    lookups: Mutex<Vec<String>>,
}

impl Fake {
    fn new(autodiscover: AutodiscoverStandIn) -> Fake {
        Fake {
            autodiscover,
            exchangers: Vec::new(),
            srv: None,
            scripted: Mutex::new(None),
            loopback: None,
            probes: Mutex::new(Vec::new()),
            lookups: Mutex::new(Vec::new()),
        }
    }

    fn scripted(self, probed: Probed) -> Fake {
        *self.scripted.lock().unwrap() = Some(probed);
        self
    }
}

impl World for Fake {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        block_on(self.autodiscover.exchange(request)).unwrap()
    }

    fn mail_exchangers(&self, domain: &str) -> Vec<String> {
        self.lookups.lock().unwrap().push(domain.to_owned());
        self.exchangers.clone()
    }

    fn srv(&self, _name: &str) -> Option<Vec<(String, u16)>> {
        self.srv.clone()
    }

    fn probe(&self, url: &str, who: &Who, trusted: &str) -> Probed {
        self.probes.lock().unwrap().push((url.to_owned(), !who.password.is_empty(), trusted.to_owned()));
        if let Some(probed) = self.scripted.lock().unwrap().take() {
            return probed;
        }
        match &self.loopback {
            Some(loopback) => crate::ews_setup::Real.probe(loopback, who, trusted),
            None => Probed::Failed(EwsSetupFailure::Unreachable),
        }
    }

    fn look(&self, url: &str, trusted: &str) -> Probed {
        self.probes.lock().unwrap().push((url.to_owned(), false, trusted.to_owned()));
        self.scripted.lock().unwrap().take().unwrap_or(Probed::Failed(EwsSetupFailure::Unreachable))
    }
}

fn info(fingerprint: &str) -> CertificateInfo {
    CertificateInfo {
        fingerprint: fingerprint.into(),
        subject: "mail.example.org".into(),
        issuer: "An Invented CA".into(),
        not_before: 1_700_000_000,
        not_after: 1_900_000_000,
    }
}

fn make_job(email: &str, username: &str, password: &str, url: &str) -> (EwsSetupJob, Sender<bool>) {
    let (tx, answers) = channel();
    let job = EwsSetupJob {
        email: email.into(),
        username: username.into(),
        password: password.into(),
        url: url.into(),
        trusted: String::new(),
        certificate_only: false,
        cancel: Arc::new(AtomicBool::new(false)),
        answers,
    };
    (job, tx)
}

/// The Autodiscover stand-in's user: its `BASIC` is "standin" and "password".
fn standin_job(url: &str) -> (EwsSetupJob, Sender<bool>) {
    make_job(ME, "standin", "password", url)
}

/// Runs a job to its end on this thread; what it told, in order.
fn run(world: &Fake, job: &EwsSetupJob) -> Vec<EwsSetupEvent> {
    let mut told = Vec::new();
    setup_with(world, job, &mut |event| told.push(event));
    told
}

fn ended(told: &[EwsSetupEvent]) -> EwsSetupEnd {
    match told.last() {
        Some(EwsSetupEvent::Finished(end)) => end.clone(),
        other => panic!("not finished: {other:?}"),
    }
}

fn document() -> Route {
    Route::Document(settings(INTERNAL, EWS, "exch01.corp.example.org"))
}

/// A loopback server that stands in for the Exchange server, with the sign-in Autodiscover's
/// stand-in takes.
fn server(mode: Mode) -> crate::ews_up_flow::Server {
    let server = serve(mode);
    if mode == Mode::Basic {
        server.standin.state().authorization = Secret::basic("standin", "password").unwrap().expose().to_owned();
    }
    server
}

#[test]
fn the_server_is_found_from_the_address_and_the_account_is_tried_at_it() {
    let standin = AutodiscoverStandIn::new().route("example.org", document());
    let loopback = server(Mode::Basic);
    let mut world = Fake::new(standin);
    world.loopback = Some(loopback.url.clone());
    let (job, _) = standin_job("");
    let told = run(&world, &job);
    // The external address is tried first (Outlook Anywhere's, which any network reaches).
    assert_eq!(ended(&told), EwsSetupEnd::Works { url: EWS.into(), signed_in_with: "basic".into() });
    assert_eq!(told.len(), 1, "no question was asked");
    assert_eq!(world.lookups.lock().unwrap().as_slice(), ["example.org"], "only the domain goes to the DNS");
    // The sign-in went to the owner's own domain only.
    assert_eq!(world.autodiscover.hosts_sent_credentials(), ["example.org"]);
    assert_eq!(world.probes.lock().unwrap().as_slice(), [(EWS.to_owned(), true, String::new())]);
}

#[test]
fn a_typed_server_skips_autodiscover_and_the_dns_altogether() {
    let loopback = server(Mode::Basic);
    let world = Fake::new(AutodiscoverStandIn::new());
    let (job, _) = standin_job(&loopback.url);
    let mut world = world;
    world.loopback = Some(loopback.url.clone());
    let told = run(&world, &job);
    assert!(matches!(ended(&told), EwsSetupEnd::Works { ref signed_in_with, .. } if signed_in_with == "basic"));
    assert!(world.autodiscover.state().seen.is_empty(), "nothing was asked of the domain");
    assert!(world.lookups.lock().unwrap().is_empty() && world.autodiscover.hosts_asked().is_empty());
}

#[test]
fn the_account_is_tried_through_the_hosts_own_transport_with_ntlm_basic_or_neither() {
    // NTLM: chosen from the server's offer, the password never sent in Basic.
    let ntlm = server(Mode::Ntlm);
    let world = Fake::new(AutodiscoverStandIn::new());
    let (job, _) = make_job(ME, USER, PASSWORD, &ntlm.url);
    let probed = crate::ews_setup::Real.probe(&ntlm.url, &Who { email: ME.into(), username: USER.into(), password: PASSWORD.into() }, "");
    assert_eq!(probed, Probed::Works("ntlm"));
    assert!(ntlm.schemes.lock().unwrap().iter().all(|s| s.is_empty() || s == "ntlm"));
    // A wrong password is a refusal, not a mystery.
    let probed = crate::ews_setup::Real.probe(&ntlm.url, &Who { email: ME.into(), username: USER.into(), password: "wrong".into() }, "");
    assert_eq!(probed, Probed::Failed(EwsSetupFailure::SignInRefused));
    let basic = server(Mode::Basic);
    let probed = crate::ews_setup::Real.probe(&basic.url, &Who { email: ME.into(), username: "standin".into(), password: "wrong".into() }, "");
    assert_eq!(probed, Probed::Failed(EwsSetupFailure::SignInRefused));
    // Kerberos alone is said to be unsupported, in words, and is no wrong password.
    let kerberos = server(Mode::Kerberos);
    let probed = crate::ews_setup::Real.probe(&kerberos.url, &Who { email: ME.into(), username: USER.into(), password: PASSWORD.into() }, "");
    assert_eq!(probed, Probed::Failed(EwsSetupFailure::SignInNotSupported));
    drop((world, job));
}

#[test]
fn an_address_that_nobody_answers_at_is_unreachable() {
    let world = Fake::new(AutodiscoverStandIn::new());
    let (job, _) = standin_job("");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::Failed(EwsSetupFailure::Unreachable));
    assert!(world.probes.lock().unwrap().is_empty());
}

#[test]
fn a_host_outside_the_domain_is_asked_about_and_gets_no_password_before_a_yes() {
    for yes in [true, false] {
        let standin = AutodiscoverStandIn::new()
            .route("example.org", Route::Redirect(302, "https://autodiscover.other.example/autodiscover/autodiscover.xml".into()))
            .route("autodiscover.other.example", document());
        let mut world = Fake::new(standin).scripted(Probed::Works("basic"));
        world.srv = None;
        let (job, answers) = standin_job("");
        let (events, seen): (Sender<EwsSetupEvent>, Receiver<EwsSetupEvent>) = channel();
        let (told, hosts_at_the_question) = std::thread::scope(|scope| {
            let world = &world;
            scope.spawn(move || setup_with(world, &job, &mut |event| events.send(event).unwrap()));
            let first = seen.recv_timeout(Duration::from_secs(10)).expect("the owner is asked");
            // The question is out and nothing is answered: what was sent where, so far.
            let credentials = world.autodiscover.hosts_sent_credentials();
            answers.send(yes).unwrap();
            let mut told = vec![first];
            while let Ok(event) = seen.recv_timeout(Duration::from_secs(10)) {
                let done = matches!(event, EwsSetupEvent::Finished(_));
                told.push(event);
                if done {
                    break;
                }
            }
            (told, credentials)
        });
        assert_eq!(told[0], EwsSetupEvent::Consent { host: "autodiscover.other.example".into(), reason: EwsConsentReason::Redirect });
        assert!(!hosts_at_the_question.iter().any(|host| host == "autodiscover.other.example"), "{hosts_at_the_question:?}");
        if yes {
            assert!(matches!(ended(&told), EwsSetupEnd::Works { .. }), "{told:?}");
        } else {
            assert!(!world.autodiscover.hosts_sent_credentials().iter().any(|host| host.ends_with("other.example")));
            assert!(matches!(ended(&told), EwsSetupEnd::Failed(_)), "{told:?}");
            assert!(world.probes.lock().unwrap().is_empty(), "no account test: nothing is sent to a host that was declined");
        }
    }
}

#[test]
fn a_giving_up_while_the_question_is_open_ends_the_job_quietly() {
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Redirect(302, "https://autodiscover.other.example/autodiscover/autodiscover.xml".into()))
        .route("autodiscover.other.example", document());
    let world = Fake::new(standin);
    let (job, _answers) = standin_job("");
    let told: Arc<Mutex<Vec<EwsSetupEvent>>> = Default::default();
    let cancel = job.cancel.clone();
    std::thread::scope(|scope| {
        let (world, told) = (&world, told.clone());
        let seen = told.clone();
        let handle = scope.spawn(move || setup_with(world, &job, &mut |event| told.lock().unwrap().push(event)));
        for _ in 0..1_000 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    });
    let told = told.lock().unwrap();
    assert_eq!(told.len(), 1, "the question, and no ending");
    assert!(matches!(told[0], EwsSetupEvent::Consent { .. }));
    assert!(world.probes.lock().unwrap().is_empty());
}

#[test]
fn a_mailbox_in_microsoft_365_stops_the_search_and_nothing_is_sent_to_anyone() {
    // By the domain's mail exchangers, before anything is asked.
    let mut world = Fake::new(AutodiscoverStandIn::new().route("example.org", document()));
    world.exchangers = vec!["example-org.mail.protection.outlook.com".into()];
    let (job, _) = standin_job("");
    let told = run(&world, &job);
    assert_eq!(ended(&told), EwsSetupEnd::ExchangeOnline);
    assert!(world.autodiscover.state().seen.is_empty(), "nothing was asked of the domain");
    assert!(world.probes.lock().unwrap().is_empty());
    // By Autodiscover's own redirect to Microsoft's host: asked nothing there.
    let standin = AutodiscoverStandIn::new()
        .route("example.org", Route::Redirect(302, "https://autodiscover.outlook.com/autodiscover/autodiscover.xml".into()))
        .route("autodiscover.outlook.com", document());
    let world = Fake::new(standin);
    let (job, _) = standin_job("");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::ExchangeOnline);
    assert!(!world.autodiscover.hosts_asked().iter().any(|host| host.ends_with("outlook.com")));
    assert!(world.probes.lock().unwrap().is_empty());
    // And by the address itself.
    let world = Fake::new(AutodiscoverStandIn::new());
    let (job, _) = job_for("ada@outlook.com");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::ExchangeOnline);
    assert!(world.autodiscover.state().seen.is_empty());
}

fn job_for(email: &str) -> (EwsSetupJob, Sender<bool>) {
    make_job(email, "standin", "password", "")
}

#[test]
fn nothing_is_found_and_the_reason_is_a_kind_not_a_server_s_words() {
    for (route, said) in [
        (Route::Status(404), EwsSetupFailure::NoAutodiscover),
        (Route::TlsFails, EwsSetupFailure::SecureConnection),
        (Route::Bytes(b"not xml <<<".to_vec()), EwsSetupFailure::Unreadable),
        (Route::Challenge("Basic realm=\"x\"".into()), EwsSetupFailure::SignInRefused),
    ] {
        let world = Fake::new(AutodiscoverStandIn::new().route("example.org", route.clone()).route("autodiscover.example.org", route));
        let (job, _) = standin_job("");
        let end = ended(&run(&world, &job));
        let EwsSetupEnd::Failed(failure) = end else { panic!("{end:?}") };
        // (A host that only says no is "no automatic setup"; what is more telling wins.)
        assert!(failure == said || failure == EwsSetupFailure::NoAutodiscover, "{said:?}: {failure:?}");
    }
    // A bad address can't be looked for.
    let world = Fake::new(AutodiscoverStandIn::new());
    let (job, _) = job_for("not an address");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::Failed(EwsSetupFailure::BadSettings));
    // A password that can't be sent (a control character) is said before any request.
    let (job, _) = make_job(ME, "standin", "pass\nword", "");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::Failed(EwsSetupFailure::BadSettings));
    assert!(world.autodiscover.state().seen.is_empty());
}

#[test]
fn a_certificate_the_system_does_not_trust_ends_the_test_with_the_question_and_nothing_else() {
    for (problem, changed) in
        [(CertificateProblem::NotTrusted(info(&"ab".repeat(32))), false), (CertificateProblem::Changed(info(&"cd".repeat(32))), true)]
    {
        let world = Fake::new(AutodiscoverStandIn::new()).scripted(Probed::Certificate(problem.clone()));
        let (job, _) = standin_job(EWS);
        let end = ended(&run(&world, &job));
        let EwsSetupEnd::Certificate(question) = end else { panic!("{end:?}") };
        assert_eq!((question.url.as_str(), question.changed), (EWS, changed));
        assert_eq!(question.fingerprint, problem.info().fingerprint);
        assert_eq!((question.subject.as_str(), question.issuer.as_str()), ("mail.example.org", "An Invented CA"));
        assert_eq!((question.not_before, question.not_after), (1_700_000_000, 1_900_000_000));
    }
}

#[test]
fn a_certificate_nobody_may_trust_is_told_in_words_and_never_offered() {
    for (fault, said) in [
        (CertificateFault::Expired, mailcraft_ui_egui::ews_setup::EwsCertificateFault::Expired),
        (CertificateFault::NotYetValid, mailcraft_ui_egui::ews_setup::EwsCertificateFault::NotYetValid),
        (CertificateFault::WrongName, mailcraft_ui_egui::ews_setup::EwsCertificateFault::WrongName),
    ] {
        let world =
            Fake::new(AutodiscoverStandIn::new()).scripted(Probed::Certificate(CertificateProblem::Unacceptable(fault, info(&"ab".repeat(32)))));
        let (job, _) = standin_job(EWS);
        assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::Failed(EwsSetupFailure::Certificate(said)));
    }
}

#[test]
fn the_fingerprint_the_owner_agreed_to_goes_with_the_next_test_and_the_certificate_alone_can_be_looked_at() {
    let world = Fake::new(AutodiscoverStandIn::new()).scripted(Probed::Works("basic"));
    let (mut job, _) = standin_job(EWS);
    job.trusted = "ab".repeat(32);
    assert!(matches!(ended(&run(&world, &job)), EwsSetupEnd::Works { .. }));
    assert_eq!(world.probes.lock().unwrap().as_slice(), [(EWS.to_owned(), true, "ab".repeat(32))]);
    // A look at the certificate sends no password and signs nobody in.
    let world = Fake::new(AutodiscoverStandIn::new()).scripted(Probed::Certificate(CertificateProblem::Changed(info(&"cd".repeat(32)))));
    let (mut job, _) = make_job(ME, "", "", EWS);
    (job.certificate_only, job.trusted) = (true, "ab".repeat(32));
    let end = ended(&run(&world, &job));
    assert!(matches!(end, EwsSetupEnd::Certificate(ref q) if q.changed && q.fingerprint == "cd".repeat(32)));
    assert_eq!(world.probes.lock().unwrap().as_slice(), [(EWS.to_owned(), false, "ab".repeat(32))]);
    // Fine now.
    let world = Fake::new(AutodiscoverStandIn::new()).scripted(Probed::Works("basic"));
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::CertificateFine);
}

#[test]
fn a_look_at_a_loopback_server_finds_nothing_to_ask() {
    let loopback = server(Mode::Basic);
    assert_eq!(crate::ews_setup::Real.look(&loopback.url, ""), Probed::Works("basic"));
    assert!(matches!(crate::ews_setup::Real.look("not an address", ""), Probed::Failed(EwsSetupFailure::BadSettings)));
    // A server that isn't there: no one answers.
    assert!(matches!(crate::ews_setup::Real.look("http://127.0.0.1:1/EWS/Exchange.asmx", ""), Probed::Failed(_)));
}

#[test]
fn the_first_address_that_answers_decides_and_a_second_is_tried_only_when_nobody_answered_at_the_first() {
    let mut world = Fake::new(AutodiscoverStandIn::new().route("example.org", document()));
    // Both addresses are tried when the first isn't there: here neither is.
    let (job, _) = standin_job("");
    let end = ended(&run(&world, &job));
    assert_eq!(end, EwsSetupEnd::Failed(EwsSetupFailure::Unreachable));
    assert_eq!(world.probes.lock().unwrap().iter().map(|p| p.0.clone()).collect::<Vec<_>>(), [EWS, INTERNAL]);
    // A refusal of the password ends it at the first: the second would be asked with the same one.
    world = Fake::new(AutodiscoverStandIn::new().route("example.org", document())).scripted(Probed::Failed(EwsSetupFailure::SignInRefused));
    let (job, _) = standin_job("");
    assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::Failed(EwsSetupFailure::SignInRefused));
    assert_eq!(world.probes.lock().unwrap().len(), 1);
}

#[test]
fn a_run_says_how_it_signed_in_for_file_info() {
    for (mode, want) in [(Mode::Ntlm, "ntlm"), (Mode::Basic, "basic")] {
        let server = serve(mode);
        let request =
            mailcraft_model::sync::EwsMailRequest { url: server.url.clone(), email: ME.into(), username: USER.into(), ..Default::default() };
        let mut pieces = Vec::new();
        crate::ews_runner::run_mail_with(&request, PASSWORD, &AtomicBool::new(false), &mut |piece| pieces.push(piece));
        let last = pieces.last().expect("the run ended");
        assert_eq!((last.partial, last.signed_in_with.as_str()), (false, want));
        assert!(last.check().is_ok(), "the piece is one the engine takes in");
    }
}

#[test]
fn a_typed_address_on_a_microsoft_host_is_handed_over_and_nothing_is_sent() {
    for url in [
        "https://outlook.office365.com/EWS/Exchange.asmx",
        "https://outlook.office.com/EWS/Exchange.asmx",
        "https://OUTLOOK.Office365.com./EWS/Exchange.asmx",
        "https://mail.outlook.com/EWS/Exchange.asmx",
    ] {
        let world = Fake::new(AutodiscoverStandIn::new());
        let (job, _) = make_job(ME, USER, PASSWORD, url);
        assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::ExchangeOnline, "{url}");
        assert!(world.probes.lock().unwrap().is_empty(), "no sign-in went to {url}");
        assert!(world.autodiscover.state().seen.is_empty() && world.lookups.lock().unwrap().is_empty(), "{url}");
        // The certificate review, which signs nobody in, is no way round it either.
        let (mut job, _) = make_job(ME, USER, "", url);
        job.certificate_only = true;
        assert_eq!(ended(&run(&world, &job)), EwsSetupEnd::ExchangeOnline, "{url}");
    }
    // An organisation's own server that merely looks alike is its own.
    assert!(!crate::ews_setup::on_microsoft("https://outlook.abc.com/EWS/Exchange.asmx"));
    assert!(!crate::ews_setup::on_microsoft("https://evil-office365.com/EWS/Exchange.asmx"));
}

#[test]
fn a_run_for_an_account_on_a_microsoft_host_is_refused_before_any_request() {
    let request = mailcraft_model::sync::EwsMailRequest {
        url: "https://outlook.office365.com/EWS/Exchange.asmx".into(),
        email: ME.into(),
        ..Default::default()
    };
    let mut pieces = Vec::new();
    crate::ews_runner::run_mail_with(&request, PASSWORD, &AtomicBool::new(false), &mut |piece| pieces.push(piece));
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].errors.iter().any(|line| line.contains("Microsoft")), "{:?}", pieces[0].errors);
    assert!(crate::ews_runner::client("https://outlook.office365.com/EWS/Exchange.asmx", "ada", ME, PASSWORD).is_err());
}
