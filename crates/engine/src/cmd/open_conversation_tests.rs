// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation opens in the message list (#503): its messages are listed under its row, a
//! reply in Sent Items with them, and each is selected, read, moved or deleted as any row. The
//! row counts what opens under it. Closed, Delete and Move on the row take the messages it stands
//! for in the folder in view, and nothing of it in another folder (#485), as in Outlook.

use std::collections::HashSet;

use mailcraft_model::{Account, AccountKind, FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::list::Row;
use crate::{Session, list, sample};

/// Wednesday 2026-10-14 16:00 UTC.
fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn folder(s: &Session, kind: FolderKind) -> Id {
    s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, kind).unwrap()
}

fn folder_of(s: &Session, id: Id) -> Id {
    s.mailbox.message(id).unwrap().folder
}

/// The sample's Inbox in conversation view, and its "Release checklist" conversation: the row's
/// message (the newer one in the Inbox), the older one in the Inbox, and the reply in Sent Items.
fn release_checklist() -> (Session, Id, Id, Id) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    let row = row_of(&mut s, |r| r.members.len() == 2);
    let (newer, older) = (row.members[0], row.members[1]);
    let sent = folder(&s, FolderKind::Sent);
    let reply = s.mailbox.conversation(newer).into_iter().find(|id| folder_of(&s, *id) == sent).unwrap();
    assert_eq!(s.mailbox.message(newer).unwrap().topic(), "Release checklist");
    (s, newer, older, reply)
}

fn row_of(s: &mut Session, f: impl Fn(&Row) -> bool) -> Row {
    s.rows().iter().find(|r| f(r)).cloned().unwrap()
}

/// The rows listed under the conversation's row `head`, top to bottom.
fn under(s: &mut Session, head: Id) -> Vec<(Id, u16)> {
    s.rows().iter().filter(|r| r.under == Some(head)).map(|r| (r.id, r.copy)).collect()
}

/// The messages of the rows drawn selected, a closed conversation's row standing for its
/// messages, which are what Delete and Move take; sorted.
fn drawn(s: &mut Session) -> Vec<Id> {
    let rows = s.rows();
    let taken: HashSet<Id> = s.acting_on_in(&rows).into_iter().collect();
    let mut ids: Vec<Id> = rows
        .iter()
        .filter(|r| s.shows_selected(r, &taken))
        .flat_map(|r| if r.under.is_none() && !r.open { r.members.clone() } else { vec![r.id] })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut acting = s.acting_on();
    acting.sort_unstable();
    assert_eq!(ids, acting, "what is drawn selected is what Delete takes");
    ids
}

fn sorted<const N: usize>(mut ids: [Id; N]) -> Vec<Id> {
    ids.sort_unstable();
    ids.to_vec()
}

#[test]
fn a_conversation_row_counts_its_reply_in_sent_items_as_the_reading_pane_does() {
    let (mut s, newer, older, reply) = release_checklist();
    let row = row_of(&mut s, |r| r.id == newer);
    assert_eq!((row.members.clone(), row.others.clone()), (vec![newer, older], vec![reply]));
    // The count the row shows, and the reading pane's "3 messages in this conversation".
    assert_eq!(row.size(), 3);
    assert_eq!(row.size(), s.mailbox.conversation(newer).len());
    // A conversation whose other message is a draft in Drafts counts it too, as the pane does.
    let drafts = folder(&s, FolderKind::Drafts);
    let rows = s.rows();
    let with_draft = rows.iter().find(|r| r.others.iter().any(|id| folder_of(&s, *id) == drafts)).unwrap();
    assert_eq!(with_draft.size(), s.mailbox.conversation(with_draft.id).len());
}

