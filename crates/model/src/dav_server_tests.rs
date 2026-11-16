// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::{Account, AccountKind};

fn server() -> DavServer {
    DavServer {
        calendar_homes: vec!["https://dav.example.org/cal/ada/".into()],
        contact_homes: vec!["https://dav.example.org/card/ada/".into()],
        username: "ada".into(),
        trusted: "ab".repeat(32),
        separate_password: true,
        collections: vec![DavCollectionState {
            href: "https://dav.example.org/cal/ada/work/".into(),
            name: "Work".into(),
            color: Some(Rgb(1, 2, 3)),
            components: vec!["VEVENT".into()],
            sync_token: "TOKEN-1".into(),
            ctag: "CTAG-1".into(),
            ..Default::default()
        }],
        other: Default::default(),
    }
}

#[test]
fn https_homes_may_be_used() {
    for good in ["https://dav.example.org/cal/", "HTTPS://Dav.Example.org:8443/cal/", "http://localhost:8080/dav/", "https://example.org"] {
        assert_eq!(DavServer::check_home(good), Ok(()), "{good}");
    }
}

#[test]
fn other_homes_are_refused() {
    for bad in [
        "",
        "http://dav.example.org/cal/",
        "ftp://dav.example.org/",
        "https://user:pw@dav.example.org/cal/",
        "https://dav.example.org/a b",
        "https://dav.example.org\\x",
        "https:///cal",
        "https://dav.example.org:0/",
        "https://dav.example.org/\u{0}",
        "https://outlook.office365.com/dav/",
        "https://calendar.hotmail.com/",
        "https://apidata.googleusercontent.com/caldav/v2/",
        "https://www.googleapis.com/carddav/v1/",
    ] {
        assert!(DavServer::check_home(bad).is_err(), "{bad:?}");
    }
    assert!(DavServer::check_home(&format!("https://example.org/{}", "a".repeat(3000))).is_err());
    // A name that only looks like Microsoft's or Google's is somebody else's.
    for ok in ["https://evil-google.com/dav/", "https://outlook.abc.com/dav/", "https://notgmail.com/"] {
        assert!(DavServer::check_home(ok).is_ok(), "{ok}");
    }
}

#[test]
fn the_settings_are_checked_as_text() {
    assert_eq!(server().problem(), None);
    assert_eq!(DavServer::default().problem().map(|p| p.0), Some("homes"));
    let many = DavServer { calendar_homes: (0..9).map(|n| format!("https://dav.example.org/{n}/")).collect(), ..server() };
    assert_eq!(many.problem().map(|p| p.0), Some("homes"));
    assert_eq!(DavServer { contact_homes: vec!["http://dav.example.org/".into()], ..server() }.problem().map(|p| p.0), Some("homes"));
    assert_eq!(DavServer { username: "ada\r\nBcc: x".into(), ..server() }.problem().map(|p| p.0), Some("username"));
    assert_eq!(DavServer { trusted: "nope".into(), ..server() }.problem().map(|p| p.0), Some("trusted"));
    // Either kind of home alone will do.
    assert_eq!(DavServer { contact_homes: Vec::new(), ..server() }.problem(), None);
    assert_eq!(DavServer { calendar_homes: Vec::new(), ..server() }.problem(), None);
}

#[test]
fn a_password_belongs_to_an_origin() {
    assert_eq!(origin_of("https://Dav.Example.org/cal/").as_deref(), Some("https://dav.example.org"));
    assert_eq!(origin_of("https://dav.example.org:443/x").as_deref(), Some("https://dav.example.org"));
    assert_eq!(origin_of("https://dav.example.org:8443/x").as_deref(), Some("https://dav.example.org:8443"));
    assert_eq!(origin_of("https://dav.example.org./x").as_deref(), Some("https://dav.example.org"));
    assert_eq!(origin_of("http://localhost:8080/x").as_deref(), Some("http://localhost:8080"));
    assert_eq!(origin_of("http://[::1]:8080/x").as_deref(), Some("http://[::1]:8080"));
    // Another host, port or scheme is another origin; a lookalike is not the same one.
    assert_ne!(origin_of("https://dav.example.org/"), origin_of("https://dav.example.org:444/"));
    assert_ne!(origin_of("https://dav.example.org/"), origin_of("https://dav.example.org.evil.example/"));
    assert_eq!(origin_of("https://user@dav.example.org/"), None);
    assert_eq!(origin_of("not an address"), None);
}

