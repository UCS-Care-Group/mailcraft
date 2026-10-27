// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation opens in the message list (#503): its messages are listed under its row, a
//! reply in Sent Items with them, and each is selected, read, moved or deleted as any row. The
//! row counts what opens under it. Closed, Delete and Move on the row take the messages it stands
//! for in the folder in view, and nothing of it in another folder (#485), as in Outlook.

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
