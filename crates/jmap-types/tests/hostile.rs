// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a server sends is hostile: reading anything at all into the types is an answer or an error,
//! never a panic.

use mailcraft_jmap_types::{Email, EmailQueryRequest, GetResponse, Response, Session, SetResponse, StateChange};
use proptest::prelude::*;

fn read_all(bytes: &[u8]) {
    let _ = serde_json::from_slice::<Session>(bytes);
    let _ = serde_json::from_slice::<Response>(bytes);
    let _ = serde_json::from_slice::<GetResponse<Email>>(bytes);
    let _ = serde_json::from_slice::<SetResponse<Email>>(bytes);
    let _ = serde_json::from_slice::<EmailQueryRequest>(bytes);
    let _ = serde_json::from_slice::<StateChange>(bytes);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        read_all(&bytes);
    }

    #[test]
    fn arbitrary_json_text_never_panics(text in r#"[\[\]{}",:0-9a-zA-Z@#/ -]{0,256}"#) {
        read_all(text.as_bytes());
    }
}

/// A filter nested far deeper than any real one is refused by the JSON reader's depth limit, not followed
/// down the stack.
#[test]
fn a_deeply_nested_filter_is_an_error() {
    let depth = 10_000;
    let mut text = String::from(r#"{"accountId": "a", "filter": "#);
    for _ in 0..depth {
        text.push_str(r#"{"operator": "NOT", "conditions": ["#);
    }
    text.push_str(r#"{"inMailbox": "m"}"#);
    for _ in 0..depth {
        text.push_str("]}");
    }
    text.push('}');
    assert!(serde_json::from_str::<EmailQueryRequest>(&text).is_err());
}