#[test]
fn a_conversation_opens_and_closes_with_its_messages_listed_under_it() {
    let (mut s, newer, older, reply) = release_checklist();
    let closed: Vec<Row> = s.rows().to_vec();
    let count = list::listed(&closed);
    assert!(under(&mut s, newer).is_empty());
    let opened = run(&mut s, "mail.expandConversation", json!({"id": newer}));
    assert_eq!((opened["open"].as_bool(), opened["messages"].as_u64()), (Some(true), Some(3)));
    // Under the row, every message of it, newest first; the row's own message is its copy 1.
    let listed = under(&mut s, newer);
    let mut by_date = vec![newer, older, reply];
    by_date.sort_by_key(|id| std::cmp::Reverse((list::when(s.mailbox.message(*id).unwrap()), *id)));
    assert_eq!(listed.iter().map(|(id, _)| *id).collect::<Vec<_>>(), by_date);
    assert_eq!(listed.iter().find(|(id, _)| *id == newer).map(|(_, copy)| *copy), Some(1));
    assert!(row_of(&mut s, |r| r.id == newer && r.under.is_none()).open);
    // It is still one conversation of the list, and the rows are the list built afresh.
    assert_eq!(list::listed(&s.rows()), count);
    assert_eq!(*s.rows(), list::rows(&s));
    // Closed again, from the row or from a message under it: the list is as it was.
    run(&mut s, "mail.collapseConversation", json!({"id": older}));
    assert_eq!(*s.rows(), closed);
}

#[test]
fn right_and_left_open_and_close_the_conversation_the_focus_is_on() {
    let (mut s, newer, _, reply) = release_checklist();
    let key = |s: &Session, k: &str| s.registry.by_shortcut(k).map(|c| c.id);
    assert_eq!((key(&s, "ArrowRight"), key(&s, "ArrowLeft")), (Some("mail.expandConversation"), Some("mail.collapseConversation")));
    run(&mut s, "mail.select", json!({"id": newer}));
    assert!(s.enabled("mail.expandConversation") && !s.enabled("mail.collapseConversation"));
    run(&mut s, "mail.expandConversation", json!({}));
    assert_eq!(under(&mut s, newer).len(), 3);
    assert!(!s.enabled("mail.expandConversation") && s.enabled("mail.collapseConversation"));
    // Down steps into it: to the newer message's own row, then the next.
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus_row, Some((newer, 1)));
    run(&mut s, "mail.collapseConversation", json!({}));
    assert!(under(&mut s, newer).is_empty());
    // Without conversations, neither key does anything.
    run(&mut s, "view.conversations", json!({"value": false}));
    assert!(!s.enabled("mail.expandConversation") && !s.enabled("mail.collapseConversation"));
    assert!(s.rows().iter().all(|r| r.id != reply));
}

#[test]
fn a_reply_in_sent_items_is_listed_under_the_conversation_and_selected_in_the_inbox() {
    let (mut s, newer, _, reply) = release_checklist();
    let inbox = s.view.folder;
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    let row = row_of(&mut s, |r| r.id == reply);
    assert_eq!((row.under, folder_of(&s, reply)), (Some(newer), folder(&s, FolderKind::Sent)));
    // Selected there, it is the message being read, and the list stays the Inbox's.
    run(&mut s, "mail.select", json!({"id": reply}));
    assert_eq!((s.view.folder, s.sel.messages.clone(), s.sel.focus()), (inbox, vec![reply], Some(reply)));
    assert!(s.sel.whole.is_empty());
    // It stays selected while the keys go on from it, and after a command that changes nothing
    // of the list.
    run(&mut s, "mail.markUnread", json!({}));
    assert_eq!(s.sel.messages, [reply]);
    run(&mut s, "mail.previous", json!({}));
    run(&mut s, "mail.next", json!({}));
    assert_eq!((s.view.folder, s.sel.messages.clone()), (inbox, vec![reply]));
}

