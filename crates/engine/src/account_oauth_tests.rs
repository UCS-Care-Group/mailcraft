// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account that signs in with an own identity provider, through the account commands: what is
//! taken, what is refused, and that Microsoft's and Google's built-in values stay theirs.

use mailcraft_model::{OAuthProvider, SaslChoice};
use serde_json::{Value, json};

use crate::Session;

const ISSUER: &str = "https://id.example.org/realms/mail";

fn own(extra: Value) -> Value {
    let mut v = json!({
        "email": "alice@example.org",
        "username": "alice",
        "incoming": {"host": "mail.example.org", "port": 993},
        "outgoing": {"host": "mail.example.org", "port": 465},
        "auth": "oauth2",
        "provider": "custom",
        "clientId": "mailcraft",
        "issuer": ISSUER,
        "authorizeUrl": format!("{ISSUER}/protocol/openid-connect/auth"),
        "tokenUrl": format!("{ISSUER}/protocol/openid-connect/token"),
    });
    if let (Some(v), Value::Object(extra)) = (v.as_object_mut(), extra) {
        v.extend(extra);
    }
    v
}

fn add(s: &mut Session, v: &Value) -> u64 {
    s.run("account.add", v).unwrap()["id"].as_u64().unwrap()
}

#[test]
fn an_imap_account_is_set_up_with_its_own_identity_provider() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({"scopes": "mail", "mechanism": "oauthbearer", "redirectPort": 53682})));
    let a = s.mailbox.account(id).unwrap();
    let oauth = a.oauth.as_ref().unwrap();
    assert_eq!((oauth.provider, oauth.client_id.as_str(), oauth.issuer.as_str()), (OAuthProvider::Custom, "mailcraft", ISSUER));
    assert_eq!((oauth.mechanism, oauth.redirect_port, oauth.scopes.as_str()), (SaslChoice::OAuthBearer, Some(53_682), "mail"));
    assert!(oauth.ready().is_ok());
    // account.list says how it signs in, and nothing secret.
    let listed = s.run("account.list", &json!({})).unwrap();
    let shown = listed.to_string();
    assert!(shown.contains("\"provider\":\"custom\"") && shown.contains(ISSUER), "{shown}");
    assert!(!shown.to_ascii_lowercase().contains("secret\":\""), "{shown}");
}

/// A client id is whatever visible ASCII the provider issued (RFC 6749 Appendix A.1), a colon
/// among it (#1844 review); a tenant stays a word.
#[test]
fn a_provider_issued_client_id_with_punctuation_is_taken() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({"clientId": "mailcraft:desktop"})));
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().client_id, "mailcraft:desktop");
    assert!(s.run("account.edit", &json!({"id": id, "tenant": "a:b"})).is_err());
}

#[test]
fn what_cant_be_used_is_refused() {
    let mut s = Session::empty();
    for extra in [
        json!({"clientId": ""}),
        json!({"clientId": "has space"}),
        json!({"clientId": "x".repeat(201)}),
        json!({"tokenUrl": "http://id.example.org/token"}),
        json!({"authorizeUrl": "https://user:pw@id.example.org/auth"}),
        json!({"issuer": "https://id.example.org/?x"}),
        json!({"scopes": "a\"b"}),
        json!({"mechanism": "plain"}),
        json!({"mechanism": 3}),
        json!({"redirectPort": 0}),
        json!({"redirectPort": 70000}),
        json!({"tokenUrl": 5}),
        json!({"clientSecret": "s3cret"}),
    ] {
        let error = s.run("account.add", &own(extra.clone())).unwrap_err().to_string();
        assert!(!error.contains("s3cret"), "{error}");
        assert!(s.mailbox.accounts.iter().all(|a| !a.signs_in_with_own_provider()), "{extra}");
    }
    // Only an account with IMAP and SMTP servers.
    let no_servers = own(json!({"incoming": null, "outgoing": null}));
    assert!(s.run("account.add", &no_servers).is_err());
    // (A JMAP account has no such sign-in: the settings go with the servers.)
    let jmap = own(json!({"kind": "jmap", "jmap": {"session": "https://jmap.example.org/session"}}));
    let id = add(&mut s, &jmap);
    assert!(s.mailbox.account(id).unwrap().oauth.is_none());
}

