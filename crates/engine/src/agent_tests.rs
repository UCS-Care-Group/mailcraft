// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The agent gate: what it asks about, and what it shows when it asks.

use mailcraft_model::{Action, Condition, Contact, FolderKind, Id, Rule, Task};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::*;

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

fn folder_of(s: &Session, kind: FolderKind) -> Id {
    s.mailbox.folders.iter().find(|f| f.kind == kind).map(|f| f.id).unwrap()
}

fn rule_to(folder: Id) -> Value {
    json!({"name": "relay", "conditions": [{"type": "fromContains", "value": "x"}], "actions": [{"type": "moveTo", "value": folder}]})
}

#[test]
fn a_rule_never_moves_mail_to_an_outbox() {
    let mut s = sample();
    let out = folder_of(&s, FolderKind::Outbox);
    let before = s.mailbox.rules.len();
    assert!(s.run("rules.add", &rule_to(out)).is_err());
    assert_eq!(s.mailbox.rules.len(), before);
    // Nor one made from a message.
    let m = *s.mailbox.messages.keys().next().unwrap();
    assert!(s.run("rules.fromMessage", &json!({"folder": out, "id": m})).is_err());
    assert_eq!(s.mailbox.rules.len(), before);
    // A folder under an Outbox is a send queue too.
    let account = s.mailbox.folder(out).unwrap().account;
    if let Ok(sub) = s.mailbox.add_folder(account, Some(out), "Queue") {
        assert!(s.mailbox.is_send_queue(sub));
        assert!(s.run("rules.add", &rule_to(sub)).is_err());
    }
    // An ordinary folder is fine.
    let archive = folder_of(&s, FolderKind::Archive);
    assert!(s.run("rules.add", &rule_to(archive)).is_ok());
}

#[test]
fn a_rule_from_a_loaded_mailbox_does_not_fill_an_outbox() {
    let mut s = sample();
    let out = folder_of(&s, FolderKind::Outbox);
    let m = *s.mailbox.messages.keys().next().unwrap();
    // A rule that holds for the message: its whole subject.
    let subject = s.mailbox.message(m).unwrap().subject.clone();
    assert!(!subject.trim().is_empty());
    s.mailbox.rules.push(Rule {
        id: 9_999,
        name: "hostile".into(),
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec![subject])],
        actions: vec![Action::MoveTo(out)],
        stop: false,
        ..Default::default()
    });
    let from = s.mailbox.message(m).unwrap().folder;
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).unwrap().folder, from);
    // The same rule to an ordinary folder moves it: the rule held, and the Outbox was refused.
    let archive = folder_of(&s, FolderKind::Archive);
    if let Some(r) = s.mailbox.rules.iter_mut().find(|r| r.id == 9_999) {
        r.actions = vec![Action::MoveTo(archive)];
    }
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert_eq!(s.mailbox.message(m).unwrap().folder, archive);
}

/// The same for a rule's copy (#566).
#[test]
fn a_rule_from_a_loaded_mailbox_does_not_copy_into_an_outbox() {
    let mut s = sample();
    let out = folder_of(&s, FolderKind::Outbox);
    let m = *s.mailbox.messages.keys().next().unwrap();
    let subject = s.mailbox.message(m).unwrap().subject.clone();
    s.mailbox.rules.push(Rule {
        id: 9_999,
        name: "hostile".into(),
        enabled: true,
        conditions: vec![Condition::SubjectContains(vec![subject.clone()])],
        actions: vec![Action::CopyTo(out)],
        ..Default::default()
    });
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert!(!s.mailbox.messages_in(out).any(|c| c.subject == subject), "a copy of it is in the Outbox");
    // (The same rule into the Archive copies it.)
    let archive = folder_of(&s, FolderKind::Archive);
    if let Some(r) = s.mailbox.rules.iter_mut().find(|r| r.id == 9_999) {
        r.actions = vec![Action::CopyTo(archive)];
    }
    s.mailbox.apply_rules(m, &s.rule_clock());
    assert!(s.mailbox.messages_in(archive).any(|c| c.subject == subject));
}