#[test]
fn deleting_or_moving_a_message_under_the_open_row_takes_only_that_one() {
    let (mut s, newer, older, reply) = release_checklist();
    let (inbox, sent, deleted, archive) =
        (folder(&s, FolderKind::Inbox), folder(&s, FolderKind::Sent), folder(&s, FolderKind::Deleted), folder(&s, FolderKind::Archive));
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": older}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([older, newer, reply].map(|id| folder_of(&s, id)), [deleted, inbox, sent]);
    // The reply in Sent Items, moved from under the row.
    run(&mut s, "mail.select", json!({"id": reply}));
    run(&mut s, "mail.move", json!({"folder": archive}));
    assert_eq!([reply, newer].map(|id| folder_of(&s, id)), [archive, inbox]);
    // The newer message by its own row under the conversation, not by the conversation's row.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    assert!(s.sel.whole.is_empty());
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([newer, older, reply].map(|id| folder_of(&s, id)), [deleted, inbox, sent]);
}

#[test]
fn delete_and_move_on_a_closed_row_take_what_it_stands_for_and_nothing_elsewhere() {
    let (mut s, newer, older, reply) = release_checklist();
    let (inbox, sent, deleted, archive) =
        (folder(&s, FolderKind::Inbox), folder(&s, FolderKind::Sent), folder(&s, FolderKind::Deleted), folder(&s, FolderKind::Archive));
    // The closed row, clicked: it stands for both messages of the Inbox; the reply in Sent Items,
    // which its count takes in, is not in the list (#485) and stays where it is.
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([newer, older, reply].map(|id| folder_of(&s, id)), [deleted, deleted, sent]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!([newer, older].map(|id| folder_of(&s, id)), [inbox, inbox]);
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.move", json!({"folder": archive}));
    assert_eq!([newer, older, reply].map(|id| folder_of(&s, id)), [archive, archive, sent]);
    run(&mut s, "edit.undo", json!({}));
    // Stepped to with the keys, the row stands for both too, and Archive takes them.
    run(&mut s, "mail.first", json!({}));
    for _ in 0..s.rows().len() {
        if s.sel.focus() == Some(newer) {
            break;
        }
        run(&mut s, "mail.next", json!({}));
    }
    assert_eq!(s.sel.whole, [newer]);
    run(&mut s, "mail.archive", json!({}));
    assert_eq!([newer, older, reply].map(|id| folder_of(&s, id)), [archive, archive, sent]);
    run(&mut s, "edit.undo", json!({}));
    // The older message, selected under the closed row (from the reading pane), counts as shown
    // and is what Delete takes: not the row's other message, nor the reply.
    run(&mut s, "mail.select", json!({"id": older}));
    assert_eq!((s.sel.messages.clone(), s.view.folder), (vec![older], inbox));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([older, newer, reply].map(|id| folder_of(&s, id)), [deleted, inbox, sent]);
}

#[test]
fn closing_with_the_focus_under_it_selects_its_row() {
    let (mut s, newer, older, reply) = release_checklist();
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": reply}));
    run(&mut s, "mail.select", json!({"id": older, "extend": true}));
    run(&mut s, "mail.collapseConversation", json!({}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone(), s.sel.focus_copy()), (vec![newer], vec![newer], 0));
    // The arrow of the row closes it with the focus elsewhere: what the list still shows of the
    // selection stays, the reply in Sent Items is let go of.
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": reply}));
    let other = row_of(&mut s, |r| r.under.is_none() && r.id != newer).id;
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    run(&mut s, "mail.collapseConversation", json!({"id": newer}));
    assert_eq!(s.sel.messages, [other]);
    // So are the Inbox's messages picked under it, the newer one by its own row too: the closed
    // row isn't drawn selected for them, and Delete takes only what is.
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    run(&mut s, "mail.select", json!({"id": older, "extend": true}));
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    run(&mut s, "mail.collapseConversation", json!({"id": newer}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![other], vec![]));
    // The row picked itself stays selected, as the conversation.
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.expandConversation", json!({}));
    run(&mut s, "mail.select", json!({"id": reply, "extend": true}));
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    run(&mut s, "mail.collapseConversation", json!({"id": newer}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![newer, other], vec![newer]));
}

#[test]
fn ctrl_click_takes_a_row_drawn_selected_out_and_adds_any_other() {
    let (mut s, newer, older, reply) = release_checklist();
    let picked = |s: &Session| (s.sel.messages.clone(), s.sel.whole.clone());
    // The conversation picked, then opened: its rows in the Inbox under it are drawn selected.
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.expandConversation", json!({}));
    // Ctrl+click on the older one takes it out; the newer stays, by itself.
    run(&mut s, "mail.select", json!({"id": older, "extend": true}));
    assert_eq!(picked(&s), (vec![newer], vec![]));
    assert_eq!(s.acting_on(), [newer]);
    // Ctrl+click on the conversation's row, its message selected by its own row: the conversation.
    run(&mut s, "mail.select", json!({"id": newer, "extend": true}));
    assert_eq!(picked(&s), (vec![newer], vec![newer]));
    assert_eq!(s.acting_on(), [newer, older]);
    // Ctrl+click on the newer one's own row takes it out; the older stays.
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1, "extend": true}));
    assert_eq!(picked(&s), (vec![older], vec![]));
    // And on the rows not drawn selected adds them, each by itself.
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1, "extend": true}));
    run(&mut s, "mail.select", json!({"id": reply, "extend": true}));
    assert_eq!(picked(&s), (vec![older, newer, reply], vec![]));
    assert_eq!(s.acting_on(), [older, newer, reply]);
    // Ctrl+click on the conversation's row while it is picked takes it out.
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer, "extend": true}));
    assert_eq!(picked(&s), (vec![], vec![]));
}

