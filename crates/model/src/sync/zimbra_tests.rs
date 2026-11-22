// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::FolderKind;
use crate::sync::{FolderChange, GraphFolder, GraphMessage, ServerMessage};

const ME: &str = "ada@example.org";
const URL: &str = "https://secret-host.example.org";

fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap();
    assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
    json
}

fn request() -> ZimbraMailRequest {
    ZimbraMailRequest {
        account: 3,
        email: ME.into(),
        url: URL.into(),
        username: "ada".into(),
        trusted: "ab".repeat(32),
        token: "4711".into(),
        changes: vec![FolderChange::Delete { id: "257".into(), shown: Default::default() }],
        background: true,
        ..Default::default()
    }
}

fn message(id: &str, folder: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), ..Default::default() },
        subject: "Secret subject".into(),
        ..Default::default()
    }
}

fn result() -> ZimbraMailResult {
    ZimbraMailResult {
        account: 3,
        email: ME.into(),
        url: URL.into(),
        listing: Some(FolderListing {
            folders: vec![GraphFolder { id: "2".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
            complete: true,
        }),
        well_known: vec!["2".into()],
        pages: vec![
            ZimbraMessagePage { folder: "2".into(), messages: vec![message("260", "2")], afresh: true, last: true, ..Default::default() },
            ZimbraMessagePage { messages: vec![message("261", "5")], removed: vec!["250".into()], last: true, ..Default::default() },
        ],
        from: "4711".into(),
        token: "4712".into(),
        resume: Some(vec![]),
        errors: vec!["one".into()],
        partial: true,
        ..Default::default()
    }
}

#[test]
fn a_request_round_trips_and_an_empty_one_is_small() {
    round_trip(&request());
    round_trip(&ZimbraFetchRequest { account: 3, email: ME.into(), url: URL.into(), ids: vec!["260".into()], ..Default::default() });
    let json = round_trip(&ZimbraMailRequest::default());
    assert!(!json.contains("trusted") && !json.contains("token") && !json.contains("sends"), "{json}");
}

#[test]
fn a_result_round_trips_and_pieces_are_absorbed_in_order() {
    let first = result();
    round_trip(&first);
    assert_eq!(first.check(), Ok(()));
    assert!(first.server_answered());
    let mut whole = first.clone();
    let (millis, requests, skipped) = (whole.timings.millis(), whole.timings.requests(), whole.timings.skipped);
    let mut piece = ZimbraMailResult {
        account: 3,
        pages: vec![ZimbraMessagePage { messages: vec![message("262", "2")], last: true, ..Default::default() }],
        token: "4713".into(),
        resume: Some(vec![]),
        errors: vec!["two".into()],
        whole: true,
        ..Default::default()
    };
    piece.timings.add(crate::sync::SyncPhase::NewMail, 40, 2);
    piece.timings.skipped = 1;
    whole.absorb(piece);
    assert_eq!((whole.pages.len(), whole.errors.len(), whole.from.as_str(), whole.token.as_str()), (3, 2, "4711", "4713"));
    assert_eq!((whole.partial, whole.whole, whole.email.as_str(), whole.url.as_str()), (false, true, ME, URL));
    assert!(whole.listing.is_some());
    assert_eq!((whole.timings.millis(), whole.timings.requests(), whole.timings.skipped), (millis + 40, requests + 2, skipped + 1));
}

#[test]
fn non_ascii_urls_use_the_settings_character_limit_in_results_too() {
    let url = format!("https://mail.example.org/{}", "é".repeat(1_100));
    assert!(url.len() > crate::ZimbraServer::MAX_URL);
    assert!(url.chars().count() <= crate::ZimbraServer::MAX_URL);
    assert_eq!(crate::ZimbraServer::check_url(&url), Ok(()));
    let mut piece = result();
    piece.url = url;
    assert_eq!(piece.check(), Ok(()));
}

#[test]
fn what_is_out_of_bounds_is_refused_whole() {
    let mut odd = result();
    odd.token = "12 34".into();
    assert!(odd.check().is_err());
    let mut odd = result();
    odd.pages[0].messages[0].server.id = "x".repeat(MAX_ZIMBRA_ID + 1);
    assert!(odd.check().is_err());
    let mut odd = result();
    odd.pages[1].removed.push("bad\nid".into());
    assert!(odd.check().is_err());
    // A fresh read is of one folder, which it names.
    let mut odd = result();
    odd.pages[0].folder.clear();
    assert!(odd.check().is_err());
    let mut odd = result();
    odd.pages[0].messages[0].subject = "x".repeat(17 * 1024);
    assert!(odd.check().is_err());
    let mut odd = result();
    odd.errors = vec!["e".into(); MAX_ZIMBRA_ERRORS + 1];
    assert!(odd.check().is_err());
    let mut odd = result();
    odd.untrusted = "nope".into();
    assert!(odd.check().is_err());
    // A conversation's id is a server id like any other, on a page and on a message read back
    // after a refused delete; and that one names itself and its folder as a page's does.
    let mut odd = result();
    odd.pages[0].messages[0].server.thread = "t".repeat(MAX_ZIMBRA_ID + 1);
    assert!(odd.check().is_err());
    let back = |there: GraphMessage| {
        let mut piece = result();
        let change = crate::sync::MessageChange::Delete { id: "260".into() };
        piece.messages = vec![crate::sync::MessageChangeDone { change, outcome: crate::sync::MessageOutcome::Gone, there: Some(Box::new(there)) }];
        piece.check()
    };
    assert_eq!(back(message("260", "2")), Ok(()));
    let mut there = message("260", "2");
    there.server.thread = "bad\nthread".into();
    assert!(back(there).is_err());
    assert!(back(message("bad\nid", "2")).is_err());
    assert!(back(message("", "2")).is_err());
    assert!(back(message("260", &"9".repeat(MAX_ZIMBRA_ID + 1))).is_err());
    let mut odd = result();
    odd.url = "u".repeat(crate::ZimbraServer::MAX_URL + 1);
    assert!(odd.check().is_err());
}

#[test]
fn debug_output_holds_counts_only() {
    let shown = format!("{:?} {:?} {:?}", request(), result(), result().pages[0]);
    for secret in [ME, "secret-host", "4711", "4712", "Secret subject", "260", "abab"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    let fetch = ZimbraFetchRequest { account: 3, email: ME.into(), url: URL.into(), ids: vec!["260".into()], ..Default::default() };
    assert!(!format!("{fetch:?}").contains("260"));
}

#[test]
fn ids_are_held_to_their_size() {
    assert!(zimbra_id_ok("257") && zimbra_id_ok("6f1c6d5a-1111-2222-3333-444455556666:257"));
    assert!(!zimbra_id_ok("") && !zimbra_id_ok("a\u{0}") && !zimbra_id_ok(&"9".repeat(MAX_ZIMBRA_ID + 1)));
}
