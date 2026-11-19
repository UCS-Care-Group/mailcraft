// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Telling a Zimbra server from another HTTPS server, for account setup's detection: an
//! `AuthRequest` with nothing to sign in with is answered with a SOAP fault in Zimbra's own JSON
//! envelope — the refusal itself is the answer — where another web server answers an error page
//! or nothing at all. A probe carries no credential and asks for nothing of the mailbox's.

use serde_json::Value;

use crate::http::{HttpRequest, HttpResponse, Method, Secret};

/// The most characters a host name may be (a DNS name's).
const HOST: usize = 253;

/// Whether `host` is shaped like a DNS name: labels of letters, digits and dashes, neither
/// beginning nor ending in a dash, with a dot or more than one label not required (a local
/// network's names count).
fn host_ok(host: &str) -> bool {
    host.len() <= HOST
        && !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// The request that asks `host` whether it is a Zimbra server: an `AuthRequest` to its SOAP
/// address with nothing to sign in with. `None` when `host` is no usable name.
pub fn request(host: &str) -> Option<HttpRequest> {
    let host = host.trim().trim_end_matches('.');
    if !host_ok(host) {
        return None;
    }
    let url = format!("https://{host}/service/soap/AuthRequest");
    let body = crate::soap::envelope("AuthRequest", crate::soap::ACCOUNT, Value::Object(Default::default()), None::<&Secret>);
    let fields = [("Content-Type", "application/json; charset=utf-8"), ("Accept", "application/json")];
    HttpRequest::new(Method::Post, &url, &fields, body.into_bytes()).ok().map(|r| r.with_response_limit(16 * 1024))
}

/// Whether `response` is a Zimbra server's: a SOAP fault in Zimbra's JSON envelope (an
/// `AuthRequest` without a sign-in is always refused), whatever its status. Anything else — a
/// page, a bare error, a different service's JSON — is not.
pub fn is_zimbra(response: &HttpResponse) -> bool {
    let Ok(parsed) = serde_json::from_slice::<Value>(response.body()) else { return false };
    parsed.get("Body").and_then(|body| body.get("Fault")).is_some_and(Value::is_object)
}
