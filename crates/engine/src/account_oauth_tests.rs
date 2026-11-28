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
    // …and, while it names Gmail's servers, refused even as an own provider: such a token never
    // goes there, so the account would never synchronise (#1844 review)…
    let error = s
        .run("account.edit", &json!({"id": id, "asCustom": true, "clientId": "mine", "tokenUrl": "https://id.example.org/token"}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("imap.gmail.com") && error.contains("own identity provider"), "{error}");
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().provider, OAuthProvider::Google);
    // …and taken when it is to be an own provider with servers of its own, from the values it had.
    let own_servers = json!({"incoming": {"host": "mail.example.org", "port": 993}, "outgoing": {"host": "mail.example.org", "port": 465}});
    let mut edit = json!({"id": id, "asCustom": true, "clientId": "mine", "tokenUrl": "https://id.example.org/token"});
    if let (Some(edit), Value::Object(servers)) = (edit.as_object_mut(), own_servers) {
        edit.extend(servers);
    }
    s.run("account.edit", &edit).unwrap();
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

/// What an agent is asked to allow names the permissions an own provider's next sign-in asks for
/// when its scopes or standard scopes change, and its identity provider when the issuer changes;
/// neither when they stay (#1844 review).
#[test]
fn an_agent_is_told_which_permissions_and_identity_provider_a_changed_sign_in_asks() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({"scopes": "mail"})));
    let scopes = crate::agent::describe(&s, "account.edit", &json!({"id": id, "scopes": "mail calendar"}));
    assert!(scopes.contains("it asks its identity provider for: openid email offline_access mail calendar"), "{scopes}");
    assert!(!scopes.contains("its identity provider is"), "{scopes}");
    let base = crate::agent::describe(&s, "account.edit", &json!({"id": id, "baseScopes": "openid"}));
    assert!(base.contains("it asks its identity provider for: openid mail"), "{base}");
    let issuer = crate::agent::describe(&s, "account.edit", &json!({"id": id, "issuer": "https://login.example.net/realms/other"}));
    assert!(issuer.contains("its identity provider is https://login.example.net/realms/other"), "{issuer}");
    assert!(!issuer.contains("asks its identity provider for"), "{issuer}");
    // Nothing of either when neither changes.
    let same = crate::agent::describe(&s, "account.edit", &json!({"id": id, "scopes": "mail", "displayName": "Alice"}));
    assert!(!same.contains("identity provider"), "{same}");
    // A password account moved to an own provider names them both.
    let pw = add(&mut s, &json!({"email": "noor@example.org", "incoming": {"host": "mail.example.org"}, "auth": "password"}));
    let moved = crate::agent::describe(
        &s,
        "account.edit",
        &json!({"id": pw, "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER, "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token")}),
    );
    assert!(
        moved.contains(&format!("its identity provider is {ISSUER}"))
            && moved.contains("asks its identity provider for: openid email offline_access"),
        "{moved}"
    );
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

/// An own identity provider's token never goes to Microsoft's or Google's mail servers, so an
/// account that would send it there is refused when it is set up or edited, whichever server it
/// is and whichever cloud (#1844 review).
#[test]
fn an_own_provider_is_refused_with_microsofts_or_googles_mail_servers() {
    let mut s = Session::empty();
    let before = s.mailbox.accounts.len();
    for (incoming, outgoing) in [
        ("outlook.office365.com", "mail.example.org"),
        ("mail.example.org", "smtp.office365.com"),
        ("imap.gmail.com", "smtp.gmail.com"),
        ("outlook.office365.us", "outlook.office365.us"),
        ("IMAP.GMAIL.COM.", "mail.example.org"),
    ] {
        let v = own(json!({"incoming": {"host": incoming, "port": 993}, "outgoing": {"host": outgoing, "port": 465}}));
        let error = s.run("account.add", &v).unwrap_err().to_string();
        assert!(error.contains("own identity provider"), "{incoming} {outgoing}: {error}");
        assert_eq!(s.mailbox.accounts.len(), before, "{incoming} {outgoing}");
    }
    // An own account can't be moved onto them either, and is left as it was.
    let id = add(&mut s, &own(json!({})));
    assert!(s.run("account.edit", &json!({"id": id, "outgoing": {"host": "smtp.gmail.com", "port": 465}})).is_err());
    let a = s.mailbox.account(id).unwrap();
    assert_eq!(a.outgoing.as_ref().map(|o| o.host.as_str()), Some("mail.example.org"));
    // A Microsoft account on Microsoft's servers keeps its own sign-in.
    let ms =
        add(&mut s, &json!({"email": "noor@example.org", "incoming": {"host": "outlook.office365.com"}, "auth": "oauth2", "provider": "microsoft"}));
    assert_eq!(s.mailbox.account(ms).unwrap().oauth.as_ref().unwrap().provider, OAuthProvider::Microsoft);
}

/// A long address the sign-in goes to is never shortened in its host: two that differ only at the
/// end of the domain are told apart, the path alone is shortened, and a short one is said whole
/// (#1844 review).
#[test]
fn an_agent_is_always_shown_the_whole_host_a_sign_in_goes_to() {
    let mut s = Session::empty();
    let id = add(&mut s, &own(json!({})));
    let host = format!("id.{}example.", "very-long-subdomain.".repeat(5));
    let path = format!("/realms/{}/protocol/openid-connect/token", "p".repeat(90));
    for suffix in ["org", "net"] {
        let token = format!("https://{host}{suffix}:8443{path}");
        let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "tokenUrl": token}));
        assert!(said.contains(&format!("https://{host}{suffix}:8443/realms/")), "{said}");
        assert!(said.contains("openid-connect/token") && !said.contains(&path), "the path is shortened: {said}");
    }
    let short = format!("{ISSUER}/token-elsewhere");
    let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "tokenUrl": short}));
    assert!(said.contains(&short), "{said}");
}

