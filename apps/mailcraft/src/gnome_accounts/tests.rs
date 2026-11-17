// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic ObjectManager replies: no session accounts or provider credentials.

use super::*;
use zbus::zvariant::{OwnedObjectPath, Str};

fn string(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

fn fixture(provider: &str, email: &str) -> zbus::fdo::ManagedObjects {
    let account = HashMap::from([
        ("ProviderType".into(), string(provider)),
        ("ProviderName".into(), string("Example provider")),
        ("MailDisabled".into(), OwnedValue::from(false)),
    ]);
    let mail = HashMap::from([("EmailAddress".into(), string(email)), ("Name".into(), string("Example Owner"))]);
    HashMap::from([(
        OwnedObjectPath::try_from("/org/gnome/OnlineAccounts/Accounts/account_1").unwrap(),
        HashMap::from([(ACCOUNT.try_into().unwrap(), account), (MAIL.try_into().unwrap(), mail)]),
    )])
}

#[test]
fn public_metadata_maps_supported_providers_and_keeps_unknown_ones_visible() {
    for (provider, expected) in [
        ("google", Provider::Google),
        ("ms_graph", Provider::Microsoft365),
        ("windows_live", Provider::OutlookCom),
        ("exchange", Provider::Unsupported),
        ("imap_smtp", Provider::Unsupported),
        ("unknown", Provider::Unsupported),
    ] {
        let accounts = parse(fixture(provider, "owner@example.org")).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].provider, expected);
        assert_eq!(accounts[0].email, "owner@example.org");
        assert_eq!(accounts[0].name, "Example Owner");
    }
}

#[test]
fn disabled_mail_is_respected_even_while_the_mail_interface_is_still_present() {
    let mut objects = fixture("google", "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(ACCOUNT).unwrap().insert("MailDisabled".into(), OwnedValue::from(true));
    }
    assert!(parse(objects).unwrap().is_empty());
}

#[test]
fn calendar_only_accounts_and_malformed_properties_are_not_imported() {
    for key in [ACCOUNT, MAIL] {
        let mut objects = fixture("google", "owner@example.org");
        for interfaces in objects.values_mut() {
            interfaces.remove(key);
        }
        assert!(parse(objects).unwrap().is_empty());
    }
    let mut objects = fixture("google", "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(MAIL).unwrap().insert("EmailAddress".into(), OwnedValue::from(42u32));
    }
    assert!(parse(objects).unwrap().is_empty());
}

#[test]
fn hostile_or_missing_addresses_never_reach_setup() {
    for email in [
        "",
        "not-an-address",
        "@example.org",
        "owner@@example.org",
        "owner@example.org\n",
        "own\u{202e}er@example.org",
        "a b@example.org",
        &format!("{}@example.org", "x".repeat(255)),
    ] {
        assert!(parse(fixture("google", email)).unwrap().is_empty(), "{email:?}");
    }
}

#[test]
fn accounts_are_sorted_deduplicated_and_bounded() {
    let mut objects = fixture("google", "owner@example.org");
    let interfaces = fixture("google", "OWNER@example.org").into_values().next().unwrap();
    objects.insert(OwnedObjectPath::try_from("/org/gnome/OnlineAccounts/Accounts/account_2").unwrap(), interfaces);
    assert_eq!(parse(objects).unwrap().len(), 1);
    let mut objects = zbus::fdo::ManagedObjects::new();
    for i in 0..257 {
        objects.insert(OwnedObjectPath::try_from(format!("/org/gnome/OnlineAccounts/Accounts/account_{i}")).unwrap(), HashMap::new());
    }
    assert_eq!(parse(objects).unwrap_err(), DiscoveryError::TooManyAccounts);
}

struct Fixture;

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Fixture {
    fn get_managed_objects(&self) -> zbus::fdo::ManagedObjects {
        fixture("google", "owner@example.org")
    }
}

