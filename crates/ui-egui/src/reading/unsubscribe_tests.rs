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
    app.session.driver.set_auto_sync(false);
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

const ONE_CLICK: [(&str, &str); 4] = [
    ("Authentication-Results", PASSED),
    ("List-Unsubscribe", "<https://news.example/u/7f3a>"),
    ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click"),
    // The list's signature covers both unsubscribe headers (RFC 8058, 4).
    ("DKIM-Signature", "v=1; d=news.example; s=s1; h=from:list-unsubscribe:list-unsubscribe-post; bh=AA; b=BB"),
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
    assert!(matches!(app.unsubscribing.said.get(&id), Some(Outcome::Failed(why)) if why.contains("500")));
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
fn without_a_host_one_click_opens_its_page_and_unvouched_mail_offers_nothing() {
    // The browser build has no host to post with: the same address is opened as a page instead.
    let (app, id) = list_app(&ONE_CLICK);
    let q = question(&app, &json!({"message": id})).unwrap();
    assert!(q.text.contains("browser"), "{}", q.text);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    let said = harness.state_mut().execute("ui.unsubscribe", json!({"message": id})).unwrap();
    assert_eq!((said["way"].as_str(), said["url"].as_str()), (Some("page"), Some("https://news.example/u/7f3a")));
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
    // What goes is shown: "Unsubscribe" when the address gives nothing, else its own words, on
    // one line and whole: what is sent is what was seen.
    assert!(q.text.contains("“Unsubscribe”"), "{}", q.text);
    let long = "x".repeat(500);
    let list = format!("<mailto:leave@news.example?subject=Stop%20please&body=Remove%0Ame%20{long}>");
    let (app, id) = list_app(&[("Authentication-Results", PASSED), ("List-Unsubscribe", list.as_str())]);
    let q = question(&app, &json!({"message": id})).unwrap();
    assert!(q.text.contains("“Stop please”") && q.text.contains(&format!("“Remove me {long}”")), "{}", q.text);
    // A message that says nothing asks nothing.
    let (app, id) = list_app(&[("Authentication-Results", PASSED)]);
    assert!(question(&app, &json!({"message": id})).is_none());
    let _ = Way::Page { url: String::new() }.kind();
}

/// [`list_app`] with one more vouched message from `from`, unsubscribing as `list` says; its id.
fn add(app: &mut MailApp, from: &str, list: &str) -> u64 {
    let inbox = app.session.mailbox.inbox().unwrap();
    let domain = from.rsplit_once('@').map_or("", |(_, d)| d);
    let m = Message {
        folder: inbox,
        from: Address::new("", from),
        subject: format!("From {domain}"),
        uid: 2,
        date: NOW - 7_200,
        headers: vec![
            ("Authentication-Results".into(), format!("mx.example.org; dkim=pass header.d={domain}; dmarc=pass header.from={domain}")),
            ("List-Unsubscribe".into(), list.into()),
            ("List-Unsubscribe-Post".into(), "List-Unsubscribe=One-Click".into()),
            ("DKIM-Signature".into(), format!("v=1; d={domain}; s=s1; h=from:list-unsubscribe:list-unsubscribe-post; bh=AA; b=BB")),
        ],
        ..Message::default()
    };
    app.session.mailbox.add_message(m).unwrap()
}

#[test]
fn a_selection_unsubscribes_each_list_once_one_click_at_a_time() {
    let (mut app, first) = list_app(&ONE_CLICK);
    let second = add(&mut app, "deals@shop.example", "<https://shop.example/leave>");
    let page = add(&mut app, "club@club.example", "<http://club.example/leave>");
    let asked = host(&mut app, Ok(()));
    let ids = json!([first, second, page]);
    let (q, _) = super::question_many(&mut app, &json!({"ids": ids})).unwrap();
    assert!(q.text.contains("3 lists") && q.text.contains("2 in one click"), "{}", q.text);
    let said = app.execute("ui.unsubscribeMany", json!({"ids": ids})).unwrap();
    assert_eq!((said["oneClick"].as_u64(), said["pages"].as_u64()), (Some(2), Some(1)));
    // One at a time: the first is on its way, the second waits its turn.
    assert_eq!(asked.lock().unwrap().len(), 1);
    for _ in 0..4 {
        poll(&mut app);
    }
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert_eq!((app.unsubscribing.said.get(&first), app.unsubscribing.said.get(&second)), (Some(&Outcome::Accepted), Some(&Outcome::Accepted)));
    // The page is its message's to open, on its own bar.
    assert_eq!(app.unsubscribing.said.get(&page), None);
}

/// A host that keeps each one-click unsubscribe waiting for its answer; the addresses asked, and
/// the senders of the answers to come.
type Held = (Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<mpsc::Sender<Result<(), String>>>>>);
fn held_host(app: &mut MailApp) -> Held {
    let (asked, answers) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
    let (kept, senders) = (asked.clone(), answers.clone());
    app.services.unsubscribe = Some(Box::new(move |url: String| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push(url);
        senders.lock().unwrap().push(tx);
        rx
    }));
    (asked, answers)
}