#[test]
fn a_shift_range_into_the_open_conversation_is_drawn_without_a_gap() {
    let (mut s, newer, older, _) = release_checklist();
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.expandConversation", json!({}));
    run(&mut s, "mail.select", json!({"id": older, "range": true}));
    let rows = s.rows();
    let taken: HashSet<Id> = s.acting_on_in(&rows).into_iter().collect();
    let drawn: Vec<usize> = (0..rows.len()).filter(|i| s.shows_selected(&rows[*i], &taken)).collect();
    let from = rows.iter().position(|r| r.id == newer && r.under.is_none()).unwrap();
    let to = rows.iter().position(|r| r.id == older).unwrap();
    assert_eq!(drawn, (from..=to).collect::<Vec<_>>(), "{:?}", rows.iter().map(|r| (r.id, r.copy)).collect::<Vec<_>>());
    // What is drawn is what Delete takes.
    let mut ids: Vec<Id> = drawn.iter().map(|i| rows[*i].id).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut acting = s.acting_on();
    acting.sort_unstable();
    assert_eq!(ids, acting);
}

#[test]
fn a_reply_read_in_the_unread_tab_stays_under_its_conversation_while_it_is_selected() {
    let (mut s, newer, older, reply) = release_checklist();
    for id in [newer, older, reply] {
        if let Ok(m) = s.mailbox.message_mut(id) {
            m.read = false;
        }
    }
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    assert!(under(&mut s, newer).iter().any(|(id, _)| *id == reply));
    run(&mut s, "mail.select", json!({"id": reply}));
    assert!(s.mailbox.message(reply).unwrap().read);
    assert!(under(&mut s, newer).iter().any(|(id, _)| *id == reply), "{:?}", under(&mut s, newer));
    // Let go of, it leaves, read.
    run(&mut s, "mail.select", json!({"id": reply, "extend": true}));
    assert!(s.sel.messages.is_empty());
    assert!(!under(&mut s, newer).iter().any(|(id, _)| *id == reply));
}

