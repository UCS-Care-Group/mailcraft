// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The crate's error type, arranged by what the mail client can do about each case, and the
//! response codes it is made from.
//!
//! No error holds text a server wrote: a server's words may be anything, and may hold a name or
//! a subject. A response code is kept only when it is a short identifier.

use std::fmt;

use crate::http::{EndpointError, TransportError};

/// The most a server may ask the client to wait, in milliseconds (five minutes); more is
/// taken as this.
pub const MAX_BACK_OFF_MS: u32 = 300_000;
/// The shortest wait a busy server is given (one second): a server that says 0 is not to be
/// asked again at once.
pub const MIN_BACK_OFF_MS: u32 = 1_000;
/// How long to wait when a server is busy and does not say for how long (one second).
pub const DEFAULT_BACK_OFF_MS: u32 = 1_000;
/// The longest response code kept.
const MAX_CODE: usize = 64;

/// How a response message says it went (`ResponseClass`, [MS-OXWSCDATA]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseClass {
    /// It was done.
    Success,
    /// It was done, with a remark.
    Warning,
    /// It was not done.
    Error,
}

impl ResponseClass {
    /// The class a document names; none for a word the schema does not have.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Success" => Some(Self::Success),
            "Warning" => Some(Self::Warning),
            "Error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Why a response message says no (`ResponseCodeType`, [MS-OXWSCDATA]): the ones the client
/// acts on by name, and any other as the identifier it was.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseCode {
    /// `NoError`.
    NoError,
    /// `ErrorServerBusy`: try again after the back-off.
    ServerBusy,
    /// `ErrorAccessDenied`.
    AccessDenied,
    /// `ErrorFolderNotFound`.
    FolderNotFound,
    /// `ErrorItemNotFound`.
    ItemNotFound,
    /// `ErrorNonExistentMailbox`.
    NonExistentMailbox,
    /// `ErrorInvalidSyncStateData`: the sync state is no longer known; read afresh.
    InvalidSyncState,
    /// `ErrorInvalidPropertyRequest`: the server doesn't know a property that was asked for.
    InvalidPropertyRequest,
    /// `ErrorSchemaValidation`: the request did not fit the schema (a bug here).
    SchemaValidation,
    /// `ErrorInvalidServerVersion`: the server does not know the version asked for.
    InvalidServerVersion,
    /// `ErrorInvalidRequest`.
    InvalidRequest,
    /// `ErrorInternalServerError` or `ErrorInternalServerTransientError`.
    InternalServerError,
    /// `ErrorMailboxStoreUnavailable` or `ErrorMailboxMoveInProgress`: the mailbox cannot be
    /// reached for now.
    MailboxUnavailable,
    /// `ErrorTimeoutExpired`.
    TimeoutExpired,
    /// Any other code, as the short identifier it was.
    Other(String),
}

impl ResponseCode {
    /// The code a document names. A name that is not a short identifier (letters and digits)
    /// is no code and reads as `Other("")`.
    pub fn from_name(name: &str) -> Self {
        match name.trim() {
            "NoError" => Self::NoError,
            "ErrorServerBusy" => Self::ServerBusy,
            "ErrorAccessDenied" => Self::AccessDenied,
            "ErrorFolderNotFound" => Self::FolderNotFound,
            "ErrorItemNotFound" => Self::ItemNotFound,
            "ErrorNonExistentMailbox" => Self::NonExistentMailbox,
            "ErrorInvalidSyncStateData" => Self::InvalidSyncState,
            "ErrorInvalidPropertyRequest" => Self::InvalidPropertyRequest,
            "ErrorSchemaValidation" => Self::SchemaValidation,
            "ErrorInvalidServerVersion" => Self::InvalidServerVersion,
            "ErrorInvalidRequest" => Self::InvalidRequest,
            "ErrorInternalServerError" | "ErrorInternalServerTransientError" => Self::InternalServerError,
            "ErrorMailboxStoreUnavailable" | "ErrorMailboxMoveInProgress" => Self::MailboxUnavailable,
            "ErrorTimeoutExpired" => Self::TimeoutExpired,
            other if !other.is_empty() && other.len() <= MAX_CODE && other.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                Self::Other(other.to_owned())
            }
            _ => Self::Other(String::new()),
        }
    }
}

