// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Identities (RFC 8621 §6) and email submissions (§7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Id, PatchObject, SetRequest};

/// An Identity (RFC 8621 §6): an address the user may send from. Every property is optional: a /get may
/// ask for some, and a /set create gives some.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// The id (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The name for `From` (none: empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The address for `From`; a `*` before the `@` stands for any name at that domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The `Reply-To` to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    /// The `Bcc` to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    /// The signature for plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    /// The signature for HTML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_signature: Option<String>,
    /// Whether the user may destroy it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
}

/// An EmailSubmission (RFC 8621 §7): an email sent, or to be sent. Every property is optional: a /get may
/// ask for some, and a /set create gives some.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmission {
    /// The id (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The identity to send as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<Id>,
    /// The email to send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_id: Option<Id>,
    /// The email's thread (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// The SMTP envelope (none: worked out from the email's header fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
    /// When it is or was to be sent (a UTCDate; server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<String>,
    /// `pending`, `final` or `canceled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    /// How delivery went, by recipient address (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<BTreeMap<String, DeliveryStatus>>,
    /// The blobs of delivery status notifications received for it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsn_blob_ids: Option<Vec<Id>>,
    /// The blobs of message disposition notifications received for it (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mdn_blob_ids: Option<Vec<Id>>,
}

/// An SMTP envelope (RFC 8621 §7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    /// The `MAIL FROM`.
    pub mail_from: Address,
    /// The `RCPT TO`s.
    pub rcpt_to: Vec<Address>,
}

/// An address of an SMTP envelope, with its parameters (RFC 8621 §7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// The address.
    pub email: String,
    /// The SMTP parameters, by name, each with its value (none for a parameter without one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, Option<String>>>,
}

/// How delivery to one recipient went (RFC 8621 §7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStatus {
    /// The last SMTP reply for the recipient.
    pub smtp_reply: String,
    /// `queued`, `yes`, `no` or `unknown`.
    pub delivered: String,
    /// `unknown` or `yes`.
    pub displayed: String,
}

/// The arguments of `EmailSubmission/set` (RFC 8621 §7.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmissionSetRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub set: SetRequest<EmailSubmission>,
    /// Emails to update once their submission succeeds, by submission id or `#` creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_email: Option<BTreeMap<String, PatchObject>>,
    /// Emails to destroy once their submission succeeds, as submission ids or `#` creation ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_destroy_email: Option<Vec<String>>,
}
