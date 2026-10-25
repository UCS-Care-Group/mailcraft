// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a Microsoft account sends: only what the owner queued here (AU2-V2), and a saved draft
//! as the owner has it, every field and attachment (AU2-14). Invented people and servers.

use mailcraft_model::sync::{DraftState, FolderListing, GraphFolder, ServerDraft, ServerMessage};
use mailcraft_model::{Address, Attachment, FolderKind, Message, ServerFolder};
use serde_json::json;

use super::*;
use crate::cmd::graph_mail::{Taken, take_listing};

/// A session with a Microsoft account, its Outbox bound to the server's, as 0.8.0 left it; the
/// account and the Outbox.
fn setup() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    s.mailbox.folder_mut(outbox).unwrap().server = Some(ServerFolder { id: "F-ou".into(), name: "Outbox".into(), ..Default::default() });
    (s, account, outbox)
}

fn queued(s: &mut Session, account: Id, outbox: Id, subject: &str, server: Option<ServerMessage>) -> Id {
    let m = Message {
        account,
        folder: outbox,
        subject: subject.into(),
        to: vec![Address::new("", "kit@example.net")],
        message_id: format!("{}@example.org", subject.to_lowercase()),
        server,
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn subjects(s: &Session, account: Id) -> Vec<String> {
    sends(s, account).iter().filter_map(|up| s.mailbox.message(up.message).map(|m| m.subject.clone())).collect()
}

/// What 0.8.0 downloaded from the server's Outbox into the queue it sends from.
fn delivered() -> Option<ServerMessage> {
    Some(ServerMessage { id: "M-9".into(), folder: "F-ou".into(), ..Default::default() })
}

#[test]
fn what_the_server_delivered_into_the_outbox_is_never_sent() {
    let (mut s, account, outbox) = setup();
    queued(&mut s, account, outbox, "Mine", None);
    let theirs = queued(&mut s, account, outbox, "Theirs", delivered());
    // A mailbox as 0.8.0 left it: both are in the Outbox, and only the owner's goes.
    assert_eq!(subjects(&s, account), ["Mine"]);
    assert_eq!(waiting(&s.mailbox, account), 1);

    // The listing now has the server's Outbox as a folder like another: the bond is let go, what
    // the server delivered is forgotten here (it comes down into that folder), what the owner
    // queued stays and is sent.
    let listing = FolderListing {
        folders: vec![GraphFolder { id: "F-ou".into(), name: "Outbox".into(), kind: FolderKind::Custom, ..Default::default() }],
        complete: true,
    };
    take_listing(&mut s.mailbox, account, &listing, &mut Taken::default());
    assert!(s.mailbox.message(theirs).is_none());
    assert_eq!(s.mailbox.folder(outbox).unwrap().server, None);
    assert_eq!(subjects(&s, account), ["Mine"]);
    let bound: Vec<_> = s.mailbox.folders.iter().filter(|f| f.account == account && f.server.as_ref().is_some_and(|x| x.id == "F-ou")).collect();
    assert!(matches!(bound.as_slice(), [f] if f.kind == FolderKind::Custom && f.id != outbox));
}

#[test]
fn a_draft_saved_here_and_queued_is_still_sent() {
    let (mut s, account, outbox) = setup();
    let id = queued(&mut s, account, outbox, "Saved", delivered());
    // Saved as a draft by MailCraft (its marker), then queued to send.
    s.mailbox.message_mut(id).unwrap().server_draft = Some(ServerDraft { marker: "mailcraft-draft-7".into(), ..Default::default() });
    assert_eq!(subjects(&s, account), ["Saved"]);
}

#[test]
fn a_saved_draft_goes_up_whole_before_it_is_sent() {
    let (mut s, account, outbox) = setup();
    let id = queued(&mut s, account, outbox, "Rota", delivered());
    {
        let m = s.mailbox.message_mut(id).unwrap();
        m.body.text = "Hello.".into();
        m.attachments =
            vec![Attachment { name: "rota.pdf".into(), mime: "application/pdf".into(), data: vec![1, 2, 3].into(), ..Default::default() }];
    }
    // What the server last had is what is here: nothing differs, so nothing would be named.
    let state = DraftState::of(s.mailbox.message(id).unwrap());
    s.mailbox.message_mut(id).unwrap().server_draft =
        Some(ServerDraft { marker: "mailcraft-draft-7".into(), synced: Some(state), ..Default::default() });
    let draft = mailcraft_model_draft(&s, id);
    assert!(draft.has_fields());
    assert_eq!((draft.subject.as_deref(), draft.bcc.as_ref().map(Vec::len)), (Some("Rota"), Some(0)));
    assert!(draft.body.is_some() && draft.to.is_some() && draft.cc.is_some() && draft.importance.is_some());
    assert!(matches!(&draft.attachments, Some(up) if up.here.len() == 1));
    assert!(draft.change_key.is_empty());
}

fn mailcraft_model_draft(s: &Session, id: Id) -> mailcraft_model::sync::DraftUp {
    let ups = sends(s, s.mailbox.message(id).unwrap().account);
    ups.into_iter().find(|up| up.message == id).and_then(|up| up.draft).unwrap()
}
