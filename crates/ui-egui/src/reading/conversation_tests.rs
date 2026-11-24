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
    assert!(doc.history.is_some(), "the reply has history to leave out");
    let text = laid_text(&w, Place::Pane, reply);
    assert!(text.contains("Freeze the main branch Tuesday noon"), "the quoted history isn't laid out: {text}");
}

/// The text of the body of `id` at `place`, as far as it is laid out.
fn laid_text(w: &testing::Window, place: Place, id: Id) -> String {
    use super::layout::ItemKind;
    view::kept_at(&w.ctx, place, id, |laid, _| {
        laid.items.iter().filter_map(|i| if let ItemKind::Text(t) = &i.kind { Some(t.job.text.clone()) } else { None }).collect::<Vec<_>>().join(
            "
",
        )
    })
    .unwrap_or_default()
}

#[test]
fn every_open_message_in_a_long_column_keeps_its_unreadable_answer() {
    use super::conversation_state::MAX_OPEN;
    let (mut w, ids) = long_conversation(60, None);
    w.state_mut().execute("mail.select", json!({"id": *ids.last().unwrap()})).unwrap();
    w.run_steps(3);
    let open = ids.iter().filter(|id| w.state().hit.rect("conversationCard", **id).is_some()).count();
    assert_eq!(open, MAX_OPEN);
    // Each frame looks the selected message and every open card up: none is looked for again.
    let key = egui::Id::unique(super::UNREADABLE_KEY);
    let kept = |w: &testing::Window| w.ctx.data(|d| d.get_temp::<Vec<(egui::Id, bool)>>(key)).unwrap_or_default();
    let before = kept(&w);
    assert_eq!(before.len(), MAX_OPEN + 1, "one answer for each message shown whole");
    w.run_steps(3);
    assert_eq!(kept(&w), before, "an answer was looked for again");
    assert!(before.len() <= super::unreadable_kept(w.state().windows.len()));
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
    use super::conversation_state::{MAX_STACKED, listed};
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
    let messages: Vec<_> = (1..=super::conversation_state::MAX_OPEN as Id + 1).map(message).collect();
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

#[test]
fn the_column_state_follows_its_rules_without_a_window() {
    use super::conversation_state::{MAX_OPEN, Open};
    // The selections' log, as the engine keeps it: (number, message selected, what it read).
    let log = |entries: &[(u64, Id, &[Id])]| -> std::collections::VecDeque<mailcraft_engine::ReadOnOpen> {
        entries.iter().map(|(count, focus, ids)| mailcraft_engine::ReadOnOpen { count: *count, focus: *focus, ids: ids.to_vec() }).collect()
    };
    let read = |id: Id| mailcraft_model::Message { id, read: true, ..Default::default() };
    let unread = |id: Id| mailcraft_model::Message { id, read: false, ..Default::default() };
    let messages: Vec<mailcraft_model::Message> = (1..=60).map(|id| if id == 3 { unread(id) } else { read(id) }).collect();
    let find = |id: Id| messages.iter().find(|m| m.id == id);
    let thread: Vec<Id> = (1..=60).collect();
    // Afresh for message 60: it and the unread one are shown whole; what selecting read too.
    let open = Open::for_frame(None, 60, 1, &log(&[(1, 60, &[10])]), &thread);
    let whole = open.shown_whole(&thread, find);
    assert!(whole.contains(&60) && whole.contains(&3) && whole.contains(&10) && whole.len() == 3);
    // Kept into the next frame; the same reads aren't taken in twice, so a close stays.
    let mut open = Open::for_frame(Some(open), 60, 2, &log(&[(1, 60, &[10])]), &thread);
    open.close(10);
    let mut open = Open::for_frame(Some(open), 60, 3, &log(&[(1, 60, &[10])]), &thread);
    assert!(!open.shown_whole(&thread, find).contains(&10));
    // A newer selection of the same message that reads it again opens it again.
    open = Open::for_frame(Some(open), 60, 4, &log(&[(1, 60, &[10]), (2, 60, &[10])]), &thread);
    assert!(open.shown_whole(&thread, find).contains(&10));
    // Expand All: the selected one and MAX_OPEN others, the newest.
    open.expand_all(&thread);
    let whole = open.shown_whole(&thread, find);
    assert_eq!(whole.len(), MAX_OPEN + 1);
    assert!(whole.contains(&59) && !whole.contains(&1));
    // Collapse All: the selected one alone, even the unread one closed.
    open.collapse_all(&thread);
    assert_eq!(open.shown_whole(&thread, find), std::collections::HashSet::from([60]));
    // Quoted text, on and off.
    open.toggle_quoted(60);
    assert!(open.quoted(60));
    open.toggle_quoted(60);
    assert!(!open.quoted(60));
    // Off screen for a frame: afresh, with what the selections of the message read open again,
    // what the owner closed forgotten.
    let open = Open::for_frame(Some(open), 60, 9, &log(&[(1, 60, &[10]), (2, 60, &[10]), (3, 60, &[])]), &thread);
    assert_eq!(open.shown_whole(&thread, find), std::collections::HashSet::from([60, 3, 10]));
    // Another message selected since: afresh, with nothing of what 60's selections read.
    let open = Open::for_frame(Some(open), 59, 10, &log(&[(1, 60, &[10]), (2, 60, &[10]), (3, 59, &[])]), &thread);
    assert_eq!(open.shown_whole(&thread, find), std::collections::HashSet::from([59, 3]));
}

/// A window on a mailbox whose Inbox holds one conversation of `n` messages, none read, linked
/// by their headers, oldest first; `long` makes one message's text far longer than a frame lays
/// out. Show Whole Conversation is on, and nothing is selected yet. The ids, oldest first.
fn long_conversation(n: u64, long: Option<u64>) -> (testing::Window, Vec<Id>) {
    use mailcraft_model::{Address, Body, Mailbox, Message};
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mailbox.inbox().unwrap();
    let mut ids = Vec::new();
    for i in 1..=n {
        let text = if Some(i) == long {
            "A line of the long message, again and again.
"
            .repeat(20_000)
        } else {
            format!("Message {i}.")
        };
        let m = Message {
            folder: inbox,
            subject: if i == 1 { "Plan".into() } else { "RE: Plan".into() },
            message_id: format!("<m{i}@example.org>"),
            in_reply_to: if i == 1 { String::new() } else { format!("<m{}@example.org>", i - 1) },
            references: (1..i).map(|j| format!("<m{j}@example.org>")).rev().take(30).collect::<Vec<_>>().into_iter().rev().collect(),
            from: Address { name: format!("Sender {i}"), email: format!("s{i}@example.org") },
            date: 1_791_000_000 + i as i64 * 60,
            body: Body { text, html: None },
            read: false,
            ..Default::default()
        };
        ids.push(mailbox.add_message(m).unwrap());
    }
    let mut app = crate::MailApp::new(mailcraft_engine::Session::at(mailbox, 1_791_993_600, 0), crate::Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.execute("view.conversationSettings", json!({"wholeConversation": true})).unwrap();
    app.execute("view.sort", json!({"by": "date"})).unwrap();
    (super::tests::window(app, (1280.0, 900.0)), ids)
}

#[test]
fn selecting_reads_only_what_the_pane_shows_whole_and_a_row_opened_is_read() {
    use super::conversation_state::MAX_OPEN;
    let (mut w, ids) = long_conversation(60, None);
    let newest = *ids.last().unwrap();
    w.state_mut().execute("mail.select", json!({"id": newest})).unwrap();
    w.run_steps(3);
    let read = |w: &testing::Window, id: Id| w.state().session.mailbox.message(id).unwrap().read;
    // The selected one and the newest MAX_OPEN others are read and shown whole; the oldest stay
    // unread rows.
    let (left, shown) = ids.split_at(ids.len() - MAX_OPEN - 1);
    assert!(shown.iter().all(|id| read(&w, *id)), "a message shown whole isn't read");
    assert!(left.iter().all(|id| !read(&w, *id)), "a message left a row was read");
    assert!(left.iter().all(|id| drawn(&w, "conversationRow", *id) || w.state().hit.rect("conversationRow", *id).is_some()));
    // A row opened reads its message.
    let oldest = ids[0];
    // The column scrolled to its top, where the oldest messages are.
    let pane = w.state().hit.rect("conversationExpandAll", 0).unwrap().center() + egui::vec2(0.0, 200.0);
    w.event(egui::Event::PointerMoved(pane));
    for _ in 0..40 {
        w.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 2_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        w.run_steps(1);
    }
    w.run_steps(3);
    click(&mut w, "conversationRow", oldest);
    assert!(read(&w, oldest));
}

#[test]
fn a_stacked_body_longer_than_a_frame_lays_out_to_its_end_with_many_open() {
    use super::conversation_state::MAX_OPEN;
    // 60 messages, none read: selecting the newest opens it and MAX_OPEN others, one of them far
    // longer than a frame lays out.
    let (mut w, ids) = long_conversation(60, Some(55));
    let long = ids[54];
    w.state_mut().execute("mail.select", json!({"id": *ids.last().unwrap()})).unwrap();
    let done = |w: &testing::Window| view::kept_at(&w.ctx, Place::Thread(long), long, |laid, _| laid.done).unwrap_or(false);
    w.step();
    assert!(!done(&w), "the body was meant to need more than a frame");
    let mut frames = 1;
    while !done(&w) && frames < 400 {
        w.step();
        frames += 1;
    }
    assert!(done(&w), "the long body was still being laid out after {frames} frames");
    let open = ids.iter().filter(|id| w.state().hit.rect("conversationCard", **id).is_some()).count();
    assert_eq!(open, MAX_OPEN, "every message read on opening is shown whole");
}

#[test]
fn a_selection_holds_in_every_body_with_49_open() {
    use super::conversation_state::MAX_OPEN;
    // 60 messages, none read: selecting the newest shows it and MAX_OPEN others whole.
    let (mut w, ids) = long_conversation(60, None);
    w.state_mut().execute("mail.select", json!({"id": *ids.last().unwrap()})).unwrap();
    w.run_steps(3);
    let open: Vec<Id> = ids.iter().copied().filter(|id| w.state().hit.rect("conversationCard", *id).is_some()).collect();
    assert_eq!(open.len(), MAX_OPEN);
    // Text selected in each in turn, frames going by: every one is still known, and selected.
    for id in &open {
        w.state_mut().execute("ui.selectAllInMessage", json!({"thread": id})).unwrap_or_else(|e| panic!("{id}: {e}"));
        w.run_steps(2);
    }
    let ctx = w.ctx.clone();
    let seen = super::selection_inspect(w.state(), &ctx);
    let selected = seen.to_string();
    for id in &open {
        assert!(selected.contains(&format!("{{\"thread\":{id}}}")), "the selection in {id} was lost: {selected}");
    }
    // And the pane's own message is still selectable.
    w.state_mut().execute("ui.selectAllInMessage", json!({"pane": true})).unwrap();
}

#[test]
fn two_selections_before_a_frame_open_what_each_read() {
    let (mut w, newer, older, reply) = release_checklist();
    // The same message selected twice before the next frame, each time reading another.
    w.state_mut().execute("mail.markUnread", json!({"ids": [older]})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.state_mut().execute("mail.markUnread", json!({"ids": [reply]})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": newer})).unwrap();
    w.run_steps(3);
    let read = |id: Id| w.state().session.mailbox.message(id).unwrap().read;
    assert!(read(older) && read(reply));
    assert!(drawn(&w, "conversationCard", older) && drawn(&w, "conversationCard", reply), "a message one of the selections read is closed");
}

#[test]
fn a_reply_that_becomes_a_forward_shows_what_it_carries() {
    use std::sync::Arc;
    let ctx = egui::Context::default();
    let body = mailcraft_model::Body {
        text: "Done.

-----Original Message-----
From: Ada
Sent: Monday
To: Ben
Subject: Plan

Old text
"
        .into(),
        html: None,
    };
    let reply = Arc::new(mailcraft_model::Message { id: 7, subject: "RE: Plan".into(), body: body.clone(), ..Default::default() });
    assert!(view::doc(&ctx, &reply, Place::Pane).history.is_some());
    // The same message, its subject changed by its server into a forward's: made again, not cut.
    let forward = Arc::new(mailcraft_model::Message { id: 7, subject: "FW: Plan".into(), body, ..Default::default() });
    assert!(view::doc(&ctx, &forward, Place::Pane).history.is_none());
}

#[test]
fn showing_the_quoted_text_lets_go_of_what_was_selected() {
    let (mut w, _, _, reply) = release_checklist();
    click(&mut w, "conversationRow", reply);
    w.state_mut().execute("ui.selectAllInMessage", json!({"thread": reply})).unwrap();
    w.run_steps(2);
    let selected = |w: &testing::Window| super::selection_inspect(w.state(), &w.ctx).to_string().contains(&format!("{{\"thread\":{reply}}}"));
    assert!(selected(&w));
    // What is shown changes: nothing hidden when it was selected may be copied.
    click(&mut w, "conversationQuoted", reply);
    w.run_steps(3);
    assert!(!selected(&w), "the selection outlived the quoted text being shown");
}

#[test]
fn page_down_after_a_click_in_a_stacked_message_scrolls_the_column() {
    let (mut w, ids) = long_conversation(60, None);
    w.state_mut().execute("mail.select", json!({"id": *ids.last().unwrap()})).unwrap();
    w.run_steps(3);
    // The keys are for a stacked message once its text is selected (as a click in it does).
    let card = ids.iter().copied().rev().find(|id| drawn(&w, "conversationCard", *id)).unwrap();
    w.state_mut().execute("ui.selectAllInMessage", json!({"thread": card})).unwrap();
    w.run_steps(2);
    let top = |w: &testing::Window| w.state().hit.rect("conversationCard", card).map(|r| r.min.y);
    let before = top(&w);
    testing::key(&mut w, egui::Key::PageUp);
    w.run_steps(3);
    assert_ne!(top(&w), before, "Page Up didn't scroll the column");
}
