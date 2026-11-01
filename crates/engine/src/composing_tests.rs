// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Several messages being written at once (#286): each has an id, commands name it (`compose`)
//! or act on the current one, an agent names it once more than one is open, and each keeps its
//! own text undo, its "the owner changed this" mark (#577) and is saved first by itself (#470).

use mailcraft_model::FolderKind;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::composing::MAX_COMPOSING;
use crate::{CmdError, Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Run as an agent's command runs (`Session::agent`), allowed nothing in advance.
fn as_agent(s: &mut Session, id: &str, params: Value) -> Result<Value, CmdError> {
    s.agent = Some(Vec::new());
    let r = s.run(id, &params);
    s.agent = None;
    r
}

/// Open a new message with `subject` and its text, as the owner: its id.
fn open(s: &mut Session, subject: &str) -> u64 {
    let id = run(s, "mail.new", json!({}))["compose"].as_u64().expect("mail.new answers the id of the message it opened");
    run(s, "compose.set", json!({"compose": id, "to": "ada@example.org", "subject": subject, "body": subject}));
    id
}

fn text(s: &Session, id: u64) -> String {
    s.composing_by_id(id).map(|c| c.body.text().to_plain()).unwrap_or_default()
}

fn drafts_with(s: &Session, subject: &str) -> usize {
    s.mailbox.messages.values().filter(|m| m.draft && m.subject == subject).count()
}

#[test]
fn two_are_open_at_once_and_the_last_opened_is_current() {
    let mut s = session();
    let first = open(&mut s, "First");
    let second = open(&mut s, "Second");
    assert_ne!(first, second);
    assert_eq!(s.composing_count(), 2);
    assert_eq!(s.compose_id(), Some(second));
    assert_eq!(s.compose.as_ref().unwrap().subject, "Second");
    // The first is still open as it was, with its changes, saved nowhere: nothing was let go of.
    let c = s.composing_by_id(first).unwrap();
    assert!(c.dirty && c.subject == "First" && text(&s, first).contains("First"));
    assert_eq!(drafts_with(&s, "First"), 0);
    // Replies, forwards and opened drafts open beside them too, each with an id of its own.
    let inbox = s.mailbox.folder_of_kind(s.mailbox.default_account().unwrap().id, FolderKind::Inbox).unwrap();
    let received = s.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.unfetched && !m.incomplete).unwrap().id;
    let mut ids = vec![first, second];
    for (command, params) in
        [("mail.reply", json!({"id": received})), ("mail.replyAll", json!({"id": received})), ("mail.forward", json!({"id": received}))]
    {
        let id = run(&mut s, command, params)["compose"].as_u64().unwrap();
        assert!(!ids.contains(&id), "{command}");
        assert_eq!(s.compose_id(), Some(id), "{command}");
        ids.push(id);
    }
    assert_eq!(s.composing_count(), 5);
}

#[test]
fn a_command_acts_on_the_message_it_names_and_the_current_one_stays_current() {
    let mut s = session();
    let first = open(&mut s, "First");
    let second = open(&mut s, "Second");
    run(&mut s, "compose.set", json!({"compose": first, "subject": "First, again"}));
    run(&mut s, "compose.type", json!({"compose": first, "text": " typed"}));
    run(&mut s, "format.bold", json!({"compose": first, "from": 0, "to": 3}));
    run(&mut s, "compose.highImportance", json!({"compose": first}));
    assert_eq!(s.compose_id(), Some(second), "the named one isn't made current");
    let c = s.composing_by_id(first).unwrap();
    assert_eq!(c.subject, "First, again");
    assert!(text(&s, first).contains("typed"));
    assert_eq!(c.importance, mailcraft_model::Importance::High);
    let c = s.composing_by_id(second).unwrap();
    assert_eq!(c.subject, "Second");
    assert!(!text(&s, second).contains("typed"));
    assert_eq!(c.importance, mailcraft_model::Importance::Normal);
    // Without one, the current one.
    run(&mut s, "compose.set", json!({"subject": "Second, again"}));
    assert_eq!(s.compose.as_ref().unwrap().subject, "Second, again");
    assert_eq!(s.composing_by_id(first).unwrap().subject, "First, again");
    // The host makes another current (the reading pane's, or the window in front).
    assert!(s.show_compose(first));
    assert_eq!(s.compose.as_ref().unwrap().subject, "First, again");
    assert!(!s.show_compose(999));
}

