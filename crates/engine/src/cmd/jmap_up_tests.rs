// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What was done to a JMAP account's mail here goes up to its server (M6): each kind of change as
//! the request says it, and each answer as it is taken in, with results made by hand. (What is
//! sent from such an account: `jmap_sending_tests.rs`; what a run reads: `jmap_mail_tests.rs`.)

use mailcraft_model::sync::{FolderListing, JmapChange, JmapChangeDone, JmapOutcome, MessagePage, SendDone, SendOutcome};
use mailcraft_model::{FlagStatus, FolderKind, Message, Queued};
use serde_json::json;

use super::tests::{ME, email, folder_here, here, mailboxes, marks, piece, run, session, window};
use super::*;

/// A session whose JMAP account has `One` (E1) and `Two` (E2) in its Inbox (M1), unread, and
/// `Plan` (P1) in Projects (M5); the account, and the three messages.
pub(super) fn mail() -> (Session, Id, [Id; 3]) {
    let (mut s, account) = session();
    let inbox = window("M1", vec![email("E1", "One", 1), email("E2", "Two", 2)], true);
    run(&mut s, account, vec![inbox, window("M5", vec![email("P1", "Plan", 3)], true)], true);
    let ids = ["E1", "E2", "P1"].map(|id| here(&s, account, id).unwrap().id);
    (s, account, ids)
}

/// What the next run of the account would take up.
pub(super) fn up(s: &Session, account: Id) -> Vec<JmapChange> {
    jmap_mail_request(s, account, true).unwrap().changes
}

/// A piece that says what became of `changes`, more of the run to come; what taking it in answered.
pub(super) fn answered(s: &mut Session, account: Id, changes: Vec<(JmapChange, JmapOutcome)>) -> Value {
    let changes = changes.into_iter().map(|(change, outcome)| JmapChangeDone { change, outcome }).collect();
    apply_jmap_mail(s, JmapMailResult { changes, ..piece(account) }).unwrap()
}

pub(super) fn done() -> JmapOutcome {
    JmapOutcome::Done { id: String::new() }
}

pub(super) fn refused(why: &str) -> JmapOutcome {
    JmapOutcome::Refused { why: why.into() }
}

/// The end of a run that went well and read nothing more.
pub(super) fn ended(s: &mut Session, account: Id) -> Value {
    apply_jmap_mail(s, JmapMailResult { whole: true, partial: false, ..piece(account) }).unwrap()
}

fn lines(answer: &Value) -> Vec<String> {
    answer["errors"].as_array().unwrap().iter().map(|line| line.as_str().unwrap().to_string()).collect()
}

#[test]
fn marks_made_here_go_up_as_keywords_naming_only_what_differs() {
    let (mut s, account, [one, two, _]) = mail();
    assert!(up(&s, account).is_empty());
    assert_eq!(crate::cmd::account::unpushed(&s, account), 0);
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    s.run("mail.flag", &json!({"ids": [one]})).unwrap();
    s.mailbox.message_mut(two).unwrap().answered = true;
    s.mailbox.message_mut(two).unwrap().forwarded = true;
    let keywords =
        |message, id: &str, read, flagged, answered, forwarded| JmapChange::Keywords { message, id: id.into(), read, flagged, answered, forwarded };
    let first = keywords(one, "E1", Some(true), Some(true), None, None);
    let second = keywords(two, "E2", None, None, Some(true), Some(true));
    assert_eq!(up(&s, account), [first.clone(), second.clone()]);
    assert_eq!(crate::cmd::account::unpushed(&s, account), 2);
    // A run of every mailbox carries them too, and the request shows no id of them in debug output.
    let request = jmap_mail_request(&s, account, false).unwrap();
    assert_eq!(request.changes.len(), 2);
    assert!(!format!("{request:?}").contains("E1"));
    // Made there: what the server last said is what is here, and nothing is left to go.
    let answer = answered(&mut s, account, vec![(first, done()), (second, done())]);
    assert_eq!((answer["pushed"].as_u64(), lines(&answer).len()), (Some(2), 0));
    let server = s.mailbox.message(one).unwrap().server.clone().unwrap();
    assert!(server.read && server.flag == FlagStatus::Flagged && !server.answered);
    let server = s.mailbox.message(two).unwrap().server.clone().unwrap();
    assert!(!server.read && server.answered && server.forwarded);
    assert!(up(&s, account).is_empty());
    // Taken off again, each is one mark that differs; a follow-up marked complete isn't flagged there.
    s.run("mail.markUnread", &json!({"ids": [one]})).unwrap();
    s.mailbox.message_mut(one).unwrap().flag = FlagStatus::Complete;
    assert_eq!(up(&s, account), [keywords(one, "E1", Some(false), Some(false), None, None)]);
    answered(&mut s, account, vec![(keywords(one, "E1", Some(false), Some(false), None, None), done())]);
    assert!(up(&s, account).is_empty());
    assert_eq!(s.mailbox.message(one).map(|m| m.flag), Some(FlagStatus::Complete));
}

