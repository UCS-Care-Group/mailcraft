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
fn an_outbox_bound_to_an_empty_server_outbox_that_isnt_listed_stays_with_its_mail() {
    let (mut s, account, outbox) = setup();
    let mine = queued(&mut s, account, outbox, "Mine", None);
    let theirs = queued(&mut s, account, outbox, "Theirs", delivered());
    // A complete listing without the server's Outbox (empty, so left out): this Outbox is let go
    // of, not taken away as a folder the server no longer has.
    let listing = FolderListing { folders: Vec::new(), complete: true };
    let mut taken = Taken::default();
    take_listing(&mut s.mailbox, account, &listing, &mut taken);
    // Counted, so the run marks the mailbox changed and it is saved.
    assert!(taken.updated > 0);
    assert_eq!(s.mailbox.folder_of_kind(account, FolderKind::Outbox), Some(outbox));
    assert_eq!(s.mailbox.folder(outbox).unwrap().server, None);
    assert!(s.mailbox.message(mine).is_some() && s.mailbox.message(theirs).is_none());
    assert_eq!(subjects(&s, account), ["Mine"]);
    // One cut short lets nothing go and takes nothing away.
    let (mut s, account, outbox) = setup();
    take_listing(&mut s.mailbox, account, &FolderListing { folders: Vec::new(), complete: false }, &mut Taken::default());
    assert!(s.mailbox.folder(outbox).unwrap().server.is_some());
}

