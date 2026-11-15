// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file behind a subscribed calendar's address (Internet Calendars), RFC 5545 text served over
//! HTTPS (RFC 9110).
//!
//! The address is the owner's choice, and it may hold a secret (a private token in its path), so
//! [`fetch`] says as little as a request can:
//!
//! - only `https://` addresses; one with a user name in it, or `http://`, is refused, and so is a
//!   redirect to anything but `https://`;
//! - nothing that says who is asking: no cookie, no credentials, no referrer, the same plain user
//!   agent as everything else in this crate. What is added is `If-None-Match` and
//!   `If-Modified-Since` with the marks the server gave for the copy taken before (a mark is
//!   sent only to the host that gave it, and only when it is plain printable text);
//! - the host's name is looked up here and every address of it must be on the public internet,
//!   and the connection goes to the addresses that were checked (`pictures::is_public`), on each
//!   redirect too, of which at most [`MAX_REDIRECTS`] are followed;
//! - at most [`MAX_BYTES`] are taken (the redirects' answers counted in, and whatever the body
//!   unpacks to), within [`PATIENCE`]; a server that says "not now" (429, 503) is asked once more.
//!
//! It is all one machinery with [`crate::pictures`], which has the notes on how the connection is
//! made. What comes back is as hostile as any file: whether it is a calendar is for the reader to
//! find out.

use std::time::{Duration, Instant};

use mailcraft_model::subscription::{Failure, MAX_TEXT, SubscriptionOutcome, clean_validator};

use crate::pictures::{self, Limits, MAX_RETRY_WAIT, PictureError, Reach, Sent};

/// Largest file taken, in bytes.
pub const MAX_BYTES: usize = MAX_TEXT;
/// Most redirects followed on the way to it.
pub const MAX_REDIRECTS: usize = pictures::MAX_REDIRECTS;
/// How long one look may take, from the host's name to the last byte, redirects included.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// What the server asks for: a calendar, and failing that anything that is text.
const ACCEPT: &str = "text/calendar, text/plain;q=0.5, */*;q=0.1";

/// What a look found.
#[derive(Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The file, as text (bytes that aren't UTF-8 are replaced), and the server's marks of it.
    Changed { text: String, etag: String, last_modified: String },
    /// The copy taken before is current (304).
    NotModified,
}

// (A calendar can be megabytes, and is somebody's private business.)
impl std::fmt::Debug for Fetched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetched::Changed { text, .. } => write!(f, "Changed({} bytes)", text.len()),
            Fetched::NotModified => f.write_str("NotModified"),
        }
    }
}

/// Look at the calendar at `address` (an address that `mailcraft_model::subscription::check_address`
/// let through), asking whether it changed since the copy that `etag` and `last_modified` mark
/// (empty for none). Blocks, for [`PATIENCE`] at most, so a host calls it on a background thread.
pub fn fetch(address: &str, etag: &str, last_modified: &str) -> Result<Fetched, Failure> {
    fetch_with(address, etag, last_modified, &Reach::web(), &Limits { max_bytes: MAX_BYTES, patience: PATIENCE, ..Limits::default() })
}

/// [`fetch`], as the outcome the engine takes in (`sync.applySubscription`). Never says anything
/// of the address.
pub fn look(address: &str, etag: &str, last_modified: &str) -> SubscriptionOutcome {
    match fetch(address, etag, last_modified) {
        Ok(Fetched::Changed { text, etag, last_modified }) => SubscriptionOutcome::Changed { text, etag, last_modified },
        Ok(Fetched::NotModified) => SubscriptionOutcome::NotModified,
        Err(failure) => {
            // (The kind of trouble, never the address.)
            log::info!("a subscribed calendar couldn't be updated: {}", failure.message());
            SubscriptionOutcome::Failed { failure: failure.code() }
        }
    }
}

fn failure(error: PictureError) -> Failure {
    match error {
        PictureError::Address => Failure::Address,
        PictureError::NotPublic => Failure::NotPublic,
        PictureError::TooManyRedirects => Failure::TooManyRedirects,
        PictureError::Status(status) => Failure::Status(status),
        PictureError::TooLarge => Failure::TooLarge,
        PictureError::TimedOut => Failure::TimedOut,
        PictureError::Packed => Failure::Packed,
        PictureError::Net(_) => Failure::Network,
    }
}

/// The header lines that ask "has it changed?": the marks, when they are fit to be sent.
fn conditions(etag: &str, last_modified: &str) -> String {
    let mut lines = String::new();
    let (etag, last_modified) = (clean_validator(etag), clean_validator(last_modified));
    if !etag.is_empty() {
        lines.push_str(&format!("If-None-Match: {etag}\r\n"));
    }
    if !last_modified.is_empty() {
        lines.push_str(&format!("If-Modified-Since: {last_modified}\r\n"));
    }
    lines
}

/// [`fetch`], with the reach and the limits said.
pub(crate) fn fetch_with(address: &str, etag: &str, last_modified: &str, reach: &Reach<'_>, limits: &Limits) -> Result<Fetched, Failure> {
    let deadline = Instant::now() + limits.patience;
    let first = pictures::address(address).map_err(|_| Failure::Address)?;
    if !first.secure {
        return Err(Failure::Address);
    }
    let marks = conditions(etag, last_modified);
    let mut at = first.clone();
    let mut left = limits.max_bytes;
    let (mut redirects, mut asked_again) = (0, false);
    loop {
        // The marks are the first host's: nobody else is told what it said about its file.
        let asking_back = !marks.is_empty() && at.host.eq_ignore_ascii_case(&first.host);
        let sent = Sent { accept: ACCEPT, extra: if asking_back { &marks } else { "" } };
        let answer = pictures::ask_with(&at, reach, limits, left, deadline, None, &sent).map_err(failure)?;
        match answer.status {
            200 => {
                let body = match &answer.said.coding {
                    Some(coding) => pictures::unpack(answer.body, coding, left).map_err(failure)?,
                    None => answer.body,
                };
                let text = String::from_utf8_lossy(&body).into_owned();
                let said = &answer.said;
                return Ok(Fetched::Changed {
                    text,
                    etag: said.etag.as_deref().map(clean_validator).unwrap_or_default(),
                    last_modified: said.last_modified.as_deref().map(clean_validator).unwrap_or_default(),
                });
            }
            // Only an answer to a question that was asked.
            304 if asking_back => return Ok(Fetched::NotModified),
            301 | 302 | 303 | 307 | 308 => {
                if redirects >= MAX_REDIRECTS {
                    return Err(Failure::TooManyRedirects);
                }
                redirects += 1;
                let location = answer.said.location.ok_or(Failure::Status(answer.status))?;
                at = pictures::follow(&at, &location).map_err(|_| Failure::Address)?;
                // Every step is `https://`, or the calendar isn't read.
                if !at.secure {
                    return Err(Failure::Address);
                }
                left = left.saturating_sub(answer.body.len());
            }
            status @ (429 | 503) if !asked_again => {
                asked_again = true;
                let wait = answer.said.retry_after.map_or(limits.retry_wait, Duration::from_secs).min(MAX_RETRY_WAIT);
                if deadline.saturating_duration_since(Instant::now()) <= wait {
                    return Err(Failure::Status(status));
                }
                std::thread::sleep(wait);
                left = left.saturating_sub(answer.body.len());
            }
            status => return Err(Failure::Status(status)),
        }
    }
}

#[cfg(test)]
#[path = "subscription_tests.rs"]
mod tests;
