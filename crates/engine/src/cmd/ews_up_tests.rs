// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What goes up from an Exchange server account (#252, E6), as the engine works it out from the
//! difference with what the server last said and takes in what became of it, with results made by
//! hand: the changes of messages and folders and their undo, drafts, sending and a send held,
//! and the Safe and Blocked Senders lists on new mail. (Against `mailcraft-ews`'s stand-in:
//! `apps/mailcraft/src/ews_up_flow.rs`.)

use mailcraft_model::sync::{
    DraftDone, EwsFolderDeltas, EwsMessagePage, FolderChange, FolderChangeDone, GraphFolder, GraphMessage, MessageChange, MessageChangeDone,
    MessageOutcome, SendDone, SendOutcome, ServerMessage,
};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::{Value, json};

use super::{apply_ews_mail, ews_inbox_request, ews_mail_request};
use crate::Session;

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn folder(id: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() }
}

fn piece(account: Id) -> mailcraft_model::sync::EwsMailResult {
    mailcraft_model::sync::EwsMailResult { account, email: ME.into(), url: URL.into(), ..Default::default() }
}

/// An Exchange server account with its folders listed and two messages in its Inbox.
fn setup() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap();
    let list = vec![
        folder("F1", "Inbox", FolderKind::Inbox),
        folder("F2", "Sent Items", FolderKind::Sent),
        folder("F3", "Drafts", FolderKind::Drafts),
        folder("F4", "Deleted Items", FolderKind::Deleted),
        folder("F6", "Junk Email", FolderKind::Junk),
        folder("F5", "Projects", FolderKind::Custom),
    ];
    let deltas = EwsFolderDeltas { changed: list, afresh: true, ..Default::default() };
    apply_ews_mail(&mut s, mailcraft_model::sync::EwsMailResult { folders: Some(deltas), hierarchy: "H".into(), partial: true, ..piece(account) })
        .unwrap();
    let message = |id: &str, from: &str, subject: &str| GraphMessage {
        server: ServerMessage { id: id.into(), folder: "F1".into(), change_key: "CK".into(), ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Zoe", from),
        to: vec![Address::new("Ada", ME)],
        sent: 900,
        received: 1_000,
        preview: subject.into(),
        ..Default::default()
    };
    let page = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![message("A1", "zoe@example.org", "Kickoff"), message("A2", "zoe@example.org", "Budget")],
        afresh: true,
        last: true,
        state: "S1".into(),
        ..Default::default()
    };
    apply_ews_mail(&mut s, mailcraft_model::sync::EwsMailResult { pages: vec![page], partial: true, ..piece(account) }).unwrap();
    let id = |server: &str| s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|x| x.id == server)).map(|m| m.id).unwrap();
    let (a, b) = (id("A1"), id("A2"));
    (s, account, a, b)
}

fn changes(s: &Session, account: Id) -> Vec<MessageChange> {
    ews_mail_request(s, account, false).unwrap().messages
}

fn took(s: &mut Session, account: Id, result: mailcraft_model::sync::EwsMailResult) -> Value {
    apply_ews_mail(s, mailcraft_model::sync::EwsMailResult { account, ..result }).unwrap()
}

#[test]
fn nothing_changed_here_sends_nothing_and_the_inbox_pass_never_does() {
    let (s, account, ..) = setup();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.changes.is_empty() && request.messages.is_empty() && request.drafts.is_empty() && request.sends.is_empty());
    assert_eq!(crate::cmd::account::unpushed(&s, account), 0);
}

#[test]
fn read_flag_category_importance_and_a_move_go_up_and_are_counted() {
    let (mut s, account, a, b) = setup();
    s.run("mail.markRead", &json!({"ids": [a]})).unwrap();
    s.run("mail.flag", &json!({"id": a})).unwrap();
    s.run("mail.categorize", &json!({"ids": [b], "category": "Blue category"})).unwrap();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    s.run("mail.move", &json!({"ids": [b], "folder": projects})).unwrap();
    let all = changes(&s, account);
    assert!(all.iter().any(|c| matches!(c, MessageChange::Update { id, read: Some(true), flag: Some(_), .. } if id == "A1")), "{all:?}");
    assert!(all.iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "A2" && to == "F5")), "{all:?}");
    assert!(all.iter().any(|c| matches!(c, MessageChange::Update { id, categories: Some(_), .. } if id == "A2")), "{all:?}");
    assert_eq!(crate::cmd::account::unpushed(&s, account), 3);
}

