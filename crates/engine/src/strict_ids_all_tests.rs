// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every command that names what it acts on by `id` or `ids` reads them one strict way (#553):
//! leaving them out takes the selection (or the folder in view); anything else that names nothing
//! (`null`, an id as text, 0, a negative number, a fraction, a wrong list entry, both keys) is
//! refused and changes nothing, never taken to mean the selection.

use serde_json::{Value, json};

use crate::{CmdError, Session};

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

/// Each way of naming nothing under `id`.
fn wrong_id() -> Vec<Value> {
    [json!(null), json!("7"), json!(0), json!(-1), json!(7.5), json!(true), json!([7]), json!({})].into_iter().map(|x| json!({ "id": x })).collect()
}

/// Each way of naming no messages under `id` or `ids`, or under both.
fn wrong_ids() -> Vec<Value> {
    let ids = [json!(null), json!("7"), json!(7), json!([]), json!([7, "8"]), json!([0]), json!([-1]), json!([7.5]), json!([null])];
    let mut all = wrong_id();
    all.extend(ids.into_iter().map(|x| json!({ "ids": x })));
    all.push(json!({"id": 7, "ids": [7]}));
    all
}

/// How a command reads what it acts on.
#[derive(Clone, Copy, Debug)]
enum Reads {
    /// Messages, by `id` or `ids` (`p::ids_strict`).
    Messages,
    /// One message, by `id` or by `ids` naming one (`p::message_strict`).
    Message,
    /// One item (a contact, a task, a folder, an event, a calendar, an account) by `id` (`p::id_strict`).
    Item,
}

/// Every command left on #553 after #673, with the other parameters it needs to get as far as
/// reading what it acts on.
fn table(s: &Session) -> Vec<(&'static str, Reads, Value)> {
    let folder = s.view.folder;
    let mut t: Vec<(&'static str, Reads, Value)> = [
        "mail.markRead",
        "mail.markUnread",
        "mail.toggleRead",
        "mail.flag",
        "mail.clearFlag",
        "mail.complete",
        "mail.toggleFlag",
        "mail.clearCategories",
        "mail.pin",
        "mail.dismissWarnings",
        "mail.select",
        "mail.delete",
        "mail.deletePermanently",
        "mail.archive",
        "mail.junk",
        "mail.notJunk",
        "junk.blockSender",
        "junk.neverBlockSender",
        "junk.neverBlockDomain",
        "file.exportContacts",
    ]
    .into_iter()
    .map(|c| (c, Reads::Messages, json!({})))
    .collect();
    t.push(("mail.categorize", Reads::Messages, json!({"category": "Strict"})));
    t.push(("mail.move", Reads::Messages, json!({"folder": folder})));
    t.push(("mail.copy", Reads::Messages, json!({"folder": folder})));
    for c in ["mail.get", "mail.findFromSender", "file.exportMessage", "calendar.fromMessage", "tasks.fromMessage", "people.fromMessage"] {
        t.push((c, Reads::Message, json!({})));
    }
    for c in ["mail.saveAttachment", "mail.removeAttachment", "mail.openAttachment"] {
        t.push((c, Reads::Message, json!({"index": 0})));
    }
    for c in [
        "people.get",
        "people.delete",
        "people.favorite",
        "people.email",
        "tasks.get",
        "tasks.complete",
        "tasks.delete",
        "folder.delete",
        "folder.empty",
        "folder.select",
        "file.exportFolder",
        "file.exportCalendar",
        "calendar.timeZones",
    ] {
        t.push((c, Reads::Item, json!({})));
    }
    // The rule commands (#566): Always Move's message, the rule to switch, the one rule to run.
    t.push(("rules.fromMessage", Reads::Message, json!({"folder": folder})));
    t.push(("rules.enable", Reads::Item, json!({})));
    t.push(("rules.run", Reads::Item, json!({})));
    t
}

