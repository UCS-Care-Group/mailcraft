// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message whole, as the RFC 5322 text it is: from its REST address
//! (`<server>/service/home/~/?id=<item>&auth=co`), with the sign-in token in the `ZM_AUTH_TOKEN`
//! cookie, the one way the REST service takes it but in the address, where it would be written to
//! the server's logs.

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Method, RESPONSE_LIMIT, Secret};

/// The request for message `id` (an own id) whole.
pub fn request(base: &str, token: &Secret, id: &str) -> Result<HttpRequest, Error> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
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
        429 | 503 => Err(Error::Busy(response.retry_after().unwrap_or(30).clamp(1, 3600))),
        status => Err(Error::Status(status)),
    }
}
