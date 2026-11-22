// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an identity provider's sign-in and token addresses from its own address (its issuer):
//! OpenID Connect Discovery 1.0 (`{issuer}/.well-known/openid-configuration`), then RFC 8414
//! (`https://host/.well-known/oauth-authorization-server{path}`).
//!
//! Only the issuer's address goes out: no mail address, no client id, no cookie. Fetched over
//! HTTPS alone (an `http://` address only on this computer, for tests), following at most
//! [`MAX_REDIRECTS`] redirects, each to an address that is checked as the first was; the answer
//! is capped by the HTTP layer.
//!
//! The document is hostile. It must name the issuer that was asked for, byte for byte (RFC 8414
//! §3.3, OpenID Connect Discovery §4.3): a document that names another is refused, so that one
//! provider can't speak for another. Only the fields a sign-in needs are read, each capped;
//! every address in it must be one an identity provider can be reached at
//! ([`mailcraft_model::oauth::endpoint`]). A provider that says which PKCE methods it takes and
//! doesn't list `S256` is refused: `plain` is never sent.

use std::time::Duration;

use mailcraft_model::ClientAuth;
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Url};

/// How long fetching one document may take.
const PATIENCE: Duration = Duration::from_secs(10);
/// Most redirects followed from one address.
pub(crate) const MAX_REDIRECTS: usize = 3;
/// Most entries of a list kept, and the longest one.
const MAX_LIST: usize = 64;
const MAX_ENTRY: usize = 256;

/// What an identity provider says of itself, as far as a sign-in needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovered {
    /// The issuer, as asked for and as the document names it.
    pub issuer: String,
    /// The page the owner signs in on.
    pub authorize: String,
    /// Where codes and refresh tokens are traded for tokens.
    pub token: String,
    /// Where a refresh token is given back; empty when it names none.
    pub revoke: String,
    /// The scopes it says it supports; empty when it doesn't say.
    pub scopes_supported: Vec<String>,
    /// The hosts of its addresses that lie outside the issuer's own domain (its registrable
    /// part), for the owner to be asked about once before anything is sent there.
    pub outside: Vec<String>,
    /// How a client secret is to be sent: in the `Authorization` header (`client_secret_basic`)
    /// unless the provider lists the form (`client_secret_post`) and not the header.
    pub client_auth: ClientAuth,
    /// It says its page names itself (`iss`) when it sends the owner back (RFC 9207).
    pub issuer_in_response: bool,
}

impl Discovered {
    /// Whether it says it can't hand out refresh tokens: it lists its scopes and
    /// `offline_access` isn't among them. (Saying nothing is no such word.)
    pub fn lacks_offline_access(&self) -> bool {
        !self.scopes_supported.is_empty() && !self.scopes_supported.iter().any(|s| s == "offline_access")
    }

    /// Which of the standard scopes (`openid email offline_access`) to ask it for: `None` for all
    /// of them, else only those it lists ([`mailcraft_model::oauth::base_scopes_for`]).
    pub fn base_scopes(&self) -> Option<String> {
        mailcraft_model::oauth::base_scopes_for(&self.scopes_supported)
    }
}

