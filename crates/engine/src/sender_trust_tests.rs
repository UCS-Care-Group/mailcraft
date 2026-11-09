// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a sender (or a file's giver, or MailCraft itself) writes of a message is never its
//! server's word (R7). One set of messages, each one whose verdict was written by somebody other
//! than the account's server, run through every use of the server's word: `vouched` (which the
//! reading pane's pictures, its offer to remember a sender and sender images all go by, and of
//! which Exchange's `internal` is part), `failed_check`, and Safe Senders. A meeting's organizer
//! checks are `cmd/calendar/invitation_tests.rs`; the link warnings, `sender_warnings_tests.rs`.
//!
//! The people, hosts and addresses are invented.

use mailcraft_model::auth_results::{failed_check, vouched};
use mailcraft_model::sync::{FolderDelta, FolderListing, GraphFolder, GraphMailResult, MessageOutcome, PlaceDone, Placed, RemoteFlags, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, JunkList, Message};
use mailcraft_pst::test_writer::{self as w, FileSpec, FolderSpec, Layout, MessageSpec};
use mailcraft_pst::{Encryption, Format};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};
use crate::cmd::graph_mail::{apply_graph_mail, graph_mail_request};

const BANK: &str = "alerts@bank.example";
const OWN_PASS: &str = "mx.lumen.example; spf=pass smtp.mailfrom=bank.example; dmarc=pass header.from=bank.example";
const OWN_FAIL: &str = "mx.lumen.example; spf=fail smtp.mailfrom=attacker.example; dmarc=fail header.from=bank.example";
const FOREIGN_PASS: &str = "mx.bank.example; dmarc=pass header.from=bank.example";
const ARC_PASS: &str = "i=1; mx.lumen.example; dmarc=pass header.from=bank.example";
/// What Exchange Online writes above the hop where it takes mail in.
const EXCHANGE_PASS: &str = "mx.microsoft.com 1; spf=pass smtp.mailfrom=bank.example; dmarc=pass action=none header.from=bank.example";

struct World {
    s: Session,
    imap: Id,
    microsoft: Id,
    kept: Id,
}

