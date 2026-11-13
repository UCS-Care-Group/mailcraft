// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Collapsing the message list's groups (#929): a collapsed group is one row, its heading with how
//! many messages it has. That row is the switch that expands the group again and stands for no
//! message, so no command acts on it; the groups are kept for each folder, Left and Right
//! collapse and expand them, and a search shows every group whole.

use std::collections::BTreeSet;

use mailcraft_model::Id;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::list::Row;
use crate::{Prefs, Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The sample's Inbox with nothing selected, and a group of it with two messages or more.
fn inbox() -> (Session, String) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    s.sel.clear_messages();
    let group = groups(&s.rows()).into_iter().find(|(_, n)| *n >= 2).map(|(g, _)| g).expect("a group of two messages or more");
    (s, group)
}

/// Each group of `rows` with how many messages its rows stand for.
fn groups(rows: &[Row]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for r in rows.iter().filter(|r| r.under.is_none()) {
        if let Some(g) = &r.group {
            out.push((g.clone(), 0));
        }
        if let Some(last) = out.last_mut() {
            last.1 += r.members.len();
        }
    }
    out
}

fn members(rows: &[Row]) -> BTreeSet<Id> {
    rows.iter().filter(|r| r.under.is_none()).flat_map(|r| r.members.iter().copied()).collect()
}

/// A collapsed group is one row: its heading, the number of its messages, and all of them as its
/// members; the other groups and every message of the list are as they were, and expanding gives
/// the same rows back.
#[test]
fn a_collapsed_group_is_one_row_for_all_its_messages() {
    let (mut s, group) = inbox();
    let before = s.rows().to_vec();
    let count = groups(&before).into_iter().find(|(g, _)| *g == group).unwrap().1;
    let answer = run(&mut s, "view.collapseGroup", json!({"group": group}));
    assert_eq!(answer["collapsed"], json!([group]));
    let rows = s.rows().to_vec();
    let folded: Vec<&Row> = rows.iter().filter(|r| r.folded.is_some()).collect();
    assert_eq!(folded.len(), 1);
    assert_eq!((folded[0].group.as_deref(), folded[0].folded, folded[0].members.len()), (Some(group.as_str()), Some(count), count));
    assert_eq!(members(&rows), members(&before), "every message is still listed, in the group's row");
    assert_eq!(groups(&rows), groups(&before), "the groups and their counts stay");
    assert_eq!(crate::list::listed(&rows), crate::list::listed(&before), "the list's count stays");

    run(&mut s, "view.expandGroup", json!({"group": group}));
    assert_eq!(s.rows().to_vec(), before);
    assert!(s.prefs.collapsed_groups.is_empty(), "nothing is kept for a folder with no collapsed group");
}

/// The groups are kept for each folder: another folder shows its own whole, and coming back finds
/// the group collapsed; the choice is in the settings that are saved.
#[test]
fn collapsed_groups_are_kept_for_each_folder() {
    let (mut s, group) = inbox();
    let inbox_id = s.view.folder;
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    run(&mut s, "folder.select", json!({"kind": "sent"}));
    assert!(s.rows().iter().all(|r| r.folded.is_none()));
    run(&mut s, "folder.select", json!({"id": inbox_id}));
    assert!(s.rows().iter().any(|r| r.folded.is_some()));

    let saved = serde_json::to_value(&s.prefs).unwrap();
    assert_eq!(saved["collapsedGroups"][inbox_id.to_string()], json!([group]));
    let back: Prefs = serde_json::from_value(saved).unwrap();
    assert!(back.is_collapsed(inbox_id, &group));
    // Settings from before there was any: nothing is collapsed.
    assert!(serde_json::from_value::<Prefs>(json!({})).unwrap().collapsed_groups.is_empty());
}

/// Left collapses the group the focus is in and the focus goes to the group's row; Down then
/// moves past the whole group; Right on the group's row opens it again.
#[test]
fn left_and_right_collapse_and_expand_the_focused_group() {
    let (mut s, group) = inbox();
    let before = s.rows().to_vec();
    let at = before.iter().position(|r| r.group.as_deref() == Some(group.as_str())).unwrap();
    let inside = before[at + 1].id;
    run(&mut s, "mail.select", json!({"id": inside}));
    let _ = s.rows();
    assert!(s.enabled("view.collapseGroup") && !s.enabled("view.expandGroup"));
    run(&mut s, "view.collapseGroup", json!({}));
    let rows = s.rows().to_vec();
    let head = rows.iter().position(|r| r.folded.is_some()).unwrap();
    assert_eq!(s.sel.focus_in(&rows), Some(head), "the focus is on the group's row");
    assert!(s.enabled("view.expandGroup") && !s.enabled("view.collapseGroup"));

    run(&mut s, "mail.next", json!({}));
    let rows = s.rows().to_vec();
    assert_eq!(s.sel.focus_in(&rows), Some(head + 1), "Down goes to the next group, past the collapsed one");
    run(&mut s, "mail.previous", json!({}));
    run(&mut s, "view.expandGroup", json!({}));
    // (The messages the focus went to are read now: the rows are the same, their counts aren't.)
    let shape = |rows: &[Row]| rows.iter().map(|r| (r.id, r.members.clone(), r.group.clone(), r.folded)).collect::<Vec<_>>();
    assert_eq!(shape(&s.rows()), shape(&before));
}

