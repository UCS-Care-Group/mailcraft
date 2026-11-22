// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in with the owner's own identity provider ([`OAuthProvider::Custom`]): an OpenID
//! Connect or OAuth 2.0 server in front of their own IMAP and SMTP servers.
//!
//! The same protected sign-in as Microsoft's and Google's (authorization code with PKCE `S256`,
//! a random `state`, the system browser, a loopback redirect: RFC 6749, 7636, 8252), at the
//! addresses the account's settings name ([`OAuth::effective`]). The client is the one the
//! owner (or their administrator) registered there for MailCraft: a public client, or one with a
//! client secret, which the host hands in from the system's credential store and which goes to
//! the token and revocation addresses alone (RFC 8252 §8.5 says what such a secret is worth).
//! A client with a secret proves who it is in the `Authorization` header (`client_secret_basic`,
//! RFC 6749 §2.3.1), or in the form for a provider that takes only that; a public client names
//! itself in the form (§3.2.1). The revocation address is asked the same way (RFC 7009 §2.1).
//!
//! When the provider's page sends the owner back it may name itself (`iss`, RFC 9207): a name
//! other than the issuer's is refused, and so is none from a provider that says it names itself,
//! so that a code meant for one provider is never traded at another.
//!
//! The redirect is `http://127.0.0.1:{port}/`, the IP literal RFC 8252 §7.3 asks for, on any free
//! port or the one the settings name for a provider that can't register "any port".
//!
//! What the provider answers is hostile. Its refusals are told apart by their code alone, in
//! fixed words; the text it adds for people is never shown (it could say anything, or repeat a
//! secret).

use mailcraft_model::ClientAuth;

use super::*;

/// The fixed words for a provider's refusal, by its code (RFC 6749 §5.2, §4.1.2.1).
fn refused(code: &str) -> NetError {
    let (text, auth) = match code {
        "invalid_grant" => ("your identity provider no longer takes this sign-in: sign in again", true),
        "invalid_client" | "unauthorized_client" => {
            ("your identity provider doesn't know this client id, or its client secret: check the OAuth settings", true)
        }
        "invalid_scope" => ("your identity provider won't give what was asked for: check the extra scopes in the OAuth settings", true),
        "access_denied" => ("access was refused on the sign-in page", true),
        "server_error" | "temporarily_unavailable" => ("your identity provider can't sign anybody in just now: try again later", false),
        _ => ("your identity provider refused the sign-in", true),
    };
    NetError::Proto(if auth { ProtoError::Auth(text.into()) } else { ProtoError::Rejected(text.into()) })
}

/// The client id and the addresses a sign-in uses, when the settings are ready for one.
fn registration(oauth: &OAuth, client_secret: &str) -> Result<mailcraft_model::OAuthDefaults, NetError> {
    if oauth.provider != OAuthProvider::Custom {
        return Err(NetError::Config("this account doesn't sign in with your own identity provider".into()));
    }
    oauth.ready().map_err(|why| NetError::Config(why.to_string().into()))?;
    if client_secret.len() > 1024 || !client_secret.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Config("the client secret can only be letters, digits and punctuation".into()));
    }
    Ok(oauth.effective())
}

