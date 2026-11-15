// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving mail between Gmail's categories (#911): who may, what is made here at once, what goes
//! to Gmail at the next run and what comes back, undo, conversations, and agents.

use mailcraft_model::sync::{FolderDelta, GmailMove, GmailSorted, SyncResult};
use mailcraft_model::{Address, FolderKind, GmailCategory, Id, Message, OAuth, OAuthProvider};
use serde_json::json;

use crate::Session;
use crate::agent::{Verdict, access, check};
use crate::cmd::account::{apply_sync, sync_request};
use crate::gmail::{MOVES_NOT_YET, NEEDS_GOOGLE_SIGN_IN};

const ID: &str = "mail.setGmailCategory";

/// A session with a Gmail account, signing in with Google (`google`) or an app password, and moves
/// switched on (as they ship off); the account's id and its Inbox's.
fn gmail_session(google: bool) -> (Session, Id, Id) {
    let mut s = Session::empty();
    s.gmail_category_moves = true;
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    if google {
        let a = s.mailbox.accounts.iter_mut().find(|a| a.id == account).unwrap();
        a.oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
    }
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    s.mailbox.folder_mut(inbox).unwrap().uid_validity = 7;
    (s, account, inbox)
}

fn gmail_id(uid: u32) -> u64 {
    9_000 + u64::from(uid)
}

fn message(uid: u32, subject: &str, category: GmailCategory, thread: &str) -> Message {
    Message {
        uid,
        from: Address::new("Rowan", "rowan@sender.example"),
        subject: subject.into(),
        message_id: format!("<{uid}@sender.example>"),
        references: if thread.is_empty() { Vec::new() } else { vec![thread.to_string()] },
        date: 1_700_000_000 + i64::from(uid),
        gmail_category: Some(category),
        gmail_id: gmail_id(uid),
        ..Default::default()
    }
}

fn sorted(uid: u32, category: GmailCategory) -> GmailSorted {
    GmailSorted { uid, category, id: gmail_id(uid) }
}

fn inbox_delta(inbox: Id, new: Vec<Message>, gmail: Vec<GmailSorted>) -> FolderDelta {
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
    FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new, gmail, ..Default::default() }
}

fn result(account: Id, folders: Vec<FolderDelta>, gmail_moved: Vec<GmailMove>) -> SyncResult {
    SyncResult { account, email: "ada@mail.example".into(), folders, gmail_moved, ..Default::default() }
}

fn by_uid(s: &Session, account: Id, uid: u32) -> &Message {
    s.mailbox.messages.values().find(|m| m.account == account && m.uid == uid).unwrap()
}

/// A Gmail Inbox, as a run brought it and Gmail sorted it: a conversation of two in Promotions
/// (1, 2), one message in Social (3), one in Primary (4).
fn sorted_inbox(google: bool) -> (Session, Id, Id) {
    let (mut s, account, inbox) = gmail_session(google);
    let new = vec![
        message(1, "Sale", GmailCategory::Promotions, ""),
        message(2, "Re: Sale", GmailCategory::Promotions, "<1@sender.example>"),
        message(3, "Party photos", GmailCategory::Social, ""),
        message(4, "Lunch", GmailCategory::Primary, ""),
    ];
    let gmail = vec![
        sorted(1, GmailCategory::Promotions),
        sorted(2, GmailCategory::Promotions),
        sorted(3, GmailCategory::Social),
        sorted(4, GmailCategory::Primary),
    ];
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, new, gmail)], Vec::new())).unwrap();
    (s, account, inbox)
}

fn id_of(s: &Session, account: Id, uid: u32) -> Id {
    by_uid(s, account, uid).id
}

fn moves(s: &Session, account: Id) -> Vec<(Id, u64, GmailCategory)> {
    sync_request(s, account).unwrap().gmail_moves.iter().map(|m| (m.message, m.id, m.category)).collect()
}