/// Every group at once, and back.
#[test]
fn every_group_collapses_and_expands_at_once() {
    let (mut s, _) = inbox();
    let before = s.rows().to_vec();
    run(&mut s, "view.collapseAllGroups", json!({}));
    let rows = s.rows().to_vec();
    assert_eq!(rows.len(), groups(&before).len());
    assert!(rows.iter().all(|r| r.folded.is_some()));
    assert_eq!(members(&rows), members(&before));
    run(&mut s, "view.expandAllGroups", json!({}));
    assert_eq!(s.rows().to_vec(), before);
}

/// A search lists every match whole, and so does a list without groups; the choice stays for
/// when the search ends or the groups come back.
#[test]
fn a_search_and_a_list_without_groups_show_everything() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    run(&mut s, "view.search", json!({"query": "the"}));
    assert!(s.rows().iter().all(|r| r.folded.is_none()));
    run(&mut s, "view.clearSearch", json!({}));
    assert!(s.rows().iter().any(|r| r.folded.is_some()));
    run(&mut s, "view.groups", json!({"value": false}));
    assert!(s.rows().iter().all(|r| r.folded.is_none()));
    run(&mut s, "view.groups", json!({"value": true}));
    assert!(s.rows().iter().any(|r| r.folded.is_some()));
}

/// A collapsed group's row is the switch that expands it again and stands for no message, its
/// own included: Delete on it is refused and takes nothing, not the group and not that message.
#[test]
fn delete_on_a_collapsed_group_takes_nothing() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let rows = s.rows().to_vec();
    let head = rows.iter().find(|r| r.folded.is_some()).unwrap().clone();
    let inbox_id = s.view.folder;
    let all: BTreeSet<Id> = members(&rows);
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert!(s.on_collapsed_group(), "the row selected is the group's");
    assert!(s.acting_on().is_empty(), "no message is acted on");
    assert!(s.run("mail.delete", &json!({})).is_err(), "Delete is refused there");
    assert!(all.iter().all(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == inbox_id)), "every message is still in the Inbox");
}

/// What isn't a group's heading is refused and keeps nothing; a settings file that keeps too
/// much, or headings too long, is read within the limits.
#[test]
fn junk_is_refused_and_the_limits_hold() {
    let (mut s, _) = inbox();
    for junk in [json!({"group": ""}), json!({"group": "   "}), json!({"group": 5}), json!({"group": null}), json!({"group": "x".repeat(300)})] {
        assert!(s.run("view.collapseGroup", &junk).is_err(), "{junk}");
    }
    assert!(s.prefs.collapsed_groups.is_empty());
    // With nothing selected and no group named, there is nothing to collapse.
    assert!(s.run("view.collapseGroup", &json!({})).is_err());

    let folder = s.view.folder;
    let many: BTreeSet<String> = (0..500).map(|i| format!("G{i}")).chain(["y".repeat(400)]).collect();
    s.prefs.collapsed_groups.insert(folder, many);
    assert_eq!(s.prefs.collapsed_in(folder).count(), Prefs::COLLAPSED_GROUPS);
    assert!(s.prefs.collapsed_in(folder).all(|g| g.chars().count() <= Prefs::GROUP_CHARS));
    // A folder past the limit of folders can't start a list of its own.
    s.prefs.collapsed_groups.clear();
    for f in 0..Prefs::COLLAPSED_FOLDERS as Id {
        s.prefs.collapsed_groups.insert(1_000_000 + f, BTreeSet::from(["Older".to_string()]));
    }
    assert!(s.run("view.collapseGroup", &json!({"group": "Older"})).is_err());
}

/// Arranged by date with older mail by month and year, Older becomes the months of this year and
/// then the years before; a group of either collapses like any other.
#[test]
fn older_mail_goes_in_a_group_for_each_month_and_year() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    // Three of the Inbox's messages made older, before the list is first built.
    let inbox_id = s.view.folder;
    let mut ids: Vec<Id> = s.mailbox.messages.values().filter(|m| m.folder == inbox_id && !m.pinned).map(|m| m.id).collect();
    ids.sort_unstable();
    let at = |y, m, d| DateTime::new(Date::new(y, m, d).unwrap(), 9, 0, 0).unwrap().to_timestamp(0);
    for (id, when) in ids.iter().zip([at(2026, 7, 2), at(2025, 3, 4), at(2024, 5, 6)]) {
        let m = s.mailbox.message_mut(*id).unwrap();
        (m.date, m.received) = (when, when);
    }
    let headings = |s: &mut Session| s.rows().iter().filter_map(|r| r.group.clone()).collect::<Vec<_>>();
    let one_older = headings(&mut s);
    assert!(one_older.contains(&"Older".to_string()) && !one_older.contains(&"July".to_string()));

    assert_eq!(run(&mut s, "view.olderByMonth", json!({}))["value"], true);
    let split = headings(&mut s);
    assert!(!split.contains(&"Older".to_string()), "{split:?}");
    let tail: Vec<&str> = split.iter().rev().take(3).rev().map(String::as_str).collect();
    assert_eq!(tail, ["July", "2025", "2024"], "the months of this year, then the years, newest first");

    run(&mut s, "view.collapseGroup", json!({"group": "2025"}));
    let rows = s.rows().to_vec();
    assert!(rows.iter().any(|r| r.group.as_deref() == Some("2025") && r.folded == Some(1)));
    assert_eq!(run(&mut s, "view.olderByMonth", json!({"value": false}))["value"], false);
    assert!(headings(&mut s).contains(&"Older".to_string()));
    // Kept with the settings, and off for settings from before.
    s.prefs.older_by_month = true;
    assert_eq!(serde_json::to_value(&s.prefs).unwrap()["olderByMonth"], true);
    assert!(!serde_json::from_value::<Prefs>(json!({})).unwrap().older_by_month);
}

