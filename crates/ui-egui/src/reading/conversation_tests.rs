// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation stacked in the reading pane (#1778): with Conversation Settings › Show Whole
//! Conversation in Reading Pane on (off by default: Outlook reads one message), the selected
//! message is read with the rest of its conversation, the others closed to a row unless they are
//! unread, a row opens its message where it is, and Expand All and Collapse All open and close
//! them all; with it off, or out of conversation view, the message is read alone with its
//! conversation's line.

use egui::PointerButton;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::Place;
use super::view;
use crate::people_view::testing;

/// The window on the sample's Inbox in conversation view, the whole conversation shown in the
/// reading pane, with the "Release checklist" conversation selected and read: its newest message
/// (selected), the older one in the Inbox, and the reply in Sent Items.
fn release_checklist() -> (testing::Window, Id, Id, Id) {
    let (mut w, newer, older, reply) = release_checklist_as_outlook();
    w.state_mut().execute("view.conversationSettings", json!({"wholeConversation": true})).unwrap();
    w.run_steps(3);
    (w, newer, older, reply)
}

/// [`release_checklist`] as Outlook reads it: Show Whole Conversation off, as it is by default.
fn release_checklist_as_outlook() -> (testing::Window, Id, Id, Id) {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.state_mut().execute("view.conversations", json!({"value": true})).unwrap();
    w.run_steps(3);
    let row = w.state_mut().session.rows().iter().find(|r| r.members.len() == 2).cloned().unwrap();
    let s = &w.state().session;
    let sent = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Sent).unwrap();
    let reply = s.mailbox.conversation(row.id).into_iter().find(|id| s.mailbox.message(*id).unwrap().folder == sent).unwrap();
    let older = row.members.iter().copied().find(|id| *id != row.id).unwrap();
    w.state_mut().execute("mail.markRead", json!({"ids": [row.id, older, reply]})).unwrap();
    click(&mut w, "message", row.id);
    (w, row.id, older, reply)
}

fn click(w: &mut testing::Window, kind: &str, id: Id) {
    let pos = w.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} {id} drawn")).center();
    testing::click(w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
}

fn drawn(w: &testing::Window, kind: &str, id: Id) -> bool {
    w.state().hit.rect(kind, id).is_some_and(|r| r.is_positive())
}

/// Whether `id`'s body is laid out at `place`.
fn laid_out(w: &testing::Window, place: Place, id: Id) -> bool {
    view::kept_at(&w.ctx, place, id, |_, _| ()).is_some()
}

#[test]
fn the_selected_message_is_read_with_its_conversation_the_others_closed() {
    let (w, newer, older, reply) = release_checklist();
    assert_eq!(w.state().session.sel.focus(), Some(newer));
    // The selected one whole, in the pane's own place; the others a row each, oldest first.
    assert!(laid_out(&w, Place::Pane, newer));
    for id in [older, reply] {
        assert!(drawn(&w, "conversationRow", id), "no row for {id}");
        assert!(!drawn(&w, "conversationCard", id));
        assert!(!laid_out(&w, Place::Thread(id), id));
    }
    // The line of buttons that jump to another message is not needed: they are all here.
    assert!(!drawn(&w, "conversationMessage", older));
    assert!(drawn(&w, "conversationExpandAll", 0) && drawn(&w, "conversationCollapseAll", 0));
}

#[test]
fn a_row_opens_its_message_where_it_is_and_leaves_the_selection_alone() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "conversationRow", older);
    w.run_steps(3);
    assert!(drawn(&w, "conversationCard", older));
    assert!(laid_out(&w, Place::Thread(older), older));
    assert!(drawn(&w, "conversationRow", reply));
    assert_eq!(w.state().session.sel.messages, [newer]);
    assert!(laid_out(&w, Place::Pane, newer));
}

#[test]
fn expand_all_and_collapse_all_open_and_close_every_message_but_the_selected_one() {
    let (mut w, newer, older, reply) = release_checklist();
    click(&mut w, "conversationExpandAll", 0);
    w.run_steps(3);
    for id in [older, reply] {
        assert!(drawn(&w, "conversationCard", id), "{id} not open");
        assert!(!drawn(&w, "conversationRow", id));
    }
    click(&mut w, "conversationCollapseAll", 0);
    for id in [older, reply] {
        assert!(drawn(&w, "conversationRow", id), "{id} not closed");
    }
    assert!(laid_out(&w, Place::Pane, newer));
    assert!(!drawn(&w, "conversationRow", newer));
}

#[test]
fn a_message_not_read_yet_is_open_unless_the_owner_closed_it() {
    let (mut w, _, older, reply) = release_checklist();
    w.state_mut().execute("mail.markUnread", json!({"ids": [older]})).unwrap();
    w.run_steps(3);
    assert!(drawn(&w, "conversationCard", older));
    assert!(drawn(&w, "conversationRow", reply));
    // Showing it doesn't read it.
    assert!(!w.state().session.mailbox.message(older).unwrap().read);
    click(&mut w, "conversationCollapseAll", 0);
    assert!(drawn(&w, "conversationRow", older));
}