/// An agent's edit that points a built-in provider's sign-in at another client says so, and so
/// does one back to MailCraft's own; a change that leaves the client says nothing of it (#1844
/// review).
#[test]
fn an_agent_is_shown_a_built_in_providers_sign_in_moved_to_another_client() {
    let mut s = Session::empty();
    for provider in ["microsoft", "google"] {
        let host = if provider == "google" { "imap.gmail.com" } else { "outlook.office365.com" };
        let v = json!({"email": format!("noor.{provider}@example.org"), "incoming": {"host": host}, "auth": "oauth2", "provider": provider});
        let id = add(&mut s, &v);
        let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "clientId": "elsewhere-client"}));
        assert!(said.contains("as the client \u{201c}elsewhere-client\u{201d}"), "{provider}: {said}");
        s.run("account.edit", &json!({"id": id, "clientId": "elsewhere-client"})).unwrap();
        let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "clientId": "another-client"}));
        assert!(said.contains("as the client \u{201c}another-client\u{201d}"), "{provider}: {said}");
        let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "clientId": ""}));
        assert!(said.contains("as MailCraft's own client"), "{provider}: {said}");
        let said = crate::agent::describe(&s, "account.edit", &json!({"id": id, "displayName": "Noor"}));
        assert!(!said.contains("client"), "{provider}: {said}");
    }
}

/// `provider` microsoft or google on an account that signs in with a password is ignored, as it
/// always was (like `clientId` and `tenant`); an own identity provider is refused there, since it
/// is only ever a sign-in on its page (#1844 review).
#[test]
fn a_built_in_provider_on_a_password_account_is_ignored_and_an_own_one_refused() {
    let mut s = Session::empty();
    let v = json!({"email": "ada@example.org", "incoming": {"host": "mail.example.org"}, "auth": "password", "provider": "google"});
    let id = add(&mut s, &v);
    assert!(s.mailbox.account(id).unwrap().oauth.is_none());
    s.run("account.edit", &json!({"id": id, "provider": "microsoft"})).unwrap();
    s.run("account.edit", &json!({"id": id, "auth": "password", "provider": "google", "clientId": "x"})).unwrap();
    assert!(s.mailbox.account(id).unwrap().oauth.is_none());
    assert!(s.run("account.edit", &json!({"id": id, "provider": "custom"})).is_err());
    assert!(s.run("account.edit", &json!({"id": id, "auth": "password", "provider": "custom"})).is_err());
    assert!(s.mailbox.account(id).unwrap().oauth.is_none());
}
