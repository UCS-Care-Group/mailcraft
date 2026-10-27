// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In the window (#503): a conversation's row has an arrow that opens it, Right and Left open and
//! close it, its messages are listed under it with a reply in Sent Items, and Delete on the row
//! asks first, with Don't ask again, per account.

use egui::{Key, PointerButton};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::people_view::testing;

/// The window on the sample's Inbox in conversation view, and its "Release checklist"
/// conversation: the row's message, the older one in the Inbox, and the reply in Sent Items.
fn release_checklist() -> (testing::Window, Id, Id, Id) {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.state_mut().execute("view.conversations", json!({"value": true})).unwrap();
    w.run_steps(3);
    let row = w.state_mut().session.rows().iter().find(|r| r.members.len() == 2).cloned().unwrap();
    let s = &w.state().session;
    let sent = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Sent).unwrap();
    let reply = s.mailbox.conversation(row.id).into_iter().find(|id| s.mailbox.message(*id).unwrap().folder == sent).unwrap();
    (w, row.id, row.members[1], reply)
}

/// The messages drawn under the conversation's row `head`, top to bottom.
fn drawn_under(w: &mut testing::Window, head: Id) -> Vec<Id> {
    let rows = w.state_mut().session.rows();
    rows.iter().filter(|r| r.under == Some(head) && w.state().hit.rect("message", r.id).is_some()).map(|r| r.id).collect()
}

fn kind(w: &testing::Window, id: Id) -> Option<FolderKind> {
    let s = &w.state().session;
    s.mailbox.message(id).and_then(|m| s.mailbox.folder(m.folder)).map(|f| f.kind)
}

fn click(w: &mut testing::Window, kind: &str, id: Id) {
    let pos = w.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} {id} drawn")).center();
    testing::click(w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
}

#[test]
fn the_arrow_opens_and_closes_the_conversation_and_leaves_the_selection_alone() {
    let (mut w, newer, older, reply) = release_checklist();
    let other = w.state_mut().session.rows().iter().find(|r| r.id != newer).map(|r| r.id).unwrap();
    click(&mut w, "message", other);
    assert!(drawn_under(&mut w, newer).is_empty());
    click(&mut w, "conversationArrow", newer);
    let under = drawn_under(&mut w, newer);
    assert_eq!(under.len(), 3);
    assert!([newer, older, reply].iter().all(|id| under.contains(id)), "{under:?}");
    // The reply says which folder it is in; the Inbox's don't.
    assert!(w.state().hit.rect("messageFolder", reply).is_some());
    assert!(w.state().hit.rect("messageFolder", older).is_none());
    assert_eq!(w.state().session.sel.messages, [other]);
    click(&mut w, "conversationArrow", newer);
    assert!(drawn_under(&mut w, newer).is_empty());
    assert_eq!(w.state().session.sel.messages, [other]);
    // A row of one message has no arrow.
    let single = w.state_mut().session.rows().iter().find(|r| r.size() == 1).map(|r| r.id).unwrap();
    assert!(w.state().hit.rect("conversationArrow", single).is_none());
}

#[test]
fn right_and_left_open_and_close_the_selected_conversation() {
    let (mut w, newer, _, reply) = release_checklist();
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::ArrowRight);
    w.run_steps(2);
    assert_eq!(drawn_under(&mut w, newer).len(), 3);
    // Down goes into it; the reply is read in the Inbox's list.
    for _ in 0..3 {
        if w.state().session.sel.focus() == Some(reply) {
            break;
        }
        testing::key(&mut w, Key::ArrowDown);
    }
    let s = &w.state().session;
    assert_eq!(s.sel.messages, [reply]);
    assert_eq!(s.mailbox.folder(s.view.folder).map(|f| f.kind), Some(FolderKind::Inbox));
    // Left closes it, and the conversation's row is selected.
    testing::key(&mut w, Key::ArrowLeft);
    w.run_steps(2);
    assert!(drawn_under(&mut w, newer).is_empty());
    assert_eq!((w.state().session.sel.messages.clone(), w.state().session.sel.whole.clone()), (vec![newer], vec![newer]));
}

#[test]
fn a_message_under_the_row_is_selected_and_deleted_alone_without_a_question() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "conversationArrow", newer);
    click(&mut w, "message", reply);
    assert_eq!(w.state().session.sel.focus(), Some(reply));
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!([kind(&w, reply), kind(&w, newer), kind(&w, older)], [Some(FolderKind::Deleted), Some(FolderKind::Inbox), Some(FolderKind::Inbox)]);
    // One of the row's own messages, too.
    click(&mut w, "message", older);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!([kind(&w, older), kind(&w, newer)], [Some(FolderKind::Deleted), Some(FolderKind::Inbox)]);
}

#[test]
fn delete_on_a_closed_conversation_asks_first_then_deletes_it_in_this_folder() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Inbox), Some(FolderKind::Inbox)], "nothing goes before the answer");
    // Cancel deletes nothing.
    click(&mut w, "questionCancel", 0);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Inbox), Some(FolderKind::Inbox)]);
    // Delete, from the row's own delete button too: both of the Inbox go, the reply stays.
    let pos = w.state().hit.rect("message", newer).unwrap().center();
    w.input_mut().events.push(egui::Event::PointerMoved(pos));
    w.run_steps(3);
    click(&mut w, "messageDelete", newer);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    click(&mut w, "questionAction", 0);
    assert_eq!([kind(&w, newer), kind(&w, older), kind(&w, reply)], [Some(FolderKind::Deleted), Some(FolderKind::Deleted), Some(FolderKind::Sent)]);
    // Still asked next time: the box wasn't ticked.
    let account = w.state().session.mailbox.accounts[0].id;
    assert!(w.state().session.mailbox.asks_before_deleting_conversation(account));
}

#[test]
fn dont_ask_again_turns_the_question_off_for_the_account() {
    let (mut w, newer, older, _) = release_checklist();
    let account = w.state().session.mailbox.accounts[0].id;
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    click(&mut w, "questionTick", 0);
    click(&mut w, "questionAction", 0);
    assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Deleted), Some(FolderKind::Deleted)]);
    assert!(!w.state().session.mailbox.asks_before_deleting_conversation(account));
    // The next conversation goes without a question.
    w.state_mut().execute("edit.undo", json!({})).unwrap();
    w.run_steps(3);
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!(kind(&w, older), Some(FolderKind::Deleted));
}
