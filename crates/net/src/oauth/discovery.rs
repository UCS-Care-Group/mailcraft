// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an identity provider's sign-in and token addresses from its own address (its issuer):
//! OpenID Connect Discovery 1.0 (`{issuer}/.well-known/openid-configuration`), then RFC 8414
//! (`https://host/.well-known/oauth-authorization-server{path}`).
//!
//! Only the issuer's address goes out: no mail address, no client id, no cookie. Fetched over
//! HTTPS alone (an `http://` address only on this computer, and only in tests:
//! `custom::provider_url`), following at most [`MAX_REDIRECTS`] redirects, each to an address
//! that is checked as the first was and that never leads from a provider on the network to this
//! computer, a private network or plain `http` ([`may_redirect`]); an address on another server
//! is asked the way a picture is, its name looked up and every address of it public, and the
//! connection made to an address that was checked ([`fetch_with`]). The answer is capped.
//!
//! The document is hostile. It must name the issuer that was asked for, byte for byte (RFC 8414
//! §3.3, OpenID Connect Discovery §4.3): a document that names another is refused, so that one
//! provider can't speak for another. Only the fields a sign-in needs are read, each capped;
//! every address in it must be one an identity provider can be reached at
//! ([`mailcraft_model::oauth::endpoint`]). A provider that says which PKCE methods it takes and
//! doesn't list `S256` is refused: `plain` is never sent.

use std::time::{Duration, Instant};

use mailcraft_i18n::{Arg, said};
use mailcraft_model::ClientAuth;
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Url};
use crate::pictures::{self, Limits, PictureError, Reach, Sent};

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
    let mut last = NetError::Config(said!("the identity provider doesn't describe itself at the usual addresses: enter its sign-in addresses"));
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
        return Err(NetError::Config(said!("enter your identity provider's address (its issuer)")));
    }
    mailcraft_model::oauth::endpoint(issuer).map_err(|why| NetError::Config(said!("the identity provider's address {why}", why)))?;
    if issuer.contains(['?', '#']) {
        return Err(NetError::Config(said!("the identity provider's address can't have a query or a fragment")));
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

/// What one request for a description brought back.
pub(super) struct Got {
    pub(super) status: u16,
    pub(super) body: Vec<u8>,
    /// Where a redirect leads, as the server wrote it.
    pub(super) location: Option<String>,
}

/// Why a redirect isn't followed.
fn elsewhere() -> NetError {
    NetError::Config(said!("the identity provider sent its description on to an address on this computer, a private network or without https"))
}

/// The body at `address`, following redirects to addresses that are checked as it was.
/// `Ok(None)` when there is nothing there (a status other than 200 and a redirect).
fn fetch(address: &str) -> Result<Option<Vec<u8>>, NetError> {
    fetch_with(address, &Reach::web())
}

/// [`fetch`], with where names lead and connections may go said.
///
/// The issuer the owner typed is asked as it is, wherever it is (an organisation's own provider
/// may be on its own network), and so is another address on the same server. An address on
/// another server, which only the provider's redirect names, is asked the way a picture is: its
/// name is looked up here, every address it has must be on the public internet, and the
/// connection goes to an address that was checked (`pictures::ask_with`), so that a redirect
/// never reaches this computer or a private network, by its address or by its name (#1844
/// review). An issuer on this computer (a test's) may lead anywhere [`may_redirect`] lets it.
pub(super) fn fetch_with(address: &str, reach: &Reach<'_>) -> Result<Option<Vec<u8>>, NetError> {
    let mut url = super::custom::provider_url(address)?;
    let first = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let got = if crate::transport::is_loopback(&first.host) || url.same_server(&first) { ask(&url)? } else { ask_public(&url, reach)? };
        match got.status {
            200 => return Ok(Some(got.body)),
            301 | 302 | 303 | 307 | 308 => {
                let Some(location) = got.location else { return Ok(None) };
                url = next_hop(&first, &url, &location, super::custom::plaintext_allowed())?;
            }
            _ => return Ok(None),
        }
    }
    Err(NetError::Config(said!("the identity provider sent its description on through too many redirects")))
}

/// Where a redirect from `url` to `location` leads, met fetching the description of the issuer at
/// `first`: checked as the issuer was (`https://` alone, unless `plaintext` and on this computer:
/// `custom::provider_url`), and by [`may_redirect`] (#1844 review).
pub(super) fn next_hop(first: &Url, url: &Url, location: &str, plaintext: bool) -> Result<Url, NetError> {
    let next = super::custom::provider_url_when(&absolute(url, location), plaintext)?;
    if !may_redirect(first, &next) {
        return Err(elsewhere());
    }
    Ok(next)
}

/// One request for a description, as the HTTP layer makes it.
fn ask(url: &Url) -> Result<Got, NetError> {
    let request =
        http::Request { method: "GET", headers: &[("Accept", "application/json")], body: None, patience: PATIENCE, max_body: http::MAX_BODY };
    let answer = http::request(url, &request)?;
    Ok(Got { status: answer.status, body: answer.body, location: answer.location })
}

/// One request for a description at an address a redirect named, to the public internet alone.
pub(super) fn ask_public(url: &Url, reach: &Reach<'_>) -> Result<Got, NetError> {
    let limits = Limits { max_bytes: http::MAX_BODY, patience: PATIENCE, ..Limits::default() };
    let deadline = Instant::now() + PATIENCE;
    let sent = Sent { accept: "application/json", extra: "" };
    let answer = pictures::ask_with(url, reach, &limits, http::MAX_BODY, deadline, None, &sent, &pictures::Cancel::new()).map_err(refused)?;
    let body = match &answer.said.coding {
        Some(coding) => pictures::unpack(answer.body, coding, http::MAX_BODY).map_err(refused)?,
        None => answer.body,
    };
    Ok(Got { status: answer.status, body, location: answer.said.location })
}

