// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic metadata and D-Bus services: no session accounts or provider credentials.

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

#[test]
fn introspection_only_lists_direct_bounded_names_without_expanding_entities() {
    let xml = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd"><node><node name="account_1"/><node name="account_2"><node name="nested"/></node></node>"#;
    assert_eq!(metadata::names(xml, "node", 2).unwrap(), ["account_1", "account_2"]);
    assert_eq!(metadata::names(xml, "node", 1).unwrap_err(), DiscoveryError::TooManyAccounts);
    for xml in ["", "<node>", "<other/>", "<node/><node/>", "<node><node name='&unknown;'/></node>"] {
        assert!(metadata::names(xml, "node", 10).is_err(), "{xml}");
    }
    assert!(metadata::names(&format!("{}{}", "<node>".repeat(40), "</node>".repeat(40)), "node", 256).is_err());
    assert!(metadata::names(&" ".repeat(262_145), "node", 256).is_err());
}

/// Run with dbus-run-session and MAILCRAFT_TEST_PRIVATE_BUS=1; never touches a user's session.
#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_discovery_reads_only_public_metadata() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _lock = PRIVATE_BUS.lock().unwrap();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _server = serve_metadata(
        LinkedFixture { oauth: true, enabled: std::sync::Arc::new(AtomicBool::new(true)) },
        calls.clone(),
        std::sync::Arc::new(AtomicBool::new(true)),
        Default::default(),
        "synthetic-mail-token",
    );
    assert!(available());
    let accounts = discover().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].email, "owner@example.org");
    assert!(accounts[0].linked.is_none(), "OAuth is offered only for separate setup");
    assert!(calls.lock().unwrap().is_empty(), "discovery never reads OAuth registration or credentials");
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
fn linked_configuration_requires_secure_password_based_mail() {
    for (provider, oauth, supported) in
        [("google", true, false), ("windows_live", true, false), ("ms_graph", true, false), ("unknown", true, false), ("imap_smtp", false, true)]
    {
        let accounts = parse(linked_fixture(provider, oauth)).unwrap();
        assert_eq!(accounts[0].linked.is_some(), supported, "{provider}");
    }
    for key in ["ImapSupported", "SmtpSupported", "ImapUseSsl", "SmtpUseTls", "SmtpUseAuth", "SmtpAuthPlain"] {
        let mut objects = linked_fixture("imap_smtp", false);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert(key.into(), OwnedValue::from(false));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none(), "{key}");
    }
    for key in ["ImapAcceptSslErrors", "SmtpAcceptSslErrors"] {
        let mut objects = linked_fixture("imap_smtp", false);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert(key.into(), OwnedValue::from(true));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none());
    }
    for host in ["imap.example.org:bad", "https://imap.example.org", "imap.example.org:0"] {
        let mut objects = linked_fixture("imap_smtp", false);
        for interfaces in objects.values_mut() {
            interfaces.get_mut(MAIL).unwrap().insert("ImapHost".into(), string(host));
        }
        assert!(parse(objects).unwrap()[0].linked.is_none(), "{host}");
    }
}

