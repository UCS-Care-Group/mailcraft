// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules that couldn't tell of new mail its server listed before it was fetched (#566): they
//! wait with it ([`Message::rules_waiting`]) and have their say once it is here whole.

use crate::*;

fn words(w: &str) -> Vec<String> {
    vec![w.to_string()]
}

/// A mailbox with an account, its Inbox and a folder "Kept"; the rules `rules` (ids from 1).
fn mailbox(rules: Vec<Rule>) -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let kept = mb.add_folder(account, None, "Kept").unwrap();
    for (n, mut rule) in rules.into_iter().enumerate() {
        (rule.id, rule.enabled) = (n as Id + 1, true);
        mb.rules.push(rule);
    }
    (mb, inbox, kept)
}

/// A message as a server lists it: its headers and a preview.
fn listed(mb: &mut Mailbox, inbox: Id) -> Id {
    let m = Message {
        folder: inbox,
        from: Address::new("Zoe", "zoe@example.org"),
        subject: "Report".into(),
        body: Body { text: "About the report".into(), html: None },
        unfetched: true,
        ..Default::default()
    };
    mb.add_message(m).unwrap()
}

/// The fetch: the text, the headers, and it is whole.
fn fetched(mb: &mut Mailbox, id: Id, text: &str) {
    let m = mb.message_mut(id).unwrap();
    m.body.text = text.into();
    m.unfetched = false;
}

fn clock() -> RuleClock {
    RuleClock { now: 1_800_000_000, zone: crate::time::Zone::UTC, due_hour: 17 }
}

