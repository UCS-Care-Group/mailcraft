// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's API, for the one thing a Gmail account's mail needs that IMAP can't do: move a message
//! of its Inbox to another of Gmail's categories (#911). Everything else of a Gmail account goes
//! over IMAP and SMTP.
//!
//! [`batch_modify`] is `POST https://gmail.googleapis.com/gmail/v1/users/me/messages/batchModify`
//! with `{"ids": [...], "addLabelIds": [the category's label], "removeLabelIds": [the other
//! four]}`, as Google's Gmail API reference has it: each message is named by Gmail's own id of it
//! (`X-GM-MSGID`, in hex), and nothing else of it or of the owner goes. The token is the one the account signs in
//! to Gmail's IMAP server with (its `https://mail.google.com/` permission covers the API).
//!
//! A Google token goes to Gmail's API host, `gmail.googleapis.com`, for this call alone, over TLS
//! and nowhere else ([`may_go_to`]): the IMAP and SMTP check ([`crate::google_destination`]) is
//! not widened for it. Redirects are not followed. What comes back is read only for its status,
//! at most [`MAX_ANSWER`] of it, and never shown: an error says what was being done and the
//! status, and never holds the token, the id or a word of Gmail's answer.
//!
//! Nothing here decides to move anything: a host calls it only for a move the owner made, and
//! only while moves between Gmail's categories are switched on (`mailcraft-engine`).

use std::time::Duration;

use mailcraft_model::GmailCategory;
use mailcraft_proto::ProtoError;
use serde_json::json;

use crate::NetError;
use crate::http::{self, Request, Url};

/// Where Gmail's API is.
const GMAIL_API: &str = "https://gmail.googleapis.com";
/// Its host, the one place a Google token goes besides Gmail's IMAP and SMTP servers.
const GMAIL_API_HOST: &str = "gmail.googleapis.com";
/// How long Gmail's API has to be reached and to answer one request.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest answer read (a batch is answered with nothing; an error, with a few lines).
const MAX_ANSWER: usize = 64 * 1024;
/// The longest token shown to it.
const MAX_TOKEN: usize = 16 * 1024;

/// Where Gmail's API is spoken to: its own address, always. (Only a test may have a stand-in on
/// this machine answer in its place, on its own thread: `standin::route`.)
fn api_base() -> String {
    #[cfg(any(test, feature = "stand-in"))]
    if let Some(base) = standin::routed() {
        return base;
    }
    GMAIL_API.to_string()
}

/// Whether moves between Gmail's categories are switched on in this build
/// ([`mailcraft_model::gmail::GMAIL_CATEGORY_MOVES`]): held to here whatever a request asks, so
/// that while they are off nothing goes to Gmail's API. (A test's stand-in may switch them on, on
/// its own thread: `standin::route`.)
pub fn switched_on() -> bool {
    #[cfg(any(test, feature = "stand-in"))]
    if standin::switched_on() {
        return true;
    }
    mailcraft_model::gmail::GMAIL_CATEGORY_MOVES
}

/// Move the messages Gmail knows as `ids` (`X-GM-MSGID`) to `category`, as the owner of `token`
/// (the access token of an account that signs in with Google): one request, at most
/// [`MAX_BATCH`] of them. No ids, nothing sent.
///
/// Gmail answers a batch as a whole (204, nothing about each message), so a message it no longer
/// has can't be told from one it moved: the next reading of the Inbox sets either right here.
///
/// What can go wrong, for the host to act on:
/// - `ProtoError::Auth` ([`NetError::is_refused_sign_in`]): Gmail didn't take the token (401).
/// - [`NetError::Forbidden`]: the token is good and doesn't let this happen (403).
/// - [`NetError::Busy`]: not now (429, 5xx). Ask again later; nothing here waits.
/// - `ProtoError::Rejected`: Gmail refused these moves (any other status).
/// - [`NetError::Config`]: nothing was sent (switched off, an id missing, too many ids, no
///   token, a token that can't be shown).
/// - Anything else: Gmail's API couldn't be reached, or its answer made no sense.
pub fn batch_modify(token: &str, ids: &[u64], category: GmailCategory) -> Result<(), NetError> {
    batch_modify_at(&api_base(), token, ids, category)
}

/// The most messages one request moves (Gmail's own limit for `batchModify`).
pub const MAX_BATCH: usize = 1000;

/// [`batch_modify`] against the API at `base`.
fn batch_modify_at(base: &str, token: &str, ids: &[u64], category: GmailCategory) -> Result<(), NetError> {
    if !switched_on() {
        return Err(NetError::Config("moving mail between Gmail's categories comes once Google has reviewed MailCraft".into()));
    }
    if ids.is_empty() {
        return Ok(());
    }
    if ids.contains(&0) {
        return Err(NetError::Config("Gmail hasn't said its id of a message, so it can't be moved between Gmail's categories yet".into()));
    }
    if ids.len() > MAX_BATCH {
        return Err(NetError::Config(format!("at most {MAX_BATCH} messages are moved between Gmail's categories at a time").into()));
    }
    // A token that isn't one is not sent anywhere; it is also what a header can't carry.
    if token.is_empty() || token.len() > MAX_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Proto(ProtoError::Auth("there is no Google sign-in to show Gmail".into())));
    }
    let base = Url::parse(base)?;
    may_go_to(&base)?;
    let url = Url { path: "/gmail/v1/users/me/messages/batchModify".into(), ..base };
    let hex: Vec<String> = ids.iter().map(|id| format!("{id:x}")).collect();
    let removed: Vec<&str> = GmailCategory::ALL.iter().filter(|c| **c != category).map(|c| c.api_label()).collect();
    let body = json!({"ids": hex, "addLabelIds": [category.api_label()], "removeLabelIds": removed}).to_string();
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", "application/json"), ("Accept", "application/json")];
    let reply =
        http::request(&url, &Request { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience: TIMEOUT, max_body: MAX_ANSWER })
            .map_err(|e| hidden(e, token))?;
    let status = reply.status;
    match status {
        200..=299 => Ok(()),
        401 => Err(NetError::Proto(ProtoError::Auth("Gmail didn't take the sign-in".into()))),
        403 => Err(NetError::Forbidden("this account may not move mail between Gmail's categories".into())),
        429 | 500..=599 => Err(NetError::Busy { text: "Gmail is busy; try again later".into(), retry_after: None }),
        _ => Err(NetError::Proto(ProtoError::Rejected(format!("Gmail answered with status {status}").into()))),
    }
}

/// Whether a Google token may go to `url`: Gmail's API host, over TLS, on its usual port, and no
/// other place. (A test's stand-in on this machine, unencrypted, in a test's build alone.)
pub(crate) fn may_go_to(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.');
    if url.secure && url.port == 0 && host.eq_ignore_ascii_case(GMAIL_API_HOST) {
        return Ok(());
    }
    #[cfg(any(test, feature = "stand-in"))]
    if !url.secure && standin::routed().is_some() && crate::transport::is_loopback(host) {
        return Ok(());
    }
    Err(NetError::Config("a Google sign-in is only used with Gmail's own servers: this isn't Gmail's API".into()))
}

/// An error on the way, with the token never in its words (a server that repeats what it was
/// sent can't put it in an error).
fn hidden(e: NetError, token: &str) -> NetError {
    if e.to_string().contains(token) { NetError::Proto(ProtoError::Io("Gmail's API couldn't be reached".into())) } else { e }
}

#[cfg(any(test, feature = "stand-in"))]
pub mod standin;

#[cfg(test)]
mod tests;