#[test]
fn a_move_takes_the_new_id_and_undo_is_another_change() {
    let (mut s, account, a, b) = setup();
    s.run("mail.markRead", &json!({"ids": [a]})).unwrap();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    s.run("mail.move", &json!({"ids": [b], "folder": projects})).unwrap();
    let done: Vec<MessageChangeDone> = changes(&s, account)
        .into_iter()
        .map(|change| {
            let outcome = match &change {
                MessageChange::Move { .. } => MessageOutcome::Done { categories: None, flag: None, folder: "F5".into(), id: "B2-new".into() },
                _ => MessageOutcome::Done { categories: None, flag: None, folder: String::new(), id: String::new() },
            };
            MessageChangeDone { change, outcome, there: None }
        })
        .collect();
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { messages: done, partial: true, ..piece(account) });
    assert_eq!(said["pushed"], 2);
    let moved = s.mailbox.message(b).unwrap().server.as_ref().unwrap();
    assert_eq!((moved.id.as_str(), moved.folder.as_str()), ("B2-new", "F5"));
    assert!(changes(&s, account).is_empty());
    // Undo is a reverse change: it goes up as the next run's.
    s.run("edit.undo", &json!({})).unwrap();
    let back = changes(&s, account);
    assert!(back.iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "B2-new" && to == "F1")), "{back:?}");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(changes(&s, account).iter().any(|c| matches!(c, MessageChange::Update { id, read: Some(false), .. } if id == "A1")));
}

#[test]
fn a_refusal_is_put_back_and_said() {
    let (mut s, account, a, _) = setup();
    s.run("mail.markRead", &json!({"ids": [a]})).unwrap();
    let change = changes(&s, account).remove(0);
    let said = took(
        &mut s,
        account,
        mailcraft_model::sync::EwsMailResult {
            messages: vec![MessageChangeDone { change, outcome: MessageOutcome::Refused { status: 403 }, there: None }],
            ..piece(account)
        },
    );
    assert!(!s.mailbox.message(a).unwrap().read, "put back to how the server has it");
    assert!(said["errors"].to_string().contains("may not change"), "{said}");
}

