// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accounts and sign-in through a fake `Platform` and a fake token endpoint: Microsoft's page is
//! played by the platform (opened, it hands the address the system would have opened the app
//! with to the core), and the token endpoint is a server on this machine. Nothing leaves it, and
//! every name is invented.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use mailcraft_charset::base64::URL_SAFE_NO_PAD;
use mailcraft_engine::Session;

use crate::fake_tests::{FakeSecrets, block_on, now};
use crate::{
    Core, CoreError, HostError, NetworkKind, NotificationRecord, Permission, PermissionOutcome, PermissionState, Platform, RedirectKind, SecretStore,
    SignInOutcome,
};

const SCHEME: &str = "org.example.mailcraft";
const ADA: &str = "ada@lumen.example";
const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";

/// A token endpoint on this machine: answers each request with what `answer` makes of its form.
struct TokenServer {
    port: u16,
    forms: Arc<Mutex<Vec<String>>>,
}

impl TokenServer {
    fn start(answer: impl Fn(&str) -> (u16, String) + Send + 'static) -> TokenServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let forms = Arc::new(Mutex::new(Vec::new()));
        let kept = forms.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                let body_at = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(body_at) = body_at else { continue };
                let head = String::from_utf8_lossy(&data[..body_at]).to_lowercase();
                let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                while data.len() < body_at + length {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                }
                let form = String::from_utf8_lossy(&data[body_at..]).to_string();
                kept.lock().unwrap().push(form.clone());
                let (status, body) = answer(&form);
                let reason = if status == 200 { "OK" } else { "Bad Request" };
                let reply = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        TokenServer { port, forms }
    }

    fn asked(&self) -> usize {
        self.forms.lock().unwrap().len()
    }
}

/// What the token endpoint gives for a good code: access and refresh tokens, and who it is.
fn tokens_for(email: &str, name: &str, refresh: &str) -> String {
    let claims = format!(r#"{{"email":"{email}","name":"{name}","tid":"{ORGANISATION}"}}"#);
    let id_token = format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.as_bytes()));
    format!(r#"{{"token_type":"Bearer","expires_in":3599,"access_token":"the-access-token","refresh_token":"{refresh}","id_token":"{id_token}"}}"#)
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|pair| std::str::from_utf8(pair).ok()).and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_of(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (percent_decoded(k), percent_decoded(v))).collect()
}

type OnPage = Box<dyn Fn(&Core, &HashMap<String, String>) + Send + Sync>;

/// A phone: Microsoft's page is `on_page`, what it does when it is opened.
struct Phone {
    secrets: Arc<FakeSecrets>,
    has_store: AtomicBool,
    opens: AtomicBool,
    core: Mutex<Weak<Core>>,
    on_page: Mutex<OnPage>,
    pages: Mutex<Vec<String>>,
}

impl Platform for Phone {
    fn secrets(&self) -> Result<Option<Arc<dyn SecretStore>>, HostError> {
        Ok(self.has_store.load(Ordering::Relaxed).then(|| self.secrets.clone() as Arc<dyn SecretStore>))
    }

    fn redirect(&self) -> Result<RedirectKind, HostError> {
        Ok(RedirectKind::CustomScheme { scheme: SCHEME.to_string() })
    }

    fn open_sign_in_page(&self, url: String) -> Result<(), HostError> {
        if !self.opens.load(Ordering::Relaxed) {
            return Err(HostError::Failed);
        }
        self.pages.lock().unwrap().push(url.clone());
        let core = self.core.lock().unwrap().upgrade();
        if let Some(core) = core {
            (self.on_page.lock().unwrap())(&core, &query_of(&url));
        }
        Ok(())
    }

    fn notify(&self, _: NotificationRecord) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn schedule_reminder(&self, _: i64, _: NotificationRecord) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn open_file(&self, _: String, _: Vec<u8>) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn now(&self) -> Result<i64, HostError> {
        Ok(now())
    }

    fn utc_offset_minutes(&self, _: i64) -> Result<i32, HostError> {
        Ok(0)
    }

    fn memory_budget(&self) -> Result<Option<u64>, HostError> {
        Ok(None)
    }

    fn network(&self) -> Result<NetworkKind, HostError> {
        Ok(NetworkKind::Unknown)
    }
}

struct Rig {
    core: Arc<Core>,
    phone: Arc<Phone>,
}