#[test]
fn the_next_list_is_asked_only_once_the_one_before_has_answered() {
    let (mut app, first) = list_app(&ONE_CLICK);
    let second = add(&mut app, "deals@shop.example", "<https://shop.example/leave>");
    let (asked, answers) = held_host(&mut app);
    app.execute("ui.unsubscribeMany", json!({"ids": [first, second]})).unwrap();
    for _ in 0..4 {
        poll(&mut app);
    }
    assert_eq!(asked.lock().unwrap().len(), 1, "the second waits while the first has no answer");
    answers.lock().unwrap()[0].send(Ok(())).unwrap();
    poll(&mut app);
    assert_eq!(asked.lock().unwrap().len(), 2);
}

/// A message from `from` in the Inbox, `ago` seconds old, vouched for, with `headers` besides.
fn add_at(app: &mut MailApp, from: &str, ago: i64, headers: &[(&str, &str)]) -> u64 {
    let inbox = app.session.mailbox.inbox().unwrap();
    let domain = from.rsplit_once('@').map_or("", |(_, d)| d);
    let mut all = vec![("Authentication-Results".to_string(), format!("mx.example.org; dmarc=pass header.from={domain}"))];
    all.extend(headers.iter().map(|(h, v)| ((*h).to_string(), (*v).to_string())));
    let m = Message {
        folder: inbox,
        from: Address::new("", from),
        subject: format!("{ago}"),
        uid: 3,
        date: NOW - ago,
        headers: all,
        ..Message::default()
    };
    app.session.mailbox.add_message(m).unwrap()
}

#[test]
fn the_question_counts_each_sender_by_the_message_that_goes() {
    let (mut app, _) = list_app(&[]);
    // Named first, the older message says nothing; the newer one, which goes, unsubscribes by mail.
    let old = add_at(&mut app, "deals@shop.example", 9_000, &[]);
    let new = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", "<mailto:leave@shop.example>")]);
    let (q, _) = super::question_many(&mut app, &json!({"ids": [old, new]})).unwrap();
    assert!(q.text.starts_with("Unsubscribe from 1 list.") && q.text.contains("1 by a message from your account"), "{}", q.text);
    assert!(!q.text.contains("1 lists"), "{}", q.text);
}

#[test]
fn messages_named_wrongly_are_refused_not_taken_for_the_selection() {
    let (mut app, id) = list_app(&ONE_CLICK);
    app.session.sel.messages = vec![id];
    for ids in [json!("all"), json!([id, "x"]), json!({"id": id})] {
        assert!(super::question_many(&mut app, &json!({"ids": ids})).is_err(), "{ids}");
        assert!(app.execute("ui.unsubscribeMany", json!({"ids": ids})).is_err(), "{ids}");
    }
    assert!(app.unsubscribing.waiting.is_empty() && app.unsubscribing.flying.is_none());
}

