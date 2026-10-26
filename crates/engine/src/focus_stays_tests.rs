// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A selected message stays in the list while it is selected (#428): one opened in the Unread tab,
//! by a click or a key, or whose flag is cleared in the Flagged tab, is still listed, and the keys
//! that move in the list go on from it; several picked with Ctrl or Shift all stay. One deleted or
//! moved leaves at once.
use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::*;

/// Wednesday 2026-10-14 16:00 UTC.
fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The rows of the list, which are those of a list built afresh: the one kept is never stale.
fn ids(s: &mut Session) -> Vec<Id> {
    let shown: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert_eq!(shown, list::rows(s).iter().map(|r| r.id).collect::<Vec<_>>(), "the list kept for the window is stale");
    shown
}

fn read(s: &Session, id: Id) -> bool {
    s.mailbox.message(id).unwrap().read
}

/// The sample's inbox in the Unread tab, and the unread messages it lists, top to bottom.
fn unread_tab() -> (Session, Vec<Id>) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let listed = ids(&mut s);
    assert!(listed.len() >= 4, "the sample has unread mail: {listed:?}");
    assert!(listed.iter().all(|id| !read(&s, *id)));
    (s, listed)
}

#[test]
fn an_opened_message_stays_in_the_unread_tab_and_down_goes_to_the_next() {
    let (mut s, listed) = unread_tab();
    let inbox = s.view.folder;
    let unread = s.mailbox.unread_count(inbox);
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    // Read, counted as read, and still listed where it was.
    assert!(read(&s, listed[1]));
    assert_eq!(s.mailbox.unread_count(inbox), unread - 1);
    assert_eq!(ids(&mut s), listed);
    // Down opens the one below it, not the first; the one read before leaves then.
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[2]));
    let mut rest = listed.clone();
    rest.remove(1);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn up_goes_on_from_the_opened_message() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[2]}));
    run(&mut s, "mail.previous", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[1]));
    let mut rest = listed.clone();
    rest.remove(2);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn home_keeps_the_message_it_opens() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[2]}));
    run(&mut s, "mail.first", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[0]));
    assert!(read(&s, listed[0]));
    let mut rest = listed.clone();
    rest.remove(2);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn end_keeps_the_message_it_opens() {
    let (mut s, listed) = unread_tab();
    let end = listed.len() - 1;
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    run(&mut s, "mail.last", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[end]));
    assert!(read(&s, listed[end]));
    assert_eq!(ids(&mut s), listed[1..]);
}

#[test]
fn page_down_goes_on_from_the_message_it_opens() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    run(&mut s, "mail.pageDown", json!({"rows": 2}));
    assert_eq!(s.sel.focus(), Some(listed[2]));
    assert!(read(&s, listed[2]));
    assert_eq!(ids(&mut s), listed[1..]);
    // Down goes on from it, not from the first row.
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[3]));
}

#[test]
fn page_up_goes_on_from_the_message_it_opens() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[3]}));
    run(&mut s, "mail.pageUp", json!({"rows": 2}));
    assert_eq!(s.sel.focus(), Some(listed[1]));
    assert!(read(&s, listed[1]));
    let mut rest = listed.clone();
    rest.remove(3);
    assert_eq!(ids(&mut s), rest);
    run(&mut s, "mail.previous", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[0]));
}

