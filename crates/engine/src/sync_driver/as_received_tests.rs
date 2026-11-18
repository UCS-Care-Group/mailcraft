// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save Message As writes a message as it was received (#1561): the server's copy asked for for
//! the save, or the bytes the mailbox keeps of a signed message, byte for byte; else it says the
//! file was written from what this computer keeps; and never a part of a message. Synthetic
//! messages and results made by hand (the host's side: `apps/mailcraft`).

use mailcraft_model::digital_signature::Verdict;
use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, EwsMessagePage, GraphFetchResult, GraphFetched, GraphFolder, GraphMessage, ServerMessage,
};
use mailcraft_model::{AccountKind, Address, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use super::as_received::ServerCopy;
use super::fetch_flow::NowNext;
use crate::Session;
use crate::cmd::{ews_mail, graph_mail};

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// A message as a server hands it over, written the way no writer of ours writes one: folded and
/// spaced header lines, a field nothing reads, the parts in an order of their own, an
/// attachment's base64 in short lines. Rebuilt from its parts it comes out otherwise.
const RECEIVED: &[u8] = b"Received: from relay.example.net by mx.example.org;\r\n\tMon, 9 Nov 2026 09:00:00 +0000\r\n\
X-Odd-Field:   kept   as   it   came\r\nSubject: Minutes\r\nFrom: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\n\
Date: Mon, 9 Nov 2026 09:00:00 +0000\r\nMessage-ID: <minutes-1@example.org>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed;\r\n boundary=\"=_one\"\r\n\r\nThis is a message in MIME format.\r\n\r\n--=_one\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nThe minutes are attached.\r\n--=_one\r\n\
Content-Type: application/octet-stream; name=\"minutes.bin\"\r\nContent-Disposition: attachment; filename=\"minutes.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nAAECAwQF\r\nBgcICQoL\r\n--=_one--\r\n";

const CA: &[u8] = include_bytes!("../../../smime/testdata/ca.rsa.der");
const DETACHED: &[u8] = include_bytes!("../../../smime/testdata/detached.eml");
/// 2026-11-09.
const NOW: i64 = 1_794_182_400;

/// An Exchange server account with one message in its Inbox, `A1`, as a listing brings it (only
/// its headers and a preview), and whole when `whole` names the bytes it was fetched with.
fn exchange(whole: Option<&[u8]>) -> (Session, Id, Id) {
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    let added = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let piece = EwsMailResult { account, email: ME.into(), url: URL.into(), partial: true, ..Default::default() };
    let inbox = GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let folders = EwsFolderDeltas { changed: vec![inbox], afresh: true, ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(folders), hierarchy: "H".into(), ..piece.clone() }).unwrap();
    let mail = GraphMessage {
        server: ServerMessage { id: "A1".into(), size: 900, change_key: "CK1".into(), ..Default::default() },
        message_id: "minutes-1@example.org".into(),
        subject: "Minutes".into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: NOW - 60,
        received: NOW,
        preview: "The minutes".into(),
        ..Default::default()
    };
    let page = EwsMessagePage { folder: "F1".into(), messages: vec![mail], afresh: true, last: true, state: "S1".into(), ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece }).unwrap();
    let id = s.mailbox.messages.values().find(|m| m.subject == "Minutes").unwrap().id;
    if let Some(bytes) = whole {
        ews_mail::apply_ews_fetch(&mut s, fetched(account, bytes)).unwrap();
        assert!(!s.mailbox.message(id).unwrap().unfetched);
    }
    s.sel.messages = vec![id];
    (s, account, id)
}

/// A fetch's answer that brings `A1` as `bytes`.
fn fetched(account: Id, bytes: &[u8]) -> GraphFetchResult {
    GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: "A1".into(), mime: bytes.to_vec() }], ..Default::default() }
}

/// Save Message As of `id` as the window runs it: it waits for what it is to wait for, the host
/// answers with `answer`, and it runs again with a path (as once the dialog is answered). What it
/// answered, and what the file holds.
fn save_as_the_window_does(s: &mut Session, id: Id, answer: GraphFetchResult) -> (Value, Vec<u8>) {
    let plan = s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().expect("it waits for the server's copy");
    assert_eq!(plan.messages, vec![id]);
    s.fetch_waiting(plan);
    let answered = s.fetch_now_answered(&[id], s.openings(), answer);
    let NowNext::Run(waiting) = answered.next else { panic!("it goes on: {:?}", answered.next) };
    // Run again, it waits for nothing more: the answer is held for it.
    assert!(s.fetch_first_plan(&waiting.command, &waiting.params).unwrap().is_none(), "asked once");
    // (A folder of its own: the tests run side by side.)
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mailcraft-as-received-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.eml");
    let mut params = waiting.params.clone();
    params["path"] = json!(path.to_string_lossy());
    let said = s.run(&waiting.command, &params).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (said, bytes)
}