#[test]
fn a_change_with_no_answer_is_asked_for_again_and_what_a_window_says_meanwhile_leaves_it() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    let before = up(&s, account);
    assert_eq!(before.len(), 1);
    // The run answered nothing of it (the connection went), and its window still says unread.
    run(&mut s, account, vec![window("M1", vec![marks("E1", false, false), marks("E2", false, false)], true)], false);
    assert!(s.mailbox.message(one).unwrap().read);
    assert_eq!(up(&s, account), before);
}

#[test]
fn a_move_between_two_of_the_servers_mailboxes_goes_up_and_a_new_id_is_adopted() {
    let (mut s, account, [one, two, _]) = mail();
    let (projects, trash) = (folder_here(&s, account, "M5"), folder_here(&s, account, "M4"));
    s.run("mail.move", &json!({"ids": [one], "folder": projects})).unwrap();
    // Delete is a move into the server's Deleted Items.
    s.run("mail.delete", &json!({"ids": [two]})).unwrap();
    assert_eq!(s.mailbox.message(two).map(|m| m.folder), Some(trash));
    let moved = JmapChange::Move { message: one, id: "E1".into(), from: "M1".into(), to: "M5".into() };
    let binned = JmapChange::Move { message: two, id: "E2".into(), from: "M1".into(), to: "M4".into() };
    assert_eq!(up(&s, account), [moved.clone(), binned.clone()]);
    // The server gave the first a new id in its new mailbox, and said none for the second.
    let answer = answered(&mut s, account, vec![(moved, JmapOutcome::Done { id: "E9".into() }), (binned, done())]);
    assert_eq!(answer["pushed"].as_u64(), Some(2));
    let server = |id: Id| s.mailbox.message(id).and_then(|m| m.server.clone()).map(|server| (server.id, server.folder));
    assert_eq!(server(one), Some(("E9".into(), "M5".into())));
    assert_eq!(server(two), Some(("E2".into(), "M4".into())));
    assert!(up(&s, account).is_empty());
    // The run's windows then list each where it is now: the same messages, none twice, none gone.
    let count = s.mailbox.messages.len();
    let pages = vec![
        window("M1", vec![], true),
        window("M5", vec![marks("P1", false, false), marks("E9", false, false)], true),
        window("M4", vec![marks("E2", false, false)], true),
    ];
    let answer = run(&mut s, account, pages, true);
    assert_eq!((answer["removedMessages"].as_u64(), answer["new"].as_u64(), s.mailbox.messages.len()), (Some(0), Some(0), count));
    assert_eq!(here(&s, account, "E9").map(|m| (m.id, m.folder)), Some((one, projects)));
}

