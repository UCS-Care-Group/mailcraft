// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent doesn't close, replace or throw away the message the owner is writing without asking
//! (#494): a command that would asks the owner first. A message only the agent changed is the
//! agent's to leave, headless too: it goes to Drafts first, as #442 made it.

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, Class, Verdict, answered, check, classify, describe, needs_owner};
use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Run as an agent's command runs (`Session::agent`), allowed nothing in advance.
fn as_agent(s: &mut Session, id: &str, params: Value) -> Value {
    s.agent = Some(Vec::new());
    let r = s.run(id, &params);
    s.agent = None;
    r.unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The owner has typed in a new message and not saved it.
fn writing() -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"to": "ada@example.org", "subject": "Mine", "body": "Half a thought"}));
    assert!(s.compose.as_ref().unwrap().dirty);
    s
}

/// A received message of the Inbox.
fn received(s: &Session) -> Id {
    let account = s.mailbox.default_account().unwrap().id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.incomplete && !m.unfetched).unwrap().id
}

/// A draft in Drafts.
fn a_draft(s: &Session) -> Id {
    let account = s.mailbox.default_account().unwrap().id;
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    s.mailbox.messages.values().find(|m| m.folder == drafts && m.draft).unwrap().id
}

/// A contact with an address to write to.
fn a_contact(s: &Session) -> Id {
    s.mailbox.contacts.values().find(|c| c.address().is_some()).unwrap().id
}

/// The draft in Drafts with `subject`.
fn draft_with(s: &Session, subject: &str) -> Option<Id> {
    s.mailbox.messages.values().find(|m| m.draft && m.subject == subject).map(|m| m.id)
}

/// The commands that close the message being written or put another in its place.
fn leaving(s: &Session) -> Vec<(&'static str, Value)> {
    let other = received(s);
    vec![
        ("compose.close", json!({})),
        ("mail.new", json!({})),
        ("mail.reply", json!({"id": other})),
        ("mail.replyAll", json!({"id": other})),
        ("mail.forward", json!({"id": other})),
        ("mail.forwardAsAttachment", json!({"id": other})),
        ("mail.editDraft", json!({"id": a_draft(s)})),
        ("people.email", json!({"id": a_contact(s)})),
    ]
}

fn with_discard(mut v: Value) -> Value {
    v["discard"] = json!(true);
    v
}

#[test]
fn an_agent_that_throws_the_owners_message_away_is_asked() {
    let s = writing();
    for (id, params) in leaving(&s) {
        let params = with_discard(params);
        assert_eq!(classify(&s, id, &params), Access::Asked(Class::Delete), "{id} {params}");
    }
}

#[test]
fn an_agent_that_closes_the_owners_message_is_asked() {
    let s = writing();
    for (id, params) in leaving(&s) {
        assert_ne!(classify(&s, id, &params), Access::Safe, "{id} {params}: the owner's message would be closed under them");
    }
}

#[test]
fn the_question_names_the_owners_message_and_what_becomes_of_it() {
    let s = writing();
    for (id, params) in leaving(&s) {
        assert_eq!(classify(&s, id, &params), Access::Asked(Class::Leave), "{id}");
        let kept = describe(&s, id, &params);
        assert!(kept.contains("\u{201c}Mine\u{201d}") && kept.contains("goes to Drafts first"), "{id}: {kept}");
        let lost = describe(&s, id, &with_discard(params));
        assert!(lost.contains("\u{201c}Mine\u{201d}") && lost.contains("without saving what the owner wrote"), "{id}: {lost}");
    }
    // And what the agent wants instead.
    let other = received(&s);
    let subject = s.mailbox.message(other).unwrap().subject.clone();
    assert!(describe(&s, "mail.new", &json!({})).starts_with("start a new message in place of"));
    assert!(describe(&s, "mail.reply", &json!({"id": other})).starts_with(&format!("reply to \u{201c}{subject}\u{201d}")));
    assert!(describe(&s, "compose.close", &json!({})).starts_with("close the message"));
}

