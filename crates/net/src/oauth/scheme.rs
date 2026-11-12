// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the provider's page sends the owner back to: a loopback port (a desktop) or a custom
//! scheme (a phone, RFC 8252 §7.1).
//!
//! On a phone there is no port to listen on: the system hands the program the address its page
//! came back to. The host that knows how supplies the [`Handback`], which waits for that address
//! and gives it over whole; what is read out of it is read here, strictly. The scheme is the
//! reverse of a domain the caller owns (`org.example.app`), passed in: nothing is built in.
//!
//! The address is hostile. A program on the same phone may be registered for the same scheme and
//! send one of its own, so the state (which only the page that was given it can send back) must
//! match, the scheme and the path must be the registered ones to the letter, nothing may come
//! back that a sign-in has no use for, and nothing may be said twice. Whatever is wrong ends the
//! sign-in in an error that says nothing of the address: the address, the code and the state
//! never appear in an error or a log line.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Redirect, Refused, code_of, consent_of, same};
use crate::NetError;
use crate::http;
use mailcraft_proto::ProtoError;

/// The path after the scheme when the caller names none: `org.example.app:/oauth2redirect`.
pub const DEFAULT_REDIRECT_PATH: &str = "/oauth2redirect";

/// The longest address that is read; the rest of a longer one is not looked at, it is refused.
const MAX_ADDRESS: usize = 8 * 1024;
/// The most names and values an address may bring.
const MAX_FIELDS: usize = 24;
/// The longest value of any one name, as it comes (before it is decoded).
const MAX_VALUE: usize = 4 * 1024;
/// The longest scheme and path that are accepted for registration.
const MAX_SCHEME: usize = 128;
const MAX_PATH: usize = 128;

/// What a sign-in's page may send back (`response_mode=query`; RFC 6749 §4.1.2, RFC 9207, and
/// what Microsoft adds). Anything else is refused.
const SIGN_IN_FIELDS: &[&str] = &["code", "state", "error", "error_description", "error_subcode", "error_uri", "session_state", "iss", "client_info"];
/// The same for the page of an administrator's approval.
const CONSENT_FIELDS: &[&str] = &["state", "admin_consent", "tenant", "error", "error_description", "error_subcode", "error_uri"];

/// What the host gives to bring the address the system opened the program with: it waits (for
/// no longer than the time given, and not past the flag being set) and returns the whole address,
/// or says why there isn't one ([`NetError::SignInCancelled`], [`NetError::SignInTimedOut`]).
pub type Handback = dyn Fn(&AtomicBool, Duration) -> Result<String, NetError>;

/// A custom-scheme redirect: the registered address, and where the address it came back to is had.
pub struct CustomScheme<'a> {
    scheme: String,
    path: String,
    handback: &'a Handback,
}

impl std::fmt::Debug for CustomScheme<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomScheme").field("scheme", &self.scheme).field("path", &self.path).finish_non_exhaustive()
    }
}

