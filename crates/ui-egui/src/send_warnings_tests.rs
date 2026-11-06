// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send warnings in the window (#509): the owner's Send asks first, with Send Anyway, Move to
//! Bcc (only when outsiders would see each other) and Cancel, which Escape answers (Enter never sends); an
//! agent's send is asked about in the window whatever it allowed or said; a script without the
//! owner's say is refused.
//!
//! The people and domains are invented.

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use crate::MailApp;
use crate::agent_gate::{self, Answer};
use crate::control::{ControlRequest, Outcome};
use crate::dialogs::{Dialog, answered, go_ahead};

const FIVE_AT_TWO: &str =
    "a.one@brightfield.example; b.two@brightfield.example; c.three@brightfield.example; d.four@corvid.example; e.five@corvid.example";
const LOOKALIKE: &str = "lin.zhao@harborwork.example";

/// Avery's local account at Lumen, who keeps Lin at Harbor Works as a contact.
fn app() -> MailApp {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn"})).unwrap();
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    let mut app = MailApp::new(s, crate::Services::default());
    app.autosave = false;
    app
}

fn write(app: &mut MailApp, to: &str) {
    app.execute("mail.new", json!({"to": to, "subject": "Plans"})).unwrap();
}

fn sent(app: &MailApp) -> Vec<mailcraft_model::Message> {
    let mb = &app.session.mailbox;
    let out: Vec<u64> = mb.folders.iter().filter(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox)).map(|f| f.id).collect();
    mb.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Plans").map(|m| (**m).clone()).collect()
}

/// The question on screen: its command, params, and the question.
fn question(app: &MailApp) -> (String, Value, &crate::dialogs::Question) {
    match &app.dialog {
        Some(Dialog::Confirm { command, params, ask }) => (command.clone(), params.clone(), ask),
        _ => panic!("no question is open"),
    }
}

#[test]
fn the_owners_send_to_outsiders_asks_with_send_anyway_move_to_bcc_and_cancel() {
    let mut a = app();
    write(&mut a, FIVE_AT_TWO);
    assert_eq!(a.run("compose.send", json!({})).unwrap(), json!({"pending": "confirm"}));
    assert!(sent(&a).is_empty(), "nothing goes before the owner answers");
    let (command, params, ask) = question(&a);
    assert_eq!(command, "compose.send");
    assert_eq!(ask.title, "Check Recipients");
    assert_eq!(ask.action, "Send Anyway");
    assert_eq!(ask.with, Value::Null, "Send Anyway is the owner's yes: `confirmed`");
    assert_eq!(ask.more, vec![("Move to Bcc".to_string(), json!({"moveToBcc": true, "confirmed": true}))]);
    assert!(ask.cancel.is_none(), "the button that goes nowhere says Cancel");
    assert!(ask.text.contains("5 people outside your organisation") && ask.text.contains("2 domains"), "{}", ask.text);
    // Move to Bcc.
    let with = ask.more[0].1.clone();
    a.dialog = None;
    go_ahead(&mut a, &command, answered(&params, &with, None));
    let out = sent(&a);
    assert_eq!(out.len(), 1);
    assert!(out[0].to.is_empty() && out[0].cc.is_empty());
    assert_eq!(out[0].bcc.len(), 5);
    assert!(a.session.compose.is_none());
}

#[test]
fn a_lookalike_asks_with_send_anyway_and_cancel_only() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    a.run("compose.send", json!({})).unwrap();
    let (command, params, ask) = question(&a);
    assert!(ask.more.is_empty(), "nobody to move to Bcc");
    assert!(ask.text.contains(LOOKALIKE) && ask.text.contains("harborworks.example"), "{}", ask.text);
    // Cancel: the message stays as it is being written.
    a.dialog = None;
    assert!(sent(&a).is_empty());
    assert_eq!(a.session.compose.as_ref().unwrap().to, LOOKALIKE);
    // Send Anyway.
    go_ahead(&mut a, &command, answered(&params, &Value::Null, None));
    assert_eq!(sent(&a).len(), 1);
    assert!(a.dialog.is_none(), "asked once");
}

#[test]
fn a_message_changed_while_the_owner_is_asked_is_asked_about_again() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    a.run("compose.send", json!({})).unwrap();
    let (command, params, _) = question(&a);
    // An agent adds outsiders while the question is up (compose.set is safe for agents).
    a.dialog = None;
    a.session.compose.as_mut().unwrap().cc = FIVE_AT_TWO.into();
    go_ahead(&mut a, &command, answered(&params, &Value::Null, None));
    assert!(sent(&a).is_empty(), "the yes was to another message");
    let (_, again, ask) = question(&a);
    assert!(ask.text.contains("6 people outside your organisation"), "{}", ask.text);
    assert!(again.get("confirmed").is_none());
    // Answered now, it goes.
    a.dialog = None;
    go_ahead(&mut a, &command, answered(&again, &Value::Null, None));
    assert_eq!(sent(&a).len(), 1);
}