#[test]
fn what_an_agent_wrote_itself_is_its_own_to_leave() {
    let mut s = session();
    as_agent(&mut s, "mail.new", json!({}));
    as_agent(&mut s, "compose.set", json!({"subject": "The agent's", "body": "Written by the agent"}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.dirty && !c.owner_changed);
    for (id, params) in leaving(&s) {
        assert_eq!(classify(&s, id, &params), Access::Safe, "{id}");
        // Throwing it away needs the owner, as it did (#442), who may allow it in advance.
        let params = with_discard(params);
        assert_eq!(classify(&s, id, &params), Access::Owner(Class::Delete), "{id}");
        assert_eq!(check(&s, id, &params, &[Class::Delete]), Verdict::Run, "{id}");
    }
    // It goes to Drafts first, as before.
    as_agent(&mut s, "mail.new", json!({}));
    assert!(draft_with(&s, "The agent's").is_some());
}

#[test]
fn the_owners_change_makes_it_theirs_until_it_is_closed() {
    let mut s = session();
    as_agent(&mut s, "mail.new", json!({}));
    as_agent(&mut s, "compose.set", json!({"subject": "Both", "body": "By the agent"}));
    // A command of the owner's that doesn't change the message leaves it the agent's.
    let other = received(&s);
    run(&mut s, "mail.select", json!({"id": other}));
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Safe);
    run(&mut s, "compose.type", json!({"text": "and the owner "}));
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Asked(Class::Leave));
    // Saved, by the owner or by the window's own saving every few minutes, it is still theirs.
    run(&mut s, "compose.saveDraft", json!({}));
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Asked(Class::Leave));
    // What the agent does after that doesn't make it the agent's (and it can't change what is
    // written in it, #577).
    s.agent = Some(Vec::new());
    assert!(s.run("compose.set", &json!({"body": "More by the agent"})).is_err());
    s.agent = None;
    as_agent(&mut s, "compose.saveDraft", json!({}));
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Asked(Class::Leave));
    // Closed, the next message is nobody's until somebody writes in it.
    run(&mut s, "compose.close", json!({}));
    as_agent(&mut s, "mail.new", json!({}));
    assert_eq!(classify(&s, "compose.close", &json!({})), Access::Safe);
}

#[test]
fn an_agent_that_saves_the_owners_message_first_is_asked_even_so() {
    let mut s = writing();
    as_agent(&mut s, "compose.saveDraft", json!({}));
    assert!(!s.compose.as_ref().unwrap().dirty);
    let everything = Class::ALL.to_vec();
    for (id, params) in leaving(&s) {
        assert!(matches!(check(&s, id, &params, &everything), Verdict::Ask(Class::Leave, _)), "{id}");
        // Nothing unsaved is lost with `discard`: it stays in Drafts, and that is what is said.
        let params = with_discard(params);
        assert!(matches!(check(&s, id, &params, &everything), Verdict::Ask(Class::Leave, _)), "{id}");
        assert!(describe(&s, id, &params).contains("stays in Drafts as it was last saved"), "{id}");
    }
}

#[test]
fn discard_and_another_mailbox_ask_the_owner_whatever_is_allowed() {
    let s = writing();
    let everything = Class::ALL.to_vec();
    // Discard throws it away, and its draft.
    assert_eq!(classify(&s, "compose.discard", &json!({})), Access::Asked(Class::Delete));
    assert!(matches!(check(&s, "compose.discard", &json!({}), &everything), Verdict::Ask(Class::Delete, _)));
    assert!(describe(&s, "compose.discard", &json!({})).contains("\u{201c}Mine\u{201d} that the owner is writing"));
    assert_eq!(answered(&s, "compose.discard", &json!({}), &[], Class::Delete), Verdict::Run);
    // Another mailbox in view closes it, what isn't saved of it saved to Drafts first (#470), or
    // let go of when the agent says so.
    for (id, params) in [("file.new", json!({})), ("file.open", json!({"name": "other.mailcraft", "data": ""}))] {
        assert_eq!(classify(&s, id, &params), Access::Asked(Class::Mailbox), "{id}");
        match check(&s, id, &params, &everything) {
            Verdict::Ask(Class::Mailbox, what) => assert!(
                what.ends_with("the message \u{201c}Mine\u{201d} that the owner is writing is closed; what isn't saved of it goes to Drafts first"),
                "{id}: {what}"
            ),
            other => panic!("{id}: {other:?}"),
        }
        let mut discard = params.clone();
        discard["discard"] = json!(true);
        match check(&s, id, &discard, &everything) {
            Verdict::Ask(Class::Mailbox, what) => {
                assert!(what.ends_with("the message \u{201c}Mine\u{201d} that the owner is writing is closed without being saved"), "{id}: {what}")
            }
            other => panic!("{id}: {other:?}"),
        }
    }
    // All of it saved: it is closed, no more is said.
    let mut saved = writing();
    saved.run("compose.saveDraft", &json!({})).unwrap();
    match check(&saved, "file.new", &json!({}), &everything) {
        Verdict::Ask(Class::Mailbox, what) => assert!(what.ends_with("\u{201c}Mine\u{201d} that the owner is writing is closed"), "{what}"),
        other => panic!("{other:?}"),
    }
    // An appointment's window with changes that aren't saved, as the window names it.
    let unsaved = json!({"unsaved": {"form": "appointment", "title": "Lunch"}});
    assert!(
        matches!(check(&s, "ui.closeWindow", &unsaved, &everything), Verdict::Ask(Class::Delete, what) if what == "close the appointment \u{201c}Lunch\u{201d} without saving the changes made in it")
    );
    // As #442 had them, for what the owner didn't write in.
    let mut s = session();
    as_agent(&mut s, "mail.new", json!({}));
    as_agent(&mut s, "compose.set", json!({"subject": "The agent's"}));
    assert_eq!(classify(&s, "compose.discard", &json!({})), Access::Owner(Class::Delete));
    assert_eq!(check(&s, "compose.discard", &json!({}), &[Class::Delete]), Verdict::Run);
    assert_eq!(classify(&s, "file.new", &json!({})), Access::Owner(Class::Mailbox));
    assert_eq!(classify(&s, "ui.closeWindow", &json!({})), Access::Owner(Class::Delete));
}

