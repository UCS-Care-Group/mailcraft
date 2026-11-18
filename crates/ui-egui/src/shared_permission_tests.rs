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

/// A shared mailbox's calendar the owner did not answer for ends in Declined; Ask again and a
/// page that is given lets it through.
#[test]
fn ask_again_and_a_given_page_unblock_a_declined_shared_calendar_and_keep_the_line_for_it() {
    use mailcraft_engine::sync_driver::SharedPermission as Permission;
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.session.driver.calendar.shared_perm.insert(shared, Permission::Declined);
    assert!(Permission::blocks(app.session.driver.calendar.shared_perm.get(&shared)));
    // File › Info still offers Ask again, with the tasks and settings working (nothing else waits).
    let (_, _, again) = app.sync_line(owner).unwrap();
    assert!(again);
    assert!(app.ask_sync_again(owner));
    assert_eq!(app.session.driver.calendar.shared_perm.get(&shared), Some(&Permission::Asking));
    app.dialog = None;
    app.sync_asking.remove(&owner);
    app.sync_signed_in(owner, Ok(mailcraft_model::sync::Tokens { access: "a".into(), refresh: "r".into(), ..Default::default() }), None);
    assert_eq!(app.session.driver.calendar.shared_perm.get(&shared), Some(&Permission::Allowed));
    assert!(!Permission::blocks(app.session.driver.calendar.shared_perm.get(&shared)));
}

/// A page answered with the shared permissions does not open again by itself when a job still finds
/// them missing; Ask again and the owner's own command still do.
#[test]
fn a_page_already_given_is_not_opened_again_by_itself() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.sync_signed_in(owner, Ok(mailcraft_model::sync::Tokens { access: "a".into(), refresh: "r".into(), ..Default::default() }), None);
    assert!(app.shared_page_given.contains(&owner));
    let result = crate::GraphResult { account: shared, consent_needed: true, ..Default::default() };
    app.finish_shared_mail(shared, false, result);
    assert!(app.signing_in.is_none() && app.dialog.is_none(), "nothing opened by itself");
    assert!(app.status_msg.as_ref().is_none_or(|(said, _)| !said.contains("opened in your browser")));
    assert!(app.ask_sync_again(owner), "Ask again still works");
    assert!(!app.shared_page_given.contains(&owner));
}

/// An Ask again that does not start a sign-in keeps the marker.
#[test]
fn a_failed_ask_again_keeps_the_page_given_marker() {
    let (mut app, owner, _) = window(Directory::Organisation);
    app.sync_signed_in(owner, Ok(mailcraft_model::sync::Tokens { access: "a".into(), refresh: "r".into(), ..Default::default() }), None);
    assert!(app.shared_page_given.contains(&owner));
    if let Some(a) = app.session.mailbox.accounts.iter_mut().find(|a| a.id == owner) {
        a.oauth = None;
    }
    assert!(app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).is_err());
    assert!(app.shared_page_given.contains(&owner));
}

/// A meeting's answer that waits to go from a shared mailbox when the page is refused: kept with
/// the reason, and not asked for again by itself.
#[test]
fn a_refusal_keeps_a_waiting_meeting_answer_with_its_reason() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.session.driver.calendar.send_waits.insert(shared);
    assert!(app.shared_sync_part(owner).is_some());
    app.sync_signed_in(owner, Err("access_denied".into()), None);
    let (line, again, _) = app.shared_sync_part(owner).unwrap();
    assert!(line.contains("not allowed") && again, "{line}");
    assert!(app.session.driver.calendar.send_waits.contains(&shared), "still waiting, nothing goes");
    // (Send/Receive by hand is the retry: it takes the wait off.)
    app.session.driver.calendar.after_round(shared);
    assert!(!app.session.driver.calendar.send_waits.contains(&shared));
}

/// A mailbox being added whose page is cancelled is taken off again, with the reason.
#[test]
fn cancelling_the_page_for_a_shared_mailbox_being_added_ends_the_check() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.shared_checking.insert(shared);
    app.session.driver.calendar.shared_perm.insert(shared, mailcraft_engine::sync_driver::SharedPermission::Needed);
    app.services.sign_in = Some(Box::new(|_: crate::SignInRequest| mpsc::channel().1));
    app.shared_next = Some(shared);
    app.begin_sync_sign_in(owner).unwrap();
    assert!(app.run("ui.cancelSignIn", json!({})).is_ok());
    assert!(!app.shared_checking.contains(&shared), "no longer checking");
    assert!(app.session.mailbox.account(shared).is_none(), "taken off again");
    assert!(app.signing_in.is_none() && !app.sync_asking.contains(&owner));
}

/// The owner's yes while another run is under way: the shared mailbox's run is put in line, not
/// dropped with a busy error.
#[test]
fn a_yes_while_a_run_is_under_way_puts_the_shared_run_in_line() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.graph_rx = Some((owner, false, false, app.session.openings(), mpsc::channel().1));
    app.shared_checking.insert(shared);
    assert_eq!(app.shared_sync_allowed(owner, Some(shared)), None);
    assert_eq!(app.shared_check_next, Some(shared), "a mailbox being added waits for the run to end");
    app.shared_checking.remove(&shared);
    app.shared_check_next = None;
    assert_eq!(app.shared_sync_allowed(owner, Some(shared)), None);
    assert!(app.session.driver.round.is_queued(shared), "in line behind the run");
}

/// What waited in a shared mailbox's Outbox when the page was refused is held with the reason,
/// not sent again by itself; Send Again lets it go.
#[test]
fn a_refused_page_holds_the_outbox_of_the_shared_mailbox_until_send_again() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.session.view.folder = app.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Inbox).unwrap();
    app.execute("mail.new", json!({"to": "bob@example.org", "subject": "One"})).unwrap();
    let id = app.execute("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    app.session.shared_send_missing(shared);
    app.sync_signed_in(owner, Err("access_denied".into()), None);
    assert!(!app.session.shared_send_waits(shared));
    let held = app.session.mailbox.message(id).unwrap().held.clone();
    assert!(!held.is_empty() && held.contains("wasn't allowed"), "{held}");
    assert!(mailcraft_engine::cmd::graph_mail::graph_mail_request(&app.session, shared).unwrap().sends.is_empty());
    app.execute("mail.sendAgain", json!({"id": id})).unwrap();
    assert_eq!(mailcraft_engine::cmd::graph_mail::graph_mail_request(&app.session, shared).unwrap().sends.len(), 1);
}

#[test]
fn a_refused_page_keeps_saying_so_for_a_held_send_without_a_shared_calendar_entry() {
    let (mut app, owner, shared) = window(Directory::Organisation);
    app.session.view.folder = app.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Inbox).unwrap();
    app.execute("mail.new", json!({"to": "bob@example.org", "subject": "One"})).unwrap();
    app.execute("compose.send", json!({})).unwrap();
    app.session.shared_send_missing(shared);
    app.sync_signed_in(owner, Err("access_denied".into()), None);
    assert!(!app.session.shared_send_waits(shared) && !app.session.driver.calendar.shared_perm.contains_key(&shared));
    let (line, again, _) = app.shared_sync_part(owner).unwrap();
    assert!(line.contains("not allowed") && again, "{line}");
    // Allowed later: the line goes.
    app.shared_sync_allowed(owner, None);
    assert!(app.shared_sync_part(owner).is_none());
}
