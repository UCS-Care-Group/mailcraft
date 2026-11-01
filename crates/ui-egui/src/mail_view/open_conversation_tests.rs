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
        "This deletes the conversation from Inbox: its 2 messages there go to Deleted Items. Its messages in other folders stay where they are."
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
    assert_eq!(ask.text, "This deletes 3 messages, including the conversation's messages listed here. They go to Deleted Items.");
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
    // Nor for one a newer MailCraft added, which keeps its settings as that one wrote them.
    let (mut w, newer, _, _) = release_checklist();
    if let Some(a) = w.state_mut().session.mailbox.accounts.iter_mut().find(|a| a.id == account) {
        a.kind = mailcraft_model::AccountKind::Unknown;
    }
    w.run_steps(3);
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert!(ask.tick.is_none());
}

#[test]
fn delete_of_two_conversations_speaks_of_conversations() {
    let (mut w, newer, _, _) = release_checklist();
    let inbox = w.state().session.view.folder;
    // A second conversation in the Inbox: a message there and a copy of it.
    let one = w.state_mut().session.rows().iter().find(|r| r.under.is_none() && r.size() == 1).map(|r| r.id).unwrap();
    w.state_mut().execute("mail.copy", json!({"ids": [one], "folder": inbox})).unwrap();
    let other = w.state_mut().session.rows().iter().find(|r| r.under.is_none() && r.members.contains(&one)).map(|r| r.id).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": other, "extend": true})).unwrap();
    w.run_steps(3);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    assert_eq!(
        ask.text,
        "This deletes 4 messages in Inbox, every message of the conversations there included. They go to Deleted Items. The conversations' messages in other folders stay where they are."
    );
}

#[test]
fn delete_for_good_takes_what_its_question_counted() {
    let (mut w, newer, older, reply) = release_checklist();
    let inbox = w.state().session.view.folder;
    click(&mut w, "message", newer);
    assert_eq!(w.state_mut().run("mail.deletePermanently", json!({})).unwrap()["pending"], "confirm");
    // A newer message of the conversation comes into the Inbox while the question is open.
    let s = &mut w.state_mut().session;
    let at = s.mailbox.message(newer).map(|m| (m.date + 60, m.received + 60)).unwrap();
    if let Ok(m) = s.mailbox.message_mut(reply) {
        (m.date, m.received) = at;
    }
    let came = w.state_mut().execute("mail.copy", json!({"ids": [reply], "folder": inbox})).unwrap()["copies"][0].as_u64().unwrap();
    w.run_steps(3);
    click(&mut w, "questionAction", 0);
    let s = &w.state().session;
    assert!(s.mailbox.message(newer).is_none() && s.mailbox.message(older).is_none());
    assert!(s.mailbox.message(came).is_some(), "the reply that came while it was asked stays");
}

#[test]
fn delete_for_good_of_the_message_an_id_names_goes_ahead_on_yes() {
    let (mut w, newer, older, _) = release_checklist();
    assert_eq!(w.state_mut().run("mail.deletePermanently", json!({"id": older})).unwrap()["pending"], "confirm");
    w.run_steps(3);
    click(&mut w, "questionAction", 0);
    let s = &w.state().session;
    assert!(s.mailbox.message(older).is_none(), "the message it named is still here");
    assert!(s.mailbox.message(newer).is_some());
}

#[test]
fn delete_for_good_takes_the_message_named_as_the_command_reads_it_never_the_selection() {
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "message", newer);
    // A whole number written as 12.0 is message 12.
    assert_eq!(w.state_mut().run("mail.deletePermanently", json!({"id": older as f64})).unwrap()["pending"], "confirm");
    w.run_steps(3);
    click(&mut w, "questionAction", 0);
    let s = &w.state().session;
    assert!(s.mailbox.message(older).is_none(), "the message it named is still here");
    assert!(s.mailbox.message(newer).is_some(), "the selected message went instead");
    // Named wrongly, it is refused without a question.
    assert_eq!(w.state().session.sel.messages, vec![newer]);
    for junk in [json!({"id": newer.to_string()}), json!({"id": null}), json!({"ids": []}), json!({"ids": [newer as f64 + 0.5]}), json!("x")] {
        for command in ["mail.delete", "mail.deletePermanently"] {
            assert!(w.state_mut().run(command, junk.clone()).is_err(), "{command} {junk}");
            assert!(w.state().dialog.is_none(), "{command} {junk}: asked");
            assert!(w.state().session.mailbox.message(newer).is_some_and(|m| kind(&w, m.id) == Some(FolderKind::Inbox)), "{command} {junk}");
        }
    }
}