#[test]
fn people_email_saves_the_message_first_or_lets_it_go_as_asked() {
    let mut s = writing();
    let contact = a_contact(&s);
    let address = s.mailbox.contact(contact).unwrap().address().unwrap().email.clone();
    run(&mut s, "people.email", json!({"id": contact}));
    assert!(draft_with(&s, "Mine").is_some(), "saved to Drafts first");
    assert!(s.compose.as_ref().unwrap().to.contains(&address));
    // Let go of, it isn't kept.
    run(&mut s, "compose.set", json!({"subject": "Not kept", "body": "Gone"}));
    run(&mut s, "people.email", json!({"id": contact, "discard": true}));
    assert!(draft_with(&s, "Not kept").is_none());
    assert!(s.compose.as_ref().is_some_and(|c| !c.dirty && c.to.contains(&address)));
}

#[test]
fn nothing_allowed_in_advance_lets_an_agent_leave_the_owners_message() {
    let s = writing();
    assert_eq!(Class::parse("leave"), None);
    let everything = Class::ALL.to_vec();
    for (id, params) in leaving(&s) {
        assert!(matches!(check(&s, id, &params, &everything), Verdict::Ask(Class::Leave, _)), "{id}");
        assert_eq!(answered(&s, id, &params, &[], Class::Leave), Verdict::Run, "{id}");
        // Nor throw it away: `--allow delete` is for mail, never for what the owner wrote.
        let params = with_discard(params);
        assert!(matches!(check(&s, id, &params, &everything), Verdict::Ask(Class::Delete, _)), "{id}");
        assert_eq!(answered(&s, id, &params, &[], Class::Delete), Verdict::Run, "{id}");
        assert!(matches!(answered(&s, id, &params, &everything, Class::Leave), Verdict::Ask(Class::Delete, _)), "{id}");
    }
    // Asked every time: whatever the class, no `--allow` is offered as a way round it.
    let asked = needs_owner("compose.discard", Class::Delete, true, "throw away the message");
    assert!(asked.contains("no `--allow` lets an agent do it") && !asked.contains("--allow delete"), "{asked}");
    let refusal = needs_owner("mail.new", Class::Leave, false, "start a new message");
    assert!(!refusal.contains("mcp --allow") && refusal.contains("every time"), "{refusal}");
}

#[test]
fn a_draft_in_a_window_takes_the_owners_message_too() {
    let s = writing();
    let draft = a_draft(&s);
    assert_eq!(classify(&s, "ui.openWindow", &json!({"message": draft})), Access::Asked(Class::Leave));
    assert!(describe(&s, "ui.openWindow", &json!({"message": draft})).starts_with("open the draft"));
    assert_eq!(classify(&s, "ui.openWindow", &json!({"message": received(&s)})), Access::Safe);
}