#[test]
fn ctrl_click_on_a_closed_row_takes_all_of_it_in_or_out() {
    let (mut s, newer, older, _) = release_checklist();
    // The older message picked from the reading pane: under the closed row, by itself.
    run(&mut s, "mail.select", json!({"id": older}));
    assert_eq!(s.acting_on(), [older]);
    // Ctrl+click on the row picks the conversation, and again lets go of all of it.
    run(&mut s, "mail.select", json!({"id": newer, "extend": true}));
    assert_eq!(drawn(&mut s), sorted([newer, older]));
    run(&mut s, "mail.select", json!({"id": newer, "extend": true}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![], vec![]));
}

#[test]
fn a_conversation_picked_by_its_row_stays_picked_when_another_message_comes_on_top() {
    let (mut s, newer, older, reply) = release_checklist();
    // In the Unread tab the older message is on top of the conversation's row, and the row is
    // picked; in the All tab the newer one is on top of it, and the row is still the one picked.
    for (id, read) in [(newer, true), (older, false), (reply, false)] {
        if let Ok(m) = s.mailbox.message_mut(id) {
            m.read = read;
        }
    }
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "mail.select", json!({"id": older}));
    assert_eq!(s.sel.whole, [older]);
    run(&mut s, "view.filter", json!({"value": "all"}));
    assert_eq!(row_of(&mut s, |r| r.members.contains(&older)).id, newer);
    assert_eq!(drawn(&mut s), sorted([newer, older]));
    // The message being read stays the one it was.
    assert_eq!(s.sel.focus(), Some(older));
}

#[test]
fn a_conversation_picked_by_its_row_takes_a_message_of_it_that_came_since() {
    let (mut s, newer, older, reply) = release_checklist();
    let (inbox, deleted, sent) = (s.view.folder, folder(&s, FolderKind::Deleted), folder(&s, FolderKind::Sent));
    // The reply a minute after the newer message, so that a copy of it in the Inbox comes on top.
    let at = s.mailbox.message(newer).map(|m| (m.date + 60, m.received + 60)).unwrap();
    if let Ok(m) = s.mailbox.message_mut(reply) {
        (m.date, m.received) = at;
    }
    run(&mut s, "mail.select", json!({"id": newer}));
    let copy = run(&mut s, "mail.copy", json!({"ids": [reply], "folder": inbox}))["copies"][0].as_u64().unwrap();
    assert_eq!(row_of(&mut s, |r| r.members.contains(&newer)).id, copy);
    assert_eq!(s.sel.messages, [newer]);
    assert_eq!(drawn(&mut s), sorted([copy, newer, older]));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([copy, newer, older, reply].map(|id| folder_of(&s, id)), [deleted, deleted, deleted, sent]);
}

#[test]
fn the_keys_go_on_from_the_row_the_focus_is_on_under_the_open_conversation() {
    let (mut s, newer, older, reply) = release_checklist();
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    let at = |s: &mut Session, id: Id| s.rows().iter().position(|r| r.id == id && r.under.is_some()).unwrap();
    // The older one picked, and the newer one's own row taken in and out with Ctrl: the focus is
    // back on the older one's row, and Down goes on below it.
    run(&mut s, "mail.select", json!({"id": older}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1, "extend": true}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1, "extend": true}));
    assert_eq!((s.sel.messages.clone(), s.sel.focus_copy()), (vec![older], 0));
    let below = s.rows().get(at(&mut s, older) + 1).map(|r| r.id);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), below);
    // A range from the newer one's own row, after a Ctrl+click elsewhere, runs from that row.
    let other = row_of(&mut s, |r| r.under.is_none() && r.id != newer).id;
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    run(&mut s, "mail.select", json!({"id": reply, "range": true}));
    assert_eq!(drawn(&mut s), sorted([newer, reply]));
}

