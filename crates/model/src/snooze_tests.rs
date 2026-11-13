// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949) in the mailbox: where a message waits, where it goes back to, and what repair
//! drops.
use crate::{Account, AccountKind, FolderKind, Id, MAX_SNOOZE_REFUSALS, Mailbox, Message, Pending, Snooze};

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
    assert_eq!(m.snooze.map(|z| (z.until, z.from, z.returned)), Some((T0 + 60, inbox, None)));
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
    assert_eq!(mb.message(id).unwrap().snooze.map(|z| (z.until, z.from, z.returned)), Some((T0 + 600, inbox, None)));
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
    assert_eq!(m.snooze.map(|z| (z.until, z.from, z.returned)), Some((T0 + 60, inbox, Some(T0 + 3_600))));
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
    assert_eq!(mb.settle_snoozes().called_off, 1);
    assert_eq!(mb.message(id).unwrap().snooze, None);
    assert!(mb.due_snoozes(T0 + 999).is_empty());
}

#[test]
fn repair_forgets_a_missing_folder_and_drops_snoozes_of_mail_outside_it_but_keeps_returned_marks() {
    let (mut mb, account, inbox, id) = world();
    let back = mb.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    mb.message_mut(back).unwrap().snooze = Some(Snooze { until: T0, after_id: 0, from: inbox, returned: Some(T0 + 5), ..Default::default() });
    mb.message_mut(id).unwrap().snooze = Some(Snooze { until: T0 + 60, after_id: 0, from: inbox, returned: None, ..Default::default() });
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

#[test]
fn a_snoozed_folder_the_owner_deleted_is_gone_and_a_new_snooze_makes_another() {
    let (mut mb, account, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    let old = mb.snoozed_folder(account).unwrap();
    // Deleting a folder puts it, with its id, under Deleted Items.
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    mb.move_folder(old, Some(deleted)).unwrap();
    assert_eq!(mb.snoozed_folder(account), None);
    assert_eq!(mb.settle_snoozes().called_off, 1);
    assert_eq!(mb.message(id).unwrap().snooze, None);
    let other = mb.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    mb.snooze_messages(&[other], T0 + 60, "Snoozed").unwrap();
    let fresh = mb.snoozed_folder(account).unwrap();
    assert_ne!(fresh, old);
    assert_eq!(mb.folder(fresh).unwrap().parent, None, "not filed into Deleted Items/Snoozed");
}

#[test]
fn a_file_naming_the_inbox_or_the_outbox_as_the_snoozed_folder_is_forgotten() {
    for kind in [FolderKind::Inbox, FolderKind::Outbox, FolderKind::Deleted, FolderKind::Sent] {
        let (mut mb, account, inbox, id) = world();
        let hostile = mb.folder_of_kind(account, kind).unwrap();
        mb.account_mut(account).unwrap().snoozed_folder = Some(hostile);
        mb.message_mut(id).unwrap().snooze = Some(Snooze { until: T0 + 60, after_id: 0, from: inbox, returned: None, ..Default::default() });
        let fixed = mb.repair();
        assert!(fixed.iter().any(|f| f.contains("snooze")), "{kind:?} {fixed:?}");
        assert_eq!(mb.account(account).unwrap().snoozed_folder, None, "{kind:?}");
        assert_eq!(mb.message(id).unwrap().snooze, None, "{kind:?}");
    }
}

#[test]
fn mail_in_an_adopted_snoozed_folder_that_was_never_snoozed_is_refused() {
    let (mut mb, account, _, _) = world();
    let theirs = mb.add_folder(account, None, "Snoozed").unwrap();
    let there = mb.add_message(Message { account, folder: theirs, ..Default::default() }).unwrap();
    assert!(mb.snooze_messages(&[there], T0 + 60, "Snoozed").is_err());
    assert_eq!(mb.message(there).unwrap().snooze, None);
}

#[test]
fn a_message_back_in_snoozed_because_the_server_refused_the_move_waits_again() {
    let (mut mb, account, _, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    assert_eq!(mb.return_snoozed(&[id], T0 + 61), vec![id]);
    // The server refused the move back: the message is in Snoozed again, marked as come back.
    let snoozed = mb.snoozed_folder(account).unwrap();
    mb.message_mut(id).unwrap().folder = snoozed;
    assert_eq!(mb.settle_snoozes().reset, 1);
    assert!(mb.message(id).unwrap().snooze.is_some_and(|s| s.waiting()));
    assert_eq!(mb.due_snoozes(T0 + 61), vec![id]);
}

/// An IMAP account whose Inbox follows "INBOX", with `n` messages of UIDs 1 to `n` in it.
fn imap_world(n: u32) -> (Mailbox, Id, Id, Vec<Id>) {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { name: "Imap".into(), email: "ann@mail.example".into(), kind: AccountKind::Imap, ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let ids = (1..=n)
        .map(|uid| mb.add_message(Message { account, folder: inbox, uid, subject: format!("m{uid}"), ..Default::default() }).unwrap())
        .collect();
    (mb, account, inbox, ids)
}

fn move_each_calls() -> usize {
    crate::mailbox::MOVE_EACH_CALLS.with(|c| c.get())
}

#[test]
fn many_messages_snooze_and_return_in_one_move_each_and_the_server_is_told_of_each() {
    let (mut mb, account, inbox, ids) = imap_world(300);
    let before = move_each_calls();
    assert_eq!(mb.snooze_messages(&ids, T0 + 60, "Snoozed").unwrap(), 300);
    assert_eq!(move_each_calls() - before, 1, "one batch, not one call a message");
    let snoozed = mb.snoozed_folder(account).unwrap();
    assert_eq!(mb.pending.len(), 300);
    assert!(mb.pending.iter().all(|p| matches!(p, Pending::Move { to, from, .. } if *to == snoozed && from == "INBOX")));
    // The server did them all: the folder is its mailbox, with UIDs there.
    mb.pending.clear();
    mb.folder_mut(snoozed).unwrap().remote = "Snoozed".into();
    for (n, id) in ids.iter().enumerate() {
        mb.message_mut(*id).unwrap().uid = u32::try_from(n).unwrap() + 1;
    }
    let before = move_each_calls();
    assert_eq!(mb.return_snoozed(&ids, T0 + 61).len(), 300);
    assert_eq!(move_each_calls() - before, 1);
    assert_eq!(mb.pending.len(), 300);
    assert!(mb.pending.iter().all(|p| matches!(p, Pending::Move { to, from, .. } if *to == inbox && from == "Snoozed")));
}

#[test]
fn a_return_the_server_refuses_is_tried_three_times_and_then_the_snooze_is_called_off() {
    let (mut mb, account, _, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    let snoozed = mb.snoozed_folder(account).unwrap();
    for round in 1..=MAX_SNOOZE_REFUSALS {
        assert_eq!(mb.return_snoozed(&[id], T0 + 61), vec![id], "round {round}");
        assert!(mb.message(id).unwrap().snooze.is_some_and(|s| s.returning));
        // The server refused: it lies in Snoozed again, as the server has it.
        mb.message_mut(id).unwrap().folder = snoozed;
        let settled = mb.settle_snoozes();
        if round < MAX_SNOOZE_REFUSALS {
            assert_eq!((settled.reset, settled.gave_up.len()), (1, 0), "round {round}");
            assert!(mb.message(id).unwrap().snooze.is_some_and(|s| s.waiting() && s.refusals == round));
        } else {
            assert_eq!((settled.reset, settled.gave_up), (0, vec![id]));
            assert_eq!(mb.message(id).unwrap().snooze, None);
        }
    }
    // Left where the server has it, and nothing tries again.
    assert_eq!(mb.message(id).unwrap().folder, snoozed);
    assert!(mb.due_snoozes(T0 + 9_999).is_empty());
}

#[test]
fn a_message_that_came_back_and_is_filed_into_snoozed_by_hand_is_left_there() {
    let (mut mb, account, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    mb.return_snoozed(&[id], T0 + 61);
    // Confirmed (nothing waits for a local message), no longer in flight.
    assert_eq!(mb.settle_snoozes().confirmed, 1);
    let snoozed = mb.snoozed_folder(account).unwrap();
    mb.move_messages(&[id], snoozed).unwrap();
    let settled = mb.settle_snoozes();
    assert_eq!((settled.reset, settled.called_off), (0, 1));
    let m = mb.message(id).unwrap();
    assert_eq!((m.folder, m.snooze), (snoozed, None));
    assert!(mb.due_snoozes(T0 + 9_999).is_empty());
    let _ = inbox;
}

#[test]
fn moving_a_message_that_is_still_returning_ends_the_return_in_flight() {
    let (mut mb, account, _, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    mb.return_snoozed(&[id], T0 + 61);
    let snoozed = mb.snoozed_folder(account).unwrap();
    // Filed into Snoozed by hand before any settling: that is not a refusal.
    mb.move_messages(&[id], snoozed).unwrap();
    assert!(mb.message(id).unwrap().snooze.is_some_and(|s| !s.returning));
    assert_eq!(mb.settle_snoozes().reset, 0);
}

#[test]
fn a_selection_with_a_shared_mailbox_leaves_nothing_behind() {
    let (mut mb, account, inbox, id) = world();
    let shared = mb.add_account(Account { name: "Team".into(), email: "team@example.org".into(), shared_of: Some(account), ..Default::default() });
    let theirs = mb.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    let other = mb.add_message(Message { account: shared, folder: theirs, ..Default::default() }).unwrap();
    let folders = mb.folders.len();
    assert!(mb.snooze_messages(&[id, other], T0 + 60, "Snoozed").is_err());
    assert_eq!((mb.folders.len(), mb.snoozed_folder(account), mb.message(id).unwrap().folder), (folders, None, inbox));
}

#[test]
fn a_copy_is_not_snoozed_and_mail_isnt_returned_into_drafts_or_an_outbox() {
    let (mut mb, account, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    let snoozed = mb.snoozed_folder(account).unwrap();
    for copy in [mb.copy_message(id, inbox).unwrap(), mb.copy_message_marked(id, inbox, 7).unwrap()] {
        assert_eq!(mb.message(copy).unwrap().snooze, None);
    }
    for kind in [FolderKind::Drafts, FolderKind::Outbox, FolderKind::Deleted] {
        let (mut mb, account, inbox, id) = world();
        mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
        let bad = mb.folder_of_kind(account, kind).unwrap();
        mb.message_mut(id).unwrap().snooze.as_mut().unwrap().from = bad;
        assert_eq!(mb.return_snoozed(&[id], T0 + 61), vec![id]);
        assert_eq!(mb.message(id).unwrap().folder, inbox, "{kind:?}");
    }
    let _ = snoozed;
}

#[test]
fn a_message_id_watermark_says_what_arrived_after_the_snooze() {
    let (mut mb, account, inbox, id) = world();
    mb.snooze_messages(&[id], T0 + 60, "Snoozed").unwrap();
    let mark = mb.message(id).unwrap().snooze.unwrap().after_id;
    let later = mb.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    assert!(mark >= id && later > mark);
}