#[test]
fn a_move_to_bcc_for_a_message_changed_while_the_owner_is_asked_is_asked_about_again() {
    let mut a = app();
    write(&mut a, FIVE_AT_TWO);
    a.run("compose.send", json!({})).unwrap();
    let (command, params, ask) = question(&a);
    let with = ask.more[0].1.clone();
    // An agent adds another outsider while the question is up; the owner then picks Move to Bcc.
    a.dialog = None;
    a.session.compose.as_mut().unwrap().cc = "f.six@dunmore.example".into();
    go_ahead(&mut a, &command, answered(&params, &with, None));
    assert!(sent(&a).is_empty(), "the Move to Bcc was for another message");
    let (_, again, ask) = question(&a);
    assert!(ask.text.contains("6 people outside your organisation") && ask.text.contains("3 domains"), "{}", ask.text);
    assert!(again.get("confirmed").is_none() && again.get("moveToBcc").is_none(), "{again}");
    assert_eq!(ask.more.len(), 1, "Move to Bcc is offered again");
}

#[test]
fn a_warning_the_question_didnt_show_is_asked_about_though_the_recipients_are_the_same() {
    // A reply to all of outsiders who saw each other on the message it answers, with a lookalike
    // in Bcc: the question shows the lookalike alone.
    let mut a = app();
    let inbox = a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Inbox).unwrap().id;
    let account = a.session.mailbox.accounts[0].id;
    let list = mailcraft_model::Address::parse_list(FIVE_AT_TWO);
    let original = mailcraft_model::Message {
        account,
        folder: inbox,
        from: list[0].clone(),
        to: vec![mailcraft_model::Address::new("Avery Quinn", "avery@lumen.example")],
        cc: list[1..].to_vec(),
        subject: "Kick-off".into(),
        message_id: "kick-off@brightfield.example".into(),
        ..mailcraft_model::Message::default()
    };
    let original = a.session.mailbox.add_message(original).unwrap();
    a.execute("mail.replyAll", json!({"id": original})).unwrap();
    let c = a.session.compose.as_mut().unwrap();
    c.subject = "Plans".into();
    c.bcc = LOOKALIKE.into();
    a.run("compose.send", json!({})).unwrap();
    let (command, params, ask) = question(&a);
    assert!(!ask.text.contains("outside your organisation"), "{}", ask.text);
    // The message it answers is deleted while the question is up: the outsiders are news.
    a.dialog = None;
    a.session.mailbox.remove_message(original).unwrap();
    go_ahead(&mut a, &command, answered(&params, &Value::Null, None));
    assert!(sent(&a).is_empty(), "the yes was to fewer warnings");
    let (_, again, ask) = question(&a);
    assert!(ask.text.contains("5 people outside your organisation"), "{}", ask.text);
    assert!(again.get("confirmed").is_none());
}

#[test]
fn escape_cancels_and_enter_never_sends() {
    // The sample mailbox: Avery at lumen.example; lumem.example is one letter away. Going ahead
    // takes a deliberate click.
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let mut h = crate::people_view::testing::window("view.mail");
        let app = h.state_mut();
        let _ = app.execute("ui.dismissReminder", json!({"all": true}));
        app.execute("mail.new", json!({"to": "tomas@lumem.example", "subject": "Plans"})).unwrap();
        h.run_steps(4);
        h.state_mut().run("compose.send", json!({})).unwrap();
        h.run_steps(4);
        assert!(matches!(h.state().dialog, Some(Dialog::Confirm { .. })));
        crate::people_view::testing::key(&mut h, key);
        h.run_steps(2);
        if key == egui::Key::Escape {
            assert!(h.state().dialog.is_none(), "Escape answers Cancel");
        }
        assert!(sent(h.state()).is_empty(), "{key:?} sent it");
        assert!(h.state().session.compose.is_some(), "the message stays as it is being written");
    }
}

#[test]
fn too_many_recipients_to_check_are_asked_about() {
    let mut a = app();
    let many: Vec<String> = (0..=mailcraft_model::send_warnings::MAX_RECIPIENTS).map(|i| format!("p{i}@lumen.example")).collect();
    write(&mut a, &many.join("; "));
    a.run("compose.send", json!({})).unwrap();
    let (command, params, ask) = question(&a);
    assert!(ask.text.contains("only the first 1000 were looked at"), "{}", ask.text);
    assert!(ask.more.is_empty());
    a.dialog = None;
    go_ahead(&mut a, &command, answered(&params, &Value::Null, None));
    assert_eq!(sent(&a).len(), 1);
}

#[test]
fn one_recipient_looked_at_is_said_in_the_singular() {
    use mailcraft_model::send_warnings::SendWarning;
    let lines = crate::send_warnings::lines(&[SendWarning::TooManyRecipients { checked: 1 }, SendWarning::TooManyRecipients { checked: 2 }]);
    assert_eq!(
        lines,
        [
            "This message has more recipients than MailCraft checks: only the first one was looked at.",
            "This message has more recipients than MailCraft checks: only the first 2 were looked at."
        ]
    );
}

#[test]
fn ordinary_mail_is_not_asked_about() {
    let mut a = app();
    write(&mut a, "lin.zhao@harborworks.example; maya@lumen.example");
    a.run("compose.send", json!({})).unwrap();
    assert!(a.dialog.is_none());
    assert_eq!(sent(&a).len(), 1);
}

