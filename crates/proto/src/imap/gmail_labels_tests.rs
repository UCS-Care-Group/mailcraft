// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's labels of a message (`X-GM-LABELS`, Google's "Gmail IMAP extensions", #1153).

use proptest::prelude::*;

use super::*;
use crate::script::Script;

fn labels(server: &str) -> Vec<(u32, Option<Vec<String>>)> {
    let mut s = Script::new(server);
    let got = Imap::resume(&mut s).uid_fetch_flags_gmail("1:*").unwrap_or_default();
    got.into_iter().map(|f| (f.uid, f.gmail_labels)).collect()
}

fn owned(labels: &[&str]) -> Option<Vec<String>> {
    Some(labels.iter().map(|l| l.to_string()).collect())
}

#[test]
fn labels_come_with_the_flags() {
    let mut server = String::new();
    // Atoms, quoted strings, Gmail's own with their backslash, an empty list.
    server += "* 1 FETCH (UID 1 FLAGS (\\Seen) X-GM-MSGID 11 X-GM-LABELS (\\Inbox \\Sent Important \"Muy Importante\"))\r\n";
    server += "* 2 FETCH (X-GM-LABELS (foo) UID 2 FLAGS ())\r\n";
    server += "* 3 FETCH (UID 3 FLAGS () X-GM-LABELS ())\r\n";
    // A literal, and modified UTF-7 as mailbox names are written.
    server += "* 4 FETCH (UID 4 FLAGS () X-GM-LABELS ({12}\r\nWork/Clients \"Caf&AOk-\" &AOk-t&AOk-))\r\n";
    // UTF-8 as it stands, and an ampersand that isn't the start of a shifted run.
    server += "* 5 FETCH (UID 5 FLAGS () X-GM-LABELS (\"Été\" \"Tom & Jerry\" \\Drafts))\r\n";
    // A message the server gave none for.
    server += "* 6 FETCH (UID 6 FLAGS ())\r\n";
    server += "A0001 OK done\r\n";
    let mut s = Script::new(server);
    let got = Imap::resume(&mut s).uid_fetch_flags_gmail("1:*").unwrap();
    assert_eq!(s.sent(), "A0001 UID FETCH 1:* (UID FLAGS X-GM-MSGID X-GM-LABELS)\r\n");
    let got: Vec<(u32, Option<Vec<String>>)> = got.into_iter().map(|f| (f.uid, f.gmail_labels)).collect();
    assert_eq!(
        got,
        [
            (1, owned(&["\\Inbox", "\\Sent", "Important", "Muy Importante"])),
            (2, owned(&["foo"])),
            (3, owned(&[])),
            (4, owned(&["Work/Clients", "Café", "été"])),
            (5, owned(&["Été", "Tom & Jerry", "\\Drafts"])),
            (6, None),
        ]
    );
}

#[test]
fn the_later_word_counts_and_other_servers_are_not_asked() {
    let got = labels(
        "* 1 FETCH (UID 1 FLAGS () X-GM-LABELS (a))\r\n* 1 FETCH (UID 1 X-GM-LABELS (b c))\r\n* 1 FETCH (UID 1 FLAGS ())\r\nA0001 OK done\r\n",
    );
    assert_eq!(got, [(1, owned(&["b", "c"]))]);
    // Without asking for them, labels the server gives anyway are not taken.
    let mut s = Script::new("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS (a))\r\nA0001 OK done\r\n");
    let plain = Imap::resume(&mut s).uid_fetch_flags("1:*").unwrap();
    assert_eq!(plain.first().and_then(|f| f.gmail_labels.clone()), None);
    assert_eq!(s.sent(), "A0001 UID FETCH 1:* (UID FLAGS)\r\n");
}