#[test]
fn origins_are_listed_once_each_homes_and_collections_alike() {
    let mut s = server();
    s.collections.push(DavCollectionState { href: "https://p12-caldav.example.net/ada/home/".into(), ..Default::default() });
    s.collections.push(DavCollectionState { href: "https://p12-caldav.example.net/ada/other/".into(), ..Default::default() });
    s.collections.push(DavCollectionState { href: "https://user:pw@bad.example.net/".into(), ..Default::default() });
    assert_eq!(s.origins(), ["https://dav.example.org", "https://p12-caldav.example.net"]);
    assert_eq!(DavServer::default().origins(), Vec::<String>::new());
    assert_eq!(s.host(), Some("dav.example.org"));
}

#[test]
fn a_changed_server_is_told_from_a_changed_setting() {
    let a = server();
    let mut b = a.clone();
    b.collections.clear();
    b.separate_password = false;
    assert!(!a.is_another_server(&b), "a password flag and the state are not the server");
    for other in [
        DavServer { calendar_homes: vec!["https://dav2.example.org/cal/".into()], ..server() },
        DavServer { contact_homes: Vec::new(), ..server() },
        DavServer { username: "grace".into(), ..server() },
        DavServer { trusted: String::new(), ..server() },
    ] {
        assert!(a.is_another_server(&other));
    }
    assert!(!a.is_another_server(&DavServer { username: " ADA ".into(), ..server() }));
}

#[test]
fn debug_output_shows_no_address_name_token_or_fingerprint() {
    let text = format!("{:?}", server());
    for word in ["example", "ada", "abab", "TOKEN", "CTAG", "Work", "ada@"] {
        assert!(!text.contains(word), "{word}: {text}");
    }
    let text = format!("{:?}", server().collections[0]);
    assert!(!text.contains("example") && !text.contains("TOKEN") && !text.contains("Work"), "{text}");
}

#[test]
fn an_account_keeps_its_dav_settings_in_the_mailbox_file() {
    let account = Account { id: 4, email: "ada@example.org".into(), kind: AccountKind::Imap, dav: Some(server()), ..Default::default() };
    let json = serde_json::to_string(&account).unwrap();
    assert!(json.contains(r#""dav":{"#) && json.contains("TOKEN-1") && json.contains(r#""separatePassword":true"#), "{json}");
    assert_eq!(serde_json::from_str::<Account>(&json).unwrap(), account);
    assert!(!serde_json::to_string(&Account::default()).unwrap().contains("dav"));
    // The server's state is not a secret but no password is ever among it.
    assert!(!json.to_lowercase().contains("password\":\""), "{json}");
}

#[test]
fn damaged_dav_settings_cost_only_themselves() {
    for odd in ["5", "\"https://dav.example.org\"", "[]", "null", r#"{"calendarHomes":5}"#, r#"{"trusted":[]}"#, r#"{"collections":"x"}"#] {
        let read: Account = serde_json::from_str(&format!(r#"{{"id":4,"email":"ada@example.org","kind":"imap","dav":{odd}}}"#)).unwrap();
        assert_eq!(read.kind, AccountKind::Imap, "{odd}");
        assert!(read.dav.as_ref().is_none_or(|server| server.problem().is_some()), "{odd}: {:?}", read.dav);
    }
    // One damaged collection, or home, is left out and the rest stays.
    let read: DavServer = serde_json::from_str(
        r#"{"calendarHomes":["https://dav.example.org/cal/", 7],"collections":[{"href":"https://dav.example.org/c/","syncToken":"T"}, 5, {"href":3}]}"#,
    )
    .unwrap();
    assert_eq!((read.calendar_homes.len(), read.collections.len()), (1, 1));
    assert_eq!(read.collections[0].sync_token, "T");
}

#[test]
fn what_a_later_version_wrote_is_kept_and_an_earlier_one_reads_the_file() {
    // A later version's file: the server has a field this one doesn't know. It survives a read
    // and a write.
    let text = r#"{"calendarHomes":["https://dav.example.org/cal/"],"futureThing":{"a":[1,2]},"collections":[]}"#;
    let read: DavServer = serde_json::from_str(text).unwrap();
    assert!(read.other.contains_key("futureThing"));
    let back = serde_json::to_string(&read).unwrap();
    assert!(back.contains(r#""futureThing":{"a":[1,2]}"#), "{back}");
    // A build from before `dav` read the file by ignoring it (every account field is optional):
    // the account is the same account, as a file from before has it.
    #[derive(Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Older {
        id: u64,
        email: String,
        kind: AccountKind,
    }
    impl Default for Older {
        fn default() -> Self {
            Older { id: 0, email: String::new(), kind: AccountKind::Local }
        }
    }
    let account = Account { id: 4, email: "ada@example.org".into(), kind: AccountKind::Imap, dav: Some(server()), ..Default::default() };
    let older: Older = serde_json::from_str(&serde_json::to_string(&account).unwrap()).unwrap();
    assert_eq!((older.id, older.email.as_str(), older.kind), (4, "ada@example.org", AccountKind::Imap));
}
