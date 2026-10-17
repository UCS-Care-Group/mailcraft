//! An organisation that has to approve MailCraft first, the host's part of it: how a sign-in
//! on Microsoft's page that brought no tokens is told to the window, how an administrator's
//! approval is asked for and read, and the whole way for an account that exists (the window's
//! commands, this host's jobs, `mailcraft-net` and its loopback port).
//!
//! Microsoft's pages are played by the test: where the host would start a browser, the test
//! answers the loopback port the program waits on, as the page would. Nothing here leaves this
//! machine, nobody is signed in and no browser is opened; every name and id is invented. (A
//! sign-in speaks to Microsoft's token endpoint only once a page has brought a code back, and
//! none of these pages brings one: where a sign-in has to go through, the test answers in the
//! host's place.)

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use mailcraft_model::{Mailbox, OAuth, OAuthProvider};
use mailcraft_ui_egui::approval::{NEEDS_APPROVAL, NO_BROWSER, SIGN_IN_DECLINED};
use mailcraft_ui_egui::{AdminConsentAnswer, AdminConsentRequest, MailApp, Services, SignInRequest};
use serde_json::json;

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// The directory of the organisation the account belongs to, and the application this
/// installation is known to Microsoft by: both invented.
const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";
const APPLICATION: &str = "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6";

/// What Microsoft's pages were opened at, in order.
type Opened = Arc<Mutex<Vec<String>>>;

/// `%XX` undone.
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|pair| std::str::from_utf8(pair).ok()).and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a page's address asks, by name.
fn asked(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map_or("", |(_, query)| query);
    query.split('&').filter_map(|pair| pair.split_once('=')).map(|(name, value)| (decoded(name), decoded(value))).collect()
}

/// A page of Microsoft's, played by the test: opened, it comes back to the program with
/// `answer` (and the state it was given), as the page would by sending the browser there.
fn page(answer: &'static str, opened: &Opened) -> impl Fn(&str) -> bool + use<> {
    let opened = opened.clone();
    move |url: &str| {
        opened.lock().unwrap().push(url.to_string());
        let asked = asked(url);
        let port: u16 = asked["redirect_uri"].strip_prefix("http://localhost:").unwrap().parse().unwrap();
        let request = format!("GET /?{answer}&state={} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n", asked["state"]);
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("localhost", port)).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let _ = stream.read_to_end(&mut Vec::new());
        });
        true
    }
}

fn settings() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: APPLICATION.into(), ..Default::default() }
}

fn sign_in_request(cancel: &Arc<AtomicBool>) -> SignInRequest {
    SignInRequest {
        account: 0,
        oauth: settings(),
        email: "avery@lumen.example".into(),
        refresh: String::new(),
        cancel: cancel.clone(),
        quiet: false,
        graph: false,
        mail_only: false,
        favorites: false,
        mail_write: false,
        shared: false,
        shared_send: false,
    }
}

fn approval_request(tenant: &str, cancel: &Arc<AtomicBool>) -> AdminConsentRequest {
    AdminConsentRequest { account: 0, oauth: settings(), tenant: tenant.into(), cancel: cancel.clone() }
}