#[test]
fn microsofts_and_googles_built_in_values_are_changed_only_by_becoming_an_own_provider() {
    let mut s = Session::empty();
    let google = json!({"email": "ada@gmail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}, "auth": "oauth2", "provider": "google"});
    let id = add(&mut s, &google);
    // An address of its own is refused as it is…
    let error = s.run("account.edit", &json!({"id": id, "tokenUrl": "https://id.example.org/token"})).unwrap_err().to_string();
    assert!(error.contains("asCustom"), "{error}");
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().provider, OAuthProvider::Google);
    // …and taken when it is to be an own provider, from the values it had.
    s.run("account.edit", &json!({"id": id, "asCustom": true, "clientId": "mine", "tokenUrl": "https://id.example.org/token"})).unwrap();
    let oauth = s.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!(oauth.provider, OAuthProvider::Custom);
    assert_eq!((oauth.token.as_str(), oauth.authorize.as_str()), ("https://id.example.org/token", "https://accounts.google.com/o/oauth2/v2/auth"));
    assert!(oauth.effective().scopes.contains("https://mail.google.com/"));
    // Its client id and redirect port can change as they are.
    let id2 =
        add(&mut s, &json!({"email": "noor@example.org", "incoming": {"host": "outlook.office365.com"}, "auth": "oauth2", "provider": "microsoft"}));
    s.run("account.edit", &json!({"id": id2, "clientId": "my-app", "redirectPort": 8400})).unwrap();
    let ms = s.mailbox.account(id2).unwrap().oauth.clone().unwrap();
    assert_eq!((ms.provider, ms.client_id.as_str(), ms.redirect_port), (OAuthProvider::Microsoft, "my-app", Some(8_400)));
}

#[test]
fn going_back_to_microsoft_or_google_takes_the_own_providers_addresses_away() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({"scopes": "mail"})));
    s.run("account.edit", &json!({"id": id, "provider": "google"})).unwrap();
    let oauth = s.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!((oauth.provider, oauth.issuer.as_str(), oauth.token.as_str(), oauth.scopes.as_str()), (OAuthProvider::Google, "", "", ""));
}

#[test]
fn the_built_in_values_of_each_provider_are_shown_and_restored() {
    let mut s = Session::empty();
    let ms = s.run("account.oauthDefaults", &json!({"provider": "microsoft", "tenant": "organizations"})).unwrap();
    assert_eq!(ms["tokenUrl"], "https://login.microsoftonline.com/organizations/oauth2/v2.0/token");
    assert_eq!((ms["mechanism"].as_str(), ms["builtInClient"].as_bool()), (Some("xoauth2"), Some(true)));
    assert!(ms["hosts"].as_array().unwrap().iter().any(|h| h == "outlook.office365.com"));
    let google = s.run("account.oauthDefaults", &json!({"provider": "google"})).unwrap();
    assert_eq!(google["revokeUrl"], "https://oauth2.googleapis.com/revoke");
    let custom = s.run("account.oauthDefaults", &json!({"provider": "custom"})).unwrap();
    assert_eq!(
        (custom["scopes"].as_str(), custom["tokenUrl"].as_str(), custom["mechanism"].as_str()),
        (Some("openid email offline_access"), Some(""), Some("auto"))
    );
    assert!(s.run("account.oauthDefaults", &json!({"provider": "carrier"})).is_err());

    let id = add(&mut s, &own(json!({"scopes": "mail", "mechanism": "xoauth2", "redirectPort": 9000})));
    let back = s.run("account.oauthRestoreDefaults", &json!({"id": id})).unwrap();
    assert_eq!(back["oauth"]["provider"], "custom");
    let oauth = s.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!((oauth.scopes.as_str(), oauth.mechanism, oauth.redirect_port, oauth.client_id.as_str()), ("", SaslChoice::Auto, None, "mailcraft"));
    let local = add(&mut s, &json!({"email": "local@example.org"}));
    assert!(s.run("account.oauthRestoreDefaults", &json!({"id": local})).is_err());
}

#[test]
fn preset_can_be_asked_for_an_own_provider() {
    let mut s = Session::empty();
    let preset = s.run("account.preset", &json!({"email": "alice@example.org", "provider": "custom"})).unwrap();
    assert_eq!((preset["auth"].as_str(), preset["provider"].as_str()), (Some("oauth2"), Some("custom")));
}

#[test]
fn an_identity_provider_a_domains_document_names_is_offered_never_taken_by_itself() {
    let mut s = Session::empty();
    let found = |issuer: &str| json!({"email": "alice@unknown-domain.example", "found": {"incoming": {"host": "mail.unknown-domain.example", "security": "tls", "via": "autoconfig"}, "oauthIssuer": issuer}});
    let preset = s.run("account.preset", &found("https://id.unknown-domain.example/realms/mail")).unwrap();
    assert_eq!(preset["oauthIssuer"], "https://id.unknown-domain.example/realms/mail");
    // Offered, not set up: the sign-in stays a password until the owner says otherwise.
    assert_eq!(preset["auth"], "password");
    for bad in ["http://id.example.org", "javascript:x", "https://id.example.org/?q", "https://u:p@id.example.org"] {
        assert_eq!(s.run("account.preset", &found(bad)).unwrap()["oauthIssuer"], Value::Null, "{bad}");
    }
    // A provider this program knows has its own sign-in: nothing is offered for it.
    let gmail = json!({"email": "ada@gmail.com", "found": {"oauthIssuer": "https://id.example.org"}});
    assert_eq!(s.run("account.preset", &gmail).unwrap()["oauthIssuer"], Value::Null);
}

