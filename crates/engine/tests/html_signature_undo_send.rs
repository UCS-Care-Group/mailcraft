// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature in a message taken back out of the Outbox (#1652, with #950's Undo Send,
//! Cancel Send and Open Draft of delayed mail): the message comes back with its signature as it
//! was inserted, its style rule and its table and its picture in place, and sent again it carries
//! all of them, the picture bound to the HTML and no stray attachment. What the message waiting
//! in the Outbox keeps of its signature for that ([`Message::draft_signature`]) stays on this
//! computer: no request handed to a host for a server carries it, and it is let go of once the
//! message has gone or has left the Outbox.
//!
//! The people, hosts and picture are invented: the picture is the synthetic one-pixel PNG the
//! MIME tests use. Nothing here reads a file, fetches or sends.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::base64_encode;
use mailcraft_mime_model::WriteOptions;
use mailcraft_model::sync::{SendDone, SendOutcome, ServerDraft, ServerMessage};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, JmapServer, Mailbox, Message};
use serde_json::{Value, json};

const T0: i64 = 1_800_000_000;
/// A one-pixel PNG, whole and valid: the same fixture the MIME integration tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const NAME: &str = "Northwind Studio";
const PICTURE_FILE: &str = "studio-mark.png";
const SHEET: &str = "<style>.studio-card { color: #1f4e79; font-weight: bold; padding: 13px; }</style>";
const BODY: &str = r#"<table cellpadding="0" cellspacing="0" border="0" style="background-color:#fdf6e3"><tr>
  <td style="padding-right:16px"><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td><div class="studio-card">Northwind Studio card</div><div>Priya Raman - Northwind Atelier</div></td>
</tr></table>"#;
/// The declarations of the signature's class rule, as they go with the message.
const RULE: &str = "color: #1f4e79; font-weight: bold; padding: 13px;";
/// The key a serialised snapshot would be under.
const SNAPSHOT_KEY: &str = "draftSignature";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The HTML signature imported into `s` and made the default of `account` for new messages.
fn with_signature(s: &mut Session, account: Id) {
    let html = format!("<html><head>{SHEET}</head><body>{BODY}</body></html>");
    if s.mailbox.signature(NAME).is_none() {
        run(s, "signature.import", json!({"name": NAME, "html": html, "assets": [{"name": PICTURE_FILE, "data": base64_encode(ONE_PIXEL_PNG)}]}));
    }
    run(s, "signature.defaults", json!({"account": account, "new": NAME}));
}

/// A new message from `account`, with its HTML signature in.
fn write(s: &mut Session, account: Id) {
    run(s, "mail.new", json!({"account": account, "to": "maya@lumen.example", "subject": "Hello", "body": "Thanks,"}));
    assert!(s.compose.as_ref().and_then(|c| c.signature_html()).is_some(), "fixture: the HTML signature went in");
}

/// An IMAP account (one that sends its mail, so Send queues in the Outbox) with the HTML
/// signature as its default, and a message being written with it.
fn writing() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), T0, 0);
    let a = run(
        &mut s,
        "account.add",
        json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example", "port": 587, "security": "starttls"}}),
    )["id"]
        .as_u64()
        .unwrap();
    with_signature(&mut s, a);
    write(&mut s, a);
    (s, a)
}

fn send(s: &mut Session) -> Id {
    let sent = run(s, "compose.send", json!({}));
    assert_eq!(sent["queued"].as_bool(), Some(true), "fixture: it waits in the Outbox: {sent}");
    sent["id"].as_u64().unwrap()
}

