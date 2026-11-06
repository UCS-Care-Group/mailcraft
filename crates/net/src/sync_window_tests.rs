// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline at a synchronisation (#653): mail that arrived before the window comes
//! down as its header and the start of its text, without `\Seen`; everything newer, and Drafts,
//! whole.

use mailcraft_model::sync::{FolderState, SyncRequest};
use mailcraft_model::{Account, AccountKind, FolderKind, Message, Security, Server};

use super::{START_BYTES, internal_date};
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";
const OLD: &str = " 1-Jul-2003 09:52:37 +0000";
const NEW: &str = "15-Oct-2026 09:52:37 +0000";

fn eml(subject: &str, text: &str) -> Vec<u8> {
    format!("From: Bob <bob@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\nContent-Type: text/plain\r\n\r\n{text}\r\n")
        .into_bytes()
}

fn message(uid: u32, flags: &str, arrived: &str, body: Vec<u8>) -> FakeMessage {
    FakeMessage { uid, flags: flags.into(), internal_date: arrived.into(), body, claimed_size: None }
}

fn mailbox(raw: &str, flags: &str, messages: Vec<FakeMessage>) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity: 11, messages, refuse_open: false, next_uid: 0 }
}

fn long_text() -> String {
    "Words of a long letter. ".repeat(2_000)
}

fn server() -> FakeImap {
    let mixed = format!(
        "From: Bob <bob@example.org>\r\nSubject: With a file\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n{}\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--b--\r\n",
        "See the file.",
        "QUJD".repeat(10_000)
    );
    // A header longer than the start that is fetched (Gmail's Received and ARC lines).
    let long_header =
        format!("{}From: Bob <bob@example.org>\r\nSubject: Long header\r\n\r\nShort text.\r\n", "Received: from relay.example.org\r\n".repeat(700));
    Fake::imap(ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![
            mailbox(
                "INBOX",
                "\\HasNoChildren",
                vec![
                    message(1, "", OLD, eml("Old", &long_text())),
                    message(2, "", NEW, eml("New", &long_text())),
                    message(3, "", OLD, eml("Old and short", "Short.")),
                    message(4, "\\Draft", OLD, eml("Old draft", &long_text())),
                    message(5, "", OLD, mixed.into_bytes()),
                    message(6, "", OLD, long_header.into_bytes()),
                    FakeMessage { claimed_size: Some(200 * 1024 * 1024), ..message(7, "", OLD, eml("Old and huge", "Big.")) },
                ],
            ),
            mailbox("Drafts", "\\HasNoChildren \\Drafts", vec![message(1, "\\Seen", OLD, eml("Draft", &long_text()))]),
        ],
        ..Default::default()
    })
}

fn request(imap: &FakeImap, keep_since: Option<i64>) -> SyncRequest {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        username: "ada".into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        ..Default::default()
    };
    let folders = FolderKind::STANDARD.iter().zip(100..).map(|(kind, id)| FolderState { id, kind: *kind, ..Default::default() }).collect();
    SyncRequest { account, password: PASSWORD.into(), folders, keep_since, ..Default::default() }
}

fn by_subject<'a>(messages: &'a [Message], subject: &str) -> &'a Message {
    messages.iter().find(|m| m.subject == subject).unwrap_or_else(|| panic!("no {subject}"))
}

#[test]
fn mail_older_than_the_window_comes_as_its_header_and_start() {
    let imap = server();
    let since = internal_date(NEW).unwrap() - 365 * 86_400;
    let result = crate::sync(&request(&imap, Some(since)));
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = result.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    assert_eq!(inbox.new.len(), 7);
    // Too large to take, however old: the header and the note, as before; it is never fetched
    // whole when opened either.
    let huge = by_subject(&inbox.new, "Old and huge");
    assert!(!huge.unfetched && huge.not_downloaded == 200 * 1024 * 1024);
    let old = by_subject(&inbox.new, "Old");
    assert!(old.unfetched, "older than the window");
    assert!(old.body.text.starts_with("Words of a long letter.") && old.body.text.len() < START_BYTES as usize, "{}", old.body.text.len());
    assert_eq!((old.uid, old.received), (1, internal_date(OLD).unwrap()));
    // Newer: whole. Short enough to have come whole in its start: whole too.
    let new = by_subject(&inbox.new, "New");
    assert!(!new.unfetched && new.body.text.len() > 40_000);
    assert!(!by_subject(&inbox.new, "Old and short").unfetched);
    // A draft, in whatever folder: whole.
    assert!(!by_subject(&inbox.new, "Old draft").unfetched);
    let drafts = result.folders.iter().find(|f| f.remote == "Drafts").unwrap();
    assert!(!drafts.new[0].unfetched && drafts.new[0].body.text.len() > 40_000);
    // A message with files says so, and they stay on the server.
    let mixed = by_subject(&inbox.new, "With a file");
    assert!(mixed.unfetched && mixed.attachments_on_server);
    // A header longer than the start: the header alone, with all its fields.
    let long = by_subject(&inbox.new, "Long header");
    assert!(long.unfetched && long.from.email == "bob@example.org");
    let world = imap.world();
    assert!(world.log.iter().any(|line| line.contains("UID FETCH 1 (UID FLAGS INTERNALDATE BODY.PEEK[]<0.16384>)")), "{:?}", world.log);
    assert!(world.log.iter().any(|line| line.contains("UID FETCH 6 (UID FLAGS INTERNALDATE BODY.PEEK[HEADER])")));
    assert!(world.log.iter().any(|line| line.contains("UID FETCH 2 (UID FLAGS INTERNALDATE BODY.PEEK[])")));
    // Nothing on the server changes: no message is marked as read.
    assert!(!world.log.iter().any(|line| line.contains("STORE") || line.contains("SELECT")));
    assert!(world.mailboxes[0].messages.iter().all(|m| !m.has("\\Seen")));
}

#[test]
fn without_a_window_everything_comes_whole() {
    let imap = server();
    let result = crate::sync(&request(&imap, None));
    let inbox = result.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    assert!(inbox.new.iter().all(|m| !m.unfetched));
    assert!(!imap.world().log.iter().any(|line| line.contains("<0.") || line.contains("INTERNALDATE)")));
}

#[test]
fn a_size_said_too_small_does_not_make_a_cut_message_whole() {
    // Exchange says a guess for a message's size, and other servers can say too little: a
    // message of 48,000 bytes said to be 10,000.
    let text = "Words of a long letter. ".repeat(2_000);
    let body = eml("Said small", &text);
    assert!(body.len() > 48_000);
    let short = eml("Short and said large", "Short.");
    let imap = Fake::imap(ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes: vec![mailbox(
            "INBOX",
            "\\HasNoChildren",
            vec![
                FakeMessage { claimed_size: Some(10_000), ..message(1, "", OLD, body) },
                FakeMessage { claimed_size: Some(60_000), ..message(2, "", OLD, short) },
            ],
        )],
        ..Default::default()
    });
    let since = internal_date(NEW).unwrap() - 365 * 86_400;
    let result = crate::sync(&request(&imap, Some(since)));
    assert_eq!(result.errors, Vec::<String>::new());
    let inbox = result.folders.iter().find(|f| f.remote == "INBOX").unwrap();
    let cut = by_subject(&inbox.new, "Said small");
    assert!(cut.unfetched, "only its start came: {}", cut.body.text.len());
    assert!(cut.body.text.len() < START_BYTES as usize);
    // The server ended it before the bytes asked for: all of it came, whatever size was said.
    assert!(!by_subject(&inbox.new, "Short and said large").unfetched);
}