fn with(base: &Value, more: &Value) -> Value {
    let mut out = base.clone();
    if let (Some(o), Some(m)) = (out.as_object_mut(), more.as_object()) {
        o.extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    out
}

#[test]
fn every_command_refuses_an_id_or_ids_named_wrongly_and_changes_nothing() {
    let mut s = sample();
    let mut messages = s.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id);
    let (first, second) = (messages.next().unwrap(), messages.next().unwrap());
    s.sel.messages = vec![first];
    s.sel.contact = s.mailbox.contacts.values().next().map(|c| c.id);
    s.sel.task = s.mailbox.tasks.values().next().map(|t| t.id);
    s.sel.event = s.mailbox.events.values().next().map(|e| e.id);
    for (command, reads, base) in table(&s) {
        let mut cases = match reads {
            Reads::Messages | Reads::Message => wrong_ids(),
            Reads::Item => wrong_id(),
        };
        if command == "mail.select" {
            // `{"ids": []}` selects nothing, as documented.
            cases.retain(|c| c != &json!({"ids": []}));
        }
        if matches!(reads, Reads::Message) {
            cases.push(json!({"ids": [first, second]}));
        }
        for case in cases {
            let params = with(&base, &case);
            let (mailbox, sel, folder, search) = (s.mailbox.clone(), s.sel.clone(), s.view.folder, s.view.search.clone());
            let r = s.run(command, &params);
            assert!(matches!(r, Err(CmdError::Params(_))), "{command} {params}: {r:?}");
            assert!(s.mailbox == mailbox, "{command} {params} changed the mailbox");
            assert_eq!(s.sel, sel, "{command} {params} changed the selection");
            assert_eq!((s.view.folder, &s.view.search), (folder, &search), "{command} {params} changed the view");
            assert!(s.compose.is_none(), "{command} {params} started a message");
        }
    }
}

#[test]
fn left_out_the_selection_is_still_taken() {
    let mut s = sample();
    let first = s.mailbox.messages.values().find(|m| !m.draft && !m.read).map(|m| m.id).unwrap();
    s.sel.messages = vec![first];
    s.run("mail.markRead", &json!({})).unwrap();
    assert!(s.mailbox.message(first).unwrap().read);
    s.run("mail.flag", &json!({"ids": [first]})).unwrap();
    assert_eq!(s.mailbox.message(first).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
    let got = s.run("mail.get", &json!({})).unwrap();
    assert_eq!(got["id"], json!(first));
    let got = s.run("mail.get", &json!({"ids": [first]})).unwrap();
    assert_eq!(got["id"], json!(first));
    let contact = s.mailbox.contacts.values().next().map(|c| c.id).unwrap();
    s.sel.contact = Some(contact);
    assert_eq!(s.run("people.get", &json!({})).unwrap()["id"], json!(contact));
}

#[test]
fn select_takes_null_for_nothing_and_refuses_the_rest() {
    let mut s = sample();
    let event = s.mailbox.events.values().next().map(|e| e.id).unwrap();
    let contact = s.mailbox.contacts.values().next().map(|c| c.id).unwrap();
    let task = s.mailbox.tasks.values().next().map(|t| t.id).unwrap();
    for (command, id) in [("calendar.select", event), ("people.select", contact), ("tasks.select", task)] {
        s.run(command, &json!({"id": id})).unwrap();
        for params in wrong_id().into_iter().filter(|p| !p["id"].is_null()) {
            let sel = s.sel.clone();
            let r = s.run(command, &params);
            assert!(matches!(r, Err(CmdError::Params(_))), "{command} {params}: {r:?}");
            assert_eq!(s.sel, sel, "{command} {params}");
        }
        // `null` selects nothing, as documented.
        s.run(command, &json!({"id": null})).unwrap();
    }
    assert_eq!((s.sel.event, s.sel.contact, s.sel.task), (None, None, None));
}

#[test]
fn the_rule_commands_read_the_folder_and_on_or_off_as_strictly() {
    let mut s = sample();
    let rule = s.mailbox.rules.first().map(|r| (r.id, r.enabled)).unwrap();
    let wrong = [json!(null), json!("7"), json!(0), json!(-1), json!(7.5), json!(true), json!([7]), json!({})];
    let mut cases: Vec<(&str, Value)> = wrong.iter().map(|x| ("rules.run", json!({ "folder": x }))).collect();
    for x in [json!(null), json!("false"), json!("true"), json!(0), json!(1), json!([true]), json!({})] {
        cases.push(("rules.enable", json!({"id": rule.0, "value": x})));
    }
    for (command, params) in cases {
        let mailbox = s.mailbox.clone();
        let r = s.run(command, &params);
        assert!(matches!(r, Err(CmdError::Params(_))), "{command} {params}: {r:?}");
        assert!(s.mailbox == mailbox, "{command} {params} changed the mailbox");
    }
    // Left out, the folder in view, and on or off the other way.
    assert!(s.run("rules.run", &json!({})).is_ok());
    s.run("rules.enable", &json!({"id": rule.0})).unwrap();
    assert_eq!(s.mailbox.rules.first().map(|r| r.enabled), Some(!rule.1));
    s.run("rules.enable", &json!({"id": rule.0, "value": rule.1})).unwrap();
    assert_eq!(s.mailbox.rules.first().map(|r| r.enabled), Some(rule.1));
}
