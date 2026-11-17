// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's token endpoint, as the Microsoft identity platform documents it for a public
//! client (RFC 6749 §4.1.3 and §6, with PKCE, RFC 7636): `POST /{tenant}/oauth2/v2.0/token`, a
//! form of `grant_type` `authorization_code` (with a `code` and a `code_verifier`) or
//! `refresh_token` (with a `refresh_token`), and a `client_id`. It answers with the token the
//! stand-in takes, a refresh token of its own and an identity token that names the owner; a form
//! that isn't one of the two is refused as the endpoint refuses (`{"error", "error_description"}`,
//! 400). Nobody's sign-in is looked at: whatever code and refresh token come are taken.
//!
//! It is for a client that is pointed at the stand-in as a whole (the phone's end-to-end run,
//! [`super::route_this_process`]); the sign-in page itself is the test's to play.

use mailcraft_charset::base64::URL_SAFE_NO_PAD;
use serde_json::json;

use super::{Answer, Asked, State};

/// The refresh token it gives (a made-up word: nothing is signed in).
pub const REFRESH: &str = "standin.refresh-0123456789";
/// The directory it names in the identity token (a GUID of the documentation's own shape).
const TENANT_ID: &str = "00000000-0000-4000-8000-000000000e2e";
/// How long the tokens it gives are good for (seconds), as Microsoft's are.
const LIFETIME: u64 = 3_599;

/// Whether `path` is the token endpoint's: `/{tenant}/oauth2/v2.0/token`.
pub(super) fn is_token_endpoint(path: &[String]) -> bool {
    matches!(path, [_, oauth, version, token] if oauth == "oauth2" && version == "v2.0" && token == "token")
}

/// The value of `name` in a form (`application/x-www-form-urlencoded`), its `%XX` and `+` undone.
fn field(form: &str, name: &str) -> Option<String> {
    form.split('&').filter_map(|pair| pair.split_once('=')).find(|(key, _)| *key == name).map(|(_, value)| unescape(value))
}

fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|pair| std::str::from_utf8(pair).ok()).and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (byte, hex) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (other, _) => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A refusal as the token endpoint words one.
fn refused(error: &str, description: &str) -> Answer {
    (400, Some(json!({"error": error, "error_description": description})))
}

impl State {
    /// `POST /{tenant}/oauth2/v2.0/token`.
    pub(super) fn token(&self, asked: &Asked) -> Answer {
        if asked.method != "POST" {
            return refused("invalid_request", "The token endpoint takes a POST.");
        }
        if !asked.header("content-type").to_ascii_lowercase().starts_with("application/x-www-form-urlencoded") {
            return refused("invalid_request", "The request body must be a form.");
        }
        let form = asked.body.as_str();
        if field(form, "client_id").is_none_or(|id| id.trim().is_empty()) {
            return refused("invalid_request", "The request body must contain the 'client_id' parameter.");
        }
        let given = |name: &str| field(form, name).is_some_and(|value| !value.is_empty());
        match field(form, "grant_type").as_deref() {
            Some("authorization_code") if given("code") && given("code_verifier") => {}
            Some("authorization_code") => return refused("invalid_grant", "The code and its verifier are both required."),
            Some("refresh_token") if given("refresh_token") => {}
            Some("refresh_token") => return refused("invalid_grant", "The refresh token is required."),
            _ => return refused("unsupported_grant_type", "The grant type is not one this endpoint takes."),
        }
        let claims = json!({"email": self.owner, "name": "Stand-in Owner", "tid": TENANT_ID});
        let id_token = format!("e30.{}.c3RhbmQtaW4", URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes()));
        let scope = field(form, "scope").unwrap_or_default();
        (
            200,
            Some(json!({
                "token_type": "Bearer",
                "scope": scope,
                "expires_in": LIFETIME,
                "access_token": self.token,
                "refresh_token": REFRESH,
                "id_token": id_token,
            })),
        )
    }
}

#[cfg(test)]
mod tests;
