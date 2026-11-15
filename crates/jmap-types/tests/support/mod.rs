// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the RFC example tests share: reading a fixture into a type and writing it back.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Drops every object member whose value is null, at any depth, but keeps nulls that are the values of
/// a map whose entries the test checks itself. RFC 8620 §3.5: null is the default of every argument that
/// may be null, and an omitted argument means the default, so the types leave a `None` out where the
/// RFC example writes `null`.
pub fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, without_nulls(v))).collect()),
        Value::Array(list) => Value::Array(list.into_iter().map(without_nulls).collect()),
        other => other,
    }
}

/// Reads `json` as a `T`, writes it back, and checks that nothing was lost or added (nulls aside, see
/// [`without_nulls`]). Returns the value read, for the test's own checks.
pub fn round_trip<T: DeserializeOwned + Serialize>(json: &str) -> T {
    let fixture: Value = serde_json::from_str(json).expect("the fixture is JSON");
    let typed: T = serde_json::from_value(fixture.clone()).expect("the fixture reads as the type");
    let written = serde_json::to_value(&typed).expect("the type writes as JSON");
    assert_eq!(without_nulls(written), without_nulls(fixture), "written back differs from the fixture");
    typed
}