#[test]
fn a_question_goes_with_the_mailbox_it_was_asked_about() {
    let (mut w, newer, _, _) = release_checklist();
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    w.state_mut().execute("file.new", json!({"sample": true})).unwrap();
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), None);
}

#[test]
fn a_message_of_the_conversation_that_leaves_the_folder_while_the_question_is_open_stays_where_it_went() {
    for to in [FolderKind::Archive, FolderKind::Deleted] {
        let (mut w, newer, older, _) = release_checklist();
        let account = w.state().session.mailbox.message(older).unwrap().account;
        let folder = w.state().session.mailbox.folder_of_kind(account, to).unwrap();
        click(&mut w, "message", newer);
        testing::key(&mut w, Key::Delete);
        w.run_steps(3);
        // Moved on another device while the question is open, which a Send/Receive brings in.
        w.state_mut().execute("mail.move", json!({"ids": [older], "folder": folder})).unwrap();
        w.run_steps(3);
        click(&mut w, "questionAction", 0);
        assert_eq!(testing::dialog(&w), None, "{to:?}: asked again");
        assert_eq!(kind(&w, newer), Some(FolderKind::Deleted), "{to:?}");
        assert_eq!(kind(&w, older), Some(to), "{to:?}: taken from where it went");
    }
}

#[test]
fn delete_for_good_leaves_a_message_that_left_the_folder_while_the_question_was_open() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    let account = w.state().session.mailbox.accounts[0].id;
    let archive = w.state().session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let rows = w.state_mut().session.rows();
    let (a, b) = (rows[0].id, rows[1].id);
    w.state_mut().execute("mail.select", json!({"ids": [a, b]})).unwrap();
    w.run_steps(3);
    assert_eq!(w.state_mut().run("mail.deletePermanently", json!({})).unwrap()["pending"], "confirm");
    w.run_steps(3);
    w.state_mut().execute("mail.move", json!({"ids": [b], "folder": archive})).unwrap();
    w.run_steps(3);
    click(&mut w, "questionAction", 0);
    let s = &w.state().session;
    assert!(s.mailbox.message(a).is_none());
    assert!(s.mailbox.message(b).is_some_and(|m| m.folder == archive), "deleted for good from Archive");
}

#[test]
fn an_agents_delete_for_good_leaves_a_message_that_left_the_folder_while_the_owner_was_asked() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    let account = w.state().session.mailbox.accounts[0].id;
    let archive = w.state().session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let rows = w.state_mut().session.rows();
    let (a, b) = (rows[0].id, rows[1].id);
    w.state_mut().execute("mail.select", json!({"ids": [a, b]})).unwrap();
    w.run_steps(3);
    let answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"ids": [a, b]}));
    w.state_mut().execute("mail.move", json!({"ids": [b], "folder": archive})).unwrap();
    w.run_steps(3);
    crate::agent_gate::answer(w.state_mut(), true);
    let done = answer.try_recv().unwrap();
    let s = &w.state().session;
    assert!(s.mailbox.message(a).is_none(), "{done}");
    assert!(s.mailbox.message(b).is_some_and(|m| m.folder == archive), "deleted for good from Archive: {done}");
    // All of them gone elsewhere: nothing is done, and the agent is told.
    let (c, d) = (rows[2].id, rows[3].id);
    w.state_mut().execute("mail.select", json!({"ids": [c]})).unwrap();
    w.run_steps(3);
    let answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"ids": [c, d]}));
    w.state_mut().execute("mail.move", json!({"ids": [c, d], "folder": archive})).unwrap();
    crate::agent_gate::answer(w.state_mut(), true);
    let done = answer.try_recv().unwrap();
    assert_eq!(done["ok"], json!(false), "{done}");
    let s = &w.state().session;
    assert!([c, d].iter().all(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == archive)), "{done}");
}

#[test]
fn a_delete_goes_ahead_with_the_rest_when_one_it_counted_is_gone() {
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    w.state_mut().execute("mail.deletePermanently", json!({"ids": [older]})).unwrap();
    w.run_steps(3);
    click(&mut w, "questionAction", 0);
    assert_eq!(kind(&w, newer), Some(FolderKind::Deleted));
}