#[test]
fn the_question_about_a_rule_names_its_folder() {
    let s = sample();
    let archive = folder_of(&s, FolderKind::Archive);
    let d = describe(&s, "rules.add", &rule_to(archive));
    assert!(d.contains("relay") && d.contains("move it to Archive") && d.contains("from contains \"x\""), "{d}");
}

#[test]
fn agents_are_asked_before_deleting_contacts_tasks_events_and_categories() {
    let mut s = sample();
    let c = s.mailbox.add_contact(Contact { display_name: "Ada Example".into(), ..Default::default() });
    let t = s.mailbox.add_task(Task { title: "Pay the invoice".into(), ..Default::default() });
    let e = *s.mailbox.events.keys().next().unwrap();
    for (id, params) in [
        ("people.delete", json!({"id": c})),
        ("tasks.delete", json!({"id": t})),
        ("calendar.delete", json!({"id": e})),
        ("category.remove", json!({"name": "Red"})),
    ] {
        assert!(matches!(check(&s, id, &params, &[]), Verdict::Ask(Class::Delete, _)), "{id}");
        assert_eq!(check(&s, id, &params, &[Class::Delete]), Verdict::Run, "{id}");
    }
    let d = describe(&s, "people.delete", &json!({"id": c}));
    assert!(d.contains("Ada Example") && d.contains("permanently"), "{d}");
    assert!(describe(&s, "tasks.delete", &json!({"id": t})).contains("Pay the invoice"));
    // Telling the people invited too is the stronger question.
    assert!(matches!(check(&s, "calendar.delete", &json!({"id": e, "send": true}), &[Class::Delete]), Verdict::Ask(Class::Send, _)));
}

#[test]
fn a_question_is_shown_in_full_or_refused() {
    let s = sample();
    // A key that sorts first can't push the real params out of sight (of a command asked about
    // by its params, which Run Rules Now no longer is: it names the folder and the rule).
    let junk = "z".repeat(250);
    let params = json!({"0": junk, "data": "visible-end", "name": "x"});
    let d = describe(&s, "rules.list", &params);
    assert!(d.contains(&junk) && d.contains("visible-end"), "{d}");
    // Too long to show: refused, not cut.
    let huge = json!({"0": "z".repeat(MAX_QUESTION * 2)});
    assert!(matches!(check(&s, "rules.list", &huge, &[]), Verdict::Refuse(_)));
    assert!(matches!(check(&s, "rules.list", &params, &[]), Verdict::Ask(Class::Settings, _)));
    // Every recipient is counted, however many.
    let mut s = sample();
    let many: Vec<String> = (0..40).map(|i| format!("p{i}@example.org")).collect();
    s.run("mail.new", &json!({"to": many.join("; "), "subject": "Hi"})).unwrap();
    let d = describe(&s, "compose.send", &json!({}));
    assert!(d.contains("p0@example.org") && d.contains("and 28 more"), "{d}");
}

#[test]
fn the_question_about_an_outbox_says_what_that_outbox_does() {
    let mut s = sample();
    let m = *s.mailbox.messages.keys().next().unwrap();
    let sends = folder_of(&s, FolderKind::Outbox);
    let imap =
        s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let holds = s.mailbox.folder_of_kind(imap, FolderKind::Outbox).unwrap();
    for (id, verb) in [("mail.move", "move"), ("mail.copy", "copy")] {
        let d = describe(&s, id, &json!({"ids": [m], "folder": sends}));
        assert!(d.ends_with("in the Outbox, which sends them"), "{d}");
        let d = describe(&s, id, &json!({"ids": [m], "folder": holds}));
        assert!(d.starts_with(verb) && d.ends_with("into the Outbox of ada@mail.example, held there unsent until Send Again"), "{d}");
    }
    // Imported mail never lies in an Outbox, of whichever kind.
    for folder in [sends, holds] {
        let d = describe(&s, "file.import", &json!({"folder": folder}));
        assert_eq!(d, "import a file with an Outbox chosen: no mail goes in it, and none is sent");
    }
}