#[test]
fn a_script_through_the_window_is_refused_unless_it_says_send_anyway() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    let (req, _) = ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req);
    assert!(matches!(&out, Outcome::Done(v) if v["ok"] == false && v["error"].as_str().unwrap().contains("lookalikeRecipient")));
    assert!(sent(&a).is_empty() && a.dialog.is_none());
    let (req, _) = ControlRequest::new("engine.execute", json!({"command": "compose.send", "params": {"confirmed": true}}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req);
    assert!(matches!(&out, Outcome::Done(v) if v["ok"] == true));
    assert_eq!(sent(&a).len(), 1);
}

/// `method` sent as an agent allowed `allow`; the answer, if there is one yet.
fn ask(a: &mut MailApp, params: Value, allow: Vec<Class>) -> (Option<Value>, std::sync::mpsc::Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": "compose.send", "params": params}));
    let req = req.from_agent(allow);
    let out = crate::control::handle(a, &egui::Context::default(), &req);
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

#[test]
fn an_agents_send_asks_the_owner_whatever_it_was_allowed_or_says() {
    let mut a = app();
    write(&mut a, FIVE_AT_TWO);
    // Allowed to send in advance, and saying send anyway itself: asked all the same.
    let (now, rx) = ask(&mut a, json!({"confirmed": true, "moveToBcc": false}), vec![Class::Send]);
    assert!(now.is_none(), "asked first");
    let q = a.asking.front().unwrap();
    assert_eq!(q.class, Class::Send);
    assert_eq!(q.warnings.iter().map(|w| w.kind()).collect::<Vec<_>>(), ["outsidersSeeEachOther"]);
    assert!(q.params.get("confirmed").is_none(), "an agent can't answer for the owner");
    // No.
    agent_gate::answer(&mut a, false);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(sent(&a).is_empty());
    // Send anyway.
    let (_, rx) = ask(&mut a, json!({}), vec![Class::Send]);
    agent_gate::answer_with(&mut a, Answer::Yes);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    let out = sent(&a);
    assert_eq!((out.len(), out[0].to.len()), (1, 5));
}

#[test]
fn the_owner_can_move_an_agents_outsiders_to_bcc() {
    let mut a = app();
    write(&mut a, FIVE_AT_TWO);
    let (_, rx) = ask(&mut a, json!({}), vec![]);
    assert!(crate::send_warnings::offers_bcc(&a.asking.front().unwrap().warnings));
    agent_gate::answer_with(&mut a, Answer::MoveToBcc);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    let out = sent(&a);
    assert!(out[0].to.is_empty());
    assert_eq!(out[0].bcc.len(), 5);
}

#[test]
fn an_agents_send_that_grew_a_warning_meanwhile_is_not_sent_on_the_old_yes() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    let (_, rx) = ask(&mut a, json!({}), vec![]);
    // While the question is up, outsiders are added.
    a.session.compose.as_mut().unwrap().cc = FIVE_AT_TWO.into();
    agent_gate::answer_with(&mut a, Answer::Yes);
    let r = rx.try_recv().unwrap();
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("since the owner was asked"), "{r}");
    assert!(sent(&a).is_empty());
}

#[test]
fn an_agents_send_whose_recipients_were_swapped_meanwhile_is_not_sent() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    let (_, rx) = ask(&mut a, json!({}), vec![]);
    // The warned lookalike swapped for somebody nobody warns about: fewer warnings, other people.
    a.session.compose.as_mut().unwrap().to = "attacker@unrelated.example".into();
    agent_gate::answer_with(&mut a, Answer::Yes);
    let r = rx.try_recv().unwrap();
    assert_eq!(r["ok"], false, "{r}");
    assert!(sent(&a).is_empty());
}

#[test]
fn an_agents_send_sends_the_message_asked_about_though_another_is_current() {
    let mut a = app();
    write(&mut a, LOOKALIKE);
    let asked = a.session.compose_id().unwrap();
    let (_, rx) = ask(&mut a, json!({}), vec![]);
    assert_eq!(a.asking.front().unwrap().params["compose"], json!(asked), "the question is about this message");
    // Another to the same people is started while the question is up.
    a.execute("mail.new", json!({"to": LOOKALIKE, "subject": "Other"})).unwrap();
    let other = a.session.compose_id().unwrap();
    assert_ne!(other, asked);
    agent_gate::answer_with(&mut a, Answer::Yes);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(sent(&a).len(), 1, "the one asked about went");
    assert!(a.session.composing_by_id(other).is_some(), "the other is still being written");
    assert!(a.session.composing_by_id(asked).is_none());
}

#[test]
fn an_agents_ordinary_send_is_as_before() {
    let mut a = app();
    write(&mut a, "lin.zhao@harborworks.example");
    let (now, _) = ask(&mut a, json!({}), vec![Class::Send]);
    assert_eq!(now.unwrap()["ok"], true);
    assert!(a.asking.is_empty());
    assert_eq!(sent(&a).len(), 1);
}
