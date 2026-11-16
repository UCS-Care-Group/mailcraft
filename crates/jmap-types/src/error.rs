// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Errors at the three levels of RFC 8620 §3.6: the request (problem details, RFC 7807), a method call,
//! and one record of a /set or /copy (SetError, §5.3).
//!
//! The types are text: the RFCs define the common ones, and other specifications and servers add their
//! own, which a client treats as the RFC says (an unknown method error like `serverFail`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A request-level error (RFC 8620 §3.6.1): an RFC 7807 problem details object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestError {
    /// The problem type, such as `urn:ietf:params:jmap:error:limit`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The HTTP status code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// A short summary of the problem type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// An explanation of this occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The name of the limit exceeded, for `urn:ietf:params:jmap:error:limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Any further members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A problem type of RFC 8620 §3.6.1.
pub mod request_error {
    /// The request used a capability the server does not support.
    pub const UNKNOWN_CAPABILITY: &str = "urn:ietf:params:jmap:error:unknownCapability";
    /// The request was not `application/json`, or did not parse as I-JSON.
    pub const NOT_JSON: &str = "urn:ietf:params:jmap:error:notJSON";
    /// The request did not match the type of a Request object.
    pub const NOT_REQUEST: &str = "urn:ietf:params:jmap:error:notRequest";
    /// The request would exceed a limit, named in `limit`.
    pub const LIMIT: &str = "urn:ietf:params:jmap:error:limit";
}

/// A method-level error (RFC 8620 §3.6.2): the arguments of a response named `error`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MethodError {
    /// The error type, such as `invalidArguments` (see [`method_error`]).
    #[serde(rename = "type")]
    pub kind: String,
    /// A description for the client's developer, not for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The type's further properties.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The method error types of RFC 8620 §3.6.2 and of the standard methods (§5).
pub mod method_error {
    /// A resource was temporarily unavailable; trying later may work.
    pub const SERVER_UNAVAILABLE: &str = "serverUnavailable";
    /// An unexpected error; trying again is expected to fail.
    pub const SERVER_FAIL: &str = "serverFail";
    /// Some changes happened, some did not; resynchronise.
    pub const SERVER_PARTIAL_FAIL: &str = "serverPartialFail";
    /// The method is not known to the server.
    pub const UNKNOWN_METHOD: &str = "unknownMethod";
    /// An argument is of the wrong type or otherwise invalid.
    pub const INVALID_ARGUMENTS: &str = "invalidArguments";
    /// A result reference did not resolve.
    pub const INVALID_RESULT_REFERENCE: &str = "invalidResultReference";
    /// The call is not allowed for the credentials.
    pub const FORBIDDEN: &str = "forbidden";
    /// The account id is not one the credentials may use.
    pub const ACCOUNT_NOT_FOUND: &str = "accountNotFound";
    /// The account does not support this method.
    pub const ACCOUNT_NOT_SUPPORTED_BY_METHOD: &str = "accountNotSupportedByMethod";
    /// The call would change a read-only account.
    pub const ACCOUNT_READ_ONLY: &str = "accountReadOnly";
    /// Too many ids or objects in one call (§5.1, §5.3).
    pub const REQUEST_TOO_LARGE: &str = "requestTooLarge";
    /// The server cannot work out changes since the state given (§5.2, §5.6).
    pub const CANNOT_CALCULATE_CHANGES: &str = "cannotCalculateChanges";
    /// `ifInState` did not match the current state (§5.3, §5.4).
    pub const STATE_MISMATCH: &str = "stateMismatch";
    /// The `fromAccountId` of a /copy is not found (§5.4).
    pub const FROM_ACCOUNT_NOT_FOUND: &str = "fromAccountNotFound";
    /// The `fromAccountId` of a /copy does not support the method (§5.4).
    pub const FROM_ACCOUNT_NOT_SUPPORTED_BY_METHOD: &str = "fromAccountNotSupportedByMethod";
    /// The anchor of a /query is not in the results (§5.5).
    pub const ANCHOR_NOT_FOUND: &str = "anchorNotFound";
    /// A sort of a /query is not supported (§5.5).
    pub const UNSUPPORTED_SORT: &str = "unsupportedSort";
    /// A filter of a /query is not supported (§5.5).
    pub const UNSUPPORTED_FILTER: &str = "unsupportedFilter";
    /// More changes than `maxChanges` (§5.6).
    pub const TOO_MANY_CHANGES: &str = "tooManyChanges";
}