/// In conversation view, Left and Right on a collapsed group's row are the group's, not the
/// conversation's under it; on a conversation in an open group they stay the conversation's.
#[test]
fn in_conversation_view_the_arrows_on_a_collapsed_group_are_the_groups() {
    let (mut s, _) = inbox();
    run(&mut s, "view.conversations", json!({"value": true}));
    let rows = s.rows().to_vec();
    let at = rows.iter().position(|r| r.conversation()).expect("a conversation in the Inbox");
    let conversation = rows[at].id;
    let group = rows[..=at].iter().rev().find_map(|r| r.group.clone()).unwrap();
    run(&mut s, "mail.select", json!({"id": conversation}));
    let _ = s.rows();
    assert!(s.enabled("mail.expandConversation"), "Right opens a closed conversation in an open group");

    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let rows = s.rows().to_vec();
    let head = rows.iter().find(|r| r.folded.is_some()).unwrap().clone();
    assert!(head.members.contains(&conversation));
    run(&mut s, "mail.select", json!({"id": head.id}));
    let _ = s.rows();
    assert!(!s.enabled("mail.expandConversation") && !s.enabled("mail.collapseConversation"));
    assert!(s.enabled("view.expandGroup"));
    // The list's count is of conversations, and stays: the group's row counts its rows.
    let open_count = {
        run(&mut s, "view.expandGroup", json!({"group": group}));
        let n = crate::list::listed(&s.rows());
        run(&mut s, "view.collapseGroup", json!({"group": group}));
        run(&mut s, "mail.select", json!({"id": head.id}));
        n
    };
    assert_eq!(crate::list::listed(&s.rows()), open_count);
    assert!(head.members.len() > head.folded.unwrap(), "a conversation is one row and more messages");
    // The row stands for no message: Delete takes nothing there, the conversation's messages with it.
    assert!(s.acting_on().is_empty(), "a collapsed group's row is the switch that expands it, and nothing more");
}

/// The group the focus is in, and its heading's row.
fn the_group_of(s: &mut Session, id: Id) -> Row {
    s.rows().iter().find(|r| r.folded.is_some() && r.members.contains(&id)).cloned().expect("a collapsed group with the message")
}

/// Collapsing a group takes what was selected in it out of the selection, wherever in the group
/// the focus was: the group's row takes the focus, so the keyboard carries on from there, and
/// Delete acts on what was selected outside the group and on nothing of the group itself.
#[test]
fn collapsing_takes_what_was_selected_in_the_group_out_of_the_selection() {
    let (mut s, group) = inbox();
    let rows = s.rows().to_vec();
    let at = rows.iter().position(|r| r.group.as_deref() == Some(group.as_str())).unwrap();
    let (second, outside) = (rows[at + 1].id, rows.iter().find(|r| r.group.is_some() && r.group.as_deref() != Some(group.as_str())).unwrap().id);
    run(&mut s, "mail.select", json!({"id": outside}));
    run(&mut s, "mail.select", json!({"id": second, "extend": true}));
    run(&mut s, "view.collapseGroup", json!({}));
    let head = the_group_of(&mut s, second);
    assert_eq!(s.sel.messages, vec![outside, head.id]);
    assert_eq!(s.sel.focus(), Some(head.id));
    let rows = s.rows();
    assert!(s.drawn(&rows).shows(&head), "the group's row is drawn selected");
    assert_eq!(s.acting_on(), vec![outside], "the message outside the group, and nothing of the group");
    assert_eq!(s.reading(), None, "the reading pane shows no message for it");
}

/// A message selected in a search that then ended, and whose group is collapsed, is the group's
/// row now: it is drawn selected as that row, and no command acts on it until its group is open
/// again.
#[test]
fn a_message_on_top_of_a_collapsed_group_is_the_rows_switch() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let subject = s.mailbox.message(head.id).unwrap().subject.clone();
    let word = subject.split(|c: char| !c.is_alphanumeric()).max_by_key(|w| w.len()).unwrap().to_string();
    run(&mut s, "view.search", json!({"query": word}));
    run(&mut s, "mail.select", json!({"id": head.id}));
    run(&mut s, "view.clearSearch", json!({}));
    let rows = s.rows();
    let row = rows.iter().find(|r| r.id == head.id).unwrap();
    assert!(row.folded.is_some(), "the group is collapsed again");
    assert!(s.drawn(&rows).shows(row), "the row it is on is drawn selected");
    assert!(s.on_collapsed_group());
    assert!(s.acting_on().is_empty(), "Delete takes neither the message nor the group");
}

/// A collapsed group's row shows no message in the reading pane and marks none read.
#[test]
fn a_collapsed_groups_row_marks_nothing_read() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&head.id).unwrap()).read = false;
    s.prefs.mark_read_on_open = true;
    let unread = |s: &Session| !s.mailbox.message(head.id).unwrap().read;
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert!(s.on_collapsed_group());
    assert_eq!(s.reading(), None, "the reading pane shows none of the group");
    assert!(unread(&s), "the row's message isn't marked read");
    // Down and back up land on it the same way.
    run(&mut s, "mail.next", json!({}));
    run(&mut s, "mail.previous", json!({}));
    assert!(s.on_collapsed_group());
    assert!(unread(&s));
    // Selected by itself (the group open), it is read as any message is.
    run(&mut s, "view.expandGroup", json!({"group": group}));
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert!(!unread(&s));
}

