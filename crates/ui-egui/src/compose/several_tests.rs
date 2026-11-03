// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Several messages being written (#286), as the window has them: each its own body (its id, its
//! caret and the keyboard), its own Bcc line; `ui.inspect` lists them; and quitting asks once
//! about every one with changes, Save Draft saving each.

use serde_json::json;

use super::editor;
use super::tests::{Window, app, plain, type_in, window};

/// Two messages being written, the owner's text in each, both with changes not saved (the first
/// was saved when the second opened, and written in since): their ids, the current one last.
fn two() -> (Window, u64, u64) {
    let mut a = app();
    let first = a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "First"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "One"})).unwrap();
    let second = a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "Second"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    a.execute("compose.set", json!({"compose": first, "body": "One, again"})).unwrap();
    (window(a), first, second)
}

#[test]
fn each_message_has_a_body_of_its_own() {
    let (mut h, first, second) = two();
    assert_ne!(editor::id_of(first), editor::id_of(second));
    assert_eq!(editor::id(h.state()), editor::id_of(second));
    // The keyboard in the current one's body types there alone.
    h.state_mut().execute("compose.select", json!({"from": 3})).unwrap();
    h.state_mut().want_body_focus();
    h.run_steps(3);
    assert!(editor::has_focus(&h.ctx));
    type_in(&mut h, " more");
    assert!(plain(&h).starts_with("Two more"), "{}", plain(&h));
    let other = h.state().session.composing_by_id(first).unwrap().body.plain();
    assert!(other.starts_with("One") && !other.contains("more"), "{other}");
    // The other one current, its body is another: the keyboard isn't carried over to it.
    assert!(h.state_mut().session.show_compose(first));
    // (Made current as a command does it: the reading pane shows it from now on, #286.)
    crate::windows::compose_window::settle_pane(h.state_mut());
    h.run_steps(3);
    assert_eq!(editor::id(h.state()), editor::id_of(first));
    assert!(!h.ctx.memory(|m| m.has_focus(editor::id_of(first))));
}

#[test]
fn the_keyboard_asked_for_one_message_is_not_taken_by_another() {
    let (mut h, first, _) = two();
    h.state_mut().want_body_focus();
    assert!(h.state_mut().session.show_compose(first));
    // (Made current as a command does it: the reading pane shows it from now on, #286.)
    crate::windows::compose_window::settle_pane(h.state_mut());
    h.run_steps(3);
    assert!(!editor::has_focus(&h.ctx), "asked for the other message's body");
}

#[test]
fn the_bcc_line_is_shown_for_the_message_it_was_asked_for() {
    let (mut h, first, second) = two();
    assert!(!h.state().bcc_shown());
    h.state_mut().compose_bcc.insert(second);
    assert!(h.state().bcc_shown());
    assert!(h.state_mut().session.show_compose(first));
    assert!(!h.state().bcc_shown(), "not the other's");
    // Closed, what was kept of it goes.
    h.state_mut().run("compose.close", json!({"compose": second, "discard": true})).unwrap();
    assert!(!h.state().compose_bcc.contains(&second));
}

#[test]
fn ui_inspect_lists_every_message_being_written() {
    let (mut h, first, second) = two();
    let ctx = h.ctx.clone();
    let said = crate::control::inspect(h.state_mut(), &ctx);
    let open = said["beingWritten"].as_array().unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!((open[0]["compose"].as_u64(), open[0]["current"].as_bool()), (Some(second), Some(true)));
    assert_eq!((open[1]["compose"].as_u64(), open[1]["current"].as_bool()), (Some(first), Some(false)));
    assert_eq!(open[1]["subject"], "First");
    assert_eq!(said["composing"], true);
}

#[test]
fn quitting_saves_them_all_to_drafts_unasked() {
    // Closing the window (#286, slice 3) asks nothing: every message being written goes to Drafts.
    let draft = |h: &Window, subject: &str| h.state().session.mailbox.messages.values().any(|m| m.draft && m.subject == subject);
    let (mut h, _, _) = two();
    assert!(h.state_mut().close_requested());
    assert!(h.state().dialog.is_none());
    assert_eq!((draft(&h, "First"), draft(&h, "Second")), (true, true));
}

/// Add an account of `kind` (`imap`, `local`, `microsoft`): its id.
fn account(a: &mut crate::MailApp, email: &str, kind: &str) -> u64 {
    let mut p = json!({"email": email, "kind": kind});
    if kind == "imap" {
        p["incoming"] = json!({"host": "imap.mail.example"});
    }
    a.execute("account.add", p).unwrap()["id"].as_u64().unwrap()
}