#[test]
fn a_move_is_made_here_at_once_and_waits_for_the_next_run() {
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    let answer = s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap();
    assert_eq!((answer["moved"].as_u64(), answer["category"].as_str()), (Some(1), Some("primary")));
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_synced), (Some(GmailCategory::Primary), Some(GmailCategory::Social)));
    // Counted where it is now.
    let inbox = m.folder;
    let counts = s.folder_counts();
    assert_eq!((counts.gmail(inbox, GmailCategory::Primary).total, counts.gmail(inbox, GmailCategory::Social).total), (2, 0));
    // The next request has it, by Gmail's id; nothing else waits.
    assert_eq!(moves(&s, account), [(three, gmail_id(3), GmailCategory::Primary)]);
    // Gmail took it: the two agree, and nothing waits any more.
    let moved = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Primary }];
    apply_sync(&mut s, result(account, Vec::new(), moved)).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_synced, Some(GmailCategory::Primary));
    assert!(moves(&s, account).is_empty());
    // And the next reading of the Inbox agrees with it.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Primary)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Primary));
    assert!(moves(&s, account).is_empty());
}

#[test]
fn a_move_waiting_wins_over_gmails_reading_until_gmail_has_it() {
    let (mut s, account, inbox) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    // A run reads the Inbox before the move went (it was made while the run was under way): Gmail
    // still says Social, and the move made here stays.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Social)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Forums));
    assert_eq!(moves(&s, account), [(three, gmail_id(3), GmailCategory::Forums)]);
    // Gmail re-sorts it meanwhile, elsewhere: the owner's move still wins.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Updates)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Forums));
    assert_eq!(moves(&s, account).len(), 1);
    // A change made only in Gmail, to a message not moved here, is taken in.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(4, GmailCategory::Updates)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 4).gmail_category, Some(GmailCategory::Updates));
    assert_eq!(moves(&s, account).len(), 1, "taking it in is no move of the owner's");
    // A move Gmail took and then the owner moved on again: the later one goes next time.
    let four = id_of(&s, account, 4);
    s.run(ID, &json!({"ids": [four], "category": "social"})).unwrap();
    let moved = vec![GmailMove { message: four, id: gmail_id(4), category: GmailCategory::Primary }];
    apply_sync(&mut s, result(account, Vec::new(), moved)).unwrap();
    assert!(moves(&s, account).contains(&(four, gmail_id(4), GmailCategory::Social)));
}

#[test]
fn the_reading_of_the_run_that_moved_it_doesnt_undo_the_move() {
    let (mut s, account, inbox) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap();
    // Gmail took the move, and the same run's search, not caught up yet, still says Social.
    let moved = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Primary }];
    let stale = inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Social), sorted(4, GmailCategory::Updates)]);
    apply_sync(&mut s, result(account, vec![stale], moved)).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_synced), (Some(GmailCategory::Primary), Some(GmailCategory::Primary)));
    assert!(moves(&s, account).is_empty());
    // The rest of the reading is taken in.
    assert_eq!(by_uid(&s, account, 4).gmail_category, Some(GmailCategory::Updates));
    // A later run's reading is believed: Gmail re-sorted it since.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Forums)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Forums));
}

fn refused(account: Id, refused: Vec<GmailMove>, unsent: usize) -> SyncResult {
    SyncResult { gmail_refused: refused, gmail_unsent: unsent, ..result(account, Vec::new(), Vec::new()) }
}