impl Rig {
    fn new(server: &TokenServer) -> Rig {
        let phone = Arc::new(Phone {
            secrets: Arc::new(FakeSecrets::default()),
            has_store: AtomicBool::new(true),
            opens: AtomicBool::new(true),
            core: Mutex::new(Weak::new()),
            on_page: Mutex::new(Box::new(|_, _| {})),
            pages: Mutex::new(Vec::new()),
        });
        let core = Core::start(phone.clone(), Session::empty).unwrap();
        *phone.core.lock().unwrap() = Arc::downgrade(&core);
        *core.signins.server.lock().unwrap() =
            Some((format!("http://127.0.0.1:{0}/authorize", server.port), format!("http://127.0.0.1:{0}/token", server.port)));
        Rig { core, phone }
    }

    /// Microsoft's page comes back with `tail` after the redirect address (`{state}` filled in).
    fn page_comes_back_with(&self, tail: &'static str) {
        *self.phone.on_page.lock().unwrap() = Box::new(move |core, asked| {
            let address = format!("{SCHEME}:/oauth2redirect{}", tail.replace("{state}", &asked["state"]));
            assert!(core.hand_back_redirect(address), "a sign-in was waiting for it");
        });
    }

    fn sign_in(&self, hint: &str) -> SignInOutcome {
        block_on(self.core.sign_in_microsoft(hint.to_string())).unwrap()
    }