#[test]
fn without_a_host_one_click_is_left_to_its_page() {
    let (mut app, id) = list_app(&ONE_CLICK);
    let (q, _) = super::question_many(&mut app, &json!({"ids": [id]})).unwrap();
    assert!(q.text.contains("1 unsubscribes on its own page"), "{}", q.text);
    let said = app.execute("ui.unsubscribeMany", json!({"ids": [id]})).unwrap();
    assert_eq!((said["oneClick"].as_u64(), said["pages"].as_u64()), (Some(0), Some(1)));
    assert!(app.unsubscribing.waiting.is_empty() && app.unsubscribing.flying.is_none());
    assert_eq!(app.unsubscribing.said.get(&id), None, "its bar still offers the page");
}

#[test]
fn each_request_by_mail_is_shown_and_only_those_shown_are_sent() {
    let (mut app, _) = list_app(&[]);
    let shop = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", "<mailto:leave@shop.example?subject=Stop%20please>")]);
    // A sender that passes DMARC for its own domain may name any address: it is shown all the same.
    let other = add_at(&mut app, "news@lure.example", 120, &[("List-Unsubscribe", "<mailto:anyone@victim.example?subject=Hello>")]);
    let (q, params) = super::question_many(&mut app, &json!({"ids": [shop, other]})).unwrap();
    for seen in ["to leave@shop.example, subject “Stop please”", "to anyone@victim.example, subject “Hello”"] {
        assert!(q.text.contains(seen), "{}", q.text);
    }
    assert_eq!(params["mail"], json!([shop, other]));
    let outbox = |app: &MailApp| {
        let id = app.session.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Outbox).map(|f| f.id).unwrap();
        app.session.mailbox.messages_in(id).map(|m| m.to.first().map(|a| a.email.clone()).unwrap_or_default()).collect::<Vec<_>>()
    };
    // Without the requests the question listed (an agent's call naming none), nothing is written.
    let said = app.execute("ui.unsubscribeMany", json!({"ids": [shop, other]})).unwrap();
    assert_eq!((said["mail"].as_u64(), said["leftOut"].as_u64()), (Some(0), Some(2)));
    assert!(outbox(&app).is_empty());
    // The yes sends those it listed, and only those.
    let said = app.execute("ui.unsubscribeMany", json!({"ids": [shop, other], "mail": [shop]})).unwrap();
    assert_eq!((said["mail"].as_u64(), said["leftOut"].as_u64()), (Some(1), Some(1)));
    assert_eq!(outbox(&app), ["leave@shop.example"]);
}

#[test]
fn the_owner_is_asked_what_goes_and_refused_before_when_it_cant() {
    let (mut app, id) = list_app(&ONE_CLICK);
    let _ = host(&mut app, Ok(()));
    // Mail its server doesn't vouch for is counted as left out, not as a list.
    let inbox = app.session.mailbox.inbox().unwrap();
    let unvouched = Message {
        folder: inbox,
        from: Address::new("", "alerts@bank.example"),
        subject: "Check".into(),
        uid: 9,
        date: NOW - 60,
        headers: vec![("List-Unsubscribe".into(), "<https://bank.example/u>".into())],
        ..Message::default()
    };
    let unvouched = app.session.mailbox.add_message(unvouched).unwrap();
    assert_eq!(app.run("ui.unsubscribeMany", json!({"ids": [id, unvouched]})).unwrap()["pending"], "confirm");
    match &app.dialog {
        Some(crate::dialogs::Dialog::Confirm { ask, params, .. }) => {
            assert!(ask.text.starts_with("Unsubscribe from 1 list.") && ask.text.contains("1 sender is left out"), "{}", ask.text);
            assert_eq!(params["ids"], json!([id, unvouched]));
        }
        _ => panic!("nothing was asked"),
    }
    // Too many, or none that can be unsubscribed from: refused before anything is asked.
    app.dialog = None;
    let many: Vec<u64> = (1..=501).collect();
    assert!(app.run("ui.unsubscribeMany", json!({"ids": many})).is_err());
    // Said, as any refusal is: the menu that ran it drops the error.
    assert!(app.status_msg.as_ref().is_some_and(|(said, _)| said.contains("500")), "{:?}", app.status_msg);
    assert!(app.run("ui.unsubscribeMany", json!({"ids": [unvouched]})).is_err());
    assert!(app.dialog.is_none());
}

