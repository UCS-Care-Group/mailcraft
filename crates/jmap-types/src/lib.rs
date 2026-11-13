// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! JMAP request, response and object types.
//!
//! Serde types for JMAP Core (RFC 8620) and JMAP Mail (RFC 8621), with no I/O of their own, reusable
//! outside the client: the session, requests and responses, the standard methods' arguments and answers,
//! errors at each level, the mail objects and push's StateChange.
//!
//! The types follow the RFCs' property names (camelCase on the wire). They stay close to the text:
//!
//! - A property or argument that may be omitted, or may be null, is an `Option` that is left out when
//!   `None`. RFC 8620 §3.5 makes an omitted argument mean its default, and null the default of whatever
//!   may be null; an `Option<bool>` that is `None` means the RFC's stated default.
//! - Objects (Mailbox, Email, …) have every property optional, because a /get may ask for some of them
//!   and a /set create sends some of them.
//! - Where the RFC leaves a shape open (capability objects, a PatchObject, a header field parsed in a form
//!   the client chose, an error's further properties), it is `serde_json::Value` or a map of them.
//!
//! - Reading is strict where the RFC fixes the shape: a [`Session`], an [`Account`] or a [`MailboxRights`]
//!   missing a required member, or with one of the wrong type, is an error as a whole (a client that
//!   would rather skip one bad account reads the JSON itself), and a /query filter that is not one the
//!   RFCs allow (an unknown operator or condition, anywhere in it) is an error, never a filter that
//!   matches more than was asked.
//! - Writing leaves out every `None`, so a null is never written for a property: a top-level Mailbox is
//!   written without `parentId`. That is what a client's requests need (RFC 8620 §3.5); a writer of /get
//!   answers that must name each property asked for, nulls included, puts them in itself.
//!
//! Nothing here checks what a server sends beyond its JSON types: a reader of hostile input bounds sizes
//! and counts itself. Reading a filter buffers each level of it, so its cost grows with its size times its
//! depth: cap the size of a request before reading it.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(missing_docs)]

pub mod email;
pub mod error;
pub mod mailbox;
pub mod method;
mod nullable;
pub mod push;
pub mod request;
pub mod session;
pub mod submission;

pub use email::{
    Email, EmailAddress, EmailAddressGroup, EmailBodyPart, EmailBodyValue, EmailFilterCondition, EmailGetRequest, EmailHeader,
    EmailQueryChangesRequest, EmailQueryRequest, Thread,
};
pub use error::{MethodError, RequestError, SetError, method_error, request_error, set_error};
pub use mailbox::{Mailbox, MailboxChangesResponse, MailboxFilterCondition, MailboxQueryRequest, MailboxRights, MailboxSetRequest};
pub use method::{
    AddedItem, ChangesRequest, ChangesResponse, Comparator, Filter, FilterOperator, GetRequest, GetResponse, Operator, PatchObject,
    QueryChangesRequest, QueryChangesResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
pub use push::StateChange;
pub use request::{Invocation, Request, Response, ResultReference};
pub use session::{
    Account, CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_SUBMISSION, CAPABILITY_VACATION_RESPONSE, CoreCapability, MailCapability, Session,
    SubmissionCapability,
};
pub use submission::{Address, DeliveryStatus, EmailSubmission, EmailSubmissionSetRequest, Envelope, Identity};

/// An id (RFC 8620 §1.2): 1 to 255 characters of `A-Za-z0-9_-`. Kept as text; the types do not check it.
pub type Id = String;