/// A list arranged by category doesn't collapse: a message is under each of its categories.
#[test]
fn a_list_arranged_by_category_does_not_collapse() {
    let (mut s, _) = inbox();
    run(&mut s, "view.sort", json!({"by": "categories"}));
    let group = s.rows().iter().find_map(|r| r.group.clone()).unwrap();
    assert!(s.run("view.collapseGroup", &json!({"group": group})).is_err());
    assert!(!s.enabled("view.collapseAllGroups"));
    assert!(s.prefs.collapsed_groups.is_empty());
    assert!(s.rows().iter().all(|r| r.folded.is_none()));
}

/// Collapse All Groups is there while the list shows groups that collapse and nothing is
/// searched for; Expand All Groups while something is collapsed. A group past the limit is
/// refused, and nothing is collapsed then.
#[test]
fn the_commands_for_all_groups_and_the_limit() {
    let (mut s, group) = inbox();
    assert!(s.enabled("view.collapseAllGroups") && !s.enabled("view.expandAllGroups"));
    run(&mut s, "view.search", json!({"query": "the"}));
    assert!(!s.enabled("view.collapseAllGroups"));
    run(&mut s, "view.clearSearch", json!({}));
    run(&mut s, "view.groups", json!({"value": false}));
    assert!(!s.enabled("view.collapseAllGroups"));
    run(&mut s, "view.groups", json!({"value": true}));

    let folder = s.view.folder;
    let full: BTreeSet<String> = (0..Prefs::COLLAPSED_GROUPS).map(|i| format!("G{i}")).collect();
    s.prefs.collapsed_groups.insert(folder, full.clone());
    assert!(s.enabled("view.expandAllGroups"));
    assert!(s.run("view.collapseGroup", &json!({"group": group})).is_err());
    assert!(s.run("view.collapseAllGroups", &json!({})).is_err());
    assert_eq!(s.prefs.collapsed_groups[&folder], full, "nothing more is kept");
    // One already kept is no new one.
    assert!(s.run("view.collapseGroup", &json!({"group": "G0"})).is_ok());
}

/// `mail.list` names a collapsed group's messages as the group's, not as a conversation.
#[test]
fn mail_list_says_what_a_collapsed_group_holds() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let listed = run(&mut s, "mail.list", json!({}));
    let item = listed["messages"].as_array().or(listed.as_array()).unwrap().iter().find(|m| m["id"] == json!(head.id)).unwrap().clone();
    assert_eq!(item["collapsed"], json!(head.members));
    assert!(item.get("conversation").is_none());
}

/// In conversation view, a conversation picked by its row in a group that collapses leaves the
/// selection with the rest of the group, the focus being elsewhere: Delete takes what stayed
/// selected outside the group and nothing that the group now hides.
#[test]
fn a_conversation_in_a_group_that_collapses_leaves_the_selection() {
    let (mut s, _) = inbox();
    run(&mut s, "view.conversations", json!({"value": true}));
    let rows = s.rows().to_vec();
    let at = rows.iter().position(|r| r.conversation()).expect("a conversation in the Inbox");
    let conversation = rows[at].clone();
    let group = rows[..=at].iter().rev().find_map(|r| r.group.clone()).unwrap();
    let outside = rows.iter().find(|r| r.under.is_none() && r.group.is_some() && r.group != Some(group.clone())).unwrap().id;
    run(&mut s, "mail.select", json!({"id": conversation.id}));
    run(&mut s, "mail.select", json!({"id": outside, "extend": true}));
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = the_group_of(&mut s, conversation.id);
    assert!(head.members.len() > conversation.members.len(), "the group holds more than the conversation");
    assert_eq!(s.acting_on(), vec![outside], "the message outside the group alone");
    let hidden: Vec<Id> = conversation.members.iter().copied().filter(|m| *m != head.id).collect();
    assert!(!hidden.is_empty() && hidden.iter().all(|m| !s.sel.messages.contains(m)), "nothing the group hides stays selected");
}

/// Delete on the last message above a collapsed group doesn't select the group (the next Delete
/// would take all of it): the selection is let go of there, and the group is all still here.
#[test]
fn deleting_up_to_a_collapsed_group_stops_before_it() {
    let (mut s, _) = inbox();
    let rows = s.rows().to_vec();
    // A group whose heading has a message row right above it.
    let at = rows.iter().enumerate().skip(1).find(|(i, r)| r.group.is_some() && rows[i - 1].under.is_none()).map(|(i, _)| i).unwrap();
    let (above, group) = (rows[at - 1].id, rows[at].group.clone().unwrap());
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    run(&mut s, "mail.select", json!({"id": above}));
    run(&mut s, "mail.delete", json!({}));
    assert!(s.sel.messages.is_empty(), "{:?}", s.sel);
    assert!(s.run("mail.delete", &json!({})).is_err(), "nothing is selected to delete");
    let inbox = s.view.folder;
    assert!(head.members.iter().all(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == inbox)));
}