#[test]
fn the_calendar_question_names_the_event_and_the_invitees() {
    let mut s = sample();
    let e = *s.mailbox.events.keys().next().unwrap();
    s.sel.event = Some(e);
    let selected = s.mailbox.event(e).unwrap().title.clone();
    let params = json!({"title": "Quarterly review", "start": "2026-11-02T10:00", "attendees": "a@example.org; b@example.org", "send": true});
    for id in ["calendar.new", "calendar.newMeeting"] {
        let d = describe(&s, id, &params);
        assert!(
            d.contains("Quarterly review") && d.contains("2026-11-02T10:00") && d.contains("a@example.org") && d.contains("b@example.org"),
            "{d}"
        );
        assert!(d.contains("2 people") && !d.contains(&selected), "{d}");
    }
}

#[test]
fn undo_that_would_revert_rules_or_accounts_asks() {
    let mut s = sample();
    let archive = folder_of(&s, FolderKind::Archive);
    // An ordinary step: safe.
    let m = *s.mailbox.messages.keys().next().unwrap();
    s.run("mail.flag", &json!({"id": m})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    // The owner's own rule: undoing it is for the owner.
    s.run("rules.add", &rule_to(archive)).unwrap();
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Settings, _)));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Settings]), Verdict::Run);
    assert!(describe(&s, "edit.undo", &json!({})).contains("rules, signatures or Safe and Blocked Senders"));
    // After the undo, redo would put the rule back.
    assert!(s.undo());
    assert!(matches!(check(&s, "edit.redo", &json!({}), &[]), Verdict::Ask(Class::Settings, _)));
}

#[test]
fn diagnostics_are_not_for_agents() {
    assert_eq!(access("ui.copyDiagnostics"), Some(Access::Never));
}

/// Removing or changing an account names it, a local mailbox (which has no address) as "Local
/// mailbox", and removing the only account says that it leaves an empty mailbox (#313).
#[test]
fn the_account_question_names_a_local_mailbox_and_the_only_account() {
    use mailcraft_model::{Account, Mailbox, Message};
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let local = s.mailbox.accounts[0].id;
    // The placeholder of an empty mailbox isn't removed (the command says so): no empty mailbox is promised.
    assert_eq!(s.mailbox.placeholder(), Some(local));
    assert_eq!(describe(&s, "account.remove", &json!({"id": local})), "remove the account Local mailbox");
    // With mail in it, it is a local mailbox, and the only account.
    let inbox = folder_of(&s, FolderKind::Inbox);
    s.mailbox.add_message(Message { account: local, folder: inbox, ..Default::default() }).unwrap();
    let only = "remove the account Local mailbox (the only account: leaves an empty mailbox)";
    assert_eq!(describe(&s, "account.remove", &json!({"id": local})), only);
    assert_eq!(describe(&s, "account.edit", &json!({"id": local, "name": "Home"})), "change the account Local mailbox");
    assert!(matches!(check(&s, "account.remove", &json!({"id": local}), &[]), Verdict::Ask(Class::Accounts, q) if q.contains(only)));
    // Its shared mailboxes go with it: still the only account.
    let shared = s.mailbox.add_account(Account { email: "team@example.org".into(), shared_of: Some(local), ..Default::default() });
    assert_eq!(describe(&s, "account.remove", &json!({"id": local})), only);
    // Beside another account, neither is the only one.
    let other = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(describe(&s, "account.remove", &json!({"id": local})), "remove the account Local mailbox");
    assert_eq!(describe(&s, "account.remove", &json!({"id": other})), "remove the account ada@example.org");
    assert_ne!(shared, other);
    // An address made only of spaces or control characters is no name either; an account that
    // isn't there leaves nothing dangling.
    s.mailbox.account_mut(other).unwrap().email = " \u{7}\t".into();
    assert_eq!(describe(&s, "account.remove", &json!({"id": other})), "remove the account Local mailbox");
    assert_eq!(describe(&s, "account.remove", &json!({"id": 999_999})), "remove an account that isn't there");
    assert_eq!(describe(&s, "account.edit", &json!({"id": 999_999})), "change an account that isn't there");
}