/// What went wrong asking at an address a redirect named, as discovery says it.
fn refused(error: PictureError) -> NetError {
    match error {
        PictureError::NotPublic | PictureError::Address => elsewhere(),
        PictureError::Net(error) => error,
        other => NetError::Config(said!("the identity provider's description couldn't be fetched: {other}", other)),
    }
}

/// Whether a redirect met fetching the description of the issuer at `first` may be followed to
/// `next`: before the owner has agreed to anything, it never leads from a provider on the
/// network to this computer or a private network (by an address written as one), nor from
/// `https` to `http` (#1844 review). An issuer on this computer (a test's) may lead anywhere
/// [`Url::parse`] takes.
pub(super) fn may_redirect(first: &Url, next: &Url) -> bool {
    if crate::transport::is_loopback(&first.host) {
        return true;
    }
    let inside = crate::transport::is_loopback(&next.host) || next.host.parse::<std::net::IpAddr>().is_ok_and(|ip| !crate::pictures::is_public(ip));
    next.secure && !inside
}

/// Whether `reference` starts with a scheme (RFC 3986 §3.1: a letter, then letters, digits,
/// `+`, `-` or `.`, then `:`), which makes it an absolute address.
fn has_scheme(reference: &str) -> bool {
    let Some((scheme, _)) = reference.split_once(':') else { return false };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic()) && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// A `Location` as an absolute address, resolved against `from` as a URI reference is (RFC 3986
/// §5.2): an absolute one as written; `//host/…` with `from`'s scheme; `/path` on `from`'s host;
/// `?query` on `from`'s path; and `name` beside `from`'s document, in its directory (#1844
/// review). What comes of it is checked by [`Url::parse`] before anything is fetched.
pub(super) fn absolute(from: &Url, location: &str) -> String {
    let location = location.trim();
    // (Absolute by the scheme it starts with, not by a `://` anywhere: `/next?u=https://x` is a
    // path on `from`'s host, #1844 review.)
    if has_scheme(location) {
        return location.to_string();
    }
    let scheme = if from.secure { "https" } else { "http" };
    if let Some(rest) = location.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    let host = if from.host.contains(':') { format!("[{}]", from.host) } else { from.host.clone() };
    let port = if from.port == 0 { String::new() } else { format!(":{}", from.port) };
    // (`from`'s path without its query, and the directory it is in.)
    let path = from.path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    let target = if location.starts_with('/') {
        location.to_string()
    } else if location.starts_with('?') {
        format!("{path}{location}")
    } else if location.is_empty() || location.starts_with('#') {
        path.to_string()
    } else {
        let directory = path.rfind('/').map_or("/", |at| path.get(..=at).unwrap_or("/"));
        format!("{directory}{location}")
    };
    format!("{scheme}://{host}{port}{}", remove_dots(&target))
}

/// A path with its `.` and `..` segments taken out (RFC 3986 §5.2.4), its query kept as it is.
fn remove_dots(target: &str) -> String {
    let (path, query) = match target.find(['?', '#']) {
        Some(at) => target.split_at(at),
        None => (target, ""),
    };
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let last = segments.len().saturating_sub(1);
    for (n, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {
                if n == last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if n == last {
                    out.push("");
                }
            }
            other => out.push(other),
        }
    }
    format!("/{}{query}", out.join("/"))
}

/// Read an identity provider's description of itself, fetched for `issuer` (as typed).
pub fn read(issuer: &str, body: &[u8]) -> Result<Discovered, NetError> {
    let bad = |why: mailcraft_i18n::Text| NetError::Config(why);
    if body.len() > http::MAX_BODY {
        return Err(bad(said!("the identity provider's description is too long")));
    }
    let not_json = || bad(said!("the identity provider's description isn't the JSON it should be"));
    let json: Value = serde_json::from_slice(body).map_err(|_| not_json())?;
    let object = json.as_object().ok_or_else(not_json)?;
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
        return Err(NetError::Config(said!(
            "the identity provider's description names another issuer than the address entered: check the address, character for character"
        )));
    }
    let address = |name: &str, what: Arg, needed: bool| -> Result<String, NetError> {
        let value = text(name);
        if value.is_empty() {
            return if needed { Err(bad(said!("the identity provider's description has no {what} address", what))) } else { Ok(String::new()) };
        }
        mailcraft_model::oauth::endpoint(value).map_err(|why| {
            let why = why.trim_start_matches("is ").trim_start_matches("must ").to_string();
            bad(said!("the identity provider's description gives a {what} address that {why}", what, why))
        })?;
        Ok(value.to_string())
    };
    let authorize = address("authorization_endpoint", Arg::words("sign-in"), true)?;
    let token = address("token_endpoint", Arg::words("token"), true)?;
    let revoke = address("revocation_endpoint", Arg::words("revocation"), false)?;
    let methods = list("code_challenge_methods_supported");
    if object.contains_key("code_challenge_methods_supported") && !methods.iter().any(|m| m == "S256") {
        return Err(bad(said!("the identity provider's description says it doesn't take the protected sign-in (PKCE with S256) MailCraft uses")));
    }
    let grants = list("grant_types_supported");
    if !grants.is_empty() && !grants.iter().any(|g| g == "authorization_code") {
        return Err(bad(said!("the identity provider's description says it doesn't sign people in on its page (authorization code)")));
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
