// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Leaving a message being written (#442): nobody who calls a command is asked, so a message
//! with changes that aren't saved goes to Drafts before a command closes or replaces it; an empty
//! one is let go of; and a save that fails leaves everything as it was.

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, Class, classify};
use crate::{ComposeMode, Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn drafts(s: &Session) -> Id {
    let account = s.mailbox.default_account().unwrap().id;
    s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap()
}

fn drafts_count(s: &Session) -> usize {
    let d = drafts(s);
    s.mailbox.messages.values().filter(|m| m.folder == d).count()
}

/// A message of the Inbox that isn't a draft.
fn received(s: &Session) -> Id {
    let account = s.mailbox.default_account().unwrap().id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.incomplete && !m.unfetched).unwrap().id
}

/// A message the owner has typed in.
fn written(s: &mut Session, subject: &str) {
    run(s, "mail.new", json!({}));
    run(s, "compose.set", json!({"to": "ada@example.org", "subject": subject, "body": "Half a thought"}));
    assert!(s.compose.as_ref().unwrap().dirty);
}

/// The draft in Drafts with `subject`.
fn draft_with(s: &Session, subject: &str) -> Option<Id> {
    let d = drafts(s);
    s.mailbox.messages.values().find(|m| m.folder == d && m.draft && m.subject == subject).map(|m| m.id)
}

/// Every command that closes or replaces the message being written, with its params.
fn leaving(s: &Session, draft: Id) -> Vec<(&'static str, Value)> {
    let other = received(s);
    vec![
        ("compose.close", json!({})),
        ("mail.new", json!({})),
        ("mail.reply", json!({"id": other})),
        ("mail.replyAll", json!({"id": other})),
        ("mail.forward", json!({"id": other})),
        ("mail.forwardAsAttachment", json!({"id": other})),
        ("mail.editDraft", json!({"id": draft})),
    ]
}

/// A draft kept in Drafts to open, and the session with nothing being written.
fn with_a_draft() -> (Session, Id) {
    let mut s = session();
    written(&mut s, "Kept draft");
    run(&mut s, "compose.saveDraft", json!({}));
    run(&mut s, "compose.close", json!({}));
    let d = draft_with(&s, "Kept draft").unwrap();
    (s, d)
}

#[test]
fn a_new_message_with_only_its_signature_is_empty_and_closes_without_a_draft() {
    let (mut s, draft) = with_a_draft();
    for (id, params) in leaving(&s, draft) {
        run(&mut s, "mail.new", json!({}));
        let c = s.compose.as_ref().unwrap();
        assert!(c.signature.is_some(), "the sample account has a signature");
        assert!(!c.dirty, "nothing typed yet");
        let before = drafts_count(&s);
        run(&mut s, id, params);
        assert_eq!(drafts_count(&s), before, "{id}: an empty message is let go of, not kept");
    }
}

#[test]
fn a_new_message_given_its_recipients_and_subject_is_empty_too() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "ada@example.org", "subject": "Given", "body": "Given text"}));
    assert!(!s.compose.as_ref().unwrap().dirty);
    let before = drafts_count(&s);
    run(&mut s, "compose.close", json!({}));
    assert!(s.compose.is_none());
    assert_eq!(drafts_count(&s), before);
}

#[test]
fn a_reply_or_forward_just_opened_is_empty_and_closes_without_a_draft() {
    let (mut s, draft) = with_a_draft();
    let original = received(&s);
    for start in ["mail.reply", "mail.replyAll", "mail.forward", "mail.forwardAsAttachment"] {
        for (id, params) in leaving(&s, draft) {
            run(&mut s, start, json!({"id": original}));
            assert!(!s.compose.as_ref().unwrap().dirty, "{start}: only what it started with");
            let before = drafts_count(&s);
            run(&mut s, id, params);
            assert_eq!(drafts_count(&s), before, "{start} then {id}");
        }
    }
}