#[test]
fn folders_made_renamed_and_deleted_go_up_and_come_back_as_the_servers() {
    let (mut s, account, ..) = setup();
    s.run("folder.new", &json!({"name": "Apollo", "parent": s.mailbox.folder_by_server(account, "F5")})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    let [FolderChange::Create { folder, name, parent, .. }] = request.changes.as_slice() else { panic!("{:?}", request.changes) };
    assert_eq!((name.as_str(), parent.as_str()), ("Apollo", "F5"));
    let change = request.changes[0].clone();
    let there = GraphFolder { id: "F9".into(), name: "Apollo".into(), parent: "F5".into(), kind: FolderKind::Custom, ..Default::default() };
    let done = FolderChangeDone { change, outcome: mailcraft_model::sync::ChangeOutcome::Done { folder: there } };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { changes: vec![done], partial: true, ..piece(account) });
    assert_eq!(s.mailbox.folder(*folder).unwrap().server.as_ref().unwrap().id, "F9");
    s.run("folder.rename", &json!({"id": *folder, "name": "Apollo 11"})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(matches!(request.changes.as_slice(), [FolderChange::Rename { id, name, .. }] if id == "F9" && name == "Apollo 11"));
}

#[test]
fn mail_moved_into_the_archive_makes_it_on_the_server_once_and_the_move_follows() {
    let (mut s, account, a, _) = setup();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    // An empty Archive is not made on the server: nothing asked for it.
    assert!(ews_mail_request(&s, account, false).unwrap().changes.is_empty());
    s.run("mail.move", &json!({"ids": [a], "folder": archive})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    let [FolderChange::Create { folder, name, parent, parent_folder }] = request.changes.as_slice() else { panic!("{:?}", request.changes) };
    // At the top of the mail, named Archive; the Outbox is never made.
    assert_eq!((*folder, name.as_str(), parent.as_str(), *parent_folder), (archive, "Archive", "", None));
    // The move waits on the folder made in the same run.
    assert!(
        request
            .messages
            .iter()
            .any(|c| matches!(c, MessageChange::Move { id, to, to_folder, .. } if id == "A1" && to.is_empty() && *to_folder == Some(archive))),
        "{:?}",
        request.messages
    );
    let there = GraphFolder { id: "FA".into(), name: "Archive".into(), parent: String::new(), kind: FolderKind::Custom, ..Default::default() };
    let made = FolderChangeDone { change: request.changes[0].clone(), outcome: mailcraft_model::sync::ChangeOutcome::Done { folder: there } };
    let moved = MessageChangeDone {
        change: request.messages[0].clone(),
        outcome: MessageOutcome::Done { categories: None, flag: None, folder: "FA".into(), id: "A1-moved".into() },
        there: None,
    };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { changes: vec![made], messages: vec![moved], partial: true, ..piece(account) });
    let f = s.mailbox.folder(archive).unwrap();
    assert_eq!((f.kind, f.server.as_ref().map(|server| server.id.as_str())), (FolderKind::Archive, Some("FA")));
    assert_eq!(s.mailbox.message(a).unwrap().server.as_ref().unwrap().folder, "FA");
    // The next run asks for nothing: the folder is not made twice.
    let next = ews_mail_request(&s, account, false).unwrap();
    assert!(next.changes.is_empty() && next.messages.is_empty(), "{:?} {:?}", next.changes, next.messages);
    // And the server's listing of it, as a folder like another, is the same folder.
    let deltas = EwsFolderDeltas { changed: vec![folder_at("FA", "Archive")], ..Default::default() };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { folders: Some(deltas), hierarchy: "H2".into(), partial: true, ..piece(account) });
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == account && f.name == "Archive").count(), 1);
    assert!(ews_mail_request(&s, account, false).unwrap().changes.is_empty());
}

fn folder_at(id: &str, name: &str) -> GraphFolder {
    folder(id, name, FolderKind::Custom)
}

#[test]
fn an_archive_the_server_has_already_is_the_archive_and_is_not_made() {
    let (mut s, account, a, _) = setup();
    let deltas = EwsFolderDeltas { changed: vec![folder_at("FA", "Archive")], ..Default::default() };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { folders: Some(deltas), hierarchy: "H2".into(), partial: true, ..piece(account) });
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    assert_eq!(s.mailbox.folder(archive).unwrap().server.as_ref().map(|server| server.id.as_str()), Some("FA"));
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == account && f.name == "Archive").count(), 1);
    s.run("mail.move", &json!({"ids": [a], "folder": archive})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.changes.is_empty(), "{:?}", request.changes);
    assert!(request.messages.iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "A1" && to == "FA")), "{:?}", request.messages);
}