#[test]
fn how_the_client_secret_is_sent_and_whether_the_provider_names_itself_are_settings() {
    use mailcraft_model::ClientAuth;
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({})));
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().client_auth, ClientAuth::Basic);
    s.run("account.edit", &json!({"id": id, "clientAuth": "post", "issuerInResponse": true})).unwrap();
    let oauth = s.mailbox.account(id).unwrap().oauth.clone().unwrap();
    assert_eq!((oauth.client_auth, oauth.issuer_in_response), (ClientAuth::Post, true));
    // Which standard scopes are asked for: as Discover found, and all three again with null.
    s.run("account.edit", &json!({"id": id, "baseScopes": "openid email"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().base_scopes.as_deref(), Some("openid email"));
    s.run("account.edit", &json!({"id": id, "baseScopes": null})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().base_scopes, None);
    for bad in [
        json!({"clientAuth": "private_key_jwt"}),
        json!({"clientAuth": 1}),
        json!({"issuerInResponse": "yes"}),
        json!({"baseScopes": "openid admin"}),
        json!({"baseScopes": 3}),
    ] {
        let mut v = bad.clone();
        v["id"] = json!(id);
        assert!(s.run("account.edit", &v).is_err(), "{bad}");
    }
    // Microsoft's are built in: changed only by becoming an own provider.
    let ms =
        add(&mut s, &json!({"email": "bo@example.org", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}));
    assert!(s.run("account.edit", &json!({"id": ms, "clientAuth": "post"})).is_err());
    assert!(s.run("account.edit", &json!({"id": ms, "baseScopes": "openid"})).is_err());
    // What each provider uses is said.
    assert_eq!(s.run("account.oauthDefaults", &json!({"provider": "custom"})).unwrap()["clientAuth"], "basic");
}

/// What an agent is asked to allow names a change of where the account signs in even when it
/// lies past what is shown of a long address, or in the revocation address (#1844 review).
#[test]
fn an_agent_is_told_of_a_sign_in_change_however_late_in_the_address() {
    let mut s = Session::empty();
    let long = format!("{ISSUER}/{}/one", "a".repeat(100));
    let id = add(&mut s, &own(json!({"tokenUrl": long})));
    let other = long.replace("/one", "/two");
    let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "tokenUrl": other}));
    assert!(said.contains("its own identity provider") && said.contains("/two"), "{said}");
    let revoke = crate::agent::describe(&s, "account.edit", &json!({"id": id, "revokeUrl": "https://elsewhere.example.org/revoke"}));
    assert!(revoke.contains("its own identity provider") && revoke.contains("elsewhere.example.org/revoke"), "{revoke}");
    let client = crate::agent::describe(&s, "account.edit", &json!({"id": id, "clientId": "other-client"}));
    assert!(client.contains("other-client"), "{client}");
}

/// What an agent is asked to allow names switching the RFC 9207 check off (or on) and a change of
/// how the client secret goes to the provider (#1844 review).
#[test]
fn an_agent_is_told_when_the_issuer_check_or_the_client_authentication_changes() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({"issuerInResponse": true})));
    let off = crate::agent::describe(&s, "account.edit", &json!({"id": id, "issuerInResponse": false}));
    assert!(off.contains("RFC 9207 check is switched off"), "{off}");
    let post = crate::agent::describe(&s, "account.edit", &json!({"id": id, "clientAuth": "post"}));
    assert!(post.contains("in the form") && !post.contains("RFC 9207"), "{post}");
    // Nothing of either when neither changes.
    let same = crate::agent::describe(&s, "account.edit", &json!({"id": id, "issuerInResponse": true, "clientAuth": "basic"}));
    assert!(!same.contains("RFC 9207") && !same.contains("client secret"), "{same}");
    s.run("account.edit", &json!({"id": id, "issuerInResponse": false})).unwrap();
    let on = crate::agent::describe(&s, "account.edit", &json!({"id": id, "issuerInResponse": true}));
    assert!(on.contains("has to name itself"), "{on}");
}

/// A client secret is kept under the settings it is for: when they change it is no longer found,
/// and the account stops saying one is kept, unless the change comes with one (#1844 review).
#[test]
fn a_kept_client_secret_follows_the_settings_it_is_for() {
    let mut s = Session::empty();
    let kept = |s: &Session, id: u64| s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().has_secret;
    let id = add(&mut s, &own(json!({"hasSecret": true})));
    s.run("account.edit", &json!({"id": id, "displayName": "Alice", "mechanism": "xoauth2"})).unwrap();
    assert!(kept(&s, id), "a change that leaves the settings it is for keeps it");
    s.run("account.edit", &json!({"id": id, "revokeUrl": format!("{ISSUER}/revoke-elsewhere")})).unwrap();
    assert!(!kept(&s, id), "another revocation address");
    s.run("account.edit", &json!({"id": id, "hasSecret": true})).unwrap();
    s.run("account.edit", &json!({"id": id, "issuer": "https://other.example.org/realms/mail"})).unwrap();
    assert!(!kept(&s, id), "another issuer");
    // A change that comes with a secret for the new settings keeps saying so.
    s.run("account.edit", &json!({"id": id, "hasSecret": true})).unwrap();
    s.run("account.edit", &json!({"id": id, "tokenUrl": format!("{ISSUER}/token2"), "hasSecret": true})).unwrap();
    assert!(kept(&s, id));
}
