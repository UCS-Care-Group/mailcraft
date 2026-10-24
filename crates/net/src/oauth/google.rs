// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google Desktop OAuth for Gmail IMAP/SMTP (issue #232).
//! Reuses the parent's hardened PKCE, state, loopback listener and token parsing.
//! No web Gmail API, calendar, contacts, or account persistence is introduced here.

use super::*;

const GOOGLE_DESKTOP_ID: &str = match option_env!("MAILCRAFT_GOOGLE_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// The OS registration's desktop "secret" is public to installed clients. It is not a user's
/// credential. Pass it explicitly for File > Options overrides and synthetic tests.
fn registration<'a>(oauth: &'a OAuth, secret: &'a str) -> Result<(&'a str, &'a str), NetError> {
    if oauth.provider != OAuthProvider::Google {
        return Err(NetError::Config("this isn't a Google account".into()));
    }
    // The maintainer's desktop client ID comes from .cargo/config.toml unless this
    // installation explicitly supplied an override (e.g. File > Options).
    let id = if oauth.client_id.trim().is_empty() && !GOOGLE_DESKTOP_ID.is_empty() { GOOGLE_DESKTOP_ID } else { client_id(oauth)? };
    if id.len() > 200 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Config("MailCraft's Google Desktop OAuth client ID is invalid".into()));
    }
    if secret.is_empty() {
        return Err(NetError::Config("MailCraft's Google Desktop OAuth client secret is not configured".into()));
    }
    if secret.len() > 512 || !secret.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Config("MailCraft's Google Desktop OAuth client secret is invalid".into()));
    }
    Ok((id, secret))
}

/// The authorization page: Google needs both an offline request and a fresh consent in order
/// to issue a refresh token; neither is sent to Microsoft's page.
fn authorization_page(endpoints: &Endpoints, id: &str, redirect: &str, state: &str, verifier: &str, login_hint: &str) -> String {
    let mut fields = vec![
        ("client_id", id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", endpoints.scope.as_str()),
        ("state", state),
        ("code_challenge", verifier),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent select_account"),
    ];
    let hint = login_hint.trim();
    if !hint.is_empty() && hint.len() <= 320 && !hint.chars().any(char::is_control) {
        fields.push(("login_hint", hint));
    }
    let address = endpoints.authorize.trim();
    format!("{address}{}{}", if address.contains('?') { '&' } else { '?' }, http::form(&fields))
}

/// Google on explicit endpoints. In tests the token server is an in-process stand-in and
/// open simulates the system browser. The client registration never comes from another app.
pub fn sign_in_google_at(
    endpoints: &Endpoints,
    oauth: &OAuth,
    client_secret: &str,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<Tokens, NetError> {
    sign_in_google_refused(endpoints, oauth, client_secret, login_hint, open, cancel, SIGN_IN_TIMEOUT).map_err(|refused| refused.error)
}

pub(super) fn sign_in_google_refused(
    endpoints: &Endpoints,
    oauth: &OAuth,
    client_secret: &str,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, Refused> {
    let (id, secret) = registration(oauth, client_secret)?;
    Url::parse(&endpoints.authorize)?;
    let endpoint = Url::parse(&endpoints.token)?;
    if !endpoints.scope.split_ascii_whitespace().any(|s| s == "https://mail.google.com/") {
        return Err(NetError::Config("Google sign-in needs the Gmail IMAP/SMTP scope".into()).into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    let redirect = Redirect::listen()?;
    // RFC 8252: Google's native client redirects directly to the IPv4 loopback address.
    let redirect_uri = format!("http://127.0.0.1:{}", redirect.port);
    let verifier = random(32)?;
    let state = random(24)?;
    open(&authorization_page(endpoints, id, &redirect_uri, &state, &challenge(&verifier), login_hint));
    let code = redirect.wait(&state, cancel, patience)?;
    drop(redirect);
    let fields = [
        ("grant_type", "authorization_code"),
        ("client_id", id),
        ("client_secret", secret),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    exchange(&endpoint, &fields, "", &[code.as_str(), verifier.as_str(), secret], true)
}

/// Renew a Google mail access token; the refresh grant stays scoped to the same account.
/// No refresh token, old or new, is written to disk by this module.
pub fn refresh_google_at(endpoints: &Endpoints, oauth: &OAuth, client_secret: &str, refresh_token: &str) -> Result<Tokens, NetError> {
    let (id, secret) = registration(oauth, client_secret)?;
    let endpoint = Url::parse(&endpoints.token)?;
    if refresh_token.is_empty() || !usable(refresh_token) {
        return Err(NetError::Proto(ProtoError::Auth("there is no earlier sign-in to renew".into())));
    }
    let fields = [("grant_type", "refresh_token"), ("client_id", id), ("client_secret", secret), ("refresh_token", refresh_token)];
    exchange(&endpoint, &fields, refresh_token, &[refresh_token, secret], false).map_err(|refused| refused.error)
}

/// Token responses are hostile. An access token without the mail scope is useless, not success.
/// On initial sign-in we require the identity token's email so that a browser choosing a
/// different account cannot be silently treated as the requested one.
fn exchange(endpoint: &Url, fields: &[(&str, &str)], previous: &str, secrets: &[&str], identify: bool) -> Result<Tokens, Refused> {
    let answer =
        http::post_form(endpoint, fields, TOKEN_TIMEOUT).map_err(|error| secrets.iter().fold(error, |error, secret| error.without(secret)))?;
    let tokens = tokens_from(&answer, previous, mailcraft_time::now(), secrets)?;
    let scopes = serde_json::from_slice::<Value>(&answer.body).ok().and_then(|data| data.get("scope").and_then(Value::as_str).map(str::to_string));
    // OAuth 2.0 permits scope omission when the grant matches what was requested (RFC 6749
    // section 5.1). A returned narrower scope must explicitly include Gmail access.
    if scopes.as_deref().is_some_and(|scope| !scope.split_ascii_whitespace().any(|part| part == "https://mail.google.com/")) {
        return Err(NetError::Proto(ProtoError::Auth("Google did not grant Gmail IMAP/SMTP access".into())).into());
    }
    if identify && tokens.email.is_empty() {
        return Err(NetError::Proto(ProtoError::Auth("Google did not identify the signed-in email address".into())).into());
    }
    Ok(tokens)
}
