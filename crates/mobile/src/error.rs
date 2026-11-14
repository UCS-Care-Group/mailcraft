// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The errors that cross the binding. Every export returns one of them, so the native side never
//! meets a thrown exception it was not told about, and a failure of native code that Rust called
//! (`UnexpectedUniFFICallbackError`) becomes an error here, never a panic.

use mailcraft_engine::CmdError;
use mailcraft_host::{InitError, PlatformError, SecretError};
use uniffi::UnexpectedUniFFICallbackError;

/// Why a call of the core did nothing (or failed). The text is the program's own words; it never
/// carries a secret or what the system said.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Error)]
pub enum CoreError {
    /// The platform can't be used: it says it can spare no memory, or its set-up failed.
    Init { reason: String },
    /// A command's parameters, or a query's, make no sense, or the command isn't known.
    BadRequest { reason: String },
    /// The command or the query is not available now, or failed; `reason` is for the owner.
    Failed { reason: String },
    /// The call was cancelled before it ran, or the core was shut down.
    Cancelled,
    /// Something went wrong inside the core that it recovered from: nothing was changed.
    Internal,
    /// The native side failed in a way it didn't describe.
    Native { reason: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Init { reason } | CoreError::BadRequest { reason } | CoreError::Failed { reason } | CoreError::Native { reason } => {
                f.write_str(reason)
            }
            CoreError::Cancelled => f.write_str("cancelled"),
            CoreError::Internal => f.write_str("something went wrong; nothing was changed"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<UnexpectedUniFFICallbackError> for CoreError {
    fn from(_: UnexpectedUniFFICallbackError) -> Self {
        CoreError::Native { reason: "the app's own code failed".to_string() }
    }
}

impl From<InitError> for CoreError {
    fn from(e: InitError) -> Self {
        CoreError::Init { reason: e.to_string() }
    }
}

impl From<CmdError> for CoreError {
    fn from(e: CmdError) -> Self {
        match e {
            CmdError::Unknown(_) | CmdError::Params(_) => CoreError::BadRequest { reason: e.to_string() },
            CmdError::Disabled(_) | CmdError::Failed(_) | CmdError::Said(_) => CoreError::Failed { reason: e.to_string() },
        }
    }
}

/// Why a platform entry (Kotlin or Swift code) did nothing. The words are the program's: native
/// code names the kind of failure and never passes a message along, which may echo a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Error)]
pub enum HostError {
    /// This platform has no such thing.
    Unsupported,
    /// The system refused or failed.
    Failed,
    /// A secret is longer than the store can keep.
    TooLong,
    /// A connection or a file failed.
    Io,
    /// The native code failed in a way it didn't describe (it threw, or answered wrongly).
    Native,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HostError::Unsupported => "this platform doesn't offer that",
            HostError::Failed => "the platform refused or failed",
            HostError::TooLong => "it is too long for the system's credential store",
            HostError::Io => "the platform's input or output failed",
            HostError::Native => "the app's own code failed",
        })
    }
}

impl std::error::Error for HostError {}

impl From<UnexpectedUniFFICallbackError> for HostError {
    fn from(_: UnexpectedUniFFICallbackError) -> Self {
        HostError::Native
    }
}

impl From<HostError> for PlatformError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::Unsupported => PlatformError::Unsupported,
            HostError::Failed => PlatformError::Failed("the platform refused or failed"),
            HostError::TooLong => PlatformError::Failed("it is too long for the platform"),
            HostError::Io => PlatformError::Io(std::io::ErrorKind::Other),
            HostError::Native => PlatformError::Failed("the app's own code failed"),
        }
    }
}

impl From<HostError> for SecretError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::TooLong => SecretError::TooLong,
            HostError::Unsupported | HostError::Failed | HostError::Io | HostError::Native => SecretError::Refused,
        }
    }
}