/// Collapsing one group leaves alone a group collapsed before: the row selected there is still
/// that row, a switch that no command acts on, and collapsing elsewhere changes nothing about it.
#[test]
fn collapsing_another_group_leaves_the_first_as_it_is() {
    let (mut s, _) = inbox();
    let names: Vec<String> = groups(&s.rows()).into_iter().filter(|(_, n)| *n >= 2).map(|(g, _)| g).collect();
    let (first, second) = (names[0].clone(), names.iter().find(|g| **g != names[0]).cloned().unwrap_or_else(|| "Older".into()));
    run(&mut s, "view.collapseGroup", json!({"group": first}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let subject = s.mailbox.message(head.id).unwrap().subject.clone();
    let word = subject.split(|c: char| !c.is_alphanumeric()).max_by_key(|w| w.len()).unwrap().to_string();
    run(&mut s, "view.search", json!({"query": word}));
    run(&mut s, "mail.select", json!({"id": head.id}));
    run(&mut s, "view.clearSearch", json!({}));
    run(&mut s, "view.collapseGroup", json!({"group": second}));
    assert_eq!(s.sel.messages, vec![head.id], "the row selected stays the one it was");
    assert!(s.on_collapsed_group());
    assert!(s.acting_on().is_empty());
}

/// A message opened from elsewhere (a task, a contact, an alert) is shown: its group expands,
/// so that nothing is selected out of sight, and the message is itself.
#[test]
fn a_message_opened_from_elsewhere_expands_its_group() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let inside = *head.members.iter().find(|m| **m != head.id).expect("a group of more than one");
    run(&mut s, "mail.select", json!({"id": inside, "elsewhere": true}));
    assert!(s.rows().iter().all(|r| r.folded.is_none()), "the group is open again");
    assert!(!s.on_collapsed_group());
    assert_eq!(s.acting_on(), vec![inside]);
    assert_eq!(s.reading(), Some(inside), "the reading pane shows it");

    // The group's first message is a message like any other when it is asked for from elsewhere.
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    run(&mut s, "mail.select", json!({"id": head.id, "elsewhere": true}));
    assert!(s.rows().iter().all(|r| r.folded.is_none()), "the group is open again");
    assert_eq!(s.acting_on(), vec![head.id]);
}

/// Arranged by subject (or size), the rows after Pinned have no heading: collapsing Pinned takes
/// the pinned messages and no other, and the list's count stays.
#[test]
fn collapsing_pinned_takes_only_what_is_pinned() {
    let (mut s, _) = inbox();
    run(&mut s, "view.sort", json!({"by": "subject"}));
    let before = s.rows().to_vec();
    assert_eq!(before.first().and_then(|r| r.group.as_deref()), Some("Pinned"), "the sample's Inbox has a pinned message");
    let pinned: BTreeSet<Id> = s.mailbox.messages.values().filter(|m| m.pinned && m.folder == s.view.folder).map(|m| m.id).collect();
    run(&mut s, "view.collapseGroup", json!({"group": "Pinned"}));
    let rows = s.rows().to_vec();
    let head = rows.iter().find(|r| r.folded.is_some()).cloned().unwrap();
    assert_eq!(head.members.iter().copied().collect::<BTreeSet<_>>(), pinned);
    assert_eq!(rows.iter().filter(|r| r.folded.is_none()).count(), before.len() - pinned.len(), "every other row is listed");
    assert_eq!(crate::list::listed(&rows), crate::list::listed(&before));
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert!(s.acting_on().is_empty(), "the row is the switch that expands Pinned, and stands for no message");
}

/// Select All takes the rows the list shows (#929): a collapsed group is one of them, the switch
/// that expands it, so none of the messages it hides joins the selection and nothing is selected
/// out of sight.
#[test]
fn select_all_leaves_a_collapsed_groups_messages_out() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let rows = s.rows().to_vec();
    let head = rows.iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let hidden: BTreeSet<Id> = head.members.iter().copied().collect();
    assert!(hidden.len() > 1, "the group holds more than the row it is now");
    let shown: BTreeSet<Id> = rows.iter().filter(|r| r.under.is_none() && r.folded.is_none()).map(|r| r.id).collect();
    for from in ["nothing selected", "the group's own row focused"] {
        if from.starts_with("the group") {
            run(&mut s, "mail.select", json!({"id": head.id}));
            assert_eq!(s.sel.focus(), Some(head.id));
        }
        run(&mut s, "mail.selectAll", json!({}));
        let selected: BTreeSet<Id> = s.sel.messages.iter().copied().collect();
        assert!(selected.is_disjoint(&hidden), "{from}: nothing of the collapsed group is selected: {selected:?}");
        assert_eq!(selected, shown, "{from}: every other row of the list, and only those");
        assert!(s.acting_on().iter().all(|id| !hidden.contains(id)), "{from}: and Delete takes none of the group");
    }
}

/// A message that leaves the selection because its group collapses is acted on by nothing: the
/// reading pane shows none of the collapsed group, Delete is refused, and the commands come back
/// once the group is open and the message selected again.
#[test]
fn nothing_acts_on_a_message_its_group_took_out_of_the_selection() {
    let (mut s, group) = inbox();
    let rows = s.rows().to_vec();
    let at = rows.iter().position(|r| r.group.as_deref() == Some(group.as_str())).unwrap();
    let inside = rows[at + 1].id;
    run(&mut s, "mail.select", json!({"id": inside}));
    assert_eq!(s.reading(), Some(inside), "it is in the reading pane to begin with");

    run(&mut s, "view.collapseGroup", json!({"group": group}));
    assert!(!s.sel.messages.contains(&inside), "it left the selection: {:?}", s.sel.messages);
    assert!(s.acting_on().is_empty(), "no command acts on it");
    assert_eq!(s.reading(), None, "and the reading pane shows no message of the collapsed group");
    assert!(s.run("mail.delete", &json!({})).is_err(), "Delete is refused");
    assert!(s.mailbox.message(inside).is_some_and(|m| m.folder == s.view.folder), "it is still where it was");

    run(&mut s, "view.expandGroup", json!({"group": group}));
    run(&mut s, "mail.select", json!({"id": inside}));
    assert_eq!(s.acting_on(), vec![inside], "selected again, it is acted on as any message is");
}