/// The background saves (#286), as a frame does them, `secs` after the clock's `start`.
fn tick_at(a: &mut crate::MailApp, start: i64, secs: i64) {
    a.session.clock.now = Some(start + secs);
    a.draft_autosave_tick(&egui::Context::default());
}

#[test]
fn every_message_with_changes_is_saved_by_itself_every_five_minutes_for_every_kind_of_account() {
    let mut a = app();
    let start = a.session.now();
    let imap = account(&mut a, "one@mail.example", "imap");
    let local = account(&mut a, "two@mail.example", "local");
    let microsoft = account(&mut a, "three@mail.example", "microsoft");
    let on_imap = a.execute("mail.new", json!({"account": imap, "subject": "On IMAP"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "One"})).unwrap();
    let here = a.execute("mail.new", json!({"account": local, "subject": "Kept here"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    // The third, the current one, has nothing in it but what it started with.
    let clean = a.execute("mail.new", json!({"account": microsoft})).unwrap()["compose"].as_u64().unwrap();
    // (Opening the next saved the one before: written in again, it has changes again.)
    a.execute("compose.set", json!({"compose": on_imap, "body": "One, again"})).unwrap();
    a.execute("compose.set", json!({"compose": here, "body": "Two, again"})).unwrap();
    let c = |a: &crate::MailApp, id: u64| a.session.composing_by_id(id).cloned().unwrap();
    let saved_as = |a: &crate::MailApp, id: u64| c(a, id).draft.and_then(|d| a.session.mailbox.message(d).cloned());
    let before = (saved_as(&a, on_imap).unwrap().body, saved_as(&a, here).unwrap().body);
    assert_eq!((c(&a, on_imap).dirty, c(&a, here).dirty, c(&a, clean).dirty), (true, true, false));
    // Not before five minutes are up.
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 299);
    assert!(c(&a, on_imap).dirty && c(&a, here).dirty);
    assert_eq!((saved_as(&a, on_imap).unwrap().body, saved_as(&a, here).unwrap().body), before);
    // Then both are saved to their account's Drafts, as Save Draft saves them, the one put aside
    // longest too; the one without changes isn't touched.
    tick_at(&mut a, start, 300);
    assert_eq!(a.session.compose_id(), Some(clean), "the current one stays current");
    for (id, account, body) in [(on_imap, imap, "One, again"), (here, local, "Two, again")] {
        assert!(!c(&a, id).dirty);
        let m = saved_as(&a, id).unwrap();
        assert!(m.draft && m.body.text.contains(body), "{}", m.body.text);
        assert_eq!(Some(m.folder), a.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Drafts));
    }
    assert_eq!((c(&a, clean).draft, c(&a, clean).dirty), (None, false));
    assert!(!a.session.mailbox.messages.values().any(|m| m.draft && m.account == microsoft));
}

#[test]
fn a_microsoft_accounts_drafts_saved_by_themselves_wait_to_go_up() {
    let mut a = app();
    let start = a.session.now();
    let account = account(&mut a, "one@example.org", "microsoft");
    let first = a.execute("mail.new", json!({"account": account, "subject": "First"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("mail.new", json!({"account": account, "subject": "Second"})).unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    a.execute("compose.set", json!({"compose": first, "body": "One"})).unwrap();
    assert!(a.session.all_composing().all(|c| c.dirty));
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 5 * 60);
    assert!(a.session.all_composing().all(|c| !c.dirty && c.draft.is_some()));
    // Each waits for the next Send/Receive to take it up to the server (R3.4.1).
    for c in a.session.all_composing() {
        let m = c.draft.and_then(|d| a.session.mailbox.message(d)).unwrap();
        assert!(m.server_draft.as_ref().is_some_and(|sd| sd.pending), "{}", m.subject);
    }
    assert!(mailcraft_engine::cmd::account::unpushed(&a.session, account) >= 2);
    // With nothing left to save the five minutes stop; the next change starts them again.
    tick_at(&mut a, start, 6 * 60);
    a.execute("compose.set", json!({"body": "Two, again"})).unwrap();
    tick_at(&mut a, start, 7 * 60);
    tick_at(&mut a, start, 11 * 60);
    assert!(a.session.compose.as_ref().unwrap().dirty);
    tick_at(&mut a, start, 12 * 60);
    assert!(!a.session.compose.as_ref().unwrap().dirty);
}

/// A new message with a body on an account of `kind`, never saved: the app, the clock's start,
/// the message and its subject.
fn new_message(kind: &str, subject: &str) -> (crate::MailApp, i64, u64) {
    let mut a = app();
    let start = a.session.now();
    let account = account(&mut a, "one@mail.example", kind);
    let compose = a.execute("mail.new", json!({"account": account, "subject": subject})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "Not for keeping"})).unwrap();
    assert!(a.session.compose.as_ref().is_some_and(|c| c.dirty && c.draft.is_none()));
    (a, start, compose)
}

fn in_drafts(a: &crate::MailApp, subject: &str) -> bool {
    a.session.mailbox.messages.values().any(|m| m.draft && m.subject == subject)
}

#[test]
fn nothing_is_saved_by_itself_while_the_owner_is_asked_whether_to_save() {
    for kind in ["imap", "local"] {
        let (mut a, start, _) = new_message(kind, "Asked about");
        tick_at(&mut a, start, 0);
        // The owner closes it: Unsaved Message asks.
        a.run("compose.close", json!({})).unwrap();
        assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::DiscardDraft { .. })), "{kind}");
        // Five minutes go by with the question open: nothing is saved behind the answer.
        tick_at(&mut a, start, 5 * 60);
        tick_at(&mut a, start, 6 * 60);
        assert!(!in_drafts(&a, "Asked about"), "{kind}");
        // Don't Save: it was never in Drafts, and isn't now.
        a.dialog = None;
        crate::dialogs::leave_message(&mut a, "discard", "compose.close", json!({}));
        assert!(a.session.compose.is_none() && !in_drafts(&a, "Asked about"), "{kind}");
    }
}

