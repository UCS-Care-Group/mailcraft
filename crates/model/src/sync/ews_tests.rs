// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::FolderKind;
use crate::sync::{FolderChange, GraphFolder, GraphMessage, MAX_LISTED_FOLDERS, ReadState};

const ME: &str = "ada@example.org";
const URL: &str = "https://secret-host.example.org/EWS/Exchange.asmx";

fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap();
    assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
    json
}

fn request() -> EwsMailRequest {
    EwsMailRequest {
        account: 3,
        email: ME.into(),
        url: URL.into(),
        username: "CORP\\ada".into(),
        trusted: "ab".repeat(32),
        list_folders: true,
        hierarchy: "H-STATE".into(),
        scope: EwsScope::Light { also: vec!["F-ONE".into()] },
        states: vec![EwsFolderState { folder: "F-ONE".into(), state: "S-ONE".into() }],
        changes: vec![FolderChange::Delete { id: "F-GONE".into() }],
        inbox_pass: true,
        full: true,
        background: true,
        ..Default::default()
    }
}

#[test]
fn a_request_round_trips_and_an_empty_one_is_small() {
    round_trip(&request());
    round_trip(&EwsFetchRequest { account: 3, email: ME.into(), url: URL.into(), ids: vec!["A".into(), "B".into()], ..Default::default() });
    let json = round_trip(&EwsMailRequest::default());
    assert!(!json.contains("trusted") && !json.contains("states") && !json.contains("sends"), "{json}");
}

#[test]
fn a_result_round_trips_and_pieces_are_absorbed_in_order() {
    let inbox = GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let first = EwsMailResult {
        account: 3,
        email: ME.into(),
        url: URL.into(),
        folders: Some(EwsFolderDeltas { changed: vec![inbox], deleted: vec!["F0".into()], afresh: true }),
        hierarchy: "H1".into(),
        states: vec![EwsFolderState { folder: "F1".into(), state: "S1".into() }],
        errors: vec!["one".into()],
        partial: true,
        ..Default::default()
    };
    let last = EwsMailResult {
        account: 3,
        folders: Some(EwsFolderDeltas { deleted: vec!["F9".into()], ..Default::default() }),
        states: vec![EwsFolderState { folder: "F1".into(), state: "S2".into() }],
        whole: true,
        ..Default::default()
    };
    round_trip(&first);
    let mut all = EwsMailResult::default();
    all.absorb(first);
    all.absorb(last);
    assert_eq!((all.account, all.email.as_str(), all.hierarchy.as_str()), (3, ME, "H1"));
    assert_eq!(all.states.iter().map(|s| s.state.as_str()).collect::<Vec<_>>(), ["S1", "S2"]);
    assert_eq!(all.errors.len(), 1);
    assert!(all.whole && !all.partial);
    let folders = all.folders.unwrap();
    assert_eq!((folders.changed.len(), folders.deleted, folders.afresh), (1, vec!["F0".to_string(), "F9".to_string()], true));
}

fn page() -> EwsMessagePage {
    let message = GraphMessage {
        server: crate::sync::ServerMessage { id: "ID-AND-KEY".into(), folder: "F1".into(), ..Default::default() },
        ..Default::default()
    };
    EwsMessagePage {
        folder: "F1".into(),
        messages: vec![message],
        removed: vec!["R1".into()],
        read: vec![ReadState { id: "M2".into(), read: true }],
        last: true,
        afresh: false,
    }
}