impl<'a> CustomScheme<'a> {
    /// `scheme` is a reverse-domain scheme in lower case (`org.example.app`: letters, digits, `.`,
    /// `-` and `+`, at least two labels), never one the program follows for other things.
    /// `path` is what follows `scheme:` in the address registered with the provider (it starts
    /// with `/`: [`DEFAULT_REDIRECT_PATH`]).
    pub fn new(scheme: &str, path: &str, handback: &'a Handback) -> Result<CustomScheme<'a>, NetError> {
        if !valid_scheme(scheme) {
            return Err(NetError::Config("the sign-in's redirect scheme must be a reverse domain in lower case, such as org.example.app".into()));
        }
        if !valid_path(path) {
            return Err(NetError::Config("the sign-in's redirect path must start with / and use letters, digits and - . _ ~ / only".into()));
        }
        Ok(CustomScheme { scheme: scheme.to_string(), path: path.to_string(), handback })
    }

    /// The address as it is registered with the provider and sent on its page.
    pub fn redirect_uri(&self) -> String {
        format!("{}:{}", self.scheme, self.path)
    }
}

/// How the owner comes back from the provider's page.
#[derive(Debug)]
pub enum RedirectStrategy<'a> {
    /// A port of this machine's loopback address: what a desktop does.
    Loopback,
    /// A custom scheme the system hands back to the program: what a phone does.
    CustomScheme(CustomScheme<'a>),
}

fn valid_scheme(scheme: &str) -> bool {
    let bytes = scheme.as_bytes();
    // Not `http`, `https`, `file`, `mailto`... : a reverse domain has a dot, and these haven't.
    scheme.len() >= 3
        && scheme.len() <= MAX_SCHEME
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.last().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+'))
        && scheme.contains('.')
        && !scheme.contains("..")
        && !scheme.contains(".-")
        && !scheme.contains("-.")
        && !scheme.contains(".+")
        && !scheme.contains("+.")
}

fn valid_path(path: &str) -> bool {
    path.len() >= 2
        && path.len() <= MAX_PATH
        && path.starts_with('/')
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/'))
        && !path.contains("//")
        && !path.contains("..")
        && !path.ends_with('/')
}

fn hostile(why: &'static str) -> NetError {
    NetError::Proto(ProtoError::Protocol(why.into()))
}

/// The names and values the address the page came back to brings with `state`, when it is the
/// registered address and the page that was given the state is the one that sent it.
fn read_return(address: &str, scheme: &CustomScheme<'_>, state: &str, allowed: &[&str]) -> Result<Vec<(String, String)>, NetError> {
    if address.len() > MAX_ADDRESS {
        return Err(hostile("the sign-in came back with an address that is too long"));
    }
    // Visible ASCII only: no white space, no control characters, no letters that look like others.
    // A `#` would be a fragment, which a page asked for the query never sends.
    if !address.bytes().all(|b| matches!(b, b'!'..=b'~')) || address.contains('#') {
        return Err(hostile("the sign-in came back with an address that is not valid"));
    }
    let (got_scheme, rest) = address.split_once(':').unwrap_or(("", ""));
    if !got_scheme.eq_ignore_ascii_case(&scheme.scheme) {
        return Err(hostile("the sign-in came back to an address that is not the registered one"));
    }
    let query = match rest.strip_prefix(scheme.path.as_str()) {
        Some("") => "",
        Some(after) => after.strip_prefix('?').ok_or_else(|| hostile("the sign-in came back to an address that is not the registered one"))?,
        None => return Err(hostile("the sign-in came back to an address that is not the registered one")),
    };
    let pieces: Vec<&str> = query.split('&').filter(|piece| !piece.is_empty()).collect();
    if pieces.len() > MAX_FIELDS || pieces.iter().any(|piece| piece.len() > MAX_VALUE) {
        return Err(hostile("the sign-in came back with too much"));
    }
    // A `%` that isn't followed by two hex digits is not an escape: the provider doesn't send one.
    let bytes = query.as_bytes();
    let broken =
        bytes.iter().enumerate().any(|(n, b)| *b == b'%' && !bytes.get(n + 1..n + 3).is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)));
    if broken {
        return Err(hostile("the sign-in came back with an address that is not valid"));
    }
    let fields = http::pairs(query);
    for (n, (name, _)) in fields.iter().enumerate() {
        if !allowed.contains(&name.as_str()) {
            return Err(hostile("the sign-in came back with something it was not asked for"));
        }
        if fields.iter().take(n).any(|(earlier, _)| earlier == name) {
            return Err(hostile("the sign-in came back with something said twice"));
        }
    }
    // Only the page that was given the state can send it back.
    let sent = fields.iter().find(|(name, _)| name == "state").map_or("", |(_, value)| value.as_str());
    if state.is_empty() || !same(sent, state) {
        return Err(hostile("the sign-in came back for another sign-in"));
    }
    Ok(fields)
}

/// Where this sign-in's page is sent back to, once it is ready for that.
pub(super) enum Landing<'a, 'b> {
    Port(Redirect),
    Scheme(&'b CustomScheme<'a>),
}

impl<'a, 'b> Landing<'a, 'b> {
    pub(super) fn open(via: &'b RedirectStrategy<'a>) -> Result<Landing<'a, 'b>, NetError> {
        match via {
            RedirectStrategy::Loopback => Ok(Landing::Port(Redirect::listen()?)),
            RedirectStrategy::CustomScheme(scheme) => Ok(Landing::Scheme(scheme)),
        }
    }

    /// The `redirect_uri` the page and the trade of the code are told.
    pub(super) fn uri(&self) -> String {
        match self {
            Landing::Port(redirect) => format!("http://localhost:{}", redirect.port),
            Landing::Scheme(scheme) => scheme.redirect_uri(),
        }
    }

    pub(super) fn port(&self) -> Option<u16> {
        match self {
            Landing::Port(redirect) => Some(redirect.port),
            Landing::Scheme(_) => None,
        }
    }

    /// Wait for the code that goes with `state`.
    pub(super) fn wait_code(&self, state: &str, cancel: &AtomicBool, patience: Duration) -> Result<String, Refused> {
        match self {
            Landing::Port(redirect) => redirect.wait(state, cancel, patience),
            Landing::Scheme(scheme) => {
                let address = handed_back(scheme, cancel, patience)?;
                code_of(&read_return(&address, scheme, state, SIGN_IN_FIELDS)?)
            }
        }
    }

    /// Wait for the answer of an administrator's approval that goes with `state`.
    pub(super) fn wait_consent(&self, state: &str, cancel: &AtomicBool, patience: Duration) -> Result<(), Refused> {
        match self {
            Landing::Port(redirect) => redirect.wait_for(state, cancel, patience, super::judge_consent),
            Landing::Scheme(scheme) => {
                let address = handed_back(scheme, cancel, patience)?;
                consent_of(&read_return(&address, scheme, state, CONSENT_FIELDS)?)
            }
        }
    }
}

/// The address the host brings, unless the sign-in was called off before it came or while it
/// was on its way.
fn handed_back(scheme: &CustomScheme<'_>, cancel: &AtomicBool, patience: Duration) -> Result<String, Refused> {
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    let address = (scheme.handback)(cancel, patience)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    Ok(address)
}
