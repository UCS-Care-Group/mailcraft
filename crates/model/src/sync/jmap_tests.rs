// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::sync::{GraphFolder, GraphMessage, SendOutcome, ServerMessage};
use crate::{Address, FolderKind, Message};

const ME: &str = "ada@example.org";
const SESSION: &str = "https://mail.example.org/jmap/session?account=ada@example.org";

fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap();
    assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
    json
}

/// A request with something in every part of it, and the address everywhere it could be.
fn request() -> JmapMailRequest {
    let to_bob = Message { id: 7, to: vec![Address::new("Bob", "bob@example.org")], subject: format!("For {ME}"), ..Default::default() };
    JmapMailRequest {
        account: 3,
        email: ME.into(),
        session: SESSION.into(),
        bearer: true,
        window: 300,
        list_folders: true,
        scope: JmapScope::Light { also: vec![format!("M-{ME}"), "M2".into()] },
        known: vec![
            JmapKnown { mailbox: "M1".into(), ids: vec![format!("E-{ME}"), "E2".into(), "E3".into()] },
            JmapKnown { mailbox: "M2".into(), ids: vec![] },
        ],
        changes: vec![
            JmapChange::Keywords { message: 4, id: "E2".into(), read: Some(true), flagged: None, answered: Some(false), forwarded: None },
            JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() },
            JmapChange::Destroy { id: "E9".into() },
        ],
        sends: vec![SendUp { message: 7, draft: None, mime: Some(Box::new(to_bob)) }],
        background: true,
    }
}

fn result() -> JmapMailResult {
    let message = GraphMessage {
        server: ServerMessage { id: "E2".into(), folder: "M1".into(), read: true, answered: true, forwarded: true, ..Default::default() },
        message_id: "a@mail.example.org".into(),
        subject: format!("Hello {ME}"),
        from: Address::new("Ada", ME),
        to: vec![Address::new("Bob", "bob@example.org")],
        preview: format!("To {ME}"),
        ..Default::default()
    };
    JmapMailResult {
        account: 3,
        email: ME.into(),
        session: SESSION.into(),
        changes: vec![
            JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Done { id: String::new() } },
            JmapChangeDone {
                change: JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() },
                outcome: JmapOutcome::Done { id: "E3b".into() },
            },
            JmapChangeDone { change: JmapChange::Destroy { id: "E8".into() }, outcome: JmapOutcome::Gone },
            JmapChangeDone {
                change: JmapChange::Destroy { id: "E7".into() },
                outcome: JmapOutcome::Refused { why: format!("{ME} may not delete it") },
            },
        ],
        sent: vec![SendDone {
            message: 7,
            outcome: SendOutcome::Held { lost: false, why: format!("{ME} was refused") },
            draft: None,
            draft_left: String::new(),
        }],
        listing: Some(FolderListing {
            folders: vec![GraphFolder { id: "M1".into(), name: ME.into(), kind: FolderKind::Inbox, total: 4, unread: 1, ..Default::default() }],
            complete: true,
        }),
        pages: vec![MessagePage { folder: "M1".into(), messages: vec![message], afresh: true, ..Default::default() }],
        errors: vec![format!("no mailbox for {ME}")],
        busy_for: Some(30),
        partial: true,
        whole: true,
        stopped: true,
        progress: Some(GraphProgress { folder: "M1".into(), read: 2, total: 4, number: 1, folders: 3 }),
        auth_refused: true,
    }
}

#[test]
fn a_request_and_a_result_read_and_write_as_json() {
    let json = round_trip(&request());
    for part in [r#""scope":{"light":{"also":["#, r#""known":[{"mailbox":"M1","ids":["#, r#""sends":[{"message":7,"mime":"#, r#""bearer":true"#] {
        assert!(json.contains(part), "{part} in {json}");
    }
    let json = round_trip(&result());
    for part in [r#""authRefused":true"#, r#""busyFor":30"#, r#""whole":true"#, r#""afresh":true"#, r#""stopped":true"#, r#""session":""#] {
        assert!(json.contains(part), "{part} in {json}");
    }
    assert!(!json.contains("auth_refused") && !json.contains("busy_for"), "{json}");
}