#[test]
fn a_message_whole_here_is_saved_as_its_server_holds_it_byte_for_byte() {
    let (mut s, account, id) = exchange(Some(RECEIVED));
    // Made of its parts, it is another file: what a save wrote before.
    let rebuilt = s.run("file.exportMessage", &json!({"id": id})).unwrap();
    assert_eq!((rebuilt["asReceived"].as_bool(), rebuilt["why"].as_str()), (Some(false), Some("notAsked")));
    let data = mailcraft_charset::base64::STANDARD.decode(rebuilt["data"].as_str().unwrap()).unwrap();
    assert_ne!(data, RECEIVED);
    // Whole here, its server is asked for it all the same, and only for the save.
    assert_eq!(ews_mail::ews_fetch_request(&s, account, Some(id), 1).unwrap().ids, Vec::<String>::new(), "not before it is asked");
    let plan = s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().unwrap();
    s.fetch_waiting(plan);
    assert_eq!(ews_mail::ews_fetch_request(&s, account, Some(id), 1).unwrap().ids, ["A1"]);
    assert!(ews_mail::ews_fetch_request(&s, account, None, 10).unwrap().ids.is_empty(), "the background doesn't fetch it again");
    s.driver.fetch.forget();
    let text = s.mailbox.message(id).unwrap().body.text.clone();
    let (said, bytes) = save_as_the_window_does(&mut s, id, fetched(account, RECEIVED));
    assert_eq!(bytes, RECEIVED);
    assert_eq!(
        (said["asReceived"].as_bool(), said["from"].as_str(), said["bytes"].as_u64()),
        (Some(true), Some("server"), Some(RECEIVED.len() as u64))
    );
    // Nothing of it is kept with the message, nor held once written.
    let m = s.mailbox.message(id).unwrap();
    assert!(m.original.is_none() && m.body.text == text);
    assert!(s.driver.fetch.for_save.is_none());
}

#[test]
fn a_message_not_fetched_yet_is_saved_as_it_came_and_taken_in_too() {
    let (mut s, account, id) = exchange(None);
    assert!(s.mailbox.message(id).unwrap().unfetched);
    let (said, bytes) = save_as_the_window_does(&mut s, id, fetched(account, RECEIVED));
    assert_eq!((bytes.as_slice(), said["asReceived"].as_bool()), (RECEIVED, Some(true)));
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.unfetched && m.body.text.contains("The minutes are attached.") && m.attachments.len() == 1);
}

#[test]
fn a_copy_the_server_didnt_send_is_said_and_the_message_written_from_its_parts() {
    let (mut s, account, id) = exchange(Some(RECEIVED));
    let refused = GraphFetchResult { account, email: ME.into(), failed: vec!["A1".into()], ..Default::default() };
    let (said, bytes) = save_as_the_window_does(&mut s, id, refused);
    assert_eq!((said["asReceived"].as_bool(), said["why"].as_str()), (Some(false), Some("notSent")));
    assert!(bytes.windows(9).any(|w| w == b"Subject: "), "the message, made of its parts");
    // The next Save As asks again.
    assert!(s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().is_some());
    // An answer for another account brings nothing for the save.
    let (said, _) = save_as_the_window_does(&mut s, id, GraphFetchResult { account: account + 100, ..fetched(account, RECEIVED) });
    assert_eq!(said["why"].as_str(), Some("notSent"));
}

#[test]
fn an_answer_held_for_a_save_is_nobodys_in_another_mailbox_and_goes_with_a_cancelled_dialog() {
    let (mut s, account, id) = exchange(Some(RECEIVED));
    let plan = s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().unwrap();
    s.fetch_waiting(plan);
    s.fetch_now_answered(&[id], s.openings(), fetched(account, RECEIVED));
    assert!(matches!(s.driver.fetch.for_save.as_ref().map(|f| &f.copy), Some(ServerCopy::Came(b)) if b == RECEIVED));
    // (Never the bytes in debug output.)
    assert!(!format!("{:?}", s.driver.fetch).contains("Minutes"));
    s.forget_for_save();
    assert_eq!(s.run("file.exportMessage", &json!({"id": id})).unwrap()["asReceived"], false);
    // Asked under another opening of the mailbox, it is never this one's.
    let plan = s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().unwrap();
    s.fetch_waiting(plan);
    if let Some(f) = s.driver.fetch.for_save.as_mut() {
        f.opening += 1;
    }
    assert!(!s.asked_for_save(id));
    assert_eq!(s.run("file.exportMessage", &json!({"id": id})).unwrap()["asReceived"], false);
}