#[test]
fn marks_and_a_move_of_one_message_are_both_settled_though_the_move_gives_it_a_new_id() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    s.run("mail.move", &json!({"ids": [one], "folder": folder_here(&s, account, "M5")})).unwrap();
    let changes = up(&s, account);
    assert_eq!(changes.len(), 2);
    // Answered in the other order, the move first: the marks are still taken in for the message.
    let outcomes = changes.into_iter().rev().map(|change| {
        let outcome = if matches!(change, JmapChange::Move { .. }) { JmapOutcome::Done { id: "E9".into() } } else { done() };
        (change, outcome)
    });
    assert_eq!(answered(&mut s, account, outcomes.collect())["pushed"].as_u64(), Some(2));
    assert!(s.mailbox.message(one).and_then(|m| m.server.as_ref()).is_some_and(|server| server.read && server.id == "E9"));
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_deletion_for_good_goes_up_and_the_message_isnt_brought_back_while_it_waits() {
    let (mut s, account, [one, two, _]) = mail();
    s.run("mail.deletePermanently", &json!({"ids": [one, two]})).unwrap();
    assert!(s.mailbox.message(one).is_none() && s.mailbox.message(two).is_none());
    let destroy = |id: &str| JmapChange::Destroy { id: id.into() };
    assert_eq!(up(&s, account), [destroy("E1"), destroy("E2")]);
    // A run that got no answer for them lists both, whole (the run was told neither is here):
    // neither comes back, and both are asked for again.
    let answer = run(&mut s, account, vec![window("M1", vec![email("E1", "One", 1), email("E2", "Two", 2)], true)], true);
    assert_eq!(answer["new"].as_u64(), Some(0));
    assert!(here(&s, account, "E1").is_none() && here(&s, account, "E2").is_none());
    assert_eq!(up(&s, account).len(), 2);
    // Done, or gone there already: nothing more is asked, and nothing is said.
    let answer = answered(&mut s, account, vec![(destroy("E1"), done()), (destroy("E2"), JmapOutcome::Gone)]);
    assert_eq!((answer["pushed"].as_u64(), lines(&answer).len()), (Some(2), 0));
    assert!(s.mailbox.queued.is_empty() && up(&s, account).is_empty());
    // An answer for a deletion nobody asked for changes nothing.
    assert_eq!(answered(&mut s, account, vec![(destroy("P1"), done())])["pushed"].as_u64(), Some(0));
    assert!(here(&s, account, "P1").is_some());
}

#[test]
fn a_refused_change_keeps_the_servers_state_is_said_once_and_isnt_sent_again() {
    let (mut s, account, [one, two, plan]) = mail();
    let inbox = folder_here(&s, account, "M1");
    s.run("mail.markRead", &json!({"ids": [one, two]})).unwrap();
    s.run("mail.move", &json!({"ids": [plan], "folder": inbox})).unwrap();
    let changes = up(&s, account);
    assert_eq!(changes.len(), 3);
    let answer = answered(&mut s, account, changes.into_iter().map(|change| (change, refused("the mailbox is read-only."))).collect());
    assert_eq!((answer["pushed"].as_u64(), answer["updated"].as_u64()), (Some(0), Some(3)));
    // Once per kind of change and reason, however many messages.
    let said = lines(&answer);
    assert_eq!(said, ["Couldn't mark 2 messages as read: the mailbox is read-only.", "Couldn't move 'Plan' to Inbox: the mailbox is read-only."]);
    // As the server has them again, so that there is nothing to send: not at the next run either.
    assert!(!s.mailbox.message(one).unwrap().read && !s.mailbox.message(two).unwrap().read);
    assert_eq!(s.mailbox.message(plan).map(|m| m.folder), Some(folder_here(&s, account, "M5")));
    assert!(up(&s, account).is_empty());
    // The status says it at the run's end, once: the run after says nothing of it.
    let last = ended(&mut s, account);
    assert!(lines(&last).is_empty());
    assert!(s.sync_status.starts_with(&said.join(" ")), "{}", s.sync_status);
    ended(&mut s, account);
    assert!(s.sync_status.starts_with("Up to date"), "{}", s.sync_status);
    // Made again by the owner, it is another change, and goes.
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    assert_eq!(up(&s, account).len(), 1);
}

#[test]
fn a_refusal_leaves_what_was_changed_again_here_meanwhile() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    let sent = up(&s, account);
    // While the run was under way the owner marked it unread again, and flagged it.
    s.run("mail.markUnread", &json!({"ids": [one]})).unwrap();
    s.run("mail.flag", &json!({"ids": [one]})).unwrap();
    answered(&mut s, account, sent.into_iter().map(|change| (change, refused(""))).collect());
    let m = s.mailbox.message(one).unwrap();
    assert!(!m.read && m.flag == FlagStatus::Flagged);
    assert_eq!(
        up(&s, account),
        [JmapChange::Keywords { message: one, id: "E1".into(), read: None, flagged: Some(true), answered: None, forwarded: None }]
    );
}

