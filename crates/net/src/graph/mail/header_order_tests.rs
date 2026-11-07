// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exchange Online's verdict is believed by where it stands among the headers (AU3-V5): a
//! message fetched whole from the strict Graph stand-in, or downloaded from the fake IMAP server,
//! keeps its headers in the order the server has them. Nothing here leaves this machine; the message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::auth_results::vouched;
use mailcraft_model::sync::{GraphFetchRequest, ServerMessage};
use mailcraft_model::{Account, AccountKind};

use mailcraft_model::sync::{FolderState, SyncRequest};
use mailcraft_model::{FolderKind, Security, Server};

use crate::fake::{Fake, FakeMailbox, FakeMessage, ImapWorld};
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };

/// A message from outside as Exchange Online delivers it.
const EXTERNAL: &str = "Received: from AM9PR01MB1234.eurprd01.prod.outlook.com (2603:10a6::1) by DB9PR01MB5678.eurprd01.prod.outlook.com with HTTPS\r\n\
Authentication-Results: mx.microsoft.com 1; spf=pass (sender ip is 192.0.2.10) smtp.mailfrom=example.org; dmarc=pass action=none header.from=example.org;compauth=pass reason=100\r\n\
Received-SPF: Pass (protection.outlook.com: domain of example.org designates 192.0.2.10 as permitted sender)\r\n\
Received: from mail.example.org (192.0.2.10) by AM3PEPF0000A001.mail.protection.outlook.com (10.167.16.1) with Microsoft SMTP Server\r\n\
Authentication-Results: mx.microsoft.com 1; dmarc=fail header.from=example.org\r\n\
From: Bank <alerts@example.org>\r\nTo: owner@example.org\r\nSubject: Statement\r\nMessage-ID: <order-1@example.org>\r\n\
Date: Mon, 2 Nov 2026 10:00:00 +0000\r\nX-MS-Exchange-Organization-MessageDirectionality: Incoming\r\n\
X-MS-Exchange-Organization-AuthAs: Anonymous\r\nContent-Type: text/plain\r\n\r\nYour statement.\r\n";

/// The names of the headers that say where a message went and what was found of it, in order.
fn path(headers: &[(String, String)]) -> Vec<String> {
    let kept = ["Received", "Authentication-Results", "Received-SPF"];
    headers.iter().filter(|(name, _)| kept.iter().any(|k| name.eq_ignore_ascii_case(k))).map(|(name, _)| name.clone()).collect()
}

const PATH: [&str; 5] = ["Received", "Authentication-Results", "Received-SPF", "Received", "Authentication-Results"];

#[test]
fn a_message_fetched_whole_keeps_its_headers_in_the_servers_order() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let id = server.state().mailbox.add(&inbox, "Statement", 1_790_000_000);
    server.state().mailbox.update(&id, |m| m.mime = Some(EXTERNAL.into()));
    let request = GraphFetchRequest { account: 3, email: "owner@example.org".into(), ids: vec![id.clone()], ..Default::default() };
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    let fetched = got.fetched.first().unwrap();
    // What the engine does with what comes.
    let mut m = mailcraft_mime::parse(&fetched.mime);
    m.server = Some(ServerMessage { id, folder: inbox, ..Default::default() });
    assert_eq!(path(&m.headers), PATH);
    let account = Account { kind: AccountKind::Microsoft, email: "owner@example.org".into(), ..Default::default() };
    assert!(vouched(Some(&account), &m));
}

#[test]
fn a_message_downloaded_over_imap_keeps_its_headers_in_the_servers_order() {
    let message =
        FakeMessage { uid: 1, flags: String::new(), internal_date: " 2-Nov-2026 10:00:05 +0000".into(), body: EXTERNAL.into(), claimed_size: None };
    let inbox = FakeMailbox {
        raw: "INBOX".into(),
        flags: "\\HasNoChildren".into(),
        uid_validity: 5,
        messages: vec![message],
        refuse_open: false,
        next_uid: 0,
    };
    let imap = Fake::imap(ImapWorld { user: "owner".into(), password: "hunter2".into(), mailboxes: vec![inbox], ..Default::default() });
    let account = Account {
        id: 7,
        email: "owner@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "owner".into(),
        ..Default::default()
    };
    let folders = FolderKind::STANDARD.iter().zip(100..).map(|(kind, id)| FolderState { id, kind: *kind, ..Default::default() }).collect();
    let result = crate::sync(&SyncRequest { account, password: "hunter2".into(), folders, ..Default::default() });
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let m = result.folders.iter().find(|f| f.remote == "INBOX").and_then(|f| f.new.first()).unwrap();
    assert_eq!(m.uid, 1);
    assert_eq!(path(&m.headers), PATH);
    // Read as Exchange Online's (the fake server stands at no host of Microsoft's).
    let microsoft = Account { kind: AccountKind::Microsoft, email: "owner@example.org".into(), ..Default::default() };
    assert!(vouched(Some(&microsoft), m));
}
