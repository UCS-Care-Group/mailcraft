// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Microsoft account's server (#566), against a small stand-in on loopback that
//! keeps the Inbox's `messageRules` and answers as the documented service does. Nothing here
//! leaves this machine.

use std::sync::{Arc, Mutex};

use mailcraft_model::remote_rules::{RemoteRule, RuleOp, RuleOpKind, RuleResult, RulesUpload};
use mailcraft_model::{Action, Condition, Importance, Sensitivity};
use serde_json::{Value, json};

use super::rules::{push_rules_at, rule_body, rule_of, rules_at};
use crate::fake::{FakeHttp, json_answer};

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";

#[derive(Default)]
struct Server {
    rules: Vec<Value>,
    /// Folders: an id a rule may give, and the immutable id it is known by.
    folders: Vec<(String, String)>,
    /// Answer every write with this status instead of doing it.
    write_status: Option<u16>,
    /// Answer every look-up of a folder with this status.
    folder_status: Option<u16>,
    /// Answer a change to a rule it says is read-only with this status; with none, answer yes and
    /// leave the rule as it was.
    read_only_status: Option<u16>,
    /// Every request, as `METHOD target` with its `Prefer` header.
    asked: Vec<String>,
    next: u32,
}

fn raw(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

fn serve(state: &Arc<Mutex<Server>>) -> FakeHttp {
    let state = state.clone();
    FakeHttp::start(move |_, request| {
        let mut server = state.lock().unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        let line = head.lines().next().unwrap_or("");
        let prefer = head.lines().find_map(|l| l.strip_prefix("Prefer: ")).unwrap_or("");
        server.asked.push(format!("{line} [{prefer}]"));
        let mut parts = line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let path = target.split('?').next().unwrap_or("");
        let pieces: Vec<&str> = path.split('/').collect();
        let json = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
        // ["", "v1.0", "me", "mailFolders", {folder}, "messageRules", {rule}]
        if pieces.get(1..4) != Some(&["v1.0", "me", "mailFolders"]) {
            return raw("404 Not Found");
        }
        match (method, pieces.get(4), pieces.get(5), pieces.get(6)) {
            ("GET", Some(_), None, None) if server.folder_status.is_some() => raw(&format!("{} Other", server.folder_status.unwrap_or(0))),
            ("GET", Some(folder), None, None) => match server.folders.iter().find(|f| f.0 == *folder) {
                Some((_, id)) => json_answer("200 OK", &json!({"id": id}).to_string()),
                None => raw("404 Not Found"),
            },
            ("GET", Some(&"inbox"), Some(&"messageRules"), None) => json_answer("200 OK", &json!({"value": server.rules}).to_string()),
            (write, Some(&"inbox"), Some(&"messageRules"), id) => {
                if let Some(status) = server.write_status {
                    return raw(&format!("{status} Other"));
                }
                server.next += 1;
                let made = format!("R{}", server.next);
                match (write, id) {
                    ("POST", None) => {
                        let mut rule = json;
                        rule["id"] = json!(made);
                        server.rules.push(rule.clone());
                        json_answer("201 Created", &rule.to_string())
                    }
                    ("PATCH", Some(id)) => match server.rules.iter_mut().find(|r| r["id"] == *id) {
                        Some(rule) if rule["isReadOnly"] == true => {
                            let rule = rule.to_string();
                            match server.read_only_status {
                                Some(status) => raw(&format!("{status} Other")),
                                None => json_answer("200 OK", &rule),
                            }
                        }
                        Some(rule) => {
                            for (k, v) in json.as_object().cloned().unwrap_or_default() {
                                rule[k] = v;
                            }
                            json_answer("200 OK", &rule.to_string())
                        }
                        None => raw("404 Not Found"),
                    },
                    ("DELETE", Some(id)) => {
                        let before = server.rules.len();
                        server.rules.retain(|r| r["id"] != *id);
                        raw(if server.rules.len() < before { "204 No Content" } else { "404 Not Found" })
                    }
                    _ => raw("405 Method Not Allowed"),
                }
            }
            _ => raw("404 Not Found"),
        }
    })
}

fn state(rules: Vec<Value>) -> Arc<Mutex<Server>> {
    Arc::new(Mutex::new(Server { rules, ..Default::default() }))
}

fn upload(ops: Vec<RuleOp>) -> RulesUpload {
    RulesUpload { account: 1, email: "ada@example.org".into(), ops }
}

fn news_rule() -> RemoteRule {
    RemoteRule {
        name: "News".into(),
        sequence: 3,
        enabled: true,
        conditions: vec![Condition::FromContains(vec!["news@example.org".into()]), Condition::HasAttachment],
        actions: vec![Action::Categorize("Blue".into()), Action::MarkRead],
        move_to: "IMM-news".into(),
        stop: true,
        ..Default::default()
    }
}

#[test]
fn a_rule_of_known_parts_is_read_whole() {
    let rule = rule_of(&json!({
        "id": "A1", "displayName": "From the shop", "sequence": 2, "isEnabled": true, "isReadOnly": false, "hasError": false,
        "conditions": {"senderContains": ["shop@example.org"], "subjectContains": ["Order"], "importance": "high",
                       "hasAttachments": true, "bodyContains": null, "isEncrypted": false, "categories": []},
        "actions": {"moveToFolder": "IMM-shop", "assignCategories": ["Orders", "Shop"], "markAsRead": true,
                    "markImportance": "low", "delete": false, "stopProcessingRules": true, "forwardTo": []}
    }))
    .unwrap();
    assert_eq!(rule.id, "A1");
    assert_eq!(rule.name, "From the shop");
    assert_eq!(rule.sequence, 2);
    assert!(rule.enabled && rule.stop && !rule.read_only && !rule.error);
    assert!(rule.unknown.is_empty(), "{:?}", rule.unknown);
    assert_eq!(rule.move_to, "IMM-shop");
    assert!(rule.conditions.contains(&Condition::FromContains(vec!["shop@example.org".into()])));
    assert!(rule.conditions.contains(&Condition::SubjectContains(vec!["Order".into()])));
    assert!(rule.conditions.contains(&Condition::Importance(Importance::High)));
    assert!(rule.conditions.contains(&Condition::HasAttachment));
    assert_eq!(rule.conditions.len(), 4);
    assert!(rule.actions.contains(&Action::Categorize("Orders".into())));
    assert!(rule.actions.contains(&Action::Categorize("Shop".into())));
    assert!(rule.actions.contains(&Action::MarkRead));
    assert!(rule.actions.contains(&Action::SetImportance(Importance::Low)));
    assert!(!rule.actions.contains(&Action::Delete));
}

#[test]
fn parts_this_version_does_not_know_are_named() {
    let rule = rule_of(&json!({
        "id": "A2", "displayName": "Odd", "sequence": 1, "isEnabled": true,
        "conditions": {"senderContains": ["a@example.org"], "isEncrypted": true, "messageActionFlag": "call"},
        "exceptions": {"subjectContains": ["private"], "isSigned": true},
        // An action Graph doesn't have now, as a later one may add.
        "actions": {"playSound": true, "delete": true}
    }))
    .unwrap();
    for part in ["isEncrypted", "messageActionFlag", "exceptions.isSigned", "playSound"] {
        assert!(rule.unknown.iter().any(|u| u == part), "{part} in {:?}", rule.unknown);
    }
    assert_eq!(rule.unknown.len(), 4, "{:?}", rule.unknown);
    assert_eq!(rule.conditions, vec![Condition::FromContains(vec!["a@example.org".into()])]);
    assert_eq!(rule.exceptions, vec![Condition::SubjectContains(vec!["private".into()])]);
    assert_eq!(rule.actions, vec![Action::Delete]);
    // A rule without conditions acts on every message there: not one that is run here.
    let every = rule_of(&json!({"id": "A3", "displayName": "All", "conditions": {}, "actions": {"markAsRead": true}})).unwrap();
    assert_eq!(every.unknown, vec!["conditions".to_string()]);
    // No id: no rule.
    assert!(rule_of(&json!({"displayName": "x"})).is_none());
    assert!(rule_of(&json!({"id": "has space"})).is_none());
}

#[test]
fn every_condition_exception_and_action_of_outlooks_is_read_whole_and_written_back() {
    let people = |a: &[&str]| json!(a.iter().map(|a| json!({"emailAddress": {"name": "N", "address": a}})).collect::<Vec<_>>());
    let entry = json!({
        "id": "A4", "displayName": "Everything", "sequence": 3, "isEnabled": true,
        "conditions": {
            "senderContains": ["shop", "store"], "bodyOrSubjectContains": ["order"], "headerContains": ["X-Shop"],
            "fromAddresses": people(&["ann@example.org"]), "sentToAddresses": people(&["team@example.org", "ops@example.org"]),
            "sensitivity": "private", "withinSizeRange": {"minimumSize": 10, "maximumSize": 500},
            "isMeetingRequest": true, "isAutomaticReply": false, "categories": ["Red"], "messageActionFlag": "any",
            "sentOnlyToMe": true, "sentToMe": true, "sentCcMe": true, "sentToOrCcMe": true, "notSentToMe": true
        },
        "exceptions": {"isAutomaticReply": true, "subjectContains": ["draft", "test"]},
        "actions": {"copyToFolder": "IMM-copies", "markAsRead": true, "stopProcessingRules": false}
    });
    let rule = rule_of(&entry).unwrap();
    assert!(rule.unknown.is_empty(), "{:?}", rule.unknown);
    for c in [
        Condition::FromContains(vec!["shop".into(), "store".into()]),
        Condition::SubjectOrBodyContains(vec!["order".into()]),
        Condition::HeaderContains(vec!["X-Shop".into()]),
        Condition::From(vec!["ann@example.org".into()]),
        Condition::SentTo(vec!["team@example.org".into(), "ops@example.org".into()]),
        Condition::Sensitivity(Sensitivity::Private),
        Condition::SizeBetween { min: 10, max: 500 },
        Condition::MeetingRequest,
        Condition::Category(vec!["Red".into()]),
        Condition::Flagged,
        Condition::SentOnlyToMe,
        Condition::ToMe,
        Condition::CcMe,
        Condition::ToOrCcMe,
        Condition::NotToMe,
    ] {
        assert!(rule.conditions.contains(&c), "{c:?} in {:?}", rule.conditions);
    }
    // `false` is a part the rule doesn't have.
    assert!(!rule.conditions.contains(&Condition::AutomaticReply));
    assert_eq!(rule.conditions.len(), 15);
    assert_eq!(rule.exceptions.len(), 2);
    assert_eq!(rule.copy_to, "IMM-copies");
    assert_eq!(rule.actions, vec![Action::MarkRead]);
    // Written back, it says the same.
    let body = rule_body(&rule).unwrap();
    let again = rule_of(&json!({"id": "A4", "displayName": body["displayName"], "isEnabled": true, "conditions": body["conditions"],
        "exceptions": body["exceptions"], "actions": body["actions"]}))
    .unwrap();
    assert_eq!((again.conditions.len(), again.exceptions.len()), (15, 2));
    assert!(rule.conditions.iter().all(|c| again.conditions.contains(c)) && rule.exceptions.iter().all(|c| again.exceptions.contains(c)));
    assert_eq!((again.copy_to.as_str(), again.actions.as_slice()), ("IMM-copies", [Action::MarkRead].as_slice()));
    assert_eq!(body["conditions"]["fromAddresses"], json!([{"emailAddress": {"address": "ann@example.org"}}]));
    // Graph's sizes are in kilobytes (`sizeRange`), as a rule's are: 10 KB to 500 KB goes up as
    // 10 and 500, never as bytes.
    assert_eq!(body["conditions"]["withinSizeRange"], json!({"minimumSize": 10, "maximumSize": 500}));
}

#[test]
fn forwarding_is_read_whole_and_written_back_by_addresses_alone() {
    let people = |a: &[&str]| json!(a.iter().map(|a| json!({"emailAddress": {"name": "Somebody", "address": a}})).collect::<Vec<_>>());
    let rule = rule_of(&json!({
        "id": "F1", "displayName": "On to the team", "sequence": 1, "isEnabled": true,
        "conditions": {"senderContains": ["accounts"]},
        "actions": {"forwardTo": people(&["ann@example.org", "bob@example.org"]), "forwardAsAttachmentTo": people(&["cat@example.org"]),
                    "redirectTo": people(&["dan@example.org"]), "stopProcessingRules": true}
    }))
    .unwrap();
    assert!(rule.unknown.is_empty(), "{:?}", rule.unknown);
    for a in [
        Action::ForwardTo(vec!["ann@example.org".into(), "bob@example.org".into()]),
        Action::ForwardAsAttachmentTo(vec!["cat@example.org".into()]),
        Action::RedirectTo(vec!["dan@example.org".into()]),
    ] {
        assert!(rule.actions.contains(&a), "{a:?} in {:?}", rule.actions);
    }
    // Written back by their addresses alone, as the server keeps them.
    let body = rule_body(&rule).unwrap();
    assert_eq!(
        body["actions"]["forwardTo"],
        json!([{"emailAddress": {"address": "ann@example.org"}}, {"emailAddress": {"address": "bob@example.org"}}])
    );
    assert_eq!(body["actions"]["forwardAsAttachmentTo"], json!([{"emailAddress": {"address": "cat@example.org"}}]));
    assert_eq!(body["actions"]["redirectTo"], json!([{"emailAddress": {"address": "dan@example.org"}}]));
    // Somebody without a whole address, or more people than a rule forwards to, isn't known here:
    // such a rule is only switched on or off, moved or deleted.
    for actions in [
        json!({"forwardTo": [{"emailAddress": {"name": "No address"}}]}),
        json!({"redirectTo": people(&["not an address"])}),
        json!({"forwardTo": people(&["a1@example.org", "a2@example.org", "a3@example.org", "a4@example.org", "a5@example.org", "a6@example.org"]),
               "redirectTo": people(&["b1@example.org", "b2@example.org", "b3@example.org", "b4@example.org", "b5@example.org"])}),
    ] {
        let odd = rule_of(&json!({"id": "F2", "displayName": "Odd", "conditions": {"subjectContains": ["x"]}, "actions": actions})).unwrap();
        assert!(!odd.unknown.is_empty(), "{actions}");
    }
    // One way twice can't go up: the server keeps one list of people each way.
    let mut twice = rule.clone();
    twice.actions.push(Action::ForwardTo(vec!["eve@example.org".into()]));
    assert!(rule_body(&twice).is_err());
}

#[test]
fn two_exceptions_of_a_kind_go_up_as_one_and_two_conditions_never_do() {
    let mut rule = RemoteRule {
        name: "Bills".into(),
        conditions: vec![Condition::SubjectContains(vec!["invoice".into()])],
        exceptions: vec![
            Condition::FromContains(vec!["boss".into()]),
            Condition::FromContains(vec!["hr".into(), "boss".into()]),
            Condition::ToMe,
            Condition::ToMe,
        ],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    let body = rule_body(&rule).unwrap();
    assert_eq!(body["exceptions"], json!({"senderContains": ["boss", "hr"], "sentToMe": true}));
    rule.exceptions = vec![Condition::Importance(Importance::High), Condition::Importance(Importance::Low)];
    assert!(rule_body(&rule).is_err(), "one importance an exception looks at");
    rule.exceptions.clear();
    rule.conditions.push(Condition::SubjectContains(vec!["bill".into()]));
    assert!(rule_body(&rule).is_err(), "both must hold, which the server's one list can't say");
    // What the server's rules can't do isn't sent.
    for (conditions, actions) in [
        (vec![Condition::ReceivedBetween { from: None, to: mailcraft_model::time::Date::new(2026, 12, 31) }], vec![Action::MarkRead]),
        (vec![Condition::ToMe], vec![Action::FlagFor(mailcraft_model::FollowUp::Today)]),
        (vec![Condition::ToMe], vec![Action::ClearFlag]),
        (vec![Condition::ToMe], vec![Action::ClearCategories]),
        (vec![Condition::ToMe], vec![Action::DeleteForGood]),
    ] {
        let rule = RemoteRule { name: "x".into(), conditions, actions, ..Default::default() };
        assert!(rule_body(&rule).is_err(), "{rule:?}");
    }
}

#[test]
fn rules_are_read_and_their_folders_named_as_the_mail_names_them() {
    let s = state(vec![
        json!({"id": "A", "displayName": "One", "sequence": 1, "isEnabled": true,
               "conditions": {"subjectContains": ["x"]}, "actions": {"moveToFolder": "OLD-news"}}),
        json!({"id": "B", "displayName": "Two", "sequence": 2, "isEnabled": false,
               "conditions": {"subjectContains": ["y"]}, "actions": {"moveToFolder": "IMM-done"}}),
        json!({"id": "C", "displayName": "Three", "sequence": 3, "isEnabled": true,
               "conditions": {"subjectContains": ["z"]}, "actions": {"moveToFolder": "OLD-gone"}}),
        json!({"displayName": "Not a rule"}),
    ]);
    s.lock().unwrap().folders = vec![("OLD-news".into(), "IMM-news".into())];
    let server = serve(&s);
    let got = rules_at(&server.url(""), TOKEN, &["IMM-done".to_string()]).unwrap();
    // One entry was no rule: what is missing is not taken as deleted.
    assert!(!got.complete);
    assert_eq!(got.rules.len(), 3);
    assert_eq!(got.rules[0].move_to, "IMM-news");
    assert_eq!(got.rules[1].move_to, "IMM-done");
    assert!(!got.rules[1].enabled);
    // A folder that can't be found is left as the rule names it.
    assert_eq!(got.rules[2].move_to, "OLD-gone");
    let asked = s.lock().unwrap().asked.clone();
    assert!(asked.iter().all(|a| a.contains("IdType=\"ImmutableId\"")), "{asked:?}");
    // The folder the mail knows is not looked up.
    assert!(!asked.iter().any(|a| a.contains("IMM-done")), "{asked:?}");
}

#[test]
fn a_rule_whose_folder_gets_no_answer_waits_for_the_next_read() {
    let s = state(vec![
        json!({"id": "A", "displayName": "One", "sequence": 1, "isEnabled": true,
               "conditions": {"subjectContains": ["x"]}, "actions": {"moveToFolder": "OLD-news"}}),
        json!({"id": "B", "displayName": "Two", "sequence": 2, "isEnabled": true,
               "conditions": {"subjectContains": ["y"]}, "actions": {"markAsRead": true}}),
    ]);
    let server = serve(&s);
    // Not to be had now: nothing can be told of that rule, not even that it isn't ours.
    for status in [429, 500, 503] {
        s.lock().unwrap().folder_status = Some(status);
        let got = rules_at(&server.url(""), TOKEN, &[]).unwrap();
        assert!(!got.complete, "{status}");
        assert_eq!(got.rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["B"], "{status}");
    }
    // A folder the server won't give is an answer: the rule moves mail to one that isn't known.
    for status in [None, Some(400), Some(403)] {
        s.lock().unwrap().folder_status = status;
        let got = rules_at(&server.url(""), TOKEN, &[]).unwrap();
        assert!(got.complete, "{status:?}");
        assert_eq!(got.rules[0].move_to, "OLD-news", "{status:?}");
    }
}

#[test]
fn a_rule_copying_to_a_folder_that_gets_no_answer_waits_too() {
    let s = state(vec![json!({"id": "A", "displayName": "One", "sequence": 1, "isEnabled": true,
           "conditions": {"subjectContains": ["x"]}, "actions": {"copyToFolder": "OLD-copies"}})]);
    let server = serve(&s);
    s.lock().unwrap().folder_status = Some(503);
    let got = rules_at(&server.url(""), TOKEN, &[]).unwrap();
    assert!(!got.complete);
    assert!(got.rules.is_empty());
}

#[test]
fn a_rule_made_here_is_sent_whole_and_comes_back_with_its_id() {
    let s = state(Vec::new());
    let server = serve(&s);
    let op = RuleOp { kind: RuleOpKind::Create, rule: 7, body: Some(news_rule()), ..Default::default() };
    let done = push_rules_at(&server.url(""), TOKEN, &upload(vec![op]));
    assert!(!done.token_refused);
    let outcome = &done.outcomes[0];
    assert_eq!(outcome.result, RuleResult::Done, "{outcome:?}");
    assert_eq!(outcome.id, "R1");
    assert_eq!((outcome.enabled, outcome.sequence), (Some(true), Some(3)));
    let made = s.lock().unwrap().rules[0].clone();
    assert_eq!(made["displayName"], "News");
    assert_eq!(made["sequence"], 3);
    assert_eq!(made["conditions"], json!({"senderContains": ["news@example.org"], "hasAttachments": true}));
    assert_eq!(made["actions"], json!({"assignCategories": ["Blue"], "markAsRead": true, "moveToFolder": "IMM-news", "stopProcessingRules": true}));
    // And it reads back as it was sent.
    let back = rule_of(&made).unwrap();
    assert!(news_rule().conditions.iter().all(|c| back.conditions.contains(c)));
    assert_eq!(back.conditions.len(), 2);
    assert_eq!(back.move_to, "IMM-news");
}

#[test]
fn a_rule_the_server_cannot_keep_is_not_sent() {
    let mut flagged = news_rule();
    flagged.actions.push(Action::Flag);
    assert!(rule_body(&flagged).is_err());
    let mut twice = news_rule();
    twice.conditions.push(Condition::FromContains(vec!["other@example.org".into()]));
    assert!(rule_body(&twice).is_err());
    let mut none = news_rule();
    none.conditions.clear();
    assert!(rule_body(&none).is_err());
    let mut idle = news_rule();
    (idle.actions, idle.move_to) = (Vec::new(), String::new());
    assert!(rule_body(&idle).is_err());
    let mut two = news_rule();
    two.actions.extend([Action::SetImportance(Importance::High), Action::SetImportance(Importance::Low)]);
    assert!(rule_body(&two).is_err());

    let s = state(Vec::new());
    let server = serve(&s);
    let op = RuleOp { kind: RuleOpKind::Create, rule: 7, body: Some(flagged), ..Default::default() };
    let done = push_rules_at(&server.url(""), TOKEN, &upload(vec![op]));
    assert_eq!(done.outcomes[0].result, RuleResult::Refused);
    assert!(s.lock().unwrap().asked.is_empty());
}

#[test]
fn switching_moving_and_deleting_send_only_that() {
    let s = state(vec![
        json!({"id": "A", "displayName": "One", "sequence": 1, "isEnabled": true, "conditions": {"subjectContains": ["x"]}, "actions": {}}),
        json!({"id": "B", "displayName": "Two", "sequence": 2, "isEnabled": true, "conditions": {"subjectContains": ["y"]}, "actions": {}}),
    ]);
    let server = serve(&s);
    let ops = vec![
        RuleOp { kind: RuleOpKind::Update, rule: 1, id: "A".into(), enabled: Some(false), sequence: Some(2), ..Default::default() },
        RuleOp { kind: RuleOpKind::Delete, id: "B".into(), ..Default::default() },
        RuleOp { kind: RuleOpKind::Delete, id: "Z".into(), ..Default::default() },
        RuleOp { kind: RuleOpKind::Update, rule: 3, id: "A".into(), ..Default::default() },
    ];
    let done = push_rules_at(&server.url(""), TOKEN, &upload(ops));
    let results: Vec<RuleResult> = done.outcomes.iter().map(|o| o.result).collect();
    assert_eq!(results, vec![RuleResult::Done, RuleResult::Done, RuleResult::Gone, RuleResult::Refused]);
    assert_eq!((done.outcomes[0].enabled, done.outcomes[0].sequence), (Some(false), Some(2)));
    let server_rules = s.lock().unwrap().rules.clone();
    assert_eq!(server_rules.len(), 1);
    assert_eq!(server_rules[0]["isEnabled"], false);
    assert_eq!(server_rules[0]["sequence"], 2);
    // The words it looks for are not sent again.
    assert_eq!(server_rules[0]["conditions"], json!({"subjectContains": ["x"]}));
}

#[test]
fn a_sign_in_without_leave_to_change_rules_stops_the_upload() {
    let s = state(Vec::new());
    s.lock().unwrap().write_status = Some(403);
    let server = serve(&s);
    let ops = vec![
        RuleOp { kind: RuleOpKind::Create, rule: 7, body: Some(news_rule()), ..Default::default() },
        RuleOp { kind: RuleOpKind::Delete, id: "B".into(), ..Default::default() },
    ];
    let done = push_rules_at(&server.url(""), TOKEN, &upload(ops));
    assert_eq!(done.outcomes.len(), 1);
    assert_eq!(done.outcomes[0].result, RuleResult::Denied);

    for (status, result, more) in [(503, RuleResult::Failed, false), (400, RuleResult::Refused, true), (401, RuleResult::Done, false)] {
        let s = state(Vec::new());
        s.lock().unwrap().write_status = Some(status);
        let server = serve(&s);
        let ops = vec![
            RuleOp { kind: RuleOpKind::Delete, id: "A".into(), ..Default::default() },
            RuleOp { kind: RuleOpKind::Delete, id: "B".into(), ..Default::default() },
        ];
        let done = push_rules_at(&server.url(""), TOKEN, &upload(ops));
        if status == 401 {
            assert!(done.token_refused);
            assert!(done.outcomes.is_empty());
            continue;
        }
        assert_eq!(done.outcomes[0].result, result, "{status}");
        assert_eq!(done.outcomes.len(), if more { 2 } else { 1 }, "{status}");
        assert!(!done.outcomes[0].error.contains(TOKEN));
    }
}

#[test]
fn a_rule_the_server_wont_move_is_refused_for_itself_and_what_follows_waits() {
    let rules = || {
        vec![
            json!({"id": "P", "displayName": "Company policy", "sequence": 2, "isEnabled": true, "isReadOnly": true}),
            json!({"id": "N", "displayName": "Newsletters", "sequence": 3, "isEnabled": true}),
        ]
    };
    let ops = || {
        vec![
            RuleOp { kind: RuleOpKind::Update, rule: 1, id: "P".into(), sequence: Some(3), locked: true, ..Default::default() },
            RuleOp { kind: RuleOpKind::Update, rule: 2, id: "N".into(), sequence: Some(2), ..Default::default() },
        ]
    };
    // Refused (a 403 too: never read as the account's), or a yes that leaves it where it was.
    for status in [Some(403), Some(400), None] {
        let s = state(rules());
        s.lock().unwrap().read_only_status = status;
        let server = serve(&s);
        let done = push_rules_at(&server.url(""), TOKEN, &upload(ops()));
        assert_eq!(done.outcomes.len(), 1, "{status:?}: nothing after it goes");
        assert_eq!(done.outcomes[0].result, RuleResult::Refused, "{status:?}");
        assert!(!done.outcomes[0].error.is_empty() && !done.outcomes[0].error.contains(TOKEN), "{status:?}");
        assert_eq!(s.lock().unwrap().rules[1]["sequence"], 3, "{status:?}");
    }
    // A server that moves it: the rest goes on.
    let s = state(rules());
    s.lock().unwrap().rules[0]["isReadOnly"] = json!(false);
    let server = serve(&s);
    let done = push_rules_at(&server.url(""), TOKEN, &upload(ops()));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [RuleResult::Done, RuleResult::Done]);
    let server_rules = s.lock().unwrap().rules.clone();
    assert_eq!((server_rules[0]["sequence"].clone(), server_rules[1]["sequence"].clone()), (json!(3), json!(2)));
    // Another rule's 403 is still the account's.
    let s = state(rules());
    s.lock().unwrap().write_status = Some(403);
    let server = serve(&s);
    let done = push_rules_at(&server.url(""), TOKEN, &upload(ops().split_off(1)));
    assert_eq!(done.outcomes[0].result, RuleResult::Denied);
}

#[test]
fn a_read_that_is_refused_says_so() {
    let server = FakeHttp::start(|_, _| raw("403 Forbidden"));
    let e = rules_at(&server.url(""), TOKEN, &[]).unwrap_err();
    assert!(matches!(e, crate::NetError::Forbidden(_)), "{e:?}");
    assert!(e.to_string().contains("rules"));
    assert!(rules_at(&server.url(""), "", &[]).is_err());
}

#[test]
fn a_read_that_skipped_folder_look_ups_past_the_most_isnt_whole() {
    let rules = |n: usize| {
        (0..n)
            .map(|i| {
                json!({"id": format!("R{i}"), "displayName": format!("Rule {i}"), "sequence": i + 1, "isEnabled": true,
                       "conditions": {"subjectContains": ["x"]}, "actions": {"moveToFolder": format!("OLD-{i}")}})
            })
            .collect::<Vec<_>>()
    };
    let s = state(rules(super::MAX_FOLDER_LOOKUPS));
    let server = serve(&s);
    assert!(rules_at(&server.url(""), TOKEN, &[]).unwrap().complete);
    s.lock().unwrap().rules = rules(super::MAX_FOLDER_LOOKUPS + 1);
    assert!(!rules_at(&server.url(""), TOKEN, &[]).unwrap().complete);
}

#[test]
fn a_rule_changed_here_is_sent_whole_in_place_of_what_the_server_had() {
    let s = state(vec![json!({"id": "A", "displayName": "Old", "sequence": 1, "isEnabled": true,
           "conditions": {"subjectContains": ["x"], "importance": "high"},
           "exceptions": {"senderContains": ["boss@example.org"]},
           "actions": {"delete": true}})]);
    let server = serve(&s);
    let op = RuleOp { kind: RuleOpKind::Update, rule: 1, id: "A".into(), body: Some(news_rule()), edit: 2, ..Default::default() };
    let done = push_rules_at(&server.url(""), TOKEN, &upload(vec![op]));
    let outcome = &done.outcomes[0];
    assert_eq!((outcome.result, outcome.edit), (RuleResult::Done, 2), "{outcome:?}");
    let rule = s.lock().unwrap().rules[0].clone();
    assert_eq!(rule["displayName"], "News");
    assert_eq!(rule["conditions"], json!({"senderContains": ["news@example.org"], "hasAttachments": true}));
    assert_eq!(rule["actions"], json!({"assignCategories": ["Blue"], "markAsRead": true, "moveToFolder": "IMM-news", "stopProcessingRules": true}));
    // An exception it no longer has is taken away; where it stands and whether it is on are not
    // what the change was about.
    assert_eq!(rule["exceptions"], json!({}));
    assert_eq!((rule["sequence"].clone(), rule["isEnabled"].clone()), (json!(1), json!(true)));

    // One the server couldn't keep is not sent at all.
    let mut flagged = news_rule();
    flagged.actions.push(Action::Flag);
    let s = state(vec![json!({"id": "A", "displayName": "Old", "sequence": 1, "isEnabled": true, "conditions": {}, "actions": {}})]);
    let server = serve(&s);
    let op = RuleOp { kind: RuleOpKind::Update, rule: 1, id: "A".into(), body: Some(flagged), edit: 1, enabled: Some(false), ..Default::default() };
    let done = push_rules_at(&server.url(""), TOKEN, &upload(vec![op]));
    assert_eq!((done.outcomes[0].result, done.outcomes[0].edit), (RuleResult::Refused, 1));
    assert!(s.lock().unwrap().asked.is_empty());
}
