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
    /// A redirect (301, 302, 307, 308) to `Location`; never followed here.
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
        c @ (301 | 302 | 307 | 308) => Verdict::Redirect(c),
        c => Verdict::Other(c),
    }
}

/// Whether an error body names the `valid-sync-token` precondition (RFC 6578 §3.2). Only the
/// first 64 KB are looked at, and as text.
fn names_valid_sync_token(body: &[u8]) -> bool {
    let head = body.get(..body.len().min(64 * 1024)).unwrap_or(&[]);
    String::from_utf8_lossy(head).contains("valid-sync-token")
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

/// An IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) as Unix seconds.
pub fn http_date(text: &str) -> Option<i64> {
    let mut words = text.split_whitespace().skip(1);
    let day: i64 = words.next()?.parse().ok()?;
    let name = words.next()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| *m == name)? as i64 + 1;
    let year: i64 = words.next()?.parse().ok()?;
    let mut clock = words.next()?.split(':');
    let (h, m, s): (i64, i64, i64) = (clock.next()?.parse().ok()?, clock.next()?.parse().ok()?, clock.next()?.parse().ok()?);
    if words.next()? != "GMT" || !(1..=31).contains(&day) || !(1970..=9999).contains(&year) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm, public domain).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + m * 60 + s)
}

/// The authentication schemes a 401 offers (`Basic`, `Digest`, `Bearer`…), lower-cased.
pub fn challenges(response: &HttpResponse) -> Vec<String> {
    response.headers_named("www-authenticate").flat_map(schemes).take(16).collect()
}

/// The compliance classes a `DAV` header names (RFC 4918 §10.1), lower-cased.
pub fn dav_classes(response: &HttpResponse) -> Vec<String> {
    response.headers_named("dav").flat_map(|v| v.split(',')).map(|c| c.trim().to_ascii_lowercase()).filter(|c| !c.is_empty()).take(32).collect()
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
