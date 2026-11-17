// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Rules dialog: a new rule spelled out for the command, and rules as people read them.

use super::*;
use mailcraft_model::Condition;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn folder_id(s: &Session, path: &str) -> Id {
    s.mailbox.folders.iter().find(|f| s.mailbox.folder_path(f.id) == path).map(|f| f.id).unwrap()
}

#[test]
fn a_new_rule_is_spelled_out_for_the_command() {
    let mut s = session();
    let mut f = RulesForm::new(&s, &json!({}));
    // The sample has a rule, so the builder starts folded, with one condition and a move to Archive.
    assert!(!f.open && !f.ready());
    assert_eq!((f.conds.len(), f.acts.len(), f.first_folder, f.first_category.as_str()), (1, 1, folder_id(&s, "Archive"), "Red category"));
    let receipts = folder_id(&s, "Receipts");
    f.name = "Invoices".into();
    f.conds = vec![
        Cond { kind: When::From, text: "billing".into(), ..Default::default() },
        Cond { kind: When::To, text: "accounts@".into(), ..Default::default() },
        Cond { kind: When::Subject, text: "invoice".into(), ..Default::default() },
        Cond { kind: When::Body, text: "amount due".into(), ..Default::default() },
        Cond { kind: When::Attachment, text: "left over".into(), ..Default::default() },
    ];
    f.acts = vec![
        Act { kind: Do::Move, folder: receipts, ..f.fresh_action() },
        Act { kind: Do::Categorize, category: "Green category".into(), ..f.fresh_action() },
        Act { kind: Do::Flag, ..f.fresh_action() },
        Act { kind: Do::MarkRead, ..f.fresh_action() },
        Act { kind: Do::SetImportance, importance: Importance::Low, ..f.fresh_action() },
    ];
    f.stop = true;
    assert!(f.ready());
    let (command, params) = f.command();
    assert_eq!(command, "rules.add");
    assert_eq!(
        params,
        json!({
            "name": "Invoices",
            // For the mail of the account in view (#566).
            "account": s.mailbox.folder(s.view.folder).unwrap().account,
            "conditions": [
                {"type": "fromContains", "value": "billing"}, {"type": "toContains", "value": "accounts@"},
                {"type": "subjectContains", "value": "invoice"}, {"type": "bodyContains", "value": "amount due"}, {"type": "hasAttachment"},
            ],
            "actions": [
                {"type": "moveTo", "value": receipts}, {"type": "categorize", "value": "Green category"},
                {"type": "flag"}, {"type": "markRead"}, {"type": "setImportance", "value": "low"},
            ],
            "stop": true,
        })
    );
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let rule = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert!(rule.enabled && rule.stop);
    assert_eq!(
        describe(&s.mailbox, rule),
        "When from contains \"billing\" and sent to \"accounts@\" and subject contains \"invoice\" and body contains \"amount due\" and has an attachment: \
         move it to Receipts, categorize it as \"Green category\", flag it, mark it as read, set importance to low"
    );
    // The other kinds.
    f.conds = vec![Cond { kind: When::Importance, ..Default::default() }];
    f.acts = vec![Act { kind: Do::Delete, ..f.fresh_action() }];
    assert_eq!(
        f.command().1,
        json!({"name": "Invoices", "account": f.account, "conditions": [{"type": "importance", "value": "high"}], "actions": [{"type": "delete"}], "stop": true})
    );
    // Starting over.
    f.reset();
    assert_eq!(
        (f.name.as_str(), f.stop, f.conds.as_slice(), f.acts.as_slice()),
        ("", false, [Cond::default()].as_slice(), [f.fresh_action()].as_slice())
    );
}

#[test]
fn add_waits_for_what_a_rule_needs() {
    let s = session();
    let mut f = RulesForm::new(&s, &json!({}));
    f.name = "News".into();
    assert!(!f.ready());
    f.conds[0].text = " digest ".into();
    assert!(f.ready());
    // A move needs a folder, Categorize a category.
    f.acts[0].folder = 0;
    assert!(!f.ready());
    f.acts[0].kind = Do::Categorize;
    assert!(f.ready());
    f.acts[0].category.clear();
    assert!(!f.ready());
    f.acts[0].kind = Do::Flag;
    assert!(f.ready());
    // Conditions that take no text.
    f.conds = vec![Cond { kind: When::Attachment, ..Default::default() }, Cond { kind: When::Importance, ..Default::default() }];
    assert!(f.ready());
    f.name = " ".into();
    assert!(!f.ready());
    // On an empty mailbox the builder is unfolded: there is nothing else to show.
    assert!(RulesForm::new(&Session::empty(), &json!({})).open);
}

#[test]
fn rules_and_folders_read_as_people_know_them() {
    let mut s = session();
    let account = account_in_view(&s);
    let paths: Vec<String> = folder_choices(&s.mailbox, account).into_iter().map(|f| f.1).collect();
    assert_eq!(
        paths,
        ["Inbox", "Inbox/Projects", "Inbox/Projects/Atlas", "Drafts", "Sent Items", "Deleted Items", "Archive", "Junk Email", "Outbox", "Receipts"]
    );
    assert!(folder_choices(&s.mailbox, 999_999).is_empty());
    let atlas = folder_id(&s, "Inbox/Projects/Atlas");
    let rule = Rule {
        conditions: vec![Condition::SubjectContains("Atlas".into())],
        actions: vec![Action::MoveTo(atlas), Action::MarkRead],
        ..Default::default()
    };
    assert_eq!(describe(&s.mailbox, &rule), "When subject contains \"Atlas\": move it to Inbox/Projects/Atlas, mark it as read");
    // The folder went away.
    let gone = Rule { actions: vec![Action::MoveTo(999_999)], ..rule };
    assert_eq!(describe(&s.mailbox, &gone), "When subject contains \"Atlas\": move it to a folder that is gone");
    // The rule the sample comes with.
    let first = s.mailbox.rules.first().cloned().unwrap();
    assert_eq!(describe(&s.mailbox, &first), "When from contains \"harborbooks\": categorize it as \"Purple category\"");
    // Hostile text in a rule is the command's to cut down.
    let junk = "\u{0}\u{202e}é\n".repeat(40_000);
    let mut f = RulesForm::new(&s, &json!({}));
    f.name.clone_from(&junk);
    f.conds[0].text.clone_from(&junk);
    f.acts[0] = Act { kind: Do::Categorize, category: junk.clone(), ..f.fresh_action() };
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let rule = s.mailbox.rules.iter().find(|r| r.id == id).cloned().unwrap();
    assert!(rule.name.chars().count() <= mailcraft_model::MAX_NAME);
    assert!(shown(&describe(&s.mailbox, &rule), 600).chars().count() <= 601);
}
