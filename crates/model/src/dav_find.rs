// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a search for an account's calendar and contacts servers found (#209, D7), as the host
//! hands it to the window: the homes that answered and how much is in them, or why nothing was
//! found, as a kind the window says in its own words. Nothing here is a server's sentence.

use std::fmt;

/// The homes a search found, and what is in them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DavFound {
    /// The calendar homes that answered, absolute; empty when calendars weren't found.
    pub calendar_homes: Vec<String>,
    /// The address book homes that answered, absolute; empty when contacts weren't found.
    pub contact_homes: Vec<String>,
    /// The calendars that hold events.
    pub calendars: usize,
    /// The address books.
    pub address_books: usize,
    /// What the homes held that isn't read (task lists, plain files, the home itself).
    pub passed_over: usize,
}

/// Only counts: the homes name the owner's servers.
impl fmt::Debug for DavFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DavFound")
            .field("calendar_homes", &self.calendar_homes.len())
            .field("contact_homes", &self.contact_homes.len())
            .field("calendars", &self.calendars)
            .field("address_books", &self.address_books)
            .field("passed_over", &self.passed_over)
            .finish()
    }
}

/// Why a search found nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DavFindFailure {
    /// The address, a user name or a typed address can't be used as it is.
    BadSettings,
    /// The address or a host the search led to is Microsoft's: nothing was sent there.
    Microsoft,
    /// The address or a host the search led to is Google's: nothing was sent there.
    Google,
    /// Nobody answered at the places looked at.
    Unreachable,
    /// No calendar or contacts server at the places looked at.
    NotFound,
    /// A host asked for a password without saying it keeps calendars or contacts: none was sent.
    NotDavServer,
    /// A secure connection couldn't be set up (the certificate, for one).
    SecureConnection,
    /// An answer that can't be read, or a redirect that isn't followed.
    Unreadable,
    /// The server or the organisation refused.
    ServerRefused,
    /// The server didn't say who the owner is on it, or where the calendars or contacts are.
    NoHome,
    /// The server signs in only in ways this version doesn't.
    SignInNotSupported,
    /// The owner said no to the host the search led to.
    Declined,
    /// The user name or password wasn't accepted. Nothing else was asked.
    SignInRefused,
    /// The search went on for too long (redirects, requests).
    TooLong,
}

/// Why the owner is asked before the password may go to a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DavFindQuestion {
    /// The host is in the owner's mail domain, but neither the mail server nor one the domain's
    /// records name: it may be a web site rather than a calendar server.
    OwnDomain,
    /// The search was sent to the host, and it isn't one of the owner's.
    Elsewhere,
}