#[test]
fn a_draft_saved_goes_up_is_taken_in_and_a_send_is_held_when_its_answer_was_lost() {
    let (mut s, account, ..) = setup();
    s.run("mail.new", &json!({"account": account, "to": "kit@example.org", "subject": "Hello"})).unwrap();
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert!(s.mailbox.message(draft).unwrap().server_draft.as_ref().is_some_and(|d| d.pending && !d.marker.is_empty()));
    let request = ews_mail_request(&s, account, false).unwrap();
    assert_eq!(request.drafts.len(), 1);
    assert_eq!(crate::cmd::account::unpushed(&s, account), 1);
    let up = request.drafts[0].clone();
    let done = DraftDone {
        change: up,
        id: "D1".into(),
        folder: "F3".into(),
        change_key: "CK2".into(),
        fields: true,
        attachments: Some(vec![]),
        ..Default::default()
    };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { drafts: vec![done], partial: true, ..piece(account) });
    let m = s.mailbox.message(draft).unwrap();
    assert_eq!((m.server.as_ref().unwrap().id.as_str(), m.server_draft.as_ref().unwrap().pending), ("D1", false));
    assert_eq!(crate::cmd::account::unpushed(&s, account), 0);
    // Written and sent: it waits in the Outbox, and goes as the draft the server has.
    s.run("compose.send", &json!({})).unwrap();
    s.run("mail.new", &json!({"account": account, "to": "kit@example.org", "subject": "Second"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert_eq!(request.sends.len(), 2);
    // The answer to one was lost: held, said so, not sent again by itself.
    let lost = request.sends[1].message;
    let sent = SendDone { message: lost, outcome: SendOutcome::Held { lost: true, why: String::new() }, draft: None, draft_left: String::new() };
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { sent: vec![sent], partial: true, ..piece(account) });
    let held = &s.mailbox.message(lost).unwrap().held;
    assert!(held.contains("may have been sent") && held.contains("from the server") && !held.contains("Microsoft"), "{held}");
    assert!(said["errors"].to_string().contains("may have been sent"), "{said}");
    let next = ews_mail_request(&s, account, false).unwrap();
    assert!(next.sends.iter().all(|send| send.message != lost), "a held message is not offered again");
    // The other went: it leaves the Outbox, the server files the copy.
    let went = request.sends[0].message;
    let sent = SendDone { message: went, outcome: SendOutcome::Sent, draft: None, draft_left: String::new() };
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { sent: vec![sent], ..piece(account) });
    assert!(s.mailbox.message(went).is_none() && said["errors"].as_array().is_some());
    // Send Again puts the held one back.
    s.run("mail.sendAgain", &json!({"id": lost})).unwrap();
    assert!(ews_mail_request(&s, account, false).unwrap().sends.iter().any(|send| send.message == lost));
}

#[test]
fn a_saved_draft_goes_as_its_mime_and_a_draft_left_after_the_send_is_deleted_by_the_next_run() {
    let (mut s, account, ..) = setup();
    s.run("mail.new", &json!({"account": account, "to": "kit@example.org", "subject": "Hello"})).unwrap();
    s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let up = ews_mail_request(&s, account, false).unwrap().drafts[0].clone();
    let done = DraftDone {
        change: up,
        id: "D1".into(),
        folder: "F3".into(),
        change_key: "CK2".into(),
        fields: true,
        attachments: Some(vec![]),
        ..Default::default()
    };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { drafts: vec![done], partial: true, ..piece(account) });
    s.run("compose.send", &json!({})).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    // The message goes as its MIME, and names the draft the run takes away after a clear send.
    let [send] = request.sends.as_slice() else { panic!("{:?}", request.sends) };
    let queued = send.message;
    assert!(send.mime.as_ref().is_some_and(|m| m.subject == "Hello"));
    assert_eq!(send.draft.as_ref().map(|d| d.id.as_str()), Some("D1"));
    // It went, and the draft could not be taken away: the send is not offered again, and the
    // takeaway is an ordinary delete of the next run.
    let sent = SendDone { message: queued, outcome: SendOutcome::Sent, draft: None, draft_left: "D1".into() };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { sent: vec![sent], partial: true, ..piece(account) });
    assert!(s.mailbox.message(queued).is_none());
    let next = ews_mail_request(&s, account, false).unwrap();
    assert!(next.sends.is_empty());
    assert!(next.messages.contains(&MessageChange::Delete { id: "D1".into() }), "{:?}", next.messages);
}

#[test]
fn the_inbox_pass_goes_up_with_nothing() {
    let (s, account, ..) = setup();
    let request = ews_inbox_request(&s, account).unwrap();
    assert!(request.messages.is_empty() && request.sends.is_empty() && request.drafts.is_empty() && request.changes.is_empty());
}

