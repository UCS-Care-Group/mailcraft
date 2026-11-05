// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send warnings in a session (#509): `compose.send` refuses a message with one unless told to
//! send anyway, moves the people outside the organisation to Bcc when told to, and measures
//! against the contacts and Sent Items as the sender warnings do.
//!
//! The people and domains are invented.

use mailcraft_model::{Address, FolderKind, Message};
use serde_json::{Value, json};

use crate::Session;
use crate::send_warnings::Lines;

const FIVE_AT_TWO: &str =
    "a.one@brightfield.example; b.two@brightfield.example; c.three@brightfield.example; d.four@corvid.example; e.five@corvid.example";

/// Avery's local account at Lumen, who keeps Lin at Harbor Works as a contact.
fn session() -> Session {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn"})).unwrap();
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    s
}

fn write(s: &mut Session, to: &str, cc: &str) {
    s.run("mail.new", &json!({"to": to, "cc": cc, "subject": "Plans"})).unwrap();
}

fn sent(s: &Session) -> Vec<&Message> {
    let out: Vec<u64> = s.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox)).map(|f| f.id).collect();
    s.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Plans").map(|m| &**m).collect()
}

fn kinds(s: &mut Session) -> Vec<&'static str> {
    s.send_warnings(&json!({})).iter().map(|w| w.kind()).collect()
}

#[test]
fn a_lookalike_recipient_is_refused_until_the_owner_says_send_anyway() {
    let mut s = session();
    write(&mut s, "lin.zhao@harborwork.example", "");
    assert_eq!(kinds(&mut s), ["lookalikeRecipient"]);
    let e = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(e.contains("lookalikeRecipient lin.zhao@harborwork.example (like harborworks.example)") && e.contains("\"confirmed\": true"), "{e}");
    // Nothing went, and the message is still being written.
    assert!(sent(&s).is_empty());
    assert!(s.compose.is_some());
    // Junk where a yes would be is no yes.
    for junk in [json!({"confirmed": "true"}), json!({"confirmed": 1}), json!({"confirmed": null}), json!({"confirmed": false})] {
        assert!(s.run("compose.send", &junk).is_err(), "{junk}");
    }
    s.run("compose.send", &json!({"confirmed": true})).unwrap();
    assert_eq!(sent(&s).len(), 1);
}

#[test]
fn ordinary_mail_goes_as_before() {
    let mut s = session();
    // The contact, in another case; a colleague; the owner; somebody new at an unrelated domain.
    write(&mut s, "LIN.ZHAO@harborworks.example; jordan@lumen.example", "avery@lumen.example; hello@kestrel.example");
    assert!(kinds(&mut s).is_empty());
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(sent(&s).len(), 1);
}

#[test]
fn whoever_mail_went_to_is_known_from_then_on() {
    let mut s = session();
    write(&mut s, "pat.morgan@harborwork.example", "");
    s.run("compose.send", &json!({"confirmed": true})).unwrap();
    // Written to once, the address is somebody the owner writes to.
    write(&mut s, "pat.morgan@harborwork.example", "");
    assert!(kinds(&mut s).is_empty());
    s.run("compose.send", &json!({})).unwrap();
    // And somebody one letter from it is asked about.
    write(&mut s, "pat.morgen@harborwork.example", "");
    assert_eq!(kinds(&mut s), ["lookalikeRecipient"]);
}

