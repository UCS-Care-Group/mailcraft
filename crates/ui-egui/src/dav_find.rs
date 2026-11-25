// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find in File › Info › Calendars and Contacts (#209, D7): what the window asks of the host
//! ([`DavFindJob`]) and what the host tells it ([`DavFindEvent`]).
//!
//! The window only collects; the host searches on a thread of its own, and nothing is sent before
//! the owner presses Find (`mailcraft_net::dav_find`). The host asks before the password may go
//! to a host that isn't the owner's, and the window puts the question to the owner, naming the
//! host whole. What comes back is homes and counts, or a kind of failure the window says in its
//! own words.
//!
//! The password travels in the job, in memory, and is in no log line: the job has no `Debug`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_model::{DavFindFailure, DavFindQuestion, DavFound};

/// What the host is asked to do ([`crate::Services::dav_find`]).
pub struct DavFindJob {
    /// The account's address.
    pub email: String,
    /// The account's mail server (its IMAP host); empty for none.
    pub mail_host: String,
    /// The name that signs in; empty means the address.
    pub username: String,
    /// The password: the servers' own, or the account's mail password.
    pub password: String,
    /// The calendars' address the owner typed; empty to search.
    pub calendars: String,
    /// The contacts' address the owner typed; empty to search.
    pub contacts: String,
    /// The SHA-256 fingerprint of the one certificate the owner trusts for these servers besides
    /// the system's; empty for none.
    pub trusted: String,
    /// Set when the owner gives up (the dialog closed): the host stops at the next safe place.
    pub cancel: Arc<AtomicBool>,
    /// The owner's answers to the host's questions ([`DavFindEvent::Question`]): yes or no.
    pub answers: Receiver<bool>,
}

/// What the host tells the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DavFindEvent {
    /// May the password go to `host`? The window asks the owner and sends the answer.
    Question { host: String, why: DavFindQuestion },
    /// The search is over.
    Finished(Result<DavFound, DavFindFailure>),
}

/// What the window says of a failed search, in its own words.
pub(crate) fn said(failure: DavFindFailure) -> &'static str {
    match failure {
        DavFindFailure::BadSettings => tl!("These settings can't be used as they are: check the addresses (https://) and the user name."),
        DavFindFailure::Microsoft => {
            tl!("This is a Microsoft address: its calendars and contacts come with the Microsoft account. Nothing was sent.")
        }
        DavFindFailure::Google => tl!("This is a Google address: calendars and contacts from Google aren't read this way. Nothing was sent."),
        DavFindFailure::Unreachable => {
            tl!("No server answered at the usual places for this address. Check the connection, or type the server's address.")
        }
        DavFindFailure::NotFound => tl!("No calendar or contacts server was found for this address. Type the server's address."),
        DavFindFailure::NotDavServer => {
            tl!("A server asked for a password without saying it keeps calendars or contacts, so none was sent. Type the server's address.")
        }
        DavFindFailure::SecureConnection => tl!("A secure connection to the server couldn't be set up. Check the address."),
        DavFindFailure::Unreadable => tl!("The server's answer couldn't be read, or led somewhere that isn't followed. Type the server's address."),
        DavFindFailure::ServerRefused => tl!("The server or the organisation wouldn't give its calendars or contacts. Ask your administrator."),
        DavFindFailure::NoHome => tl!("The server didn't say where your calendars or contacts are. Type the server's address."),
        DavFindFailure::SignInNotSupported => tl!("The server wants a way of signing in that MailCraft can't do yet."),
        DavFindFailure::Declined => tl!("You didn't agree to send the password to the server the search led to. Type the server's address."),
        DavFindFailure::SignInRefused => tl!("The server didn't accept the user name and password. Nothing else was tried."),
        DavFindFailure::TooLong => tl!("The search went on too long without an end. Type the server's address."),
    }
}

/// What the window asks the owner about a host, in its own words.
pub(crate) fn question(why: DavFindQuestion) -> &'static str {
    match why {
        DavFindQuestion::OwnDomain => tl!(
            "The search found {host} in your mail domain. It isn't your mail server, and may be a web site rather than a calendar server. Send your password there?"
        ),
        DavFindQuestion::Elsewhere => tl!("The search was sent to {host}, which isn't one of your servers. Send your password there?"),
    }
}
