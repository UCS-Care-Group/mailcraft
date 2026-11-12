// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's yes to an agent's question reaches what the question showed, and nothing else
//! (AU3-24, AU3-25, AU3-23, AU3-20): one row for every command an agent can be asked about, and
//! the cases that were reproduced.

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::{self, Access};
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.auto_sync = false;
    a
}

/// `command` sent by an agent that was allowed nothing in advance: its answer, when it has one.
fn agent(a: &mut MailApp, command: &str, params: Value) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(Vec::new()));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// The sample's ids that the rows name.
struct Ids {
    account: Id,
    inbox: Id,
    deleted: Id,
    outbox: Id,
    message: Id,
    other: Id,
    binned: Id,
    contact: Id,
    contact2: Id,
    task: Id,
    task2: Id,
    event: Id,
    event2: Id,
    rule: Id,
    calendar: Id,
}

fn ids(a: &MailApp) -> Ids {
    let mb = &a.session.mailbox;
    let account = mb.accounts[0].id;
    let kind = |k| mb.folder_of_kind(account, k).unwrap();
    let (inbox, deleted) = (kind(FolderKind::Inbox), kind(FolderKind::Deleted));
    let mut inboxed = mb.messages.values().filter(|m| m.folder == inbox).map(|m| m.id);
    let (message, other) = (inboxed.next().unwrap(), inboxed.next().unwrap());
    let contacts: Vec<Id> = mb.contacts.keys().copied().take(2).collect();
    let tasks: Vec<Id> = mb.tasks.keys().copied().take(2).collect();
    let events: Vec<Id> = mb.events.keys().copied().take(2).collect();
    Ids {
        account,
        inbox,
        deleted,
        outbox: kind(FolderKind::Outbox),
        message,
        other,
        binned: mb.messages.values().find(|m| m.folder == deleted).unwrap().id,
        contact: contacts[0],
        contact2: contacts[1],
        task: tasks[0],
        task2: tasks[1],
        event: events[0],
        event2: events[1],
        rule: mb.rules[0].id,
        calendar: mb.calendars.first().map(|c| c.id).unwrap_or(0),
    }
}