/// The Outbox's messages: to whom each goes.
fn outbox(app: &MailApp) -> Vec<String> {
    let id = app.session.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Outbox).map(|f| f.id).unwrap();
    app.session.mailbox.messages_in(id).map(|m| m.to.first().map(|a| a.email.clone()).unwrap_or_default()).collect()
}

#[test]
fn twenty_requests_by_mail_are_listed_and_only_they_are_sent() {
    let (mut app, _) = list_app(&[]);
    // Twenty-one lists, no two alike (lists named alike would look like each other's once one is
    // written to, and get a sender warning).
    const LISTS: [&str; 21] = [
        "acorn", "birch", "cobalt", "dune", "ember", "fjord", "garnet", "harbor", "indigo", "juniper", "kestrel", "lagoon", "meadow", "nectar",
        "orchid", "pebble", "quartz", "raven", "saffron", "tundra", "umber",
    ];
    let ids: Vec<u64> = LISTS
        .iter()
        .zip(0..)
        .map(|(name, n)| {
            add_at(&mut app, &format!("news@{name}.example"), 60 + n, &[("List-Unsubscribe", &format!("<mailto:leave@{name}.example>"))])
        })
        .collect();
    let (q, params) = super::question_many(&mut app, &json!({"ids": ids})).unwrap();
    assert_eq!(q.text.matches("• to ").count(), 20, "{}", q.text);
    assert!(q.text.contains("1 more by mail isn't sent now"), "{}", q.text);
    let listed: Vec<u64> = params["mail"].as_array().unwrap().iter().filter_map(serde_json::Value::as_u64).collect();
    assert_eq!(listed.len(), 20);
    // The newest twenty: the oldest, `umber`, is the one left for later.
    assert!(!listed.contains(&ids[20]) && q.text.contains("leave@acorn.example") && !q.text.contains("leave@umber.example"), "{}", q.text);
    let mut params = params;
    params["confirmed"] = json!(true);
    let said = app.execute("ui.unsubscribeMany", params).unwrap();
    assert_eq!(said["mail"].as_u64(), Some(20));
    let mut sent = outbox(&app);
    sent.sort();
    let mut expected: Vec<String> = LISTS.iter().take(20).map(|name| format!("leave@{name}.example")).collect();
    expected.sort();
    assert_eq!(sent, expected);
}

#[test]
fn mail_named_null_or_wrongly_sends_nothing_and_never_takes_the_selection() {
    let (mut app, _) = list_app(&[]);
    let shop = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", "<mailto:leave@shop.example>")]);
    app.session.sel.messages = vec![shop];
    let said = app.execute("ui.unsubscribeMany", json!({"ids": [shop], "mail": null})).unwrap();
    assert_eq!(said["mail"].as_u64(), Some(0));
    for wrong in [json!("all"), json!([shop, "x"]), json!({"id": shop})] {
        assert!(app.execute("ui.unsubscribeMany", json!({"ids": [shop], "mail": wrong})).is_err(), "{wrong}");
    }
    assert!(outbox(&app).is_empty());
}

#[test]
fn a_long_address_and_subject_are_shown_whole() {
    let (mut app, _) = list_app(&[]);
    let local = "a".repeat(60);
    let to = format!("{local}@unsubscribe.far-away-domain.example");
    let subject = "Please remove me from every list you keep about me, now and for good, thank you".repeat(2);
    let list = format!("<mailto:{to}?subject={}>", subject.replace(' ', "%20"));
    let shop = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", list.as_str())]);
    let (q, _) = super::question_many(&mut app, &json!({"ids": [shop]})).unwrap();
    assert!(q.text.contains(&to) && q.text.contains(&subject), "{}", q.text);
}

