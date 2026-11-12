// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A draft kept Encrypted going up through Microsoft Graph (#30 part 7), against the strict
//! stand-in: it is made from its MIME (`POST /me/messages`, `text/plain`, base64) and nothing of
//! its fields goes, and the server's earlier copy, its fields in the clear, is taken away once the
//! new one is there. Nothing here leaves this machine; every name and message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{DraftBody, DraftUp, GraphMailRequest, GraphMailResult};
use mailcraft_model::{Address, Message};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn run(server: &StandIn, drafts: Vec<DraftUp>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), drafts, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

/// What the desktop program writes for a draft kept Encrypted: the message's header and an
/// Encrypted body (a stand-in's: the server reads no encryption).
fn sealed() -> Vec<u8> {
    let m = Message {
        from: Address::new("Owner", "owner@example.org"),
        to: vec![Address::new("Kit", "kit@example.net")],
        subject: "Plans".into(),
        date: NOW,
        message_id: "plans.1@example.org".into(),
        original: mailcraft_model::Original::new(mailcraft_mime::smime_enveloped(&[0x30, 0x03, 0x02, 0x01, 0x01]).unwrap()),
        ..Message::default()
    };
    mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())
}

fn up(id: &str) -> DraftUp {
    up_with_key(id, "")
}

fn up_with_key(id: &str, key: &str) -> DraftUp {
    DraftUp {
        message: 5,
        id: id.into(),
        change_key: key.into(),
        marker: "mailcraft-draft-5-1".into(),
        saves: 1,
        sealed: Arc::new(sealed()),
        ..Default::default()
    }
}

#[test]
fn an_encrypted_draft_is_made_from_its_mime_and_nothing_of_its_fields_goes() {
    let server = StandIn::start().unwrap();
    let all = run(&server, vec![up("")]);
    let done = all.drafts.first().cloned().unwrap_or_else(|| panic!("an answer: {:?}", all.errors));
    assert!(!done.id.is_empty() && done.fields && done.refused.is_none(), "{done:?}");
    // The one request that made it was the MIME create, and no JSON of fields went anywhere.
    let state = server.state();
    let made = state.mailbox.messages.get(&done.id).unwrap_or_else(|| panic!("the server has it"));
    // What the server holds is the message as written: its subject, its recipients, and the
    // Encrypted body (base64 of the enveloped data, whose first bytes are 0x30 0x03...).
    assert_eq!(made.subject, "Plans");
    assert!(made.to.iter().any(|a| a.contains("kit@example.net")), "{:?}", made.to);
    assert!(made.body.starts_with("MAMCAQE"), "{}", made.body);
    // The marker went on after (a MIME create can't carry it).
    assert!(made.extended.iter().any(|(_, value)| value == "mailcraft-draft-5-1"), "{:?}", made.extended);
    let drafts = state.well_known("drafts");
    assert_eq!(made.folder, drafts);
}

#[test]
fn a_draft_saved_again_is_made_again_and_the_earlier_copy_is_taken_away() {
    let server = StandIn::start().unwrap();
    // The server has the draft in the clear, as an earlier save (before Encrypt was turned on) left it.
    let first = run(
        &server,
        vec![DraftUp {
            message: 5,
            marker: "mailcraft-draft-5-1".into(),
            saves: 1,
            subject: Some("Plans".into()),
            body: Some(DraftBody { html: false, content: "The vault code is 4417.".into(), hash: 7 }),
            to: Some(vec![Address::new("Kit", "kit@example.net")]),
            ..Default::default()
        }],
    );
    let (clear, key) = (first.drafts[0].id.clone(), first.drafts[0].change_key.clone());
    assert!(server.state().mailbox.messages.contains_key(&clear));
    // Encrypt is on, and the draft is saved: the Encrypted one is made, the clear one goes.
    let all = run(&server, vec![DraftUp { purge: true, ..up_with_key(&clear, &key) }]);
    let done = all.drafts.first().cloned().unwrap_or_else(|| panic!("an answer: {:?}", all.errors));
    assert!(!done.id.is_empty() && done.id != clear && done.fields, "{done:?}");
    let state = server.state();
    assert!(!state.mailbox.messages.contains_key(&clear), "the copy with its fields in the clear is gone");
    assert_eq!(state.mailbox.purged.as_slice(), std::slice::from_ref(&clear), "taken away with permanentDelete, not left in the recoverable items");
    assert!(state.mailbox.messages.contains_key(&done.id));
    assert!(!state.mailbox.messages.values().any(|m| m.preview.contains("4417")), "nothing of the clear text is left on the server");
}

#[test]
fn a_mailbox_that_refuses_a_permanent_delete_gets_the_plain_delete() {
    let server = StandIn::start().unwrap();
    let first = run(
        &server,
        vec![DraftUp { message: 5, marker: "mailcraft-draft-5-1".into(), saves: 1, subject: Some("Plans".into()), ..Default::default() }],
    );
    let (clear, key) = (first.drafts[0].id.clone(), first.drafts[0].change_key.clone());
    server.state().mailbox.no_permanent_delete = true;
    let all = run(&server, vec![DraftUp { purge: true, ..up_with_key(&clear, &key) }]);
    let done = all.drafts.first().cloned().unwrap_or_else(|| panic!("an answer: {:?}", all.errors));
    assert!(!done.id.is_empty() && done.id != clear && done.fields, "{done:?}");
    let state = server.state();
    assert!(!state.mailbox.messages.contains_key(&clear), "gone all the same, by the plain delete");
    assert!(state.mailbox.purged.is_empty());
}