struct LinkedFixture {
    oauth: bool,
    enabled: std::sync::Arc<AtomicBool>,
}
impl LinkedFixture {
    fn objects(&self) -> zbus::fdo::ManagedObjects {
        let mut objects = linked_fixture(if self.oauth { "google" } else { "imap_smtp" }, self.oauth);
        if !self.enabled.load(Ordering::Relaxed) {
            for interfaces in objects.values_mut() {
                interfaces.get_mut(ACCOUNT).unwrap().insert("MailDisabled".into(), OwnedValue::from(true));
            }
        }
        objects
    }
}
struct Credentials {
    metadata: LinkedFixture,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    allowed: std::sync::Arc<AtomicBool>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.Account")]
impl Credentials {
    #[zbus(property)]
    fn id(&self) -> &str {
        "account_1"
    }
    #[zbus(property)]
    fn provider_type(&self) -> &str {
        if self.metadata.oauth { "google" } else { "imap_smtp" }
    }
    #[zbus(property)]
    fn provider_name(&self) -> &str {
        "Example provider"
    }
    #[zbus(property)]
    fn mail_disabled(&self) -> bool {
        !self.metadata.enabled.load(Ordering::Relaxed)
    }
    fn ensure_credentials(&self) -> zbus::fdo::Result<i32> {
        self.calls.lock().unwrap().push("ensure".into());
        if self.allowed.load(Ordering::Relaxed) { Ok(3600) } else { Err(zbus::fdo::Error::Failed("synthetic-private-error".into())) }
    }
}
struct Passwords {
    smtp_state: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.PasswordBased")]
impl Passwords {
    fn get_password(&self, id: &str) -> zbus::fdo::Result<String> {
        self.calls.lock().unwrap().push(id.into());
        if id == "smtp-password" {
            match self.smtp_state.load(Ordering::Relaxed) {
                1 => return Err(zbus::fdo::Error::Failed("synthetic-private-error".into())),
                2 => return Ok(String::new()),
                3 => return Ok("x".repeat(65_537)),
                _ => {}
            }
        }
        Ok(format!("synthetic-{id}"))
    }
}
struct AccessToken {
    token: &'static str,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.OAuth2Based")]
impl AccessToken {
    #[zbus(property)]
    fn client_id(&self) -> &str {
        self.calls.lock().unwrap().push("client-id".into());
        "synthetic-client"
    }
    #[zbus(property)]
    fn client_secret(&self) -> &str {
        self.calls.lock().unwrap().push("client-secret".into());
        "synthetic-registration-secret"
    }
    fn get_access_token(&self) -> (String, i32) {
        self.calls.lock().unwrap().push("token".into());
        (self.token.into(), 3600)
    }
}

struct MailMetadata {
    properties: Properties,
}
#[zbus::interface(name = "org.gnome.OnlineAccounts.Mail")]
impl MailMetadata {
    #[zbus(property, name = "EmailAddress")]
    fn email_address(&self) -> String {
        text(&self.properties, "EmailAddress").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "Name")]
    fn name(&self) -> String {
        text(&self.properties, "Name").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "ImapHost")]
    fn imap_host(&self) -> String {
        text(&self.properties, "ImapHost").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "SmtpHost")]
    fn smtp_host(&self) -> String {
        text(&self.properties, "SmtpHost").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "ImapUserName")]
    fn imap_user_name(&self) -> String {
        text(&self.properties, "ImapUserName").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "SmtpUserName")]
    fn smtp_user_name(&self) -> String {
        text(&self.properties, "SmtpUserName").unwrap_or_default().to_string()
    }
    #[zbus(property, name = "ImapSupported")]
    fn imap_supported(&self) -> bool {
        flag(&self.properties, "ImapSupported")
    }
    #[zbus(property, name = "ImapUseSsl")]
    fn imap_use_ssl(&self) -> bool {
        flag(&self.properties, "ImapUseSsl")
    }
    #[zbus(property, name = "ImapUseTls")]
    fn imap_use_tls(&self) -> bool {
        flag(&self.properties, "ImapUseTls")
    }
    #[zbus(property, name = "ImapAcceptSslErrors")]
    fn imap_accept_ssl_errors(&self) -> bool {
        flag(&self.properties, "ImapAcceptSslErrors")
    }
    #[zbus(property, name = "SmtpSupported")]
    fn smtp_supported(&self) -> bool {
        flag(&self.properties, "SmtpSupported")
    }
    #[zbus(property, name = "SmtpUseSsl")]
    fn smtp_use_ssl(&self) -> bool {
        flag(&self.properties, "SmtpUseSsl")
    }
    #[zbus(property, name = "SmtpUseTls")]
    fn smtp_use_tls(&self) -> bool {
        flag(&self.properties, "SmtpUseTls")
    }
    #[zbus(property, name = "SmtpAcceptSslErrors")]
    fn smtp_accept_ssl_errors(&self) -> bool {
        flag(&self.properties, "SmtpAcceptSslErrors")
    }
    #[zbus(property, name = "SmtpUseAuth")]
    fn smtp_use_auth(&self) -> bool {
        flag(&self.properties, "SmtpUseAuth")
    }
    #[zbus(property, name = "SmtpAuthPlain")]
    fn smtp_auth_plain(&self) -> bool {
        flag(&self.properties, "SmtpAuthPlain")
    }
    #[zbus(property, name = "SmtpAuthLogin")]
    fn smtp_auth_login(&self) -> bool {
        flag(&self.properties, "SmtpAuthLogin")
    }
    #[zbus(property, name = "SmtpAuthXoauth2")]
    fn smtp_auth_xoauth2(&self) -> bool {
        flag(&self.properties, "SmtpAuthXoauth2")
    }
}

