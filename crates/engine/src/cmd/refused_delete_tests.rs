// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #793: a message deleted for good here whose delete the server refused is never put back
//! hollow (no text, files of no bytes), however many times the mailbox was saved and opened in
//! between, and nothing hollow is ever moved or copied to another mailbox, which would leave an
//! empty copy there while the server deletes the original. The servers' side of each run is
//! written out by hand, as `mailcraft-net` answers it.

use mailcraft_model::sync::{
    FolderListing, GraphFetchResult, GraphFetched, GraphFolder, GraphMailResult, GraphMessage, MessageChange, MessageChangeDone, MessageOutcome,
    ServerMessage,
};
use mailcraft_model::{Address, Attachment, Body, FolderKind, Id, Message, Queued};
use serde_json::json;

use crate::Session;
use crate::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};

const OWNER: &str = "owner@example.org";
const OTHER: &str = "owner@outlook.example";

/// A directory of the test's own, made empty.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-793-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Microsoft account whose server lists an Inbox ("in").
fn microsoft(s: &mut Session, email: &str) -> Id {
    let account = s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_graph_mail(s, listing).unwrap();
    account
}

fn text() -> String {
    "Please pay invoice 42 by Friday.\n".repeat(100)
}

fn plan() -> Vec<u8> {
    "The plan, line by line.\n".repeat(200).into_bytes()
}

/// The message as the server has it, whole: its text and a file, both more than a mailbox file
/// keeps inside itself.
fn invoice(folder: Id) -> Message {
    let server = ServerMessage { id: "AAkA1".into(), folder: "in".into(), size: 12_000, ..Default::default() };
    let mut m = Message {
        folder,
        subject: "Invoice 42".into(),
        message_id: "invoice-42@example.org".into(),
        from: Address::new("Ada", "ada@example.org"),
        to: vec![Address::new("Owner", OWNER)],
        date: 1_700_000_000,
        received: 1_700_000_000,
        body: Body { text: text(), html: None },
        server: Some(server),
        ..Default::default()
    };
    m.attachments.push(Attachment::new("plan.txt", "text/plain", plan()));
    m
}

fn reopen(path: &std::path::Path) -> Session {
    crate::store::clear_cache();
    let mut s = Session::empty();
    s.run("file.open", &json!({"path": path})).unwrap();
    s
}

