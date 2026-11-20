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
    // Marked unread while it is on screen, it stays so until it is selected again.
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

#[test]
fn selecting_a_message_reads_its_whole_conversation_when_the_pane_shows_it() {
    let (mut w, newer, older, reply) = release_checklist_as_outlook();
    let read = |w: &testing::Window, id: Id| w.state().session.mailbox.message(id).unwrap().read;
    // Read alone (the default), only the selected one is read.
    w.state_mut().execute("mail.markUnread", json!({"ids": [newer, older, reply]})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    assert!(read(&w, newer) && !read(&w, older) && !read(&w, reply));
    // Stacked, the whole conversation is read with it.
    w.state_mut().execute("mail.markUnread", json!({"ids": [newer]})).unwrap();
    w.state_mut().execute("view.conversationSettings", json!({"wholeConversation": true})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    assert!(read(&w, newer) && read(&w, older) && read(&w, reply));
    // Unless File › Options says opening a message doesn't read it.
    w.state_mut().execute("mail.markUnread", json!({"ids": [newer, older, reply]})).unwrap();
    w.state_mut().execute("file.options", json!({"markReadOnOpen": false})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    assert!(!read(&w, newer) && !read(&w, older) && !read(&w, reply));
}

#[test]
fn a_message_selecting_read_is_shown_whole_as_one_not_read_yet_is() {
    let (mut w, newer, older, reply) = release_checklist();
    // Another message, then back to this conversation with its older message not read yet.
    let other = w.state_mut().session.rows().iter().find(|r| r.id != newer && r.size() == 1).map(|r| r.id).unwrap();
    click(&mut w, "message", other);
    w.state_mut().execute("mail.markUnread", json!({"ids": [older]})).unwrap();
    click(&mut w, "message", newer);
    assert!(w.state().session.mailbox.message(older).unwrap().read);
    assert!(drawn(&w, "conversationCard", older), "the message read on opening is closed");
    assert!(drawn(&w, "conversationRow", reply));
}

#[test]
fn selecting_the_same_message_again_opens_what_it_reads_and_keeps_what_the_owner_closed() {
    let (mut w, newer, older, reply) = release_checklist();
    assert!(drawn(&w, "conversationRow", older));
    // Marked unread, then the same message selected again: it reads the older one, which opens.
    w.state_mut().execute("mail.markUnread", json!({"ids": [older]})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.run_steps(3);
    assert!(w.state().session.mailbox.message(older).unwrap().read);
    assert!(drawn(&w, "conversationCard", older), "the message read again is closed");
    // Closed by the owner, it stays closed in the frames after.
    click(&mut w, "conversationHeader", older);
    w.run_steps(3);
    assert!(drawn(&w, "conversationRow", older) && drawn(&w, "conversationRow", reply));
}

#[test]
fn the_selected_message_is_listed_however_long_its_conversation() {
    use super::conversation::{MAX_STACKED, listed};
    let thread: Vec<Id> = (1..=250).collect();
    // The newest, when the selected one is among them.
    let newest = listed(&thread, 240);
    assert_eq!((newest.len(), newest.first(), newest.last()), (MAX_STACKED, Some(&51), Some(&250)));
    // The oldest selected: it, then the newest but one, oldest first.
    let oldest = listed(&thread, 1);
    assert_eq!((oldest.len(), oldest.first(), oldest.get(1), oldest.last()), (MAX_STACKED, Some(&1), Some(&52), Some(&250)));
    assert_eq!(listed(&thread[..5], 3), thread[..5].to_vec());
}

#[test]
fn the_layouts_of_many_stacked_messages_are_kept_from_frame_to_frame() {
    use std::sync::Arc;
    let ctx = egui::Context::default();
    let message = |id: Id| {
        Arc::new(mailcraft_model::Message { id, body: mailcraft_model::Body { text: format!("Message {id}"), html: None }, ..Default::default() })
    };
    let messages: Vec<_> = (1..=super::conversation::MAX_OPEN as Id + 1).map(message).collect();
    // Every message the column shows whole, frame after frame: each one's blocks are made once.
    for _ in 0..3 {
        for m in &messages {
            let _ = view::doc(&ctx, m, Place::Thread(m.id));
        }
    }
    assert_eq!(view::docs_made(&ctx), messages.len());
    // And what is kept for the column is bounded.
    let more: Vec<_> = (1_000..1_000 + 2 * super::view::KEPT_IN_STACK as Id).map(message).collect();
    for m in &more {
        let _ = view::doc(&ctx, m, Place::Thread(m.id));
    }
    assert!(view::kept_count(&ctx) <= super::view::KEPT_IN_STACK);
}
