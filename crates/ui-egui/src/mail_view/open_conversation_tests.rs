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
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert_eq!(
        ask.text,
        "This deletes the whole conversation: its 2 messages in Inbox go to Deleted Items. Its messages in other folders stay where they are."
    );
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

#[test]
fn a_right_click_keeps_the_rows_drawn_selected_and_picks_any_other() {
    let (mut w, newer, older, _) = release_checklist();
    // The conversation's row picked, then opened: the newer message's own row under it is drawn
    // selected too, being one of what the row stands for.
    click(&mut w, "message", newer);
    click(&mut w, "conversationArrow", newer);
    let arrow = w.state().hit.rect("conversationArrow", newer).unwrap();
    let below = w.state().hit.rect("message", older).unwrap();
    let head = egui::pos2(below.center().x, arrow.center().y);
    let own = egui::pos2(below.center().x, arrow.center().y + below.height());
    let picked = |w: &testing::Window| {
        let s = &w.state().session;
        (s.sel.messages.clone(), s.sel.whole.clone(), s.sel.focus_copy())
    };
    testing::click(&mut w, own, PointerButton::Secondary, 1);
    w.run_steps(3);
    assert_eq!(picked(&w), (vec![newer], vec![newer], 0));
    testing::key(&mut w, Key::Escape);
    w.run_steps(2);
    // Its own row picked alone, a right click on the conversation's row picks the conversation.
    testing::click(&mut w, own, PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(picked(&w), (vec![newer], vec![], 1));
    testing::click(&mut w, head, PointerButton::Secondary, 1);
    w.run_steps(3);
    assert_eq!(picked(&w), (vec![newer], vec![newer], 0));
    testing::key(&mut w, Key::Escape);
    w.run_steps(2);
}

#[test]
fn delete_during_a_search_asks_about_what_the_search_found() {
    let (mut w, newer, older, reply) = release_checklist();
    w.state_mut().execute("view.search", json!({"query": "checklist", "scope": "everywhere"})).unwrap();
    w.run_steps(3);
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert_eq!(ask.text, "This deletes the 3 messages of this conversation that are listed here. They go to Deleted Items.");
    click(&mut w, "questionAction", 0);
    assert_eq!([kind(&w, newer), kind(&w, older), kind(&w, reply)], [Some(FolderKind::Deleted); 3]);
}

#[test]
fn delete_with_the_reply_picked_as_well_asks_about_every_message_and_says_none_stays() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "message", newer);
    click(&mut w, "conversationArrow", newer);
    w.state_mut().execute("mail.select", json!({"id": reply, "extend": true})).unwrap();
    w.run_steps(3);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert_eq!(ask.text, "This deletes 3 messages, including the conversations' messages listed here. They go to Deleted Items.");
    click(&mut w, "questionAction", 0);
    assert_eq!([kind(&w, newer), kind(&w, older), kind(&w, reply)], [Some(FolderKind::Deleted); 3]);
}

#[test]
fn delete_in_the_unread_tab_says_it_takes_what_is_listed_not_the_whole_conversation() {
    let (mut w, newer, older, reply) = release_checklist();
    let inbox = w.state().session.view.folder;
    // A third message of it in the Inbox, read: the Unread tab doesn't list it.
    let copy = w.state_mut().execute("mail.copy", json!({"ids": [reply], "folder": inbox})).unwrap()["copies"][0].as_u64().unwrap();
    for (id, read) in [(newer, false), (older, false), (copy, true)] {
        if let Ok(m) = w.state_mut().session.mailbox.message_mut(id) {
            m.read = read;
        }
    }
    w.state_mut().execute("view.filter", json!({"value": "unread"})).unwrap();
    w.run_steps(3);
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert_eq!(
        ask.text,
        "This deletes the 2 messages of this conversation that are listed here. They go to Deleted Items. Its messages in other folders stay where they are."
    );
    click(&mut w, "questionAction", 0);
    assert_eq!([kind(&w, newer), kind(&w, older), kind(&w, copy)], [Some(FolderKind::Deleted), Some(FolderKind::Deleted), Some(FolderKind::Inbox)]);
}

#[test]
fn delete_of_every_message_picked_by_its_own_row_asks_nothing() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "conversationArrow", newer);
    w.state_mut().execute("mail.select", json!({"id": newer, "copy": 1})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": older, "extend": true})).unwrap();
    w.run_steps(3);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!([kind(&w, newer), kind(&w, older), kind(&w, reply)], [Some(FolderKind::Deleted), Some(FolderKind::Deleted), Some(FolderKind::Sent)]);
}

#[test]
fn the_newest_message_picked_in_the_reading_pane_is_its_own_row_under_the_open_conversation() {
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "conversationArrow", newer);
    // The older one read (it is below the list's edge), then the newest picked in the reading pane.
    w.state_mut().execute("mail.select", json!({"id": older})).unwrap();
    w.run_steps(3);
    click(&mut w, "conversationMessage", newer);
    let s = &w.state().session;
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone(), s.sel.focus_copy()), (vec![newer], vec![], 1));
}

#[test]
fn the_account_on_this_computer_is_asked_with_no_box_as_it_has_no_account_settings() {
    let (mut w, newer, _, _) = release_checklist();
    // The conversation's account as the one on this computer, which has no address.
    let session = &w.state().session;
    let account = session.mailbox.folders.iter().find(|f| f.id == session.view.folder).map(|f| f.account).unwrap();
    if let Some(a) = w.state_mut().session.mailbox.accounts.iter_mut().find(|a| a.id == account) {
        a.email.clear();
    }
    w.run_steps(3);
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert!(ask.tick.is_none());
}