impl fmt::Display for ResponseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoError => f.write_str("NoError"),
            Self::ServerBusy => f.write_str("ErrorServerBusy"),
            Self::AccessDenied => f.write_str("ErrorAccessDenied"),
            Self::FolderNotFound => f.write_str("ErrorFolderNotFound"),
            Self::ItemNotFound => f.write_str("ErrorItemNotFound"),
            Self::NonExistentMailbox => f.write_str("ErrorNonExistentMailbox"),
            Self::InvalidSyncState => f.write_str("ErrorInvalidSyncStateData"),
            Self::InvalidPropertyRequest => f.write_str("ErrorInvalidPropertyRequest"),
            Self::SchemaValidation => f.write_str("ErrorSchemaValidation"),
            Self::InvalidServerVersion => f.write_str("ErrorInvalidServerVersion"),
            Self::InvalidRequest => f.write_str("ErrorInvalidRequest"),
            Self::InternalServerError => f.write_str("ErrorInternalServerError"),
            Self::MailboxUnavailable => f.write_str("ErrorMailboxStoreUnavailable"),
            Self::TimeoutExpired => f.write_str("ErrorTimeoutExpired"),
            Self::Other(code) if code.is_empty() => f.write_str("an unnamed error"),
            Self::Other(code) => f.write_str(code),
        }
    }
}

/// Why the policy of an organisation shuts the service (`X-EWS-Policy-Reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyReason {
    /// "EWS is disabled for this user or tenant".
    Disabled,
    /// "EWS is blocked by policy for this user or tenant".
    Blocked,
    /// Any other words.
    Other,
}

impl PolicyReason {
    /// What the header says, told apart.
    pub fn from_header(value: &str) -> Self {
        let value = value.to_ascii_lowercase();
        if value.contains("disabled") {
            Self::Disabled
        } else if value.contains("blocked") {
            Self::Blocked
        } else {
            Self::Other
        }
    }
}

/// Why an operation did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The address is not one a request goes to.
    Endpoint(EndpointError),
    /// The program could not complete the exchange.
    Transport(TransportError),
    /// 401: the credentials are not accepted.
    Unauthorized,
    /// 403 with a policy reason: the organisation does not allow the service.
    Blocked(PolicyReason),
    /// 403 without a reason.
    Forbidden,
    /// The server is busy (`ErrorServerBusy`, 429 or 503): wait `back_off_ms` and try again.
    Busy {
        /// How long to wait, in milliseconds, capped at [`MAX_BACK_OFF_MS`].
        back_off_ms: u32,
    },
    /// The server answered with an error of its own, in a fault or in a response message.
    Server(ResponseCode),
    /// The answer is not a document that can be read: not XML, cut short, nested without end.
    Malformed,
    /// The answer is a document but not the answer to this request (wrong namespace, a missing
    /// element, fewer answers than questions, a value that is not one).
    Unexpected,
    /// A change in the answer can't be taken in (it has no usable id, or no item): the page holding
    /// it is no page, since its state would say the change was taken in.
    Unreadable,
    /// The answer was longer than the limit.
    TooLarge,
    /// An HTTP status the service does not use.
    Status(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(e) => e.fmt(f),
            Self::Transport(e) => e.fmt(f),
            Self::Unauthorized => f.write_str("the server did not accept the user name and password"),
            Self::Blocked(PolicyReason::Disabled) => f.write_str("the organisation has switched Exchange Web Services off for this user"),
            Self::Blocked(_) => f.write_str("the organisation does not allow Exchange Web Services for this user"),
            Self::Forbidden => f.write_str("the server refused access to Exchange Web Services"),
            Self::Busy { back_off_ms } => write!(f, "the server is busy; trying again in {} s", back_off_ms.div_ceil(1000)),
            Self::Server(code) => write!(f, "the server answered with {code}"),
            Self::Malformed => f.write_str("the server's answer could not be read"),
            Self::Unexpected => f.write_str("the server's answer is not the answer to the request"),
            Self::Unreadable => f.write_str("a change in the server's answer could not be taken in"),
            Self::TooLarge => f.write_str("the server's answer is larger than any this program reads"),
            Self::Status(code) => write!(f, "the server answered with HTTP status {code}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<EndpointError> for Error {
    fn from(error: EndpointError) -> Self {
        Self::Endpoint(error)
    }
}

impl From<crate::xml::Malformed> for Error {
    fn from(_: crate::xml::Malformed) -> Self {
        Self::Malformed
    }
}
