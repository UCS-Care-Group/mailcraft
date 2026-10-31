// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies in the window: a job reads them; a change that finds the
//! sign-in doesn't cover changing them asks the owner, by itself, and goes once he allows it;
//! nothing is changed before they are read, nothing goes while offline, a read that keeps failing
//! backs off, and a new sign-in lets them be read again. Hosts are fakes; every address is
//! invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience};
use mailcraft_model::sync::Tokens;
use serde_json::json;

use crate::auto_replies::{Again, AutoRepliesJob, AutoRepliesResult, Draft, Permission, Trouble};
use crate::dialogs::Dialog;
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "ada@example.org";
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(Id, Option<AutoReplies>)>>>;
type Answers = Arc<Mutex<VecDeque<AutoRepliesResult>>>;
type SignIns = Arc<Mutex<Vec<(Id, bool, bool)>>>;

fn tokens(refresh: &str) -> Tokens {
    Tokens { access: "a".into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn window() -> (MailApp, Id, Answers, Jobs, SignIns) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = true;
    let added = app.execute("account.add", json!({"email": OWNER, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let owner = added["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("ref-1"));
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph_auto_replies = Some(Box::new(move |job: AutoRepliesJob| {
        log.lock().unwrap().push((job.account.id, job.change.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.replies, req.tasks));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(tokens("ref-2"))).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

fn away() -> AutoReplies {
    AutoReplies {
        status: Some(AutoReplyStatus::AlwaysEnabled),
        internal_reply_message: Some("<p>Away this week.</p>".into()),
        external_reply_message: Some("<p>Out of the office.</p>".into()),
        external_audience: Some(ExternalAudience::All),
        ..Default::default()
    }
}

/// The window with what the server has read in.
fn read(app: &mut MailApp, owner: Id, answers: &Answers) {
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    app.poll_auto_replies();
}

fn said(app: &MailApp) -> String {
    app.status_msg.clone().unwrap_or_default().0
}

#[test]
fn a_job_reads_them_and_the_status_bar_says_they_are_on() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    assert!(!app.auto_replies_tick_at(1.0), "one at a time");
    app.poll_auto_replies();
    assert_eq!(jobs.lock().unwrap()[0], (owner, None));
    assert_eq!(app.auto_replies_on(), vec![owner]);
    let (line, again) = app.auto_replies_line(owner).unwrap();
    assert!(again.is_none() && line.contains("on, until you turn them off"), "{line}");
    // Not again for a while.
    assert!(!app.auto_replies_tick_at(60_000.0));
}

/// P2: OK before the read landed wrote empty replies over the server's.
#[test]
fn nothing_is_changed_before_the_servers_setting_is_here() {
    let (mut app, owner, _, jobs, _) = window();
    // The dialog opens while nothing is known: nothing in it can be changed, nor OK pressed.
    let draft = Draft::open(&mut app, &json!({"account": owner})).unwrap();
    assert!(!draft.filled && !draft.editable(&app));
    // And a change asked for anyway (a script, an agent) is refused, with no job.
    let before = jobs.lock().unwrap().len();
    let refused = app
        .execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled", "internal": "", "external": "", "audience": "all"}));
    assert!(refused.unwrap_err().contains("haven't been read"));
    assert_eq!(jobs.lock().unwrap().len(), before);
    // A read that failed leaves it so.
    app.poll_auto_replies();
    assert!(!draft.editable(&app));
}

#[test]
fn the_dialog_can_be_used_once_the_setting_is_read() {
    let (mut app, owner, answers, _, _) = window();
    read(&mut app, owner, &answers);
    let draft = Draft::open(&mut app, &json!({"account": owner})).unwrap();
    assert!(draft.filled && draft.editable(&app) && draft.on && draft.internal.contains("Away this week."));
    app.execute("sync.offline", json!({"value": true})).unwrap();
    assert!(!draft.editable(&app));
}

#[test]
fn a_change_without_the_permission_asks_the_owner_and_goes_once_he_allows_it() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    read(&mut app, owner, &answers);
    // The sign-in reads them only: the change isn't sent; the owner is asked, by itself.
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, write_consent_needed: true, ..Default::default() });
    app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).unwrap();
    app.poll_auto_replies();
    let Some(Dialog::Confirm { command, params, .. }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowAutomaticReplies", Some(owner)));
    assert_eq!(app.auto_replies.perm.get(&owner), Some(&Permission::Asking));
    assert!(sign_ins.lock().unwrap().is_empty(), "no page before a yes");
    // Not meant without a yes.
    assert!(app.run("ui.allowAutomaticReplies", json!({"account": owner})).is_err());
    // Yes: Microsoft's page for that permission alone, never with a sign-in of another kind.
    app.dialog = None;
    answers.lock().unwrap().push_back(AutoRepliesResult {
        account: owner,
        changed: true,
        setting: Some(AutoReplies { status: Some(AutoReplyStatus::Disabled), ..away() }),
        ..Default::default()
    });
    app.run("ui.allowAutomaticReplies", json!({"account": owner, "confirmed": true})).unwrap();
    assert_eq!(sign_ins.lock().unwrap()[0], (owner, true, false));
    app.poll_sign_in();
    // The change that waited goes now, with the new sign-in, and is said.
    let (account, change) = jobs.lock().unwrap().last().cloned().unwrap();
    assert_eq!((account, change.and_then(|c| c.status)), (owner, Some(AutoReplyStatus::Disabled)));
    app.poll_auto_replies();
    assert_eq!(app.auto_replies.perm.get(&owner), Some(&Permission::Given));
    assert!(said(&app).contains("off"), "{}", said(&app));
    let (line, again) = app.auto_replies_line(owner).unwrap();
    assert!(again.is_none() && line.contains("may change them"), "{line}");
}

#[test]
fn a_question_that_goes_away_drops_the_change_and_file_info_asks_again() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    read(&mut app, owner, &answers);
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, write_consent_needed: true, ..Default::default() });
    app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).unwrap();
    app.poll_auto_replies();
    // Closed unanswered: not allowed, the change dropped and said.
    app.dialog = None;
    app.auto_replies_tick_at(1.0);
    assert_eq!(app.auto_replies.perm.get(&owner), Some(&Permission::Declined));
    assert!(said(&app).contains("weren't changed"), "{}", said(&app));
    let (line, again) = app.auto_replies_line(owner).unwrap();
    assert!(again == Some(Again::Ask) && line.contains("waits for your permission"), "{line}");
    // The next change asks at once, without a job.
    let before = jobs.lock().unwrap().len();
    app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).unwrap();
    assert!(matches!(&app.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowAutomaticReplies"));
    assert_eq!(jobs.lock().unwrap().len(), before);
    // And Microsoft's no: dropped, said, offered again.
    app.dialog = None;
    app.services.sign_in = Some(Box::new(|_| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(crate::favorites_sync::REFUSED_BY_ORGANISATION.to_string())).unwrap();
        rx
    }));
    app.run("ui.allowAutomaticReplies", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    assert!(matches!(app.auto_replies.perm.get(&owner), Some(Permission::Refused(_))));
    assert_eq!(app.auto_replies_line(owner).unwrap().1, Some(Again::Ask));
    assert_eq!(jobs.lock().unwrap().len(), before);
    assert!(sign_ins.lock().unwrap().is_empty());
}

