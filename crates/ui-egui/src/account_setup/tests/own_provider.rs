// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account that signs in with an own identity provider (OAuth 2.0): set up from Add Account
//! with Discover and a client secret, and a built-in account turned into one after a question.
//! Invented example.org addresses; nothing leaves the test.

use mailcraft_model::{ClientAuth, OAuthProvider, SaslChoice};

use super::*;
use crate::OAuthFound;

const ISSUER: &str = "https://id.example.org/realms/mail";

type Requests = Log<(OAuth, String)>;

/// A host that signs owners in and says yes; the log is the settings and client secret each
/// sign-in was handed.
fn signing_in(app: &mut MailApp) -> Requests {
    let asked: Requests = Default::default();
    let log = asked.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.oauth.clone(), req.client_secret.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(signed_in("Alice"))).unwrap();
        rx
    }));
    asked
}

/// A host that finds the identity provider's addresses; the log is the issuers it was asked about.
fn discovering(app: &mut MailApp, outside: &[&str]) -> Log<String> {
    let asked: Log<String> = Default::default();
    let log = asked.clone();
    let outside: Vec<String> = outside.iter().map(|h| h.to_string()).collect();
    app.services.oauth_discover = Some(Box::new(move |issuer: String| {
        log.lock().unwrap().push(issuer.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(OAuthFound {
            authorize: format!("{issuer}/protocol/openid-connect/auth"),
            // (Outside hosts are those of addresses found, as Discover finds them: the token
            // address on the first, when there is one.)
            token: outside.first().map_or_else(|| format!("{issuer}/protocol/openid-connect/token"), |host| format!("https://{host}/token")),
            revoke: String::new(),
            outside: outside.clone(),
            no_refresh: false,
            // A provider that lists only the form for a secret, and names itself in its answers.
            client_auth: ClientAuth::Post,
            issuer_in_response: true,
            // And one that lists its scopes without offline_access.
            base_scopes: Some("openid email".into()),
        }))
        .unwrap();
        rx
    }));
    asked
}

/// The window with the wizard open, tall enough for every OAuth setting to be on screen.
fn open_tall(mut app: MailApp, mut params: Value) -> Harness<'static, MailApp> {
    params["name"] = json!("account");
    app.execute("ui.dialog", params).unwrap();
    window(app, (1280.0, 2400.0))
}

/// The wizard on the password page for an address at a domain nobody knows.
fn at_password(a: MailApp) -> Harness<'static, MailApp> {
    let mut h = open_tall(a, json!({}));
    connect(&mut h, "alice@unknown.example");
    assert_eq!(page(&h), Some(Page::Password));
    h
}

#[test]
fn a_new_account_signs_in_with_its_own_identity_provider_found_from_its_issuer() {
    let mut a = app(false);
    let requests = signing_in(&mut a);
    let asked = discovering(&mut a, &[]);
    let mut h = at_password(a);
    click(&mut h, "Sign in with your identity provider (OAuth 2.0) instead");
    assert!(wizard(&h).unwrap().oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Custom));
    assert!(says(&h, "Your own identity provider"));
    // The issuer, then Discover: only the issuer goes out.
    wizard_mut(&mut h).oauth.as_mut().unwrap().issuer = ISSUER.into();
    h.run_steps(2);
    click(&mut h, "Discover");
    h.run_steps(3);
    assert_eq!(*asked.lock().unwrap(), [ISSUER]);
    let oauth = wizard(&h).unwrap().oauth.clone().unwrap();
    assert_eq!(oauth.token, format!("{ISSUER}/protocol/openid-connect/token"));
    assert!(says(&h, "Found: sign-in at id.example.org"));
    // How it takes a secret and that it names itself come with the addresses (RFC 6749 §2.3.1, RFC 9207).
    assert_eq!((oauth.client_auth, oauth.issuer_in_response), (ClientAuth::Post, true));
    assert_eq!(oauth.base_scopes.as_deref(), Some("openid email"));
    assert!(says(&h, "Client secret sent"));
    // The client, and its secret typed in the masked field.
    wizard_mut(&mut h).client_id = "mailcraft".into();
    type_password(&mut h);
    click(&mut h, "Connect");
    // Signed in with the settings on screen and the secret typed.
    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "{:?}", wizard(&h).map(|w| w.error.clone()));
    let (sent, secret) = &requests[0];
    assert_eq!((sent.provider, sent.client_id.as_str(), secret.as_str()), (OAuthProvider::Custom, "mailcraft", TYPED));
    // The account is there, with its own provider and the word that a secret is kept.
    let account = h.state().session.mailbox.accounts.iter().find(|x| x.email == "alice@unknown.example").cloned().expect("the account is added");
    let oauth = account.oauth.clone().unwrap();
    assert_eq!((oauth.provider, oauth.issuer.as_str(), oauth.has_secret), (OAuthProvider::Custom, ISSUER, true));
    assert_eq!((oauth.client_auth, oauth.issuer_in_response, sent.client_auth), (ClientAuth::Post, true, ClientAuth::Post));
    // The secret is the account's for the session; never in the mailbox, a command's answer or Debug.
    assert_eq!(h.state().client_secret_of(account.id), TYPED);
    let mailbox = serde_json::to_string(&h.state().session.mailbox).unwrap();
    let listed = h.state_mut().execute("account.list", json!({})).unwrap().to_string();
    assert!(!mailbox.contains(TYPED) && !listed.contains(TYPED) && !format!("{account:?}").contains(TYPED));
}