/// What a collapsed group hides, if anything of it is selected: nothing, ever (#929).
fn hidden_selection(s: &mut Session) -> Vec<Id> {
    let rows = s.rows().to_vec();
    let selected = s.sel.messages.clone();
    selected.into_iter().filter(|id| rows.iter().any(|r| r.folded.is_some() && r.id != *id && r.members.contains(id))).collect()
}

/// A group of the Inbox with three messages or more, and a message of it that isn't its first
/// row: the one a fold hides when the group collapses.
fn a_group_and_a_message_in_it(s: &mut Session) -> (String, Id) {
    let rows = s.rows().to_vec();
    let at = rows
        .iter()
        .position(|r| r.group.is_some() && rows.iter().skip_while(|x| !std::ptr::eq(*x, r)).skip(1).take_while(|x| x.group.is_none()).count() >= 2)
        .expect("a group of three rows or more");
    let group = rows[at].group.clone().unwrap();
    (group, rows[at + 1].id)
}

/// Nothing a collapsed group hides stays selected, however the fold came to hide it (#929).
/// Collapsing is only one of the ways: the groups turned off and on again, another arrangement
/// and back, Older Mail by Month and Year, or a search that ends all put a message out of sight
/// without anyone collapsing anything, and each is a way to have Delete take a message the list
/// doesn't show.
#[test]
fn nothing_a_fold_comes_to_hide_stays_selected() {
    // The groups turned off, a message of the collapsed group picked, and the groups back on.
    let (mut s, _) = inbox();
    let (group, inside) = a_group_and_a_message_in_it(&mut s);
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    run(&mut s, "view.groups", json!({"value": false}));
    run(&mut s, "mail.select", json!({"id": inside}));
    run(&mut s, "view.groups", json!({"value": true}));
    assert!(hidden_selection(&mut s).is_empty(), "the groups off and on again");
    assert!(s.acting_on().iter().all(|id| *id != inside), "and no command takes it");

    // Another arrangement and back: the groups are kept for the folder, not for the arrangement.
    let (mut s, _) = inbox();
    let (group, inside) = a_group_and_a_message_in_it(&mut s);
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    run(&mut s, "view.sort", json!({"by": "from"}));
    run(&mut s, "mail.select", json!({"id": inside}));
    run(&mut s, "view.sort", json!({"by": "date"}));
    assert!(hidden_selection(&mut s).is_empty(), "another arrangement and back");

    // A search shows every group whole; ending it folds them again.
    let (mut s, _) = inbox();
    let (group, inside) = a_group_and_a_message_in_it(&mut s);
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let subject = s.mailbox.message(inside).unwrap().subject.clone();
    let word = subject.split(|c: char| !c.is_alphanumeric()).max_by_key(|w| w.len()).unwrap().to_string();
    run(&mut s, "view.search", json!({"query": word}));
    run(&mut s, "mail.select", json!({"id": inside}));
    run(&mut s, "view.clearSearch", json!({}));
    assert!(hidden_selection(&mut s).is_empty(), "a search that ends");

    // Undo: the message comes back selected, and its group has been collapsed meanwhile.
    let (mut s, _) = inbox();
    let (group, inside) = a_group_and_a_message_in_it(&mut s);
    run(&mut s, "mail.select", json!({"id": inside}));
    run(&mut s, "mail.delete", json!({}));
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let _ = s.run("edit.undo", &json!({}));
    assert!(hidden_selection(&mut s).is_empty(), "an undo into a collapsed group");
}

/// Mail arriving in a collapsed group takes its row, and the message that was the row is one of
/// the hidden ones from then on (#929): without the rule it would stay selected with nothing on
/// screen to show it, and the next Delete would take it.
#[test]
fn mail_arriving_in_a_collapsed_group_takes_nothing_with_it() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert_eq!(s.sel.messages, vec![head.id], "the group's row is what is selected");

    let folder = s.mailbox.message(head.id).unwrap().folder;
    let account = s.mailbox.message(head.id).unwrap().account;
    let at = s.mailbox.message(head.id).unwrap().received;
    let newer = mailcraft_model::Message { folder, account, received: at + 1, subject: "Newer".into(), ..Default::default() };
    let newer = s.mailbox.add_message(newer).unwrap();
    s.mailbox_changed();

    let now = s.rows().iter().find(|r| r.folded.is_some()).cloned().expect("the group is still collapsed");
    assert_eq!(now.id, newer, "the new message is the group's row");
    assert!(hidden_selection(&mut s).is_empty(), "and nothing it hides is selected: {:?}", s.sel.messages);
    assert!(s.acting_on().iter().all(|id| *id != head.id), "no command takes the message that was the row");
}