#[test]
fn an_agents_delete_for_good_takes_only_what_the_owner_was_asked_about() {
    let (mut w, newer, older, reply) = release_checklist();
    let inbox = w.state().session.view.folder;
    click(&mut w, "message", newer);
    let ids = row_ids(&mut w, newer);
    let (request, answer) = crate::ControlRequest::new("engine.execute", json!({"command": "mail.deletePermanently", "params": {"ids": ids}}));
    let request = request.from_agent(Vec::new());
    assert!(matches!(crate::agent_gate::screen(w.state_mut(), &request, &[]), Some(crate::control::Outcome::Pending)));
    let what = w.state().asking.front().map(|a| a.what.clone()).unwrap();
    assert!(what.starts_with("delete 2 messages"), "{what}");
    // A newer message of the conversation comes in before the owner says yes.
    let s = &mut w.state_mut().session;
    let at = s.mailbox.message(newer).map(|m| (m.date + 60, m.received + 60)).unwrap();
    if let Ok(m) = s.mailbox.message_mut(reply) {
        (m.date, m.received) = at;
    }
    let came = w.state_mut().execute("mail.copy", json!({"ids": [reply], "folder": inbox})).unwrap()["copies"][0].as_u64().unwrap();
    w.run_steps(3);
    crate::agent_gate::answer(w.state_mut(), true);
    let done = answer.try_recv().unwrap();
    let s = &w.state().session;
    assert!(s.mailbox.message(newer).is_none() && s.mailbox.message(older).is_none(), "{done}");
    assert!(s.mailbox.message(came).is_some(), "it came while the owner was asked: {done}");
}

/// The messages an agent names for a conversation's row: those `mail.list` lists it by (#620).
fn row_ids(w: &mut testing::Window, row: u64) -> serde_json::Value {
    let list = w.state_mut().execute("mail.list", json!({})).unwrap();
    list["messages"].as_array().and_then(|m| m.iter().find(|m| m["id"] == row && m["under"].is_null())).map(|m| m["conversation"].clone()).unwrap()
}

/// An agent's call waiting for the owner's yes, with nothing having the keyboard.
fn an_agent_asks(w: &mut testing::Window, command: &str, params: serde_json::Value) -> std::sync::mpsc::Receiver<serde_json::Value> {
    let (request, answer) = crate::ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let request = request.from_agent(Vec::new());
    assert!(matches!(crate::agent_gate::screen(w.state_mut(), &request, &[]), Some(crate::control::Outcome::Pending)));
    w.run_steps(3);
    assert_eq!(w.state().asking.len(), 1);
    assert!(!w.ctx.egui_wants_keyboard_input(), "nothing has the keyboard");
    answer
}

#[test]
fn right_left_and_delete_under_an_agents_question_leave_the_list_as_it_is() {
    for held in [false, true] {
        let press = |w: &mut testing::Window, k| if held { testing::held(w, k) } else { testing::key(w, k) };
        let (mut w, newer, older, _) = release_checklist();
        click(&mut w, "message", newer);
        let ids = row_ids(&mut w, newer);
        let _answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"ids": ids}));
        press(&mut w, Key::ArrowRight);
        w.run_steps(2);
        assert!(drawn_under(&mut w, newer).is_empty(), "Right opened the conversation under the agent's question");
        press(&mut w, Key::Delete);
        w.run_steps(2);
        assert_eq!(testing::dialog(&w), None, "Delete opened a question under the agent's question");
        assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Inbox); 2]);
    }
    let (mut w, newer, _, _) = release_checklist();
    click(&mut w, "message", newer);
    click(&mut w, "conversationArrow", newer);
    let ids = row_ids(&mut w, newer);
    let _answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"ids": ids}));
    testing::key(&mut w, Key::ArrowLeft);
    w.run_steps(2);
    assert_eq!(drawn_under(&mut w, newer).len(), 3, "Left closed the conversation under the agent's question");
}

#[test]
fn right_with_delete_in_the_same_frame_opens_nothing_under_its_question() {
    let (mut w, newer, _, _) = release_checklist();
    click(&mut w, "message", newer);
    for pressed in [true, false] {
        for key in [Key::Delete, Key::ArrowRight] {
            w.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        }
        w.run_steps(1);
    }
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    assert!(drawn_under(&mut w, newer).is_empty(), "Right opened the conversation under the question");
}

#[test]
fn a_delete_whose_counted_messages_are_all_gone_does_nothing_and_says_nothing_of_ids() {
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "message", newer);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    w.state_mut().execute("mail.deletePermanently", json!({"ids": [newer, older]})).unwrap();
    w.run_steps(3);
    let selected = w.state().session.sel.messages.clone();
    click(&mut w, "questionAction", 0);
    let status = w.state().status_msg.as_ref().map(|(s, _)| s.clone());
    assert!(!status.as_deref().unwrap_or("").contains("`ids`"), "the owner is told: {status:?}");
    assert_eq!(w.state().session.sel.messages, selected);
}