#[test]
fn the_servers_outbox_listed_before_goes_once_the_listing_leaves_it_out() {
    // The folder pane showed two Outboxes: this one, and the server's, empty.
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let theirs = GraphFolder { id: "F-ou".into(), name: "Outbox".into(), kind: FolderKind::Custom, ..Default::default() };
    take_listing(&mut s.mailbox, account, &FolderListing { folders: vec![theirs], complete: true }, &mut Taken::default());
    let named = |s: &Session| s.mailbox.folders.iter().filter(|f| f.account == account && f.name == "Outbox").count();
    assert_eq!(named(&s), 2);
    take_listing(&mut s.mailbox, account, &FolderListing { folders: Vec::new(), complete: true }, &mut Taken::default());
    assert_eq!(named(&s), 1);
    assert_eq!(s.mailbox.folder_of_kind(account, FolderKind::Outbox), Some(outbox));
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

/// AU3-26: a message handed to a server is marked before it leaves; the run's answer clears the
/// mark; a mark still there when the next run begins holds the message rather than sending it.
#[test]
fn a_send_whose_answer_never_came_is_held_at_the_next_run() {
    let (mut s, account, outbox) = setup();
    let first = queued(&mut s, account, outbox, "First", None);
    let second = queued(&mut s, account, outbox, "Second", None);
    assert!(mark_sending(&mut s.mailbox, account, &[first]));
    // Marked once: the same mark again is nothing new, and another account's id marks nothing.
    assert!(!mark_sending(&mut s.mailbox, account, &[first]));
    assert!(!mark_sending(&mut s.mailbox, account + 100, &[second]));
    let mut said = Vec::new();
    assert!(hold_unanswered(&mut s.mailbox, account, mailcraft_i18n::Lang::default(), &mut said));
    let m = s.mailbox.message(first).unwrap();
    assert_eq!((m.sending, m.held.as_str()), (false, MAY_HAVE_GONE));
    assert_eq!(said.len(), 1);
    assert!(s.mailbox.message(second).unwrap().held.is_empty());
    // Not sent again by itself: only the one never handed over goes.
    assert_eq!(subjects(&s, account), ["Second"]);
    // The owner's Send Again puts it back.
    s.run("mail.sendAgain", &json!({"id": first})).unwrap();
    assert_eq!(subjects(&s, account), ["First", "Second"]);
}

#[test]
fn a_send_whose_answer_was_taken_in_is_not_held() {
    let (mut s, account, outbox) = setup();
    let first = queued(&mut s, account, outbox, "First", None);
    mark_sending(&mut s.mailbox, account, &[first]);
    assert!(sending_answered(&mut s.mailbox, account));
    let mut said = Vec::new();
    assert!(!hold_unanswered(&mut s.mailbox, account, mailcraft_i18n::Lang::default(), &mut said));
    assert!(said.is_empty());
    assert_eq!(subjects(&s, account), ["First"]);
}

#[test]
fn the_sending_mark_is_saved_with_the_mailbox() {
    let (mut s, account, outbox) = setup();
    let first = queued(&mut s, account, outbox, "First", None);
    mark_sending(&mut s.mailbox, account, &[first]);
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let back = crate::io::read_mailbox(&bytes).unwrap();
    assert!(back.message(first).unwrap().sending);
}

/// A message moved out of the Outbox (filed in Sent Items, moved away by its owner) is not one
/// being sent any more: the move takes its mark off, and a mark found anywhere but the Outbox is
/// only taken off, never a reason to hold it.
#[test]
fn only_what_is_still_in_the_outbox_is_held() {
    let (mut s, account, outbox) = setup();
    let moved = queued(&mut s, account, outbox, "Moved", None);
    let stray = queued(&mut s, account, outbox, "Stray", None);
    assert!(mark_sending(&mut s.mailbox, account, &[moved, stray]));
    let sent_items = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    s.mailbox.move_message(moved, sent_items).unwrap();
    assert!(!s.mailbox.message(moved).unwrap().sending, "the move takes the mark off");
    // (As a file written by a version that left the mark on.)
    s.mailbox.message_mut(stray).unwrap().folder = sent_items;
    let mut said = Vec::new();
    assert!(hold_unanswered(&mut s.mailbox, account, mailcraft_i18n::Lang::default(), &mut said));
    for id in [moved, stray] {
        let m = s.mailbox.message(id).unwrap();
        assert_eq!((m.sending, m.held.as_str()), (false, ""));
    }
    assert!(said.is_empty());
}

/// What became of each message sent, as the answer says it, takes its mark off whatever it was.
#[test]
fn each_send_the_answer_accounts_for_loses_its_mark() {
    let (mut s, account, outbox) = setup();
    let refused = queued(&mut s, account, outbox, "Refused", None);
    let busy = queued(&mut s, account, outbox, "Busy", None);
    let untold = queued(&mut s, account, outbox, "Untold", None);
    mark_sending(&mut s.mailbox, account, &[refused, busy, untold]);
    let lang = mailcraft_i18n::Lang::default();
    let mut said = Vec::new();
    for (message, outcome) in [(refused, SendOutcome::Held { lost: false, why: "no recipients".into() }), (busy, SendOutcome::Waits)] {
        send_done(&mut s.mailbox, account, &SendDone { message, outcome, draft: None, draft_left: String::new() }, lang, &mut said);
        assert!(!s.mailbox.message(message).unwrap().sending);
    }
    assert!(s.mailbox.message(untold).unwrap().sending, "one the answer doesn't name keeps its mark");
}

/// What the server put in the Outbox and an answer names anyway (`sync.holdSends` holds all that
/// waits in a shared mailbox's Outbox) is not taken for the owner's (#738): it gets no marker, so
/// Send Again doesn't make it one to send.
#[test]
fn a_held_answer_for_the_servers_own_mail_doesnt_make_it_the_owners() {
    let (mut s, account, outbox) = setup();
    let theirs = queued(&mut s, account, outbox, "Theirs", delivered());
    // Only what a fetch gave it: no marker, nothing pending.
    s.mailbox.message_mut(theirs).unwrap().server_draft = Some(ServerDraft::default());
    let done =
        SendDone { message: theirs, outcome: SendOutcome::Held { lost: false, why: "declined".into() }, draft: None, draft_left: String::new() };
    let mut said = Vec::new();
    send_done(&mut s.mailbox, account, &done, mailcraft_i18n::Lang::default(), &mut said);
    assert!(s.mailbox.message(theirs).unwrap().server_draft.as_ref().unwrap().marker.is_empty());
    s.run("mail.sendAgain", &json!({"id": theirs})).unwrap();
    assert!(subjects(&s, account).is_empty());
    assert!(from_server(s.mailbox.message(theirs).unwrap()));
}