/// What a result said, taken in (the errors the run shows).
fn said(s: &mut Session, r: SyncResult) -> Vec<String> {
    let answer = apply_sync(s, r).unwrap();
    answer["errors"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

#[test]
fn a_move_gmail_keeps_refusing_is_given_up_after_three_runs() {
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    let no = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Forums }];
    // Two runs refused: asked again, and said so.
    for run in 1..=2u8 {
        let lines = said(&mut s, refused(account, no.clone(), 0));
        assert!(lines.iter().any(|l| l.contains("Gmail refused to move 1 message")), "{lines:?}");
        assert_eq!(by_uid(&s, account, 3).gmail_refusals, run);
        assert_eq!(moves(&s, account).len(), 1);
    }
    // A new move of the owner's starts again from nothing.
    s.run(ID, &json!({"ids": [three], "category": "updates"})).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 0);
    let no = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Updates }];
    said(&mut s, refused(account, no.clone(), 0));
    said(&mut s, refused(account, no.clone(), 0));
    // The third run in a row: given up, where Gmail has it, said once, nothing queued.
    let lines = said(&mut s, refused(account, no.clone(), 0));
    assert!(lines.iter().any(|l| l == "Gmail wouldn't move 1 message; it stays where Gmail has it"), "{lines:?}");
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_synced, m.gmail_refusals), (Some(GmailCategory::Social), Some(GmailCategory::Social), 0));
    assert!(moves(&s, account).is_empty());
    // Not said again: there is no move left to refuse.
    assert!(said(&mut s, refused(account, no, 0)).is_empty());
    // Gmail taking a move clears the count.
    s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap();
    let primary = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Primary }];
    said(&mut s, refused(account, primary.clone(), 0));
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 1);
    apply_sync(&mut s, result(account, Vec::new(), primary)).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 0);
}

#[test]
fn refusals_count_by_gmail_message_and_only_when_the_move_was_tried() {
    let (mut s, account, inbox) = sorted_inbox(true);
    let mut copy = by_uid(&s, account, 3).clone();
    (copy.id, copy.uid, copy.message_id) = (0, 30, "<copy@sender.example>".into());
    let copy = s.mailbox.add_message(copy).unwrap();
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    let no: Vec<GmailMove> = [three, copy].map(|message| GmailMove { message, id: gmail_id(3), category: GmailCategory::Forums }).into();
    // Two copies refused: one message of Gmail's.
    let lines = said(&mut s, refused(account, no.clone(), 0));
    assert!(lines.iter().any(|l| l.contains("Gmail refused to move 1 message ")), "{lines:?}");
    // A run that didn't get to try it (it read the Inbox, Gmail still says Social): no change.
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Social)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 1);
    said(&mut s, refused(account, no.clone(), 0));
    let lines = said(&mut s, refused(account, no, 0));
    assert!(lines.iter().any(|l| l == "Gmail wouldn't move 1 message; it stays where Gmail has it"), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("refused to move")), "{lines:?}");
}

#[test]
fn refusals_go_back_to_nothing_whenever_no_move_is_waiting() {
    let (mut s, account, inbox) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    let refuse = |s: &mut Session, category| {
        let no = vec![GmailMove { message: three, id: gmail_id(3), category }];
        said(s, refused(account, no, 0));
    };
    // Gmail comes to agree (moved there some other way): nothing waits, nothing counts.
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    refuse(&mut s, GmailCategory::Forums);
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 1);
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Forums)])], Vec::new())).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_move_waiting(), m.gmail_refusals), (None, 0));
    // Moves switched off, so Gmail's word wins: likewise.
    s.run(ID, &json!({"ids": [three], "category": "updates"})).unwrap();
    refuse(&mut s, GmailCategory::Updates);
    s.gmail_category_moves = false;
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Forums)])], Vec::new())).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_refusals), (Some(GmailCategory::Forums), 0));
    s.gmail_category_moves = true;
    // Undo leaving nothing waiting.
    s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap();
    refuse(&mut s, GmailCategory::Primary);
    assert_eq!(by_uid(&s, account, 3).gmail_refusals, 1);
    s.run("edit.undo", &json!({})).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_move_waiting(), m.gmail_refusals), (Some(GmailCategory::Forums), None, 0));
}

#[test]
fn undo_leaves_no_refusals_where_no_move_waits_even_from_an_older_file() {
    // A file from before refusals were cleared with the move: a count left on a message whose
    // move is no longer waiting.
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.mailbox.message_mut(three).unwrap().gmail_refusals = 2;
    assert_eq!(by_uid(&s, account, 3).gmail_move_waiting(), None);
    // A move and its undo: the step's snapshot holds the count from before.
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_move_waiting(), m.gmail_refusals), (Some(GmailCategory::Social), None, 0));
}