#[test]
fn a_blocked_sender_on_new_mail_goes_to_junk_and_that_move_goes_up() {
    let (mut s, account, a, _) = setup();
    // The owner blocks Zoe: the lists sort what arrives from now on.
    s.run("junk.blockSender", &json!({"id": a})).unwrap();
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    let late = GraphMessage {
        server: ServerMessage { id: "A9".into(), folder: "F1".into(), ..Default::default() },
        message_id: "a9@example.org".into(),
        subject: "Buy now".into(),
        from: Address::new("Zoe", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        received: 2_000,
        ..Default::default()
    };
    let page = EwsMessagePage { folder: "F1".into(), messages: vec![late], from: "S1".into(), state: "S2".into(), last: true, ..Default::default() };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { pages: vec![page], partial: true, ..piece(account) });
    let id = s.mailbox.messages.values().find(|m| m.subject == "Buy now").map(|m| m.id).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, junk, "sorted by the Blocked Senders list as it arrives");
    assert!(changes(&s, account).iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "A9" && to == "F6")));
}

#[test]
fn delete_for_good_goes_up_and_is_not_undone_there() {
    let (mut s, account, a, b) = setup();
    // Delete: into Deleted Items, a move.
    s.run("mail.delete", &json!({"ids": [a]})).unwrap();
    assert!(changes(&s, account).iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "A1" && to == "F4")));
    // Delete for good (also what Delete in Deleted Items is): the server deletes it.
    s.run("mail.deletePermanently", &json!({"ids": [b]})).unwrap();
    assert!(s.mailbox.message(b).is_none());
    let all = changes(&s, account);
    let delete = all.iter().find(|c| matches!(c, MessageChange::Delete { id } if id == "A2")).unwrap_or_else(|| panic!("{all:?}")).clone();
    assert_eq!(crate::cmd::account::unpushed(&s, account), 2);
    let outcome = MessageOutcome::Done { categories: None, flag: None, folder: String::new(), id: String::new() };
    let done = MessageChangeDone { change: delete, outcome, there: None };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { messages: vec![done], partial: true, ..piece(account) });
    assert!(!changes(&s, account).iter().any(|c| matches!(c, MessageChange::Delete { .. })));
    // As for a Microsoft account, undo doesn't make it again on the server (it is said, and stays gone).
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.messages.values().all(|m| m.server.as_ref().is_none_or(|x| x.id != "A2")));
}

#[test]
fn a_copy_goes_up_and_takes_the_new_id() {
    let (mut s, account, a, _) = setup();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [a], "folder": projects})).unwrap()["copies"][0].as_u64().unwrap();
    let all = changes(&s, account);
    let change =
        all.iter().find(|c| matches!(c, MessageChange::Copy { id, to, .. } if id == "A1" && to == "F5")).unwrap_or_else(|| panic!("{all:?}")).clone();
    let done = MessageChangeDone {
        change,
        outcome: MessageOutcome::Done { categories: None, flag: None, folder: "F5".into(), id: "A1-copy".into() },
        there: None,
    };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { messages: vec![done], partial: true, ..piece(account) });
    let server = s.mailbox.message(copy).unwrap().server.as_ref().unwrap();
    assert_eq!((server.id.as_str(), server.folder.as_str()), ("A1-copy", "F5"));
    assert!(changes(&s, account).is_empty());
}

#[test]
fn undo_of_a_copy_the_server_made_goes_to_deleted_items_and_is_no_removal() {
    use crate::agent::{Verdict, check};
    let (mut s, account, a, _) = setup();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    s.run("mail.copy", &json!({"ids": [a], "folder": projects})).unwrap();
    let change = changes(&s, account).into_iter().find(|c| matches!(c, MessageChange::Copy { .. })).unwrap();
    let done = MessageChangeDone {
        change,
        outcome: MessageOutcome::Done { categories: None, flag: None, folder: "F5".into(), id: "A1-copy".into() },
        there: None,
    };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { messages: vec![done], partial: true, ..piece(account) });
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    assert!(!s.undo_takes_mail(false));
}