/// No command acts on a collapsed group's row, not only Delete (#929): what reads the selection
/// one message at a time (Mark Read, Flag, Categorize, Pin, Copy to Folder) and what reads the
/// focus (Reply, Forward) all stand on the same ground, so the row marks nothing read and
/// answers nothing.
#[test]
fn no_command_acts_on_a_collapsed_groups_row() {
    let (mut s, group) = inbox();
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    let head = s.rows().iter().find(|r| r.folded.is_some()).cloned().unwrap();
    let theirs: BTreeSet<Id> = head.members.iter().copied().collect();
    std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&head.id).unwrap()).read = false;
    run(&mut s, "mail.select", json!({"id": head.id}));
    assert!(s.on_collapsed_group());

    // Those that take the selection, and those that take the one message in the reading pane:
    // the row stands for neither, so each of them has nothing to work on.
    let commands = [
        "mail.delete",
        "mail.markRead",
        "mail.markUnread",
        "mail.toggleFlag",
        "mail.pin",
        "mail.reply",
        "mail.replyAll",
        "mail.forward",
        "mail.get",
        "mail.viewSource",
        "mail.findRelated",
        "mail.findFromSender",
        "mail.saveAllAttachments",
        "mail.digitalSignature",
        "tasks.fromMessage",
        "rules.fromMessage",
        "people.fromMessage",
    ];
    let inbox_id = s.view.folder;
    for id in commands {
        // (The few that want a target of their own get it, so that what refuses them is the row.)
        let params = match id {
            "rules.fromMessage" => json!({"folder": inbox_id}),
            _ => json!({}),
        };
        let e = s.run(id, &params).expect_err(id).to_string();
        assert!(e.contains("select a message"), "{id}: {e}");
    }
    assert!(!s.mailbox.message(head.id).unwrap().read, "the row's message is still unread");
    assert!(s.compose.is_none(), "and nothing is being written");
    assert!(theirs.iter().all(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == inbox_id)), "every message of the group is where it was");
}

/// Arranged by subject or by size the rows after Pinned carry no heading of their own, and the
/// heading above them is Pinned's (#929). Left on one of them must not collapse the pinned
/// messages: the group a message is in is the message's own, not whichever heading stands above
/// its row.
#[test]
fn a_message_after_pinned_is_in_no_group() {
    let (mut s, _) = inbox();
    run(&mut s, "view.sort", json!({"by": "subject"}));
    let rows = s.rows().to_vec();
    assert_eq!(rows.first().and_then(|r| r.group.as_deref()), Some("Pinned"), "the sample's Inbox has a pinned message");
    let pinned: BTreeSet<Id> = s.mailbox.messages.values().filter(|m| m.pinned && m.folder == s.view.folder).map(|m| m.id).collect();
    let after = rows.iter().find(|r| r.under.is_none() && !pinned.contains(&r.id)).map(|r| r.id).expect("a row after the pinned ones");

    run(&mut s, "mail.select", json!({"id": after}));
    assert!(s.run("view.collapseGroup", &json!({})).is_err(), "the message selected is in no group");
    assert!(s.rows().iter().all(|r| r.folded.is_none()), "and nothing is collapsed: {:?}", s.prefs.collapsed_groups);
}

/// A heading too long to be kept is refused wherever it came from (#929): stored, it would be
/// skipped when the groups are read back and the group would stay open, while still taking one
/// of the places the folder has.
#[test]
fn a_heading_too_long_is_refused_from_collapse_all_too() {
    let (mut s, _) = inbox();
    let long = "N".repeat(crate::Prefs::GROUP_CHARS + 1);
    let folder = s.view.folder;
    let account = s.mailbox.folder(folder).unwrap().account;
    let from = mailcraft_model::Address::new(long.clone(), "long@example.org");
    let m = mailcraft_model::Message { folder, account, from, subject: "Long".into(), ..Default::default() };
    s.mailbox.add_message(m).unwrap();
    s.mailbox_changed();
    run(&mut s, "view.sort", json!({"by": "from"}));
    assert!(s.rows().iter().any(|r| r.group.as_deref() == Some(long.as_str())), "the long name is a heading of the list");

    assert!(s.run("view.collapseAllGroups", &json!({})).is_err(), "Collapse All is refused rather than storing it");
    assert!(s.prefs.collapsed_groups.is_empty(), "and nothing is kept: {:?}", s.prefs.collapsed_groups);
    assert!(s.run("view.collapseGroup", &json!({"group": long})).is_err(), "and so is naming it");
}

/// A list arranged by categories never folds, whatever is kept for the folder (#929): a message
/// is listed under each of its categories, so folding one heading there would take rows out of
/// sight that belong under another. A group collapsed under a different arrangement whose name a
/// category happens to share is the case that would do it.
#[test]
fn a_category_whose_name_a_collapsed_group_shares_does_not_fold() {
    let (mut s, _) = inbox();
    run(&mut s, "view.sort", json!({"by": "from"}));
    let rows = s.rows().to_vec();
    let group = rows.iter().find_map(|r| r.group.clone()).expect("a sender's group");
    // A message under another heading, so that collapsing the first doesn't take it with it.
    let other = rows
        .iter()
        .skip_while(|r| r.group.as_ref() != Some(&group))
        .skip(1)
        .find(|r| r.under.is_none() && r.group.is_some())
        .map(|r| r.id)
        .expect("a second group");
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    assert!(s.rows().iter().any(|r| r.folded.is_some()), "it folds under From");

    // That name as a category of a message the fold doesn't hide.
    run(&mut s, "mail.select", json!({"id": other}));
    run(&mut s, "mail.categorize", json!({"category": group.clone(), "value": true}));
    run(&mut s, "view.sort", json!({"by": "categories"}));
    assert!(
        s.rows().iter().any(|r| r.group.as_deref() == Some(group.as_str())),
        "the category is a heading there: {:?}",
        s.rows().iter().filter_map(|r| r.group.clone()).collect::<Vec<_>>()
    );
    assert!(s.rows().iter().all(|r| r.folded.is_none()), "and nothing folds under categories");
}