/// Run with dbus-run-session and MAILCRAFT_TEST_PRIVATE_BUS=1; never touches a user's session.
#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_discovery_reads_only_public_metadata() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _lock = PRIVATE_BUS.lock().unwrap();
    let _server = zbus::blocking::connection::Builder::session().unwrap().name(SERVICE).unwrap().serve_at(PATH, Fixture).unwrap().build().unwrap();
    let accounts = discover().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].email, "owner@example.org");
}

fn linked_fixture(provider: &str, oauth: bool) -> zbus::fdo::ManagedObjects {
    let mut objects = fixture(provider, "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(ACCOUNT).unwrap().insert("Id".into(), string("account_1"));
        let mail = interfaces.get_mut(MAIL).unwrap();
        for key in ["ImapSupported", "SmtpSupported", "ImapUseSsl", "SmtpUseTls", "SmtpUseAuth"] {
            mail.insert(key.into(), OwnedValue::from(true));
        }
        mail.insert(
            "ImapHost".into(),
            string(if provider == "windows_live" {
                "outlook.office365.com"
            } else if oauth {
                "imap.gmail.com"
            } else {
                "imap.example.org:993"
            }),
        );
        mail.insert(
            "SmtpHost".into(),
            string(if provider == "windows_live" {
                "smtp-mail.outlook.com"
            } else if oauth {
                "smtp.gmail.com"
            } else {
                "smtp.example.org:587"
            }),
        );
        mail.insert("ImapUserName".into(), string("incoming@example.org"));
        mail.insert("SmtpUserName".into(), string("outgoing@example.org"));
        mail.insert(if oauth { "SmtpAuthXoauth2" } else { "SmtpAuthPlain" }.into(), OwnedValue::from(true));
        interfaces.insert(if oauth { OAUTH } else { PASSWORD }.try_into().unwrap(), HashMap::new());
    }
    objects
}

#[test]
fn linked_configuration_requires_secure_protocols_and_matching_oauth_capabilities() {
    for (provider, oauth, supported) in
        [("google", true, true), ("windows_live", true, true), ("ms_graph", true, true), ("unknown", true, false), ("imap_smtp", false, true)]
    {
        let accounts = parse(linked_fixture(provider, oauth)).unwrap();
        assert_eq!(accounts[0].linked.is_some(), supported, "{provider}");
    }
    for key in ["ImapSupported", "SmtpSupported", "ImapUseSsl", "SmtpUseTls", "SmtpUseAuth", "SmtpAuthXoauth2"] {
        let mut objects = linked_fixture("google", true);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert(key.into(), OwnedValue::from(false));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none(), "{key}");
    }
    for key in ["ImapAcceptSslErrors", "SmtpAcceptSslErrors"] {
        let mut objects = linked_fixture("google", true);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert(key.into(), OwnedValue::from(true));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none());
    }
    for host in ["imap.evil.example", "imap.gmail.com.evil.example", "imap.gmail.com:bad", "https://imap.gmail.com", "imap.gmail.com:0"] {
        let mut objects = linked_fixture("google", true);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert("ImapHost".into(), string(host));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none(), "{host}");
    }
}

