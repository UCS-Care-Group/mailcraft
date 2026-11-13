// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Unsubscribe in the window (#1207): the bar above a message from a list, the question asked
//! first, and one click through the host.
//!
//! The people, lists and hosts are invented.

use std::sync::{Arc, Mutex, mpsc};

use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::{Address, Mailbox, Message};
use serde_json::json;

use super::super::tests::{select, window};
use super::{Outcome, Way, poll, question};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
/// What Avery's server writes for mail from the list that passed its checks.
const PASSED: &str = "mx.example.org; dkim=pass header.d=news.example; dmarc=pass header.from=news.example";

/// A window on Avery's mailbox with one message from the list with `headers`; its id.
fn list_app(headers: &[(&str, &str)]) -> (MailApp, u64) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    for account in &mut mailbox.accounts {
        account.kind = mailcraft_model::AccountKind::Imap;
        account.incoming = Some(mailcraft_model::Server { host: "imap.example.org".into(), port: 993, security: mailcraft_model::Security::Tls });
    }
    let inbox = mailbox.inbox().unwrap();
    let m = Message {
        folder: inbox,
        from: Address::new("Lumen News", "letter@news.example"),
        subject: "This week".into(),
        // On the server (mail from a file is never vouched for).
        uid: 1,
        date: NOW - 3_600,
        headers: headers.iter().map(|(h, v)| ((*h).into(), (*v).into())).collect(),
        ..Message::default()
    };
    let id = mailbox.add_message(m).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    (app, id)
}

/// A host that answers each one-click unsubscribe with `answer`, and keeps the addresses asked.
fn host(app: &mut MailApp, answer: Result<(), String>) -> Arc<Mutex<Vec<String>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    app.services.unsubscribe = Some(Box::new(move |url: String| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push(url);
        tx.send(answer.clone()).unwrap();
        rx
    }));
    asked
}

const ONE_CLICK: [(&str, &str); 3] = [
    ("Authentication-Results", PASSED),
    ("List-Unsubscribe", "<https://news.example/u/7f3a>"),
    ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click"),
];

#[test]
fn a_list_message_shows_the_bar_and_one_click_goes_through_the_host() {
    let (mut app, id) = list_app(&ONE_CLICK);
    let asked = host(&mut app, Ok(()));
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    assert!(harness.query_all_by_label_contains("From a mailing list").next().is_some());
    // The question says who is told, and nothing else.
    let q = question(harness.state(), &json!({"message": id})).unwrap();
    assert!(q.text.contains("news.example"), "{}", q.text);
    let said = harness.state_mut().execute("ui.unsubscribe", json!({"message": id})).unwrap();
    assert_eq!((said["way"].as_str(), said["started"].as_bool()), (Some("oneClick"), Some(true)));
    assert_eq!(*asked.lock().unwrap(), ["https://news.example/u/7f3a"]);
    harness.run_steps(4);
    assert!(harness.query_all_by_label_contains("Unsubscribed.").next().is_some());
}

#[test]
fn a_refusal_from_the_list_is_said() {
    let (mut app, id) = list_app(&ONE_CLICK);
    let _ = host(&mut app, Err("the list's server answered 500".into()));
    app.execute("ui.unsubscribe", json!({"message": id})).unwrap();
    poll(&mut app);
    assert!(matches!(&app.unsubscribing.said, Some((m, Outcome::Failed(why))) if *m == id && why.contains("500")));
    // The bar says so, and offers to try again.
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    assert!(harness.query_all_by_label_contains("Not unsubscribed").next().is_some());
    assert!(harness.query_all_by_label("Try again").next().is_some());
}

#[test]
fn a_page_opened_is_not_called_unsubscribed() {
    let (app, id) = list_app(&[("Authentication-Results", PASSED), ("List-Unsubscribe", "<https://news.example/leave>")]);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    let said = harness.state_mut().execute("ui.unsubscribe", json!({"message": id})).unwrap();
    assert_eq!(said["way"], "page");
    harness.run_steps(4);
    assert!(harness.query_all_by_label_contains("Page opened").next().is_some());
    assert!(harness.query_all_by_label_contains("Unsubscribed.").next().is_none());
}

#[test]
fn the_message_asked_about_is_the_one_unsubscribed_from() {
    let (app, id) = list_app(&ONE_CLICK);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    // Pinned when the question opens: a yes acts on it, whatever is selected by then.
    for params in [json!({}), json!({"message": null})] {
        assert_eq!(crate::dialogs::pin_targets(harness.state(), "ui.unsubscribe", params)["message"], json!(id));
    }
    assert_eq!(crate::dialogs::pin_targets(harness.state(), "ui.unsubscribe", json!({"message": 7}))["message"], json!(7));
}

#[test]
fn without_a_host_or_a_vouched_message_nothing_is_offered() {
    // The browser and the tests have no host: one click isn't there.
    let (mut app, id) = list_app(&ONE_CLICK);
    assert!(app.execute("ui.unsubscribe", json!({"message": id})).is_err());
    // Headers the account's server doesn't vouch for: no bar, and the command refuses.
    let (mut app, id) = list_app(&[("List-Unsubscribe", "<https://news.example/u/7f3a>"), ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click")]);
    let asked = host(&mut app, Ok(()));
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    assert!(harness.query_all_by_label_contains("From a mailing list").next().is_none());
    assert!(harness.state_mut().execute("ui.unsubscribe", json!({"message": id})).is_err());
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn the_question_says_what_each_way_does() {
    let (app, id) = list_app(&[("Authentication-Results", PASSED), ("List-Unsubscribe", "<https://news.example/leave>")]);
    let q = question(&app, &json!({"message": id})).unwrap();
    assert!(q.text.contains("browser"), "{}", q.text);
    let (app, id) = list_app(&[("Authentication-Results", PASSED), ("List-Unsubscribe", "<mailto:leave@news.example>")]);
    let q = question(&app, &json!({"message": id})).unwrap();
    assert!(q.text.contains("leave@news.example") && q.text.contains("avery@lumen-works.example"), "{}", q.text);
    // A message that says nothing asks nothing.
    let (app, id) = list_app(&[("Authentication-Results", PASSED)]);
    assert!(question(&app, &json!({"message": id})).is_none());
    let _ = Way::Page { url: String::new() }.kind();
}