#[test]
fn a_change_waiting_when_work_offline_is_turned_on_is_dropped_and_said() {
    let (mut app, owner, answers, jobs, _) = window();
    read(&mut app, owner, &answers);
    // A job is under way: the change waits for it.
    app.auto_replies_read_now(owner);
    app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).unwrap();
    let before = jobs.lock().unwrap().len();
    app.execute("sync.offline", json!({"value": true})).unwrap();
    app.poll_auto_replies();
    assert!(!app.auto_replies_tick_at(1.0));
    assert!(said(&app).contains("offline"), "{}", said(&app));
    app.execute("sync.offline", json!({"value": false})).unwrap();
    app.auto_replies_tick_at(2.0);
    assert!(jobs.lock().unwrap().iter().skip(before).all(|(_, change)| change.is_none()), "nothing goes up later");
    assert!(app.execute("sync.offline", json!({"value": true})).is_ok());
    assert!(app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).is_err());
}

#[test]
fn a_read_that_keeps_failing_backs_off_up_to_an_hour() {
    let (mut app, owner, answers, _, _) = window();
    let minute = 60_000.0;
    let mut at = 0.0;
    for wait in [2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0] {
        answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, error: "the server is busy".into(), ..Default::default() });
        assert!(app.auto_replies_tick_at(at), "due at {at}");
        app.poll_auto_replies();
        // (The answer is stamped with the clock; the next wait is measured from it.)
        at = app.auto_replies.checked_at(owner);
        assert!(!app.auto_replies_tick_at(at + wait * minute - 1.0), "{wait} minutes");
        at += wait * minute;
    }
    assert!(matches!(app.auto_replies.trouble.get(&owner), Some(Trouble::Failed(_))));
    // A good read starts over.
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), ..Default::default() });
    assert!(app.auto_replies_tick_at(at));
    app.poll_auto_replies();
    assert!(!app.auto_replies.trouble.contains_key(&owner));
}

#[test]
fn a_new_sign_in_of_any_kind_lets_them_be_read_again() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, sign_in_refused: true, ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    app.poll_auto_replies();
    assert!(matches!(app.auto_replies.trouble.get(&owner), Some(Trouble::SignInOver(_))));
    assert_eq!(app.auto_replies_line(owner).unwrap().1, Some(Again::SignIn));
    // Not tried again with the same sign-in, however long.
    assert!(!app.auto_replies_tick_at(24.0 * 3_600_000.0));
    // Signed in again some other way (Send/Receive): read again.
    app.tokens.insert(owner, tokens("ref-9"));
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), ..Default::default() });
    assert!(app.auto_replies_tick_at(24.0 * 3_600_000.0 + 1.0));
    app.poll_auto_replies();
    assert!(!app.auto_replies.trouble.contains_key(&owner));
    assert_eq!(jobs.lock().unwrap().len(), 2);
    // The same for a sign-in that doesn't read the settings.
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, consent_needed: true, ..Default::default() });
    app.auto_replies_read_now(owner);
    app.poll_auto_replies();
    assert!(matches!(app.auto_replies.trouble.get(&owner), Some(Trouble::SignInToRead(_))));
    app.tokens.insert(owner, tokens("ref-10"));
    assert!(app.auto_replies_tick_at(48.0 * 3_600_000.0));
}

#[test]
fn an_account_without_them_is_refused() {
    let (mut app, _, _, _, _) = window();
    let imap =
        app.execute("account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let refused = app.execute("account.automaticReplies", json!({"account": imap})).unwrap_err();
    assert!(refused.contains("Sieve"), "{refused}");
    assert!(app.auto_replies_line(imap).is_none());
    assert!(app.run("ui.allowAutomaticReplies", json!({"account": imap, "confirmed": true})).is_err());
}