/// Sign the owner in on their identity provider's page and bring back tokens for the mail
/// servers. `client_secret` is empty for a public client. As for [`sign_in`]: `open` is given the
/// page's address, the wait is five minutes at most, and `cancel` ends it.
pub fn sign_in_custom(oauth: &OAuth, client_secret: &str, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, NetError> {
    sign_in_custom_refused(oauth, client_secret, login_hint, open, cancel, SIGN_IN_TIMEOUT).map_err(|refused| refused.error)
}

/// [`sign_in_custom`], saying how a sign-in that brought no tokens ended ([`SignInEnd`]).
pub fn sign_in_custom_explained(
    oauth: &OAuth,
    client_secret: &str,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<Tokens, SignInEnd> {
    sign_in_custom_refused(oauth, client_secret, login_hint, open, cancel, SIGN_IN_TIMEOUT).map_err(SignInEnd::from)
}

/// [`sign_in_custom`], with what a no was about and whose it was.
pub(super) fn sign_in_custom_refused(
    oauth: &OAuth,
    client_secret: &str,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, Refused> {
    let used = registration(oauth, client_secret)?;
    Url::parse(&used.authorize)?;
    let token_endpoint = Url::parse(&used.token)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    let redirect = match used.redirect_port {
        Some(port) => Redirect::listen_at(port)?,
        None => Redirect::listen()?,
    };
    let redirect_uri = format!("http://127.0.0.1:{}/", redirect.port);
    let verifier = random(32)?;
    let state = random(24)?;
    open(&page(&used, &redirect_uri, &state, &challenge(&verifier), login_hint));
    let (issuer, iss_required) = (used.issuer.clone(), used.issuer_in_response);
    let code = redirect.wait_for(&state, cancel, patience, |request, state| judge_issued(request, state, &issuer, iss_required))?;
    drop(redirect);
    let mut fields = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    let authorization = client_authentication(&used, client_secret, &mut fields);
    let secrets = [code.as_str(), verifier.as_str(), client_secret, credentials(&authorization)];
    trade_custom(&token_endpoint, &fields, authorization.as_deref(), "", &secrets).map_err(Refused::from)
}

/// How the client proves who it is to the token or revocation address (RFC 6749 §2.3.1, §3.2.1):
/// with a secret, the `Authorization` header to send (`client_secret_basic`: the id and secret
/// each form-encoded, joined by a colon, in base64), or the two in the form (`client_secret_post`);
/// without one, its id in the form. The id is never sent both ways (a provider may refuse that).
fn client_authentication<'a>(used: &'a mailcraft_model::OAuthDefaults, client_secret: &'a str, fields: &mut Vec<(&str, &'a str)>) -> Option<String> {
    if client_secret.is_empty() {
        fields.push(("client_id", used.client_id.as_str()));
        return None;
    }
    match used.client_auth {
        ClientAuth::Basic => {
            let pair = format!("{}:{}", http::encode(&used.client_id), http::encode(client_secret));
            Some(format!("Basic {}", mailcraft_charset::base64::STANDARD.encode(pair.as_bytes())))
        }
        ClientAuth::Post => {
            fields.push(("client_id", used.client_id.as_str()));
            fields.push(("client_secret", client_secret));
            None
        }
    }
}

/// The credentials in an `Authorization` header (the base64 after `Basic `), to be kept out of
/// errors as the secret is: a server that repeats them repeats the secret.
fn credentials(authorization: &Option<String>) -> &str {
    authorization.as_deref().map_or("", |header| header.strip_prefix("Basic ").unwrap_or(header))
}

/// [`judge`] for an identity provider that may name itself in its answer (RFC 9207 §2.4): the
/// answer, a code or a refusal, is taken only from `issuer` (when it is known), and only with
/// its name when `required`. Checked before anything else in it is believed.
fn judge_issued(request: &[u8], state: &str, issuer: &str, required: bool) -> (&'static Page, Option<Result<String, Refused>>) {
    let fields = match brought(request, state) {
        Ok(fields) => fields,
        Err(page) => return (page, None),
    };
    let named = fields.iter().find(|(name, _)| name == "iss").map(|(_, value)| value.as_str());
    let wrong = match named {
        Some(named) => !issuer.is_empty() && named != issuer,
        None => required,
    };
    if wrong {
        let why = match named {
            Some(_) => "the sign-in came back from another identity provider than the one asked: nothing of it was used",
            None => "the sign-in came back without your identity provider's name, which it says it always gives: nothing of it was used",
        };
        return (&FAILED, Some(Err(Refused::from(NetError::Proto(ProtoError::Protocol(why.into()))))));
    }
    // A no from the page is said in this module's own words, by its code alone: the provider's
    // words (`error_description`) may name the owner or the organisation, and are neither logged
    // nor shown (#1844 review).
    if let Some((_, error)) = fields.iter().find(|(name, _)| name == "error").filter(|(_, error)| !error.is_empty()) {
        log::info!("the identity provider's page came back with a no");
        // (The owner's own no, `access_denied` or `consent_required`, stays theirs: Declined.)
        let declined = matches!(error.trim().to_ascii_lowercase().as_str(), "access_denied" | "consent_required");
        return (&FAILED, Some(Err(Refused { declined, ..Refused::from(refused(error)) })));
    }
    match code_of(&fields) {
        Ok(code) => (&DONE, Some(Ok(code))),
        Err(refused) => (&FAILED, Some(Err(refused))),
    }
}

/// The address of the identity provider's sign-in page, with everything it needs to send the
/// owner back here. Nothing of the owner's but the address they sign in as (`login_hint`).
fn page(used: &mailcraft_model::OAuthDefaults, redirect_uri: &str, state: &str, challenge: &str, login_hint: &str) -> String {
    let mut fields = vec![
        ("client_id", used.client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("scope", used.scopes.as_str()),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    let hint = login_hint.trim();
    if !hint.is_empty() && hint.len() <= 320 && !hint.chars().any(char::is_control) {
        fields.push(("login_hint", hint));
    }
    let address = used.authorize.trim();
    format!("{address}{}{}", if address.contains('?') { '&' } else { '?' }, http::form(&fields))
}

/// Trade a refresh token for new tokens for the mail servers, without asking the owner anything.
/// The tokens carry the refresh token to use next time (a new one when the provider sent one).
/// `ProtoError::Auth`: the provider no longer takes it, and the owner has to sign in again;
/// `ProtoError::Rejected` or a connection error: it couldn't say, and the refresh token is as good
/// as it was.
pub fn refresh_custom(oauth: &OAuth, client_secret: &str, refresh_token: &str) -> Result<Tokens, NetError> {
    let used = registration(oauth, client_secret)?;
    let token_endpoint = Url::parse(&used.token)?;
    if refresh_token.is_empty() || !usable(refresh_token) {
        return Err(NetError::Proto(ProtoError::Auth("there is no earlier sign-in to renew".into())));
    }
    // (No scope: the new tokens are for what the sign-in was given, RFC 6749 §6.)
    let mut fields = vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
    let authorization = client_authentication(&used, client_secret, &mut fields);
    let secrets = [refresh_token, client_secret, credentials(&authorization)];
    trade_custom(&token_endpoint, &fields, authorization.as_deref(), refresh_token, &secrets)
}

/// Give a refresh token back to the identity provider (RFC 7009), when it names where: what is
/// done when the account is removed. Nothing to do when it names nowhere. A refusal or a provider
/// that can't be reached is an error, which the caller may let be: the token is forgotten here
/// either way.
pub fn revoke_custom(oauth: &OAuth, client_secret: &str, refresh_token: &str) -> Result<(), NetError> {
    let used = registration(oauth, client_secret).or_else(|error| {
        // (Settings that can't sign in may still say where to give a token back.)
        if oauth.provider == OAuthProvider::Custom && !oauth.revoke.trim().is_empty() { Ok(oauth.effective()) } else { Err(error) }
    })?;
    if used.revoke.is_empty() || refresh_token.is_empty() || !usable(refresh_token) {
        return Ok(());
    }
    mailcraft_model::oauth::endpoint(&used.revoke).map_err(|why| NetError::Config(format!("the revocation address {why}").into()))?;
    let endpoint = Url::parse(&used.revoke)?;
    let mut fields = vec![("token", refresh_token), ("token_type_hint", "refresh_token")];
    let authorization = client_authentication(&used, client_secret, &mut fields);
    let answer = http::post_form_as(&endpoint, &fields, authorization.as_deref(), TOKEN_TIMEOUT).map_err(|e| {
        [refresh_token, client_secret, credentials(&authorization)].iter().filter(|s| !s.is_empty()).fold(e, |e, secret| e.without(secret))
    })?;
    // RFC 7009 §2.2: 200 whether or not the token was still good.
    if answer.status == 200 {
        return Ok(());
    }
    let json = serde_json::from_slice::<Value>(&answer.body).ok();
    let code = json.as_ref().and_then(|j| j.get("error")).and_then(Value::as_str).unwrap_or("");
    Err(if code.is_empty() {
        NetError::Proto(ProtoError::Rejected(format!("your identity provider answered with status {}", answer.status).into()))
    } else {
        refused(code)
    })
}

/// POST the form to the token endpoint and read the tokens out of the answer, the refusal by its
/// code alone, with the client's credentials in the `Authorization` header when it sends them so.
/// `secrets` are the parts of the request that must not come back in an error.
fn trade_custom(
    endpoint: &Url,
    fields: &[(&str, &str)],
    authorization: Option<&str>,
    earlier_refresh: &str,
    secrets: &[&str],
) -> Result<Tokens, NetError> {
    let answer = http::post_form_as(endpoint, fields, authorization, TOKEN_TIMEOUT)
        .map_err(|e| secrets.iter().filter(|s| !s.is_empty()).fold(e, |e, secret| e.without(secret)))?;
    let json = serde_json::from_slice::<Value>(&answer.body).ok().filter(Value::is_object);
    if let Some(code) = json.as_ref().and_then(|j| j.get("error")).and_then(Value::as_str) {
        let code: String = code.chars().take(64).collect();
        return Err(refused(&code));
    }
    if answer.status >= 500 {
        return Err(NetError::Proto(ProtoError::Rejected(format!("your identity provider answered with status {}", answer.status).into())));
    }
    tokens_from(&answer, earlier_refresh, mailcraft_time::now(), secrets).map_err(|refused| refused.error)
}
