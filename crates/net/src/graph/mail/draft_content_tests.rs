// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a draft carries when it goes up, and what is sent, against the strict Graph stand-in:
//! a forward's attachments are the owner's, whatever the server's copies are named (AU2-09), and
//! a draft saved earlier is sent as it is here, not as the server has it now (AU2-14). Nothing
//! here leaves this machine; every name and message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{
    AttachmentUp, AttachmentsUp, DraftBody, DraftKind, DraftOf, DraftUp, GraphMailRequest, GraphMailResult, SendOutcome, SendUp, SyncedAttachment,
};
use mailcraft_model::{Address, Importance};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{Attached, StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn run(server: &StandIn, drafts: Vec<DraftUp>, sends: Vec<SendUp>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), drafts, sends, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

fn file(name: &str, bytes: &[u8]) -> AttachmentUp {
    AttachmentUp {
        name: name.into(),
        mime: "application/pdf".into(),
        hash: u64::from(bytes.first().copied().unwrap_or(0)),
        size: bytes.len() as u64,
        data: Arc::new(bytes.to_vec()),
        ..Default::default()
    }
}

fn server_file(id: &str, name: &str, bytes: &[u8]) -> Attached {
    Attached { id: id.into(), name: name.into(), content_type: "application/pdf".into(), bytes: bytes.to_vec(), ..Default::default() }
}

/// A message in the Inbox with these attachments; its id.
fn original(server: &StandIn, attachments: Vec<Attached>) -> String {
    let inbox = server.state().well_known("inbox");
    let id = server.state().mailbox.add(&inbox, "Contracts", NOW);
    server.state().mailbox.update(&id, |m| m.attachments = attachments);
    id
}

/// A forward of `source` that carries `here`, made and brought up to date; the bytes of the
/// attachments on the server's draft afterwards, by name.
fn forwarded(server: &StandIn, source: &str, here: Vec<AttachmentUp>) -> (GraphMailResult, Vec<(String, Vec<u8>)>) {
    let up = AttachmentsUp { here, synced: Vec::new() };
    let draft = DraftUp {
        message: 5,
        marker: "mailcraft-draft-5".into(),
        of: Some(DraftOf { id: source.into(), kind: DraftKind::Forward }),
        attachments: Some(up),
        ..Default::default()
    };
    let all = run(server, vec![draft], Vec::new());
    let id = all.drafts.first().map(|d| d.id.clone()).unwrap_or_default();
    let state = server.state();
    let there =
        state.mailbox.messages.get(&id).map(|m| m.attachments.iter().map(|a| (a.name.clone(), a.bytes.clone())).collect()).unwrap_or_default();
    drop(state);
    (all, there)
}

#[test]
fn a_replacement_with_the_name_of_a_forwarded_attachment_is_the_owners_file_not_the_originals() {
    let server = StandIn::start().unwrap();
    let source = original(&server, vec![server_file("a1", "contract.pdf", b"ORIGINAL")]);
    // The owner took the original off and attached a redacted file of the same name and size.
    let (all, there) = forwarded(&server, &source, vec![file("contract.pdf", b"REDACTED")]);
    assert!(all.errors.is_empty(), "{:?}", all.errors);
    assert_eq!(there, [("contract.pdf".to_string(), b"REDACTED".to_vec())]);
    assert!(matches!(all.drafts.first().and_then(|d| d.attachments.as_ref()), Some(kept) if kept.len() == 1));
}

#[test]
fn of_two_forwarded_attachments_with_one_name_the_one_the_owner_kept_survives() {
    let server = StandIn::start().unwrap();
    let source = original(&server, vec![server_file("a1", "scan.pdf", b"FIRST"), server_file("a2", "scan.pdf", b"OTHER")]);
    let (_, there) = forwarded(&server, &source, vec![file("scan.pdf", b"OTHER")]);
    assert_eq!(there, [("scan.pdf".to_string(), b"OTHER".to_vec())]);
    let (_, there) = forwarded(&server, &source, vec![file("scan.pdf", b"FIRST")]);
    assert_eq!(there, [("scan.pdf".to_string(), b"FIRST".to_vec())]);
}

#[test]
fn a_forwarded_attachment_the_owner_kept_is_taken_as_it_is_not_sent_up_again() {
    let server = StandIn::start().unwrap();
    let source = original(&server, vec![server_file("a1", "plan.pdf", b"SAME")]);
    let (all, there) = forwarded(&server, &source, vec![file("plan.pdf", b"SAME")]);
    assert_eq!(there, [("plan.pdf".to_string(), b"SAME".to_vec())]);
    // The server's own copy (the one the forward made) is the one that is kept.
    let kept = all.drafts.first().and_then(|d| d.attachments.clone()).unwrap_or_default();
    assert!(matches!(kept.as_slice(), [SyncedAttachment { id, .. }] if id.ends_with("-att")), "{kept:?}");
}

/// What the owner has of a saved draft, whole: every field and the attachments.
fn whole_draft(id: &str, subject: &str, body: &str, bcc: &[&str], here: Vec<AttachmentUp>, synced: Vec<SyncedAttachment>) -> DraftUp {
    DraftUp {
        message: 5,
        id: id.into(),
        marker: "mailcraft-draft-5".into(),
        subject: Some(subject.into()),
        body: Some(DraftBody { html: false, content: body.into(), hash: 7 }),
        to: Some(vec![Address::new("", "kit@example.net")]),
        cc: Some(Vec::new()),
        bcc: Some(bcc.iter().map(|b| Address::new("", *b)).collect()),
        importance: Some(Importance::Normal),
        attachments: Some(AttachmentsUp { here, synced }),
        ..Default::default()
    }
}

#[test]
fn a_saved_draft_is_sent_as_it_is_here_whatever_the_server_has_changed_since() {
    let server = StandIn::start().unwrap();
    // Saved: made with its fields and one attachment.
    let saved = run(&server, vec![whole_draft("", "Rota", "Hello.", &[], vec![file("rota.pdf", b"ROTA")], Vec::new())], Vec::new());
    let done = saved.drafts.first().cloned().unwrap();
    assert!(!done.id.is_empty() && done.fields, "{:?}", saved.errors);
    let synced = done.attachments.clone().unwrap();
    // Meanwhile somebody with access to the mailbox changed it on the server.
    server.state().mailbox.update(&done.id, |m| {
        m.bcc.push("spy@example.net".into());
        m.body = "Pay the other account.".into();
        m.subject = "Rota (changed)".into();
        m.attachments.push(Attached { id: "x9".into(), name: "extra.pdf".into(), bytes: b"EXTRA".to_vec(), ..Default::default() });
    });
    // Sent from here, with everything asserted.
    let draft = whole_draft(&done.id, "Rota", "Hello.", &[], vec![file("rota.pdf", b"ROTA")], synced);
    let all = run(&server, Vec::new(), vec![SendUp { message: 5, draft: Some(draft), mime: None }]);
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    let sent_items = server.state().well_known("sentitems");
    let state = server.state();
    let copy = state.mailbox.messages.values().find(|m| m.folder == sent_items).unwrap();
    assert_eq!((copy.subject.as_str(), copy.body.as_str()), ("Rota", "Hello."));
    assert!(copy.bcc.is_empty(), "{:?}", copy.bcc);
    assert_eq!(copy.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["rota.pdf"]);
}
