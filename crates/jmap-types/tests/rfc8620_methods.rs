// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The standard methods' examples of RFC 8620 §5.7 (a made-up Todo type), each call's arguments read into
//! the method's types and written back. The Todo objects stay JSON (`Value`), being no type of JMAP's.
//!
//! Edits to the RFC's text: the `...` standing for more entries removed. Each fixture is one invocation's
//! arguments.

mod support;

use mailcraft_jmap_types::{
    ChangesRequest, ChangesResponse, Filter, FilterOperator, GetRequest, GetResponse, Operator, QueryChangesRequest, QueryChangesResponse,
    QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use serde_json::{Value, json};
use support::round_trip;

#[test]
fn query_with_an_or_filter() {
    let query: QueryRequest<Value> = round_trip(
        r#"{
          "accountId": "x",
          "filter": { "operator": "OR", "conditions": [ { "hasKeyword": "music" }, { "hasKeyword": "video" } ] },
          "sort": [{ "property": "title" }],
          "position": 0,
          "limit": 10
        }"#,
    );
    let Some(Filter::Operator(FilterOperator { operator, conditions })) = query.filter else { panic!("an operator") };
    assert_eq!(operator, Operator::Or);
    assert_eq!(conditions, [Filter::Condition(json!({"hasKeyword": "music"})), Filter::Condition(json!({"hasKeyword": "video"}))]);
    let sort = query.sort.unwrap();
    assert_eq!(sort[0].property, "title");
    assert_eq!(sort[0].is_ascending, None);
    assert_eq!((query.position, query.limit), (Some(0), Some(10)));
}

#[test]
fn query_response() {
    let found: QueryResponse = round_trip(
        r#"{
          "accountId": "x",
          "queryState": "y13213",
          "canCalculateChanges": true,
          "position": 0,
          "ids": [ "a", "b", "c", "d", "e", "f", "g", "h", "i", "j" ]
        }"#,
    );
    assert_eq!(found.query_state, "y13213");
    assert!(found.can_calculate_changes);
    assert_eq!(found.ids.len(), 10);
    assert_eq!(found.total, None);
}

/// §5.7 prints the response without `notFound`, which §5.1 makes required; it is put in here.
#[test]
fn get_response() {
    let got: GetResponse<Value> = round_trip(
        r#"{
          "accountId": "x",
          "state": "10324",
          "list": [{
            "id": "a",
            "title": "Practise Piano",
            "keywords": { "music": true, "beethoven": true, "mozart": true, "liszt": true, "rachmaninov": true },
            "neuralNetworkTimeEstimation": 3600
          }, {
            "id": "b",
            "title": "Watch Daft Punk music video",
            "keywords": { "music": true, "video": true, "trance": true },
            "neuralNetworkTimeEstimation": 18000
          }],
          "notFound": []
        }"#,
    );
    assert_eq!(got.state, "10324");
    assert_eq!(got.list[1]["id"], "b");
}

#[test]
fn get_request_with_ids_and_properties() {
    let get: GetRequest = round_trip(r#"{"accountId": "x", "ids": ["a"], "properties": ["title"]}"#);
    assert_eq!(get.ids.as_deref(), Some(&["a".to_owned()][..]));
    let all: GetRequest = round_trip(r#"{"accountId": "x"}"#);
    assert_eq!(all.ids, None);
}

#[test]
fn set_with_a_whole_object_and_with_a_minimal_patch() {
    let whole: SetRequest<Value> = round_trip(
        r#"{
          "accountId": "x",
          "ifInState": "10324",
          "update": {
            "a": {
              "id": "a",
              "title": "Practise Piano",
              "keywords": { "music": true, "beethoven": true, "chopin": true, "liszt": true, "rachmaninov": true },
              "neuralNetworkTimeEstimation": 360
            }
          }
        }"#,
    );
    assert_eq!(whole.if_in_state.as_deref(), Some("10324"));
    let patch: SetRequest<Value> =
        round_trip(r#"{ "accountId": "x", "ifInState": "10324", "update": { "a": { "keywords/chopin": true, "keywords/mozart": null } } }"#);
    // A null in a patch resets the property: it is kept.
    let a = &patch.update.unwrap()["a"];
    assert_eq!(a.get("keywords/mozart"), Some(&Value::Null));
    assert_eq!(serde_json::to_value(a).unwrap(), json!({"keywords/chopin": true, "keywords/mozart": null}));
}

#[test]
fn set_create_and_its_response() {
    let create: SetRequest<Value> = round_trip(
        r##"{
          "accountId": "x",
          "create": { "k15": { "title": "Warm up with scales" } },
          "update": { "a": { "subTodoIds": [ "#k15" ] } }
        }"##,
    );
    assert_eq!(create.create.unwrap()["k15"]["title"], "Warm up with scales");
    let done: SetResponse<Value> =
        round_trip(r#"{ "accountId": "x", "oldState": "10324", "newState": "10329", "updated": { "a": { "neuralNetworkTimeEstimation": 5400 } } }"#);
    assert_eq!(done.new_state, "10329");
    assert_eq!(done.updated.unwrap()["a"], Some(json!({"neuralNetworkTimeEstimation": 5400})));
}

#[test]
fn changes_and_query_changes() {
    let changes: ChangesRequest = round_trip(r#"{ "accountId": "x", "sinceState": "10324", "maxChanges": 50 }"#);
    assert_eq!(changes.max_changes, Some(50));
    let since: QueryChangesRequest<Value> = round_trip(
        r#"{
          "accountId": "x",
          "filter": { "operator": "OR", "conditions": [ { "hasKeyword": "music" }, { "hasKeyword": "video" } ] },
          "sort": [{ "property": "title" }],
          "sinceQueryState": "y13213",
          "maxChanges": 50
        }"#,
    );
    assert_eq!(since.since_query_state, "y13213");
    let changed: ChangesResponse = round_trip(
        r#"{
          "accountId": "x",
          "oldState": "10324",
          "newState": "871903",
          "hasMoreChanges": false,
          "created": [],
          "updated": [],
          "destroyed": ["b"]
        }"#,
    );
    assert_eq!(changed.destroyed, ["b"]);
    assert!(!changed.has_more_changes);
}

/// §5.7 prints `"added": null`, which §5.6 types `AddedItem[]`: read as none added.
#[test]
fn query_changes_response_with_added_null() {
    let changed: QueryChangesResponse = serde_json::from_value(json!({
      "accountId": "x",
      "oldQueryState": "y13213",
      "newQueryState": "y13218",
      "removed": ["b"],
      "added": null
    }))
    .unwrap();
    assert_eq!(changed.removed, ["b"]);
    assert!(changed.added.is_empty());
    assert_eq!(serde_json::to_value(&changed).unwrap()["added"], json!([]));
}

/// The implicit Todo/set of a /copy with `onSuccessDestroyOriginal`, which destroys the original.
#[test]
fn set_response_of_a_copys_destroy() {
    let destroyed: SetResponse<Value> = round_trip(r#"{ "accountId": "x", "oldState": "871903", "newState": "871909", "destroyed": [ "a" ] }"#);
    assert_eq!(destroyed.destroyed.unwrap(), ["a"]);
}