#[test]
fn shift_down_runs_from_the_opened_message_and_the_range_stays() {
    let (mut s, listed) = unread_tab();
    let (b, c) = (listed[1], listed[2]);
    run(&mut s, "mail.select", json!({"id": b}));
    assert!(read(&s, b));
    // Shift+Down past it: the range runs from it, and marked read, they stay together.
    run(&mut s, "mail.next", json!({"extend": true}));
    assert_eq!(s.sel.messages, [b, c]);
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // Shift+Up takes the last one out again, and it leaves.
    run(&mut s, "mail.previous", json!({"extend": true}));
    assert_eq!(s.sel.messages, [b]);
    let mut rest = listed.clone();
    rest.remove(2);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn shift_home_keeps_the_range() {
    let (mut s, listed) = unread_tab();
    let (a, b) = (listed[0], listed[1]);
    run(&mut s, "mail.select", json!({"id": b}));
    run(&mut s, "mail.first", json!({"extend": true}));
    assert_eq!(s.sel.messages, [b, a]);
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn shift_page_down_keeps_the_range() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    run(&mut s, "mail.pageDown", json!({"rows": 2, "extend": true}));
    assert_eq!(s.sel.messages, listed[..3]);
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn an_unread_search_keeps_the_opened_message_too() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "is:unread"}));
    let listed = ids(&mut s);
    assert!(listed.len() >= 3);
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    assert!(read(&s, listed[0]));
    assert_eq!(ids(&mut s), listed);
    // A space typed after it looks for the same: the same list.
    run(&mut s, "view.search", json!({"query": "is:unread "}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[1]));
    assert_eq!(ids(&mut s), listed[1..]);
}

#[test]
fn a_message_whose_flag_is_cleared_stays_in_the_flagged_tab() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let all = ids(&mut s);
    run(&mut s, "mail.flag", json!({"ids": [all[0], all[1], all[2]]}));
    run(&mut s, "view.filter", json!({"value": "flagged"}));
    let listed = ids(&mut s);
    assert!(listed.len() >= 3);
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    run(&mut s, "mail.clearFlag", json!({}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[2]));
    let mut rest = listed.clone();
    rest.remove(1);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn marked_read_by_hand_it_stays_and_a_click_elsewhere_drops_it() {
    let (mut s, listed) = unread_tab();
    s.prefs.mark_read_on_open = false;
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    assert!(!read(&s, listed[1]));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // Nothing about the mailbox changes with this click: the list is built again all the same.
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    let mut rest = listed.clone();
    rest.remove(1);
    assert_eq!(ids(&mut s), rest);
}

#[test]
fn after_a_delete_the_message_that_takes_its_place_stays_too() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    run(&mut s, "mail.delete", json!({}));
    // The deleted one leaves at once, read or not; the one below takes the focus.
    assert_eq!(s.sel.focus(), Some(listed[2]));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), [listed[0], listed[2], listed[3]]);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[3]));
}

#[test]
fn opened_in_its_own_folder_it_stays_there() {
    let (mut s, _) = unread_tab();
    let junk = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Junk).unwrap();
    let elsewhere = s.mailbox.messages_in(junk).find(|m| !m.read).map(|m| m.id).unwrap();
    // Selected from another folder (an alert, a conversation), the window goes to its folder.
    run(&mut s, "mail.select", json!({"id": elsewhere}));
    assert_eq!((s.view.folder, s.view.filter), (junk, Filter::Unread));
    assert!(read(&s, elsewhere));
    assert!(ids(&mut s).contains(&elsewhere));
}