#[test]
fn addresses_outside_the_providers_domain_are_agreed_to_first() {
    let mut a = app(false);
    let requests = signing_in(&mut a);
    discovering(&mut a, &["tokens.example.net"]);
    let mut h = at_password(a);
    click(&mut h, "Sign in with your identity provider (OAuth 2.0) instead");
    wizard_mut(&mut h).oauth.as_mut().unwrap().issuer = ISSUER.into();
    wizard_mut(&mut h).client_id = "mailcraft".into();
    h.run_steps(2);
    click(&mut h, "Discover");
    h.run_steps(3);
    assert!(says(&h, "tokens.example.net"));
    click(&mut h, "Connect");
    assert!(requests.lock().unwrap().is_empty());
    assert!(wizard(&h).unwrap().error.contains("outside"), "{}", wizard(&h).unwrap().error);
    h.get_by_role_and_label(Role::CheckBox, "I trust these addresses").click();
    h.run_steps(3);
    click(&mut h, "Connect");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn settings_that_cant_be_used_are_said_and_nothing_is_signed_in() {
    let mut a = app(false);
    let requests = signing_in(&mut a);
    let mut h = at_password(a);
    click(&mut h, "Sign in with your identity provider (OAuth 2.0) instead");
    // No client id; no addresses; an address that isn't https; a port that isn't one.
    click(&mut h, "Connect");
    assert!(!wizard(&h).unwrap().error.is_empty());
    wizard_mut(&mut h).client_id = "mailcraft".into();
    click(&mut h, "Connect");
    assert!(wizard(&h).unwrap().error.contains("Discover"), "{}", wizard(&h).unwrap().error);
    {
        let w = wizard_mut(&mut h);
        let oauth = w.oauth.as_mut().unwrap();
        oauth.authorize = "https://id.example.org/auth".into();
        oauth.token = "http://id.example.org/token".into();
    }
    click(&mut h, "Connect");
    assert!(wizard(&h).unwrap().error.contains("https"), "{}", wizard(&h).unwrap().error);
    wizard_mut(&mut h).oauth.as_mut().unwrap().token = "https://id.example.org/token".into();
    wizard_mut(&mut h).own.port = "70000".into();
    click(&mut h, "Connect");
    assert!(wizard(&h).unwrap().error.contains("redirect port"), "{}", wizard(&h).unwrap().error);
    assert!(requests.lock().unwrap().is_empty());
    // A password instead, after all.
    click(&mut h, "Use a password instead");
    assert!(wizard(&h).unwrap().oauth.is_none());
}

#[test]
fn a_google_account_becomes_one_with_its_own_provider_only_after_the_question() {
    let mut a = app(false);
    let id = a
        .execute(
            "account.add",
            json!({"email": "ada@gmail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}, "auth": "oauth2", "provider": "google"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = open_tall(a, json!({"id": id}));
    assert_eq!(page(&h), Some(Page::Settings));
    // Its built-in values are shown.
    assert!(says(&h, "Google (built in)") && says(&h, "https://oauth2.googleapis.com/token"));
    // Where a built-in provider's token goes isn't only the mail servers: no list claims so.
    assert!(!says(&h, "The sign-in is only sent to"));
    // While its servers are Gmail's it can't be one: an own provider's sign-in never goes there,
    // and the owner is told to change the servers first (#1844 review).
    assert!(says(&h, "first change the incoming and outgoing servers to your own"));
    assert!(h.query_by_role_and_label(Role::Button, "Use your own identity provider instead…").is_none());
    {
        let w = wizard_mut(&mut h);
        w.incoming.host = "mail.example.org".into();
        w.outgoing.host = "mail.example.org".into();
    }
    h.run_steps(3);
    assert!(!says(&h, "first change the incoming and outgoing servers to your own"));
    click(&mut h, "Use your own identity provider instead…");
    assert!(says(&h, "will no longer be Google's"));
    click(&mut h, "Keep the built-in settings");
    assert_eq!(wizard(&h).unwrap().oauth.as_ref().unwrap().provider, OAuthProvider::Google);
    click(&mut h, "Use your own identity provider instead…");
    click(&mut h, "Use my own identity provider");
    // Google's client is the build's: an own provider needs its own before anything is saved.
    assert!(wizard(&h).unwrap().edit_command().is_none());
    wizard_mut(&mut h).client_id = "my-own-client".into();
    let (_, params) = wizard(&h).unwrap().edit_command().expect("a change to save");
    assert_eq!((params["asCustom"].as_bool(), params["provider"].as_str()), (Some(true), None));
    assert_eq!(params["tokenUrl"], "https://oauth2.googleapis.com/token");
    click(&mut h, "Save");
    let oauth = h.state().session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!((oauth.provider, oauth.client_id.as_str(), oauth.mechanism), (OAuthProvider::Custom, "my-own-client", SaslChoice::XOAuth2));
    let saved = h.state().session.mailbox.account(id).unwrap();
    assert_eq!(saved.microsoft_or_google_server(), None);
}

/// A Microsoft account kept in step through Microsoft Graph has no mail servers, and its token goes
/// to Graph: its settings never say the sign-in goes only to some mail servers (#1844 review).
#[test]
fn a_microsoft_account_on_graph_names_no_servers_its_sign_in_is_only_sent_to() {
    let mut a = app(false);
    let id = a.execute("account.add", json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let h = open_tall(a, json!({"id": id}));
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(!says(&h, "The sign-in is only sent to"));
}

#[test]
fn an_own_providers_settings_are_edited_and_its_secret_replaced_or_forgotten() {
    let mut a = app(false);
    let id = a
        .execute(
            "account.add",
            json!({"email": "alice@example.org", "incoming": {"host": "mail.example.org"}, "outgoing": {"host": "mail.example.org"}, "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER, "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token"), "hasSecret": true}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = open_tall(a, json!({"id": id}));
    assert!(says(&h, "Your own identity provider") && says(&h, "The sign-in is only sent to: mail.example.org"));
    // Nothing changed: nothing to save.
    assert!(wizard(&h).unwrap().edit_command().is_none());
    wizard_mut(&mut h).oauth.as_mut().unwrap().mechanism = SaslChoice::OAuthBearer;
    type_password(&mut h);
    click(&mut h, "Save");
    let oauth = h.state().session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!((oauth.mechanism, oauth.has_secret), (SaslChoice::OAuthBearer, true));
    assert_eq!(h.state().client_secret_of(id), TYPED);
    // Forgotten.
    let mut h = open_tall(app_with(h), json!({"id": id}));
    h.get_by_role_and_label(Role::CheckBox, "Forget the client secret").click();
    h.run_steps(3);
    click(&mut h, "Save");
    assert!(!h.state().session.mailbox.account(id).unwrap().oauth.as_ref().unwrap().has_secret);
    assert_eq!(h.state().client_secret_of(id), "");
}

/// A kept client secret is kept for the settings it was given with: when the revocation address
/// (or the issuer, the token address, the client or a server) changes, the dialog says it has to
/// be entered again, and saving without it says none is kept (#1844 review).
#[test]
fn a_secret_kept_for_settings_that_change_is_asked_for_again() {
    let mut a = app(false);
    let id = a
        .execute(
            "account.add",
            json!({"email": "alice@example.org", "incoming": {"host": "mail.example.org"}, "outgoing": {"host": "mail.example.org"}, "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER, "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token"), "hasSecret": true}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = open_tall(a, json!({"id": id}));
    assert!(!says(&h, "enter it again to use it with these"));
    wizard_mut(&mut h).oauth.as_mut().unwrap().revoke = format!("{ISSUER}/revoke");
    h.run_steps(3);
    assert!(says(&h, "enter it again to use it with these"));
    let (_, params) = wizard(&h).unwrap().edit_command().expect("a change to save");
    assert_eq!(params["hasSecret"], false);
    click(&mut h, "Save");
    assert!(!h.state().session.mailbox.account(id).unwrap().oauth.as_ref().unwrap().has_secret);
}

/// The app a harness holds, for another dialog.
fn app_with(h: Harness<'static, MailApp>) -> MailApp {
    let mut a = h.into_state();
    a.dialog = None;
    a
}

#[test]
fn removing_the_account_gives_the_refresh_token_back_where_the_provider_says() {
    let mut a = app(false);
    let given: Log<(String, String, String)> = Default::default();
    let log = given.clone();
    a.services.revoke_sign_in = Some(Box::new(move |oauth: OAuth, secret: String, refresh: String| {
        log.lock().unwrap().push((oauth.revoke, secret, refresh));
    }));
    let add = |a: &mut MailApp, revoke: &str| {
        let v = json!({"email": "alice@example.org", "incoming": {"host": "mail.example.org"}, "outgoing": {"host": "mail.example.org"}, "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER, "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token"), "revokeUrl": revoke, "hasSecret": true});
        a.execute("account.add", v).unwrap()["id"].as_u64().unwrap()
    };
    let id = add(&mut a, &format!("{ISSUER}/revoke"));
    a.adopt_tokens(id, signed_in("Alice"));
    a.set_client_secret(id, TYPED);
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(*given.lock().unwrap(), [(format!("{ISSUER}/revoke"), TYPED.to_string(), "ref-1".to_string())]);
    assert_eq!(a.client_secret_of(id), "");
    // A provider that names nowhere to give it back is told nothing.
    let id = add(&mut a, "");
    a.adopt_tokens(id, signed_in("Alice"));
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(given.lock().unwrap().len(), 1);
}

/// An address typed over needs no agreement; one still as Discover found it on an outside host
/// does (#1844 review).
#[test]
fn only_an_outside_address_still_as_found_needs_the_agreement() {
    let mut a = app(false);
    let requests = signing_in(&mut a);
    discovering(&mut a, &["tokens.example.net"]);
    let mut h = at_password(a);
    click(&mut h, "Sign in with your identity provider (OAuth 2.0) instead");
    wizard_mut(&mut h).oauth.as_mut().unwrap().issuer = ISSUER.into();
    wizard_mut(&mut h).client_id = "mailcraft".into();
    h.run_steps(2);
    click(&mut h, "Discover");
    h.run_steps(3);
    // The sign-in address typed over: the outside token address still asks.
    wizard_mut(&mut h).oauth.as_mut().unwrap().authorize = format!("{ISSUER}/other/auth");
    h.run_steps(2);
    click(&mut h, "Connect");
    assert!(requests.lock().unwrap().is_empty());
    assert!(wizard(&h).unwrap().error.contains("outside"), "{}", wizard(&h).unwrap().error);
    // The token address typed over too: nothing left to agree to.
    wizard_mut(&mut h).oauth.as_mut().unwrap().token = format!("{ISSUER}/other/token");
    h.run_steps(2);
    click(&mut h, "Connect");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

/// What Discover found for one issuer isn't kept once the issuer is another (#1844 review).
#[test]
fn another_issuer_takes_away_what_discover_found() {
    let mut a = app(false);
    let _requests = signing_in(&mut a);
    discovering(&mut a, &[]);
    let mut h = at_password(a);
    click(&mut h, "Sign in with your identity provider (OAuth 2.0) instead");
    wizard_mut(&mut h).oauth.as_mut().unwrap().issuer = ISSUER.into();
    wizard_mut(&mut h).client_id = "mailcraft".into();
    h.run_steps(2);
    click(&mut h, "Discover");
    h.run_steps(3);
    assert!(!wizard(&h).unwrap().oauth.as_ref().unwrap().token.is_empty());
    wizard_mut(&mut h).oauth.as_mut().unwrap().issuer = "https://id.example.org/realms/other".into();
    h.run_steps(2);
    let oauth = wizard(&h).unwrap().oauth.clone().unwrap();
    assert!(oauth.token.is_empty() && oauth.authorize.is_empty(), "{oauth:?}");
    assert!(!oauth.issuer_in_response && oauth.base_scopes.is_none());
}