#[test]
fn dont_save_takes_out_of_drafts_what_only_the_background_saves_put_there() {
    for kind in ["imap", "local", "microsoft"] {
        // Saved by itself, then Don't Save.
        let (mut a, start, _) = new_message(kind, "Saved by itself");
        tick_at(&mut a, start, 0);
        tick_at(&mut a, start, 5 * 60);
        assert!(in_drafts(&a, "Saved by itself"), "{kind}");
        a.execute("compose.set", json!({"body": "Still not for keeping"})).unwrap();
        a.run("compose.close", json!({})).unwrap();
        a.dialog = None;
        crate::dialogs::leave_message(&mut a, "discard", "compose.close", json!({}));
        assert!(!in_drafts(&a, "Saved by itself"), "{kind}");
        // Nothing waits to go to a server for it.
        if kind == "microsoft" {
            let account = a.session.mailbox.accounts.iter().find(|x| x.email == "one@mail.example").unwrap().id;
            assert_eq!(mailcraft_engine::cmd::account::unpushed(&a.session, account), 0);
        }
        // A script's (or an agent's) `discard` too.
        let (mut a, start, compose) = new_message(kind, "Thrown away");
        tick_at(&mut a, start, 0);
        tick_at(&mut a, start, 5 * 60);
        assert!(in_drafts(&a, "Thrown away"), "{kind}");
        a.execute("compose.close", json!({"compose": compose, "discard": true})).unwrap();
        assert!(!in_drafts(&a, "Thrown away"), "{kind}");
    }
}

#[test]
fn a_message_saved_on_purpose_stays_in_drafts_whatever_the_background_saves_did() {
    // Saved by itself, then by Save Draft: a draft for good, which Don't Save leaves as last saved.
    let (mut a, start, _) = new_message("imap", "Kept");
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 5 * 60);
    a.execute("compose.set", json!({"body": "Saved on purpose"})).unwrap();
    a.run("compose.saveDraft", json!({})).unwrap();
    a.execute("compose.set", json!({"body": "Then thrown away"})).unwrap();
    tick_at(&mut a, start, 10 * 60);
    a.execute("compose.close", json!({"discard": true})).unwrap();
    let kept = a.session.mailbox.messages.values().find(|m| m.draft && m.subject == "Kept").map(|m| m.body.text.clone());
    assert!(kept.is_some_and(|b| b.contains("Then thrown away")), "as last saved");
    // A draft opened from Drafts: Don't Save after a save by itself leaves it there.
    let mut a = app();
    let start = a.session.now();
    let draft = a.session.mailbox.messages.values().find(|m| m.draft).map(|m| (m.id, m.subject.clone())).unwrap();
    a.execute("mail.editDraft", json!({"id": draft.0})).unwrap();
    a.execute("compose.set", json!({"body": "Changed"})).unwrap();
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 5 * 60);
    a.execute("compose.close", json!({"discard": true})).unwrap();
    assert!(a.session.mailbox.message(draft.0).is_some_and(|m| m.draft));
}