#[test]
fn the_menus_answer_is_kept_until_the_mailbox_or_the_selection_changes() {
    let (mut app, id) = list_app(&ONE_CLICK);
    let shop = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", "<mailto:leave@shop.example>")]);
    assert!(super::any_offered(&mut app, &[id, shop]));
    let kept = app.unsubscribing.menu.clone();
    assert!(super::any_offered(&mut app, &[id, shop]));
    assert_eq!(app.unsubscribing.menu, kept, "asked again, the same answer is kept");
    // Both moved to Junk E-mail: the mailbox changed, so it is asked again, and nothing is offered.
    app.session.run("mail.junk", &json!({"ids": [id, shop]})).unwrap();
    assert!(!super::any_offered(&mut app, &[id, shop]));
    // Another selection is asked about on its own.
    let (mut app, id) = list_app(&ONE_CLICK);
    assert!(super::any_offered(&mut app, &[id]));
    assert!(!super::any_offered(&mut app, &[]));
}

/// `command` sent by an agent allowed nothing in advance, so that the owner is asked: what the
/// question says it would do, and where its answer comes once the owner has said yes.
fn agent_asks(app: &mut MailApp, command: &str, params: serde_json::Value) -> (String, mpsc::Receiver<serde_json::Value>) {
    let (req, rx) = crate::control::ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(app, &egui::Context::default(), &req.from_agent(Vec::new()));
    assert!(!matches!(out, crate::control::Outcome::Done(_)), "the owner is asked first");
    let what = app.asking.front().map(|a| a.what.clone()).unwrap_or_default();
    crate::agent_gate::answer(app, true);
    (what, rx)
}

#[test]
fn what_an_agent_is_allowed_is_what_is_written_and_nothing_more() {
    let (mut app, one) = list_app(&ONE_CLICK);
    let _ = host(&mut app, Ok(()));
    let shop = add_at(&mut app, "deals@shop.example", 60, &[("List-Unsubscribe", "<mailto:leave@shop.example?subject=Leave%20me>")]);
    let club = add_at(&mut app, "news@club.example", 120, &[("List-Unsubscribe", "<mailto:quit@club.example>")]);
    app.session.sel.messages = vec![one, shop, club];
    // One message named by `id`: the question is about it alone, so the selection never runs.
    let (_, rx) = agent_asks(&mut app, "ui.unsubscribeMany", json!({"id": shop}));
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(app.unsubscribing.waiting.is_empty() && app.unsubscribing.flying.is_none() && outbox(&app).is_empty());
    // A request named wrongly among those to write (0): the question shows none, and none goes.
    let (what, rx) = agent_asks(&mut app, "ui.unsubscribeMany", json!({"ids": [shop], "mail": [shop, 0]}));
    assert!(!what.contains("writing"), "{what}");
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(outbox(&app).is_empty());
    // A request named that isn't one of these messages': refused, not left out quietly.
    let (_, rx) = agent_asks(&mut app, "ui.unsubscribeMany", json!({"ids": [shop], "mail": [club]}));
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(outbox(&app).is_empty());
    // Named rightly: each address and subject is in the question, and exactly those are written.
    let (what, rx) = agent_asks(&mut app, "ui.unsubscribeMany", json!({"ids": [shop, club], "mail": [shop, club]}));
    for shown in
        ["to leave@shop.example with the subject \u{201c}Leave me\u{201d}", "to quit@club.example with the subject \u{201c}Unsubscribe\u{201d}"]
    {
        assert!(what.contains(shown), "{what}");
    }
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    let mut sent = outbox(&app);
    sent.sort();
    assert_eq!(sent, ["leave@shop.example", "quit@club.example"]);
}

#[test]
fn more_requests_by_mail_than_a_question_lists_are_refused() {
    let (mut app, _) = list_app(&[]);
    let mail: Vec<u64> = (1..=21).collect();
    let refused = app.execute("ui.unsubscribeMany", json!({"ids": [1], "mail": mail})).unwrap_err();
    assert!(refused.contains("20"), "{refused}");
    assert!(outbox(&app).is_empty());
}