#[test]
fn another_mailbox_asks_about_an_appointments_window_with_changes() {
    // Nothing is being written; an appointment's window has changes that aren't saved.
    let s = session();
    let unsaved = json!({"unsaved": {"form": "appointment", "title": "Lunch", "more": 1}});
    for id in ["file.new", "file.open"] {
        assert_eq!(classify(&s, id, &unsaved), Access::Asked(Class::Mailbox), "{id}");
        let what = describe(&s, id, &unsaved);
        assert!(
            what.ends_with("; the appointment \u{201c}Lunch\u{201d} and 1 more window are closed without saving the changes made in them"),
            "{id}: {what}"
        );
        assert!(!what.contains("unsaved"), "{id}: said in words, not as params: {what}");
        assert_eq!(classify(&s, id, &json!({})), Access::Owner(Class::Mailbox), "{id}");
    }
}

#[test]
fn taking_away_an_appointment_whose_window_has_changes_asks_whatever_is_allowed() {
    let s = session();
    let everything = Class::ALL.to_vec();
    let unsaved = json!({"form": "appointment", "title": "Lunch"});
    for (id, mut params) in
        [("calendar.delete", json!({"id": 1})), ("calendar.removeCalendar", json!({"id": 1})), ("edit.undo", json!({})), ("edit.redo", json!({}))]
    {
        assert_ne!(classify(&s, id, &params), Access::Asked(Class::Delete), "{id}: nothing in a window");
        params["unsaved"] = unsaved.clone();
        assert_eq!(classify(&s, id, &params), Access::Asked(Class::Delete), "{id}");
        assert!(
            matches!(check(&s, id, &params, &everything), Verdict::Ask(Class::Delete, what) if what.ends_with("; the appointment \u{201c}Lunch\u{201d} is closed without saving the changes made in it")),
            "{id}"
        );
    }
    // A cancellation to the people invited is the stronger question.
    let params = json!({"id": 1, "send": true, "unsaved": unsaved});
    assert_eq!(classify(&s, "calendar.delete", &params), Access::Asked(Class::Send));
}

#[test]
fn an_agents_command_that_takes_away_an_appointment_the_owner_is_changing_is_undone() {
    let mut s = session();
    let mut events = s.mailbox.events.values().filter(|e| !e.title.is_empty() && e.recurrence.is_none()).map(|e| e.id);
    let (event, other) = (events.next().unwrap(), events.next().unwrap());
    // The window says the owner is changing it; whatever the command, and whatever is allowed.
    s.owner_changing = vec![event];
    s.agent = Some(Class::ALL.to_vec());
    let refused = s.run("calendar.delete", &json!({"id": event}));
    let gone_too = s.run("calendar.delete", &json!({"id": other}));
    s.agent = None;
    let e = refused.unwrap_err().to_string();
    assert!(e.contains("the owner is changing in the window"), "{e}");
    assert!(s.mailbox.event(event).is_some(), "nothing was changed");
    assert!(gone_too.is_ok() && s.mailbox.event(other).is_none(), "another appointment goes as allowed");
    // The owner's own command takes it.
    run(&mut s, "calendar.delete", json!({"id": event}));
    assert!(s.mailbox.event(event).is_none());
}

#[test]
fn a_message_whose_draft_was_taken_away_is_saved_again_before_an_agent_leaves_it() {
    let mut s = writing();
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    // An agent's undo takes the saved draft away; the message stays open with what was written.
    as_agent(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.message(draft).is_none() && !s.compose.as_ref().unwrap().dirty);
    // The question doesn't say it is in Drafts, and `discard` would throw it away.
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Asked(Class::Leave));
    let what = describe(&s, "mail.new", &json!({}));
    assert!(what.ends_with("what isn't saved of it goes to Drafts first"), "{what}");
    assert_eq!(classify(&s, "mail.new", &json!({"discard": true})), Access::Asked(Class::Delete));
    // On the owner's yes it goes to Drafts first.
    s.agent = Some(vec![Class::Leave]);
    let r = s.run("mail.new", &json!({}));
    s.agent = None;
    r.unwrap();
    assert!(draft_with(&s, "Mine").is_some());
}

#[test]
fn moving_the_cursor_in_the_agents_message_doesnt_make_it_the_owners() {
    let mut s = session();
    as_agent(&mut s, "mail.new", json!({}));
    as_agent(&mut s, "compose.set", json!({"subject": "The agent's", "body": "Written by the agent"}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 7}));
    run(&mut s, "compose.select", json!({"from": 3}));
    assert!(!s.compose.as_ref().unwrap().owner_changed);
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Safe);
    // Typing does.
    run(&mut s, "compose.type", json!({"text": "and the owner "}));
    assert_eq!(classify(&s, "mail.new", &json!({})), Access::Asked(Class::Leave));
}