/// The message taken back is being written again as it was: the owner's words, the signature's
/// words, and the HTML signature itself, as stored, with its picture as the signature's own
/// rather than as a file attached to the message.
fn assert_given_back(label: &str, s: &Session) {
    let c = s.compose.as_ref().unwrap_or_else(|| panic!("{label}: a message is being written"));
    let words = c.body.text().text();
    assert!(words.contains("Thanks,") && words.contains("Priya Raman - Northwind Atelier"), "{label}: the words: {words:?}");
    let stored = s.mailbox.signature(NAME).unwrap();
    let inserted = c.signature.as_ref().unwrap_or_else(|| panic!("{label}: the message lost its HTML signature (it came back as plain rich text)"));
    assert_eq!(inserted.name, NAME, "{label}: the signature's name");
    assert_eq!(c.signature_html(), stored.html(), "{label}: the HTML signature goes out unedited, as imported");
    assert_eq!(inserted.pictures.len(), 1, "{label}: the signature keeps its picture");
    let attached: Vec<(String, bool)> = c.attachments.iter().map(|a| (a.name.clone(), a.inline)).collect();
    assert!(attached.is_empty(), "{label}: the signature's picture is back in the signature, not an attachment: {attached:?}");
}

/// What goes out: the signature's style rule and table, and its one picture as an inline part
/// the HTML names, with no other part, in the stored message and in its MIME.
fn assert_sent_whole(label: &str, m: &Message) {
    let written = mailcraft_mime_model::write(m, &WriteOptions { include_bcc: true, ..Default::default() });
    let parsed = mailcraft_mime_model::parse(&written);
    for (what, m) in [("stored", m), ("written", &parsed)] {
        let html = m.body.html.as_deref().unwrap_or_else(|| panic!("{label} ({what}): an HTML message"));
        assert!(html.contains(RULE), "{label} ({what}): the signature's style rule goes with it: {html}");
        assert!(html.contains("<table") && html.contains("Northwind Studio card"), "{label} ({what}): the signature's table: {html}");
        // The pictures' sources as the reader finds them, compared whole: not a `cid:` in the
        // text, nor one that only starts with the part's Content-ID.
        let sources: Vec<String> = mailcraft_html::image_sources(html)
            .unwrap_or_else(|e| panic!("{label} ({what}): its pictures: {e}"))
            .into_iter()
            .map(|s| s.src)
            .collect();
        let parts: Vec<(String, bool, bool)> = m
            .attachments
            .iter()
            .map(|a| (a.name.clone(), a.inline, !a.content_id.is_empty() && sources.iter().any(|src| *src == format!("cid:{}", a.content_id))))
            .collect();
        assert_eq!(parts.len(), 1, "{label} ({what}): the signature's picture and nothing else goes: {parts:?}");
        let a = &m.attachments[0];
        assert!(a.inline && a.data.as_slice() == ONE_PIXEL_PNG, "{label} ({what}): the part is the signature's picture: {parts:?}");
        assert!(parts[0].2, "{label} ({what}): the HTML names the picture's part, so it shows in the signature: {parts:?}");
    }
}

/// Send what is being written again, at once, and the message that waits in the Outbox.
fn send_again(s: &mut Session) -> Message {
    run(s, "sync.waitBeforeSending", json!({"seconds": 0}));
    run(s, "compose.delay", json!({"at": null}));
    let id = send(s);
    s.mailbox.message(id).unwrap().clone()
}

#[test]
fn undo_send_gives_the_html_signature_back_as_it_was_and_it_goes_again_whole() {
    let (mut s, _) = writing();
    run(&mut s, "sync.waitBeforeSending", json!({"seconds": 10}));
    let id = send(&mut s);
    assert_sent_whole("queued", s.mailbox.message(id).unwrap());
    run(&mut s, "compose.undoSend", json!({}));
    assert_given_back("undo send", &s);
    let again = send_again(&mut s);
    assert_sent_whole("sent again after Undo Send", &again);
}

#[test]
fn cancel_send_then_open_draft_gives_the_html_signature_back_and_it_goes_again_whole() {
    let (mut s, _) = writing();
    let id = run(&mut s, "compose.sendLater", json!({"at": T0 + 3600}))["id"].as_u64().unwrap();
    run(&mut s, "mail.cancelSend", json!({"id": id}));
    let draft = s.mailbox.message(id).unwrap();
    assert!(draft.draft && draft.draft_signature.is_some(), "the draft it became keeps what it needs to know its signature again");
    run(&mut s, "mail.editDraft", json!({"id": id}));
    assert_given_back("cancel send, open draft", &s);
    let again = send_again(&mut s);
    assert_sent_whole("sent again after Cancel Send", &again);
}

