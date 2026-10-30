// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn an_https_address_may_be_used() {
    for good in
        ["https://mail.example.org/EWS/Exchange.asmx", "HTTPS://Mail.Example.org:444/ews/exchange.asmx", "http://localhost:8080/EWS/Exchange.asmx"]
    {
        assert_eq!(EwsServer::check_url(good), Ok(()), "{good}");
    }
}

#[test]
fn other_addresses_are_refused() {
    for bad in [
        "",
        "http://mail.example.org/EWS/Exchange.asmx",
        "ftp://mail.example.org/",
        "https://user:pw@mail.example.org/EWS/Exchange.asmx",
        "https://mail.example.org/a b",
        "https://mail.example.org\\x",
        "https:///EWS",
        "https://mail.example.org:0/",
        "https://mail.example.org/\u{0}",
    ] {
        assert!(EwsServer::check_url(bad).is_err(), "{bad:?}");
    }
    assert!(EwsServer::check_url(&format!("https://example.org/{}", "a".repeat(3000))).is_err());
}

#[test]
fn user_names_and_fingerprints_are_checked_as_text() {
    assert!(EwsServer::check_username("CORP\\ada").is_ok());
    assert!(EwsServer::check_username("ada@example.org").is_ok());
    assert!(EwsServer::check_username("ada\r\nBcc: x").is_err());
    assert!(EwsServer::check_username(&"x".repeat(300)).is_err());
    assert!(EwsServer::check_fingerprint("").is_ok());
    assert!(EwsServer::check_fingerprint(&"ab".repeat(32)).is_ok());
    assert!(EwsServer::check_fingerprint(&"ab".repeat(31)).is_err());
    assert!(EwsServer::check_fingerprint(&"zz".repeat(32)).is_err());
}

#[test]
fn problem_and_host_follow_the_checks() {
    let mut server = EwsServer { url: "https://Mail.Example.org/EWS/Exchange.asmx".into(), ..Default::default() };
    assert_eq!(server.problem(), None);
    assert_eq!(server.host(), Some("Mail.Example.org"));
    server.trusted = "nope".into();
    assert!(server.problem().is_some());
    assert!(EwsServer::default().problem().is_some());
    assert_eq!(EwsServer::default().host(), None);
}

#[test]
fn debug_output_shows_no_address_name_or_fingerprint() {
    let server = EwsServer {
        url: "https://secret.example.org/EWS".into(),
        username: "CORP\\ada".into(),
        trusted: "ab".repeat(32),
        hierarchy: "HIER-STATE".into(),
    };
    let text = format!("{server:?}");
    assert!(!text.contains("secret") && !text.contains("ada") && !text.contains("abab") && !text.contains("HIER"), "{text}");
}

#[test]
fn an_exchange_account_has_a_server_server_ids_and_sends_mail_but_only_behind_the_switch() {
    use crate::AccountKind::{self, *};
    assert_eq!([Local, Imap, Microsoft, Jmap, Exchange, Unknown].map(AccountKind::has_server), [false, true, true, true, true, false]);
    assert_eq!([Local, Imap, Microsoft, Jmap, Exchange, Unknown].map(AccountKind::sends_mail), [false, true, true, false, true, false]);
    assert_eq!([Local, Imap, Microsoft, Jmap, Exchange, Unknown].map(AccountKind::server_ids), [false, false, true, true, true, false]);
    assert_eq!([Local, Imap, Microsoft, Jmap, Exchange, Unknown].map(AccountKind::needs_switch), [false, false, false, false, true, false]);
}

#[test]
fn an_exchange_account_keeps_its_setting_and_a_damaged_one_costs_only_itself() {
    use crate::{Account, AccountKind};
    let server = EwsServer {
        url: "https://mail.example.org/EWS/Exchange.asmx".into(),
        username: "CORP\\ada".into(),
        trusted: "ab".repeat(32),
        hierarchy: "HIER-STATE".into(),
    };
    let account = Account { id: 4, email: "ada@example.org".into(), kind: AccountKind::Exchange, ews: Some(server), ..Default::default() };
    let json = serde_json::to_string(&account).unwrap();
    assert!(json.contains(r#""kind":"exchange""#) && json.contains(r#""ews":{"url""#), "{json}");
    assert_eq!(serde_json::from_str::<Account>(&json).unwrap(), account);
    assert!(json.contains("HIER-STATE"), "the hierarchy's state is kept with the account: {json}");
    assert!(!serde_json::to_string(&Account::default()).unwrap().contains("ews"));
    // A folder keeps its own state, and a folder of any other account has none to write.
    let folder = crate::ServerFolder { id: "F1".into(), sync_state: "FOLDER-STATE".into(), ..Default::default() };
    let json = serde_json::to_string(&folder).unwrap();
    assert!(json.contains(r#""syncState":"FOLDER-STATE""#), "{json}");
    assert_eq!(serde_json::from_str::<crate::ServerFolder>(&json).unwrap(), folder);
    assert!(!serde_json::to_string(&crate::ServerFolder::default()).unwrap().contains("syncState"));
    for odd in ["5", "\"https://mail.example.org\"", "[]", "null", r#"{"url":5}"#, r#"{"trusted":[]}"#] {
        let read: Account = serde_json::from_str(&format!(r#"{{"id":4,"email":"ada@example.org","kind":"exchange","ews":{odd}}}"#)).unwrap();
        assert_eq!(read.kind, AccountKind::Exchange, "{odd}");
        assert!(read.ews.as_ref().is_none_or(|server| server.problem().is_some()), "{odd}: {:?}", read.ews);
    }
}
