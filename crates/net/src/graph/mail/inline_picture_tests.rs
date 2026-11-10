// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture written into a message (#547) goes up as an inline attachment (`isInline`, with its
//! `contentId`) beside an HTML body that refers to it by `cid:`, against the strict Graph
//! stand-in; sent, the copy in Sent Items has it. Nothing here leaves this machine.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{
    AttachmentUp, AttachmentsUp, DraftBody, DraftUp, GraphMailRequest, GraphMailResult, SendOutcome, SendUp, SyncedAttachment,
};
use mailcraft_model::{Address, Importance};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const CID: &str = "pic-1048576.m1@mailcraft.invalid";

fn run(server: &StandIn, drafts: Vec<DraftUp>, sends: Vec<SendUp>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), drafts, sends, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

fn picture() -> AttachmentUp {
    let bytes = b"\x89PNG\r\n\x1a\nIHDR-picture".to_vec();
    AttachmentUp {
        name: "chart.png".into(),
        mime: "image/png".into(),
        content_id: CID.into(),
        inline: true,
        hash: 11,
        size: bytes.len() as u64,
        data: Arc::new(bytes),
    }
}

fn draft(id: &str, here: Vec<AttachmentUp>, synced: Vec<SyncedAttachment>) -> DraftUp {
    let html = format!("<html><body><div>Look: <img src=\"cid:{CID}\" alt=\"Sales\"></div></body></html>");
    DraftUp {
        message: 5,
        id: id.into(),
        marker: "mailcraft-draft-5".into(),
        subject: Some("Chart".into()),
        body: Some(DraftBody { html: true, content: html, hash: 9 }),
        to: Some(vec![Address::new("", "kit@example.net")]),
        cc: Some(Vec::new()),
        bcc: Some(Vec::new()),
        importance: Some(Importance::Normal),
        attachments: Some(AttachmentsUp { here, synced }),
        ..Default::default()
    }
}

#[test]
fn a_picture_in_the_text_is_an_inline_attachment_of_the_server_draft() {
    let server = StandIn::start().unwrap();
    let saved = run(&server, vec![draft("", vec![picture()], Vec::new())], Vec::new());
    let done = saved.drafts.first().cloned().expect("the draft was made");
    assert!(!done.id.is_empty() && done.fields && saved.errors.is_empty(), "{:?}", saved.errors);
    let state = server.state();
    let there = state.mailbox.messages.get(&done.id).expect("on the server");
    assert!(there.html && there.body.contains(&format!("cid:{CID}")), "{}", there.body);
    assert!(
        matches!(there.attachments.as_slice(), [a] if a.inline && a.content_id == CID && a.content_type == "image/png" && a.name == "chart.png" && a.bytes.starts_with(b"\x89PNG")),
        "{:?}",
        there.attachments
    );
}

#[test]
fn sent_from_the_draft_the_copy_keeps_the_picture_inline() {
    let server = StandIn::start().unwrap();
    let saved = run(&server, vec![draft("", vec![picture()], Vec::new())], Vec::new());
    let done = saved.drafts.first().cloned().unwrap();
    let synced = done.attachments.clone().unwrap_or_default();
    let all = run(&server, Vec::new(), vec![SendUp { message: 5, draft: Some(draft(&done.id, vec![picture()], synced)), mime: None }]);
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    let sent_items = server.state().well_known("sentitems");
    let state = server.state();
    let copy = state.mailbox.messages.values().find(|m| m.folder == sent_items).expect("in Sent Items");
    assert!(copy.body.contains(&format!("cid:{CID}")));
    assert!(matches!(copy.attachments.as_slice(), [a] if a.inline && a.content_id == CID), "{:?}", copy.attachments);
}
