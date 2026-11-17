// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared mailboxes and the calendars shared with the owner as a part of the one question for
//! an older sign-in: what the question says, and File › Info's part. Hosts are fakes; every
//! address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::{Directory, Id};
use serde_json::json;

use crate::dialogs::Dialog;
use crate::task_sync::{TasksJob, TasksResult};
use crate::{MailApp, Services};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "rota@example.org";
const NOW: i64 = 1_791_993_600;

/// A window with one Microsoft account of `directory` whose sign-in doesn't cover its to-dos, and
/// a shared mailbox opened through it.
fn window(directory: Directory) -> (MailApp, Id, Id) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    if let Some(oauth) = s.mailbox.accounts.iter_mut().find(|a| a.id == owner).and_then(|a| a.oauth.as_mut()) {
        oauth.directory = directory;
    }
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    let fresh = mailcraft_model::sync::Tokens {
        access: "own".into(),
        refresh: "ref-1".into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    app.tokens.insert(owner, fresh);
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED, "account": owner})).map(|v| v["id"].as_u64().unwrap_or(0));
    let tasks: Arc<Mutex<VecDeque<TasksResult>>> = Default::default();
    tasks.lock().unwrap().push_back(TasksResult { account: owner, consent_needed: true, ..Default::default() });
    app.services.graph_tasks = Some(Box::new(move |_: TasksJob| {
        let (tx, rx) = mpsc::channel();
        tx.send(tasks.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    (app, owner, shared.unwrap_or(0))
}

fn question_text(app: &MailApp) -> String {
    let Some(Dialog::Confirm { command, ask, .. }) = &app.dialog else { panic!("asked") };
    assert_eq!(command, "ui.allowSync");
    ask.text.clone()
}

#[test]
fn the_one_question_says_its_page_asks_for_the_shared_mailboxes_too_except_of_a_personal_account() {
    for (directory, said) in [(Directory::Organisation, true), (Directory::Unknown, true), (Directory::Personal, false)] {
        let (mut app, owner, _) = window(directory);
        assert!(app.tasks_tick_at(0.0));
        app.poll_tasks();
        app.tasks_tick_at(1.0);
        let text = question_text(&app);
        assert!(text.contains("tasks"), "{text}");
        assert_eq!(text.contains("The same page asks to read and send from the shared mailboxes"), said, "{directory:?}: {text}");
        assert_eq!(app.shared_on_page(owner), said);
    }
}

#[test]
fn an_account_of_unknown_directory_is_personal_when_its_tenant_or_its_address_says_so() {
    let (mut app, owner, _) = window(Directory::Unknown);
    assert!(app.shared_on_page(owner));
    let set = |app: &mut MailApp, tenant: &str, email: &str| {
        if let Some(a) = app.session.mailbox.accounts.iter_mut().find(|a| a.id == owner) {
            a.email = email.into();
            if let Some(oauth) = a.oauth.as_mut() {
                oauth.tenant = tenant.into();
            }
        }
    };
    set(&mut app, "consumers", OWNER);
    assert!(!app.shared_on_page(owner));
    set(&mut app, "", "ada@outlook.com");
    assert!(!app.shared_on_page(owner));
}

#[test]
fn file_infos_part_for_shared_mailboxes_shows_only_while_something_of_them_waits() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    assert!(app.session.mailbox.account(shared).is_some_and(|a| a.is_shared()));
    assert!(app.shared_sync_part(owner).is_none());
    // Mail waits in its Outbox for the permission to send from it.
    assert!(app.session.shared_send_missing(shared).is_some());
    let (line, again, _) = app.shared_sync_part(owner).unwrap();
    assert!(line.contains("waiting for your permission") && again, "{line}");
    // Asked: no Ask again while the question or the page is up.
    assert!(app.ask_sync_again(owner));
    let (line, again, _) = app.shared_sync_part(owner).unwrap();
    assert!(line.contains("asking") && !again, "{line}");
    // Refused by the organisation: the mail is held in its Outbox with why, and File › Info says
    // why with Ask again; nothing of it goes from the owner's address.
    app.dialog = None;
    app.sync_signed_in(owner, Err(crate::favorites_sync::REFUSED_BY_ORGANISATION.into()), None);
    assert!(!app.session.shared_send_waits(shared), "held, not waiting for permission");
    assert!(app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("organisation")));
    // (Its calendars, never ticked, are left alone.)
    assert!(!app.session.driver.calendar.shared_perm.contains_key(&shared));
}

#[test]
fn the_shared_mailboxes_alone_waiting_put_a_question_of_their_own_words() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.session.shared_send_missing(shared);
    assert!(app.ask_shared_send(shared));
    let text = question_text(&app);
    assert!(text.contains(OWNER) && text.contains("shared mailboxes") && !text.contains("tasks"), "{text}");
    assert!(matches!(&app.dialog, Some(Dialog::Confirm { ask, .. }) if ask.title == "Shared Mailboxes and Calendars"));
    let tip = app.sync_tip(owner);
    assert!(tip.contains("shared mailboxes") && !tip.contains("tasks"), "{tip}");
    // The page of a refusal by an administrator's rule names shared mailboxes too.
    app.dialog = None;
    app.sync_signed_in(owner, Err(crate::favorites_sync::NEEDS_APPROVAL.into()), None);
    assert!(app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("reading shared mailboxes and calendars")), "{:?}", app.status_msg);
}
