// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies in the window: a job reads them, a sign-in that only reads
//! them is told to sign in again, a change goes to the host, and nothing goes while offline or
//! for an account that has none. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience};
use mailcraft_model::sync::Tokens;
use serde_json::json;

use crate::auto_replies::{AutoRepliesJob, AutoRepliesResult, Draft, Trouble};
use crate::{MailApp, Services};

const OWNER: &str = "ada@example.org";
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(Id, Option<AutoReplies>)>>>;
type Answers = Arc<Mutex<VecDeque<AutoRepliesResult>>>;

fn window() -> (MailApp, Id, Answers, Jobs) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = true;
    let added = app.execute("account.add", json!({"email": OWNER, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let owner = added["id"].as_u64().unwrap();
    app.tokens.insert(owner, Tokens { access: "a".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() });
    let (answers, jobs): (Answers, Jobs) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph_auto_replies = Some(Box::new(move |job: AutoRepliesJob| {
        log.lock().unwrap().push((job.account.id, job.change.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    (app, owner, answers, jobs)
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

#[test]
fn a_job_reads_them_and_the_status_bar_says_they_are_on() {
    let (mut app, owner, answers, jobs) = window();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), can_write: true, ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    assert!(!app.auto_replies_tick_at(1.0), "one at a time");
    app.poll_auto_replies();
    assert_eq!(jobs.lock().unwrap()[0], (owner, None));
    assert_eq!(app.auto_replies_on(), vec![owner]);
    let (line, again) = app.auto_replies_line(owner).unwrap();
    assert!(!again && line.contains("on, until you turn them off"), "{line}");
    // Not again for a while.
    assert!(!app.auto_replies_tick_at(60_000.0));
}

#[test]
fn a_sign_in_that_only_reads_them_says_to_sign_in_again() {
    let (mut app, owner, answers, _) = window();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), can_write: false, ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    app.poll_auto_replies();
    assert_eq!(app.auto_replies.trouble.get(&owner), Some(&Trouble::SignInToChange));
    let (line, again) = app.auto_replies_line(owner).unwrap();
    assert!(again && line.contains("sign in again to change them"), "{line}");
    // What is there is still known and shown.
    assert_eq!(app.auto_replies_on(), vec![owner]);
    let draft = Draft::open(&mut app, &json!({"account": owner})).unwrap();
    assert!(draft.on && draft.internal.contains("Away this week."));
}

#[test]
fn a_change_goes_to_the_host_and_a_refusal_is_said() {
    let (mut app, owner, answers, jobs) = window();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, setting: Some(away()), can_write: true, ..Default::default() });
    assert!(app.auto_replies_tick_at(0.0));
    app.poll_auto_replies();
    answers.lock().unwrap().push_back(AutoRepliesResult { account: owner, write_refused: true, setting: Some(away()), ..Default::default() });
    app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).unwrap();
    let (account, change) = jobs.lock().unwrap().last().cloned().unwrap();
    assert_eq!((account, change.and_then(|c| c.status)), (owner, Some(AutoReplyStatus::Disabled)));
    app.poll_auto_replies();
    assert_eq!(app.auto_replies.trouble.get(&owner), Some(&Trouble::SignInToChange));
    let said = app.status_msg.clone().unwrap_or_default().0;
    assert!(said.contains("weren't changed"), "{said}");
}

#[test]
fn nothing_goes_while_offline_nor_for_an_account_without_them() {
    let (mut app, owner, _, jobs) = window();
    app.execute("sync.offline", json!({"value": true})).unwrap();
    assert!(!app.auto_replies_tick_at(0.0));
    assert!(app.execute("account.setAutomaticReplies", json!({"account": owner, "status": "disabled"})).is_err());
    assert!(jobs.lock().unwrap().is_empty());
    let imap =
        app.execute("account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let refused = app.execute("account.automaticReplies", json!({"account": imap})).unwrap_err();
    assert!(refused.contains("Sieve") || refused.contains("Microsoft"), "{refused}");
    assert!(app.auto_replies_line(imap).is_none());
}