/// Run Rules Now named wrongly (`folder` as text or null, `id` as text) is refused by the command,
/// and the question says so: it doesn't name the folder in view, nor a rule there, as if they
/// would run (#566).
#[test]
fn run_rules_named_wrongly_is_said_as_that_not_as_the_folder_in_view() {
    let mut s = sample();
    let view = s.view.folder;
    let path = s.mailbox.folder_path(view);
    let subject = s.mailbox.messages_in(view).map(|m| m.subject.clone()).find(|t| !t.trim().is_empty()).unwrap();
    let rule = json!({"name": "Gone", "conditions": [{"type": "subjectContains", "value": subject}], "actions": [{"type": "delete"}]});
    let rule = s.run("rules.add", &rule).unwrap()["id"].as_u64().unwrap();
    assert!(describe(&s, "rules.run", &json!({})).contains(&path));
    for bad in [json!({"folder": view.to_string()}), json!({"folder": null}), json!({"id": rule.to_string()})] {
        assert!(s.run("rules.run", &bad).is_err(), "the command refuses {bad}");
        let what = describe(&s, "rules.run", &bad);
        assert!(!what.contains(&path) && !what.contains("Gone"), "{bad}: {what}");
    }
}

/// A word with a double quote in it doesn't read, in the owner's question, as the end of the word
/// and more parts after it: a rule that spares the boss's mail and one whose one exception is the
/// text `x" or from contains "boss`, which spares nearly nothing, are asked about in other words
/// (#566).
#[test]
fn a_quote_in_a_word_doesnt_read_as_another_exception() {
    let s = sample();
    let account = s.mailbox.accounts.first().map(|a| a.id).unwrap();
    let rule = |exceptions: Value| {
        json!({"name": "Clean up", "account": account, "conditions": [{"type": "subjectContains", "value": "invoice"}],
            "exceptions": exceptions, "actions": [{"type": "delete"}]})
    };
    let real = rule(json!([{"type": "subjectContains", "value": "x"}, {"type": "fromContains", "value": "boss"}]));
    let forged = rule(json!([{"type": "subjectContains", "value": "x\" or from contains \"boss"}]));
    let (real, forged) = (describe(&s, "rules.add", &real), describe(&s, "rules.add", &forged));
    assert_ne!(real, forged, "the owner is asked the same about both: {real}");
    assert!(forged.contains(r#"subject contains "x\" or from contains \"boss""#), "{forged}");
}

/// An agent's change of an account's address, with two rules switched on here that read that
/// address and move mail onto another account's server: the question names both, and its verbs
/// agree with "the rules".
#[test]
fn the_account_question_about_two_rules_that_read_its_address_agrees_in_number() {
    let mut s = sample();
    let a = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let other = s.run("account.add", &json!({"email": "elsewhere@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let there = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    for name in ["One", "Two"] {
        let rule = json!({"name": name, "account": a, "conditions": [{"type": "toMe"}], "actions": [{"type": "moveTo", "value": there}]});
        s.run("rules.add", &rule).unwrap();
    }
    let q = describe(&s, "account.edit", &json!({"id": a, "email": "lovelace@example.org"}));
    assert!(
        q.contains(
            "the rules \u{201c}One\u{201d}, \u{201c}Two\u{201d}, switched on here, put mail onto another account's server, and the rules for that mail read"
        ),
        "{q}"
    );
}