#[test]
fn the_focus_stays_on_its_message_when_a_newer_one_comes_into_the_open_conversation() {
    let (mut s, newer, _, reply) = release_checklist();
    let inbox = s.view.folder;
    let at = s.mailbox.message(newer).map(|m| (m.date + 60, m.received + 60)).unwrap();
    if let Ok(m) = s.mailbox.message_mut(reply) {
        (m.date, m.received) = at;
    }
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    let copy = run(&mut s, "mail.copy", json!({"ids": [reply], "folder": inbox}))["copies"][0].as_u64().unwrap();
    // The newer one's row under the conversation is its only one now: Down goes on below it.
    let rows = s.rows();
    let mine = rows.iter().position(|r| r.id == newer && r.under.is_some()).unwrap();
    assert_eq!(list::position_of(&rows, newer, s.sel.focus_copy()), Some(mine));
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), rows.get(mine + 1).map(|r| r.id));
    // And Left, with the focus under it, selects the conversation's row.
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.collapseConversation", json!({}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![copy], vec![copy]));
}

#[test]
fn a_closed_row_selected_stands_for_its_conversation_however_it_came_to_be() {
    let (mut s, newer, older, reply) = release_checklist();
    let (deleted, sent) = (folder(&s, FolderKind::Deleted), folder(&s, FolderKind::Sent));
    // The newer message picked by its own row; then another list closes the conversation, and
    // the row it is selected in is the conversation's.
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    run(&mut s, "view.search", json!({"query": "checklist"}));
    run(&mut s, "view.search", json!({"query": ""}));
    assert!(s.view.open.is_empty());
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![newer], vec![]));
    assert_eq!(s.acting_on(), [newer, older]);
    run(&mut s, "mail.delete", json!({}));
    assert_eq!([newer, older, reply].map(|id| folder_of(&s, id)), [deleted, deleted, sent]);
}

#[test]
fn after_deleting_a_message_under_the_open_row_the_next_one_under_it_is_selected() {
    let (mut s, newer, older, reply) = release_checklist();
    // The older message a minute before the newer, so the conversation stays where it is in the
    // list when the newer goes.
    let at = s.mailbox.message(newer).map(|m| (m.date - 60, m.received - 60)).unwrap();
    if let Ok(m) = s.mailbox.message_mut(older) {
        (m.date, m.received) = at;
    }
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    let place = s.rows().iter().position(|r| r.id == newer && r.under.is_some()).unwrap();
    run(&mut s, "mail.select", json!({"id": newer, "copy": 1}));
    run(&mut s, "mail.delete", json!({}));
    // The row that took its place: the older message's own row under the conversation, alone; not
    // the conversation's row above it, which shows the older message now.
    assert_eq!(s.rows().get(place).map(|r| (r.id, r.copy, r.under)), Some((older, 1, Some(older))));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone(), s.sel.focus_copy()), (vec![older], vec![], 1));
    assert_eq!(s.acting_on(), [older]);
    assert_eq!(folder_of(&s, reply), folder(&s, FolderKind::Sent));
}

#[test]
fn a_message_in_another_folder_opens_its_conversation() {
    let (mut s, newer, _, reply) = release_checklist();
    let opened = run(&mut s, "mail.expandConversation", json!({"id": reply}));
    assert_eq!((opened["open"].as_bool(), opened["id"].as_u64()), (Some(true), Some(newer)));
    assert_eq!(under(&mut s, newer).len(), 3);
    // The list says which rows are conversations, one of a single message here with a reply
    // elsewhere too.
    let listed = run(&mut s, "mail.list", json!({}));
    let head = listed["messages"].as_array().unwrap().iter().find(|m| m["id"] == json!(newer) && m.get("under").is_none()).unwrap();
    assert_eq!((head["open"].as_bool(), head["elsewhere"].clone()), (Some(true), json!([reply])));
}

