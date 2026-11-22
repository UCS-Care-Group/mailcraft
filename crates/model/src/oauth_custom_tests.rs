// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in with an own identity provider: the settings, how they are written and read, and
//! what is refused.

use proptest::prelude::*;

use crate::oauth::{CUSTOM_SCOPE, GOOGLE_TOKEN, MICROSOFT_MAIL_SCOPE, endpoint, scopes};
use crate::{Account, AccountKind, Mailbox, OAuth, OAuthProvider, SaslChoice};

fn own() -> OAuth {
    OAuth {
        provider: OAuthProvider::Custom,
        client_id: "mailcraft".into(),
        issuer: "https://id.example.org/realms/mail".into(),
        authorize: "https://id.example.org/realms/mail/protocol/openid-connect/auth".into(),
        token: "https://id.example.org/realms/mail/protocol/openid-connect/token".into(),
        revoke: "https://id.example.org/realms/mail/protocol/openid-connect/revoke".into(),
        scopes: "mail".into(),
        mechanism: SaslChoice::OAuthBearer,
        redirect_port: Some(53_682),
        has_secret: true,
        ..OAuth::default()
    }
}

fn imap_account(oauth: OAuth) -> Account {
    Account { id: 7, email: "alice@example.org".into(), kind: AccountKind::Imap, oauth: Some(oauth), ..Default::default() }
}

#[test]
fn the_settings_go_round_and_come_back() {
    let oauth = own();
    let text = serde_json::to_string(&oauth).unwrap();
    assert_eq!(serde_json::from_str::<OAuth>(&text).unwrap(), oauth);
    assert!(
        text.contains("\"provider\":\"custom\"") && text.contains("\"mechanism\":\"oauthbearer\"") && text.contains("\"hasSecret\":true"),
        "{text}"
    );
    // A secret is never part of the settings: only that there is one.
    assert!(!text.to_ascii_lowercase().contains("secret\":\""), "{text}");
}

#[test]
fn a_file_from_before_reads_as_it_did() {
    let old: OAuth = serde_json::from_str(r#"{"provider":"google","clientId":"abc"}"#).unwrap();
    assert_eq!((old.provider, old.client_id.as_str(), old.issuer.as_str(), old.mechanism), (OAuthProvider::Google, "abc", "", SaslChoice::Auto));
    // Nothing new is written for an account that has none of it.
    assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"provider":"google","clientId":"abc"}"#);
    // Nothing said is Microsoft, as ever.
    assert_eq!(serde_json::from_str::<OAuth>("{}").unwrap().provider, OAuthProvider::Microsoft);
}

