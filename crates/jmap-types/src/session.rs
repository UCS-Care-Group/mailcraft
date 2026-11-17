// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Session object (RFC 8620 §2) and the capability objects of core (§2) and mail (RFC 8621 §1.3).

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Id;

/// The core capability (RFC 8620 §2).
pub const CAPABILITY_CORE: &str = "urn:ietf:params:jmap:core";
/// The mail capability (RFC 8621 §1.3.1).
pub const CAPABILITY_MAIL: &str = "urn:ietf:params:jmap:mail";
/// The submission capability (RFC 8621 §1.3.2).
pub const CAPABILITY_SUBMISSION: &str = "urn:ietf:params:jmap:submission";
/// The vacation response capability (RFC 8621 §1.3.3).
pub const CAPABILITY_VACATION_RESPONSE: &str = "urn:ietf:params:jmap:vacationresponse";

/// A Session object (RFC 8620 §2). Capability objects are kept as JSON, their shape being the
/// capability's own; [`Session::capability`] reads one as a type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The capabilities the server supports, by identifier.
    pub capabilities: BTreeMap<String, Value>,
    /// The accounts the credentials give access to, by account id.
    pub accounts: BTreeMap<Id, Account>,
    /// The account to use for each capability, by capability identifier.
    #[serde(default)]
    pub primary_accounts: BTreeMap<String, Id>,
    /// The user name the credentials are for (may be empty).
    #[serde(default)]
    pub username: String,
    /// The address of the API endpoint.
    pub api_url: String,
    /// The download template (RFC 6570 level 1).
    pub download_url: String,
    /// The upload template.
    pub upload_url: String,
    /// The push template (RFC 8620 §7.3).
    pub event_source_url: String,
    /// The state of this object.
    pub state: String,
}

impl Session {
    /// The server-level object of `capability` read as a `T`, when the session lists it.
    pub fn capability<T: DeserializeOwned>(&self, capability: &str) -> Option<Result<T, serde_json::Error>> {
        self.capabilities.get(capability).map(|value| T::deserialize(value))
    }

    /// The core capability (RFC 8620 §2), when the session lists it.
    pub fn core_capability(&self) -> Option<Result<CoreCapability, serde_json::Error>> {
        self.capability(CAPABILITY_CORE)
    }
}

/// An account (RFC 8620 §1.6.2, §2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// A name to show for the account.
    pub name: String,
    /// Whether the account belongs to the authenticated user.
    pub is_personal: bool,
    /// Whether the whole account is read-only.
    pub is_read_only: bool,
    /// The account-level capability objects, by identifier.
    #[serde(default)]
    pub account_capabilities: BTreeMap<String, Value>,
}

impl Account {
    /// The account-level object of `capability` read as a `T`, when the account lists it.
    pub fn capability<T: DeserializeOwned>(&self, capability: &str) -> Option<Result<T, serde_json::Error>> {
        self.account_capabilities.get(capability).map(|value| T::deserialize(value))
    }
}

/// The core capability's object (RFC 8620 §2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreCapability {
    /// The largest file the server accepts, in octets.
    pub max_size_upload: u64,
    /// The most uploads at once.
    pub max_concurrent_upload: u64,
    /// The largest request, in octets.
    pub max_size_request: u64,
    /// The most requests at once. (The example of RFC 8620 §2.1 spells it without the final "s"; that
    /// spelling is read too.)
    #[serde(alias = "maxConcurrentRequest")]
    pub max_concurrent_requests: u64,
    /// The most method calls in one request.
    pub max_calls_in_request: u64,
    /// The most objects one /get may ask for.
    pub max_objects_in_get: u64,
    /// The most objects one /set may change.
    pub max_objects_in_set: u64,
    /// The collation algorithms the server supports (RFC 4790).
    #[serde(default)]
    pub collation_algorithms: Vec<String>,
}

/// The mail capability's account-level object (RFC 8621 §1.3.1). The server-level object is empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailCapability {
    /// The most mailboxes one email may be in (none: no limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mailboxes_per_email: Option<u64>,
    /// The deepest mailbox tree (none: no limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mailbox_depth: Option<u64>,
    /// The longest mailbox name, in octets of UTF-8.
    pub max_size_mailbox_name: u64,
    /// The largest total size of attachments the server allows in an email it creates, in octets.
    pub max_size_attachments_per_email: u64,
    /// The sort properties Email/query supports.
    pub email_query_sort_options: Vec<String>,
    /// Whether the user may create a mailbox at the top level.
    pub may_create_top_level_mailbox: bool,
}

/// The submission capability's account-level object (RFC 8621 §1.3.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionCapability {
    /// The longest delay of a delayed send, in seconds (0: none supported).
    pub max_delayed_send: u64,
    /// The SMTP submission extensions supported, by EHLO name, with their arguments.
    pub submission_extensions: BTreeMap<String, Vec<String>>,
}