#[test]
fn open_draft_of_a_delayed_message_gives_the_html_signature_back_and_it_goes_again_whole() {
    let (mut s, _) = writing();
    let id = run(&mut s, "compose.sendLater", json!({"at": T0 + 3600}))["id"].as_u64().unwrap();
    run(&mut s, "mail.editDraft", json!({"id": id}));
    assert_given_back("open draft of a delayed message", &s);
    let again = send_again(&mut s);
    assert_sent_whole("sent again after Open Draft", &again);
}

/// The snapshot is kept with the queued message in the mailbox (its file too, so that Undo Send
/// and Open Draft after a restart give the signature back), and a Send/Receive's request carries
/// the message without it.
#[test]
fn a_queued_message_keeps_its_snapshot_here_and_the_imap_request_carries_none() {
    let (mut s, a) = writing();
    run(&mut s, "sync.waitBeforeSending", json!({"seconds": 0}));
    let id = send(&mut s);
    assert!(s.mailbox.message(id).unwrap().draft_signature.is_some(), "the queued message keeps its signature's snapshot here");
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let read = mailcraft_engine::io::read_mailbox(&bytes).unwrap();
    assert!(read.message(id).unwrap().draft_signature.is_some(), "and in the mailbox file");

    let request = mailcraft_engine::cmd::account::sync_request(&s, a).unwrap();
    assert_eq!(request.outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [id], "the request sends it");
    assert!(request.outbox.iter().all(|m| m.draft_signature.is_none()), "but without the snapshot");
    let answered = run(&mut s, "sync.request", json!({"account": a}));
    assert_eq!(answered["outbox"][0]["id"].as_u64(), Some(id));
    assert!(!answered.to_string().contains(SNAPSHOT_KEY), "sync.request's answer carries no snapshot: {answered}");
    // The message itself still has it.
    assert!(s.mailbox.message(id).unwrap().draft_signature.is_some());
}

/// The runs of the other kinds of account (Exchange Web Services, JMAP, Microsoft Graph) send
/// the queued message whole too, and without the snapshot.
#[test]
fn exchange_jmap_and_graph_sends_carry_no_snapshot() {
    let mut mb = Mailbox::with_local_account("", "");
    let server = JmapServer { session: "https://jmap.example/.well-known/jmap".into(), bearer: true };
    let jmap = mb.add_account(Account { email: "avery@jmap.example".into(), kind: AccountKind::Jmap, jmap: Some(server), ..Default::default() });
    let mut s = Session::at(mb, T0, 0);
    run(&mut s, "sync.waitBeforeSending", json!({"seconds": 0}));
    let ews = run(
        &mut s,
        "account.add",
        json!({"email": "avery@ews.example", "kind": "exchange", "ews": {"url": "https://exchange.ews.example/EWS/Exchange.asmx"}}),
    )["id"]
        .as_u64()
        .unwrap();
    let graph = run(&mut s, "account.add", json!({"email": "avery@graph.example", "kind": "microsoft"}))["id"].as_u64().unwrap();
    for (label, account, request) in [
        ("exchange", ews, json!({"method": "sync.ewsRequest", "params": {"account": ews}})),
        ("jmap", jmap, json!({"method": "sync.jmapRequest", "params": {"account": jmap, "light": true}})),
        ("graph", graph, Value::Null),
    ] {
        with_signature(&mut s, account);
        write(&mut s, account);
        let id = send(&mut s);
        assert!(s.mailbox.message(id).unwrap().draft_signature.is_some(), "{label}: the queued message keeps its snapshot here");
        let sends = mailcraft_engine::cmd::graph_send::sends(&s, account);
        let up = sends.iter().find(|up| up.message == id).unwrap_or_else(|| panic!("{label}: the run sends it: {sends:?}"));
        if let Some(mime) = &up.mime {
            assert!(mime.draft_signature.is_none(), "{label}: the message handed to the host has no snapshot");
        }
        let answered = serde_json::to_string(&sends).unwrap();
        assert!(!answered.contains(SNAPSHOT_KEY), "{label}: what goes to the host carries no snapshot: {answered}");
        if let Some(method) = request["method"].as_str() {
            let answered = run(&mut s, method, request["params"].clone());
            assert!(answered["sends"].as_array().is_some_and(|a| !a.is_empty()), "{label}: {method} sends it: {answered}");
            assert!(!answered.to_string().contains(SNAPSHOT_KEY), "{label}: {method}'s answer carries no snapshot: {answered}");
        }
    }
}