#[test]
fn hostile_label_lists_are_kept_small() {
    // A huge list: only so many are kept.
    let many: String = (0..1000).map(|n| format!(" L{n}")).collect();
    let got = labels(&format!("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS ({many}))\r\nA0001 OK done\r\n"));
    assert_eq!(got.first().and_then(|(_, l)| l.as_ref()).map(Vec::len), Some(MAX_GMAIL_LABELS));
    // A long label is cut, at a character boundary.
    let long = "é".repeat(400);
    let got = labels(&format!("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS (\"{long}\"))\r\nA0001 OK done\r\n"));
    let label = got.first().and_then(|(_, l)| l.as_ref()).and_then(|l| l.first()).cloned().unwrap_or_default();
    assert!(label.len() <= MAX_GMAIL_LABEL_LEN && !label.is_empty() && label.chars().all(|c| c == 'é'));
    // A NUL in a literal is carried as text for the model to clean; a list inside the list is
    // skipped; a list that isn't closed is read as far as it goes.
    let got = labels("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS ({3}\r\na\0b (nested list) ok))\r\nA0001 OK done\r\n");
    assert_eq!(got, [(1, owned(&["a\0b", "ok"]))]);
    let got = labels("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS (a (b\r\nA0001 OK done\r\n");
    assert!(got.len() <= 1);
    // Something other than a list is no labels.
    let got = labels("* 1 FETCH (UID 1 FLAGS () X-GM-LABELS NIL)\r\nA0001 OK done\r\n");
    assert_eq!(got, [(1, None)]);
}

#[test]
fn a_fetch_keeps_no_more_label_bytes_than_its_budget() {
    // Labels as literals, which the cap on a command's text doesn't count: each message with as
    // many as a response may carry, of 256 bytes each. Past 32 MiB the rest come without.
    let label = "x".repeat(MAX_GMAIL_LABEL_LEN);
    let list: String = (0..MAX_LITERALS).map(|_| format!(" {{{}}}\r\n{label}", label.len())).collect();
    let count = MAX_GMAIL_LABEL_BYTES / (MAX_GMAIL_LABEL_LEN * MAX_LITERALS) + 4;
    let mut server = String::new();
    for uid in 1..=count {
        server += &format!("* {uid} FETCH (UID {uid} FLAGS () X-GM-LABELS ({list}))\r\n");
    }
    server += "A0001 OK done\r\n";
    let mut s = Script::new(server);
    let mut imap = Imap::resume(&mut s);
    imap.selected = Selected { exists: u32::try_from(count).unwrap_or(u32::MAX), uid_validity: 1, uid_next: 0 };
    let got = imap.uid_fetch_flags_gmail("1:*").unwrap();
    assert_eq!(got.len(), count);
    let kept: usize = got.iter().filter_map(|f| f.gmail_labels.as_ref()).flatten().map(String::len).sum();
    assert!(kept <= MAX_GMAIL_LABEL_BYTES);
    assert!(got.last().is_some_and(|f| f.gmail_labels.is_none()));
}

proptest! {
    #[test]
    fn arbitrary_label_lists_never_panic(inside in proptest::collection::vec(any::<u8>(), 0..400)) {
        let mut server = b"* 1 FETCH (UID 1 FLAGS () X-GM-LABELS (".to_vec();
        server.extend(inside.iter().filter(|b| **b != b'\r' && **b != b'\n'));
        server.extend_from_slice(b"))\r\nA0001 OK done\r\n");
        let mut s = Script::new(server);
        if let Ok(got) = Imap::resume(&mut s).uid_fetch_flags_gmail("1:*") {
            for labels in got.iter().filter_map(|f| f.gmail_labels.as_ref()) {
                prop_assert!(labels.len() <= MAX_GMAIL_LABELS);
            }
        }
    }

    #[test]
    fn arbitrary_server_bytes_never_panic_in_a_gmail_fetch(server in proptest::collection::vec(any::<u8>(), 0..600)) {
        let mut s = Script::new(server);
        let _ = Imap::resume(&mut s).uid_fetch_flags_gmail("1:*");
    }
}