#[test]
fn undo_of_a_copy_the_server_has_not_made_asks_for_delete_failing_closed() {
    // Pinned: the copy is dropped here and never made there, which counts as a removal although
    // nothing of the server's goes away.
    use crate::agent::{Class, Verdict, check};
    let (mut s, account, a, _) = setup();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    s.run("mail.copy", &json!({"ids": [a], "folder": projects})).unwrap();
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Delete, _)));
}

#[test]
fn a_folder_deleted_for_good_goes_up() {
    let (mut s, account, ..) = setup();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    // The first delete moves it into Deleted Items; the second removes it for good.
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    assert!(matches!(ews_mail_request(&s, account, false).unwrap().changes.as_slice(), [FolderChange::Move { id, .. }] if id == "F5"));
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    assert!(s.mailbox.folder(projects).is_none());
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.changes.iter().any(|c| matches!(c, FolderChange::Delete { id, .. } if id == "F5")), "{:?}", request.changes);
}

/// 0.10.0 listed the server's Outbox as a folder of its own, and mail moved into it from this
/// computer stays only here (an Exchange server account doesn't put it on its server). When the
/// list leaves that Outbox out while it is empty (#747), such mail goes to a folder on this
/// computer and the folder goes, rather than staying for it and being made on the server with
/// every run, which the server refuses: its Outbox is there.
#[test]
fn mail_only_here_in_the_servers_outbox_left_out_is_kept_on_this_computer_and_the_folder_is_not_made_again() {
    let (mut s, account, ..) = setup();
    let listed = EwsFolderDeltas { changed: vec![folder("FO", "Outbox", FolderKind::Custom)], ..Default::default() };
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { folders: Some(listed), partial: true, ..piece(account) });
    let copy = s.mailbox.folder_by_server(account, "FO").unwrap();
    let local = s.mailbox.local_account();
    let notes = s.mailbox.add_folder(local, None, "Notes").unwrap();
    let mine = s.mailbox.add_message(Message { account: local, folder: notes, subject: "Mine".into(), ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [mine], "folder": copy})).unwrap();
    let moved = s.mailbox.messages.values().find(|m| m.subject == "Mine").unwrap();
    assert!(moved.folder == copy && moved.account == account && moved.server.is_none(), "only here: {moved:?}");
    let left_out = EwsFolderDeltas { deleted: vec!["FO".into()], left_out: vec!["FO".into()], ..Default::default() };
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { folders: Some(left_out), partial: true, ..piece(account) });
    let told = said["errors"].to_string();
    assert!(told.contains("1 message that was only on this computer") && told.contains("Kept from ada@example.org"), "the owner is told: {told}");
    assert!(s.mailbox.folder(copy).is_none(), "the server's Outbox is left out");
    let kept = s.mailbox.messages.values().find(|m| m.subject == "Mine").unwrap();
    let there = s.mailbox.folder(kept.folder).unwrap();
    assert_eq!((there.account, there.name.as_str()), (local, "Kept from ada@example.org"));
    assert!(kept.account == local && kept.server.is_none());
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.changes.is_empty(), "nothing is made on the server: {:?}", request.changes);
    assert!(s.mailbox.folders.iter().filter(|f| f.account == account && f.name == "Outbox").all(|f| f.kind == FolderKind::Outbox));
}

/// The server's Outbox is listed (as a folder of its own) while mail is in it, and 0.10.0 listed it
/// always: put into `s` as the folder `FO`, with its local copy returned.
fn with_listed_outbox(s: &mut Session, account: Id) -> Id {
    let listed = EwsFolderDeltas { changed: vec![folder("FO", "Outbox", FolderKind::Custom)], ..Default::default() };
    took(s, account, mailcraft_model::sync::EwsMailResult { folders: Some(listed), partial: true, ..piece(account) });
    s.mailbox.folder_by_server(account, "FO").unwrap()
}