#[test]
fn by_default_the_message_is_read_alone_as_outlook_reads_it() {
    let (w, newer, older, reply) = release_checklist_as_outlook();
    assert!(!w.state().session.view.whole_conversation);
    assert!(laid_out(&w, Place::Pane, newer));
    assert!(!drawn(&w, "conversationRow", older) && !drawn(&w, "conversationRow", reply));
    assert!(!drawn(&w, "conversationExpandAll", 0));
    assert!(drawn(&w, "conversationMessage", older));
}

#[test]
fn turning_the_setting_on_turns_conversation_view_on_and_off_leaves_it() {
    let mut w = testing::window("view.mail");
    let a = w.state_mut();
    a.execute("view.conversations", json!({"value": false})).unwrap();
    let on = a.execute("view.conversationSettings", json!({"wholeConversation": true})).unwrap();
    assert_eq!(on, json!({"wholeConversation": true, "conversations": true}));
    let off = a.execute("view.conversationSettings", json!({"wholeConversation": false})).unwrap();
    assert_eq!(off, json!({"wholeConversation": false, "conversations": true}));
    // Without params it says the settings and changes nothing.
    assert_eq!(a.execute("view.conversationSettings", json!({})).unwrap(), off);
}

#[test]
fn out_of_conversation_view_the_message_is_read_alone() {
    let (mut w, newer, older, _) = release_checklist();
    w.state_mut().execute("view.conversations", json!({"value": false})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.run_steps(3);
    assert!(!drawn(&w, "conversationRow", older));
    assert!(!drawn(&w, "conversationExpandAll", 0));
    // Its conversation's line, to go to the others.
    assert!(drawn(&w, "conversationMessage", older));
}

#[test]
fn a_selection_command_naming_a_stacked_message_wrongly_is_refused() {
    let (mut w, _, older, _) = release_checklist();
    assert!(w.state_mut().execute("ui.copySelection", json!({"thread": "x"})).is_err());
    assert!(w.state_mut().execute("ui.copySelection", json!({"thread": -1})).is_err());
    // Nothing selected in it: nothing to copy, and no harm.
    let _ = w.state_mut().execute("ui.copySelection", json!({"thread": older}));
}

/// How many things the body of `id` at `place` is laid out as.
fn laid_items(w: &testing::Window, place: Place, id: Id) -> usize {
    view::kept_at(&w.ctx, place, id, |laid, _| laid.items.len()).unwrap_or(0)
}

#[test]
fn a_click_on_the_line_of_who_sent_it_closes_a_message_and_its_row_opens_it_again() {
    let (mut w, newer, older, _) = release_checklist();
    click(&mut w, "conversationRow", older);
    assert!(drawn(&w, "conversationCard", older));
    click(&mut w, "conversationHeader", older);
    assert!(drawn(&w, "conversationRow", older) && !drawn(&w, "conversationCard", older));
    click(&mut w, "conversationRow", older);
    assert!(drawn(&w, "conversationCard", older));
    // The selected one closes too, and stays selected.
    click(&mut w, "conversationHeader", newer);
    assert!(drawn(&w, "conversationRow", newer));
    assert_eq!(w.state().session.sel.messages, [newer]);
    click(&mut w, "conversationRow", newer);
    assert!(laid_out(&w, Place::Pane, newer) && !drawn(&w, "conversationRow", newer));
}

#[test]
fn the_earlier_messages_a_reply_quotes_are_left_out_until_asked_for() {
    let (mut w, _, _, reply) = release_checklist();
    // The reply in Sent Items carries the message it answers under "-----Original Message-----".
    click(&mut w, "conversationRow", reply);
    w.run_steps(3);
    assert!(drawn(&w, "conversationQuoted", reply), "no quoted-text line");
    let trimmed = laid_items(&w, Place::Thread(reply), reply);
    click(&mut w, "conversationQuoted", reply);
    w.run_steps(5);
    let whole = laid_items(&w, Place::Thread(reply), reply);
    assert!(whole > trimmed && trimmed > 0, "{trimmed} then {whole}");
    click(&mut w, "conversationQuoted", reply);
    w.run_steps(5);
    assert_eq!(laid_items(&w, Place::Thread(reply), reply), trimmed);
}

#[test]
fn read_alone_a_reply_shows_what_it_quotes() {
    let (mut w, _, _, reply) = release_checklist_as_outlook();
    w.state_mut().execute("mail.select", json!({"id": reply})).unwrap();
    w.run_steps(5);
    assert!(!drawn(&w, "conversationQuoted", reply));
    let doc = view::current_doc(&w.ctx, Place::Pane, reply).unwrap();
    assert!(doc.history.is_some());
}