#[test]
fn a_signed_message_saved_as_it_came_still_verifies() {
    // Kept with the bytes it came as (#30): those are what is saved, header and all.
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime_model::parse_fetched(DETACHED);
    m.folder = inbox;
    let kept = mb.add_message(m).unwrap();
    // Read before they were kept, of a Microsoft account: its server's copy is saved.
    let mut m = mailcraft_mime_model::parse(DETACHED);
    m.folder = inbox;
    m.server = Some(ServerMessage { id: "AAkA1".into(), folder: "F".into(), ..Default::default() });
    let before = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    let account = s.mailbox.accounts[0].id;
    s.mailbox.accounts[0].kind = AccountKind::Microsoft;
    assert_eq!(graph_mail::graph_fetch_request(&s, account, Some(before), 1).unwrap().ids, Vec::<String>::new());
    let said = s.run("file.exportMessage", &json!({"id": kept})).unwrap();
    assert_eq!((said["asReceived"].as_bool(), said["from"].as_str()), (Some(true), Some("kept")));
    let saved_kept = mailcraft_charset::base64::STANDARD.decode(said["data"].as_str().unwrap()).unwrap();
    assert_eq!(saved_kept, DETACHED);
    let answer = GraphFetchResult { account, fetched: vec![GraphFetched { id: "AAkA1".into(), mime: DETACHED.to_vec() }], ..Default::default() };
    let plan = s.fetch_first_plan("file.exportMessage", &json!({"id": before})).unwrap().unwrap();
    s.fetch_waiting(plan);
    assert_eq!(graph_mail::graph_fetch_request(&s, account, Some(before), 1).unwrap().ids, ["AAkA1"]);
    s.fetch_now_answered(&[before], s.openings(), answer);
    let said = s.run("file.exportMessage", &json!({"id": before})).unwrap();
    let saved_copy = mailcraft_charset::base64::STANDARD.decode(said["data"].as_str().unwrap()).unwrap();
    assert_eq!(saved_copy, DETACHED);
    // Each read again as a message file is, its signature checks out.
    for bytes in [saved_kept, saved_copy] {
        let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
        let mut m = mailcraft_mime_model::parse_fetched(&bytes);
        m.folder = mb.inbox().unwrap();
        let opened = mb.add_message(m).unwrap();
        let mut s = Session::new(mb);
        s.clock.now = Some(NOW);
        s.set_trust_anchors([CA]);
        assert_eq!(s.digital_signature(opened).map(|d| d.verdict), Some(Verdict::Verified));
    }
}

#[test]
fn only_part_of_a_message_is_never_saved() {
    let (mut s, _, id) = exchange(None);
    // Asked by a caller that doesn't fetch first (an agent, the command line): refused, not a file
    // of the headers and preview.
    let dir = std::env::temp_dir().join(format!("mailcraft-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.eml");
    let save = |s: &mut Session| s.run("file.exportMessage", &json!({"id": id, "path": path.to_string_lossy()}));
    assert!(save(&mut s).unwrap_err().to_string().contains("isn't downloaded yet"));
    s.mailbox.message_mut(id).unwrap().incomplete = true;
    assert!(save(&mut s).unwrap_err().to_string().contains("only the headers"));
    // An IMAP message too large to download: only its header is here.
    let m = s.mailbox.message_mut(id).unwrap();
    (m.unfetched, m.incomplete, m.server, m.uid, m.not_downloaded) = (false, false, None, 7, 90 << 20);
    s.mailbox.accounts.iter_mut().for_each(|a| a.kind = AccountKind::Imap);
    assert!(save(&mut s).unwrap_err().to_string().contains("too large to download"));
    assert!(!path.exists(), "no file was written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_cant_be_had_as_it_came_says_why() {
    let (mut s, _, id) = exchange(Some(RECEIVED));
    let why = |s: &mut Session| s.run("file.exportMessage", &json!({"id": id})).unwrap()["why"].as_str().map(str::to_string);
    // An IMAP account's server isn't asked (yet).
    let m = s.mailbox.message_mut(id).unwrap();
    (m.server, m.uid) = (None, 7);
    s.mailbox.accounts.iter_mut().for_each(|a| a.kind = AccountKind::Imap);
    assert!(s.fetch_first_plan("file.exportMessage", &json!({"id": id})).unwrap().is_none());
    assert_eq!(why(&mut s).as_deref(), Some("notAsked"));
    // Mail of this computer alone.
    s.mailbox.message_mut(id).unwrap().uid = 0;
    s.mailbox.accounts.iter_mut().for_each(|a| a.kind = AccountKind::Local);
    assert_eq!(why(&mut s).as_deref(), Some("noServer"));
    // A draft, and a message a part was removed from here.
    s.mailbox.message_mut(id).unwrap().draft = true;
    assert_eq!(why(&mut s).as_deref(), Some("draft"));
    let m = s.mailbox.message_mut(id).unwrap();
    (m.draft, m.parts_removed) = (false, true);
    assert_eq!(why(&mut s).as_deref(), Some("partsRemoved"));
}