#[test]
fn in_conversations_and_arranged_by_categories_it_stays_too() {
    // A conversation of two unread messages: the newer one, opened, stays in its row.
    let (mut s, _) = unread_tab();
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    let pair = s.rows().iter().find(|r| r.members.len() == 2).map(|r| r.members.clone()).unwrap();
    run(&mut s, "mail.markUnread", json!({"ids": pair}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let rows = ids(&mut s);
    let at = rows.iter().position(|id| *id == pair[0]).unwrap();
    assert!(at + 1 < rows.len(), "{rows:?}");
    let members = |s: &mut Session| s.rows().iter().find(|r| r.members.contains(&pair[1])).map(|r| r.members.clone());
    run(&mut s, "mail.select", json!({"id": pair[0]}));
    assert!(read(&s, pair[0]) && !read(&s, pair[1]));
    assert_eq!(ids(&mut s), rows);
    assert_eq!(members(&mut s), Some(pair.clone()));
    // Down: it leaves its row, which the other one keeps.
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(rows[at + 1]));
    assert_eq!(members(&mut s), Some(vec![pair[1]]));

    // Under two categories it is listed twice, and both of its rows stay; Down goes on from the
    // one clicked.
    let (mut s, listed) = unread_tab();
    for (id, names) in [(listed[1], vec!["Red category", "Blue category"]), (listed[2], vec!["Red category"])] {
        s.mailbox.message_mut(id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    s.mailbox_changed();
    run(&mut s, "view.sort", json!({"by": "categories"}));
    let before = ids(&mut s);
    let second = s.rows().iter().position(|r| r.id == listed[1] && r.copy == 1).unwrap();
    run(&mut s, "mail.select", json!({"id": listed[1], "copy": 1}));
    assert!(read(&s, listed[1]));
    assert_eq!(ids(&mut s), before);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(before[second + 1]));
    assert!(!ids(&mut s).contains(&listed[1]));
}

#[test]
fn selected_messages_stay_while_they_are_selected() {
    let (mut s, listed) = unread_tab();
    let (a, b, c, d) = (listed[0], listed[1], listed[2], listed[3]);
    run(&mut s, "mail.select", json!({"id": a}));
    run(&mut s, "mail.select", json!({"id": b, "extend": true}));
    assert!(read(&s, a));
    assert_eq!(ids(&mut s), listed);
    // Marked read together, they stay together.
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // A click on another message alone: both leave.
    run(&mut s, "mail.select", json!({"id": c}));
    assert_eq!(ids(&mut s), [c, d]);
}

#[test]
fn select_all_keeps_them_all_and_a_click_lets_the_others_go() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.selectAll", json!({}));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.select", json!({"id": listed[2]}));
    assert_eq!(ids(&mut s), [listed[2]]);
}

#[test]
fn a_message_taken_out_of_the_selection_leaves() {
    let (mut s, listed) = unread_tab();
    s.prefs.mark_read_on_open = false;
    let (a, b) = (listed[0], listed[1]);
    run(&mut s, "mail.select", json!({"ids": [a, b]}));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // Ctrl+click on a selected one takes it out: nothing about the mailbox changes, and it goes.
    run(&mut s, "mail.select", json!({"id": a, "extend": true}));
    assert_eq!(s.sel.messages, [b]);
    assert_eq!(ids(&mut s), listed[1..]);
}

#[test]
fn a_change_made_outside_a_command_is_followed() {
    let (mut s, listed) = unread_tab();
    let (a, b, c) = (listed[0], listed[1], listed[2]);
    run(&mut s, "mail.select", json!({"ids": [a, b, c]}));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // Set from outside a command (as the window's own tests do): what it no longer has goes at
    // once, from a list built before the session has caught up too, and the next command keeps
    // what it still has.
    s.sel.messages = vec![a, b];
    let built: Vec<Id> = list::rows(&s).iter().map(|r| r.id).collect();
    assert_eq!(built, [a, b, listed[3]]);
    assert_eq!(ids(&mut s), [a, b, listed[3]]);
    run(&mut s, "mail.list", json!({}));
    assert_eq!(ids(&mut s), [a, b, listed[3]]);
    // Built again (the mailbox changed), the list still doesn't keep the one let go.
    run(&mut s, "mail.flag", json!({"ids": [listed[3]]}));
    assert_eq!(ids(&mut s), [a, b, listed[3]]);
    // A search typed in: another list, which keeps nothing of the first.
    s.view.search = "nothing is called this".into();
    assert!(list::rows(&s).is_empty());
    assert!(ids(&mut s).is_empty());
}

#[test]
fn deleting_one_of_two_selected_keeps_the_other() {
    let (mut s, listed) = unread_tab();
    let (a, b) = (listed[0], listed[1]);
    run(&mut s, "mail.select", json!({"ids": [a, b]}));
    run(&mut s, "mail.markRead", json!({}));
    run(&mut s, "mail.delete", json!({"ids": [a]}));
    assert_eq!(s.sel.messages, [b]);
    assert_eq!(ids(&mut s), listed[1..]);
}

