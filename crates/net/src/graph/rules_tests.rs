// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Microsoft account's server (#566), against a small stand-in on loopback that
//! keeps the Inbox's `messageRules` and answers as the documented service does. Nothing here
//! leaves this machine.

use std::sync::{Arc, Mutex};

use mailcraft_model::remote_rules::{RemoteRule, RuleOp, RuleOpKind, RuleResult, RulesUpload};
use mailcraft_model::{Action, Condition, Importance};
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
        conditions: vec![Condition::FromContains("news@example.org".into()), Condition::HasAttachment],
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
    assert!(rule.conditions.contains(&Condition::FromContains("shop@example.org".into())));
    assert!(rule.conditions.contains(&Condition::SubjectContains("Order".into())));
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
        "id": "A2", "displayName": "Forward", "sequence": 1, "isEnabled": true,
        "conditions": {"senderContains": ["a@example.org", "b@example.org"], "sentToMe": true},
        "exceptions": {"subjectContains": ["private"]},
        "actions": {"forwardTo": [{"emailAddress": {"address": "c@example.org"}}], "delete": true}
    }))
    .unwrap();
    for part in ["senderContains", "sentToMe", "exceptions", "forwardTo"] {
        assert!(rule.unknown.iter().any(|u| u == part), "{part} in {:?}", rule.unknown);
    }
    assert_eq!(rule.actions, vec![Action::Delete]);
    // A rule without conditions acts on every message there: not one that is run here.
    let every = rule_of(&json!({"id": "A3", "displayName": "All", "conditions": {}, "actions": {"markAsRead": true}})).unwrap();
    assert_eq!(every.unknown, vec!["conditions".to_string()]);
    // No id: no rule.
    assert!(rule_of(&json!({"displayName": "x"})).is_none());
    assert!(rule_of(&json!({"id": "has space"})).is_none());
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
    twice.conditions.push(Condition::FromContains("other@example.org".into()));
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
fn a_read_that_is_refused_says_so() {
    let server = FakeHttp::start(|_, _| raw("403 Forbidden"));
    let e = rules_at(&server.url(""), TOKEN, &[]).unwrap_err();
    assert!(matches!(e, crate::NetError::Forbidden(_)), "{e:?}");
    assert!(e.to_string().contains("rules"));
    assert!(rules_at(&server.url(""), "", &[]).is_err());
}