/// `text` with the sample's ids put in for `$name`, as JSON.
fn params(text: &str, i: &Ids) -> Value {
    let mut t = text.to_string();
    for (k, v) in [
        ("$account", i.account),
        ("$inbox", i.inbox),
        ("$deleted", i.deleted),
        ("$outbox", i.outbox),
        ("$message", i.message),
        ("$binned", i.binned),
        ("$contact2", i.contact2),
        ("$contact", i.contact),
        ("$task", i.task),
        ("$event", i.event),
        ("$rule", i.rule),
        ("$calendar", i.calendar),
    ] {
        t = t.replace(k, &v.to_string());
    }
    serde_json::from_str(&t).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// What the owner does first, for a row whose question needs it.
#[derive(Clone, Copy)]
enum Setup {
    None,
    /// A message being written, with recipients and an attachment, that the owner wrote in.
    Writing,
    /// A meeting in a window of its own.
    Meeting,
    /// A message in a window of its own.
    Window,
    /// A folder of the owner's, in view.
    Folder,
    /// A change to the Safe Senders to undo, or (`true`) to redo.
    Step(bool),
    /// The account signs in with Microsoft, and the selected to-do is in its To Do list shared
    /// with other people, with a category.
    Shared,
}

/// One row for every command an agent can be asked about: its params in the sample mailbox and
/// what the owner does first. (The commands of the calendar that write to people when `send` is
/// true have a row each too, [`rows`].)
const ROWS: &[(&str, &str, Setup)] = &[
    // Send.
    ("compose.send", "{}", Setup::Writing),
    ("account.setAutomaticReplies", r#"{"status": "disabled"}"#, Setup::None),
    ("mail.sendAgain", r#"{"id": $message}"#, Setup::None),
    ("ui.downloadPictures", "{}", Setup::None),
    ("ui.retryPictures", "{}", Setup::None),
    ("ui.joinMeeting", r#"{"event": $event}"#, Setup::None),
    ("ui.reportProblem", r#"{"what": "x"}"#, Setup::None),
    // Delete.
    ("mail.deletePermanently", r#"{"ids": [$message]}"#, Setup::None),
    ("mail.removeAttachment", r#"{"id": $message, "index": 0}"#, Setup::None),
    ("folder.delete", "{}", Setup::Folder),
    ("folder.empty", r#"{"id": $deleted}"#, Setup::None),
    ("people.delete", "{}", Setup::None),
    ("people.merge", r#"{"keep": $contact, "others": [$contact2]}"#, Setup::None),
    ("tasks.delete", "{}", Setup::None),
    ("category.remove", r#"{"name": "Purple category"}"#, Setup::None),
    ("compose.discard", "{}", Setup::Writing),
    ("ui.closeWindow", "{}", Setup::Window),
    // Accounts.
    ("account.add", r#"{"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}"#, Setup::None),
    ("account.edit", r#"{"id": $account, "name": "X"}"#, Setup::None),
    ("account.remove", r#"{"id": $account}"#, Setup::None),
    ("account.forgetEwsCertificate", r#"{"id": $account}"#, Setup::None),
    ("account.graphSwitch", r#"{"account": $account}"#, Setup::None),
    ("account.addSharedMailbox", r#"{"address": "shared@example.org"}"#, Setup::None),
    ("account.removeSharedMailbox", r#"{"address": "shared@example.org"}"#, Setup::None),
    ("account.thinOffline", r#"{"account": $account}"#, Setup::None),
    ("ui.keepOffline", "{}", Setup::None),
    ("ui.connectMicrosoft", "{}", Setup::None),
    ("ui.automaticRepliesSignIn", r#"{"account": $account}"#, Setup::None),
    ("ui.connectFavorites", "{}", Setup::None),
    ("ui.allowSharedSend", r#"{"account": $account}"#, Setup::None),
    ("ui.allowSharedCalendars", r#"{"account": $account, "confirmed": true}"#, Setup::None),
    ("ui.allowTasks", r#"{"account": $account, "confirmed": true}"#, Setup::None),
    ("ui.allowRules", r#"{"account": $account, "confirmed": true}"#, Setup::None),
    ("ui.allowAutomaticReplies", r#"{"account": $account, "confirmed": true}"#, Setup::None),
    ("ui.adminConsent", "{}", Setup::None),
    ("ui.signInAgain", "{}", Setup::None),
    ("ui.microsoftClientId", r#"{"value": "x"}"#, Setup::None),
    ("ui.rememberSignIn", "{}", Setup::None),
    ("ui.graphSwitch", "{}", Setup::None),
    ("ui.switchToGraph", "{}", Setup::None),
    ("ui.confirmServers", "{}", Setup::None),
    ("ui.sync", r#"{"password": "x"}"#, Setup::None),
    // Calendars.
    ("calendar.removeCalendar", r#"{"id": $calendar}"#, Setup::None),
    ("calendar.openShared", r#"{"address": "x@example.org"}"#, Setup::None),
    ("calendar.removeShared", r#"{"address": "x@example.org"}"#, Setup::None),
    ("calendar.hideListedCopy", r#"{"address": "x@example.org"}"#, Setup::None),
    ("calendar.moveToAccount", "{}", Setup::None),
    ("ui.checkCalendar", "{}", Setup::None),
    ("calendar.forward", r#"{"to": "x@example.org"}"#, Setup::None),
    ("calendar.propose", "{}", Setup::None),
    ("calendar.delete", "{}", Setup::None),
    ("calendar.deleteAnsweredRequests", "{}", Setup::None),
    ("people.meeting", r#"{"send": true}"#, Setup::None),
    // The mailbox file.
    ("file.new", "{}", Setup::None),
    ("file.open", r#"{"data": "e30=", "name": "x.mailcraft"}"#, Setup::None),
    ("file.save", "{}", Setup::None),
    ("file.saveAs", "{}", Setup::None),
    // Options, rules, junk, signatures.
    ("mail.dismissWarnings", r#"{"ids": [$message]}"#, Setup::None),
    ("file.options", "{}", Setup::None),
    ("sync.offline", "{}", Setup::None),
    ("rules.add", r#"{"name": "x"}"#, Setup::None),
    ("rules.remove", r#"{"id": $rule}"#, Setup::None),
    ("rules.enable", r#"{"id": $rule}"#, Setup::None),
    ("rules.list", "{}", Setup::None),
    ("rules.run", "{}", Setup::None),
    ("rules.fromMessage", "{}", Setup::None),
    ("rules.move", r#"{"id": $rule}"#, Setup::None),
    ("rules.keepOnServer", r#"{"id": $rule}"#, Setup::None),
    ("junk.blockSender", "{}", Setup::None),
    ("junk.neverBlockSender", "{}", Setup::None),
    ("junk.neverBlockDomain", "{}", Setup::None),
    ("junk.add", r#"{"list": "safe", "entry": "a@example.org"}"#, Setup::None),
    ("junk.remove", r#"{"list": "safe", "entry": "a@example.org"}"#, Setup::None),
    ("junk.list", r#"{"list": "safe"}"#, Setup::None),
    ("keys.import", r#"{"data": "AAAA"}"#, Setup::None),
    ("keys.remove", r#"{"address": "a@example.org", "fingerprint": "00"}"#, Setup::None),
    ("signature.set", r#"{"name": "x", "text": "y"}"#, Setup::None),
    ("signature.remove", r#"{"name": "Standard"}"#, Setup::None),
    ("signature.rename", r#"{"from": "Standard", "to": "y"}"#, Setup::None),
    ("signature.defaults", "{}", Setup::None),
    ("signature.picture.add", "{}", Setup::None),
    ("signature.picture.set", "{}", Setup::None),
    ("signature.picture.remove", "{}", Setup::None),
    ("signature.layout", "{}", Setup::None),
    ("signature.details", "{}", Setup::None),
    ("signature.html", "{}", Setup::None),
    ("signature.open", "{}", Setup::None),
    ("signature.save", "{}", Setup::None),
    ("signature.close", "{}", Setup::None),
    ("ui.alwaysDownloadPictures", "{}", Setup::None),
    ("ui.downloadPicturesAutomatically", "{}", Setup::None),
    ("ui.senderImages", "{}", Setup::None),
    ("ui.desktopAlerts", r#"{"newMail": true}"#, Setup::None),
    ("ui.repliesInWindow", "{}", Setup::None),
    ("ui.updateCheck", "{}", Setup::None),
    ("ui.updateInstall", "{}", Setup::None),
    ("ui.dismissUpdateNotice", "{}", Setup::None),
    ("ui.dismissSettingsNotice", "{}", Setup::None),
    ("ui.dismissSoftwareNotice", "{}", Setup::None),
    // What is asked about only now and then (`agent::classify`).
    ("edit.undo", "{}", Setup::Step(false)),
    ("edit.redo", "{}", Setup::Step(true)),
    ("ui.eventWindow", r#"{"do": "send"}"#, Setup::Meeting),
    ("mail.move", r#"{"ids": [$message], "folder": $outbox}"#, Setup::None),
    ("mail.copy", r#"{"ids": [$message], "folder": $outbox}"#, Setup::None),
    ("mail.delete", r#"{"ids": [$binned]}"#, Setup::None),
    ("file.import", r#"{"data": "U3ViamVjdDogeA0KDQp5DQo=", "name": "x.eml", "folder": $outbox}"#, Setup::None),
    ("compose.close", "{}", Setup::Writing),
    ("ui.dialog", r#"{"name": "accountSettings", "page": "certificate"}"#, Setup::None),
    // To Do that a Microsoft account keeps, and a list of it shared with other people.
    ("tasks.edit", r#"{"title": "x"}"#, Setup::Shared),
    ("tasks.complete", "{}", Setup::Shared),
    ("tasks.categorize", r#"{"category": "Red category"}"#, Setup::Shared),
    ("tasks.new", r#"{"title": "x", "account": $account}"#, Setup::Shared),
    ("tasks.fromMessage", "{}", Setup::Shared),
    ("category.rename", r#"{"from": "Shared", "to": "y"}"#, Setup::Shared),
];

/// Asked about only for a dialog with changes the owner hasn't saved, which a test can't fill in
/// here: its question is bound as every other's, and the table holds its place.
const NOT_IN_THE_SAMPLE: &[&str] = &["ui.closeDialog"];

/// Every row: [`ROWS`], and each command of the calendar asked about when it writes to people.
fn rows(a: &MailApp) -> Vec<(String, String, Setup)> {
    let mut rows: Vec<(String, String, Setup)> = ROWS.iter().map(|(id, p, s)| (id.to_string(), p.to_string(), *s)).collect();
    for c in a.session.registry.all() {
        if c.id.starts_with("calendar.") && agent::access(c.id) == Some(Access::Safe) && !rows.iter().any(|r| r.0 == c.id) {
            rows.push((c.id.to_string(), r#"{"send": true}"#.to_string(), Setup::None));
        }
    }
    rows
}

/// Every command an agent may be asked about: each one with a class that needs the owner, and
/// each one said to be about something ([`agent::bind::object`]), which those asked about only
/// now and then are.
fn asked_about(a: &MailApp) -> BTreeSet<String> {
    let ids = a.session.registry.all().iter().map(|c| c.id).chain(crate::UI_COMMANDS.iter().map(|c| c.0));
    ids.filter(|id| matches!(agent::access(id), Some(Access::Owner(_) | Access::Asked(_))) || agent::bind::object(id).is_some())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_command_an_agent_is_asked_about_says_what_it_is_about_and_has_a_row() {
    let a = app();
    let unbound: Vec<String> = asked_about(&a).into_iter().filter(|id| agent::bind::object(id).is_none()).collect();
    assert!(unbound.is_empty(), "say what these are about in agent::bind::object: {unbound:?}");
    let mut have: BTreeSet<String> = rows(&a).into_iter().map(|r| r.0).collect();
    have.extend(NOT_IN_THE_SAMPLE.iter().map(|s| s.to_string()));
    let want = asked_about(&a);
    let missing: Vec<&String> = want.difference(&have).collect();
    let extra: Vec<&String> = have.difference(&want).collect();
    assert!(missing.is_empty(), "give these a row in ROWS: {missing:?}");
    assert!(extra.is_empty(), "these rows are for commands an agent isn't asked about: {extra:?}");
}

#[test]
fn a_command_that_isnt_said_to_be_about_anything_is_never_asked_about() {
    // Not a command of the program: refused before it is classed. And no command of the program
    // asks without saying what it is about (the test above).
    let a = app();
    for id in asked_about(&a) {
        assert!(agent::bind::object(&id).is_some(), "{id}");
    }
}

fn set_up(a: &mut MailApp, setup: Setup, i: &Ids) {
    match setup {
        Setup::None => {}
        Setup::Writing => {
            a.execute("mail.new", json!({"to": "alice@example.org", "subject": "Audit lunch"})).unwrap();
            a.execute("compose.attach", json!({"name": "notes.txt", "data": "AAAA"})).unwrap();
        }
        Setup::Meeting => {
            a.execute("ui.openWindow", json!({"new": "meeting", "title": "Review", "attendees": "alice@example.org"})).unwrap();
        }
        Setup::Window => {
            a.execute("ui.openWindow", json!({"message": i.message})).unwrap();
        }
        Setup::Folder => {
            let made = a.execute("folder.new", json!({"name": "Old", "parent": i.inbox})).unwrap();
            let id = made["id"].as_u64().unwrap();
            a.execute("folder.select", json!({"id": id})).unwrap();
        }
        Setup::Shared => {
            let mb = &mut a.session.mailbox;
            mb.accounts[0].oauth = Some(mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Microsoft, ..Default::default() });
            mb.shared_task_lists.insert(i.account, ["L1".to_string()].into());
            let t = mb.task_mut(i.task).unwrap();
            t.origin.account = i.account;
            t.origin.id = "t1".to_string();
            t.list = "L1".to_string();
            t.categories = vec!["Shared".to_string()];
        }
        Setup::Step(redo) => {
            a.execute("junk.add", json!({"list": "safe", "entry": "z@example.org"})).unwrap();
            if redo {
                a.execute("edit.undo", json!({})).unwrap();
            }
        }
    }
}

/// What an agent does while the question is up, each with commands that run at once: it selects
/// other things, puts another message in view, writes to the message being written, opens a
/// meeting in a window and moves mail into the folder in view.
fn meanwhile(a: &mut MailApp, i: &Ids) {
    let _ = agent(a, "mail.select", json!({"id": i.other}));
    let _ = agent(a, "people.select", json!({"id": i.contact2}));
    let _ = agent(a, "tasks.select", json!({"id": i.task2}));
    let _ = agent(a, "calendar.select", json!({"id": i.event2}));
    let _ = agent(a, "compose.set", json!({"to": "mallory@example.org", "bcc": "eve@example.org"}));
    let _ = agent(a, "ui.openWindow", json!({"new": "meeting", "title": "Other", "attendees": "mallory@example.org"}));
    let _ = agent(a, "folder.select", json!({"id": i.inbox}));
}

#[test]
fn a_yes_reaches_nothing_but_what_the_question_showed_for_every_command() {
    let mut not_asked = Vec::new();
    let mut reached = Vec::new();
    let mut a = app();
    for (id, text, setup) in rows(&a) {
        a = app();
        let i = ids(&a);
        // The selection the commands without an object fall back on, as the owner left it.
        for (command, p) in [
            ("mail.select", json!({"id": i.message})),
            ("people.select", json!({"id": i.contact})),
            ("tasks.select", json!({"id": i.task})),
            ("calendar.select", json!({"id": i.event})),
        ] {
            a.execute(command, p).unwrap();
        }
        set_up(&mut a, setup, &i);
        let (now, rx) = agent(&mut a, &id, params(&text, &i));
        if now.is_some() || a.asking.is_empty() {
            not_asked.push(format!("{id}: {now:?}"));
            continue;
        }
        meanwhile(&mut a, &i);
        let before = a.session.mailbox.clone();
        // The question the agent asked first is the one on screen.
        answer(&mut a, true);
        let r = rx.try_recv().unwrap();
        let refused = r["ok"] == false && r["error"].as_str().is_some_and(|e| e.contains("changed since the owner was asked"));
        if !refused || a.session.mailbox != before {
            reached.push(format!("{id}: {r}"));
        }
    }
    assert!(not_asked.is_empty(), "these weren't asked about in the sample (give them params or a setup): {not_asked:#?}");
    assert!(reached.is_empty(), "a yes reached what the question didn't show: {reached:#?}");
}

#[test]
fn deleted_items_emptied_with_messages_moved_in_meanwhile_takes_only_those_shown() {
    let mut a = app();
    let i = ids(&a);
    let shown: Vec<Id> = a.session.mailbox.messages_in(i.deleted).map(|m| m.id).collect();
    assert!(!shown.is_empty());
    let (now, rx) = agent(&mut a, "folder.empty", json!({"id": i.deleted}));
    assert!(now.is_none(), "asked: {now:?}");
    let moved: Vec<Id> = a.session.mailbox.messages_in(i.inbox).map(|m| m.id).take(3).collect();
    let (r, _) = agent(&mut a, "mail.move", json!({"ids": moved, "folder": i.deleted}));
    let r = r.unwrap();
    assert_eq!(r["ok"], true, "the agent's move runs at once: nothing is locked: {r}");
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(shown.iter().all(|m| a.session.mailbox.message(*m).is_none()), "what was shown is gone");
    assert!(moved.iter().all(|m| a.session.mailbox.message(*m).is_some_and(|m| m.folder == i.deleted)), "what came since stays");
}

#[test]
fn deleted_items_emptied_as_shown_takes_everything_in_it() {
    let mut a = app();
    let i = ids(&a);
    let (_, rx) = agent(&mut a, "folder.empty", json!({"id": i.deleted}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(a.session.mailbox.count(i.deleted), 0);
}

#[test]
fn a_folder_deleted_with_messages_moved_into_it_meanwhile_isnt_deleted() {
    let mut a = app();
    let i = ids(&a);
    let made = a.execute("folder.new", json!({"name": "Old", "parent": i.inbox})).unwrap()["id"].as_u64().unwrap();
    let (now, rx) = agent(&mut a, "folder.delete", json!({"id": made}));
    assert!(now.is_none());
    agent(&mut a, "mail.move", json!({"ids": [i.message], "folder": made}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox.folder(made).is_some());
    assert_eq!(a.session.mailbox.message(i.message).unwrap().folder, made);
}

#[test]
fn deleted_items_emptied_with_a_folder_moved_in_meanwhile_keeps_that_folder() {
    let mut a = app();
    let i = ids(&a);
    let keep = a.execute("folder.new", json!({"name": "Keep", "parent": i.inbox})).unwrap()["id"].as_u64().unwrap();
    let (now, rx) = agent(&mut a, "folder.empty", json!({"id": i.deleted}));
    assert!(now.is_none(), "asked: {now:?}");
    let (r, _) = agent(&mut a, "folder.move", json!({"id": keep, "parent": i.deleted}));
    assert_eq!(r.unwrap()["ok"], true);
    assert_eq!(a.session.mailbox.folder(keep).unwrap().parent, Some(i.deleted));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(a.session.mailbox.folder(keep).is_some(), "a folder that wasn't shown stays");
}

#[test]
fn a_folder_deleted_with_a_folder_moved_into_it_meanwhile_isnt_deleted() {
    let mut a = app();
    let i = ids(&a);
    let old = a.execute("folder.new", json!({"name": "Old", "parent": i.inbox})).unwrap()["id"].as_u64().unwrap();
    let keep = a.execute("folder.new", json!({"name": "Keep", "parent": i.inbox})).unwrap()["id"].as_u64().unwrap();
    // In Deleted Items, deleting it is for good (and is sent to the server).
    a.execute("folder.delete", json!({"id": old})).unwrap();
    assert_eq!(a.session.mailbox.folder(old).unwrap().parent, Some(i.deleted));
    let (now, rx) = agent(&mut a, "folder.delete", json!({"id": old}));
    assert!(now.is_none());
    let (r, _) = agent(&mut a, "folder.move", json!({"id": keep, "parent": old}));
    assert_eq!(r.unwrap()["ok"], true);
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox.folder(old).is_some());
    assert_eq!(a.session.mailbox.folder(keep).unwrap().parent, Some(old));
}

#[test]
fn a_contact_deleted_from_the_selection_is_the_one_shown_or_none() {
    let mut a = app();
    let i = ids(&a);
    a.execute("people.select", json!({"id": i.contact})).unwrap();
    let (_, rx) = agent(&mut a, "people.delete", json!({}));
    agent(&mut a, "people.select", json!({"id": i.contact2}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox.contacts.contains_key(&i.contact) && a.session.mailbox.contacts.contains_key(&i.contact2));
    // As shown: that one goes.
    let (_, rx) = agent(&mut a, "people.delete", json!({}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(!a.session.mailbox.contacts.contains_key(&i.contact2) && a.session.mailbox.contacts.contains_key(&i.contact));
}

#[test]
fn a_sender_put_on_the_safe_senders_is_the_one_shown() {
    let mut a = app();
    let i = ids(&a);
    a.execute("mail.select", json!({"id": i.message})).unwrap();
    let (_, rx) = agent(&mut a, "junk.neverBlockSender", json!({}));
    agent(&mut a, "mail.select", json!({"id": i.other}));
    let before = a.session.mailbox.clone();
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox == before, "no list changed");
}

#[test]
fn rules_run_over_the_folder_shown() {
    let mut a = app();
    let i = ids(&a);
    a.execute("folder.select", json!({"id": i.inbox})).unwrap();
    let (_, rx) = agent(&mut a, "rules.run", json!({}));
    agent(&mut a, "folder.select", json!({"id": i.deleted}));
    let before = a.session.mailbox.clone();
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox == before);
}

/// Mail in the Outbox or Sent Items with this subject.
fn sent(a: &MailApp, subject: &str) -> Vec<mailcraft_model::Message> {
    let out: Vec<Id> = a.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
    a.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == subject).map(|m| (**m).clone()).collect()
}

#[test]
fn a_message_sent_is_the_one_the_question_showed_whole() {
    // Whatever changed in it while the question was up, shown in the question or not (the
    // thirteenth recipient, Reply-To, the text), nothing is sent.
    let changes: [(&str, Value); 6] = [
        ("compose.set", json!({"to": "mallory@example.org"})),
        ("compose.set", json!({"bcc": "eve@example.org"})),
        ("compose.replyTo", json!({"value": "eve@example.org"})),
        ("compose.attach", json!({"name": "id_ed25519", "data": "AAAA"})),
        ("compose.type", json!({"text": " and more"})),
        ("compose.highImportance", json!({})),
    ];
    for (command, p) in changes {
        let mut a = app();
        agent_writes(&mut a);
        let (now, rx) = agent(&mut a, "compose.send", json!({}));
        assert!(now.is_none(), "asked");
        let (r, _) = agent(&mut a, command, p.clone());
        let r = r.unwrap();
        assert_eq!(r["ok"], true, "{command} runs at once: {r}");
        answer(&mut a, true);
        assert_eq!(rx.try_recv().unwrap()["ok"], false, "{command} {p}");
        assert!(sent(&a, "Audit lunch").is_empty(), "{command} {p}: nothing sent");
    }
    // Unchanged, it goes.
    let mut a = app();
    agent_writes(&mut a);
    let (_, rx) = agent(&mut a, "compose.send", json!({}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(sent(&a, "Audit lunch").len(), 1);
}

/// A message the agent itself writes, with a recipient and an attachment: the agent may change it
/// (one the owner wrote in is the owner's until it is closed).
fn agent_writes(a: &mut MailApp) {
    let (r, _) = agent(a, "mail.new", json!({"to": "alice@example.org", "subject": "Audit lunch"}));
    assert_eq!(r.unwrap()["ok"], true);
    let (r, _) = agent(a, "compose.attach", json!({"name": "notes.txt", "data": "AAAA"}));
    assert_eq!(r.unwrap()["ok"], true);
}

#[test]
fn a_meeting_sent_is_the_one_in_the_window_the_question_named() {
    let mut a = app();
    let i = ids(&a);
    set_up(&mut a, Setup::Meeting, &i);
    let (now, rx) = agent(&mut a, "ui.eventWindow", json!({"do": "send"}));
    assert!(now.is_none(), "asked");
    assert!(a.asking.front().unwrap().what.contains("alice@example.org"));
    let (r, _) = agent(&mut a, "ui.openWindow", json!({"new": "meeting", "title": "Other", "attendees": "mallory@example.org"}));
    let second = r.unwrap()["result"]["window"].as_u64().unwrap();
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    for w in 1..=second {
        assert!(!crate::windows::told(&a, w), "window {w} isn't told to send");
    }
    // Asked about the one in front, unchanged, it is told.
    let (_, rx) = agent(&mut a, "ui.eventWindow", json!({"do": "send"}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(crate::windows::told(&a, second));
}