/// Once the server has taken it, the message is filed in Sent Items without the snapshot.
#[test]
fn a_message_filed_in_sent_items_lets_its_snapshot_go() {
    let (mut s, a) = writing();
    run(&mut s, "sync.waitBeforeSending", json!({"seconds": 0}));
    let id = send(&mut s);
    assert!(s.mailbox.message(id).unwrap().draft_signature.is_some());
    run(&mut s, "sync.apply", json!({"result": {"account": a, "sent": [id]}}));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Sent), "fixture: it is filed in Sent Items");
    assert!(m.draft_signature.is_none(), "a message that has gone keeps no snapshot");
}

/// A Microsoft account's message sent as the draft its server has stays here as the message
/// sent, in Sent Items: without the snapshot.
#[test]
fn a_message_sent_as_its_server_draft_lets_its_snapshot_go() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), T0, 0);
    run(&mut s, "sync.waitBeforeSending", json!({"seconds": 0}));
    let a = run(&mut s, "account.add", json!({"email": "avery@graph.example", "kind": "microsoft"}))["id"].as_u64().unwrap();
    with_signature(&mut s, a);
    write(&mut s, a);
    let id = send(&mut s);
    {
        let m = s.mailbox.message_mut(id).unwrap();
        assert!(m.draft_signature.is_some(), "fixture: the queued message keeps its snapshot");
        m.server = Some(ServerMessage { id: "AAkD1".into(), ..Default::default() });
        m.server_draft = Some(ServerDraft { marker: "mailcraft-draft-1-1".into(), ..Default::default() });
    }
    let done = SendDone { message: id, outcome: SendOutcome::Sent, ..Default::default() };
    let lang = s.interface_language;
    mailcraft_engine::cmd::graph_send::send_done_at(&mut s.mailbox, a, &done, lang, &mut Vec::new(), Some(T0));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Sent), "fixture: kept as the message sent, in Sent Items");
    assert!(m.draft_signature.is_none(), "a message that has gone keeps no snapshot");
}

/// Moved out of the Outbox by the owner, or deleted, it is no longer waiting to be sent: the
/// snapshot goes, from it and from a copy of it.
#[test]
fn a_message_moved_out_of_the_outbox_deleted_or_copied_lets_its_snapshot_go() {
    for how in ["move", "delete", "copy"] {
        let (mut s, a) = writing();
        run(&mut s, "sync.waitBeforeSending", json!({"seconds": 0}));
        let id = send(&mut s);
        assert!(s.mailbox.message(id).unwrap().draft_signature.is_some());
        let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
        match how {
            "move" => drop(run(&mut s, "mail.move", json!({"ids": [id], "folder": inbox}))),
            "delete" => drop(run(&mut s, "mail.delete", json!({"ids": [id]}))),
            _ => drop(run(&mut s, "mail.copy", json!({"ids": [id], "folder": inbox}))),
        }
        if how == "copy" {
            let copies: Vec<bool> = s.mailbox.messages_in(inbox).map(|m| m.draft_signature.is_some()).collect();
            assert_eq!(copies, [false], "copy: one copy in the Inbox, and it keeps no snapshot");
            continue;
        }
        let m = s.mailbox.message(id).unwrap_or_else(|| panic!("{how}: the message is kept somewhere"));
        assert_ne!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Outbox), "{how}: fixture: it left the Outbox");
        assert!(m.draft_signature.is_none(), "{how}: a message no longer waiting to be sent keeps no snapshot");
    }
}