/// Why one record of a /set or /copy was not created, updated or destroyed (RFC 8620 §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetError {
    /// The error type, such as `invalidProperties` (see [`set_error`]).
    #[serde(rename = "type")]
    pub kind: String,
    /// A description for the client's developer (for the user, where the type says so).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The properties at fault, for `invalidProperties`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// The type's further properties (`existingId` of `alreadyExists`, `maxRecipients`, …).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The SetError types of RFC 8620 §5.3 and §5.4 and of RFC 8621.
pub mod set_error {
    /// The action is not allowed.
    pub const FORBIDDEN: &str = "forbidden";
    /// The account is out of quota.
    pub const OVER_QUOTA: &str = "overQuota";
    /// The object would be too large.
    pub const TOO_LARGE: &str = "tooLarge";
    /// Too many objects of this type were created recently.
    pub const RATE_LIMIT: &str = "rateLimit";
    /// The id to update or destroy was not found.
    pub const NOT_FOUND: &str = "notFound";
    /// The PatchObject is not valid.
    pub const INVALID_PATCH: &str = "invalidPatch";
    /// The object was destroyed in the same request.
    pub const WILL_DESTROY: &str = "willDestroy";
    /// Some properties are invalid; named in `properties`.
    pub const INVALID_PROPERTIES: &str = "invalidProperties";
    /// A singleton cannot be created or destroyed.
    pub const SINGLETON: &str = "singleton";
    /// A /copy found the object already exists, named in `existingId` (§5.4).
    pub const ALREADY_EXISTS: &str = "alreadyExists";
    /// A mailbox with children or emails cannot be destroyed (RFC 8621 §2.5).
    pub const MAILBOX_HAS_CHILD: &str = "mailboxHasChild";
    /// A mailbox with emails cannot be destroyed without `onDestroyRemoveEmails` (RFC 8621 §2.5).
    pub const MAILBOX_HAS_EMAIL: &str = "mailboxHasEmail";
    /// A blob the email names is not found (RFC 8621 §4.6), named in `notFound`.
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    /// The email has too many keywords (RFC 8621 §4.6).
    pub const TOO_MANY_KEYWORDS: &str = "tooManyKeywords";
    /// The email is in too many mailboxes (RFC 8621 §4.6).
    pub const TOO_MANY_MAILBOXES: &str = "tooManyMailboxes";
    /// The email to submit is not valid for sending (RFC 8621 §7.5).
    pub const INVALID_EMAIL: &str = "invalidEmail";
    /// Too many recipients, the limit in `maxRecipients` (RFC 8621 §7.5).
    pub const TOO_MANY_RECIPIENTS: &str = "tooManyRecipients";
    /// No recipients (RFC 8621 §7.5).
    pub const NO_RECIPIENTS: &str = "noRecipients";
    /// Some recipient addresses are not valid, named in `invalidRecipients` (RFC 8621 §7.5).
    pub const INVALID_RECIPIENTS: &str = "invalidRecipients";
    /// The envelope's sender is not allowed for the identity (RFC 8621 §7.5).
    pub const FORBIDDEN_MAIL_FROM: &str = "forbiddenMailFrom";
    /// The From header is not allowed for the identity (RFC 8621 §7.5).
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    /// The user may not send (RFC 8621 §7.5).
    pub const FORBIDDEN_TO_SEND: &str = "forbiddenToSend";
    /// The submission can no longer be cancelled (RFC 8621 §7.5).
    pub const CANNOT_UNSEND: &str = "cannotUnsend";
}
