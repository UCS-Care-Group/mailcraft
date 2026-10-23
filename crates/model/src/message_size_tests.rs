// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn server(size: u64) -> Option<crate::sync::ServerMessage> {
    Some(crate::sync::ServerMessage { size, ..Default::default() })
}

fn whole() -> Message {
    Message { body: Body { text: "x".repeat(2000), html: None }, ..Default::default() }
}

fn preview() -> Body {
    Body { text: "Only the start".into(), html: None }
}

/// #273: a message all of which is here has its own size, whatever its server said.
#[test]
fn a_whole_message_has_its_own_size() {
    assert_eq!(whole().list_size(), Some(2000));
    assert_eq!(Message { server: server(30 << 20), ..whole() }.list_size(), Some(2000));
}

/// #273: of a Microsoft account's message only partly here, the size its server said (or, #300,
/// that of the whole message when it was fetched); what is here when there is none.
#[test]
fn a_message_only_partly_here_has_its_servers_size_when_there_is_one() {
    let unfetched = Message { body: preview(), unfetched: true, server: server(30 << 20), ..Default::default() };
    assert_eq!(unfetched.list_size(), Some(30 << 20));
    let incomplete = Message { body: preview(), incomplete: true, server: server(4096), ..Default::default() };
    assert_eq!(incomplete.list_size(), Some(4096));
    // No size known (a Graph delta never gives one): what is here. A whole message with an
    // unreadable character is marked to be fetched again and keeps its body meanwhile.
    assert_eq!(Message { unfetched: true, server: server(0), ..whole() }.list_size(), Some(2000));
    assert_eq!(Message { unfetched: true, ..whole() }.list_size(), Some(2000));
    let preview_only = Message { body: preview(), unfetched: true, server: server(0), ..Default::default() };
    assert_eq!(preview_only.list_size(), Some(preview_only.size()));
}

/// #300: an IMAP message too large to download has the size its server gave, whatever header
/// it has; without that, a header of the mark's name is not believed, whoever wrote it.
#[test]
fn a_message_too_large_to_download_has_its_size_on_the_server() {
    let header = |name: &str, size: &str| vec![(name.to_string(), size.to_string())];
    let too_large = Message { body: preview(), not_downloaded: 62_914_560, ..Default::default() };
    assert_eq!(too_large.list_size(), Some(62_914_560));
    let marked = Message { headers: header(NOT_DOWNLOADED, "62914560"), ..too_large.clone() };
    assert_eq!(marked.list_size(), Some(62_914_560));
    // A sender's header saying otherwise changes nothing.
    assert_eq!(Message { headers: header(NOT_DOWNLOADED, "1"), ..too_large.clone() }.list_size(), Some(62_914_560));
    // Only the header (a sender's, or one a file from before kept): no size.
    for name in [NOT_DOWNLOADED, "x-mailcraft-not-downloaded"] {
        let only_header = Message { body: preview(), headers: header(name, "62914560"), ..Default::default() };
        assert_eq!(only_header.list_size(), None, "{name}");
    }
}

/// #300: the size kept in the mailbox file, under its own name, and only when there is one; a
/// file from before has none.
#[test]
fn the_size_of_a_message_too_large_is_written_and_read_back() {
    let too_large = Message { id: 9, body: preview(), not_downloaded: 62_914_560, ..Default::default() };
    let json = serde_json::to_string(&too_large).unwrap();
    assert!(json.contains(r#""notDownloaded":62914560"#), "{json}");
    assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), too_large);
    assert!(!serde_json::to_string(&whole()).unwrap().contains("notDownloaded"));
    let before: Message = serde_json::from_str(r#"{"id": 9, "headers": [["X-MailCraft-Not-Downloaded", "62914560"]]}"#).unwrap();
    assert_eq!((before.not_downloaded, before.list_size()), (0, None));
}