    fn secret(&self, key: &str) -> Option<String> {
        self.phone.secrets.kept.lock().unwrap().iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn secrets_kept(&self) -> usize {
        self.phone.secrets.kept.lock().unwrap().len()
    }

    fn accounts(&self) -> Vec<(u64, String)> {
        block_on(self.core.folders(0)).unwrap().accounts.into_iter().filter(|a| a.kind == "microsoft").map(|a| (a.id, a.email)).collect()
    }
}

const CODE_BACK: &str = "?code=the-code&state={state}";

fn good_server() -> TokenServer {
    TokenServer::start(|_| (200, tokens_for(ADA, "Ada Lumen", "refresh-token-1")))
}

fn signed_in(rig: &Rig) -> u64 {
    rig.page_comes_back_with(CODE_BACK);
    match rig.sign_in("") {
        SignInOutcome::SignedIn { account, .. } => account,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_sign_in_adds_the_account_and_keeps_only_the_refresh_token_in_the_secure_store() {
    let server = good_server();
    let rig = Rig::new(&server);
    rig.page_comes_back_with(CODE_BACK);
    let outcome = rig.sign_in("ada@lumen.example");
    let SignInOutcome::SignedIn { account, email, name } = outcome.clone() else { panic!("{outcome:?}") };
    assert_eq!((email.as_str(), name.as_str()), (ADA, "Ada Lumen"));
    assert_eq!(rig.accounts(), vec![(account, ADA.to_string())]);
    // The refresh token is in the store, under the account's address, and nothing else is.
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("refresh-token-1"));
    assert_eq!(rig.secrets_kept(), 1);
    // No token is in the outcome's debug text, nor is the access token kept anywhere.
    let text = format!("{outcome:?}");
    assert!(!text.contains("refresh-token-1") && !text.contains("the-access-token"));
    assert!(!rig.phone.secrets.kept.lock().unwrap().iter().any(|(_, v)| v.contains("the-access-token")));
    // The page was sent where the system brings the app back to, for what a Graph account takes
    // and nothing that is asked for by itself.
    let page = query_of(&rig.phone.pages.lock().unwrap()[0]);
    assert_eq!(page["redirect_uri"], format!("{SCHEME}:/oauth2redirect"));
    assert_eq!(page["login_hint"], ADA);
    assert!(page["scope"].contains("Mail.ReadWrite") && !page["scope"].contains("Tasks") && !page["scope"].contains("Shared"));
    // The code went to the token endpoint once, with the verifier whose hash the page was shown.
    assert_eq!(server.asked(), 1);
    // The next hand-back finds nobody waiting.
    assert!(!rig.core.hand_back_redirect(format!("{SCHEME}:/oauth2redirect?code=x&state=y")));
}

#[test]
fn signing_in_again_to_an_account_the_owner_has_renews_its_sign_in_and_adds_nothing() {
    let server = good_server();
    let rig = Rig::new(&server);
    let first = signed_in(&rig);
    *rig.phone.secrets.kept.lock().unwrap() = vec![("refresh:ada@lumen.example|graph".into(), "old".into())];
    let second = signed_in(&rig);
    assert_eq!(first, second);
    assert_eq!(rig.accounts().len(), 1);
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("refresh-token-1"));
    assert_eq!(rig.secrets_kept(), 1);
}

#[test]
fn an_address_with_the_wrong_state_is_refused_and_nothing_is_traded_or_kept() {
    let server = good_server();
    let rig = Rig::new(&server);
    for tail in [
        "?code=c&state=not-the-state",
        "?code=c",
        "?code=c&state={state}&evil=1",
        "?code=c&state={state}&code=d",
        "?state={state}",
        "?code=c&state={state}#more",
    ] {
        rig.page_comes_back_with(tail);
        let outcome = rig.sign_in("");
        assert!(matches!(outcome, SignInOutcome::Failed { .. } | SignInOutcome::Declined), "{tail}: {outcome:?}");
        assert!(rig.accounts().is_empty(), "{tail}");
    }
    assert_eq!((server.asked(), rig.secrets_kept()), (0, 0));
}

#[test]
fn another_program_naming_our_scheme_is_not_believed() {
    let server = good_server();
    let rig = Rig::new(&server);
    // Nobody waits: an address is dropped.
    assert!(!rig.core.hand_back_redirect(format!("{SCHEME}:/oauth2redirect?code=c&state=s")));
    assert!(!rig.core.hand_back_redirect(String::new()));
    // An address from another scheme, or of any size, while a sign-in waits, ends it and trades nothing.
    for address in [
        "http://localhost/?code=c&state={state}".to_string(),
        "org.example.other:/oauth2redirect?code=c&state={state}".to_string(),
        format!("{SCHEME}:/oauth2redirect?code={}&state={{state}}", "a".repeat(1 << 20)),
        "\u{0}".to_string(),
    ] {
        let template = address.clone();
        *rig.phone.on_page.lock().unwrap() = Box::new(move |core, asked| {
            core.hand_back_redirect(template.replace("{state}", &asked["state"]));
            // (An address too long for the hand-back is not taken: the sign-in waits until called off.)
            core.cancel_sign_in();
        });
        let outcome = rig.sign_in("");
        assert!(matches!(outcome, SignInOutcome::Failed { .. } | SignInOutcome::Cancelled), "{outcome:?}");
    }
    assert_eq!((server.asked(), rig.secrets_kept(), rig.accounts().len()), (0, 0, 0));
}

#[test]
fn only_the_first_address_is_taken() {
    let server = good_server();
    let rig = Rig::new(&server);
    *rig.phone.on_page.lock().unwrap() = Box::new(|core, asked| {
        let state = &asked["state"];
        assert!(core.hand_back_redirect(format!("{SCHEME}:/oauth2redirect?code=the-code&state={state}")));
        assert!(!core.hand_back_redirect(format!("{SCHEME}:/oauth2redirect?code=evil&state={state}")));
    });
    assert!(matches!(rig.sign_in(""), SignInOutcome::SignedIn { .. }));
    let form = server.forms.lock().unwrap()[0].clone();
    assert!(form.contains("code=the-code") && !form.contains("evil"));
}

#[test]
fn the_ways_a_sign_in_ends_without_tokens_are_told_apart() {
    let server = good_server();
    let rig = Rig::new(&server);
    let ends = [
        ("?error=access_denied&error_description=AADSTS90094%3A+Admin+consent+is+required.&state={state}", SignInOutcome::NeedsApproval),
        ("?error=access_denied&error_description=AADSTS53003%3A+Blocked+by+Conditional+Access.&state={state}", SignInOutcome::OrganisationBlocks),
        ("?error=access_denied&error_description=AADSTS65004%3A+User+declined.&state={state}", SignInOutcome::Declined),
    ];
    for (tail, expected) in ends {
        rig.page_comes_back_with(tail);
        assert_eq!(rig.sign_in(""), expected, "{tail}");
    }
    assert_eq!((server.asked(), rig.secrets_kept(), rig.accounts().len()), (0, 0, 0));
}

#[test]
fn the_owner_leaving_the_page_ends_the_wait_and_so_does_a_browser_that_cannot_start() {
    let server = good_server();
    let rig = Rig::new(&server);
    // The page opens and nothing comes back; the owner calls it off.
    let core = rig.core.clone();
    *rig.phone.on_page.lock().unwrap() = Box::new(move |_, _| {
        let core = core.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            core.cancel_sign_in();
        });
    });
    assert_eq!(rig.sign_in(""), SignInOutcome::Cancelled);
    // No browser: said so, at once.
    rig.phone.opens.store(false, Ordering::Relaxed);
    assert_eq!(rig.sign_in(""), SignInOutcome::NoBrowser);
    assert_eq!((server.asked(), rig.secrets_kept(), rig.accounts().len()), (0, 0, 0));
}