#[test]
fn a_rule_that_couldnt_tell_acts_once_the_message_is_whole_and_one_that_acted_isnt_run_again() {
    let unless = Rule {
        conditions: vec![Condition::FromContains(words("zoe"))],
        exceptions: vec![Condition::BodyContains(words("unsubscribe"))],
        actions: vec![Action::Flag],
        ..Default::default()
    };
    let copy =
        |kept| Rule { conditions: vec![Condition::SubjectContains(words("report"))], actions: vec![Action::CopyTo(kept)], ..Default::default() };
    let (mut mb, inbox, kept) = mailbox(vec![]);
    for (n, mut rule) in [unless, copy(kept)].into_iter().enumerate() {
        (rule.id, rule.enabled) = (n as Id + 1, true);
        mb.rules.push(rule);
    }
    let m = listed(&mut mb, inbox);
    assert_eq!(mb.apply_rules(m, &clock()), ["".to_string()], "the copy, which could tell");
    assert_eq!(mb.messages_in(kept).count(), 1);
    let waiting = mb.message(m).unwrap().rules_waiting.clone().unwrap();
    assert_eq!(*waiting, RulesWaiting { folder: inbox, rules: vec![RuleWaiting { rule: 1, delete_only: false }] });
    // A copy of it doesn't wait for anything.
    let other = mb.add_folder(mb.accounts[0].id, None, "Other").unwrap();
    let copied = mb.copy_message(m, other).unwrap();
    assert!(mb.message(copied).unwrap().rules_waiting.is_none());
    // Kept with the message in the file, and a file without it (or with nonsense there) has none.
    let json = serde_json::to_string(mb.message(m).unwrap()).unwrap();
    assert!(json.contains(r#""rulesWaiting":{"folder":"#), "{json}");
    assert_eq!(serde_json::from_str::<Message>(&json).unwrap().rules_waiting, Some(waiting));
    assert!(serde_json::from_str::<Message>(r#"{"id": 3, "rulesWaiting": "soon"}"#).unwrap().rules_waiting.is_none());
    // Not while it is only listed.
    assert!(mb.run_waiting_rules(m, &clock()).is_empty());
    assert!(mb.message(m).unwrap().rules_waiting.is_some());
    fetched(&mut mb, m, "The report, in full.");
    assert_eq!(mb.run_waiting_rules(m, &clock()).len(), 1);
    let got = mb.message(m).unwrap();
    assert!(got.flag == FlagStatus::Flagged && got.rules_waiting.is_none());
    assert_eq!(mb.messages_in(kept).count(), 1, "the copy isn't made twice");
    // Once is all: back to its preview and fetched again, nothing runs.
    mb.message_mut(m).unwrap().flag = FlagStatus::None;
    mb.message_mut(m).unwrap().unfetched = true;
    fetched(&mut mb, m, "The report, in full.");
    assert!(mb.run_waiting_rules(m, &clock()).is_empty());
    assert_eq!(mb.message(m).unwrap().flag, FlagStatus::None);
}

#[test]
fn an_exception_that_holds_once_the_text_is_here_leaves_the_message_alone() {
    let unless = Rule {
        conditions: vec![Condition::FromContains(words("zoe"))],
        exceptions: vec![Condition::BodyContains(words("unsubscribe"))],
        actions: vec![Action::Flag],
        ..Default::default()
    };
    let (mut mb, inbox, _) = mailbox(vec![unless]);
    let m = listed(&mut mb, inbox);
    mb.apply_rules(m, &clock());
    fetched(&mut mb, m, "To unsubscribe, write back.");
    assert!(mb.run_waiting_rules(m, &clock()).is_empty());
    let got = mb.message(m).unwrap();
    assert!(got.flag == FlagStatus::None && got.rules_waiting.is_none());
}

#[test]
fn a_delete_for_good_that_waited_on_a_rule_before_it_goes_only_if_that_rule_doesnt_apply() {
    let rules = |kept| {
        vec![
            // Couldn't tell, and stops the rules after it.
            Rule { conditions: vec![Condition::BodyContains(words("keep"))], actions: vec![Action::MoveTo(kept)], stop: true, ..Default::default() },
            Rule {
                conditions: vec![Condition::SubjectContains(words("report"))],
                actions: vec![Action::MarkRead, Action::DeleteForGood],
                ..Default::default()
            },
        ]
    };
    for (text, gone) in [("Nothing to see.", true), ("Please keep this.", false)] {
        let (mut mb, inbox, kept) = mailbox(vec![]);
        for (n, mut rule) in rules(kept).into_iter().enumerate() {
            (rule.id, rule.enabled) = (n as Id + 1, true);
            mb.rules.push(rule);
        }
        let m = listed(&mut mb, inbox);
        mb.apply_rules(m, &clock());
        // Marked read as it arrived, and not deleted: the rule before may yet apply.
        let got = mb.message(m).unwrap();
        assert!(got.read && got.folder == inbox, "{text}");
        assert_eq!(
            got.rules_waiting.as_deref().map(|w| w.rules.clone()),
            Some(vec![RuleWaiting { rule: 1, delete_only: false }, RuleWaiting { rule: 2, delete_only: true }])
        );
        fetched(&mut mb, m, text);
        mb.run_waiting_rules(m, &clock());
        match mb.message(m) {
            None => assert!(gone, "{text}"),
            Some(got) => assert!(!gone && got.folder == kept, "{text}"),
        }
    }
}

#[test]
fn the_waiting_rules_let_alone_a_message_moved_before_it_was_fetched() {
    let rule = Rule { conditions: vec![Condition::BodyContains(words("invoice"))], actions: vec![Action::DeleteForGood], ..Default::default() };
    let (mut mb, inbox, kept) = mailbox(vec![rule]);
    let m = listed(&mut mb, inbox);
    mb.apply_rules(m, &clock());
    assert!(mb.message(m).unwrap().rules_waiting.is_some());
    // The owner filed it meanwhile.
    mb.move_message(m, kept).unwrap();
    fetched(&mut mb, m, "The invoice, in full.");
    assert!(mb.run_waiting_rules(m, &clock()).is_empty());
    let got = mb.message(m).unwrap();
    assert!(got.folder == kept && got.rules_waiting.is_none());
}

#[test]
fn nothing_waits_for_a_message_that_is_here_whole_or_found_or_run_by_hand() {
    let rule = Rule { conditions: vec![Condition::BodyContains(words("report"))], actions: vec![Action::Flag], ..Default::default() };
    let (mut mb, inbox, _) = mailbox(vec![rule]);
    // Here whole, the rule tells at once.
    let whole = listed(&mut mb, inbox);
    fetched(&mut mb, whole, "No.");
    mb.apply_rules(whole, &clock());
    assert!(mb.message(whole).unwrap().rules_waiting.is_none());
    // Mail its server had before, and Run Rules Now: not new mail.
    let found = listed(&mut mb, inbox);
    mb.apply_rules_found(found, &clock());
    mb.run_rules(found, true, &clock());
    assert!(mb.message(found).unwrap().rules_waiting.is_none());
}
