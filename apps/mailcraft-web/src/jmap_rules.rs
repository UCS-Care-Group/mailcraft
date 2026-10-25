// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the browser's JMAP host (`jmap.rs`) decides that needs no browser: which accounts are the
//! page's, whether a run the server turned the page's credential down for goes again, and what is
//! said of a redirect the browser doesn't follow. Kept apart from `jmap.rs` so that it is tested on
//! the host.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use mailcraft_jmap::{Error, TransportError};
use mailcraft_model::sync::{JmapMailRequest, JmapMailResult};

/// An account the page named.
pub(crate) struct PageAccount {
    pub(crate) email: String,
    pub(crate) session: String,
    pub(crate) bearer: bool,
    pub(crate) name: String,
}

/// Whether `accounts` name the account at `email` on the server whose session address is
/// `session`: the address in any case, the session address as the page gave it (another one is
/// another server, on the same host or not).
pub(crate) fn named(accounts: &[PageAccount], email: &str, session: &str) -> bool {
    accounts.iter().any(|p| p.email.eq_ignore_ascii_case(email.trim()) && p.session == session.trim())
}

/// What a run does with the secret at hand now.
#[derive(Debug)]
pub(crate) enum Next {
    /// It goes, with this secret.
    Go(String),
    /// It ends as the server answered the try before: with that try's last piece.
    End(Box<JmapMailResult>),
    /// It ends with no secret to go with.
    NoSecret,
}

/// What a run does once it has `fresh`, the secret at hand now (the one typed, or the page's),
/// after the server turned down `turned_down`, the one the try before went with, and that try's
/// last piece. Only a secret the server hasn't turned down goes.
pub(crate) fn next(fresh: Option<String>, turned_down: Option<(String, JmapMailResult)>) -> Next {
    match (fresh, turned_down) {
        // The page has nothing newer.
        (Some(secret), Some((before, last))) if secret == before => Next::End(Box::new(last)),
        (None, Some((_, last))) => Next::End(Box::new(last)),
        (None, None) => Next::NoSecret,
        (Some(secret), _) => Next::Go(secret),
    }
}

/// Whether a run that ended with `last` goes again: once, when the server turned down the page's
/// credential (`held`), so that the page can hand over a renewed one. One the owner typed is asked
/// for again by the window.
pub(crate) fn again(last: &JmapMailResult, held: bool, tried_again: bool) -> bool {
    last.auth_refused && held && !tried_again
}

/// The last piece of a run stopped before it read anything: while the page was asked for the
/// credential, or while signing in (the exchange called off).
pub(crate) fn stopped(request: &JmapMailRequest) -> JmapMailResult {
    JmapMailResult { account: request.account, email: request.email.clone(), session: request.session.clone(), stopped: true, ..Default::default() }
}

/// What is said, in place of the client's line, of a connection that failed because the server
/// sent it on: a page sees no redirect's target, so it follows none, the one a server may answer
/// its `/.well-known/jmap` with (RFC 8620 §2.2) included. `None` for any other failure.
pub(crate) fn redirect_line(error: &Error, downloading: bool) -> Option<&'static str> {
    if !matches!(error, Error::Transport(TransportError::RedirectRefused)) {
        return None;
    }
    Some(if downloading {
        "Couldn't download the messages from the server: it sent the request on to another address, which MailCraft in a browser can't follow. \
         Give the address of the JMAP session itself, not the server's name or its /.well-known/jmap."
    } else {
        "Couldn't reach the server: it sent the request on to another address, which MailCraft in a browser can't follow. \
         Give the address of the JMAP session itself, not the server's name or its /.well-known/jmap."
    })
}