#[test]
fn every_command_that_leaves_a_message_with_changes_saves_it_to_drafts_first() {
    let (mut s, draft) = with_a_draft();
    for (n, (id, params)) in leaving(&s, draft).into_iter().enumerate() {
        let subject = format!("Unsaved {n}");
        written(&mut s, &subject);
        let steps = s.history.len();
        run(&mut s, id, params);
        let kept = draft_with(&s, &subject).unwrap_or_else(|| panic!("{id}: the message was dropped"));
        let m = s.mailbox.message(kept).unwrap();
        assert!(m.body.text.contains("Half a thought"), "{id}");
        // What the command was for happened.
        match id {
            "compose.close" => assert!(s.compose.is_none()),
            "mail.editDraft" => assert_eq!(s.compose.as_ref().unwrap().draft, Some(draft)),
            _ => assert!(s.compose.as_ref().is_some_and(|c| !c.dirty && c.subject != subject), "{id}"),
        }
        // The save is a step of its own, Save Draft, and the command added none: undoing what
        // the command did never takes the draft away.
        assert_eq!(s.undo_label(), Some("Save Draft"), "{id}");
        assert_eq!(s.history.len(), steps + 1, "{id}");
    }
}

#[test]
fn a_reply_with_changes_is_saved_as_a_reply() {
    let mut s = session();
    let original = received(&s);
    run(&mut s, "mail.reply", json!({"id": original}));
    run(&mut s, "compose.type", json!({"text": "Thanks, "}));
    assert!(s.compose.as_ref().unwrap().dirty);
    let subject = s.compose.as_ref().unwrap().subject.clone();
    run(&mut s, "mail.new", json!({}));
    let kept = draft_with(&s, &subject).expect("the reply is in Drafts");
    assert!(s.mailbox.message(kept).unwrap().body.text.contains("Thanks,"));
    assert_eq!(s.compose.as_ref().unwrap().mode, ComposeMode::New);
}

#[test]
fn an_opened_draft_with_changes_is_saved_over_itself() {
    let (mut s, draft) = with_a_draft();
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(!s.compose.as_ref().unwrap().dirty, "opened as it was saved");
    run(&mut s, "compose.set", json!({"body": "Rewritten"}));
    let before = drafts_count(&s);
    run(&mut s, "compose.close", json!({}));
    assert_eq!(drafts_count(&s), before, "saved over itself, not a second time");
    assert!(s.mailbox.message(draft).unwrap().body.text.contains("Rewritten"));
}

#[test]
fn discard_lets_the_changes_go_and_keeps_what_was_saved() {
    let (mut s, draft) = with_a_draft();
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    run(&mut s, "compose.set", json!({"body": "Not wanted"}));
    run(&mut s, "compose.close", json!({"discard": true}));
    assert!(s.compose.is_none());
    let m = s.mailbox.message(draft).expect("what was saved stays in Drafts");
    assert!(!m.body.text.contains("Not wanted"));
    // A new message let go of leaves nothing behind.
    written(&mut s, "Thrown away");
    run(&mut s, "compose.close", json!({"discard": true}));
    assert!(s.compose.is_none() && draft_with(&s, "Thrown away").is_none());
}

#[test]
fn a_save_that_fails_fails_the_command_and_changes_nothing() {
    let (mut s, draft) = with_a_draft();
    for (id, params) in leaving(&s, draft) {
        written(&mut s, "Nowhere to go");
        // The account has no Drafts folder any more.
        let d = drafts(&s);
        s.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Custom;
        let (mailbox, compose) = (s.mailbox.clone(), s.compose.clone());
        let r = s.run(id, &params);
        assert!(r.is_err(), "{id}: {r:?}");
        assert!(r.unwrap_err().to_string().contains("Drafts"), "{id}: the reason is said");
        assert_eq!(s.compose, compose, "{id}: the message stays open, as it was");
        assert!(s.mailbox == mailbox, "{id}");
        s.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Drafts;
        run(&mut s, "compose.close", json!({"discard": true}));
    }
}

#[test]
fn an_agent_needs_the_owner_to_throw_away_what_the_owner_wrote() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let discard = json!({"discard": true});
    // Nothing written: nothing to lose.
    assert_eq!(classify(&s, "compose.close", &discard), Access::Safe);
    run(&mut s, "compose.set", json!({"body": "Mine"}));
    assert_eq!(classify(&s, "compose.close", &discard), Access::Owner(Class::Delete));
    assert!(crate::agent::describe(&s, "compose.close", &discard).contains("without saving"));
    // Closing as it is saves it, and is the agent's to do.
    assert_eq!(classify(&s, "compose.close", &json!({})), Access::Safe);
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Safe);
}