#[test]
fn a_provider_this_version_doesnt_know_is_never_microsoft() {
    for provider in [r#""carrierPigeon""#, "7", "null", r#"{"x":1}"#, r#""Microsoft365x""#] {
        let oauth: OAuth = serde_json::from_str(&format!(r#"{{"provider":{provider}}}"#)).unwrap();
        assert_eq!(oauth.provider, OAuthProvider::Custom, "{provider}");
        // With no addresses, it can't sign in anywhere.
        assert!(oauth.ready().is_err());
    }
    for (word, provider) in [("microsoft", OAuthProvider::Microsoft), ("google", OAuthProvider::Google), ("custom", OAuthProvider::Custom)] {
        assert_eq!(serde_json::from_str::<OAuth>(&format!(r#"{{"provider":"{word}"}}"#)).unwrap().provider, provider);
    }
    // A new value that can't be read doesn't stop the rest being read.
    let odd: OAuth = serde_json::from_str(r#"{"provider":"custom","clientId":"c","mechanism":"carrier","redirectPort":-4,"issuer":7}"#).unwrap();
    assert_eq!((odd.client_id.as_str(), odd.mechanism, odd.redirect_port, odd.issuer.as_str()), ("c", SaslChoice::Auto, None, ""));
}

#[test]
fn an_earlier_version_keeps_such_an_account_whole_instead_of_failing_to_open_the_mailbox() {
    let mut mb = Mailbox::default();
    mb.accounts.push(imap_account(own()));
    let file = serde_json::to_value(&mb).unwrap();
    let written = &file["accounts"][0];
    // Written with a kind earlier versions don't know: they keep such an account as its file has it.
    assert_eq!(written["kind"], "imapOwnSignIn");
    // And read back here as the IMAP account it is.
    let back: Mailbox = serde_json::from_value(file.clone()).unwrap();
    assert_eq!(back.accounts[0].kind, AccountKind::Imap);
    assert_eq!(back.accounts[0].oauth, Some(own()));
    // A Microsoft or Google one is written as ever.
    let mut plain = Mailbox::default();
    plain.accounts.push(imap_account(OAuth { provider: OAuthProvider::Google, ..OAuth::default() }));
    assert_eq!(serde_json::to_value(&plain).unwrap()["accounts"][0]["kind"], "imap");
}

#[test]
fn what_is_used_is_built_in_for_microsoft_and_google_and_the_accounts_own_otherwise() {
    let microsoft = OAuth { tenant: "contoso.example".into(), ..OAuth::default() }.effective();
    assert_eq!(microsoft.token, "https://login.microsoftonline.com/contoso.example/oauth2/v2.0/token");
    assert_eq!((microsoft.scopes.as_str(), microsoft.mechanism, microsoft.built_in_client), (MICROSOFT_MAIL_SCOPE, SaslChoice::XOAuth2, true));
    // A tenant that can't be part of an address is `common`, as in the sign-in itself.
    assert!(OAuth { tenant: "a/../b".into(), ..OAuth::default() }.effective().token.contains("/common/"));
    let google = OAuth { provider: OAuthProvider::Google, client_id: "mine".into(), ..OAuth::default() }.effective();
    assert_eq!((google.token.as_str(), google.client_id.as_str(), google.built_in_client), (GOOGLE_TOKEN, "mine", false));
    let custom = own().effective();
    assert_eq!(custom.scopes, format!("{CUSTOM_SCOPE} mail"));
    assert_eq!((custom.mechanism, custom.redirect_port, custom.hosts.is_empty()), (SaslChoice::OAuthBearer, Some(53_682), true));
    // A scope said twice is asked for once.
    assert_eq!(OAuth { scopes: "email mail mail".into(), ..own() }.effective().scopes, format!("{CUSTOM_SCOPE} mail"));
}

#[test]
fn microsofts_and_googles_built_in_values_cant_be_changed_on_their_accounts() {
    for provider in [OAuthProvider::Microsoft, OAuthProvider::Google] {
        let base = OAuth { provider, ..OAuth::default() };
        assert!(base.check().is_ok());
        for changed in [
            OAuth { token: "https://id.example.org/token".into(), ..base.clone() },
            OAuth { issuer: "https://id.example.org".into(), ..base.clone() },
            OAuth { scopes: "more".into(), ..base.clone() },
            OAuth { mechanism: SaslChoice::OAuthBearer, ..base.clone() },
        ] {
            assert!(changed.check().is_err(), "{changed:?}");
        }
        // Their client id and redirect port can.
        assert!(OAuth { client_id: "mine".into(), redirect_port: Some(8_400), ..base.clone() }.check().is_ok());
    }
}

#[test]
fn an_own_provider_needs_a_client_id_and_its_two_addresses_to_sign_in() {
    assert!(own().ready().is_ok());
    assert!(OAuth { client_id: String::new(), ..own() }.check().is_err());
    // The addresses may be found later (Discover): saved without them, not signed in with.
    let found_later = OAuth { authorize: String::new(), token: String::new(), ..own() };
    assert!(found_later.check().is_ok() && found_later.ready().is_err());
    // Typed by hand, no issuer is needed.
    assert!(OAuth { issuer: String::new(), ..own() }.ready().is_ok());
    assert!(OAuth::custom().ready().is_err());
}

#[test]
fn addresses_that_cant_be_used_are_refused() {
    for bad in [
        "http://id.example.org/token",
        "https://user:pw@id.example.org/token",
        "https://id.example.org/token#x",
        "javascript:alert(1)",
        "ftp://id.example.org/",
        "https://id.example.org/a b",
        "https://id.example.org/\r\nHost: x",
        "https://id.example.org/\0",
        "id.example.org/token",
        "https:///token",
    ] {
        assert!(endpoint(bad).is_err(), "{bad:?}");
        assert!(OAuth { token: bad.into(), ..own() }.check().is_err(), "{bad:?}");
    }
    assert!(endpoint(&format!("https://id.example.org/{}", "a".repeat(5_000))).is_err());
    for good in
        ["https://id.example.org/token", "https://xn--bcher-kva.example/t", "http://127.0.0.1:8080/token", "http://[::1]/t", "http://localhost/t", ""]
    {
        assert!(endpoint(good).is_ok(), "{good:?}");
    }
    assert!(OAuth { issuer: "https://id.example.org/realms/mail?x=1".into(), ..own() }.check().is_err());
}

#[test]
fn scopes_ports_and_client_ids_that_cant_be_sent_are_refused() {
    for bad in ["a\"b", "a\\b", "é", "a\nb", &"s ".repeat(40), &"x".repeat(2_000)] {
        assert!(scopes(bad).is_err(), "{bad:?}");
    }
    assert!(scopes("openid mail urn:ietf:params:oauth:scope:mail").is_ok());
    assert!(OAuth { redirect_port: Some(0), ..own() }.check().is_err());
    for bad in ["has space", "a\u{7f}", &"c".repeat(201), "ünïcode"] {
        assert!(OAuth { client_id: bad.into(), ..own() }.check().is_err(), "{bad:?}");
    }
    assert!(OAuth { tenant: "../evil".into(), ..OAuth::default() }.check().is_err());
}

#[test]
fn the_binding_changes_with_every_value_a_remembered_sign_in_belongs_to() {
    let base = own().binding();
    for changed in [
        OAuth { issuer: "https://id2.example.org".into(), ..own() },
        OAuth { token: "https://id.example.org/other".into(), ..own() },
        OAuth { client_id: "other".into(), ..own() },
        OAuth { provider: OAuthProvider::Microsoft, ..OAuth::default() },
    ] {
        assert_ne!(changed.binding(), base, "{changed:?}");
    }
    // Not with the ones it doesn't.
    assert_eq!(OAuth { scopes: String::new(), mechanism: SaslChoice::XOAuth2, redirect_port: None, ..own() }.binding(), base);
}

proptest! {
    #[test]
    fn checking_never_panics(issuer in ".{0,300}", token in ".{0,300}", scope in ".{0,300}", client in ".{0,300}", tenant in ".{0,80}", port in proptest::option::of(any::<u16>())) {
        for provider in [OAuthProvider::Microsoft, OAuthProvider::Google, OAuthProvider::Custom] {
            let oauth = OAuth { provider, issuer: issuer.clone(), token: token.clone(), authorize: token.clone(), revoke: issuer.clone(), scopes: scope.clone(), client_id: client.clone(), tenant: tenant.clone(), redirect_port: port, ..OAuth::default() };
            let _ = oauth.check();
            let _ = oauth.ready();
            let _ = oauth.effective();
            let _ = oauth.binding();
        }
    }

    #[test]
    fn reading_any_provider_value_never_panics_and_is_never_microsoft_unless_it_says_so(value in ".{0,40}") {
        let json = serde_json::json!({ "provider": value });
        let oauth: OAuth = serde_json::from_value(json).unwrap();
        if oauth.provider == OAuthProvider::Microsoft {
            prop_assert_eq!(OAuthProvider::parse(&value), Some(OAuthProvider::Microsoft));
        }
    }
}

#[test]
fn how_the_secret_is_sent_is_kept_and_the_standard_way_is_the_default() {
    use crate::ClientAuth;
    // RFC 6749 §2.3.1: the header, which every provider takes, unless the account says the form.
    assert_eq!(own().client_auth, ClientAuth::Basic);
    assert!(!serde_json::to_string(&own()).unwrap().contains("clientAuth"));
    let post = OAuth { client_auth: ClientAuth::Post, issuer_in_response: true, ..own() };
    let text = serde_json::to_string(&post).unwrap();
    assert!(text.contains("\"clientAuth\":\"post\"") && text.contains("\"issuerInResponse\":true"), "{text}");
    assert_eq!(serde_json::from_str::<OAuth>(&text).unwrap(), post);
    assert_eq!((post.effective().client_auth, post.effective().issuer_in_response), (ClientAuth::Post, true));
    // A way this version doesn't know reads as the standard one, and the account still opens.
    let odd: OAuth = serde_json::from_str(r#"{"provider":"custom","clientId":"m","clientAuth":"private_key_jwt","issuerInResponse":"yes"}"#).unwrap();
    assert_eq!((odd.client_auth, odd.issuer_in_response), (ClientAuth::Basic, false));
    // Names a command may use.
    for (text, way) in [("basic", ClientAuth::Basic), ("client_secret_post", ClientAuth::Post), (" POST ", ClientAuth::Post)] {
        assert_eq!(ClientAuth::parse(text), Some(way), "{text}");
    }
    assert_eq!(ClientAuth::parse("private_key_jwt"), None);
    // Microsoft's and Google's are built in.
    for provider in [OAuthProvider::Microsoft, OAuthProvider::Google] {
        assert!(OAuth { provider, client_auth: ClientAuth::Post, ..OAuth::default() }.check().is_err());
        assert!(OAuth { provider, issuer_in_response: true, ..OAuth::default() }.check().is_err());
    }
}
