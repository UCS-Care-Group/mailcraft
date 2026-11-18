// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in (`AuthRequest` in `urn:zimbraAccount`): the account's name and password go once, in
//! the request's body over TLS, and the server answers with a token that the run's other requests
//! carry in their SOAP header. The token is held in a [`Secret`] for the length of the run and
//! never written anywhere.
//!
//! **A second step.** An account that signs in with two factors answers the password with
//! `twoFactorAuthRequired` and a token good for that second step alone; the code of the owner's
//! authenticator app then goes with that token (`twoFactorCode`). The request asks for this
//! computer to be trusted (`deviceTrusted`): the server answers with a trusted-device token and a
//! device id, which a later sign-in sends beside the password instead of a code. Both are secrets
//! the host keeps with the operating system's credential store, as the password, and nowhere else.

use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::json;
use crate::soap;

/// Longest token taken from the server.
const MAX_TOKEN: usize = 4096;
/// Longest device id taken.
const MAX_DEVICE: usize = 256;

/// What a sign-in goes with: the password, and for an account with a second step the code the
/// owner just typed, or the trusted-device token an earlier sign-in brought.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    pub password: Secret,
    /// The code of the owner's authenticator app, when they typed one for this sign-in.
    pub code: Option<Secret>,
    /// The trusted-device token an earlier sign-in with a code brought, with its device id.
    pub trusted: Option<(Secret, String)>,
    /// A sign-in token of an earlier sign-in this session that may still be good: tried first, so
    /// that a run after one the owner gave a code for asks for none again.
    pub session: Option<Secret>,
}

impl Credentials {
    /// A password alone.
    pub fn password(password: &str) -> Credentials {
        Credentials { password: Secret::new(password), ..Credentials::default() }
    }
}

/// What a sign-in brought besides the token: a trusted-device token and device id to keep, when
/// the server issued one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trust {
    pub token: Option<Secret>,
    pub device: String,
    /// How many milliseconds the sign-in token the server gave lives, when it said (`lifetime`).
    /// 0 for a kept token that was reused: its time left isn't known here.
    pub lifetime: u64,
}

/// What the server answered a sign-in with.
#[derive(Clone, Debug)]
pub enum Answer {
    /// Signed in: the token for the run's requests, and a trust to keep, if any.
    Token(Secret, Trust),
    /// The password was taken and a code is wanted: the token that goes with it.
    Code(Secret),
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// The request that signs `user` in with `credentials` at the server whose web client is at
/// `base`: the password, with the trusted-device token when there is one, and a code when one was
/// typed (and then this computer is asked to be trusted).
pub fn request(base: &str, user: &str, credentials: &Credentials) -> Result<HttpRequest, Error> {
    let mut body = Map::new();
    body.insert("account".into(), json!({"by": "name", "_content": clean(user)}));
    body.insert("password".into(), json!({"_content": credentials.password.expose()}));
    if let Some((token, device)) = &credentials.trusted {
        body.insert("trustedToken".into(), json!({"_content": token.expose()}));
        body.insert("deviceId".into(), json!({"_content": clean(device)}));
    }
    if let Some(code) = &credentials.code {
        body.insert("twoFactorCode".into(), json!({"_content": clean(code.expose())}));
        body.insert("deviceTrusted".into(), json!("1"));
        if credentials.trusted.is_none() {
            body.insert("generateDeviceId".into(), json!("1"));
        }
    }
    soap::request(base, "AuthRequest", soap::ACCOUNT, Value::Object(body), None)
}

/// The second step: `code` with the token the password brought (`first`), this computer asked to
/// be trusted.
pub fn second(base: &str, user: &str, first: &Secret, code: &Secret) -> Result<HttpRequest, Error> {
    let body = json!({
        "account": {"by": "name", "_content": clean(user)},
        "authToken": {"_content": first.expose()},
        "twoFactorCode": {"_content": clean(code.expose())},
        "deviceTrusted": "1",
        "generateDeviceId": "1",
    });
    soap::request(base, "AuthRequest", soap::ACCOUNT, body, None)
}

/// A token of the server's, when it is one: not empty, not too long, nothing a token hasn't.
fn token(value: Option<&Value>) -> Option<Secret> {
    let token = json::raw(value).unwrap_or_default();
    let fit = !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.=+/".contains(&b));
    fit.then(|| Secret::new(&token))
}

/// What the server answered a sign-in with. A token that is not one is [`Error::Unreadable`].
pub fn answer(response: &HttpResponse) -> Result<Answer, Error> {
    let answer = soap::answer("AuthRequest", response)?;
    let signed = token(answer.get("authToken")).ok_or(Error::Unreadable)?;
    // A flag that is no flag can't tell a second step is asked for: read off, the token it
    // came without would be handed out while the account is asked for more.
    if json::checked_on(answer.get("twoFactorAuthRequired")).ok_or(Error::Unreadable)? {
        return Ok(Answer::Code(signed));
    }
    let device = json::raw(answer.get("deviceId")).unwrap_or_default().trim().to_string();
    let lifetime = answer.get("lifetime").and_then(|v| v.get("_content").unwrap_or(v).as_u64()).unwrap_or(0);
    let trust = Trust { token: token(answer.get("trustedToken")), device: if device.len() <= MAX_DEVICE { device } else { String::new() }, lifetime };
    Ok(Answer::Token(signed, trust))
}

/// A cheap request a kept sign-in token is tried on (`GetInfoRequest`): a run that has one asks
/// this before it signs in afresh, so a token that is still good makes the sign-in needless.
pub fn verify(base: &str, token: &Secret) -> Result<HttpRequest, Error> {
    soap::request(base, "GetInfoRequest", soap::ACCOUNT, json!({"sections": "mbox"}), Some(token))
}

/// Whether `response` means the token it was asked with is still good.
pub fn verified(response: &HttpResponse) -> bool {
    soap::answer("GetInfoRequest", response).is_ok()
}
