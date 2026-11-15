// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mailboxes (RFC 8621 §2) and the arguments their methods add to the standard ones.

use serde::{Deserialize, Serialize};

use crate::{ChangesResponse, Id, QueryRequest, SetRequest};

/// A Mailbox (RFC 8621 §2). Every property is optional: a /get may ask for some, and a /set create
/// gives some.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    /// The id (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The name to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The parent mailbox (none: the top level).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    /// The role, from the IANA "IMAP Mailbox Name Attributes" registry in lower case (`inbox`, `sent`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Where to sort it among its siblings (none: 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u64>,
    /// The number of emails in it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_emails: Option<u64>,
    /// The number of emails in it without `$seen` (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_emails: Option<u64>,
    /// The number of threads with an email in it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_threads: Option<u64>,
    /// The number of those threads with an unread email in it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_threads: Option<u64>,
    /// What the user may do with it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<MailboxRights>,
    /// Whether the user has subscribed to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
}

/// What the user may do with a mailbox (RFC 8621 §2, `myRights`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxRights {
    /// Read the emails in it.
    pub may_read_items: bool,
    /// Add emails to it.
    pub may_add_items: bool,
    /// Take emails out of it.
    pub may_remove_items: bool,
    /// Set or clear `$seen` on its emails.
    pub may_set_seen: bool,
    /// Set or clear other keywords on its emails.
    pub may_set_keywords: bool,
    /// Create a mailbox under it.
    pub may_create_child: bool,
    /// Rename or move it.
    pub may_rename: bool,
    /// Destroy it.
    pub may_delete: bool,
    /// Submit emails in it.
    pub may_submit: bool,
}

/// The response of `Mailbox/changes` (RFC 8621 §2.2): the standard one and `updatedProperties`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxChangesResponse {
    /// The standard response.
    #[serde(flatten)]
    pub changes: ChangesResponse,
    /// When only counts changed, the properties that did (none: any may have).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_properties: Option<Vec<String>>,
}

/// A FilterCondition of `Mailbox/query` (RFC 8621 §2.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailboxFilterCondition {
    /// The parent (`Some(None)`: the top level).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::nullable")]
    pub parent_id: Option<Option<Id>>,
    /// Text the name holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The role (`Some(None)`: no role).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::nullable")]
    pub role: Option<Option<String>>,
    /// Whether it has any role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_any_role: Option<bool>,
    /// Whether the user has subscribed to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
}

/// The arguments of `Mailbox/query` (RFC 8621 §2.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxQueryRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub query: QueryRequest<MailboxFilterCondition>,
    /// Sort parents before their children (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_as_tree: Option<bool>,
    /// Leave out a mailbox whose parent does not match (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_as_tree: Option<bool>,
}

/// The arguments of `Mailbox/set` (RFC 8621 §2.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSetRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub set: SetRequest<Mailbox>,
    /// Destroy a mailbox's emails with it when they are in no other (none: false, and a mailbox with
    /// emails is not destroyed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_emails: Option<bool>,
}