#[test]
fn outsiders_who_would_see_each_other_are_asked_about_and_can_go_to_bcc() {
    let mut s = session();
    write(&mut s, "Maya Okafor <maya@lumen.example>; Bea Two <b.two@brightfield.example>", FIVE_AT_TWO);
    assert_eq!(kinds(&mut s), ["outsidersSeeEachOther"]);
    let e = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(e.contains("outsidersSeeEachOther (5 at 2 domains in To and Cc)") && e.contains("\"moveToBcc\": true"), "{e}");
    // As Move to Bcc would send it: nothing left to ask.
    assert!(s.send_warnings(&json!({"moveToBcc": true})).is_empty());
    let id = s.run("compose.send", &json!({"moveToBcc": true})).unwrap()["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.to, vec![Address::new("Maya Okafor", "maya@lumen.example")]);
    assert!(m.cc.is_empty());
    let bcc: Vec<&str> = m.bcc.iter().map(|a| a.email.as_str()).collect();
    assert_eq!(
        bcc,
        ["b.two@brightfield.example", "a.one@brightfield.example", "c.three@brightfield.example", "d.four@corvid.example", "e.five@corvid.example"]
    );
    assert_eq!(m.bcc[0].name, "Bea Two");
}

#[test]
fn move_to_bcc_still_asks_about_a_lookalike() {
    let mut s = session();
    write(&mut s, &format!("{FIVE_AT_TWO}; lin.zhao@harborwork.example"), "");
    assert_eq!(kinds(&mut s), ["lookalikeRecipient", "outsidersSeeEachOther"]);
    assert_eq!(s.send_warnings(&json!({"moveToBcc": true})).iter().map(|w| w.kind()).collect::<Vec<_>>(), ["lookalikeRecipient"]);
    let e = s.run("compose.send", &json!({"moveToBcc": true})).unwrap_err().to_string();
    assert!(e.contains("lookalikeRecipient") && !e.contains("outsidersSeeEachOther"), "{e}");
    // Refused, the message is as it was written: nobody was moved.
    assert_eq!(s.compose.as_ref().unwrap().to, format!("{FIVE_AT_TWO}; lin.zhao@harborwork.example"));
    assert!(s.compose.as_ref().unwrap().bcc.is_empty());
    s.run("compose.send", &json!({"moveToBcc": true, "confirmed": true})).unwrap();
    assert!(sent(&s)[0].to.is_empty());
}

#[test]
fn a_reply_all_to_people_who_saw_each_other_already_goes() {
    let mut s = session();
    let inbox = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Inbox).unwrap().id;
    let account = s.mailbox.accounts[0].id;
    let list: Vec<Address> = Address::parse_list(FIVE_AT_TWO);
    let original = Message {
        account,
        folder: inbox,
        from: list[0].clone(),
        to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
        cc: list[1..].to_vec(),
        subject: "Kick-off".into(),
        message_id: "kick-off@brightfield.example".into(),
        ..Message::default()
    };
    let id = s.mailbox.add_message(original).unwrap();
    s.run("mail.replyAll", &json!({"id": id})).unwrap();
    assert!(kinds(&mut s).is_empty(), "they saw each other on the message replied to");
    // One more outsider sees them all.
    let c = s.compose.as_mut().unwrap();
    c.cc.push_str("; f.six@kestrel.example");
    assert_eq!(kinds(&mut s), ["outsidersSeeEachOther"]);
    // A draft of the reply, opened again, knows its original too.
    let c = s.compose.as_mut().unwrap();
    c.cc = c.cc.replace("; f.six@kestrel.example", "");
    s.run("compose.saveDraft", &json!({})).unwrap();
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    s.run("compose.close", &json!({})).unwrap();
    s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    assert!(Lines::of(&s).unwrap().original.is_some());
    assert!(kinds(&mut s).is_empty());
}

#[test]
fn a_typed_line_with_nobody_to_move_stays_as_it_was() {
    let mut s = session();
    write(&mut s, "jordan@lumen.example , maya@lumen.example", "");
    let lines = Lines::of(&s).unwrap();
    assert_eq!(s.outsiders_to_bcc(&lines), lines);
}

#[test]
fn hostile_send_params_never_panic() {
    let mut s = session();
    write(&mut s, FIVE_AT_TWO, "");
    for junk in [Value::Null, json!([]), json!("x"), json!({"moveToBcc": "yes"}), json!({"moveToBcc": [true], "confirmed": {}})] {
        let _ = s.send_warnings(&junk);
        assert!(s.run("compose.send", &junk).is_err(), "{junk}");
    }
}
