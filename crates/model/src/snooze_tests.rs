// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949) in the mailbox: where a message waits, where it goes back to, and what repair
//! drops.
use crate::{FolderKind, Id, Mailbox, Message, Snooze};

const T0: i64 = 1_800_000_000;

fn world() -> (Mailbox, Id, Id, Id) {
    let mut mb = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id =
        mb.add_message(Message { account, folder: inbox, subject: "Lunch".into(), read: true, received: T0 - 100, ..Default::default() }).unwrap();
    (mb, account, inbox, id)
}

#[test]
fn snoozing_makes_the_folder_once_and_remembers_where_the_message_came_from() {
    let (mut mb, account, inbox, id) = world();
    assert_eq!(mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap(), 1);
    let folder = mb.snoozed_folder(account).unwrap();
    let m = mb.message(id).unwrap();
    assert_eq!(m.folder, folder);
    assert_eq!(m.snooze, Some(Snooze { until: T0 + 60, from: inbox, returned: None }));
    // A second message, and the same folder; the name in another language makes no second one.
    let other = mb.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    mb.snooze_messages(&[other], T0 + 90, "Zurückgestellt").unwrap();
    assert_eq!(mb.message(other).unwrap().folder, folder);
    assert_eq!(mb.folders.iter().filter(|f| f.account == account && f.parent.is_none() && f.kind == FolderKind::Custom).count(), 1);
}

#[test]
fn a_folder_with_that_name_already_there_is_taken_for_it() {
    let (mut mb, account, _, id) = world();
    let made = mb.add_folder(account, None, "snoozed").unwrap();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    assert_eq!(mb.snoozed_folder(account), Some(made));
    assert_eq!(mb.folders.iter().filter(|f| f.name.eq_ignore_ascii_case("snoozed")).count(), 1);
}

#[test]
fn snoozing_again_changes_the_time_and_keeps_the_first_source() {
    let (mut mb, _, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    mb.snooze_messages(&[id], T0 + 600, "Snoozed").unwrap();
    assert_eq!(mb.message(id).unwrap().snooze, Some(Snooze { until: T0 + 600, from: inbox, returned: None }));
}

#[test]
fn drafts_and_messages_in_the_outbox_are_refused_and_nothing_is_moved() {
    let (mut mb, account, inbox, id) = world();
    let draft = mb.add_message(Message { account, folder: inbox, draft: true, ..Default::default() }).unwrap();
    assert!(mb.snooze_messages(&[id, draft], T0 + 60, "Snoozed").is_err());
    assert_eq!(mb.message(id).unwrap().folder, inbox);
    assert_eq!(mb.snoozed_folder(account), None);
}

#[test]
fn due_messages_come_back_unread_marked_with_the_received_date_as_it_was() {
    let (mut mb, account, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    assert!(mb.due_snoozes(T0 + 59).is_empty());
    assert_eq!(mb.due_snoozes(T0 + 60), vec![id]);
    assert_eq!(mb.return_snoozed(&[id], T0 + 3_600), vec![id]);
    let m = mb.message(id).unwrap();
    assert_eq!((m.folder, m.read, m.received), (inbox, false, T0 - 100));
    assert_eq!(m.snooze, Some(Snooze { until: T0 + 60, from: inbox, returned: Some(T0 + 3_600) }));
    assert!(mb.due_snoozes(T0 + 9_999).is_empty());
    assert!(!mb.has_snoozed());
    // It isn't returned twice.
    assert!(mb.return_snoozed(&[id], T0 + 4_000).is_empty());
    assert!(mb.snoozed_folder(account).is_some());
}

#[test]
fn a_message_goes_back_to_the_inbox_when_the_folder_it_came_from_is_gone() {
    let (mut mb, account, inbox, _) = world();
    let projects = mb.add_folder(account, None, "Projects").unwrap();
    let id = mb.add_message(Message { account, folder: projects, ..Default::default() }).unwrap();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    mb.folders.retain(|f| f.id != projects);
    assert_eq!(mb.return_snoozed(&[id], T0 + 61), vec![id]);
    assert_eq!(mb.message(id).unwrap().folder, inbox);
}

#[test]
fn a_message_moved_out_by_hand_has_its_snooze_called_off() {
    let (mut mb, _, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    mb.move_messages(&[id], inbox).unwrap();
    assert_eq!(mb.call_off_strays(), 1);
    assert_eq!(mb.message(id).unwrap().snooze, None);
    assert!(mb.due_snoozes(T0 + 999).is_empty());
}

#[test]
fn repair_forgets_a_missing_folder_and_drops_snoozes_of_mail_outside_it_but_keeps_returned_marks() {
    let (mut mb, account, inbox, id) = world();
    let back = mb.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    mb.message_mut(back).unwrap().snooze = Some(Snooze { until: T0, from: inbox, returned: Some(T0 + 5) });
    mb.message_mut(id).unwrap().snooze = Some(Snooze { until: T0 + 60, from: inbox, returned: None });
    mb.account_mut(account).unwrap().snoozed_folder = Some(987_654);
    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("snooze")), "{fixed:?}");
    assert_eq!(mb.account(account).unwrap().snoozed_folder, None);
    assert_eq!(mb.message(id).unwrap().snooze, None);
    assert!(mb.message(back).unwrap().snooze.is_some_and(|s| s.returned.is_some()));
}

#[test]
fn an_old_file_loads_and_a_snooze_round_trips() {
    let (mut mb, account, _, id) = world();
    let plain = serde_json::to_string(&mb).unwrap();
    assert!(!plain.contains("snooze"), "nothing is written for mail never snoozed");
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    let json = serde_json::to_string(&mb).unwrap();
    let again: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(again.message(id).unwrap().snooze, mb.message(id).unwrap().snooze);
    assert_eq!(again.snoozed_folder(account), mb.snoozed_folder(account));
    // What isn't a snooze in its place is none, and the file still opens.
    let junk = json.replace("\"until\"", "\"until\":\"x\",\"u\"");
    let opened: Mailbox = serde_json::from_str(&junk).unwrap();
    assert!(opened.message(id).unwrap().snooze.is_none());
}