/// A message listed under an open conversation is shown under that conversation's heading, not
/// under its own date's (#503), so Left on it collapses the group the owner sees it in (#929).
/// Without that, a reply from last month under a conversation in Today would collapse Last Month
/// and the row the focus is on would stay where it is.
#[test]
fn left_under_an_open_conversation_collapses_the_conversations_group() {
    let (mut s, _) = inbox();
    run(&mut s, "view.conversations", json!({"value": true}));
    let rows = s.rows().to_vec();
    let at = rows.iter().position(|r| r.conversation()).expect("a conversation in the Inbox");
    let (head, group) = (rows[at].id, rows[..=at].iter().rev().find_map(|r| r.group.clone()).unwrap());

    // A message of it from another date group, so that its own heading would be a different one.
    let older = *rows[at].members.iter().find(|m| **m != head).expect("a conversation of more than one");
    let day = 24 * 60 * 60;
    let moved = s.mailbox.message(head).unwrap().received - 40 * day;
    std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&older).unwrap()).received = moved;
    s.mailbox_changed();
    assert_ne!(crate::list::heading_of(&s, older), Some(group.clone()), "its own date is another group's");

    run(&mut s, "mail.select", json!({"id": head}));
    run(&mut s, "mail.expandConversation", json!({}));
    run(&mut s, "mail.select", json!({"id": older}));
    assert!(s.rows().iter().any(|r| r.id == older && r.under == Some(head)), "it is listed under the conversation");

    run(&mut s, "view.collapseGroup", json!({}));
    assert!(s.prefs.is_collapsed(s.view.folder, &group), "the conversation's group collapsed: {:?}", s.prefs.collapsed_groups);
}

/// Naming the `group` makes `view.collapseGroup` run while the menu has it greyed, because a
/// command with a target of its own isn't held back by the selection. It has to refuse the same
/// states the menu refuses in (#929): a heading kept for a list that doesn't fold would wait
/// there for the day the list folds again, and an agent would collapse a group the owner can't
/// see collapse.
#[test]
fn naming_the_group_does_not_get_past_the_states_that_refuse_it() {
    let (mut s, group) = inbox();

    // While something is searched for, every group shows whole.
    run(&mut s, "view.search", json!({"query": "is:unread"}));
    assert!(s.run("view.collapseGroup", &json!({"group": group.clone()})).is_err(), "during a search");
    assert!(s.run("view.collapseAllGroups", &json!({})).is_err());
    run(&mut s, "view.clearSearch", json!({}));

    // With the groups turned off there are no headings to collapse.
    run(&mut s, "view.groups", json!({"value": false}));
    assert!(s.run("view.collapseGroup", &json!({"group": group.clone()})).is_err(), "with the groups off");
    run(&mut s, "view.groups", json!({"value": true}));

    // Arranged by categories a message is listed under each of its categories.
    run(&mut s, "view.sort", json!({"by": "categories"}));
    assert!(s.run("view.collapseGroup", &json!({"group": group.clone()})).is_err(), "under categories");
    run(&mut s, "view.sort", json!({"by": "date"}));

    assert!(s.prefs.collapsed_groups.is_empty(), "and none of them kept anything: {:?}", s.prefs.collapsed_groups);
    // It still works where the menu offers it.
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    assert!(s.rows().iter().any(|r| r.folded.is_some()));
}

/// The groups kept for a folder are let go of where the folder is gone for good (#929), as the
/// working hours of a removed account are. Not before: a deleted folder comes back with an undo
/// while the preferences do not, so letting go of them at the delete would lose them for a step
/// the owner can take back. And another mailbox file gives the same ids to folders of its own,
/// so they go with the mailbox they belong to.
#[test]
fn the_groups_of_a_folder_that_is_gone_are_let_go_of() {
    let (mut s, group) = inbox();
    let inbox_id = s.view.folder;
    let account = s.mailbox.folder(inbox_id).unwrap().account;
    let other = s.mailbox.add_folder(account, None, "Projects").unwrap();
    s.mailbox_changed();
    s.prefs.collapsed_groups.entry(other).or_default().insert("Today".into());
    run(&mut s, "view.collapseGroup", json!({"group": group}));
    assert_eq!(s.prefs.collapsed_groups.len(), 2, "both folders keep something");

    // Deleted, and deleted again out of Deleted Items: an undo still brings it back, so the
    // groups stay until something says they can't.
    s.mailbox.delete_folder(other).expect("into Deleted Items");
    s.mailbox.delete_folder(other).expect("and gone");
    s.mailbox_changed();
    assert!(s.prefs.collapsed_groups.contains_key(&other), "kept while an undo could bring the folder back");

    s.forget_gone_groups();
    assert!(!s.prefs.collapsed_groups.contains_key(&other), "and let go of once nothing can: {:?}", s.prefs.collapsed_groups);
    assert!(s.prefs.collapsed_groups.contains_key(&inbox_id), "the Inbox keeps its own");

    // Reading the same file again is not another file: Save reads its own file back that way,
    // and the groups are the owner's, not the file's.
    let dir = std::env::temp_dir().join(format!("mailcraft-929-groups-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mine, theirs) = (dir.join("mine.mailcraft"), dir.join("theirs.mailcraft"));
    run(&mut s, "file.save", json!({"path": mine}));
    run(&mut s, "file.save", json!({"path": theirs}));
    s.run("file.save", &json!({"path": mine})).unwrap();
    run(&mut s, "file.open", json!({"path": mine}));
    assert!(s.prefs.collapsed_groups.contains_key(&inbox_id), "the same file keeps them: {:?}", s.prefs.collapsed_groups);

    // Another file gives those ids to folders of its own.
    run(&mut s, "file.open", json!({"path": theirs}));
    assert!(s.prefs.collapsed_groups.is_empty(), "another file carries none of them: {:?}", s.prefs.collapsed_groups);
    let _ = std::fs::remove_dir_all(&dir);
}