#[test]
fn a_page_has_no_link_and_says_whether_it_is_the_last_and_whether_it_begins_afresh() {
    let json = round_trip(&page());
    assert!(!json.contains("link") && !json.contains("caughtUp") && json.contains(r#""last":true"#), "{json}");
    round_trip(&EwsMessagePage { afresh: true, ..page() });
    assert_eq!(serde_json::from_str::<EwsMessagePage>("{}").unwrap(), EwsMessagePage::default());
}

fn whole_result() -> EwsMailResult {
    EwsMailResult {
        account: 3,
        email: ME.into(),
        url: URL.into(),
        folders: Some(EwsFolderDeltas {
            changed: vec![GraphFolder { id: "F1".into(), name: "Inbox".into(), ..Default::default() }],
            ..Default::default()
        }),
        hierarchy: "H1".into(),
        pages: vec![page()],
        states: vec![EwsFolderState { folder: "F1".into(), state: "S1".into() }],
        errors: vec!["one".into()],
        untrusted: "ab".repeat(32),
        ..Default::default()
    }
}

/// The caps are checked, one at a time: each of these results is refused, and the sound one is not.
#[test]
fn a_result_beyond_the_caps_is_refused() {
    assert_eq!(whole_result().check(), Ok(()));
    assert_eq!(EwsMailResult::default().check(), Ok(()));
    let long = "x".repeat(MAX_EWS_STATE + 1);
    let long_id = "x".repeat(MAX_EWS_ID + 1);
    let states = |n: usize| (0..n).map(|i| EwsFolderState { folder: format!("F{i}"), state: "S".into() }).collect::<Vec<_>>();
    let refused: Vec<(&str, Box<dyn Fn(&mut EwsMailResult)>)> = vec![
        ("too many states", Box::new(|r| r.states = states(MAX_EWS_STATES + 1))),
        ("a long state", Box::new(|r| r.states[0].state = long.clone())),
        ("a long folder id in a state", Box::new(|r| r.states[0].folder = long_id.clone())),
        ("an empty folder id in a state", Box::new(|r| r.states[0].folder = String::new())),
        ("a control character in a state", Box::new(|r| r.states[0].state = "a\nb".into())),
        ("a long hierarchy state", Box::new(|r| r.hierarchy = long.clone())),
        ("too many pages", Box::new(|r| r.pages = vec![EwsMessagePage { folder: "F".into(), ..Default::default() }; MAX_EWS_PAGES + 1])),
        ("too many messages in a page", Box::new(|r| r.pages[0].messages = vec![GraphMessage::default(); MAX_EWS_PAGE_MESSAGES + 1])),
        ("too many removed in a page", Box::new(|r| r.pages[0].removed = vec!["R".into(); MAX_EWS_PAGE_MESSAGES + 1])),
        ("a long message id", Box::new(|r| r.pages[0].messages[0].server.id = long_id.clone())),
        ("an empty message id", Box::new(|r| r.pages[0].messages[0].server.id = String::new())),
        ("a long removed id", Box::new(|r| r.pages[0].removed[0] = long_id.clone())),
        ("a page of no folder", Box::new(|r| r.pages[0].folder = String::new())),
        ("too many folders", Box::new(|r| r.folders.as_mut().unwrap().deleted = vec!["F".into(); MAX_LISTED_FOLDERS + 1])),
        ("a long folder id", Box::new(|r| r.folders.as_mut().unwrap().changed[0].id = long_id.clone())),
        ("a long folder name", Box::new(|r| r.folders.as_mut().unwrap().changed[0].name = "x".repeat(5000))),
        ("too many problems", Box::new(|r| r.errors = vec!["e".into(); MAX_EWS_ERRORS + 1])),
        ("a long problem", Box::new(|r| r.errors[0] = "e".repeat(MAX_EWS_ERROR + 1))),
        ("a fingerprint that is none", Box::new(|r| r.untrusted = "nope".into())),
        ("a long url", Box::new(|r| r.url = "u".repeat(3000))),
    ];
    for (what, change) in refused {
        let mut r = whole_result();
        change(&mut r);
        assert!(r.check().is_err(), "{what}");
    }
}

#[test]
fn a_missing_or_unknown_field_still_reads() {
    let result: EwsMailResult = serde_json::from_str("{}").unwrap();
    assert_eq!(result, EwsMailResult::default());
    let result: EwsMailResult = serde_json::from_str(r#"{"account": 2, "future": [1, 2]}"#).unwrap();
    assert_eq!(result.account, 2);
}

#[test]
fn debug_output_is_counts_only() {
    let fetch = EwsFetchRequest { email: ME.into(), url: URL.into(), ids: vec!["SECRET-ID".into()], ..Default::default() };
    let text = format!("{:?} {fetch:?}", request());
    for secret in [ME, "secret-host", "CORP", "ababab", "H-STATE", "S-ONE", "F-ONE", "SECRET-ID"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let result = EwsMailResult {
        email: ME.into(),
        url: URL.into(),
        states: vec![EwsFolderState { folder: "F-ONE".into(), state: "S-ONE".into() }],
        hierarchy: "H-STATE".into(),
        untrusted: "ab".repeat(32),
        ..Default::default()
    };
    let text = format!("{result:?}");
    for secret in [ME, "secret-host", "S-ONE", "F-ONE", "H-STATE", "ababab"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}