#[test]
fn an_agents_question_goes_with_the_mailbox_it_was_asked_about() {
    // Delete for Good, and a Move to the Outbox, which sends.
    for (command, outbox) in [("mail.deletePermanently", false), ("mail.move", true)] {
        let (mut w, newer, older, _) = release_checklist();
        click(&mut w, "message", newer);
        let mut params = json!({"ids": row_ids(&mut w, newer)});
        if outbox {
            let s = &w.state().session;
            params["folder"] = json!(s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Outbox).unwrap());
        }
        let answer = an_agent_asks(&mut w, command, params);
        // Another mailbox takes the place of this one before the owner answers; it has the same ids.
        w.state_mut().execute("file.new", json!({"sample": true})).unwrap();
        w.run_steps(3);
        assert!(w.state().asking.is_empty(), "{command}: the question went with its mailbox");
        crate::agent_gate::answer(w.state_mut(), true);
        let done = answer.try_recv().unwrap();
        assert_eq!(done["ok"], false, "{command}: {done}");
        assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Inbox); 2], "{command}: {done}");
    }
    // A mailbox swapped in the frame the owner answers in: the yes isn't the other mailbox's.
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "message", newer);
    let ids = row_ids(&mut w, newer);
    let answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"ids": ids}));
    let other = mailcraft_engine::sample::mailbox(w.state().session.now(), 0);
    w.state_mut().session.replace_mailbox(other);
    crate::agent_gate::answer(w.state_mut(), true);
    assert_eq!(answer.try_recv().unwrap()["ok"], false);
    assert!(w.state().session.mailbox.message(newer).is_some() && w.state().session.mailbox.message(older).is_some());
}

#[test]
fn an_agents_call_with_params_the_command_refuses_is_refused_not_asked_about_the_selection() {
    for params in [json!({"id": "14"}), json!([]), json!("x")] {
        let (mut w, newer, older, _) = release_checklist();
        click(&mut w, "message", newer);
        let (request, answer) = crate::ControlRequest::new("engine.execute", json!({"command": "mail.deletePermanently", "params": params}));
        let request = request.from_agent(Vec::new());
        assert!(matches!(crate::agent_gate::screen(w.state_mut(), &request, &[]), Some(crate::control::Outcome::Done(_))), "{params}");
        assert!(w.state().asking.is_empty() && answer.try_recv().is_err(), "{params}: the owner isn't asked");
        assert_eq!([kind(&w, newer), kind(&w, older)], [Some(FolderKind::Inbox); 2]);
    }
}

#[test]
fn delete_in_the_frame_whose_escape_or_enter_answers_an_agents_question_leaves_the_list() {
    for second in [Key::Escape, Key::Enter] {
        let (mut w, newer, _, _) = release_checklist();
        // A message of its own row: Delete on it asks nothing.
        let single = w.state_mut().session.rows().iter().find(|r| r.size() == 1).map(|r| r.id).unwrap();
        click(&mut w, "message", single);
        let answer = an_agent_asks(&mut w, "mail.deletePermanently", json!({"id": newer}));
        for pressed in [true, false] {
            for key in [Key::Delete, second] {
                w.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            }
            w.run_steps(1);
        }
        w.run_steps(2);
        assert!(answer.try_recv().is_ok_and(|v| v["ok"] == json!(false)), "{second:?}: the agent was told no");
        assert_eq!(kind(&w, single), Some(FolderKind::Inbox), "{second:?}: Delete under the agent's question took the message");
    }
}

/// The question's text, with Delete Conversation open.
fn question(w: &testing::Window) -> String {
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else { panic!("no question") };
    ask.text.clone()
}

#[test]
fn the_question_doesnt_say_other_folders_stay_when_one_picked_there_goes() {
    // The reply in Sent Items picked under the open conversation as well, and a copy in Archive
    // that stays.
    let (mut w, newer, older, reply) = release_checklist();
    let archive = {
        let s = &w.state().session;
        s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Archive).unwrap()
    };
    w.state_mut().execute("mail.copy", json!({"ids": [older], "folder": archive})).unwrap();
    w.state_mut().execute("mail.expandConversation", json!({"id": newer})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": reply, "extend": true})).unwrap();
    w.run_steps(3);
    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let text = question(&w);
    click(&mut w, "questionAction", 0);
    assert_eq!(kind(&w, reply), Some(FolderKind::Deleted));
    assert!(text.starts_with("This deletes 3 messages") && !text.contains("other folders stay"), "{text}");
}