#[test]
fn copies_share_one_count_and_are_given_up_together() {
    let (mut s, account, _) = sorted_inbox(true);
    let mut copy = by_uid(&s, account, 3).clone();
    (copy.id, copy.uid, copy.message_id) = (0, 30, "<copy@sender.example>".into());
    let copy = s.mailbox.add_message(copy).unwrap();
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    // Counts that drifted apart (an older file): 1 and 2.
    s.mailbox.message_mut(three).unwrap().gmail_refusals = 1;
    s.mailbox.message_mut(copy).unwrap().gmail_refusals = 2;
    // One refusal, of one copy: the message's count is the higher, one more, and that is three.
    let no = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Forums }];
    let lines = said(&mut s, refused(account, no, 0));
    assert_eq!(lines, ["Gmail wouldn't move 1 message; it stays where Gmail has it"]);
    for id in [three, copy] {
        let m = s.mailbox.message(id).unwrap();
        assert_eq!((m.gmail_category, m.gmail_move_waiting(), m.gmail_refusals), (Some(GmailCategory::Social), None, 0));
    }
    assert!(moves(&s, account).is_empty());
}

#[test]
fn moves_left_unsent_are_said_without_blaming_gmail_for_them() {
    let (mut s, account, _) = sorted_inbox(true);
    let (three, four) = (id_of(&s, account, 3), id_of(&s, account, 4));
    s.run(ID, &json!({"ids": [three, four], "category": "forums"})).unwrap();
    let no = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Forums }];
    let lines = said(&mut s, refused(account, no, 5));
    assert!(
        lines.iter().any(|l| l == "6 messages couldn't be moved between Gmail's categories this run: Gmail refused 1 of them; the rest wait for the next run"),
        "{lines:?}"
    );
    let lines = said(&mut s, refused(account, Vec::new(), 2));
    assert!(lines.iter().any(|l| l == "2 messages couldn't be moved between Gmail's categories this run; they wait for the next run"), "{lines:?}");
}

#[test]
fn copies_of_one_gmail_message_move_together_and_the_last_move_wins() {
    let (mut s, account, inbox) = sorted_inbox(true);
    // A second copy here of Gmail's message 3.
    let mut copy = by_uid(&s, account, 3).clone();
    (copy.id, copy.uid, copy.message_id) = (0, 30, "<copy@sender.example>".into());
    let copy = s.mailbox.add_message(copy).unwrap();
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    s.run(ID, &json!({"ids": [copy], "category": "updates"})).unwrap();
    let both = [three, copy].map(|id| s.mailbox.message(id).unwrap().gmail_category);
    assert_eq!(both, [Some(GmailCategory::Updates); 2]);
    let mut waiting = moves(&s, account);
    waiting.sort();
    let mut expected = vec![(three, gmail_id(3), GmailCategory::Updates), (copy, gmail_id(3), GmailCategory::Updates)];
    expected.sort();
    assert_eq!(waiting, expected);
    let _ = inbox;
}

#[test]
fn the_reasons_compared_are_the_reasons_given() {
    // The window compares with these: they must be what the two functions say.
    let (mut s, account, _) = sorted_inbox(false);
    assert_eq!(crate::gmail::moves_offered(&s), Some(NEEDS_GOOGLE_SIGN_IN));
    let three = by_uid(&s, account, 3).clone();
    assert_eq!(crate::gmail::why_not_movable(&s, &three), Some(NEEDS_GOOGLE_SIGN_IN));
    s.gmail_category_moves = false;
    assert_eq!(crate::gmail::moves_offered(&s), Some(MOVES_NOT_YET));
}

#[test]
fn undo_puts_it_back_and_after_gmail_had_it_moves_it_back_there() {
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    // Before it went: nothing to tell Gmail.
    s.run(ID, &json!({"ids": [three], "category": "updates"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social));
    assert!(moves(&s, account).is_empty());
    // After it went: the undone category goes to Gmail at the next run.
    s.run(ID, &json!({"ids": [three], "category": "updates"})).unwrap();
    let moved = vec![GmailMove { message: three, id: gmail_id(3), category: GmailCategory::Updates }];
    apply_sync(&mut s, result(account, Vec::new(), moved)).unwrap();
    assert!(moves(&s, account).is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social));
    assert_eq!(moves(&s, account), [(three, gmail_id(3), GmailCategory::Social)]);
    // And redo moves it on again, to what Gmail has.
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Updates));
    assert!(moves(&s, account).is_empty());
}

