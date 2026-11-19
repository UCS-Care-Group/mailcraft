// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What can go wrong with a request, in kinds and in our own words: no error holds text a server
//! wrote (a fault's reason may echo what was sent, the password among it).

use std::fmt;

use crate::http::{EndpointError, TransportError};

/// A fault the server answered with, by the kinds the crate tells apart. The server's own code is
/// read only to choose one of these; its reason is never kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fault {
    /// The account may not do this (`service.PERM_DENIED`).
    PermissionDenied,
    /// Something of that name is there already (`mail.ALREADY_EXISTS`).
    AlreadyExists,
    /// The server didn't take the request as it was written (`service.INVALID_REQUEST`, a parse
    /// error): a fault of the client's, or of a server that speaks another version.
    InvalidRequest,
    /// The account can't be used now (maintenance, locked, closed, a password that must be
    /// changed first).
    AccountUnavailable,
    /// The mailbox is over its quota (`mail.QUOTA_EXCEEDED`).
    QuotaExceeded,
    /// Sending was refused (`mail.SEND_FAILURE`, `mail.SEND_ABORTED_ADDRESS_FAILURE`).
    SendRefused,
    /// Any other fault.
    Other,
}

/// What went wrong with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server's address is not one a request may go to.
    Endpoint(EndpointError),
    /// The exchange itself failed.
    Transport(TransportError),
    /// The server turned the sign-in down: a wrong password, or a token it no longer takes.
    AuthRefused,
    /// The account signs in with a second step, and no code went with the sign-in, or the server
    /// turned the code down: the owner is asked for one.
    TwoFactor,
    /// The change token is too old for the server to go on from (`mail.MUST_RESYNC`): the mailbox
    /// is read afresh.
    MustResync,
    /// There is no such item (or folder) any more (`mail.NO_SUCH_*`).
    NoSuchItem,
    /// The server is busy and asked to be left alone for this many seconds.
    Busy(u32),
    /// The server answered with a fault of this kind.
    Fault(Fault),
    /// The server answered with this HTTP status and nothing the crate could read.
    Status(u16),
    /// The answer couldn't be read as what was asked for (not JSON, cut short, too deep, missing
    /// what it must have).
    Unreadable,
}

impl Error {
    /// Whether the run can go on with the next request after this one (the connection and the
    /// sign-in are still good).
    pub fn is_local(self) -> bool {
        matches!(self, Error::NoSuchItem | Error::Fault(_))
    }
}

impl From<TransportError> for Error {
    fn from(e: TransportError) -> Self {
        Error::Transport(e)
    }
}

impl From<EndpointError> for Error {
    fn from(e: EndpointError) -> Self {
        Error::Endpoint(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Endpoint(e) => e.fmt(f),
            Error::Transport(e) => e.fmt(f),
            Error::AuthRefused => f.write_str("the Zimbra server turned the sign-in down: check the user name and the password"),
            Error::TwoFactor => f.write_str("this Zimbra account wants the code from your authenticator app"),
            Error::MustResync => f.write_str("the Zimbra server can no longer say what changed since the last Send/Receive"),
            Error::NoSuchItem => f.write_str("the Zimbra server no longer has what it was asked for"),
            Error::Busy(_) => f.write_str("the Zimbra server is busy and asked to be left alone for a while"),
            Error::Fault(Fault::PermissionDenied) => f.write_str("the Zimbra server doesn't allow this for the account"),
            Error::Fault(Fault::AlreadyExists) => f.write_str("the Zimbra server has one of that name already"),
            Error::Fault(Fault::InvalidRequest) => f.write_str("the Zimbra server didn't take the request"),
            Error::Fault(Fault::AccountUnavailable) => f.write_str("the Zimbra account can't be used now (it may be locked or need a new password)"),
            Error::Fault(Fault::QuotaExceeded) => f.write_str("the Zimbra mailbox is full"),
            Error::Fault(Fault::SendRefused) => f.write_str("the Zimbra server refused to send the message"),
            Error::Fault(Fault::Other) => f.write_str("the Zimbra server refused the request"),
            Error::Status(status) => write!(f, "the Zimbra server answered with HTTP {status}"),
            Error::Unreadable => f.write_str("the Zimbra server's answer couldn't be read"),
        }
    }
}

impl std::error::Error for Error {}

/// The kind of error a fault's code says; the code is the server's and is read, never kept.
pub(crate) fn from_code(code: &str) -> Error {
    match code {
        "account.AUTH_FAILED" | "service.AUTH_REQUIRED" | "service.AUTH_EXPIRED" => Error::AuthRefused,
        "account.TWO_FACTOR_AUTH_REQUIRED" | "account.TWO_FACTOR_AUTH_FAILED" | "account.TWO_FACTOR_SETUP_REQUIRED" => Error::TwoFactor,
        "mail.MUST_RESYNC" => Error::MustResync,
        "service.PERM_DENIED" => Error::Fault(Fault::PermissionDenied),
        "mail.ALREADY_EXISTS" => Error::Fault(Fault::AlreadyExists),
        "service.INVALID_REQUEST" | "service.PARSE_ERROR" | "service.UNKNOWN_DOCUMENT" => Error::Fault(Fault::InvalidRequest),
        "account.MAINTENANCE_MODE" | "account.ACCOUNT_INACTIVE" | "account.CHANGE_PASSWORD" | "account.PASSWORD_LOCKED" => {
            Error::Fault(Fault::AccountUnavailable)
        }
        "mail.QUOTA_EXCEEDED" => Error::Fault(Fault::QuotaExceeded),
        "mail.SEND_FAILURE" | "mail.SEND_ABORTED_ADDRESS_FAILURE" | "mail.SEND_PARTIAL_ADDRESS_FAILURE" => Error::Fault(Fault::SendRefused),
        code if code.starts_with("mail.NO_SUCH_") => Error::NoSuchItem,
        _ => Error::Fault(Fault::Other),
    }
}