#[test]
fn escape_after_a_background_save_asks_and_dont_save_takes_the_copy_out() {
    for kind in ["imap", "local", "microsoft"] {
        // Saved by itself, then closed without a word typed since: the owner is asked.
        let (mut a, start, _) = new_message(kind, "Closed untouched");
        tick_at(&mut a, start, 0);
        tick_at(&mut a, start, 5 * 60);
        let c = a.session.compose.clone().unwrap();
        assert!(in_drafts(&a, "Closed untouched") && !c.dirty, "{kind}");
        // (What the window's ✕ asks by too.)
        assert!(mailcraft_engine::cmd::compose::unsaved(&a.session, &c), "{kind}");
        a.run("compose.close", json!({})).unwrap();
        assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::DiscardDraft { .. })), "{kind}");
        a.dialog = None;
        crate::dialogs::leave_message(&mut a, "discard", "compose.close", json!({}));
        assert!(a.session.compose.is_none() && !in_drafts(&a, "Closed untouched"), "{kind}");
        // And Undo doesn't bring it back: the background saves are no steps to undo.
        let _ = a.execute("edit.undo", json!({}));
        assert!(!in_drafts(&a, "Closed untouched"), "{kind}: undone");
    }
}

#[test]
fn a_message_saved_by_itself_and_put_aside_unasked_still_goes_with_dont_save() {
    let (mut a, start, first) = new_message("imap", "Put aside");
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 5 * 60);
    // Typed in again, then put aside for New Email: saved unasked, which isn't the owner's choice.
    a.execute("compose.set", json!({"body": "More, not for keeping"})).unwrap();
    a.run("mail.new", json!({})).unwrap();
    assert!(a.session.composing_by_id(first).is_some_and(|c| !c.dirty && c.autosaved));
    a.execute("compose.close", json!({"compose": first, "discard": true})).unwrap();
    assert!(!in_drafts(&a, "Put aside"));
}

#[test]
fn a_new_message_put_aside_unasked_goes_with_dont_save_and_stays_with_save() {
    for kind in ["imap", "local", "microsoft"] {
        // Never saved, then put aside for New Email: saved to Drafts unasked, which isn't the
        // owner's choice.
        let (mut a, _, first) = new_message(kind, "Put aside unasked");
        a.run("mail.new", json!({})).unwrap();
        assert!(in_drafts(&a, "Put aside unasked"), "{kind}");
        assert!(a.session.composing_by_id(first).is_some_and(|c| !c.dirty && c.autosaved), "{kind}");
        // The owner goes back to it, closes it and says Don't Save: out of Drafts again.
        assert!(a.session.show_compose(first));
        a.run("compose.close", json!({})).unwrap();
        assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::DiscardDraft { .. })), "{kind}");
        a.dialog = None;
        crate::dialogs::leave_message(&mut a, "discard", "compose.close", json!({}));
        assert!(a.session.composing_by_id(first).is_none() && !in_drafts(&a, "Put aside unasked"), "{kind}");
        if kind == "microsoft" {
            let account = a.session.mailbox.accounts.iter().find(|x| x.email == "one@mail.example").unwrap().id;
            assert_eq!(mailcraft_engine::cmd::account::unpushed(&a.session, account), 0, "nothing waits to go up");
        }
        // Save on closing keeps it: a draft for good, which a discard later leaves.
        let (mut a, _, first) = new_message(kind, "Put aside, then kept");
        a.run("mail.new", json!({})).unwrap();
        assert!(a.session.show_compose(first));
        a.run("compose.close", json!({})).unwrap();
        a.dialog = None;
        crate::dialogs::leave_message(&mut a, "save", "compose.close", json!({}));
        assert!(a.session.composing_by_id(first).is_none() && in_drafts(&a, "Put aside, then kept"), "{kind}");
        let kept = a.session.mailbox.messages.values().find(|m| m.draft && m.subject == "Put aside, then kept").map(|m| m.id).unwrap();
        a.execute("mail.editDraft", json!({"id": kept})).unwrap();
        a.execute("compose.close", json!({"discard": true})).unwrap();
        assert!(in_drafts(&a, "Put aside, then kept"), "{kind}: kept");
    }
}