#[test]
fn in_conversation_view_the_selection_takes_its_conversation_and_named_ids_move_alone() {
    let (mut s, account, inbox) = sorted_inbox(true);
    let (one, two) = (id_of(&s, account, 1), id_of(&s, account, 2));
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    // Named, as an agent names them: that message alone, as `mail.move` moves it.
    let answer = s.run(ID, &json!({"ids": [two], "category": "updates"})).unwrap();
    assert_eq!(answer["moved"].as_u64(), Some(1));
    assert_eq!(
        (by_uid(&s, account, 1).gmail_category, by_uid(&s, account, 2).gmail_category),
        (Some(GmailCategory::Promotions), Some(GmailCategory::Updates))
    );
    // The selection: its whole conversation in that Inbox, and nothing else.
    s.sel.messages = vec![one];
    let answer = s.run(ID, &json!({"category": "primary"})).unwrap();
    assert_eq!(answer["moved"].as_u64(), Some(2));
    assert_eq!(by_uid(&s, account, 2).gmail_category, Some(GmailCategory::Primary));
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social), "not of the conversation");
    let mut waiting: Vec<Id> = moves(&s, account).iter().map(|m| m.0).collect();
    waiting.sort();
    assert_eq!(waiting, [one, two]);
    // Without conversations, the selection alone.
    s.run("view.conversations", &json!({"value": false})).unwrap();
    s.sel.messages = vec![one];
    s.run(ID, &json!({"category": "forums"})).unwrap();
    assert_eq!(
        (by_uid(&s, account, 1).gmail_category, by_uid(&s, account, 2).gmail_category),
        (Some(GmailCategory::Forums), Some(GmailCategory::Primary))
    );
}

#[test]
fn switched_off_gmails_reading_wins_over_a_move_left_waiting() {
    // A mailbox file saved with a move waiting (made in a build with moves on), opened in one with
    // them off: the next reading of the Inbox has Gmail's word, and nothing is queued.
    let (mut s, account, inbox) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    let saved = serde_json::to_string(&s.mailbox).unwrap();
    let mut s = Session::empty();
    s.mailbox = serde_json::from_str(&saved).unwrap();
    assert!(!s.gmail_category_moves);
    assert_eq!(by_uid(&s, account, 3).gmail_move_waiting(), Some(GmailCategory::Forums));
    assert!(moves(&s, account).is_empty());
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Updates)])], Vec::new())).unwrap();
    let m = by_uid(&s, account, 3);
    assert_eq!((m.gmail_category, m.gmail_synced, m.gmail_move_waiting()), (Some(GmailCategory::Updates), Some(GmailCategory::Updates), None));
    assert!(moves(&s, account).is_empty());
    // Likewise once the account no longer signs in with Google, moves on or not.
    let (mut s, account, inbox) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.run(ID, &json!({"ids": [three], "category": "forums"})).unwrap();
    s.mailbox.accounts.iter_mut().find(|a| a.id == account).unwrap().oauth = None;
    apply_sync(&mut s, result(account, vec![inbox_delta(inbox, Vec::new(), vec![sorted(3, GmailCategory::Social)])], Vec::new())).unwrap();
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social));
    assert!(moves(&s, account).is_empty());
}