#[test]
fn another_folder_tab_or_search_starts_with_every_conversation_closed() {
    let (mut s, newer, _, _) = release_checklist();
    for (command, params) in
        [("view.filter", json!({"value": "unread"})), ("view.search", json!({"query": "checklist"})), ("folder.select", json!({"kind": "sent"}))]
    {
        run(&mut s, "folder.select", json!({"kind": "inbox"}));
        run(&mut s, "view.filter", json!({"value": "all"}));
        run(&mut s, "mail.expandConversation", json!({"id": newer}));
        assert!(!s.view.open.is_empty());
        run(&mut s, command, params);
        assert!(s.view.open.is_empty(), "{command}");
    }
    // A search lists what it finds where it looks: the reply in Sent Items isn't counted.
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "checklist"}));
    assert!(s.rows().iter().all(|r| r.others.is_empty()));
    // And one opens in it, what was found listed under its row.
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    assert_eq!(under(&mut s, newer).len(), 2);
}

#[test]
fn each_account_asks_before_deleting_a_conversation_unless_told_not_to() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let owner = s.mailbox.accounts[0].id;
    assert!(s.mailbox.asks_before_deleting_conversation(owner));
    let listed = |s: &mut Session| run(s, "account.list", json!({}))["accounts"][0]["askBeforeDeletingConversation"].as_bool();
    assert_eq!(listed(&mut s), Some(true));
    run(&mut s, "account.edit", json!({"id": owner, "askBeforeDeletingConversation": false}));
    assert!(!s.mailbox.asks_before_deleting_conversation(owner));
    assert_eq!(listed(&mut s), Some(false));
    // Kept in the mailbox file only when it is off.
    let saved = serde_json::to_value(&s.mailbox.accounts[0]).unwrap();
    assert_eq!(saved["skipConversationQuestion"], json!(true));
    run(&mut s, "account.edit", json!({"id": owner, "askBeforeDeletingConversation": true}));
    assert!(serde_json::to_value(&s.mailbox.accounts[0]).unwrap().get("skipConversationQuestion").is_none());
    // A file from before there was one asks.
    let old: Account = serde_json::from_value(json!({"id": 9, "email": "old@example.org"})).unwrap();
    assert!(!old.skip_conversation_question);
    // A shared mailbox follows the account it is opened through, and has no switch of its own.
    s.mailbox.accounts[0].kind = AccountKind::Microsoft;
    let shared = Account { id: 900, email: "team@example.org".into(), kind: AccountKind::Microsoft, shared_of: Some(owner), ..Default::default() };
    s.mailbox.accounts.push(shared);
    run(&mut s, "account.edit", json!({"id": owner, "askBeforeDeletingConversation": false}));
    assert!(!s.mailbox.asks_before_deleting_conversation(900));
    assert!(s.run("account.edit", &json!({"id": 900, "askBeforeDeletingConversation": true})).is_err());
}

#[test]
fn the_account_on_this_computer_can_stop_asking_before_deleting_a_conversation() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    // The one imported mail goes to, with no address.
    let local = s.mailbox.add_account(Account { name: "On this computer".into(), ..Default::default() });
    run(&mut s, "account.edit", json!({"id": local, "askBeforeDeletingConversation": false}));
    assert!(!s.mailbox.asks_before_deleting_conversation(local));
    // It has no address, and gets none that isn't one.
    assert!(s.run("account.edit", &json!({"id": local, "email": "nobody"})).is_err());
    assert!(s.run("account.add", &json!({"name": "Nobody"})).is_err());
}

#[test]
fn a_ctrl_click_under_a_conversation_picked_by_its_row_leaves_the_focus_on_the_newest_message_row() {
    let (mut s, newer, older, _) = release_checklist();
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    let pick_and_take_out = |s: &mut Session| {
        run(s, "mail.select", json!({"id": newer}));
        run(s, "mail.select", json!({"id": older, "extend": true}));
    };
    // The conversation picked by its row, and the older one taken out with Ctrl: the newer one
    // stays, by its own row under the conversation, and the focus and a range's start are there.
    pick_and_take_out(&mut s);
    assert_eq!((s.sel.messages.clone(), s.sel.focus_copy(), s.sel.anchor_row), (vec![newer], 1, Some((newer, 1))));
    // Shift+Down runs from it, and doesn't take the conversation back.
    let rows = s.rows();
    let next = rows.iter().position(|r| r.id == newer && r.under.is_some()).and_then(|i| rows.get(i + 1)).map(|r| r.id).unwrap();
    run(&mut s, "mail.next", json!({"extend": true}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![newer, next], vec![]));
    // And Left, with the focus under it, selects the conversation's row.
    pick_and_take_out(&mut s);
    run(&mut s, "mail.collapseConversation", json!({}));
    assert_eq!((s.sel.messages.clone(), s.sel.whole.clone()), (vec![newer], vec![newer]));
}

