// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message whole, as the RFC 5322 text it is: from its REST address
//! (`<server>/service/home/~/?id=<item>&auth=co`), with the sign-in token in the `ZM_AUTH_TOKEN`
//! cookie, the one way the REST service takes it but in the address, where it would be written to
//! the server's logs.

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Method, RESPONSE_LIMIT, Secret};

/// The request for message `id` whole: an own id, or one of a mailbox mounted into this one's
/// tree (`<account id>:<item id>`, as the REST service takes them).
pub fn request(base: &str, token: &Secret, id: &str) -> Result<HttpRequest, Error> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let ok = match id.split_once(':') {
        None => digits(id),
        Some((zid, rid)) => !zid.is_empty() && zid.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)) && digits(rid),
    };
    if !ok {
        return Err(Error::NoSuchItem);
    }
    let url = format!("{}/service/home/~/?id={id}&auth=co", base.trim_end_matches('/'));
    let cookie = Secret::new(&format!("ZM_AUTH_TOKEN={}", token.expose()));
    let fields = [("Cookie", cookie.expose()), ("Accept", "message/rfc822")];
    Ok(HttpRequest::new(Method::Get, &url, &fields, Vec::new())?.with_response_limit(RESPONSE_LIMIT))
}

/// The message's text, or what went wrong.
pub fn answer(response: &HttpResponse) -> Result<Vec<u8>, Error> {
    match response.status() {
        200 if !response.body().is_empty() => Ok(response.body().to_vec()),
        200 => Err(Error::Unreadable),
        401 | 403 => Err(Error::AuthRefused),
        404 => Err(Error::NoSuchItem),
        429 | 503 => Err(Error::Busy(response.retry_after_seconds().unwrap_or(30).clamp(1, 3600))),
        status => Err(Error::Status(status)),
    }
}