#[test]
fn a_sign_in_that_ends_on_microsofts_page_is_told_to_the_window_in_its_own_words() {
    let opened = Opened::default();
    let free = || Arc::new(AtomicBool::new(false));
    // "Need admin approval", and the owner goes back to the program from it: Microsoft's own
    // number for it says whose no it is.
    let needs = page("error=access_denied&error_description=AADSTS90094%3A+Administrator+consent+is+required.%0D%0ATrace+ID%3A+12", &opened);
    assert_eq!(super::sign_in_with(sign_in_request(&free()), &needs), Err(NEEDS_APPROVAL.to_string()));
    // The page the owner was sent to: Microsoft's sign-in, for this application and this
    // address, coming back to this machine.
    let first = opened.lock().unwrap()[0].clone();
    assert!(first.starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"), "{first}");
    let sent = asked(&first);
    assert_eq!((sent["client_id"].as_str(), sent["login_hint"].as_str()), (APPLICATION, "avery@lumen.example"));
    assert!(
        sent["redirect_uri"].starts_with("http://localhost:") && sent["scope"].contains("IMAP.AccessAsUser.All") && !sent["scope"].contains("EWS")
    );
    // The same for a sign-in that asks for mail alone, and for the calendar and contacts.
    let mail_only = SignInRequest { mail_only: true, ..sign_in_request(&free()) };
    assert_eq!(super::sign_in_with(mail_only, &needs), Err(NEEDS_APPROVAL.to_string()));
    assert!(!asked(opened.lock().unwrap().last().unwrap())["scope"].contains("graph.microsoft.com"));
    let graph = SignInRequest { graph: true, ..sign_in_request(&free()) };
    assert_eq!(super::sign_in_with(graph, &needs), Err(NEEDS_APPROVAL.to_string()));
    assert!(asked(opened.lock().unwrap().last().unwrap())["scope"].contains("https://graph.microsoft.com/Calendars.ReadWrite"));

    // A no that was given on the page (and what Microsoft sends for the way back from the page
    // that asks for an administrator, where its approval requests are switched on).
    let no = page("error=access_denied&error_subcode=cancel&error_description=AADSTS65004%3A+User+declined+to+consent+to+access+the+app.", &opened);
    assert_eq!(super::sign_in_with(sign_in_request(&free()), &no), Err(SIGN_IN_DECLINED.to_string()));
    // Anything else is said in Microsoft's words.
    let shut = page("error=access_denied&error_description=AADSTS53003%3A+Access+has+been+blocked+by+Conditional+Access+policies.", &opened);
    let said = super::sign_in_with(sign_in_request(&free()), &shut).unwrap_err();
    assert!(said.contains("AADSTS53003: Access has been blocked by Conditional Access policies."), "{said}");

    // No browser could be started: said as that, at once, and the wait is over.
    let cancel = free();
    let started = Instant::now();
    assert_eq!(super::sign_in_with(sign_in_request(&cancel), &|_| false), Err(NO_BROWSER.to_string()));
    assert!(started.elapsed() < Duration::from_secs(20) && cancel.load(Ordering::Relaxed), "{:?}", started.elapsed());
    for request in [
        SignInRequest { graph: true, ..sign_in_request(&free()) },
        SignInRequest { graph: true, favorites: true, ..sign_in_request(&free()) },
        SignInRequest { mail_only: true, ..sign_in_request(&free()) },
    ] {
        assert_eq!(super::sign_in_with(request, &|_| false), Err(NO_BROWSER.to_string()));
    }
    // A sign-in nobody asked for puts no page in front of anybody, as before.
    let nobody = |_: &str| -> bool { panic!("no page is to be opened") };
    let quiet = super::sign_in_with(SignInRequest { quiet: true, ..sign_in_request(&free()) }, &nobody).unwrap_err();
    assert!(quiet.contains("has run out"), "{quiet}");
    // Nothing of the owner went anywhere but to the page that was "opened".
    assert!(opened.lock().unwrap().iter().all(|url| url.starts_with("https://login.microsoftonline.com/")));
}

#[test]
fn an_administrators_approval_is_asked_on_microsofts_page_and_its_answer_read() {
    let opened = Opened::default();
    let free = || Arc::new(AtomicBool::new(false));
    // An administrator approves: Microsoft's page comes back with a yes.
    let yes = page("admin_consent=True&tenant=3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b&scope=https%3A%2F%2Fgraph.microsoft.com%2FMail.Send", &opened);
    let approved = super::admin_consent_with(approval_request("lumen.example", &free()), &yes);
    assert_eq!(approved, AdminConsentAnswer::Approved);
    // The page that was opened: Microsoft's admin-consent page for the organisation of the
    // address, this application, everything the program asks an owner for, and the way back.
    let url = opened.lock().unwrap()[0].clone();
    assert!(url.starts_with("https://login.microsoftonline.com/lumen.example/v2.0/adminconsent?"), "{url}");
    let sent = asked(&url);
    assert_eq!((sent.len(), sent["client_id"].as_str()), (4, APPLICATION));
    assert!(sent["redirect_uri"].starts_with("http://localhost:") && sent["state"].len() >= 32);
    for permission in [
        "https://outlook.office.com/IMAP.AccessAsUser.All",
        "https://outlook.office.com/SMTP.Send",
        "https://graph.microsoft.com/Mail.Send",
        "https://graph.microsoft.com/Calendars.ReadWrite",
        "https://graph.microsoft.com/Contacts.Read",
        "https://graph.microsoft.com/MailboxConfigItem.Read",
        "https://graph.microsoft.com/Mail.ReadBasic",
        "offline_access",
        "openid",
    ] {
        assert!(sent["scope"].split(' ').any(|scope| scope == permission), "{permission}: {}", sent["scope"]);
    }
    // Nothing of Exchange Web Services is in it.
    assert!(!sent["scope"].contains("EWS") && !sent["scope"].contains("office365"));
    // Without a directory that is known: whichever the administrator is of.
    assert_eq!(super::admin_consent_with(approval_request("", &free()), &yes), AdminConsentAnswer::Approved);
    let any = opened.lock().unwrap()[1].clone();
    assert!(any.starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?"), "{any}");
    assert_eq!(asked(&any)["scope"], sent["scope"]);
    // Nothing of the owner is in either address.
    for url in opened.lock().unwrap().iter() {
        assert!(!url.contains("avery") && !url.contains("login_hint") && !decoded(url).contains('@'), "{url}");
    }

    // Whoever signed in on the page is no administrator; the approval was declined there; no
    // browser could be started; a directory that can't be one. Each is said as what it is.
    let nobody = page("admin_consent=True&error=access_denied&error_description=AADSTS90094%3A+Administrator+consent+is+required.", &opened);
    let request = || approval_request(ORGANISATION, &free());
    assert_eq!(
        super::admin_consent_with(request(), &nobody),
        AdminConsentAnswer::NotAdministrator("AADSTS90094: Administrator consent is required.".into())
    );
    let no = page(
        "admin_consent=True&error=consent_required&error_description=AADSTS65004%3A+The+resource+owner+or+authorization+server+denied+the+request.",
        &opened,
    );
    let declined = AdminConsentAnswer::Declined("AADSTS65004: The resource owner or authorization server denied the request.".into());
    assert_eq!(super::admin_consent_with(request(), &no), declined);
    let cancel = free();
    assert_eq!(super::admin_consent_with(approval_request("", &cancel), &|_| false), AdminConsentAnswer::NoBrowser);
    assert!(cancel.load(Ordering::Relaxed));
    let unopened = |_: &str| -> bool { panic!("no page is to be opened") };
    let odd = super::admin_consent_with(approval_request("lumen.example/evil", &free()), &unopened);
    assert!(matches!(&odd, AdminConsentAnswer::Failed(why) if why.contains("tenant")), "{odd:?}");
    let called_off = super::admin_consent_with(approval_request("", &Arc::new(AtomicBool::new(true))), &unopened);
    assert_eq!(called_off, AdminConsentAnswer::Cancelled);
}

/// What the organisation has done, and what the test saw the program ask.
#[derive(Default)]
struct Microsoft {
    /// An administrator has approved MailCraft.
    approved: bool,
    opened: Opened,
    /// The Send/Receive runs that started: the access token each brought.
    syncs: Vec<String>,
}

/// An account of an organisation that signs in with Microsoft, in a mailbox of its own, with
/// this host's own jobs for signing in and for an administrator's approval. Microsoft's pages
/// are the test's: its sign-in page says an administrator has to approve first until one has,
/// and its admin-consent page is answered by an administrator who approves.
fn organisation() -> (MailApp, mailcraft_model::Id, Arc<Mutex<Microsoft>>) {
    let microsoft: Arc<Mutex<Microsoft>> = Default::default();
    let mut app = MailApp::new(
        Session::at(Mailbox::with_local_account("", ""), NOW, 0),
        Services { microsoft_client_id: APPLICATION.into(), ..Default::default() },
    );
    (app.autosave, app.auto_sync) = (false, false);
    let seen = microsoft.clone();
    app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
        let (approved, opened) = {
            let microsoft = seen.lock().unwrap();
            (microsoft.approved, microsoft.opened.clone())
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let answer = if approved {
            // (The test in Microsoft's place: the page lets the owner in, and the tokens come.)
            Ok(Tokens {
                access: "mail-access".into(),
                refresh: "refresh-1".into(),
                expires_at: mailcraft_time::now() + 3_600,
                email: request.email.clone(),
                tenant: ORGANISATION.into(),
                ..Default::default()
            })
        } else {
            // The host's own job, with the test's page in the browser's place.
            let needs = page("error=access_denied&error_description=AADSTS90094%3A+Administrator+consent+is+required.", &opened);
            super::sign_in_with(request, &needs)
        };
        let _ = tx.send(answer);
        rx
    }));
    let seen = microsoft.clone();
    app.services.admin_consent = Some(Box::new(move |request: AdminConsentRequest| {
        let opened = seen.lock().unwrap().opened.clone();
        let yes = page("admin_consent=True&tenant=3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b", &opened);
        let answer = super::admin_consent_with(request, &yes);
        seen.lock().unwrap().approved |= answer == AdminConsentAnswer::Approved;
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(answer);
        rx
    }));
    // The host's own way of making the address an administrator is sent.
    app.services.admin_consent_link = Some(Box::new(|oauth, tenant| mailcraft_net::oauth::admin_consent_url(oauth, tenant).ok()));
    let seen = microsoft.clone();
    app.services.sync = Some(Box::new(move |request: SyncRequest| {
        seen.lock().unwrap().syncs.push(request.access_token.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(SyncResult { account: request.account.id, ..Default::default() });
        rx
    }));
    let added = app.execute(
        "account.add",
        json!({"email": "avery@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}),
    );
    let account = added.unwrap()["id"].as_u64().unwrap();
    (app, account, microsoft)
}

#[test]
fn an_account_refused_for_approval_is_told_what_to_do_and_signs_in_once_an_administrator_approved() {
    let (mut app, account, microsoft) = organisation();
    // Send/Receive, as the owner asks for it: Microsoft's page says an administrator has to
    // approve first. The window says so, and brings up the page that says what to do next.
    assert_eq!(app.execute("ui.sync", json!({"account": account})).unwrap()["pending"], "signIn");
    app.poll_sign_in();
    assert!(!app.signing_in() && !app.syncing() && app.has_sync_problems());
    assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some(NEEDS_APPROVAL));
    assert_eq!(app.dialog.as_ref().map(|dialog| dialog.name()), Some("account"));
    assert_eq!(microsoft.lock().unwrap().opened.lock().unwrap().len(), 1);

    // The message for an administrator: Microsoft's own page for approving this application,
    // for everything it asks an owner for; nothing of the owner in it; and
    // nothing is opened or sent for it.
    let copied = app.execute("ui.copyAdminMessage", json!({})).unwrap();
    let (text, link) = (copied["text"].as_str().unwrap(), copied["url"].as_str().unwrap());
    assert_eq!(copied["account"], account);
    assert!(link.starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?"), "{link}");
    let sent = asked(link);
    assert_eq!((sent.len(), sent["client_id"].as_str(), sent["redirect_uri"].as_str()), (3, APPLICATION, "http://localhost"));
    assert!(sent["scope"].contains("IMAP.AccessAsUser.All") && sent["scope"].contains("MailboxConfigItem.Read") && !sent["scope"].contains("EWS"));
    assert!(text.lines().any(|line| line == link) && text.contains("MailCraft is an open-source mail and calendar program"));
    for never in ["avery", "Avery", "lumen", "@"] {
        assert!(!text.contains(never), "{never}: {text}");
    }
    assert_eq!(microsoft.lock().unwrap().opened.lock().unwrap().len(), 1);
    // (The bare address, for a script that wants only that, is the same one.)
    assert_eq!(app.execute("ui.copyAdminConsentLink", json!({"account": account})).unwrap()["url"], link);

    // "I'm an administrator": Microsoft's admin-consent page, in this host's own way; an
    // administrator approves there; the account then signs in again by itself, Microsoft lets
    // it, and its Send/Receive starts with the token.
    assert_eq!(app.execute("ui.adminConsent", json!({})).unwrap(), json!({"pending": "adminConsent", "account": account}));
    app.poll_admin_consent();
    assert!(!app.approving() && app.signing_in());
    {
        let microsoft = microsoft.lock().unwrap();
        let opened = microsoft.opened.lock().unwrap();
        assert_eq!(opened.len(), 2);
        assert!(opened[1].starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?"), "{}", opened[1]);
        // The same permissions as in the message, and the way back to this machine.
        let page = asked(&opened[1]);
        assert_eq!((page["client_id"].as_str(), page["scope"].as_str()), (APPLICATION, sent["scope"].as_str()));
        assert!(page["redirect_uri"].starts_with("http://localhost:") && page["state"].len() >= 32);
        assert!(microsoft.approved);
    }
    app.poll_sign_in();
    assert!(app.syncing() && !app.signing_in());
    assert_eq!(microsoft.lock().unwrap().syncs, ["mail-access"]);

    // Nothing of any of it is in the mailbox: no token, no directory, no approval.
    let file = String::from_utf8(mailcraft_engine::io::write_mailbox(&app.session.mailbox).unwrap()).unwrap();
    for never in ["mail-access", "refresh-1", ORGANISATION, "adminconsent", "approv"] {
        assert!(!file.contains(never), "{never}");
    }
}

#[test]
fn a_sign_in_nobody_asked_for_and_a_script_without_the_page_are_served_too() {
    // A Send/Receive that started by itself never puts a page of Microsoft's in front of the
    // owner, so it can't be told that an approval is needed, and no dialog comes of it.
    let (mut app, account, microsoft) = organisation();
    assert_eq!(app.execute("ui.sync", json!({"account": account, "quiet": true})).unwrap()["pending"], "signIn");
    app.poll_sign_in();
    assert!(app.dialog.is_none() && microsoft.lock().unwrap().opened.lock().unwrap().is_empty());
    assert!(app.status_msg.as_ref().is_some_and(|(text, _)| text.contains("has run out")));

    // A script that runs the commands without the page: the approval, then the sign-in that
    // follows it by itself, and nothing is shown that nobody asked for.
    assert_eq!(app.execute("ui.adminConsent", json!({"account": account})).unwrap(), json!({"pending": "adminConsent", "account": account}));
    assert!(app.approving() && app.dialog.is_none());
    // (One page of Microsoft's at a time.)
    assert!(app.execute("ui.sync", json!({"account": account})).is_err());
    app.poll_admin_consent();
    app.poll_sign_in();
    assert!(app.syncing() && app.dialog.is_none() && microsoft.lock().unwrap().approved);
    // The page opens on the command for it, and "try again" from it is the account's Send/Receive.
    let (mut app, account, microsoft) = organisation();
    assert_eq!(app.execute("ui.approvalHelp", json!({})).unwrap(), json!({"dialog": "account", "page": "approval", "account": account}));
    assert!(app.dialog.is_some() && microsoft.lock().unwrap().opened.lock().unwrap().is_empty());
    assert_eq!(app.execute("ui.signInAgain", json!({})).unwrap()["pending"], "signIn");
    assert!(app.dialog.is_none() && app.signing_in());
}
