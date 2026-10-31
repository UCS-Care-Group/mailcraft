// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One strict way to read `id` and `ids` for the commands that write to other people (#553):
//! leaving them out takes the selection; anything else that names nothing is refused, and never
//! taken to mean the selection.

use serde_json::{Value, json};

use crate::agent::{Class, Verdict, check, describe};
use crate::{CmdError, Session, p};

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

/// Two messages that aren't drafts: the first is selected, the second is the one named.
fn two(s: &Session) -> (u64, u64) {
    let mut ids = s.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id);
    (ids.next().unwrap(), ids.next().unwrap())
}

/// Every way of naming nothing under `key` that isn't leaving it out.
fn wrong(key: &str) -> Vec<Value> {
    let mut v = vec![json!(null), json!("7"), json!(0), json!(7.5), json!(-1), json!(true), json!([7]), json!({})];
    if key == "ids" {
        v = vec![json!(null), json!("7"), json!(7), json!([]), json!([7, "8"]), json!([0]), json!([7.5]), json!([null])];
    }
    v.into_iter().map(|x| json!({ key: x })).collect()
}

#[test]
fn the_readers_take_only_what_names_one_thing() {
    assert_eq!(p::message_strict(&json!({})).unwrap(), None);
    assert_eq!(p::message_strict(&Value::Null).unwrap(), None);
    assert_eq!(p::message_strict(&json!({"id": 7})).unwrap(), Some(7));
    assert_eq!(p::message_strict(&json!({"id": 7.0})).unwrap(), Some(7));
    assert_eq!(p::message_strict(&json!({"ids": [7]})).unwrap(), Some(7));
    for params in wrong("id").into_iter().chain(wrong("ids")).chain([json!({"id": 7, "ids": [7]}), json!({"ids": [7, 8]}), json!("7"), json!([7])]) {
        assert!(matches!(p::message_strict(&params), Err(CmdError::Params(_))), "{params}");
    }
    assert_eq!(p::id_strict(&json!({}), "message").unwrap(), None);
    assert_eq!(p::id_strict(&json!({"message": 9}), "message").unwrap(), Some(9));
    for params in wrong("message").into_iter().chain([json!(9)]) {
        assert!(matches!(p::id_strict(&params, "message"), Err(CmdError::Params(_))), "{params}");
    }
}

#[test]
fn a_reply_or_send_again_named_wrongly_is_refused_and_changes_nothing() {
    let mut s = sample();
    let (selected, _) = two(&s);
    s.sel.messages = vec![selected];
    for command in ["mail.reply", "mail.replyAll", "mail.editDraft", "mail.sendAgain"] {
        for params in wrong("id").into_iter().chain(wrong("ids")).chain([json!({"ids": [selected, selected]})]) {
            let r = s.run(command, &params);
            assert!(matches!(r, Err(CmdError::Params(_))), "{command} {params}: {r:?}");
            assert!(s.compose.is_none(), "{command} {params}");
        }
    }
    // Left out, it is the selection; named as a list of one, that one.
    s.run("mail.reply", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().map(|c| c.in_reply_to.clone()), s.mailbox.message(selected).map(|m| m.message_id.clone()));
}

#[test]
fn a_reply_named_by_a_list_of_one_answers_that_one() {
    let mut s = sample();
    let (selected, named) = two(&s);
    s.sel.messages = vec![selected];
    s.run("mail.reply", &json!({"ids": [named]})).unwrap();
    assert_eq!(s.compose.as_ref().map(|c| c.in_reply_to.clone()), s.mailbox.message(named).map(|m| m.message_id.clone()));
}

#[test]
fn a_command_named_wrongly_with_nothing_selected_says_what_is_wrong() {
    let mut s = sample();
    s.sel.messages.clear();
    let r = s.run("mail.reply", &json!({"id": null}));
    assert!(matches!(r, Err(CmdError::Params(ref why)) if why.contains("`id`")), "{r:?}");
}

#[test]
fn an_appointment_named_wrongly_is_refused_and_kept() {
    let mut s = sample();
    let event = s.mailbox.events.values().next().map(|e| e.id).unwrap();
    let count = s.mailbox.events.len();
    s.sel.event = Some(event);
    for command in ["calendar.delete", "calendar.edit", "calendar.forward", "calendar.copy"] {
        for params in wrong("id") {
            let mut params = params;
            params["title"] = json!("Changed");
            params["to"] = json!("someone@example.org");
            params["start"] = json!("2026-11-02T10:00");
            let r = s.run(command, &params);
            assert!(matches!(r, Err(CmdError::Params(_))), "{command} {params}: {r:?}");
            assert_eq!(s.mailbox.events.len(), count, "{command} {params}");
            assert_ne!(s.mailbox.event(event).map(|e| e.title.as_str()), Some("Changed"));
        }
    }
    // Left out, it is the selected one.
    s.run("calendar.delete", &json!({})).unwrap();
    assert!(s.mailbox.event(event).is_none());
}

#[test]
fn an_answer_to_a_meeting_request_named_wrongly_is_refused() {
    let mut s = sample();
    let event = s.mailbox.events.values().next().map(|e| e.id).unwrap();
    s.sel.event = Some(event);
    let before = s.mailbox.event(event).cloned();
    for params in wrong("message") {
        let mut params = params;
        params["response"] = json!("accept");
        let r = s.run("calendar.respond", &params);
        assert!(matches!(r, Err(CmdError::Params(_))), "{params}: {r:?}");
        assert_eq!(s.mailbox.event(event).cloned(), before, "{params}");
        let r = s.run("calendar.invitation", &params);
        assert!(matches!(r, Err(CmdError::Params(_))), "{params}: {r:?}");
    }
}

#[test]
fn the_gate_asks_about_what_the_command_acts_on() {
    let mut s = sample();
    let (selected, named) = two(&s);
    s.sel.messages = vec![selected];
    let subject = |id| s.mailbox.message(id).map(|m| m.subject.clone()).unwrap();
    let said = describe(&s, "mail.reply", &json!({"ids": [named]}));
    assert!(said.contains(&subject(named)), "{said}");
    for params in wrong("id").into_iter().chain(wrong("ids")).chain([json!({"ids": [selected, named]})]) {
        for command in ["mail.reply", "mail.replyAll", "mail.sendAgain"] {
            assert!(matches!(check(&s, command, &params, &Class::ALL), Verdict::Refuse(_)), "{command} {params}");
        }
    }
}
