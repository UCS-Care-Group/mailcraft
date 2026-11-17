// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A /query filter that is not one RFC 8620 §5.5 and RFC 8621 allow is an error when read, never a filter
//! that matches more than was asked (an empty condition matches everything).

use mailcraft_jmap_types::{EmailQueryRequest, Filter, MailboxQueryRequest, QueryRequest};
use serde_json::{Value, json};

fn email_query(filter: Value) -> Result<EmailQueryRequest, serde_json::Error> {
    serde_json::from_value(json!({"accountId": "a", "filter": filter}))
}

#[test]
fn a_good_nested_filter_reads() {
    let query = email_query(json!({
        "operator": "AND",
        "conditions": [{"inMailbox": "m"}, {"operator": "NOT", "conditions": [{"hasKeyword": "$seen"}]}]
    }))
    .unwrap();
    assert!(matches!(query.query.filter, Some(Filter::Operator(_))));
}

#[test]
fn an_unknown_operator_is_an_error() {
    assert!(email_query(json!({"operator": "XOR", "conditions": [{"inMailbox": "m"}]})).is_err());
}

#[test]
fn an_operator_without_conditions_is_an_error() {
    assert!(email_query(json!({"operator": "AND"})).is_err());
}

#[test]
fn a_bad_leaf_deep_in_an_and_is_an_error() {
    let filter = json!({
        "operator": "AND",
        "conditions": [{"inMailbox": "m"}, {"operator": "OR", "conditions": [{"hasKeyword": "$seen"}, {"inMailbx": "typo"}]}]
    });
    assert!(email_query(filter).is_err());
}

#[test]
fn a_leaf_of_the_wrong_type_is_an_error() {
    assert!(email_query(json!({"inMailbox": 5})).is_err());
}

#[test]
fn an_unknown_key_in_a_condition_is_an_error() {
    assert!(email_query(json!({"inMailbox": "m", "colour": "red"})).is_err());
    let mailbox: Result<MailboxQueryRequest, _> = serde_json::from_value(json!({"accountId": "a", "filter": {"role": "inbox", "colour": "red"}}));
    assert!(mailbox.is_err());
}

/// With an open condition type (`Value`), an object naming an operator is still read as an operator only.
#[test]
fn an_object_with_an_operator_never_falls_back_to_a_condition() {
    let bad: Result<QueryRequest<Value>, _> = serde_json::from_value(json!({"accountId": "a", "filter": {"operator": "XOR", "conditions": []}}));
    assert!(bad.is_err());
    let good: QueryRequest<Value> = serde_json::from_value(json!({"accountId": "a", "filter": {"hasKeyword": "x"}})).unwrap();
    assert_eq!(good.filter, Some(Filter::Condition(json!({"hasKeyword": "x"}))));
}