#[test]
fn a_range_goes_on_from_its_anchor_and_a_delete_takes_what_is_shown() {
    let (mut s, listed) = unread_tab();
    let (a, b, d) = (listed[0], listed[1], listed[3]);
    run(&mut s, "mail.select", json!({"id": a}));
    run(&mut s, "mail.select", json!({"id": b, "extend": true}));
    // Shift+click from the first one, read since it was opened and still in the list.
    run(&mut s, "mail.select", json!({"id": d, "range": true}));
    let mut picked = s.sel.messages.clone();
    picked.sort();
    let mut all = listed.clone();
    all.sort();
    assert_eq!(picked, all);
    assert_eq!(ids(&mut s), listed);
    // Delete takes exactly what is selected, all of it in the list.
    let answer = run(&mut s, "mail.delete", json!({}));
    assert_eq!(answer["deleted"], json!(listed.len()));
    let deleted = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Deleted).unwrap();
    assert!(listed.iter().all(|id| s.mailbox.message(*id).unwrap().folder == deleted));
    assert!(ids(&mut s).is_empty());
}

#[test]
fn a_message_selected_from_outside_a_command_stays_too() {
    let (mut s, listed) = unread_tab();
    // Set by the host (a search result on the server opened), with the list on screen.
    s.sel.messages = vec![listed[1]];
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);

    // Added beside one kept already, it stays too.
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    assert_eq!(ids(&mut s), listed);
    s.sel.messages = vec![listed[1], listed[2]];
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.markRead", json!({}));
    assert!(read(&s, listed[2]));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn mail_opened_from_its_alert_stays_though_the_list_was_not_shown() {
    let (mut s, _) = unread_tab();
    let inbox = s.view.folder;
    // Unread mail comes while Calendar or File is shown, and the list isn't built for it: its
    // alert goes to the folder and opens it.
    let new = s.mailbox.messages_in(inbox).filter(|m| m.read).max_by_key(|m| m.date).map(|m| m.id).unwrap();
    s.mailbox.message_mut(new).unwrap().read = false;
    s.mailbox_changed();
    run(&mut s, "folder.select", json!({"id": inbox}));
    run(&mut s, "mail.select", json!({"id": new}));
    assert!(read(&s, new));
    let listed = ids(&mut s);
    let at = listed.iter().position(|id| *id == new).unwrap();
    assert!(at + 1 < listed.len(), "{listed:?}");
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[at + 1]));

    // So does mail that comes into a search, opened before the list is shown again.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "is:unread"}));
    let found = ids(&mut s);
    let new = s.mailbox.messages_in(inbox).filter(|m| m.read).max_by_key(|m| m.date).map(|m| m.id).unwrap();
    s.mailbox.message_mut(new).unwrap().read = false;
    s.mailbox_changed();
    run(&mut s, "mail.select", json!({"id": new}));
    assert!(read(&s, new));
    let now_found = ids(&mut s);
    assert!(now_found.contains(&new) && now_found.len() == found.len() + 1, "{now_found:?}");
}

#[test]
fn what_is_kept_stays_when_a_click_comes_after_a_change() {
    let (mut s, listed) = unread_tab();
    let (a, b) = (listed[0], listed[1]);
    run(&mut s, "mail.select", json!({"id": a}));
    // Something changes in the mailbox before the list is shown again (a flag set on the
    // server), then Ctrl+click: the one opened, read, stays with the other.
    s.mailbox.message_mut(listed[3]).unwrap().flag = mailcraft_model::FlagStatus::Flagged;
    s.mailbox_changed();
    run(&mut s, "mail.select", json!({"id": b, "extend": true}));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn a_message_selected_before_the_flagged_tab_was_shown_stays_in_it() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let all = ids(&mut s);
    run(&mut s, "mail.flag", json!({"ids": [all[0], all[1], all[2]]}));
    // Opened in the All tab, then the Flagged tab, where it is listed with the focus.
    run(&mut s, "mail.select", json!({"id": all[1]}));
    run(&mut s, "view.filter", json!({"value": "flagged"}));
    let listed = ids(&mut s);
    let at = listed.iter().position(|id| *id == all[1]).unwrap();
    assert!(at + 1 < listed.len(), "{listed:?}");
    run(&mut s, "mail.clearFlag", json!({}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[at + 1]));
}