#[test]
fn a_message_named_wrongly_is_refused_and_nothing_changes() {
    let mut s = session();
    let first = open(&mut s, "First");
    let second = open(&mut s, "Second");
    let before = (s.compose.clone(), s.composing.clone());
    for junk in [json!(null), json!("1"), json!(0), json!(-1), json!(1.5), json!([first]), json!({"id": first})] {
        let r = s.run("compose.set", &json!({"compose": junk, "subject": "Wrong"}));
        assert!(matches!(r, Err(CmdError::Params(_))), "{junk}: {r:?}");
    }
    let r = s.run("compose.set", &json!({"compose": 999, "subject": "Wrong"}));
    assert!(r.unwrap_err().to_string().contains("no message being written has the id 999"));
    assert_eq!((s.compose.clone(), s.composing.clone()), before);
    // A closed one is no longer there to name.
    run(&mut s, "compose.close", json!({"compose": first, "discard": true}));
    assert!(s.run("compose.set", &json!({"compose": first, "subject": "Gone"})).is_err());
    assert_eq!(s.compose_id(), Some(second));
}

#[test]
fn an_agent_names_the_message_once_more_than_one_is_open() {
    let mut s = session();
    let first = as_agent(&mut s, "mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    // One open: naming none is that one, as before.
    as_agent(&mut s, "compose.set", json!({"subject": "Only"})).unwrap();
    let second = as_agent(&mut s, "mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    for (command, params) in [
        ("compose.set", json!({"subject": "Which?"})),
        ("compose.type", json!({"text": "Which?"})),
        ("format.bold", json!({})),
        ("compose.undo", json!({})),
        ("compose.saveDraft", json!({})),
        ("compose.send", json!({})),
        ("compose.close", json!({})),
    ] {
        let e = as_agent(&mut s, command, params.clone()).unwrap_err().to_string();
        assert!(e.contains("2 messages are being written") && e.contains("`compose`"), "{command}: {e}");
        assert_eq!(s.composing_count(), 2, "{command}: nothing changed");
    }
    assert_eq!(s.composing_by_id(first).unwrap().subject, "Only");
    // Named, it goes; commands that don't act on a message being written don't need it.
    as_agent(&mut s, "compose.set", json!({"compose": first, "subject": "First"})).unwrap();
    as_agent(&mut s, "compose.set", json!({"compose": second, "subject": "Second"})).unwrap();
    let row = s.rows().first().unwrap().id;
    as_agent(&mut s, "mail.select", json!({"id": row})).unwrap();
    assert_eq!(s.composing_by_id(first).unwrap().subject, "First");
    assert_eq!(s.composing_by_id(second).unwrap().subject, "Second");
    // The owner (the window, a script of theirs) acts on the current one without naming it.
    run(&mut s, "compose.set", json!({"subject": "Current"}));
    assert_eq!(s.compose.as_ref().unwrap().subject, "Current");
}

#[test]
fn each_message_has_its_own_text_undo() {
    let mut s = session();
    let first = open(&mut s, "A");
    let second = open(&mut s, "B");
    run(&mut s, "compose.type", json!({"compose": first, "text": " one"}));
    run(&mut s, "compose.type", json!({"compose": second, "text": " two"}));
    run(&mut s, "compose.undo", json!({"compose": first}));
    assert!(!text(&s, first).contains("one"), "{}", text(&s, first));
    assert!(text(&s, second).contains("two"), "the other's typing stays: {}", text(&s, second));
    run(&mut s, "compose.undo", json!({}));
    assert!(!text(&s, second).contains("two"));
    run(&mut s, "compose.redo", json!({"compose": first}));
    assert!(text(&s, first).contains("one") && !text(&s, second).contains("two"));
}

#[test]
fn each_is_saved_first_by_itself_and_another_mailbox_saves_every_one() {
    let mut s = session();
    let first = open(&mut s, "First");
    let second = open(&mut s, "Second");
    // Closing one saves that one first, and leaves the other open as it is.
    run(&mut s, "compose.close", json!({"compose": first}));
    assert_eq!((drafts_with(&s, "First"), drafts_with(&s, "Second")), (1, 0));
    assert!(s.composing_by_id(first).is_none() && s.composing_by_id(second).unwrap().dirty);
    assert_eq!(s.compose_id(), Some(second));
    // Closing the current one makes the one put aside last current again.
    let third = open(&mut s, "Third");
    assert_eq!(s.compose_id(), Some(third));
    run(&mut s, "compose.close", json!({"discard": true}));
    assert_eq!(s.compose_id(), Some(second));
    assert_eq!(drafts_with(&s, "Third"), 0, "let go of, as asked");
    // Another mailbox closes them all.
    open(&mut s, "Fourth");
    run(&mut s, "file.new", json!({}));
    assert_eq!(s.composing_count(), 0);
}

#[test]
fn another_mailbox_saves_every_message_being_written_first() {
    let mut s = session();
    open(&mut s, "First");
    open(&mut s, "Second");
    let steps = s.history.len();
    // A mailbox that can't be read fails, but what was saved first stays saved.
    assert!(s.run("file.open", &json!({"name": "other.mailcraft", "data": "not a mailbox"})).is_err());
    assert_eq!((drafts_with(&s, "First"), drafts_with(&s, "Second")), (1, 1));
    assert_eq!(s.history.len(), steps + 2, "a Save Draft step each");
    assert_eq!(s.composing_count(), 2, "still open, now saved");
    assert!(s.all_composing().all(|c| !c.dirty));
}

#[test]
fn the_owners_mark_is_kept_by_each_message() {
    let mut s = session();
    let mine = open(&mut s, "Mine");
    assert!(s.compose.as_ref().unwrap().owner_changed);
    // An agent's message opens behind the owner's, which stays where they are writing (#286).
    let agents = as_agent(&mut s, "mail.new", json!({"subject": "The agent's"})).unwrap()["compose"].as_u64().unwrap();
    assert_eq!(s.compose_id(), Some(mine));
    // The agent writes in its own, by its id; not in the owner's (#577).
    as_agent(&mut s, "compose.set", json!({"compose": agents, "body": "By the agent"})).unwrap();
    assert!(!s.composing_by_id(agents).unwrap().owner_changed);
    let e = as_agent(&mut s, "compose.set", json!({"compose": mine, "body": "Overwritten"})).unwrap_err().to_string();
    assert!(e.contains("the owner is writing"), "{e}");
    assert!(text(&s, mine).contains("Mine"));
    // The owner writing in the agent's makes that one theirs too, and only that one.
    run(&mut s, "compose.type", json!({"compose": agents, "text": "and the owner "}));
    assert!(s.composing_by_id(agents).unwrap().owner_changed);
    // The gate names which one a question is about.
    let what = crate::agent::describe(&s, "compose.close", &json!({"compose": agents, "discard": true}));
    assert!(what.contains("\u{201c}The agent's\u{201d}") && what.contains(&format!("(message being written {agents})")), "{what}");
    let what = crate::agent::describe(&s, "compose.send", &json!({"compose": agents}));
    assert!(what.contains("\u{201c}The agent's\u{201d}"), "{what}");
}

#[test]
fn an_agent_closing_its_own_message_leaves_the_owners_alone() {
    let mut s = session();
    let mine = open(&mut s, "Mine");
    let agents = as_agent(&mut s, "mail.new", json!({"subject": "The agent's"})).unwrap()["compose"].as_u64().unwrap();
    assert_eq!(crate::agent::classify(&s, "compose.close", &json!({"compose": agents})), crate::agent::Access::Safe);
    assert_eq!(crate::agent::classify(&s, "compose.close", &json!({"compose": mine})), crate::agent::Access::Asked(crate::agent::Class::Leave));
    as_agent(&mut s, "compose.close", json!({"compose": agents})).unwrap();
    assert_eq!(s.compose_id(), Some(mine));
    assert!(s.compose.as_ref().unwrap().owner_changed && s.compose.as_ref().unwrap().dirty);
}

#[test]
fn an_open_draft_opened_again_is_the_same_message() {
    let mut s = session();
    let id = open(&mut s, "Kept");
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    open(&mut s, "Other");
    let r = run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert_eq!(r["compose"].as_u64(), Some(id));
    assert_eq!(s.compose_id(), Some(id));
    assert_eq!(s.composing_count(), 2);
}

#[test]
fn no_more_than_the_most_are_open_at_once() {
    let mut s = session();
    for n in 0..MAX_COMPOSING {
        run(&mut s, "mail.new", json!({"subject": format!("Message {n}")}));
    }
    assert_eq!(s.composing_count(), MAX_COMPOSING);
    let current = s.compose_id();
    let e = s.run("mail.new", &json!({})).unwrap_err().to_string();
    assert!(e.contains("13 messages are being written already"), "{e}");
    assert!(s.run("mail.reply", &json!({})).is_err());
    assert_eq!((s.composing_count(), s.compose_id()), (MAX_COMPOSING, current), "nothing changed");
    // One closed, there is room again.
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.new", json!({}));
    assert_eq!(s.composing_count(), MAX_COMPOSING);
}
