// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Threads (RFC 8621 §3) and emails (§4), and the arguments the email methods add to the standard ones.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{GetRequest, Id, QueryChangesRequest, QueryRequest};

/// A Thread (RFC 8621 §3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// The id (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The emails in it, oldest first (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_ids: Option<Vec<Id>>,
}

/// An Email (RFC 8621 §4.1). Every property is optional: a /get may ask for some, and a /set create
/// gives some.
///
/// The header fields asked for by name and form (`header:List-Post:asURLs`, RFC 8621 §4.1.3) have
/// property names of the client's making; they are in [`Email::extra`], as JSON, under those names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    /// The id (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The blob of the whole message (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The thread (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// The mailboxes it is in, each to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<BTreeMap<Id, bool>>,
    /// The keywords (`$seen`, `$flagged`, …), each to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, bool>>,
    /// The size of the message, in octets (server-set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// When it arrived (a UTCDate, RFC 8620 §1.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<String>,
    /// Every header field, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// `Message-ID`, as message ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Vec<String>>,
    /// `In-Reply-To`, as message ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Vec<String>>,
    /// `References`, as message ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    /// `Sender`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Vec<EmailAddress>>,
    /// `From`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<EmailAddress>>,
    /// `To`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<EmailAddress>>,
    /// `Cc`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<EmailAddress>>,
    /// `Bcc`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    /// `Reply-To`, as addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    /// `Subject`, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `Date`, as a Date (RFC 8620 §1.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// The whole MIME structure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_structure: Option<EmailBodyPart>,
    /// The text of body parts, by part id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_values: Option<BTreeMap<String, EmailBodyValue>>,
    /// The parts to show as text, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Vec<EmailBodyPart>>,
    /// The parts to show as HTML, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Vec<EmailBodyPart>>,
    /// The attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<EmailBodyPart>>,
    /// Whether the email has attachments, as the server judges it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// Up to 256 characters of plain text from the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The header fields asked for by name and form (`header:…`), and any other property.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A header field as it is in the message (RFC 8621 §4.1.2, the Raw form of `headers`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailHeader {
    /// The field name, as written.
    pub name: String,
    /// The raw value, unfolded only by the server's choosing.
    pub value: String,
}

/// An address of the Addresses form (RFC 8621 §4.1.2.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailAddress {
    /// The display name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The address (`addr-spec`).
    pub email: String,
}

/// A group of the GroupedAddresses form (RFC 8621 §4.1.2.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailAddressGroup {
    /// The group's name (none: addresses not in a group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The addresses in it.
    pub addresses: Vec<EmailAddress>,
}

/// A body part (RFC 8621 §4.1.4). Every property is optional: the client chooses them
/// (`bodyProperties`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyPart {
    /// The id of the part within the email (none for a multipart).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    /// The blob of the decoded content (none for a multipart).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The size of the decoded content, in octets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The part's header fields, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// The file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type (`type`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The charset of a text part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    /// `inline` or `attachment`, from `Content-Disposition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    /// `Content-Id`, without its angle brackets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// `Content-Language`, as language tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Vec<String>>,
    /// `Content-Location`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The parts of a multipart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_parts: Option<Vec<EmailBodyPart>>,
    /// The header fields asked for by name and form (`header:…`), and any other property.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The text of a body part (RFC 8621 §4.1.4, `bodyValues`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyValue {
    /// The decoded text.
    pub value: String,
    /// Whether decoding met a problem (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_encoding_problem: Option<bool>,
    /// Whether the text was cut short at `maxBodyValueBytes` (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_truncated: Option<bool>,
}

/// The arguments of `Email/get` (RFC 8621 §4.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailGetRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub get: GetRequest,
    /// The properties of each body part to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_properties: Option<Vec<String>>,
    /// Return the values of the `textBody` parts (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_text_body_values: Option<bool>,
    /// Return the values of the `htmlBody` parts (none: false).
    #[serde(default, rename = "fetchHTMLBodyValues", skip_serializing_if = "Option::is_none")]
    pub fetch_html_body_values: Option<bool>,
    /// Return the values of every text part (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_all_body_values: Option<bool>,
    /// Cut each value at this many octets (none or 0: no limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_value_bytes: Option<u64>,
}

/// A FilterCondition of `Email/query` (RFC 8621 §4.4.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailFilterCondition {
    /// In this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_mailbox: Option<Id>,
    /// In a mailbox other than these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_mailbox_other_than: Option<Vec<Id>>,
    /// Arrived before (a UTCDate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Arrived at or after (a UTCDate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// At least this many octets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    /// Fewer than this many octets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    /// Every email in its thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_in_thread_have_keyword: Option<String>,
    /// Some email in its thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub some_in_thread_have_keyword: Option<String>,
    /// No email in its thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none_in_thread_have_keyword: Option<String>,
    /// Has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_keyword: Option<String>,
    /// Has not this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_keyword: Option<String>,
    /// `hasAttachment` is this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// Text in any of the addresses, the subject or the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Text in `From`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Text in `To`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Text in `Cc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    /// Text in `Bcc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,
    /// Text in the subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Text in the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// A header field's name, and optionally text its value must hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Vec<String>>,
}

/// The arguments of `Email/query` (RFC 8621 §4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailQueryRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub query: QueryRequest<EmailFilterCondition>,
    /// Keep only the first email of each thread (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_threads: Option<bool>,
}

/// The arguments of `Email/queryChanges` (RFC 8621 §4.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailQueryChangesRequest {
    /// The standard arguments.
    #[serde(flatten)]
    pub query_changes: QueryChangesRequest<EmailFilterCondition>,
    /// As in the query (none: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_threads: Option<bool>,
}