#[test]
fn only_a_google_sign_in_moves_and_only_while_moves_are_on() {
    // Switched off, as the program ships: disabled with the reason, refused when named, nothing in
    // the request.
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    s.gmail_category_moves = false;
    assert_eq!(s.registry.get(ID).map(|c| (c.enabled)(&s)), Some(Some(MOVES_NOT_YET)));
    assert!(s.run(ID, &json!({"ids": [three], "category": "primary"})).is_err());
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social));
    // Even a move made while they were on stays out of the request once they are off.
    s.gmail_category_moves = true;
    s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap();
    s.gmail_category_moves = false;
    assert!(moves(&s, account).is_empty());
    const { assert!(!crate::gmail::GMAIL_CATEGORY_MOVES, "ships switched off until Google's review") };
    assert!(!Session::empty().gmail_category_moves);

    // An app password: categories to look at, none to move between.
    let (mut s, account, _) = sorted_inbox(false);
    let three = id_of(&s, account, 3);
    assert_eq!(s.registry.get(ID).map(|c| (c.enabled)(&s)), Some(Some(NEEDS_GOOGLE_SIGN_IN)));
    let e = s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap_err();
    assert!(e.to_string().contains("Google sign-in"), "{e}");
    assert!(moves(&s, account).is_empty());

    // Signed in with Google beside it: offered, but this account's mail still can't move.
    let (mut s, account, _) = sorted_inbox(false);
    let three = id_of(&s, account, 3);
    let other = s.run("account.add", &json!({"email": "bea@mail.example", "incoming": {"host": "imap.gmail.com"}})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.accounts.iter_mut().find(|a| a.id == other).unwrap().oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
    assert_eq!(s.registry.get(ID).map(|c| (c.enabled)(&s)), Some(None));
    let e = s.run(ID, &json!({"ids": [three], "category": "primary"})).unwrap_err();
    assert!(e.to_string().contains(NEEDS_GOOGLE_SIGN_IN), "{e}");
}

#[test]
fn what_isnt_in_a_sorted_gmail_inbox_is_refused_and_nothing_changes() {
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    // A message Gmail gave no id of.
    let four = id_of(&s, account, 4);
    s.mailbox.message_mut(four).unwrap().gmail_id = 0;
    assert!(s.run(ID, &json!({"ids": [three, four], "category": "updates"})).is_err());
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Social), "all or nothing");
    // Mail of another folder, or of a local account.
    let local = s.mailbox.inbox().unwrap();
    let elsewhere = s.mailbox.add_message(Message { folder: local, subject: "Here".into(), ..Default::default() }).unwrap();
    assert!(s.run(ID, &json!({"ids": [elsewhere], "category": "updates"})).is_err());
    // Words that are no category, and ids that are none.
    for params in [
        json!({"ids": [three], "category": "spam"}),
        json!({"ids": [three]}),
        json!({"ids": [three], "category": 3}),
        json!({"ids": [999_999], "category": "social"}),
        json!({"ids": "three", "category": "social"}),
    ] {
        assert!(s.run(ID, &params).is_err(), "{params}");
    }
    assert!(moves(&s, account).is_empty());
    // The owner's selection, though, moves what of it can move.
    s.sel.messages = vec![three, four];
    assert_eq!(s.run(ID, &json!({"category": "updates"})).unwrap()["moved"].as_u64(), Some(1));
    assert_eq!(by_uid(&s, account, 3).gmail_category, Some(GmailCategory::Updates));
    s.sel.messages = vec![four];
    assert!(s.run(ID, &json!({"category": "forums"})).is_err());
}

#[test]
fn an_agent_moves_as_it_moves_to_a_folder_naming_the_messages() {
    let (mut s, account, _) = sorted_inbox(true);
    let three = id_of(&s, account, 3);
    assert_eq!(access(ID), access("mail.move"));
    assert!(crate::agent::NEEDS_IDS.contains(&ID));
    // The selection is the owner's: an agent names the messages.
    s.run("mail.select", &json!({"id": three})).unwrap();
    assert!(matches!(check(&s, ID, &json!({"category": "primary"}), &[]), Verdict::Refuse(_)));
    assert_eq!(check(&s, ID, &json!({"ids": [three], "category": "primary"}), &[]), Verdict::Run);
    // Switched off, an agent gets the same refusal as anyone.
    s.gmail_category_moves = false;
    assert!(s.run(ID, &json!({"ids": [three], "category": "primary"})).is_err());
}