fn serve_metadata(
    metadata: LinkedFixture,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    allowed: std::sync::Arc<AtomicBool>,
    smtp_state: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    token: &'static str,
) -> zbus::blocking::Connection {
    let oauth = metadata.oauth;
    let properties = metadata.objects().into_values().next().unwrap().remove(MAIL).unwrap();
    let path = "/org/gnome/OnlineAccounts/Accounts/account_1";
    let builder = zbus::blocking::connection::Builder::session()
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(path, Credentials { metadata, calls: calls.clone(), allowed })
        .unwrap()
        .serve_at(path, MailMetadata { properties })
        .unwrap();
    let builder = if oauth {
        builder.serve_at(path, AccessToken { token, calls }).unwrap()
    } else {
        builder.serve_at(path, Passwords { calls, smtp_state }).unwrap()
    };
    builder.build().unwrap()
}

static PRIVATE_BUS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn outgoing_credential_warning_keeps_the_ui_receiver_open_for_incoming_mail() {
    use mailcraft_model::sync::FolderDelta;
    use mailcraft_model::{Address, FolderKind, Message};
    use mailcraft_ui_egui::{MailApp, Services};
    use serde_json::json;

    let account = parse(linked_fixture("imap_smtp", false)).unwrap().remove(0).linked.unwrap();
    let mut session = mailcraft_engine::Session::empty();
    let id = session
        .run(
            "account.add",
            &json!({
                "email": account.email, "kind": account.kind, "incoming": account.incoming,
                "outgoing": account.outgoing, "username": account.username, "goa": account.goa,
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let inbox = session.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    let outbox = session.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap();
    let queued = session
        .mailbox
        .add_message(Message {
            account: id,
            folder: outbox,
            subject: "Queued synthetic mail".into(),
            from: Address::new("Example Owner", "owner@example.org"),
            to: vec![Address::new("Recipient", "recipient@example.org")],
            ..Default::default()
        })
        .unwrap();
    session.driver.set_auto_sync(false);
    let mut app = MailApp::new(
        session,
        Services {
            gnome_accounts: Some(Box::new(|| std::sync::mpsc::channel().1)),
            sync: Some(Box::new(move |request| {
                assert!(request.account.goa.is_some());
                assert_eq!(request.outbox.len(), 1);
                let (tx, rx) = std::sync::mpsc::channel();
                // The production producer's warning, followed by ordinary incoming/terminal
                // transport pieces. No real bus, mail server, password or provider is used.
                tx.send(outgoing_problem(&request, said!("Repair this account's outgoing sign-in in GNOME Online Accounts."))).unwrap();
                tx.send(SyncResult {
                    account: id,
                    email: request.account.email.clone(),
                    partial: true,
                    folders: vec![FolderDelta {
                        id: Some(inbox),
                        remote: "INBOX".into(),
                        kind: FolderKind::Inbox,
                        uid_validity: 1,
                        uid_seen: 1,
                        new: vec![Message {
                            uid: 1,
                            subject: "Incoming after SMTP warning".into(),
                            from: Address::new("Sender", "sender@example.org"),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .unwrap();
                tx.send(SyncResult { account: id, email: request.account.email, ..Default::default() }).unwrap();
                rx
            })),
            ..Default::default()
        },
    );
    app.autosave = false;
    app.execute("ui.sync", json!({"account": id})).unwrap();
    for _ in 0..8 {
        app.poll_sync();
    }
    assert!(app.session.mailbox.messages.values().any(|m| m.account == id && m.subject == "Incoming after SMTP warning"));
    assert!(app.session.mailbox.message(queued).is_some(), "the SMTP failure keeps the outgoing message queued");
    assert!(!app.syncing());
}

#[test]
#[ignore = "requires an isolated D-Bus session (see docs/gnome-accounts.md)"]
fn private_bus_linked_credentials_refresh_without_local_login_or_storage() {
    assert_eq!(std::env::var("MAILCRAFT_TEST_PRIVATE_BUS").as_deref(), Ok("1"));
    let _lock = PRIVATE_BUS.lock().unwrap();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let enabled = std::sync::Arc::new(AtomicBool::new(true));
    let allowed = std::sync::Arc::new(AtomicBool::new(true));
    let smtp_state = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server = serve_metadata(
        LinkedFixture { oauth: false, enabled: enabled.clone() },
        calls.clone(),
        allowed.clone(),
        smtp_state.clone(),
        "synthetic-mail-token",
    );
    let bus = zbus::blocking::connection::Builder::session().unwrap().method_timeout(Duration::from_secs(2)).build().unwrap();
    let accounts = read(&bus).unwrap();
    let mut request = SyncRequest { account: accounts[0].linked.clone().unwrap(), outbox: vec![Default::default()], ..Default::default() };
    request.account.display_name = "Local sender".into();
    assert!(calls.lock().unwrap().is_empty(), "discovery does not read credentials");
    let cancelled = AtomicBool::new(false);
    for _ in 0..2 {
        let (resolved, outgoing_error) = resolve(&bus, &request, &cancelled).unwrap();
        assert!(outgoing_error.is_none());
        assert!(resolved.account.goa.is_none());
        assert_eq!(resolved.account.display_name, "Local sender");
        assert!(resolved.gmail_moves.is_empty());
        assert_eq!(resolved.smtp_username.as_deref(), Some("outgoing@example.org"));
        assert!(resolved.access_token.is_empty());
        assert_eq!(resolved.password, "synthetic-imap-password");
        assert_eq!(resolved.smtp_password.as_deref(), Some("synthetic-smtp-password"));
        let saved = serde_json::to_string(&resolved).unwrap();
        let debug = format!("{resolved:?}");
        for secret in ["synthetic-imap-password", "synthetic-smtp-password", "synthetic-mail-token"] {
            assert!(!saved.contains(secret));
            assert!(!debug.contains(secret));
        }
    }
    assert_eq!(calls.lock().unwrap().len(), 6);
    for state in [1, 2, 3] {
        smtp_state.store(state, Ordering::Relaxed);
        let (resolved, error) = resolve(&bus, &request, &cancelled).unwrap();
        assert!(!error.unwrap().to_string().contains("synthetic-private-error"));
        assert_eq!(resolved.password, "synthetic-imap-password");
        assert!(resolved.outbox.is_empty() && resolved.smtp_password.is_none());
        assert_eq!(request.outbox.len(), 1, "a failed SMTP credential never removes queued mail");
    }
    smtp_state.store(0, Ordering::Relaxed);
    let count = calls.lock().unwrap().len();
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
    let mut pieces = Vec::new();
    sync(&request, &cancelled, &mut |piece| pieces.push(piece));
    assert_eq!(pieces.len(), 1);
    assert!(!pieces[0].partial && !pieces[0].errors.is_empty(), "incoming credential failures remain terminal");
    drop(server);
}

#[test]
fn persisted_link_is_matched_before_card_deduplication() {
    let objects = || {
        let mut objects = linked_fixture("imap_smtp", false);
        let mut duplicate = linked_fixture("imap_smtp", false).into_values().next().unwrap();
        duplicate.get_mut(ACCOUNT).unwrap().insert("Id".into(), string("second"));
        objects.insert(OwnedObjectPath::try_from("/org/gnome/OnlineAccounts/Accounts/second").unwrap(), duplicate);
        objects
    };
    assert_eq!(parse(objects()).unwrap().len(), 1);
    let link = GoaAccount { id: "second".into(), provider: "imap_smtp".into() };
    assert_eq!(parse_for(objects(), Some(&link)).unwrap()[0].linked.as_ref().unwrap().goa.as_ref(), Some(&link));
}
