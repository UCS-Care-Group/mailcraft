// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's labels of a message (#1153).

use proptest::prelude::*;

use crate::gmail::{MAX_GMAIL_LABEL_LEN, MAX_GMAIL_LABELS, is_system_label, label_leaf, label_name, tidy_labels};
use crate::search::{Query, suggestions};
use crate::sync::{FolderDelta, GmailLabels, SyncResult};
use crate::*;

fn labelled(labels: &[&str]) -> Message {
    Message { gmail_labels: tidy_labels(labels), ..Default::default() }
}

#[test]
fn labels_are_kept_clean_and_few() {
    // Control and format characters go, white space runs together, empty ones and repeats go.
    let got = tidy_labels(["  Work\u{202e}/Clients ", "a\0b", "\u{200b}", "", "\\", "work/clients", "Line\r\nbreak", "\\Inbox"]);
    assert_eq!(got, ["Work/Clients", "ab", "Line break", "\\Inbox"]);
    // At most so many, each at most so long.
    let many: Vec<String> = (0..1000).map(|n| format!("L{n}")).collect();
    assert_eq!(tidy_labels(&many).len(), MAX_GMAIL_LABELS);
    let long = tidy_labels(["é".repeat(1000)]);
    assert_eq!(long.first().map(|l| l.chars().count()), Some(MAX_GMAIL_LABEL_LEN));
}

#[test]
fn system_labels_and_paths() {
    assert!(is_system_label("\\Important") && !is_system_label("Important"));
    assert_eq!(label_name("\\Starred"), "Starred");
    assert_eq!(label_leaf("Work/Clients/Acme"), "Acme");
    assert_eq!(label_leaf("Work/"), "Work");
    assert_eq!(label_leaf("/"), "/");
    assert_eq!(label_leaf("Plain"), "Plain");
}

#[test]
fn shown_labels_leave_out_gmails_own_and_the_folder_in_view() {
    let m = labelled(&["\\Inbox", "\\Important", "\\Starred", "Work/Clients", "Receipts", "\\Sent"]);
    assert_eq!(m.shown_gmail_labels("INBOX").collect::<Vec<_>>(), ["Work/Clients", "Receipts"]);
    assert_eq!(m.shown_gmail_labels("work/clients").collect::<Vec<_>>(), ["Receipts"]);
}

#[test]
fn labels_round_trip_through_a_file_and_older_files_read() {
    let m = labelled(&["Receipts", "\\Inbox"]);
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["gmailLabels"], serde_json::json!(["Receipts", "\\Inbox"]));
    let back: Message = serde_json::from_value(json).unwrap();
    assert_eq!(back.gmail_labels, m.gmail_labels);
    // None: not written at all.
    let plain = serde_json::to_value(Message::default()).unwrap();
    assert!(plain.get("gmailLabels").is_none());
    let back: Message = serde_json::from_value(plain).unwrap();
    assert!(back.gmail_labels.is_empty());
    // A file that holds something else there, or dirty labels, reads cleaned.
    for (held, want) in [
        (serde_json::json!("Receipts"), vec![]),
        (serde_json::json!({"a": 1}), vec![]),
        (serde_json::json!(["ok", 5, null, ["x"], "\u{202e}ok2\u{0007}"]), vec!["ok".to_string(), "ok2".to_string()]),
    ] {
        let back: Message = serde_json::from_value(serde_json::json!({ "gmailLabels": held })).unwrap();
        assert_eq!(back.gmail_labels, want);
    }
}

#[test]
fn a_message_that_moves_forgets_its_labels() {
    let mut m = labelled(&["Receipts"]);
    m.folder = 3;
    m.lies_in(3, 1, 9, None);
    assert_eq!(m.gmail_labels, ["Receipts"]);
    m.lies_in(4, 1, 0, None);
    assert!(m.gmail_labels.is_empty());
}

#[test]
fn a_copy_takes_no_labels() {
    // A Gmail account's labelled message, copied into a folder on this computer and into another
    // folder of its own account: neither copy shows, finds or tells agents of the labels.
    let mut mb = Mailbox::with_local_account("Rowan", "rowan@example.org");
    let local = mb.accounts.first().map_or(0, |a| a.id);
    let gmail = mb.add_account(Account {
        email: "rowan@mail.example".into(),
        incoming: Some(Server { host: "imap.gmail.com".into(), ..Default::default() }),
        ..Default::default()
    });
    let inbox = mb.folder_of_kind(gmail, FolderKind::Inbox).unwrap();
    let archive = mb.folder_of_kind(gmail, FolderKind::Archive).unwrap();
    let here = mb.add_folder(local, None, "Kept").unwrap();
    let id =
        mb.add_message(Message { folder: inbox, account: gmail, uid: 3, subject: "Labelled".into(), ..labelled(&["\\Inbox", "Receipts"]) }).unwrap();
    for folder in [here, archive] {
        let copy = mb.copy_message(id, folder).unwrap();
        let m = mb.message(copy).unwrap();
        assert!(m.gmail_labels.is_empty(), "{folder}");
        assert!(!Query::parse("label:receipts").matches(m, 0));
    }
    // The message itself keeps them.
    assert_eq!(mb.message(id).map(|m| m.gmail_labels.len()), Some(2));
}

#[test]
fn pieces_of_a_folder_keep_their_labels() {
    let piece = |uid: u32| FolderDelta {
        remote: "INBOX".into(),
        uid_validity: 1,
        gmail_labels: vec![GmailLabels { uid, labels: vec!["Receipts".into()] }],
        ..Default::default()
    };
    let mut result = SyncResult { folders: vec![piece(1)], ..Default::default() };
    result.absorb(SyncResult { folders: vec![piece(2)], ..Default::default() });
    assert_eq!(result.folders.len(), 1);
    assert_eq!(result.folders.first().map(|f| f.gmail_labels.iter().map(|l| l.uid).collect::<Vec<_>>()), Some(vec![1, 2]));
}

#[test]
fn the_search_box_finds_labels() {
    let m = labelled(&["\\Inbox", "\\Important", "Work/Clients", "Café receipts"]);
    let hit = |q: &str| Query::parse(q).matches(&m, 0);
    assert!(hit("label:clients") && hit("label:work/clients") && hit("label:WORK"));
    assert!(hit("label:important") && hit("labels:inbox") && hit("label:\"café receipts\""));
    assert!(!hit("label:personal") && hit("-label:personal") && !hit("-label:work"));
    // A label of nothing but space finds nothing (`label:""` is a keyword with nothing after it,
    // which asks for nothing, as every keyword's does).
    assert!(!hit("label:\" \""));
    // Words of a label are not words of the message.
    assert!(!hit("clients"));
    // A message of no Gmail account has none.
    assert!(!Query::parse("label:work").matches(&Message::default(), 0));
    // The keyword is offered as the others are.
    let offered: Vec<String> = suggestions("lab", 3, &[]).into_iter().map(|s| s.text).collect();
    assert_eq!(offered, ["label:"]);
}

proptest! {
    #[test]
    fn any_labels_are_cleaned_within_bounds(labels in proptest::collection::vec(any::<String>(), 0..300)) {
        let got = tidy_labels(&labels);
        prop_assert!(got.len() <= MAX_GMAIL_LABELS);
        for label in &got {
            prop_assert!(!label.is_empty() && label.chars().count() <= MAX_GMAIL_LABEL_LEN);
            prop_assert!(!label.chars().any(|c| c.is_control() || mailcraft_names::is_format(c) && !mailcraft_names::is_joiner(c)));
            let _ = label_leaf(label);
        }
    }
}