fn titled(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

/// The answer to the one delete for good of a run.
fn delete_answered(s: &mut Session, account: Id, outcome: MessageOutcome, there: Option<GraphMessage>) {
    let change = graph_mail_request(s, account).unwrap().messages.into_iter().find(|c| matches!(c, MessageChange::Delete { .. })).unwrap();
    let done = MessageChangeDone { change, outcome, there: there.map(Box::new) };
    apply_graph_mail(s, GraphMailResult { account, email: OWNER.into(), messages: vec![done], ..Default::default() }).unwrap();
}

/// What the server says of the message after it refused to delete it.
fn there() -> GraphMessage {
    let mut there = GraphMessage {
        subject: "Invoice 42".into(),
        preview: "Please pay invoice 42 by Friday.".into(),
        has_attachments: true,
        ..Default::default()
    };
    there.server = ServerMessage { id: "AAkA1".into(), folder: "in".into(), ..Default::default() };
    there
}

/// Whether message `id` is here whole: its text and its file as the server has them.
fn whole(s: &Session, id: Id) -> bool {
    let m = s.mailbox.message(id).unwrap();
    let body = s.content(id, crate::Part::Text).map(|t| t.to_vec()).unwrap_or_else(|_| m.body.text.clone().into_bytes());
    let body = if m.body.text.is_empty() { String::from_utf8(body).unwrap() } else { m.body.text.clone() };
    let file = s.content(id, crate::Part::Attachment(0)).map(|d| d.to_vec()).unwrap_or_default();
    !m.unfetched && body.trim_end() == text().trim_end() && file == plan()
}

/// Neither whole nor marked to be fetched again: what #793 put back.
fn hollow(s: &Session, id: Id) -> bool {
    !s.mailbox.message(id).unwrap().unfetched && !whole(s, id)
}

#[test]
fn refused_delete_after_a_reopen_puts_the_message_back_to_be_fetched_whole_and_a_move_then_sends_all_of_it() {
    let dir = scratch("reopen");
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let ms = microsoft(&mut s, OWNER);
    microsoft(&mut s, OTHER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(invoice(inbox)).unwrap();
    s.run("file.save", &json!({"path": path})).unwrap();
    // Opened again: its file is kept in the content folder, not in memory.
    let mut s = reopen(&path);
    assert!(s.mailbox.message(id).unwrap().attachments[0].data.is_empty(), "kept outside memory");
    assert!(whole(&s, id));
    // Deleted for good while offline, and MailCraft closed.
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    s.run("file.save", &json!({"path": path})).unwrap();
    // Later the server refuses the delete.
    let mut s = reopen(&path);
    let ms = s.mailbox.accounts.iter().find(|a| a.email == OWNER).unwrap().id;
    let other = s.mailbox.accounts.iter().find(|a| a.email == OTHER).unwrap().id;
    delete_answered(&mut s, ms, MessageOutcome::Refused { status: 403 }, Some(there()));
    let back = titled(&s, "Invoice 42");
    assert!(!hollow(&s, back), "put back with no text and an empty file: {:?}", s.mailbox.message(back).unwrap());
    // Saved and opened again, it is still not hollow.
    s.run("file.save", &json!({"path": path})).unwrap();
    let mut s = reopen(&path);
    let back = titled(&s, "Invoice 42");
    assert!(!hollow(&s, back), "{:?}", s.mailbox.message(back).unwrap());
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    // To be fetched again: its text and file are on the server, and nowhere here.
    let m = s.mailbox.message(back).unwrap();
    assert!(m.unfetched && m.has_attachments(), "{m:?}");
    {
        // Until the server has given it whole again, it moves nowhere, and nothing is sent.
        assert!(s.run("mail.move", &json!({"ids": [back], "folder": theirs})).is_err());
        assert!(graph_mail_request(&s, other).unwrap().places.is_empty());
        // Fetched from the server, which still has it, when it is opened (it is older than the
        // mail kept offline, so the background leaves it).
        assert_eq!(graph_fetch_request(&s, ms, Some(back), 1).unwrap().ids, ["AAkA1"]);
        let mime = crate::mime::write(&invoice(inbox), &crate::mime::WriteOptions::default());
        let fetched =
            GraphFetchResult { account: ms, email: OWNER.into(), fetched: vec![GraphFetched { id: "AAkA1".into(), mime }], ..Default::default() };
        apply_graph_fetch(&mut s, fetched).unwrap();
        assert!(whole(&s, back), "{:?}", s.mailbox.message(back).unwrap());
    }
    // Moved to the other account now, the copy made there is the whole message.
    s.run("mail.move", &json!({"ids": [back], "folder": theirs})).unwrap();
    let places = graph_mail_request(&s, other).unwrap().places;
    let [place] = places.as_slice() else { panic!("{places:?}") };
    assert!(place.body.content.contains("Please pay invoice 42 by Friday."), "{:?}", place.body);
    assert_eq!(place.attachments.len(), 1);
    assert_eq!(place.attachments[0].data.as_slice(), plan().as_slice());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_is_kept_for_a_refused_delete_never_counts_on_content_nothing_names() {
    let dir = scratch("in-session");
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let ms = microsoft(&mut s, OWNER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(invoice(inbox)).unwrap();
    s.run("file.save", &json!({"path": path})).unwrap();
    let mut s = reopen(&path);
    // Deleted for good with its file on disk only: what is kept for putting it back names no
    // file that a sweep may take once the mailbox file no longer does; it is to be fetched again.
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let kept = s.mailbox.queued.iter().find_map(|q| match q {
        Queued::DeleteMessage { kept, .. } => kept.as_deref(),
        _ => None,
    });
    let kept = kept.unwrap();
    assert!(kept.unfetched && kept.attachments.is_empty() && kept.kept.is_none() && kept.has_attachments(), "{kept:?}");
    // One whole in memory is kept whole: put back at once, with all that was fetched.
    let mut s = Session::empty();
    let ms = microsoft(&mut s, OWNER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(invoice(inbox)).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    delete_answered(&mut s, ms, MessageOutcome::Refused { status: 403 }, Some(there()));
    assert!(whole(&s, titled(&s, "Invoice 42")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_kept_copy_read_back_from_a_file_is_fetched_again_rather_than_believed() {
    // As 0.5.0 to now wrote it: the copy's text and files outside the file, and so lost.
    let json = r#"{"deleteMessage": {"account": 3, "id": "AAkA1", "kept": {"id": 9, "folder": 4, "subject": "Invoice 42",
        "body": {"text": ""}, "attachments": [{"name": "plan.txt", "mime": "text/plain", "data": ""}],
        "server": {"id": "AAkA1", "folder": "in"}}}}"#;
    let q: Queued = serde_json::from_str(json).unwrap();
    let Queued::DeleteMessage { kept: Some(kept), .. } = q else { panic!("{q:?}") };
    assert!(kept.unfetched && kept.attachments.is_empty() && kept.has_attachments(), "{kept:?}");
    // Written, a copy whole in memory goes as what reading it back would leave: no body or
    // files written into the file at every save only to be let go of.
    let mut whole = Message { subject: "Invoice 42".into(), body: Body { text: text(), html: Some("<p>Pay</p>".into()) }, ..Default::default() };
    whole.attachments.push(Attachment::new("plan.txt", "text/plain", plan()));
    let q = Queued::DeleteMessage { account: 3, id: "AAkA1".into(), kept: Some(Box::new(whole)) };
    let written = serde_json::to_value(&q).unwrap();
    let kept = &written["deleteMessage"]["kept"];
    assert_eq!((kept["unfetched"].clone(), kept["attachments"].clone()), (json!(true), serde_json::Value::Null), "{kept}");
    assert_eq!(kept["body"]["text"].as_str().unwrap().chars().count(), mailcraft_model::PREVIEW_CHARS);
    assert!(kept["body"].get("html").is_none_or(serde_json::Value::is_null), "{kept}");
    // A short text in memory, its HTML on disk: the text is the preview. One that couldn't be
    // read gives none (the words saying so aren't its text).
    let mut m = Message { body: Body { text: "Short.".into(), html: None }, ..Default::default() };
    let r = mailcraft_model::ContentRef { sha256: [7; 32], len: 4096 };
    m.kept = Some(Box::new(mailcraft_model::KeptBody { html: Some(r), ..Default::default() }));
    assert!(m.body_is_kept());
    m.to_fetch_again();
    assert_eq!((m.body.text.as_str(), m.unfetched), ("Short.", true));
    let mut m = Message { body: Body { text: "[unavailable]".into(), html: None }, ..Default::default() };
    m.kept = Some(Box::new(mailcraft_model::KeptBody { text: Some(r), failed: true, ..Default::default() }));
    m.to_fetch_again();
    assert_eq!(m.body.text, "");
    // A queue entry without a copy still reads as one.
    let q: Queued = serde_json::from_str(r#"{"deleteMessage": {"account": 3, "id": "AAkA1"}}"#).unwrap();
    assert_eq!(q, Queued::DeleteMessage { account: 3, id: "AAkA1".into(), kept: None });
}

#[test]
fn mail_put_back_hollow_by_an_earlier_version_is_fetched_again_once_and_moves_nowhere_till_then() {
    let dir = scratch("legacy");
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let ms = microsoft(&mut s, OWNER);
    let other = microsoft(&mut s, OTHER);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    // What an earlier version put back: no text, a file of no bytes.
    let mut m = invoice(inbox);
    m.body = Body::default();
    m.attachments[0].data = std::sync::Arc::new(Vec::new());
    let hollowed = s.mailbox.add_message(m).unwrap();
    // A message with its text and no file is whole: it is left alone.
    let mut fine = invoice(inbox);
    (fine.subject, fine.attachments, fine.server.as_mut().unwrap().id) = ("Fine".into(), Vec::new(), "AAkA2".into());
    let fine = s.mailbox.add_message(fine).unwrap();
    s.run("file.save", &json!({"path": path})).unwrap();
    let mut s = reopen(&path);
    let m = s.mailbox.message(hollowed).unwrap();
    assert!(m.unfetched && m.has_attachments(), "{m:?}");
    assert!(!s.mailbox.message(fine).unwrap().unfetched);
    assert_eq!(graph_fetch_request(&s, ms, Some(hollowed), 1).unwrap().ids, ["AAkA1"]);
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    assert!(s.run("mail.move", &json!({"ids": [hollowed], "folder": theirs})).is_err());
    // Once: a message fetched and still empty isn't fetched at every start.
    s.mailbox.message_mut(hollowed).unwrap().unfetched = false;
    s.run("file.save", &json!({"path": path})).unwrap();
    let s = reopen(&path);
    assert!(!s.mailbox.message(hollowed).unwrap().unfetched);
    let _ = std::fs::remove_dir_all(dir);
}
