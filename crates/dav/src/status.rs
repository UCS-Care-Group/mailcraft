// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a response's status means to a DAV client (RFC 4918, 4791, 6578, 9110 §13/§15).

use crate::answer::{AnswerError, Multistatus};
use crate::href::Origin;
use crate::http::HttpResponse;

/// The longest wait a `Retry-After` is obeyed for, in seconds.
pub const MAX_WAIT: u64 = 600;

/// What a response means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 207: a multistatus follows.
    Multistatus,
    /// 200, 201 or 204: done; the response may carry an `ETag`.
    Done,
    /// 304: nothing changed.
    NotModified,
    /// 401: the credential was not accepted or none was given. On its own it proves nothing
    /// about what the server is (see [`is_dav_evidence`]).
    Unauthorized,
    /// 403 or 409 with `valid-sync-token`: the sync token is not known any more; read afresh.
    SyncTokenInvalid,
    /// 403 otherwise: the server refuses this.
    Forbidden,
    /// 404 or 410: not there.
    Gone,
    /// 412: the resource changed (`If-Match`) or exists (`If-None-Match: *`).
    PreconditionFailed,
    /// 428: the server wants a precondition this client did not send.
    PreconditionRequired,
    /// 429 or 503: wait this many seconds (at most [`MAX_WAIT`], 60 when the server says none).
    Wait(u64),
    /// 507 as the status of the whole response: the server is out of room. (RFC 6578 truncation
    /// is a 507 response element inside a 207, which [`crate::answer::Multistatus::truncated`] says.)
    InsufficientStorage,
    /// A redirect (301, 302, 303, 307, 308) to `Location`; never followed here.
    Redirect(u16),
    /// Any other code.
    Other(u16),
}

/// Classifies `response`. `now` (Unix seconds) is read only to turn an HTTP-date `Retry-After`
/// into a wait.
pub fn classify(response: &HttpResponse, now: i64) -> Verdict {
    match response.status() {
        207 => Verdict::Multistatus,
        200 | 201 | 204 => Verdict::Done,
        304 => Verdict::NotModified,
        401 => Verdict::Unauthorized,
        403 | 409 if names_valid_sync_token(response.body()) => Verdict::SyncTokenInvalid,
        403 => Verdict::Forbidden,
        404 | 410 => Verdict::Gone,
        412 => Verdict::PreconditionFailed,
        428 => Verdict::PreconditionRequired,
        429 | 503 => Verdict::Wait(retry_after(response.header("retry-after"), now)),
        507 => Verdict::InsufficientStorage,
        c @ (301 | 302 | 303 | 307 | 308) => Verdict::Redirect(c),
        c => Verdict::Other(c),
    }
}

/// Whether an error body is a `DAV:error` holding `DAV:valid-sync-token` (RFC 6578 §3.2),
/// parsed as XML under the usual budgets. (The request is not known here: a 403 or 409 that names
/// this precondition can only be an answer to a sync.)
fn names_valid_sync_token(body: &[u8]) -> bool {
    crate::xml::parse_document(body).is_ok_and(|root| root.is(crate::xml::DAV, "error") && root.child(crate::xml::DAV, "valid-sync-token").is_some())
}

/// The seconds a `Retry-After` asks for (RFC 9110 §10.2.3: a number, or an HTTP-date), at most
/// [`MAX_WAIT`]; 60 when it is missing or not understood.
pub fn retry_after(value: Option<&str>, now: i64) -> u64 {
    let Some(v) = value.map(str::trim) else { return 60 };
    if !v.is_empty() && v.len() <= 10 && v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse::<u64>().map_or(60, |n| n.min(MAX_WAIT));
    }
    match http_date(v) {
        Some(at) => u64::try_from(at.saturating_sub(now)).unwrap_or(0).min(MAX_WAIT),
        None => 60,
    }
}

