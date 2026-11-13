// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Request and Response objects (RFC 8620 §3.2 to §3.4) and result references (§3.7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Id;

/// A method call or a method response (RFC 8620 §3.2): the name, the arguments and the method call id,
/// written as a JSON array of the three.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Invocation(pub String, pub Map<String, Value>, pub String);

/// A Request object (RFC 8620 §3.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The capabilities the request uses.
    pub using: Vec<String>,
    /// The method calls, in order.
    pub method_calls: Vec<Invocation>,
    /// Creation ids already known to the client, to their server ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
}

/// A Response object (RFC 8620 §3.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// The method responses, in the order the server processed the calls. A method-level error is the
    /// response named `error`, whose arguments read as a [`MethodError`](crate::MethodError).
    pub method_responses: Vec<Invocation>,
    /// The creation ids and the ids the server gave them, when the request named `createdIds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<BTreeMap<Id, Id>>,
    /// The session's current `state`.
    pub session_state: String,
}

/// A reference to the result of an earlier call of the same request (RFC 8620 §3.7), the value of an
/// argument whose name is the referring argument's with `#` before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultReference {
    /// The method call id of the earlier call.
    pub result_of: String,
    /// The response name required.
    pub name: String,
    /// A JSON Pointer (RFC 6901, with `*` for every item of an array) into that response's arguments.
    pub path: String,
}
