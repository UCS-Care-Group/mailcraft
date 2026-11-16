// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The arguments and responses of the standard methods (RFC 8620 §5): /get, /changes, /set,
//! /query and /queryChanges, for any type `T` of object and any filter condition `C`.

use std::collections::BTreeMap;

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::{Id, SetError};

/// A PatchObject (RFC 8620 §5.3): JSON Pointer paths to new values, a null resetting a property.
pub type PatchObject = Map<String, Value>;

/// Reads a null as an empty list, for a list a server may write as null.
fn null_as_empty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// The arguments of `Foo/get` (RFC 8620 §5.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRequest {
    /// The account.
    pub account_id: Id,
    /// The ids to read (none: every object of the type, where the server allows it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    /// The properties to read (none: all of them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
}

/// The response of `Foo/get` (RFC 8620 §5.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResponse<T> {
    /// The account.
    pub account_id: Id,
    /// The state of the type's objects.
    pub state: String,
    /// The objects found.
    pub list: Vec<T>,
    /// The ids asked for and not found.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub not_found: Vec<Id>,
}

/// The arguments of `Foo/changes` (RFC 8620 §5.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesRequest {
    /// The account.
    pub account_id: Id,
    /// The state the client has.
    pub since_state: String,
    /// The most ids to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
}

/// The response of `Foo/changes` (RFC 8620 §5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResponse {
    /// The account.
    pub account_id: Id,
    /// The `sinceState` of the call.
    pub old_state: String,
    /// The state the changes bring the client to.
    pub new_state: String,
    /// Whether there are more changes after `newState`.
    pub has_more_changes: bool,
    /// The ids created.
    pub created: Vec<Id>,
    /// The ids updated.
    pub updated: Vec<Id>,
    /// The ids destroyed.
    pub destroyed: Vec<Id>,
}

/// The arguments of `Foo/set` (RFC 8620 §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRequest<T> {
    /// The account.
    pub account_id: Id,
    /// Make no change unless the state is this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<String>,
    /// Objects to create, by creation id.
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub create: Option<BTreeMap<Id, T>>,
    /// Patches to apply, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<BTreeMap<Id, PatchObject>>,
    /// Ids to destroy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<Vec<Id>>,
}

/// `None`, for a default that must not ask `T: Default`.
fn none<T>() -> Option<T> {
    None
}

/// The response of `Foo/set` (RFC 8620 §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResponse<T> {
    /// The account.
    pub account_id: Id,
    /// The state before the call, when the server knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<String>,
    /// The state after the call.
    pub new_state: String,
    /// The objects created, by creation id: the properties the server set or changed.
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<Id, T>>,
    /// The ids updated, each with the properties the server changed beyond the patch (or null).
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub updated: Option<BTreeMap<Id, Option<T>>>,
    /// The ids destroyed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroyed: Option<Vec<Id>>,
    /// Why each object not created was not, by creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<Id, SetError>>,
    /// Why each object not updated was not, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_updated: Option<BTreeMap<Id, SetError>>,
    /// Why each object not destroyed was not, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_destroyed: Option<BTreeMap<Id, SetError>>,
}

/// How a FilterOperator joins its conditions (RFC 8620 §5.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Operator {
    /// Every condition must match.
    #[serde(rename = "AND")]
    And,
    /// At least one condition must match.
    #[serde(rename = "OR")]
    Or,
    /// No condition may match.
    #[serde(rename = "NOT")]
    Not,
}

/// A FilterOperator (RFC 8620 §5.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "C: DeserializeOwned"))]
pub struct FilterOperator<C> {
    /// How the conditions join.
    pub operator: Operator,
    /// The conditions, each a FilterOperator or a FilterCondition.
    pub conditions: Vec<Filter<C>>,
}

/// A /query filter (RFC 8620 §5.5): a FilterOperator or a FilterCondition `C` of the type queried.
///
/// An object with an `operator` member is read as a FilterOperator and nothing else, so that a filter the
/// RFC does not allow (an unknown operator, a bad condition anywhere in it) is an error and never a
/// condition that matches more than was asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Filter<C> {
    /// Conditions joined.
    Operator(FilterOperator<C>),
    /// One condition.
    Condition(C),
}

impl<'de, C: DeserializeOwned> Deserialize<'de> for Filter<C> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value.get("operator").is_some() {
            FilterOperator::deserialize(value).map(Filter::Operator).map_err(D::Error::custom)
        } else {
            C::deserialize(value).map(Filter::Condition).map_err(D::Error::custom)
        }
    }
}

/// A Comparator (RFC 8620 §5.5), with the `keyword` of RFC 8621 §4.4.2.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparator {
    /// The property to sort by.
    pub property: String,
    /// Ascending (none: true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ascending: Option<bool>,
    /// The collation algorithm (RFC 4790) for strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    /// The keyword, for the Email sorts `hasKeyword`, `allInThreadHaveKeyword` and
    /// `someInThreadHaveKeyword` (RFC 8621 §4.4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
}

/// The arguments of `Foo/query` (RFC 8620 §5.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "C: DeserializeOwned"))]
pub struct QueryRequest<C> {
    /// The account.
    pub account_id: Id,
    /// What the objects must match (none: all).
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter<C>>,
    /// The order of the results (none: the server's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    /// The first result to return, counting from 0; negative counts from the end (none: 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
    /// An id to start from instead of a position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Id>,
    /// The offset from the anchor (none: 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_offset: Option<i64>,
    /// The most results to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Whether to work out the total (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calculate_total: Option<bool>,
}

/// The response of `Foo/query` (RFC 8620 §5.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    /// The account.
    pub account_id: Id,
    /// The state of the query's results.
    pub query_state: String,
    /// Whether /queryChanges can work from this query.
    pub can_calculate_changes: bool,
    /// The position of the first id returned.
    pub position: u64,
    /// The ids, in order.
    pub ids: Vec<Id>,
    /// The number of results, when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The limit the server applied, when lower than the one asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// The arguments of `Foo/queryChanges` (RFC 8620 §5.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "C: DeserializeOwned"))]
pub struct QueryChangesRequest<C> {
    /// The account.
    pub account_id: Id,
    /// The filter of the query.
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter<C>>,
    /// The sort of the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Vec<Comparator>>,
    /// The query state the client has.
    pub since_query_state: String,
    /// The most changes to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_changes: Option<u64>,
    /// The last id the client has cached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_id: Option<Id>,
    /// Whether to work out the total (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calculate_total: Option<bool>,
}

/// An id added to the results of a query, and where (RFC 8620 §5.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedItem {
    /// The id.
    pub id: Id,
    /// Its index in the new results.
    pub index: u64,
}

/// The response of `Foo/queryChanges` (RFC 8620 §5.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChangesResponse {
    /// The account.
    pub account_id: Id,
    /// The `sinceQueryState` of the call.
    pub old_query_state: String,
    /// The state the changes bring the client to.
    pub new_query_state: String,
    /// The number of results, when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The ids removed from the results.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub removed: Vec<Id>,
    /// The ids added to the results (a null read as none).
    #[serde(default, deserialize_with = "null_as_empty")]
    pub added: Vec<AddedItem>,
}