struct LinkedFixture {
    graph: bool,
    oauth: bool,
    enabled: std::sync::Arc<AtomicBool>,
}
#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl LinkedFixture {
    fn get_managed_objects(&self) -> zbus::fdo::ManagedObjects {
        let mut objects = if self.graph { graph_fixture() } else { linked_fixture(if self.oauth { "google" } else { "imap_smtp" }, self.oauth) };
        if !self.enabled.load(Ordering::Relaxed) {
            for interfaces in objects.values_mut() {
                interfaces.get_mut(ACCOUNT).unwrap().insert("MailDisabled".into(), OwnedValue::from(true));
            }
        }
        objects
    }
}
struct Credentials {
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    allowed: std::sync::Arc<AtomicBool>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.Account")]
impl Credentials {
    fn ensure_credentials(&self) -> zbus::fdo::Result<i32> {
        self.calls.lock().unwrap().push("ensure".into());
        if self.allowed.load(Ordering::Relaxed) { Ok(3600) } else { Err(zbus::fdo::Error::Failed("synthetic-private-error".into())) }
    }
}
struct Passwords {
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.PasswordBased")]
impl Passwords {
    fn get_password(&self, id: &str) -> String {
        self.calls.lock().unwrap().push(id.into());
        format!("synthetic-{id}")
    }
}
struct AccessToken {
    token: &'static str,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.OAuth2Based")]
impl AccessToken {
    fn get_access_token(&self) -> (String, i32) {
        self.calls.lock().unwrap().push("token".into());
        (self.token.into(), 3600)
    }
}

static PRIVATE_BUS: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_linked_credentials_refresh_without_local_login_or_storage() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _lock = PRIVATE_BUS.lock().unwrap();
    for oauth in [false, true] {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let enabled = std::sync::Arc::new(AtomicBool::new(true));
        let allowed = std::sync::Arc::new(AtomicBool::new(true));
        let path = "/org/gnome/OnlineAccounts/Accounts/account_1";
        let server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(PATH, LinkedFixture { graph: false, oauth, enabled: enabled.clone() })
            .unwrap()
            .serve_at(path, Credentials { calls: calls.clone(), allowed: allowed.clone() })
            .unwrap()
            .serve_at(path, Passwords { calls: calls.clone() })
            .unwrap()
            .serve_at(path, AccessToken { token: "synthetic-mail-token", calls: calls.clone() })
            .unwrap()
            .build()
            .unwrap();
        let bus = zbus::blocking::connection::Builder::session().unwrap().method_timeout(Duration::from_secs(2)).build().unwrap();
        let accounts = read(&bus).unwrap();
        let mut request = SyncRequest { account: accounts[0].linked.clone().unwrap(), outbox: vec![Default::default()], ..Default::default() };
        assert!(calls.lock().unwrap().is_empty(), "discovery does not read credentials");
        let cancelled = AtomicBool::new(false);
        for _ in 0..2 {
            let resolved = resolve(&bus, &request, &cancelled).unwrap();
            assert!(resolved.account.goa.is_none());
            assert!(resolved.gmail_moves.is_empty());
            assert_eq!(resolved.smtp_username.as_deref(), Some("outgoing@example.org"));
            if oauth {
                assert_eq!(resolved.access_token, "synthetic-mail-token");
            } else {
                assert_eq!(resolved.password, "synthetic-imap-password");
                assert_eq!(resolved.smtp_password.as_deref(), Some("synthetic-smtp-password"));
            }
            let saved = serde_json::to_string(&resolved).unwrap();
            let debug = format!("{resolved:?}");
            for secret in ["synthetic-imap-password", "synthetic-smtp-password", "synthetic-mail-token"] {
                assert!(!saved.contains(secret));
                assert!(!debug.contains(secret));
            }
        }
        let count = calls.lock().unwrap().len();
        assert_eq!(count, if oauth { 4 } else { 6 });
        enabled.store(false, Ordering::Relaxed);
        assert!(resolve(&bus, &request, &cancelled).is_err());
        enabled.store(true, Ordering::Relaxed);
        request.account.email = "different@example.org".into();
        assert!(resolve(&bus, &request, &cancelled).is_err());
        request.account.email = "owner@example.org".into();
        request.account.incoming.as_mut().unwrap().host = "evil.example".into();
        assert!(resolve(&bus, &request, &cancelled).is_err());
        request.account = accounts[0].linked.clone().unwrap();
        cancelled.store(true, Ordering::Relaxed);
        assert!(resolve(&bus, &request, &cancelled).is_err());
        assert_eq!(calls.lock().unwrap().len(), count, "rejected accounts never request credentials");
        cancelled.store(false, Ordering::Relaxed);
        allowed.store(false, Ordering::Relaxed);
        let error = resolve(&bus, &request, &cancelled).unwrap_err();
        assert!(!error.to_string().contains("synthetic-private-error"));
        assert!(error.to_string().contains("GNOME Online Accounts"));
        assert_eq!(calls.lock().unwrap().len(), count + 1, "a refused EnsureCredentials never requests tokens or passwords");
        drop(server);
    }
}

fn graph_fixture() -> zbus::fdo::ManagedObjects {
    let mut objects = fixture("ms_graph", "owner@example.org");
    for interfaces in objects.values_mut() {
        interfaces.get_mut(ACCOUNT).unwrap().insert("Id".into(), string("account_1"));
        interfaces.insert(OAUTH.try_into().unwrap(), HashMap::new());
        let mail = interfaces.get_mut(MAIL).unwrap();
        mail.insert("ImapSupported".into(), OwnedValue::from(false));
        mail.insert("SmtpSupported".into(), OwnedValue::from(false));
    }
    objects
}

#[test]
fn graph_profile_links_without_inventing_mail_servers_or_a_mailcraft_registration() {
    let accounts = parse(graph_fixture()).unwrap();
    let account = accounts[0].linked.as_ref().unwrap();
    assert_eq!(account.kind, AccountKind::Microsoft);
    assert!(account.incoming.is_none() && account.outgoing.is_none() && account.oauth.is_none());
    assert_eq!(account.goa.as_ref().unwrap().provider, "ms_graph");
}

fn graph_request(account: MailAccount, mail: mailcraft_model::sync::GraphMailRequest) -> mailcraft_host::graph::GraphRequest {
    mailcraft_host::graph::GraphRequest {
        signer: Some(account.clone()),
        account,
        oauth: Default::default(),
        refresh: "must-not-renew-with-mailcraft".into(),
        access: "must-not-use-cached-token".into(),
        outbox: vec![],
        directory: false,
        calendar: None,
        changes: None,
        asks: vec![],
        settings: false,
        favorites: false,
        favorites_scopes: false,
        mail: Some(mail),
        mail_pieces: None,
        cancel: Default::default(),
    }
}

#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_graph_mail_uses_goa_for_sync_fetch_search_and_repairs() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _lock = PRIVATE_BUS.lock().unwrap();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let enabled = std::sync::Arc::new(AtomicBool::new(true));
    let allowed = std::sync::Arc::new(AtomicBool::new(true));
    let path = "/org/gnome/OnlineAccounts/Accounts/account_1";
    let _goa = zbus::blocking::connection::Builder::session()
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(PATH, LinkedFixture { graph: true, oauth: true, enabled: enabled.clone() })
        .unwrap()
        .serve_at(path, Credentials { calls: calls.clone(), allowed: allowed.clone() })
        .unwrap()
        .serve_at(path, AccessToken { token: mailcraft_net::graph::standin::TOKEN, calls: calls.clone() })
        .unwrap()
        .build()
        .unwrap();
    let server = mailcraft_net::graph::standin::StandIn::start().unwrap();
    let _route = server.route();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 2, 1_791_993_600);
    let mut session = mailcraft_engine::Session::empty();
    let id = session
        .run(
            "account.add",
            &serde_json::json!({"email": "owner@example.org", "kind": "microsoft", "goa": {"id": "account_1", "provider": "ms_graph"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let account = session.mailbox.account(id).unwrap().clone();
    assert!(account.oauth.is_none());
    let ask = mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap();
    let blocked = mailcraft_host::graph::graph(graph_request(account.clone(), ask.clone()));
    assert!(!blocked.errors.is_empty());
    assert!(
        calls.lock().unwrap().is_empty() && server.state().requests.is_empty(),
        "an ordinary host never renews GOA credentials with MailCraft OAuth"
    );
    let result = graph(graph_request(account.clone(), ask));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.tokens.is_none(), "GOA tokens never enter UI caches or local credential stores");
    let mail = result.mail.unwrap();
    assert!(mail.errors.is_empty(), "{:?}", mail.errors);
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut session, mail).unwrap();
    let message = session.mailbox.messages.values().find(|m| m.account == id).unwrap().id;
    let request = mailcraft_engine::cmd::graph_mail::graph_fetch_request(&session, id, Some(message), 1).unwrap();
    let fetched = fetch(mailcraft_host::fetch::GraphFetchJob {
        goa: Some(account.clone()),
        request,
        access: "must-not-use".into(),
        cancel: Default::default(),
    });
    assert!(fetched.errors.is_empty(), "{:?}", fetched.errors);
    assert_eq!(fetched.fetched.len(), 1);
    let searched = search(mailcraft_ui_egui::server_search::GraphSearchJob {
        goa: Some(account.clone()),
        access: "must-not-use".into(),
        request: mailcraft_model::sync::GraphSearchRequest {
            account: id,
            email: account.email.clone(),
            query: "message".into(),
            ..Default::default()
        },
    });
    assert!(searched.errors.is_empty(), "{:?}", searched.errors);
    assert_eq!(calls.lock().unwrap().iter().filter(|s| *s == "token").count(), 3);
    // Reads, drafts and sending all use the same managed-mail credential path.
    session.view.folder = session.mailbox.folder_of_kind(id, mailcraft_model::FolderKind::Inbox).unwrap();
    session.run("mail.flag", &serde_json::json!({"id": message})).unwrap();
    session
        .run("mail.new", &serde_json::json!({"to": "recipient@example.org", "subject": "GOA synthetic send", "body": "Synthetic mail only."}))
        .unwrap();
    session.run("compose.send", &serde_json::json!({})).unwrap();
    let result = graph(graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap()));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let mail = result.mail.unwrap();
    assert!(mail.errors.is_empty(), "{:?}", mail.errors);
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut session, mail).unwrap();
    assert_eq!(server.state().sent.len(), 1);
    assert!(
        server
            .state()
            .requests
            .iter()
            .all(|r| !r.path.iter().any(|part| ["oauth2", "calendars", "contacts", "todo", "mailboxSettings"].contains(&part.as_str())))
    );
    // A bounded 401 renewal asks GOA again, never MailCraft's OAuth endpoint.
    let token_calls = calls.lock().unwrap().iter().filter(|s| *s == "token").count();
    server.state().token = "no-longer-valid".into();
    let result = graph(graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap()));
    assert!(result.tokens.is_none());
    assert!(!result.errors.is_empty() || result.mail.as_ref().is_some_and(|m| !m.errors.is_empty()));
    assert_eq!(calls.lock().unwrap().iter().filter(|s| *s == "token").count(), token_calls + 2);
    server.state().token = mailcraft_net::graph::standin::TOKEN.into();
    let before = server.state().requests.len();
    let credential_calls = calls.lock().unwrap().len();
    let mut outside_mail = graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap());
    outside_mail.directory = true;
    assert!(!graph(outside_mail).errors.is_empty());
    assert_eq!(calls.lock().unwrap().len(), credential_calls);
    assert_eq!(server.state().requests.len(), before);
    let cancelled = graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap());
    cancelled.cancel.store(true, Ordering::Relaxed);
    assert!(!graph(cancelled).errors.is_empty());
    assert_eq!(calls.lock().unwrap().len(), credential_calls);
    assert_eq!(server.state().requests.len(), before);
    enabled.store(false, Ordering::Relaxed);
    let result = graph(graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap()));
    assert!(!result.errors.is_empty());
    assert!(result.tokens.is_none() && !result.consent_needed && !result.sign_in_refused);
    assert_eq!(server.state().requests.len(), before, "disabled GOA mail reaches no Graph endpoint");
    enabled.store(true, Ordering::Relaxed);
    allowed.store(false, Ordering::Relaxed);
    let result = graph(graph_request(account.clone(), mailcraft_engine::cmd::graph_mail::graph_mail_request(&session, id).unwrap()));
    assert!(!result.errors.is_empty());
    assert_eq!(server.state().requests.len(), before, "a refused GOA sign-in reaches no Graph endpoint");
    assert!(result.errors.iter().all(|s| !s.contains("synthetic-private-error")));
}