#[test]
fn messages_selected_before_the_unread_tab_was_shown_stay_in_it() {
    let (mut s, listed) = unread_tab();
    let (a, b) = (listed[0], listed[1]);
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "mail.select", json!({"ids": [a, b]}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn a_selected_message_that_comes_into_the_list_stays_in_it() {
    let (mut s, listed) = unread_tab();
    let seen = s.mailbox.messages_in(s.view.folder).find(|m| m.read).map(|m| m.id).unwrap();
    // Selected though the Unread tab doesn't list it (from the reading pane, an alert), then
    // marked unread: it comes into the list, and stays when it is marked read again.
    run(&mut s, "mail.select", json!({"id": seen}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.markUnread", json!({}));
    let with = ids(&mut s);
    assert!(with.contains(&seen));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), with);
}

#[test]
fn a_search_box_with_nothing_to_look_for_keeps_the_list() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    // All Mailboxes chosen with nothing typed, and Escape in the empty box: the same list.
    run(&mut s, "view.search", json!({"query": "", "scope": "everywhere"}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "view.clearSearch", json!({}));
    assert_eq!(ids(&mut s), listed);
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[2]));
}

#[test]
fn undo_brings_back_what_was_kept() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    run(&mut s, "mail.delete", json!({}));
    assert!(!ids(&mut s).contains(&listed[1]));
    // Back in the inbox, read and selected: listed where it was.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.sel.focus(), Some(listed[1]));
    assert!(read(&s, listed[1]));
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn a_command_that_fails_leaves_the_list_as_it_was() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"ids": [listed[0], listed[1]]}));
    run(&mut s, "mail.markRead", json!({}));
    assert_eq!(ids(&mut s), listed);
    // One that lets one of them go and looks at the list before it fails: the selection is put
    // back, and so is what the list keeps.
    let fails = CommandSpec::new("test.fails", "Fails", "Test", |s, _| {
        s.sel.messages.truncate(1);
        s.rows();
        Err(CmdError::Failed("no".into()))
    });
    s.registry = std::sync::Arc::new(Registry::new(vec![fails]));
    assert!(s.run("test.fails", &json!({})).is_err());
    assert_eq!(s.sel.messages, [listed[0], listed[1]]);
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn a_message_being_written_saved_first_leaves_the_list_as_it_was() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    let write = |s: &mut Session| {
        run(s, "mail.new", json!({}));
        run(s, "compose.set", json!({"to": "ada@example.org", "subject": "Half", "body": "Half a thought"}));
    };
    let drafts = |s: &Session| s.mailbox.messages.values().filter(|m| m.draft && m.subject == "Half").count();
    // Each of these saves the message being written to Drafts first (#442), a change to the
    // mailbox of its own, so the list is built again: the opened message is still in it.
    for (command, params) in [("mail.new", json!({})), ("compose.close", json!({})), ("mail.reply", json!({}))] {
        write(&mut s);
        let before = drafts(&s);
        run(&mut s, command, params);
        assert_eq!(drafts(&s), before + 1, "{command}: saved to Drafts first");
        assert_eq!(s.sel.focus(), Some(listed[1]), "{command}");
        assert_eq!(ids(&mut s), listed, "{command}");
    }
    // Also when the command then fails: the draft stays saved, and the list as it was.
    write(&mut s);
    let before = drafts(&s);
    assert!(s.run("mail.reply", &json!({"id": 999_999})).is_err());
    assert_eq!(drafts(&s), before + 1);
    assert_eq!(s.sel.focus(), Some(listed[1]));
    assert_eq!(ids(&mut s), listed);
}

// Guards: what isn't kept.

#[test]
fn a_deleted_or_moved_message_leaves_at_once() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    run(&mut s, "mail.delete", json!({}));
    assert!(!ids(&mut s).contains(&listed[1]));
    let archive = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Archive).unwrap();
    run(&mut s, "mail.select", json!({"id": listed[0]}));
    run(&mut s, "mail.move", json!({"folder": archive}));
    assert!(!ids(&mut s).contains(&listed[0]));
    // A search of every folder leaves out Deleted Items.
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.search", json!({"query": "is:unread", "scope": "everywhere"}));
    let found = ids(&mut s);
    run(&mut s, "mail.select", json!({"id": found[0]}));
    assert_eq!(ids(&mut s), found);
    run(&mut s, "mail.delete", json!({}));
    assert!(!ids(&mut s).contains(&found[0]));
}