#[test]
fn one_sign_in_is_open_at_a_time_and_the_place_is_given_up() {
    let server = good_server();
    let rig = Rig::new(&server);
    let first = rig.core.signins.begin().unwrap();
    assert!(rig.core.signins.begin().is_none());
    assert_eq!(rig.sign_in(""), SignInOutcome::Failed { reason: "a sign-in is already open".to_string() });
    drop(first);
    assert!(rig.core.signins.begin().is_some());
}

#[test]
fn a_store_that_refuses_or_is_missing_means_no_account_and_no_half_kept_sign_in() {
    let server = good_server();
    let rig = Rig::new(&server);
    rig.page_comes_back_with(CODE_BACK);
    rig.phone.secrets.refuse.store(true, Ordering::Relaxed);
    assert!(matches!(rig.sign_in(""), SignInOutcome::Failed { .. }));
    rig.phone.secrets.refuse.store(false, Ordering::Relaxed);
    rig.phone.has_store.store(false, Ordering::Relaxed);
    assert!(matches!(rig.sign_in(""), SignInOutcome::Failed { .. }));
    assert_eq!((rig.accounts().len(), rig.secrets_kept()), (0, 0));
}

#[test]
fn a_sign_in_that_brings_no_refresh_token_or_no_address_makes_no_account() {
    let rig = Rig::new(&TokenServer::start(|_| (200, tokens_for(ADA, "Ada", ""))));
    rig.page_comes_back_with(CODE_BACK);
    assert!(matches!(rig.sign_in(""), SignInOutcome::Failed { .. }));
    let rig =
        Rig::new(&TokenServer::start(|_| (200, r#"{"token_type":"Bearer","expires_in":3599,"access_token":"a","refresh_token":"r"}"#.to_string())));
    rig.page_comes_back_with(CODE_BACK);
    // (No address from Microsoft: a hint is only for the page, never taken for who signed in.)
    assert!(matches!(rig.sign_in("not an address\u{7}"), SignInOutcome::Failed { .. }));
    assert!(matches!(rig.sign_in(ADA), SignInOutcome::Failed { .. }));
    assert_eq!((rig.accounts().len(), rig.secrets_kept()), (0, 0));
}

#[test]
fn a_sign_in_that_makes_no_account_takes_its_sign_in_back_out_of_the_store() {
    let server = good_server();
    let rig = Rig::new(&server);
    // The engine refuses a second account of an address it already has (of another kind).
    block_on(rig.core.execute("account.add".to_string(), format!(r#"{{"email":"{ADA}"}}"#))).unwrap();
    rig.page_comes_back_with(CODE_BACK);
    let outcome = rig.sign_in("");
    assert!(matches!(outcome, SignInOutcome::Failed { .. }), "{outcome:?}");
    assert_eq!((rig.secrets_kept(), rig.accounts().len()), (0, 0));
}

#[test]
fn a_permission_is_asked_for_by_itself_and_replaces_the_sign_in_with_the_one_that_covers_it() {
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    rig.page_comes_back_with(CODE_BACK);
    let server2 = TokenServer::start(|_| (200, tokens_for(ADA, "Ada Lumen", "refresh-token-2")));
    *rig.core.signins.server.lock().unwrap() =
        Some((format!("http://127.0.0.1:{0}/authorize", server2.port), format!("http://127.0.0.1:{0}/token", server2.port)));
    let outcome = block_on(rig.core.grant_permission(account, Permission::Tasks)).unwrap();
    assert_eq!(outcome, PermissionOutcome::Granted);
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("refresh-token-2"));
    let page = query_of(rig.phone.pages.lock().unwrap().last().unwrap());
    assert!(page["scope"].contains("Tasks.ReadWrite") && !page["scope"].contains("Shared"), "{}", page["scope"]);
    assert_eq!(page["login_hint"], ADA);
    // Each of the three has its own page.
    for (permission, wanted) in [(Permission::SharedMail, "Mail.ReadWrite.Shared"), (Permission::SharedSend, "Mail.Send.Shared")] {
        assert_eq!(block_on(rig.core.grant_permission(account, permission)).unwrap(), PermissionOutcome::Granted);
        assert!(query_of(rig.phone.pages.lock().unwrap().last().unwrap())["scope"].contains(wanted));
    }
}

#[test]
fn a_permission_signed_in_by_somebody_else_is_not_kept() {
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    let other = TokenServer::start(|_| (200, tokens_for("bob@lumen.example", "Bob", "bobs-refresh-token")));
    *rig.core.signins.server.lock().unwrap() =
        Some((format!("http://127.0.0.1:{0}/authorize", other.port), format!("http://127.0.0.1:{0}/token", other.port)));
    rig.page_comes_back_with(CODE_BACK);
    let outcome = block_on(rig.core.grant_permission(account, Permission::Tasks)).unwrap();
    assert!(matches!(outcome, PermissionOutcome::Failed { .. }), "{outcome:?}");
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("refresh-token-1"));
    assert_eq!(rig.secrets_kept(), 1);
}

#[test]
fn a_permissions_no_says_whose_it_is_and_changes_nothing() {
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    let asks = server.asked();
    let no = [
        ("?error=access_denied&error_description=AADSTS90094%3A+Admin.&state={state}", PermissionOutcome::NeedsApproval),
        ("?error=access_denied&error_description=AADSTS53003%3A+Policy.&state={state}", PermissionOutcome::OrganisationBlocks),
        ("?error=access_denied&error_description=AADSTS500207%3A+Kind.&state={state}", PermissionOutcome::NotForThisAccount),
    ];
    for (tail, expected) in no {
        rig.page_comes_back_with(tail);
        assert_eq!(block_on(rig.core.grant_permission(account, Permission::SharedSend)).unwrap(), expected, "{tail}");
    }
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("refresh-token-1"));
    assert_eq!(server.asked(), asks);
    // An account that isn't there is asked for nothing.
    assert!(block_on(rig.core.grant_permission(account + 99, Permission::Tasks)).is_err());
    assert_eq!(rig.phone.pages.lock().unwrap().len(), 4);
}

#[test]
fn what_a_sign_in_covers_is_asked_of_microsoft_with_the_stored_token_and_never_opens_a_page() {
    // To Do is covered only when the scope asked for has it.
    let covers = |scope_has: &'static str| {
        TokenServer::start(move |form| {
            let asked = percent_decoded(form);
            if asked.contains("grant_type=refresh_token") && (asked.contains(scope_has) || !asked.contains("Tasks")) {
                (200, tokens_for(ADA, "Ada", "refresh-token-1"))
            } else {
                (400, r#"{"error":"invalid_grant","error_description":"AADSTS65001: not consented"}"#.to_string())
            }
        })
    };
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    let pages = rig.phone.pages.lock().unwrap().len();
    let ask = |rig: &Rig, p| block_on(rig.core.permission_state(account, p)).unwrap();
    let point = |rig: &Rig, s: &TokenServer| {
        *rig.core.signins.server.lock().unwrap() =
            Some((format!("http://127.0.0.1:{0}/authorize", s.port), format!("http://127.0.0.1:{0}/token", s.port)));
    };
    // Everything is covered.
    assert_eq!(ask(&rig, Permission::Tasks), PermissionState::Allowed);
    assert_eq!(ask(&rig, Permission::SharedMail), PermissionState::Allowed);
    // Mail works, To Do doesn't: not allowed yet.
    point(&rig, &covers("never"));
    assert_eq!(ask(&rig, Permission::Tasks), PermissionState::NotAllowed);
    // Nothing works: the sign-in is over.
    point(&rig, &TokenServer::start(|_| (400, r#"{"error":"invalid_grant","error_description":"AADSTS70000: expired"}"#.to_string())));
    assert_eq!(ask(&rig, Permission::SharedSend), PermissionState::SignInNeeded);
    // The server can't be told: unknown, and nothing is concluded.
    point(&rig, &TokenServer::start(|_| (500, "{}".to_string())));
    assert_eq!(ask(&rig, Permission::Tasks), PermissionState::Unknown);
    // Nothing kept: sign in.
    rig.phone.secrets.kept.lock().unwrap().clear();
    assert_eq!(ask(&rig, Permission::Tasks), PermissionState::SignInNeeded);
    assert_eq!(rig.phone.pages.lock().unwrap().len(), pages);
}

#[test]
fn removing_an_account_removes_its_mail_and_its_sign_in_and_nothing_else() {
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    rig.phone.secrets.kept.lock().unwrap().push(("refresh:someone@else.example|graph".into(), "theirs".into()));
    // A wrong id removes nothing.
    assert!(block_on(rig.core.remove_account(account + 99)).is_err());
    assert_eq!((rig.accounts().len(), rig.secrets_kept()), (1, 2));
    block_on(rig.core.remove_account(account)).unwrap();
    assert!(rig.accounts().is_empty());
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph"), None);
    assert_eq!(rig.secret("refresh:someone@else.example|graph").as_deref(), Some("theirs"));
    // Gone: removing it again is an error, not a panic.
    assert!(matches!(block_on(rig.core.remove_account(account)), Err(CoreError::Failed { .. })));
}

#[test]
fn the_message_for_an_administrator_has_the_page_and_nothing_of_the_owner() {
    let rig = Rig::new(&good_server());
    let link = rig.core.admin_approval_link("ada@lumen.example".to_string()).unwrap();
    assert!(link.starts_with("https://login.microsoftonline.com/lumen.example/v2.0/adminconsent?"), "{link}");
    assert!(link.contains("client_id=") && link.contains("scope=") && !link.contains("ada"), "{link}");
    // With no address, or a hostile one, it is for whichever organisation the administrator is of.
    for typed in ["", "nobody", "a@b/../c", "a@\u{7}"] {
        let link = rig.core.admin_approval_link(typed.to_string()).unwrap();
        assert!(link.starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?"), "{typed}: {link}");
    }
}

#[test]
fn junk_in_every_account_call_is_an_answer_and_never_a_panic() {
    let server = good_server();
    let rig = Rig::new(&server);
    let long = "a".repeat(1 << 16);
    for junk in ["", "\u{0}", "\u{202e}x@y", long.as_str(), "ada@lumen.example\r\nBcc: x"] {
        assert!(!rig.core.hand_back_redirect(junk.to_string()));
        rig.page_comes_back_with("?error=access_denied&state={state}");
        let outcome = rig.sign_in(junk);
        assert!(!matches!(outcome, SignInOutcome::SignedIn { .. }));
        let _ = rig.core.admin_approval_link(junk.to_string());
    }
    for id in [0, 1, u64::MAX] {
        assert!(block_on(rig.core.remove_account(id)).is_err());
        assert!(block_on(rig.core.grant_permission(id, Permission::Tasks)).is_err());
        assert!(block_on(rig.core.permission_state(id, Permission::SharedMail)).is_err());
    }
    rig.core.cancel_sign_in();
}

#[test]
fn a_new_refresh_token_in_a_check_replaces_the_kept_one_only_when_it_is_the_accounts_own() {
    let server = good_server();
    let rig = Rig::new(&server);
    let account = signed_in(&rig);
    let point = |s: &TokenServer| {
        *rig.core.signins.server.lock().unwrap() =
            Some((format!("http://127.0.0.1:{0}/authorize", s.port), format!("http://127.0.0.1:{0}/token", s.port)));
    };
    point(&TokenServer::start(|_| (200, tokens_for(ADA, "Ada", "rotated-refresh-token"))));
    assert_eq!(block_on(rig.core.permission_state(account, Permission::Tasks)).unwrap(), PermissionState::Allowed);
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("rotated-refresh-token"));
    // An answer that names somebody else is not the account's to keep.
    point(&TokenServer::start(|_| (200, tokens_for("bob@lumen.example", "Bob", "bobs-refresh-token"))));
    assert_eq!(block_on(rig.core.permission_state(account, Permission::SharedMail)).unwrap(), PermissionState::Allowed);
    assert_eq!(rig.secret("refresh:ada@lumen.example|graph").as_deref(), Some("rotated-refresh-token"));
}
