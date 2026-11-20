// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Errors from reading an affix or dictionary file.

use std::fmt;

/// A dictionary file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// `SET` names a character set other than UTF-8.
    UnsupportedEncoding,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedEncoding => f.write_str("dictionary encoding is not UTF-8"),
        }
    }
}

impl std::error::Error for Error {}