#[test]
fn what_is_not_said_is_empty() {
    let none: JmapMailRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(none, JmapMailRequest::default());
    assert_eq!((none.window, none.scope.clone(), none.known.len(), none.changes.len(), none.sends.len()), (0, JmapScope::All, 0, 0, 0));
    let none: JmapMailResult = serde_json::from_str("{}").unwrap();
    assert_eq!(none, JmapMailResult::default());
    // An empty run writes next to nothing, and reads back as it was.
    assert_eq!(round_trip(&JmapMailResult::default()), "{\"account\":0}");
    assert_eq!(round_trip(&JmapMailRequest::default()), r#"{"account":0,"email":"","session":"","window":0,"scope":"all"}"#);
    // A result with only the parts that came: the others are empty.
    let some: JmapMailResult = serde_json::from_str(r#"{"account": 3, "pages": [{"folder": "M1"}], "whole": true}"#).unwrap();
    assert_eq!((some.account, some.pages.len(), some.whole, some.partial, some.listing.is_none()), (3, 1, true, false, true));
}

#[test]
fn a_fetch_request_reads_and_writes_as_json_and_shows_counts_only() {
    let request = JmapFetchRequest {
        account: 3,
        email: ME.into(),
        session: SESSION.into(),
        bearer: true,
        ids: vec![format!("E-{ME}"), "E2".into()],
        background: true,
    };
    let json = round_trip(&request);
    for part in [r#""session":"https://mail.example.org/"#, r#""bearer":true"#, r#""ids":["E-ada@example.org","E2"]"#, r#""background":true"#] {
        assert!(json.contains(part), "{part} in {json}");
    }
    // What is false is not written, and what is not said reads as empty.
    let plain = JmapFetchRequest { account: 3, ids: vec!["E1".into()], ..Default::default() };
    assert_eq!(round_trip(&plain), r#"{"account":3,"email":"","session":"","ids":["E1"]}"#);
    assert_eq!(serde_json::from_str::<JmapFetchRequest>("{}").unwrap(), JmapFetchRequest::default());
    // Neither the account's address, nor the server's, nor a message's id is printed.
    for shown in [format!("{request:?}"), format!("{request:#?}")] {
        assert!(!shown.contains('@') && !shown.contains("example.org") && !shown.contains("E2") && !shown.contains("session"), "{shown}");
    }
    assert_eq!(format!("{request:?}"), "JmapFetchRequest { account: 3, bearer: true, ids: 2, background: true }");
}

#[test]
fn a_scope_is_a_word_or_a_list() {
    assert_eq!(round_trip(&JmapScope::All), r#""all""#);
    assert_eq!(round_trip(&JmapScope::Light { also: vec!["M1".into()] }), r#"{"light":{"also":["M1"]}}"#);
    assert_eq!(serde_json::from_str::<JmapScope>(r#"{"light":{}}"#).unwrap(), JmapScope::Light { also: vec![] });
    assert!(serde_json::from_str::<JmapScope>(r#""everything""#).is_err());
    assert_eq!(JmapScope::default(), JmapScope::All);
}

#[test]
fn a_change_names_only_what_changed() {
    let keywords = JmapChange::Keywords { message: 4, id: "E2".into(), read: Some(true), flagged: None, answered: None, forwarded: Some(false) };
    assert_eq!(round_trip(&keywords), r#"{"keywords":{"message":4,"id":"E2","read":true,"forwarded":false}}"#);
    let moved = JmapChange::Move { message: 5, id: "E3".into(), from: "M1".into(), to: "M2".into() };
    assert_eq!(round_trip(&moved), r#"{"move":{"message":5,"id":"E3","from":"M1","to":"M2"}}"#);
    let destroy = JmapChange::Destroy { id: "E9".into() };
    assert_eq!(round_trip(&destroy), r#"{"destroy":{"id":"E9"}}"#);
    assert_eq!([&keywords, &moved, &destroy].map(JmapChange::id), ["E2", "E3", "E9"]);
    // A change without the marks it names reads as one that names none.
    let bare: JmapChange = serde_json::from_str(r#"{"keywords":{"message":4,"id":"E2"}}"#).unwrap();
    assert_eq!(bare, JmapChange::Keywords { message: 4, id: "E2".into(), read: None, flagged: None, answered: None, forwarded: None });
    assert!(serde_json::from_str::<JmapChange>(r#"{"explode":{"id":"E2"}}"#).is_err());
    assert!(serde_json::from_str::<JmapChange>(r#"{"move":{"message":5,"id":"E3"}}"#).is_err());
}

#[test]
fn an_outcome_is_done_gone_or_refused() {
    assert_eq!(round_trip(&JmapOutcome::Done { id: String::new() }), r#"{"done":{}}"#);
    assert_eq!(round_trip(&JmapOutcome::Done { id: "E3b".into() }), r#"{"done":{"id":"E3b"}}"#);
    assert_eq!(round_trip(&JmapOutcome::Gone), r#""gone""#);
    assert_eq!(round_trip(&JmapOutcome::Refused { why: "not allowed".into() }), r#"{"refused":{"why":"not allowed"}}"#);
    let done = JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Gone };
    assert_eq!(round_trip(&done), r#"{"change":{"destroy":{"id":"E9"}},"outcome":"gone"}"#);
    assert_eq!(round_trip(&JmapKnown { mailbox: "M1".into(), ids: vec!["E1".into()] }), r#"{"mailbox":"M1","ids":["E1"]}"#);
    assert_eq!(serde_json::from_str::<JmapKnown>("{}").unwrap(), JmapKnown::default());
}

#[test]
fn the_pieces_of_a_run_come_together() {
    let page = |folder: &str, ids: &[&str]| MessagePage {
        folder: folder.into(),
        messages: ids
            .iter()
            .map(|id| GraphMessage { server: ServerMessage { id: (*id).into(), ..Default::default() }, ..Default::default() })
            .collect(),
        ..Default::default()
    };
    let pieces = [
        JmapMailResult {
            account: 3,
            email: ME.into(),
            session: SESSION.into(),
            partial: true,
            listing: Some(FolderListing { folders: vec![GraphFolder { id: "M1".into(), ..Default::default() }], complete: true }),
            changes: vec![JmapChangeDone { change: JmapChange::Destroy { id: "E9".into() }, outcome: JmapOutcome::Gone }],
            errors: vec!["One".into()],
            progress: Some(GraphProgress { folder: "M1".into(), read: 0, total: 2, number: 1, folders: 2 }),
            ..Default::default()
        },
        JmapMailResult {
            account: 3,
            partial: true,
            pages: vec![page("M1", &["E1", "E2"])],
            busy_for: Some(30),
            auth_refused: true,
            ..Default::default()
        },
        // (Nothing says the address here: it stays.)
        JmapMailResult { account: 3, pages: vec![page("M2", &["E3"])], errors: vec!["Two".into()], whole: true, stopped: true, ..Default::default() },
    ];
    let mut whole = JmapMailResult::default();
    pieces.into_iter().for_each(|piece| whole.absorb(piece));
    assert_eq!((whole.account, whole.email.as_str(), whole.session.as_str()), (3, ME, SESSION));
    assert_eq!((whole.partial, whole.whole, whole.stopped, whole.auth_refused, whole.busy_for), (false, true, true, true, Some(30)));
    assert_eq!(whole.pages.iter().map(|p| (p.folder.as_str(), p.messages.len())).collect::<Vec<_>>(), [("M1", 2), ("M2", 1)]);
    assert_eq!((whole.changes.len(), whole.errors.join(", ").as_str(), whole.listing.map(|l| l.folders.len())), (1, "One, Two", Some(1)));
    // The progress of the piece that had one stays until another says more.
    assert_eq!(whole.progress.map(|p| p.total), Some(2));
}

/// The address of the account is in everything a request and a result are made of, and in
/// none of what they print.
#[test]
fn debug_output_has_counts_only() {
    let (request, result) = (request(), result());
    for shown in [format!("{request:?}"), format!("{request:#?}"), format!("{result:?}"), format!("{result:#?}")] {
        assert!(!shown.contains('@'), "{shown}");
        for secret in ["example.org", "mail.", "Ada", "Bob", "Hello", "jmap/session", "E-", "M-", "may not delete"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }
    assert_eq!(
        format!("{request:?}"),
        "JmapMailRequest { account: 3, bearer: true, window: 300, list_folders: true, scope: \"light+2\", mailboxes: 2, known: 3, changes: 3, sends: 1, background: true }"
    );
    assert_eq!(
        format!("{result:?}"),
        "JmapMailResult { account: 3, changes: 4, sent: 1, listed: Some(1), pages: 1, messages: 1, errors: 1, busy_for: Some(30), partial: true, whole: true, stopped: true, progress: true, auth_refused: true }"
    );
}

#[test]
fn the_new_marks_of_the_server_are_written_only_when_they_are_set() {
    let plain = ServerMessage { id: "AAkA".into(), folder: "F1".into(), ..Default::default() };
    assert_eq!(round_trip(&plain), r#"{"id":"AAkA","folder":"F1","read":false,"flag":"none","importance":"normal"}"#);
    let marked = ServerMessage { answered: true, forwarded: true, ..plain };
    assert_eq!(
        round_trip(&marked),
        r#"{"id":"AAkA","folder":"F1","read":false,"answered":true,"forwarded":true,"flag":"none","importance":"normal"}"#
    );
    // One from before reads as neither answered nor forwarded.
    let old: ServerMessage = serde_json::from_str(r#"{"id":"AAkA","folder":"F1","read":true}"#).unwrap();
    assert!(old.read && !old.answered && !old.forwarded);
}