/// A local account, Avery's IMAP account at Lumen (its Inbox and Junk E-mail on the server,
/// `delivered` in the Inbox and `junk` in Junk E-mail, new since that folder was read whole) with
/// the bank's address among its Safe Senders, and Ada's Microsoft account at Lumen.
fn world(delivered: Vec<Message>, junk: Vec<Message>) -> World {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "local@example.org"})).unwrap()["id"].as_u64().unwrap();
    let servers = json!({"email": "avery@lumen.example", "incoming": {"host": "imap.lumen.example"}, "outgoing": {"host": "smtp.lumen.example"}});
    let imap = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let folder = |remote: &str, kind, uid_validity, new: Vec<Message>| FolderDelta {
        remote: remote.into(),
        kind,
        uid_validity,
        uid_seen: new.iter().map(|m| m.uid).max().unwrap_or(0),
        new,
        ..Default::default()
    };
    let first = SyncResult {
        account: imap,
        folders: vec![folder("INBOX", FolderKind::Inbox, 9, delivered), folder("Junk", FolderKind::Junk, 10, Vec::new())],
        ..Default::default()
    };
    apply_sync(&mut s, first).unwrap();
    s.mailbox.junk_lists_mut(imap).add(JunkList::Safe, BANK);
    if !junk.is_empty() {
        let again = SyncResult { account: imap, folders: vec![folder("Junk", FolderKind::Junk, 10, junk)], ..Default::default() };
        apply_sync(&mut s, again).unwrap();
    }
    let microsoft = s.run("account.add", &json!({"email": "ada@lumen.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let listing = GraphMailResult {
        account: microsoft,
        email: "ada@lumen.example".into(),
        listing: Some(FolderListing {
            folders: vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
            complete: true,
        }),
        ..Default::default()
    };
    apply_graph_mail(&mut s, listing).unwrap();
    let kept = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    World { s, imap, microsoft, kept }
}

/// A message from the bank's address as its server hands it over, with `headers`.
fn delivered(uid: u32, subject: &str, headers: &[(&str, &str)]) -> Message {
    Message {
        uid,
        subject: subject.into(),
        message_id: format!("{uid}-{}@bank.example", subject.replace(' ', "-")),
        from: Address::new("Bank", BANK),
        headers: headers.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect(),
        ..Default::default()
    }
}

fn trusted(s: &Session, id: Id) -> bool {
    let m = s.mailbox.message(id).unwrap();
    vouched(s.mailbox.account(m.account), m)
}

fn titled(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

/// The forged cases, and the control: what the account's own server passed.
fn forged(first_uid: u32) -> Vec<Message> {
    vec![
        delivered(first_uid, "control", &[("Authentication-Results", OWN_PASS)]),
        delivered(first_uid + 1, "forged from", &[("Authentication-Results", OWN_FAIL)]),
        delivered(first_uid + 2, "forged results", &[("Authentication-Results", OWN_FAIL), ("Authentication-Results", OWN_PASS)]),
        delivered(first_uid + 3, "results of another server", &[("Authentication-Results", FOREIGN_PASS)]),
        delivered(
            first_uid + 4,
            "forged arc",
            &[("ARC-Authentication-Results", ARC_PASS), ("ARC-Seal", "i=1; a=rsa-sha256; d=bank.example; s=s; b=AA")],
        ),
    ]
}

#[test]
fn a_verdict_the_sender_wrote_vouches_for_nothing_and_hides_no_failure() {
    let w = world(forged(1), Vec::new());
    let s = &w.s;
    assert!(trusted(s, titled(s, "control")));
    for subject in ["forged from", "forged results", "results of another server", "forged arc"] {
        assert!(!trusted(s, titled(s, subject)), "{subject}");
    }
    let failed = |subject: &str| {
        let m = s.mailbox.message(titled(s, subject)).unwrap();
        failed_check(s.mailbox.account(m.account), m).is_some()
    };
    assert!(failed("forged from") && failed("forged results"));
    assert!(!failed("control") && !failed("results of another server") && !failed("forged arc"));
}

#[test]
fn a_safe_senders_address_brings_out_of_junk_only_what_its_server_found_no_fault_with() {
    let mut w = world(Vec::new(), forged(11));
    // As each arrives in Junk E-mail of a folder read whole before.
    let arrived: Vec<Id> = w.s.mailbox.messages.values().filter(|m| m.uid > 10).map(|m| m.id).collect();
    for id in arrived {
        w.s.mailbox.sort_junk(id);
    }
    let s = &w.s;
    let kind = |subject: &str| s.mailbox.folder(s.mailbox.message(titled(s, subject)).unwrap().folder).unwrap().kind;
    // The server said the From failed: no list of the owner's overrules that.
    assert_eq!(kind("forged from"), FolderKind::Junk);
    assert_eq!(kind("forged results"), FolderKind::Junk);
    // Where it found no fault, the owner's list decides, as it always did.
    for subject in ["control", "results of another server", "forged arc"] {
        assert_eq!(kind(subject), FolderKind::Inbox, "{subject}");
    }
}

/// The bytes of a message whose verdicts its author wrote: one that looks like Lumen's own
/// server's, one like Exchange Online's above an edge hop, and an ARC one.
fn eml(subject: &str) -> String {
    format!(
        "Authentication-Results: {EXCHANGE_PASS}\r\nReceived: from mail.bank.example (192.0.2.1) by A1.mail.protection.outlook.com (10.0.0.1)\r\n\
         Authentication-Results: {OWN_PASS}\r\nARC-Authentication-Results: {ARC_PASS}\r\n\
         X-MS-Exchange-Organization-MessageDirectionality: Incoming\r\nFrom: Bank <{BANK}>\r\nTo: avery@lumen.example\r\n\
         Subject: {subject}\r\nMessage-ID: <{}@bank.example>\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\n\
         Content-Type: text/plain\r\n\r\nYour statement.\r\n",
        subject.replace(' ', "-")
    )
}

fn is_verdict(name: &str) -> bool {
    name.eq_ignore_ascii_case("Authentication-Results") || name.eq_ignore_ascii_case("ARC-Authentication-Results")
}

/// Put `id` into the IMAP account's Inbox by copying it there, and have the server take it as `uid`.
fn copied_into_imap(w: &mut World, id: Id, uid: u32) -> Id {
    let inbox = w.s.mailbox.folder_of_kind(w.imap, FolderKind::Inbox).unwrap();
    let copy = w.s.run("mail.copy", &json!({"ids": [id], "folder": inbox})).unwrap()["copies"][0].as_u64().unwrap();
    let request = sync_request(&w.s, w.imap).unwrap();
    let place = request.places.iter().find(|p| p.message.id == copy).unwrap();
    // None of what it said of itself goes up with it.
    assert!(!place.message.headers.iter().any(|(name, _)| is_verdict(name)), "{:?}", place.message.headers);
    let flags = RemoteFlags { draft: false, ..RemoteFlags::of(&place.message, 0) };
    let placed = vec![Placed { message: copy, to_folder: place.to_folder, uid, uid_validity: 9, flags, original: false }];
    apply_sync(&mut w.s, SyncResult { account: w.imap, placed, ..Default::default() }).unwrap();
    assert_eq!(w.s.mailbox.message(copy).map(|m| m.uid), Some(uid));
    copy
}

/// Move `id` into the Microsoft account's Inbox, and have its server make it.
fn moved_into_microsoft(w: &mut World, id: Id, made: &str) {
    let inbox = w.s.mailbox.folder_of_kind(w.microsoft, FolderKind::Inbox).unwrap();
    w.s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
    let asked = graph_mail_request(&w.s, w.microsoft).unwrap();
    let outcome = MessageOutcome::Done { categories: None, flag: None, folder: "in".into(), id: made.into() };
    let placed = asked.places.iter().map(|p| PlaceDone { change: p.without_data(), outcome: outcome.clone() }).collect();
    apply_graph_mail(&mut w.s, GraphMailResult { account: w.microsoft, email: "ada@lumen.example".into(), placed, ..Default::default() }).unwrap();
    let m = w.s.mailbox.message(id).unwrap();
    assert!(m.server.as_ref().is_some_and(|server| server.id == made), "{:?}", m.server);
    assert!(!m.headers.iter().any(|(name, _)| is_verdict(name)), "{:?}", m.headers);
}

#[test]
fn mail_from_a_file_put_on_a_server_by_mailcraft_is_not_vouched_for_by_what_it_says_of_itself() {
    let mut w = world(Vec::new(), Vec::new());
    let dir = std::env::temp_dir().join(format!("mailcraft-r7-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // From an .eml file.
    let path = dir.join("statement.eml");
    std::fs::write(&path, eml("statement")).unwrap();
    let imported = w.s.run("file.import", &json!({"path": path, "folder": w.kept})).unwrap()["ids"][0].as_u64().unwrap();
    assert!(!trusted(&w.s, imported));
    let copy = copied_into_imap(&mut w, imported, 77);
    assert!(!trusted(&w.s, copy));
    moved_into_microsoft(&mut w, imported, "AAkP1");
    assert!(!trusted(&w.s, imported));

    // From a .pst file, the same headers in the item's transport headers.
    let mut item = MessageSpec { class: "IPM.Note".into(), subject: "pst statement".into(), body: "Your statement.".into(), ..Default::default() };
    let whole = eml("pst statement");
    let headers = whole.split("\r\n\r\n").next().unwrap();
    item.extra = vec![(0x007D, w::unicode(&format!("{headers}\r\n")))];
    let file = FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![FolderSpec { name: "Inbox".into(), class: Some("IPF.Note".into()), messages: vec![item], children: Vec::new() }],
        named: Vec::new(),
        named_strings: Vec::new(),
    };
    let data = crate::cmd::base64_encode(&w::build(&file));
    w.s.run("file.importPst", &json!({"name": "old-mail.pst", "data": data})).unwrap();
    let from_pst = titled(&w.s, "pst statement");
    assert!(w.s.mailbox.message(from_pst).unwrap().headers.iter().any(|(name, _)| name == "Authentication-Results"));
    assert!(!trusted(&w.s, from_pst));
    let copy = copied_into_imap(&mut w, from_pst, 78);
    assert!(!trusted(&w.s, copy));
    moved_into_microsoft(&mut w, from_pst, "AAkP2");
    assert!(!trusted(&w.s, from_pst));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mail_its_server_delivered_stays_vouched_for() {
    let w = world(vec![delivered(1, "delivered", &[("Authentication-Results", OWN_PASS)])], Vec::new());
    assert!(trusted(&w.s, titled(&w.s, "delivered")));
}