fn outbox_left_out() -> mailcraft_model::sync::EwsMailResult {
    let left_out = EwsFolderDeltas { deleted: vec!["FO".into()], left_out: vec!["FO".into()], ..Default::default() };
    mailcraft_model::sync::EwsMailResult { folders: Some(left_out), partial: true, ..Default::default() }
}

/// A draft that is here whole has not gone up yet: with the folder it is in left out it is kept on
/// this computer too, and the server's own copy of a message (it has a server id) is not touched.
#[test]
fn a_draft_here_whole_is_kept_and_what_the_server_has_is_left_to_it() {
    let (mut s, account, ..) = setup();
    let copy = with_listed_outbox(&mut s, account);
    let draft = s.mailbox.add_message(Message { account, folder: copy, subject: "Draft".into(), draft: true, ..Default::default() }).unwrap();
    let theirs = s
        .mailbox
        .add_message(Message {
            account,
            folder: copy,
            subject: "Theirs".into(),
            server: Some(ServerMessage { id: "M9".into(), folder: "FO".into(), change_key: "CK".into(), ..Default::default() }),
            ..Default::default()
        })
        .unwrap();
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { account, ..outbox_left_out() });
    let local = s.mailbox.local_account();
    let kept = s.mailbox.message(draft).unwrap();
    let there = s.mailbox.folder(kept.folder).unwrap();
    assert_eq!((kept.account, there.name.as_str()), (local, "Kept from ada@example.org"));
    assert!(kept.server.is_none() && kept.draft, "the draft is whole and still a draft: {kept:?}");
    assert!(s.mailbox.message(theirs).is_none_or(|m| m.account == account), "the server's own message stays the server's");
    assert!(said["errors"].to_string().contains("1 message that was only on this computer"), "{said}");
}

/// Mail moved here out of a Microsoft account is on its way out of that account (its original goes
/// once the copy is safe here): keeping it on this computer leaves that as it is, so the original
/// is still removed and the copy is not lost.
#[test]
fn mail_moved_in_from_a_microsoft_account_keeps_its_move_out() {
    let (mut s, account, ..) = setup();
    let copy = with_listed_outbox(&mut s, account);
    let moving = mailcraft_model::sync::MovingOut { account: account + 100, id: "G1".into(), folder: "GF".into(), left: false };
    let id = s
        .mailbox
        .add_message(Message { account, folder: copy, subject: "From Microsoft".into(), moving_out: Some(moving.clone()), ..Default::default() })
        .unwrap();
    took(&mut s, account, mailcraft_model::sync::EwsMailResult { account, ..outbox_left_out() });
    let local = s.mailbox.local_account();
    let kept = s.mailbox.message(id).unwrap();
    assert_eq!(kept.account, local);
    assert_eq!(kept.moving_out.as_ref(), Some(&moving), "still being moved out of its Microsoft account: {kept:?}");
}

/// A list that leaves out a folder of the server's by mistake (or an id that is the account's own
/// Outbox) never sweeps MailCraft's own Outbox, which is sent from: only a folder that is the
/// server's own custom folder can be kept from.
#[test]
fn the_accounts_own_outbox_is_never_swept() {
    let (mut s, account, ..) = setup();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let held = s.mailbox.add_message(Message { account, folder: outbox, subject: "Waiting".into(), ..Default::default() }).unwrap();
    // (An id the Outbox could be given if a server's folder were ever mistaken for it.)
    if let Ok(server) = s.mailbox.folder_mut(outbox) {
        server.server = Some(mailcraft_model::ServerFolder { id: "FO".into(), ..Default::default() });
    }
    let said = took(&mut s, account, mailcraft_model::sync::EwsMailResult { account, ..outbox_left_out() });
    let still = s.mailbox.message(held).unwrap();
    assert_eq!((still.account, still.folder), (account, outbox), "left in the Outbox it is sent from: {still:?}");
    assert!(!said["errors"].to_string().contains("only on this computer"), "{said}");
}