#[test]
fn delete_for_good_of_more_messages_than_a_command_names_still_goes_ahead() {
    // Deleted Items with more messages than a command's `ids` may name: the yes takes the
    // selection, as before (#503).
    let mut mb = mailcraft_engine::sample::mailbox(1_791_993_600, 0);
    let deleted = mb.folder_of_kind(mb.accounts[0].id, FolderKind::Deleted).unwrap();
    for i in 0..=mailcraft_engine::p::MAX_IDS as i64 {
        let m = mailcraft_model::Message {
            folder: deleted,
            subject: format!("Old {i}"),
            date: 1_700_000_000 + i,
            received: 1_700_000_000 + i,
            read: true,
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    let mut w = testing::window_on(mb, "view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"id": deleted})).unwrap();
    w.state_mut().execute("mail.selectAll", json!({})).unwrap();
    w.run_steps(2);
    assert!(w.state().session.sel.messages.len() > mailcraft_engine::p::MAX_IDS);
    assert_eq!(w.state_mut().run("mail.delete", json!({})).unwrap()["pending"], "confirm");
    w.run_steps(2);
    click(&mut w, "questionAction", 0);
    assert_eq!(w.state().session.mailbox.messages_in(deleted).count(), 0, "{:?}", w.state().status_msg);
}

#[test]
fn left_far_down_a_long_conversation_scrolls_the_list_to_its_row() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    let inbox = w.state().session.view.folder;
    let now = testing::now();
    let ada = mailcraft_model::Address::new("Ada", "ada@example.org");
    for i in 0..40i64 {
        let at = now - 60 - i * 60;
        let m = mailcraft_model::Message {
            folder: inbox,
            subject: "Big thread".into(),
            from: ada.clone(),
            date: at,
            received: at,
            read: true,
            ..Default::default()
        };
        w.state_mut().session.mailbox.add_message(m).unwrap();
    }
    for i in 0..30i64 {
        let at = now - 400 * 86_400 - i * 60;
        let m = mailcraft_model::Message {
            folder: inbox,
            subject: format!("Old note {i}"),
            from: ada.clone(),
            date: at,
            received: at,
            read: true,
            ..Default::default()
        };
        w.state_mut().session.mailbox.add_message(m).unwrap();
    }
    w.state_mut().session.mailbox_changed();
    w.state_mut().execute("view.conversations", json!({"value": true})).unwrap();
    w.run_steps(3);
    let head = w.state_mut().session.rows().iter().find(|r| r.members.len() == 40).unwrap().id;
    click(&mut w, "message", head);
    testing::key(&mut w, Key::ArrowRight);
    w.run_steps(2);
    for _ in 0..39 {
        testing::key(&mut w, Key::ArrowDown);
    }
    w.run_steps(3);
    assert!(w.state().hit.rect("message", head).is_none(), "the conversation's row is scrolled away");
    testing::key(&mut w, Key::ArrowLeft);
    w.run_steps(5);
    assert_eq!(w.state().session.sel.messages, [head]);
    assert!(w.state().hit.rect("message", head).is_some(), "the selected conversation's row is off screen");
}

#[test]
fn what_the_list_keeps_of_the_rows_drawn_selected_follows_every_change_of_the_selection() {
    let (mut w, newer, older, _) = release_checklist();
    // What the list drew selected, against what it would draw now.
    let check = |w: &mut testing::Window, step: &str| {
        w.run_steps(2);
        let rows = w.state_mut().session.rows();
        let now = w.state().session.drawn(&rows);
        let kept = w.state().drawn.as_ref().map(|d| d.3.clone()).unwrap();
        let drawn = |d: &mailcraft_engine::Drawn| rows.iter().map(|r| d.shows(r)).collect::<Vec<_>>();
        assert_eq!(drawn(&kept), drawn(&now), "{step}");
    };
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    check(&mut w, "the conversation's row");
    w.state_mut().execute("mail.expandConversation", json!({})).unwrap();
    check(&mut w, "opened");
    // The same message selected, by its own row under the conversation's: only what it was picked
    // by changes.
    w.state_mut().execute("mail.select", json!({"id": newer, "copy": 1})).unwrap();
    check(&mut w, "its own row");
    w.state_mut().execute("mail.select", json!({"id": older, "extend": true})).unwrap();
    check(&mut w, "and the older one");
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    check(&mut w, "the conversation's row again");
}