/// Fetch and read what the identity provider at `issuer` says of itself: its OpenID Connect
/// document, else its RFC 8414 one. The error says what was wrong with the last one tried.
pub fn discover(issuer: &str) -> Result<Discovered, NetError> {
    let issuer = checked_issuer(issuer)?;
    let mut last = NetError::Config("the identity provider doesn't describe itself at the usual addresses: enter its sign-in addresses".into());
    for address in addresses(&issuer) {
        match fetch(&address) {
            Ok(Some(body)) => match read(&issuer, &body) {
                Ok(found) => return Ok(found),
                // A document that is there but wrong says so, whatever the other address has.
                Err(error) => last = error,
            },
            Ok(None) => {}
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// The issuer as typed, if it is one: an `https://` address with no query or fragment.
fn checked_issuer(issuer: &str) -> Result<String, NetError> {
    let issuer = issuer.trim();
    if issuer.is_empty() {
        return Err(NetError::Config("enter your identity provider's address (its issuer)".into()));
    }
    mailcraft_model::oauth::endpoint(issuer).map_err(|why| NetError::Config(format!("the identity provider's address {why}").into()))?;
    if issuer.contains(['?', '#']) {
        return Err(NetError::Config("the identity provider's address can't have a query or a fragment".into()));
    }
    Ok(issuer.to_string())
}

/// Where the issuer's documents are, in the order they are tried.
pub(crate) fn addresses(issuer: &str) -> Vec<String> {
    let base = issuer.trim_end_matches('/');
    let mut out = vec![format!("{base}/.well-known/openid-configuration")];
    // RFC 8414 §3.1: the well-known part goes between the host and the issuer's path.
    if let Some((scheme, rest)) = base.split_once("://") {
        let (host, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
        out.push(format!("{scheme}://{host}/.well-known/oauth-authorization-server{path}"));
    }
    out
}

/// The body at `address`, following redirects to addresses that are checked as it was.
/// `Ok(None)` when there is nothing there (a status other than 200 and a redirect).
fn fetch(address: &str) -> Result<Option<Vec<u8>>, NetError> {
    let mut url = Url::parse(address)?;
    for _ in 0..=MAX_REDIRECTS {
        let request =
            http::Request { method: "GET", headers: &[("Accept", "application/json")], body: None, patience: PATIENCE, max_body: http::MAX_BODY };
        let answer = http::request(&url, &request)?;
        match answer.status {
            200 => return Ok(Some(answer.body)),
            301 | 302 | 303 | 307 | 308 => {
                let Some(next) = answer.location.as_deref().map(|to| absolute(&url, to)) else { return Ok(None) };
                // (`Url::parse` refuses `http://` anywhere but on this computer.)
                url = Url::parse(&next)?;
            }
            _ => return Ok(None),
        }
    }
    Err(NetError::Config("the identity provider sent its description on through too many redirects".into()))
}

/// A `Location` as an absolute address: as written when it is one, else on `from`'s host.
fn absolute(from: &Url, location: &str) -> String {
    let location = location.trim();
    if location.contains("://") {
        return location.to_string();
    }
    let scheme = if from.secure { "https" } else { "http" };
    let host = if from.host.contains(':') { format!("[{}]", from.host) } else { from.host.clone() };
    let port = if from.port == 0 { String::new() } else { format!(":{}", from.port) };
    let path = if location.starts_with('/') { location.to_string() } else { format!("/{location}") };
    format!("{scheme}://{host}{port}{path}")
}

/// Read an identity provider's description of itself, fetched for `issuer` (as typed).
pub fn read(issuer: &str, body: &[u8]) -> Result<Discovered, NetError> {
    let bad = |why: &str| NetError::Config(format!("the identity provider's description {why}").into());
    if body.len() > http::MAX_BODY {
        return Err(bad("is too long"));
    }
    let json: Value = serde_json::from_slice(body).map_err(|_| bad("isn't the JSON it should be"))?;
    let object = json.as_object().ok_or_else(|| bad("isn't the JSON it should be"))?;
    let text = |name: &str| object.get(name).and_then(Value::as_str).map(str::trim).unwrap_or("");
    let list = |name: &str| -> Vec<String> {
        let entries = object.get(name).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        entries
            .iter()
            .take(MAX_LIST)
            .filter_map(Value::as_str)
            .filter(|entry| entry.len() <= MAX_ENTRY && entry.bytes().all(|b| b.is_ascii_graphic()))
            .map(str::to_string)
            .collect()
    };
    let issuer = issuer.trim();
    if text("issuer") != issuer {
        return Err(NetError::Config(
            "the identity provider's description names another issuer than the address entered: check the address, character for character".into(),
        ));
    }
    let address = |name: &str, what: &str, needed: bool| -> Result<String, NetError> {
        let value = text(name);
        if value.is_empty() {
            return if needed { Err(bad(&format!("has no {what} address"))) } else { Ok(String::new()) };
        }
        mailcraft_model::oauth::endpoint(value)
            .map_err(|why| bad(&format!("gives a {what} address that {}", why.trim_start_matches("is ").trim_start_matches("must "))))?;
        Ok(value.to_string())
    };
    let authorize = address("authorization_endpoint", "sign-in", true)?;
    let token = address("token_endpoint", "token", true)?;
    let revoke = address("revocation_endpoint", "revocation", false)?;
    let methods = list("code_challenge_methods_supported");
    if object.contains_key("code_challenge_methods_supported") && !methods.iter().any(|m| m == "S256") {
        return Err(bad("says it doesn't take the protected sign-in (PKCE with S256) MailCraft uses"));
    }
    let grants = list("grant_types_supported");
    if !grants.is_empty() && !grants.iter().any(|g| g == "authorization_code") {
        return Err(bad("says it doesn't sign people in on its page (authorization code)"));
    }
    let client_auth = client_auth_of(&list("token_endpoint_auth_methods_supported"));
    let issuer_in_response = object.get("authorization_response_iss_parameter_supported").and_then(Value::as_bool).unwrap_or(false);
    let home = host_of(issuer);
    let home = mailcraft_model::sender_warnings::registrable(&home).to_string();
    let mut outside: Vec<String> = Vec::new();
    for found in [&authorize, &token, &revoke].into_iter().filter(|a| !a.is_empty()) {
        let host = host_of(found);
        if !mailcraft_model::is_within(&host, &home) && !outside.contains(&host) {
            outside.push(host);
        }
    }
    Ok(Discovered {
        issuer: issuer.to_string(),
        authorize,
        token,
        revoke,
        scopes_supported: list("scopes_supported"),
        outside,
        client_auth,
        issuer_in_response,
    })
}

/// How a client secret goes to a provider that lists these ways of taking one
/// (`token_endpoint_auth_methods_supported`): in the header, which every provider must take
/// (RFC 6749 §2.3.1) and which one that lists nothing takes (RFC 8414 §2), unless it lists the
/// form and not the header. (Its revocation address takes what its token address takes: a
/// provider that lists other ways there is rare enough not to be asked about.)
pub(crate) fn client_auth_of(methods: &[String]) -> ClientAuth {
    let lists = |method: &str| methods.iter().any(|m| m == method);
    if lists("client_secret_post") && !lists("client_secret_basic") { ClientAuth::Post } else { ClientAuth::Basic }
}

/// The host of an address, lower-cased.
fn host_of(address: &str) -> String {
    Url::parse(address).map(|url| url.host.to_ascii_lowercase()).unwrap_or_default()
}