#[test]
fn the_focus_stays_on_the_row_of_a_conversation_picked_by_it_when_newer_mail_comes() {
    // The conversation picked by its row and open, and a newer message of it in the Inbox.
    let picked = || {
        let (mut s, newer, older, reply) = release_checklist();
        let inbox = s.view.folder;
        let at = s.mailbox.message(newer).map(|m| (m.date + 60, m.received + 60)).unwrap();
        if let Ok(m) = s.mailbox.message_mut(reply) {
            (m.date, m.received) = at;
        }
        run(&mut s, "mail.select", json!({"id": newer}));
        run(&mut s, "mail.expandConversation", json!({}));
        let copy = run(&mut s, "mail.copy", json!({"ids": [reply], "folder": inbox}))["copies"][0].as_u64().unwrap();
        (s, newer, older, reply, copy)
    };
    let (mut s, _, _, _, copy) = picked();
    let rows = s.rows();
    assert_eq!(s.sel.focus_in(&rows), rows.iter().position(|r| r.id == copy && r.under.is_none()));
    // Down goes to the first row under it: the newest message's own.
    run(&mut s, "mail.next", json!({}));
    assert_eq!((s.sel.focus(), s.sel.focus_copy()), (Some(copy), 1));
    // Shift+Down and Shift+click run from the conversation's row, and so take all of it (and the
    // reply in Sent Items, whose row is in the range).
    let (mut s, newer, older, _, copy) = picked();
    run(&mut s, "mail.next", json!({"extend": true}));
    assert_eq!(drawn(&mut s), sorted([copy, newer, older]));
    let (mut s, newer, older, reply, copy) = picked();
    run(&mut s, "mail.select", json!({"id": older, "range": true}));
    assert_eq!(drawn(&mut s), sorted([copy, newer, older, reply]));
}

#[test]
fn closing_a_conversation_picked_in_another_tab_keeps_the_message_being_read() {
    let (mut s, newer, older, reply) = release_checklist();
    for (id, read) in [(newer, true), (older, false), (reply, false)] {
        if let Ok(m) = s.mailbox.message_mut(id) {
            m.read = read;
        }
    }
    // Picked in the Unread tab with the older message on top, and opened in the All tab.
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "mail.select", json!({"id": older}));
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "mail.expandConversation", json!({}));
    let rows = s.rows();
    assert_eq!(s.sel.focus_in(&rows), rows.iter().position(|r| r.id == newer && r.under.is_none()));
    run(&mut s, "mail.collapseConversation", json!({}));
    assert_eq!((s.sel.focus(), s.sel.whole.clone()), (Some(older), vec![older]));
}

#[test]
fn shift_down_from_a_conversation_picked_by_its_row_goes_on_under_it() {
    let (mut s, newer, _, _) = release_checklist();
    run(&mut s, "mail.expandConversation", json!({"id": newer}));
    run(&mut s, "mail.select", json!({"id": newer}));
    run(&mut s, "mail.next", json!({"extend": true}));
    assert_eq!((s.sel.focus(), s.sel.focus_copy()), (Some(newer), 1));
    run(&mut s, "mail.next", json!({"extend": true}));
    let rows = s.rows();
    let second = rows.iter().position(|r| r.id == newer && r.under.is_some()).map(|i| i + 1);
    assert_eq!(s.sel.focus_in(&rows), second);
}
