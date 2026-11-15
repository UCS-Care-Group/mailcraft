// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's labels of messages, read with their flags (#1153), against the fake server.

use mailcraft_model::sync::GmailLabels;

use super::*;

fn gmail() -> Host {
    Host { files_sent: true, gmail: true, ..Default::default() }
}

fn labelled_world() -> ImapWorld {
    let labels = [
        ("INBOX", 1, "\\Inbox \\Important \"Work/Clients\""),
        ("INBOX", 2, "\\Inbox \\Starred {8}\r\nReceipts"),
        ("Projects", 1, "Projects \"Caf&AOk-\""),
    ];
    ImapWorld { gmail: true, gmail_labels: labels.iter().map(|(at, uid, l)| (at.to_string(), *uid, l.to_string())).collect(), ..imap_world() }
}

fn of(uid: u32, labels: &[&str]) -> GmailLabels {
    GmailLabels { uid, labels: labels.iter().map(|l| l.to_string()).collect() }
}

fn new_labels(delta: &mailcraft_model::sync::FolderDelta) -> Vec<(u32, Vec<String>)> {
    delta.new.iter().map(|m| (m.uid, m.gmail_labels.clone())).collect()
}

#[test]
fn labels_come_with_new_mail_and_follow_the_server() {
    let imap = Fake::imap(labelled_world());
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    let s = |v: &[&str]| v.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    assert_eq!(
        new_labels(delta(&result, "INBOX")),
        [(1, s(&["\\Inbox", "\\Important", "Work/Clients"])), (2, s(&["\\Inbox", "\\Starred", "Receipts"])), (5, vec![])]
    );
    // In every folder, not only the Inbox, and decoded as mailbox names are.
    assert_eq!(new_labels(delta(&result, "Projects")), [(1, s(&["Projects", "Café"]))]);
    // Nothing for messages already here at a first run.
    assert!(delta(&result, "INBOX").gmail_labels.is_empty());
    // No command more than without labels: one listing per folder read, and no FETCH by message
    // asks for them.
    let log = imap.world().log.clone();
    assert!(log.iter().filter(|line| line.contains("X-GM-LABELS")).all(|line| line.contains("UID FETCH 1:*")));
    // Gmail's ids are asked of the Inbox alone.
    assert_eq!(log.iter().filter(|line| line.contains("X-GM-MSGID")).count(), 1);
    assert!(log.iter().any(|line| line.contains("UID FETCH 1:* (UID FLAGS X-GM-LABELS)")));

    // Labels change on the server: the next run says so, for the messages already here.
    let folders = applied(&local_folders(), &result);
    imap.world().gmail_labels.retain(|(at, uid, _)| !(at == "INBOX" && *uid == 1));
    imap.world().gmail_labels.push(("INBOX".into(), 1, "\\Inbox Personal".into()));
    let again = whole(&request(account(Some(&imap), None), folders), Limits::in_pieces(), gmail());
    let inbox = delta(&again, "INBOX");
    assert!(inbox.new.is_empty());
    assert_eq!(inbox.gmail_labels, [of(1, &["\\Inbox", "Personal"]), of(2, &["\\Inbox", "\\Starred", "Receipts"]), of(5, &[])]);
}

#[test]
fn hostile_labels_are_cleaned_and_capped() {
    let flood: String = (0..500).map(|n| format!(" L{n}")).collect();
    let labels = [("INBOX", 1, format!("\"a\u{202e}b\" {{3}}\r\nx\0y{flood}")), ("INBOX", 2, "(nested) \"\" ok".to_string())];
    let world =
        ImapWorld { gmail: true, gmail_labels: labels.iter().map(|(at, uid, l)| (at.to_string(), *uid, l.clone())).collect(), ..imap_world() };
    let imap = Fake::imap(world);
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), gmail());
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = new_labels(delta(&result, "INBOX"));
    let one = inbox.iter().find(|(uid, _)| *uid == 1).map(|(_, l)| l.clone()).unwrap_or_default();
    assert_eq!(one.len(), mailcraft_model::gmail::MAX_GMAIL_LABELS);
    assert_eq!(one.get(..2), Some(&["ab".to_string(), "xy".to_string()][..]));
    assert_eq!(inbox.iter().find(|(uid, _)| *uid == 2).map(|(_, l)| l.clone()), Some(vec!["ok".to_string()]));
}

#[test]
fn no_other_server_is_asked_for_labels() {
    let imap = Fake::imap(labelled_world());
    let result = whole(&request(account(Some(&imap), None), local_folders()), Limits::in_pieces(), Host::default());
    assert_eq!(result.errors, Vec::<String>::new());
    assert!(result.folders.iter().all(|f| f.gmail_labels.is_empty() && f.new.iter().all(|m| m.gmail_labels.is_empty())));
    assert!(!imap.world().log.iter().any(|line| line.contains("X-GM-LABELS")));
}
