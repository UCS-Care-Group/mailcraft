// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a search of an Exchange server or JMAP account on its server is asked with (#1725).

use super::*;

#[test]
fn debug_output_shows_nothing_of_the_server_the_owner_or_the_search() {
    let ews = EwsSearchRequest {
        email: "ada@example.org".into(),
        url: "https://mail.example.org/EWS/Exchange.asmx".into(),
        username: "EXAMPLE\\ada".into(),
        trusted: "AB:CD".into(),
        folders: vec!["folder-one".into()],
        query: "budget from:zoe".into(),
        next: "0 25".into(),
        ..Default::default()
    };
    let shown = format!("{ews:?}");
    for secret in ["ada", "mail.example", "AB:CD", "folder-one", "budget", "25"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    let jmap = JmapSearchRequest {
        email: "ada@example.org".into(),
        session: "https://mail.example.org/jmap/session".into(),
        folder: "mailbox-one".into(),
        outside: vec!["mailbox-trash".into()],
        places: vec![SearchPlace { id: "mailbox-one".into(), kind: crate::FolderKind::Inbox }],
        query: "budget from:zoe".into(),
        next: "50".into(),
        ..Default::default()
    };
    let shown = format!("{jmap:?}");
    for secret in ["ada", "mail.example", "mailbox", "budget", "50"] {
        assert!(!shown.contains(secret), "{shown}");
    }
}

#[test]
fn the_requests_go_through_json_as_they_are() {
    let ews = EwsSearchRequest { account: 3, ticket: 7, folders: vec!["a".into(), "b".into()], query: "budget".into(), ..Default::default() };
    let json = serde_json::to_string(&ews).unwrap();
    assert_eq!(serde_json::from_str::<EwsSearchRequest>(&json).unwrap(), ews);
    let jmap = JmapSearchRequest {
        account: 3,
        ticket: 7,
        bearer: true,
        places: vec![SearchPlace { id: "a".into(), kind: crate::FolderKind::Archive }],
        query: "budget".into(),
        ..Default::default()
    };
    let json = serde_json::to_string(&jmap).unwrap();
    assert_eq!(serde_json::from_str::<JmapSearchRequest>(&json).unwrap(), jmap);
    // What is missing reads as empty.
    assert_eq!(serde_json::from_str::<JmapSearchRequest>("{}").unwrap(), JmapSearchRequest::default());
}
