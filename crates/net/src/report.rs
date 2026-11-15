// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sending a problem report to the project's report relay (#956), once the owner has pressed
//! Send report in Help › Report a Problem: one `POST` of the report as JSON, and its answer read.
//!
//! - **Only to the relay this build names** ([`relay`], `MAILCRAFT_REPORT_URL` when it was
//!   built; a build without one has no relay and sends nothing), over HTTPS (plain `http` only
//!   to this machine, for the tests). The crate's own HTTP client sends no cookie, credential or
//!   referrer, and its `User-Agent` says nothing of this machine: the report is all that goes.
//! - **Checked before it goes**: a report the relay would refuse (a part too long, a username
//!   that isn't one, the request over 64 KB) is refused here and sends nothing
//!   ([`ProblemReport::json`]).
//! - **The answer is hostile**: at most [`MAX_ANSWER`] bytes in [`PATIENCE`] twice over, no
//!   redirects, and read by [`answer`], which takes the relay's own answers and calls anything
//!   else garbled. Nothing of it is shown or logged but the reference.
//! - **Sent once.** Nothing here tries again: the window keeps the draft and the owner decides.

use std::time::Duration;

use mailcraft_model::report::{Field, NotSent, ProblemReport};
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Request, Url};

/// How long connecting may take, and so may the answer. The relay files an issue on GitHub
/// before it answers, which can take a few seconds.
pub const PATIENCE: Duration = Duration::from_secs(20);
/// Most bytes of the answer read: the relay's are a few dozen.
pub const MAX_ANSWER: usize = 4 * 1024;
/// The largest reference taken (a GitHub issue's number).
const MAX_REFERENCE: u64 = 1_000_000_000;

/// The relay this build sends reports to: `MAILCRAFT_REPORT_URL` as the build was given it, when
/// that is an `https://` address. `None` in a build without one (a fork's): such a build offers
/// Open on GitHub alone.
pub fn relay() -> Option<&'static str> {
    option_env!("MAILCRAFT_REPORT_URL").map(str::trim).filter(|url| is_relay(url))
}

/// Whether `url` will do as the relay: an `https://` address.
pub(crate) fn is_relay(url: &str) -> bool {
    url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) && Url::parse(url).is_ok()
}

/// Send `report` to this build's relay ([`relay`]). Blocks for [`PATIENCE`] twice over at most.
/// The reference the relay filed it under, or why it wasn't sent.
pub fn send(report: &ProblemReport) -> Result<u64, NotSent> {
    let Some(url) = relay() else { return Err(NotSent::Unreachable) };
    send_to(url, report)
}

/// [`send`] to the relay at `url` (an `https://` address, or `http://` on this machine).
pub fn send_to(url: &str, report: &ProblemReport) -> Result<u64, NotSent> {
    send_within(url, report, PATIENCE)
}

/// [`send_to`], waiting `patience` to connect and as long again for the answer.
pub(crate) fn send_within(url: &str, report: &ProblemReport, patience: Duration) -> Result<u64, NotSent> {
    let body = report.json().map_err(NotSent::Refused)?;
    let url = Url::parse(url).map_err(|_| NotSent::Unreachable)?;
    let headers = [("Content-Type", "application/json"), ("Accept", "application/json")];
    let request = Request { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience, max_body: MAX_ANSWER };
    match http::request(&url, &request) {
        Ok(reply) => {
            let got = answer(reply.status, &reply.body);
            // The reference alone: nothing of what was sent, nor of the answer.
            match &got {
                Ok(reference) => log::info!("problem report sent, reference R-{reference}"),
                Err(why) => log::info!("problem report not sent: {why:?}"),
            }
            got
        }
        Err(e) if http::is_too_slow(&e) => Err(NotSent::TooSlow),
        // No connection was made (the name, the address, TLS): nothing went.
        Err(NetError::Connect { .. } | NetError::Tls { .. } | NetError::Certificate(_) | NetError::Config(_)) => Err(NotSent::Unreachable),
        // Anything after the connection was made: the report may have gone, and that is said.
        Err(_) => Err(NotSent::Garbled),
    }
}

/// What the relay's answer says: the reference of a report it took (200 `{"reference": n}`),
/// or why it didn't. Anything that isn't one of its answers is [`NotSent::Garbled`]; it never
/// panics, whatever `body` holds.
pub fn answer(status: u16, body: &[u8]) -> Result<u64, NotSent> {
    let json: Option<Value> = if body.len() <= MAX_ANSWER { serde_json::from_slice(body).ok() } else { None };
    let object = json.as_ref().and_then(Value::as_object);
    let said = |k: &str| object.and_then(|o| o.get(k)).and_then(Value::as_str);
    match status {
        200 => match object.and_then(|o| o.get("reference")).and_then(Value::as_u64) {
            Some(n) if (1..=MAX_REFERENCE).contains(&n) => Ok(n),
            _ => Err(NotSent::Garbled),
        },
        400 if said("error") == Some("invalid") => Err(NotSent::Refused(said("field").and_then(Field::named).unwrap_or(Field::Request))),
        429 => Err(NotSent::RateLimited),
        503 if said("error") == Some("disabled") => Err(NotSent::Disabled),
        502 if said("error") == Some("failed") => Err(NotSent::Failed),
        500..=599 => Err(NotSent::Unavailable),
        _ => Err(NotSent::Garbled),
    }
}