/// Exactly `min..=max` ASCII digits, no sign, as a number.
fn digits(text: &str, min: usize, max: usize) -> Option<i64> {
    if text.len() < min || text.len() > max || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// An IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) as Unix seconds. Every number is unsigned
/// digits of the length the format has, and the arithmetic is checked.
pub fn http_date(text: &str) -> Option<i64> {
    let mut words = text.split_whitespace().skip(1);
    let day = digits(words.next()?, 1, 2)?;
    let name = words.next()?;
    let month = i64::try_from(["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| *m == name)?)
        .ok()?
        .checked_add(1)?;
    let year = digits(words.next()?, 4, 4)?;
    let mut clock = words.next()?.split(':');
    let (h, m, s) = (digits(clock.next()?, 2, 2)?, digits(clock.next()?, 2, 2)?, digits(clock.next()?, 2, 2)?);
    if clock.next().is_some()
        || words.next()? != "GMT"
        || words.next().is_some()
        || !(1..=31).contains(&day)
        || year < 1970
        || h > 23
        || m > 59
        || s > 60
    {
        return None;
    }
    civil_seconds(year, month, day, h * 3_600 + m * 60 + s)
}

/// Unix seconds of a proleptic Gregorian date and a time of day in seconds, checked.
pub(crate) fn civil_seconds(year: i64, month: i64, day: i64, clock: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm, public domain).
    let y = if month <= 2 { year.checked_sub(1)? } else { year };
    let era = y.div_euclid(400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe.checked_mul(365)?.checked_add(yoe / 4)?.checked_sub(yoe / 100)?.checked_add(doy)?;
    let days = era.checked_mul(146_097)?.checked_add(doe)?.checked_sub(719_468)?;
    days.checked_mul(86_400)?.checked_add(clock)
}

/// The authentication schemes a 401 offers (`Basic`, `Digest`, `Bearer`…), lower-cased.
pub fn challenges(response: &HttpResponse) -> Vec<String> {
    response.headers_named("www-authenticate").flat_map(schemes).take(16).collect()
}

/// Most compliance classes read from a response's `DAV` headers: the cap `mailcraft_net` keeps them
/// at, so that a class it kept is a class that is read.
// Must match `MAX_DAV` in `mailcraft_net::http` (64) and its 96-character class cap: a test in
// `mailcraft_net` (`dav_tests`) holds the two together.
pub const MAX_DAV_CLASSES: usize = 64;

/// The compliance classes a `DAV` header names (RFC 4918 §10.1), lower-cased: the comma-separated
/// list of `compliance-class = "1" | "2" | "3" | extend`, an extension being a token or a
/// `<Coded-URL>` (RFC 4918 §10.1), in which a comma is part of the URL.
pub fn dav_classes(response: &HttpResponse) -> Vec<String> {
    response
        .headers_named("dav")
        .flat_map(split_classes)
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .take(MAX_DAV_CLASSES)
        .collect()
}

/// `value` cut at the commas that are outside `<…>`.
fn split_classes(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (false, 0);
    for (at, c) in value.char_indices() {
        match c {
            '<' => depth = true,
            '>' => depth = false,
            ',' if !depth => {
                parts.push(value.get(start..at).unwrap_or(""));
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(value.get(start..).unwrap_or(""));
    parts
}

/// Whether a probe's response is evidence that a CalDAV or CardDAV server answered: a 207 whose
/// body is a `DAV:` multistatus, or a 401 **and** a `DAV` header naming `calendar-access` or
/// `addressbook`. A bare 401 is never evidence: a mail filter's web front answers 401 to
/// everything, and the password would go to it.
pub fn is_dav_evidence(response: &HttpResponse, origin: &Origin, base: &str) -> bool {
    match response.status() {
        207 => Multistatus::parse(response.body(), origin, base).is_ok(),
        401 => dav_classes(response).iter().any(|c| c == "calendar-access" || c == "addressbook"),
        _ => false,
    }
}

/// Reads a 207 answer; a convenience over [`Multistatus::parse`] that insists on the status.
pub fn multistatus(response: &HttpResponse, origin: &Origin, base: &str) -> Result<Multistatus, AnswerError> {
    if response.status() != 207 {
        return Err(AnswerError::Invalid);
    }
    Multistatus::parse(response.body(), origin, base)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;

/// Every scheme in one `WWW-Authenticate` value (RFC 9110 §11.6.1: a value may hold several
/// challenges): a token that starts a challenge is one followed by a space and then an auth-param
/// or a token68, not the parameters inside it. Quoted strings are skipped.
fn schemes(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut part = String::new();
    let mut parts = Vec::new();
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                part.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut part)),
            _ => part.push(c),
        }
    }
    parts.push(part);
    for p in parts {
        let p = p.trim();
        let mut words = p.splitn(2, ' ');
        let first = words.next().unwrap_or("");
        let rest = words.next().unwrap_or("").trim_start();
        // `Basic realm="x"` starts a challenge; `realm="x"` (a parameter) does not; a bare
        // `Negotiate` does.
        if !first.is_empty() && !first.contains('=') && (rest.is_empty() || rest.contains('=') || !first.contains('"')) {
            out.push(first.to_ascii_lowercase());
        }
    }
    out
}