#[test]
fn only_a_message_that_was_in_the_list_is_kept() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let seen = ids(&mut s).into_iter().find(|id| read(&s, *id)).unwrap();
    let unread = s.mailbox.unread_count(s.view.folder);
    // Read and with the focus, then the Unread tab: not in it, nor counted.
    run(&mut s, "mail.select", json!({"id": seen}));
    let shown = run(&mut s, "view.filter", json!({"value": "unread"}));
    assert_eq!(shown["shown"], json!(unread));
    assert!(!ids(&mut s).contains(&seen));
    // Nor when it is given the focus there (the list as it is built afresh too).
    run(&mut s, "mail.select", json!({"id": seen}));
    assert!(!ids(&mut s).contains(&seen));
    assert!(!list::rows(&s).iter().any(|r| r.id == seen));
    // Nor when the tab changed with no list built for it yet: the one built for All isn't the
    // list it was clicked in.
    run(&mut s, "view.filter", json!({"value": "all"}));
    assert!(ids(&mut s).contains(&seen));
    s.view.filter = Filter::Unread;
    run(&mut s, "mail.select", json!({"id": seen}));
    assert!(!ids(&mut s).contains(&seen));
    // Nor one a search doesn't find.
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.search", json!({"query": "is:unread"}));
    run(&mut s, "mail.select", json!({"id": seen}));
    assert!(!ids(&mut s).contains(&seen));
}

#[test]
fn one_selected_in_another_folder_is_not_kept_here() {
    let (mut s, listed) = unread_tab();
    let inbox = s.view.folder;
    let elsewhere = s.mailbox.messages.values().find(|m| !m.read && m.folder != inbox).map(|m| m.id).unwrap();
    // Unread elsewhere, selected with one of the inbox's (an agent's `ids`), then moved into the
    // inbox and read on the server: it was never in this list.
    run(&mut s, "mail.select", json!({"ids": [elsewhere, listed[0]]}));
    let m = s.mailbox.message_mut(elsewhere).unwrap();
    (m.folder, m.read) = (inbox, true);
    s.mailbox_changed();
    assert_eq!(ids(&mut s), listed);
}

#[test]
fn one_selected_again_after_it_left_is_not_kept() {
    let (mut s, listed) = unread_tab();
    let (a, b) = (listed[0], listed[1]);
    // An agent's clicks, with no look at the list between them: the first, read, left when the
    // second was opened, and isn't brought back by opening it again.
    run(&mut s, "mail.select", json!({"id": a}));
    run(&mut s, "mail.select", json!({"id": b}));
    run(&mut s, "mail.select", json!({"id": a}));
    assert!(!ids(&mut s).contains(&a));
    run(&mut s, "mail.markRead", json!({"ids": [listed[2]]}));
    run(&mut s, "mail.select", json!({"id": listed[2]}));
    assert!(!ids(&mut s).contains(&listed[2]));

    // In a search, by the list built while it was kept: Ctrl+click took it out, so it left.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "is:unread"}));
    let found = ids(&mut s);
    run(&mut s, "mail.select", json!({"id": found[1]}));
    assert_eq!(ids(&mut s), found);
    run(&mut s, "mail.select", json!({"id": found[1], "extend": true}));
    run(&mut s, "mail.select", json!({"id": found[1]}));
    assert!(!ids(&mut s).contains(&found[1]));
}

#[test]
fn another_tab_and_back_starts_the_list_afresh() {
    let (mut s, listed) = unread_tab();
    run(&mut s, "mail.select", json!({"id": listed[1]}));
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    assert_eq!(s.sel.focus(), Some(listed[1]));
    assert!(!ids(&mut s).contains(&listed[1]));
}
