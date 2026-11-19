// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in (`AuthRequest` in `urn:zimbraAccount`): the account's name and password go once, in
//! the request's body over TLS, and the server answers with a token that the run's other requests
//! carry in their SOAP header. The token is held in a [`Secret`] for the length of the run and
//! never written anywhere.

use serde_json::json;

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::json;
use crate::soap;

/// Longest token taken from the server.
const MAX_TOKEN: usize = 4096;

/// The request that signs `user` in with `password` at the server whose web client is at `base`.
pub fn request(base: &str, user: &str, password: &Secret) -> Result<HttpRequest, Error> {
    let user: String = user.chars().filter(|c| !c.is_control()).collect();
    let body = json!({"account": {"by": "name", "_content": user.trim()}, "password": {"_content": password.expose()}});
    soap::request(base, "AuthRequest", soap::ACCOUNT, body, None)
}

/// The token the server answered with. An account that signs in with a second step says so
/// ([`Error::TwoFactor`]) rather than handing a token over; a token that is not one (empty, too
/// long, or with a character no token has) is [`Error::Unreadable`].
pub fn answer(response: &HttpResponse) -> Result<Secret, Error> {
    let answer = soap::answer("AuthRequest", response)?;
    // A flag that is no flag can't tell a second step is asked for: read off, the token it came
    // without would be handed out while the account is asked for more.
    if json::checked_on(answer.get("twoFactorAuthRequired")).ok_or(Error::Unreadable)? {
        return Err(Error::TwoFactor);
    }
    let token = json::raw(answer.get("authToken")).unwrap_or_default();
    let fit = !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.=".contains(&b));
    if fit { Ok(Secret::new(&token)) } else { Err(Error::Unreadable) }
}