#[test]
fn a_refused_deletion_puts_the_message_back_where_the_server_has_it() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.deletePermanently", &json!({"ids": [one]})).unwrap();
    let answer = answered(&mut s, account, vec![(JmapChange::Destroy { id: "E1".into() }, refused("not allowed"))]);
    assert_eq!(lines(&answer), ["Couldn't permanently delete 'One': not allowed."]);
    let back = here(&s, account, "E1").unwrap();
    assert_eq!((back.folder, back.subject.as_str()), (folder_here(&s, account, "M1"), "One"));
    assert!(s.mailbox.queued.is_empty() && up(&s, account).is_empty());
    // A reason the server didn't give is said in the usual words.
    s.run("mail.deletePermanently", &json!({"ids": [here(&s, account, "E1").unwrap().id]})).unwrap();
    let answer = answered(&mut s, account, vec![(JmapChange::Destroy { id: "E1".into() }, refused(" "))]);
    assert_eq!(lines(&answer), ["Couldn't permanently delete 'One': the server says it can't make that change."]);
}

#[test]
fn a_change_of_a_message_the_server_no_longer_has_is_said_and_the_message_goes_with_its_window() {
    let (mut s, account, [one, two, _]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": folder_here(&s, account, "M5")})).unwrap();
    let changes = up(&s, account);
    let answer = answered(&mut s, account, changes.into_iter().map(|change| (change, JmapOutcome::Gone)).collect());
    assert_eq!(
        lines(&answer),
        [
            "Couldn't mark 'One' as read: the server says it no longer exists.",
            "Couldn't move 'Two' to Projects: the server says it no longer exists."
        ]
    );
    // Not asked for again; and the window of their mailbox, which no longer has them, takes them away.
    assert!(up(&s, account).is_empty());
    let answer = run(&mut s, account, vec![window("M1", vec![], true)], true);
    assert_eq!(answer["removedMessages"].as_u64(), Some(2));
    assert!(s.mailbox.message(one).is_none() && s.mailbox.message(two).is_none());
}

#[test]
fn a_run_takes_up_so_many_changes_and_the_rest_with_the_next() {
    let (mut s, account) = session();
    let page = |mailbox: &str| window(mailbox, (0..300).map(|n| email(&format!("{mailbox}-{n}"), "Mail", n)).collect(), true);
    run(&mut s, account, vec![page("M1"), page("M5")], true);
    s.run("mail.markAllRead", &json!({"folder": folder_here(&s, account, "M1")})).unwrap();
    s.run("mail.markAllRead", &json!({"folder": folder_here(&s, account, "M5")})).unwrap();
    let first = up(&s, account);
    assert_eq!((first.len(), jmap_up::MAX_JMAP_CHANGES, crate::cmd::account::unpushed(&s, account)), (500, 500, 600));
    answered(&mut s, account, first.into_iter().map(|change| (change, done())).collect());
    assert_eq!((up(&s, account).len(), crate::cmd::account::unpushed(&s, account)), (100, 100));
}

#[test]
fn what_a_host_says_became_of_changes_is_held_to_sizes_and_to_ids() {
    let (mut s, account, [one, two, plan]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": folder_here(&s, account, "M5")})).unwrap();
    let keywords = |id: &str| JmapChange::Keywords { message: one, id: id.into(), read: Some(true), flagged: None, answered: None, forwarded: None };
    let moved = |id: &str, to: &str| JmapChange::Move { message: two, id: id.into(), from: "M1".into(), to: to.into() };
    let hostile = vec![
        // Ids that are none: passed over, whatever they say.
        (keywords("../E1"), done()),
        (moved("E2", "not a mailbox"), done()),
        (JmapChange::Destroy { id: String::new() }, done()),
        // For another message than the one it names: nothing of that message is touched.
        (JmapChange::Keywords { message: plan, id: "E1".into(), read: Some(true), flagged: None, answered: None, forwarded: None }, done()),
        // A new id that is none is no new id; a reason of many lines is one, and short.
        (moved("E2", "M5"), JmapOutcome::Done { id: "new id\r\n".into() }),
        (keywords("E1"), refused(&format!("no\r\nBcc: x {}", "y".repeat(5_000)))),
    ];
    let answer = answered(&mut s, account, hostile);
    assert_eq!((answer["pushed"].as_u64(), answer["updated"].as_u64()), (Some(1), Some(1)));
    assert_eq!(s.mailbox.message(two).and_then(|m| m.server.clone()).map(|server| (server.id, server.folder)), Some(("E2".into(), "M5".into())));
    assert!(s.mailbox.message(plan).is_some_and(|m| !m.read && !m.server.as_ref().unwrap().read));
    let said = lines(&answer);
    assert!(said.len() == 1 && said[0].starts_with("Couldn't mark 'One' as read: no Bcc: x y") && said[0].chars().count() < 500, "{said:?}");
    // More answers than a run asks about: the rest is passed over (and worked out again).
    let many = (0..2_000).map(|n| (JmapChange::Destroy { id: format!("X{n}") }, refused("no"))).collect();
    assert!(lines(&answered(&mut s, account, many)).is_empty());
    // What a host says of a draft a send left behind queues no deletion: none went up.
    let sent = vec![SendDone { message: plan, outcome: SendOutcome::Waits, draft: None, draft_left: "P1".into() }];
    apply_jmap_mail(&mut s, JmapMailResult { sent, ..piece(account) }).unwrap();
    assert!(s.mailbox.queued.iter().all(|q| !matches!(q, Queued::DeleteMessage { .. })));
}

#[test]
fn undo_of_a_change_that_reached_the_server_is_another_change() {
    let (mut s, account, [one, two, plan]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    s.run("mail.move", &json!({"ids": [two], "folder": folder_here(&s, account, "M5")})).unwrap();
    let changes = up(&s, account);
    answered(&mut s, account, changes.into_iter().map(|change| (change, done())).collect());
    ended(&mut s, account);
    assert!(up(&s, account).is_empty() && s.can_undo());
    // The move back, then the mark off again: each the reverse, against what the server has now.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(up(&s, account), [JmapChange::Move { message: two, id: "E2".into(), from: "M5".into(), to: "M1".into() }]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(up(&s, account).len(), 2);
    assert!(!s.mailbox.message(one).unwrap().read && s.mailbox.message(one).unwrap().server.as_ref().unwrap().read);
    // Deleted for good can't be undone here: the server's copy is gone, or going.
    s.run("mail.deletePermanently", &json!({"ids": [plan]})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.message(plan).is_none());
    assert!(s.undo_said().iter().any(|line| line.contains("'Plan' was permanently deleted")), "{:?}", s.undo_said());
    assert!(up(&s, account).contains(&JmapChange::Destroy { id: "P1".into() }));
}

#[test]
fn the_folders_of_a_listing_that_comes_with_the_answers_are_taken_after_them() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.move", &json!({"ids": [one], "folder": folder_here(&s, account, "M5")})).unwrap();
    let changes = up(&s, account).into_iter().map(|change| JmapChangeDone { change, outcome: done() }).collect();
    // One piece with everything: the answers, the listing and the windows.
    let pages: Vec<MessagePage> =
        vec![window("M1", vec![marks("E2", false, false)], true), window("M5", vec![marks("P1", false, false), marks("E1", false, false)], true)];
    let listing = Some(FolderListing { folders: mailboxes(), complete: true });
    let answer = apply_jmap_mail(
        &mut s,
        JmapMailResult { changes, listing, pages, whole: true, partial: false, email: ME.into(), account, ..Default::default() },
    )
    .unwrap();
    assert_eq!((answer["pushed"].as_u64(), answer["removedMessages"].as_u64()), (Some(1), Some(0)));
    assert_eq!(s.mailbox.message(one).map(|m| m.folder), Some(folder_here(&s, account, "M5")));
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_message_deleted_for_good_while_its_move_is_on_its_way_is_deleted_where_the_server_has_it_now() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.move", &json!({"ids": [one], "folder": folder_here(&s, account, "M5")})).unwrap();
    let moved = up(&s, account);
    assert_eq!(moved.len(), 1);
    // Deleted for good before the server answers the move: the deletion waits under the old id.
    s.run("mail.deletePermanently", &json!({"ids": [one]})).unwrap();
    assert_eq!(up(&s, account), [JmapChange::Destroy { id: "E1".into() }]);
    // The server made the move and gave the message a new id: that is the one to delete, and what
    // was kept of the message says where the server has it, should the server refuse.
    let outcomes = moved.into_iter().map(|change| (change, JmapOutcome::Done { id: "E9".into() }));
    // (That alone is a change of the mailbox, to be saved: nothing else came with the answer.)
    let before = s.rev();
    answered(&mut s, account, outcomes.collect());
    assert!(s.rev() > before, "the deletion that waits changed, so the mailbox did");
    assert_eq!(up(&s, account), [JmapChange::Destroy { id: "E9".into() }]);
    let kept = s.mailbox.queued.iter().find_map(|queued| match queued {
        Queued::DeleteMessage { kept, .. } => kept.as_ref().and_then(|kept| kept.server.clone()),
        _ => None,
    });
    assert_eq!(kept.map(|server| (server.id, server.folder)), Some(("E9".into(), "M5".into())));
    let answer = answered(&mut s, account, vec![(JmapChange::Destroy { id: "E9".into() }, done())]);
    assert_eq!(answer["pushed"].as_u64(), Some(1));
    assert!(s.mailbox.queued.is_empty() && up(&s, account).is_empty());
}

#[test]
fn a_message_the_server_never_had_moved_to_another_account_goes_back_with_undo() {
    let (mut s, account, _) = mail();
    let other = s.run("account.add", &json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let elsewhere = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let inbox = folder_here(&s, account, "M1");
    // Only this computer has it: nothing of it is on the JMAP server.
    let mine = s.mailbox.add_message(Message { folder: inbox, subject: "Mine".into(), ..Default::default() }).unwrap();
    s.run("mail.move", &json!({"ids": [mine], "folder": elsewhere})).unwrap();
    assert_eq!(s.mailbox.message(mine).map(|m| (m.account, m.folder)), Some((other, elsewhere)));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(mine).map(|m| (m.account, m.folder)), Some((account, inbox)), "{:?}", s.undo_said());
    assert!(s.mailbox.message(mine).is_some_and(|m| m.server.is_none() && m.moving_out.is_none()));
    assert!(s.undo_said().is_empty(), "{:?}", s.undo_said());
    assert!(up(&s, account).is_empty());
}

#[test]
fn a_folder_whose_server_id_is_no_id_takes_none_of_the_servers_mail_so_no_move_is_left_unsent() {
    let (mut s, account, [one, ..]) = mail();
    let inbox = folder_here(&s, account, "M1");
    // A mailbox file can say anything: a folder of the account tied to a "mailbox" no server names.
    let odd = s.run("folder.new", &json!({"name": "Odd", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.folder_mut(odd).unwrap().server = Some(mailcraft_model::ServerFolder { id: "not an id".into(), ..Default::default() });
    // The move is refused, as into a folder the server hasn't got: where mail may go and what
    // goes up are told by one test, so nothing lies in a folder with its move never sent.
    let before = s.mailbox.clone();
    assert_eq!(s.run("mail.move", &json!({"ids": [one], "folder": odd})).unwrap_err().to_string(), mailcraft_model::ONLY_SERVER_FOLDERS);
    assert!(s.mailbox == before && up(&s, account).is_empty());
    // With an id that is one, it takes the message and the move goes up.
    s.mailbox.folder_mut(odd).unwrap().server = Some(mailcraft_model::ServerFolder { id: "M9".into(), ..Default::default() });
    s.run("mail.move", &json!({"ids": [one], "folder": odd})).unwrap();
    assert_eq!(up(&s, account), [JmapChange::Move { message: one, id: "E1".into(), from: "M1".into(), to: "M9".into() }]);
}

#[test]
fn the_outbox_takes_none_of_the_servers_mail_whatever_a_mailbox_file_says_of_it() {
    let (mut s, account, [one, ..]) = mail();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    // A mailbox file can say anything: the Outbox tied to a mailbox id that is one.
    s.mailbox.folder_mut(outbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "M7".into(), ..Default::default() });
    assert_eq!(s.mailbox.jmap_mailbox(account, outbox), None);
    // What lies in the Outbox is mail to send and no change of a message: a move into it would
    // never go up, so it is refused like a folder the server hasn't got.
    let before = s.mailbox.clone();
    assert_eq!(s.run("mail.move", &json!({"ids": [one], "folder": outbox})).unwrap_err().to_string(), mailcraft_model::ONLY_SERVER_FOLDERS);
    assert!(s.mailbox == before && up(&s, account).is_empty());
}

#[test]
fn what_a_run_that_never_ended_was_refused_is_not_said_at_the_end_of_the_next() {
    let (mut s, account, [one, ..]) = mail();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    let changes = up(&s, account);
    let answer = answered(&mut s, account, changes.into_iter().map(|change| (change, refused("the mailbox is read-only."))).collect());
    assert_eq!(lines(&answer).len(), 1);
    // That run never came to its end (the host's task stopped). The next begins with its request,
    // and at its end says nothing of what the one before was refused.
    s.run("sync.jmapRequest", &json!({})).unwrap();
    let last = ended(&mut s, account);
    assert!(lines(&last).is_empty());
    assert!(s.sync_status.starts_with("Up to date"), "{}", s.sync_status);
}
