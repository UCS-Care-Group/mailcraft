//! JMAP (RFC 8620 core, RFC 8621 mail) without sockets and without browser APIs.
//!
//! A [`Client`] speaks JMAP over any [`Http`]: the browser's `fetch`, a blocking client (for a host
//! with sockets), or the in-process server of the tests (`standin`, behind the `stand-in` feature). Nothing
//! here reads the clock, starts a thread or opens a socket, so the crate builds for the browser as it
//! is, and its futures need not be `Send`.
//!
//! - [`Client::connect`] fetches the session and checks it: mail must be on offer, and the addresses it
//!   names are resolved against its own. The credentials go to the server the session is served from and
//!   to no other: an address on another server, or on the same one without encryption, is refused (plain
//!   `http` is for this computer alone). The limits the session states are clamped.
//! - [`Client::call`] sends one batch of method calls in one request. A method that fails fails its own
//!   answer ([`Answers::get`]), never the others; a request that fails as a whole is an [`Error`].
//! - [`sync_mail`] reads an account's mailboxes and the newest messages in them for the engine, as the
//!   request it worked out says, and hands them over in pieces ([`mail`]).
//! - [`fetch`] brings a message whole: the raw octets an email's blob holds, downloaded from the
//!   session's `downloadUrl`.
//!
//! Everything a server sends is hostile: answers are capped ([`MAX_JSON`]), ids are held to
//! [`valid_id`], numbers are clamped, and nothing here indexes into what came over the wire. The
//! credentials ([`Auth`]) stay out of `Debug` output and out of error text, even when a server echoes
//! them back, and so do the addresses and the mail a request or an answer carries.
//!
//! The address given to [`Client::connect`] is the one the session is served from. Finding it from a
//! bare host name through a redirect to another server (RFC 8620 §2.2) is for the caller: an [`Http`]
//! does not hand the credentials on to another origin, so a session that is behind such a redirect is
//! refused for want of them.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

use base64::Engine as _;
use serde_json::Value;

pub mod fetch;
pub mod mail;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin;
pub mod url;

pub use mail::{failed_run, sync_mail};
pub use url::{Url, pct, pct_decode};

/// The capability every session has (RFC 8620 §2).
pub const CORE: &str = "urn:ietf:params:jmap:core";
/// The capability of mail (RFC 8621 §1.3).
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
/// The kind of a problem that names a limit of the request that was exceeded (RFC 8620 §3.6.1).
const LIMIT_PROBLEM: &str = "urn:ietf:params:jmap:error:limit";

/// What a bare host is given after it (RFC 8620 §2.2).
const WELL_KNOWN: &str = "/.well-known/jmap";
/// Largest JSON answer accepted to a request of method calls. What is parsed out of it takes up much
/// more memory: a list of small numbers about sixteen times as much, a list of small objects about eighty
/// times (each object is a tree node), so a host that counts its memory should count that, and a caller
/// that knows how much it can be given asks for less ([`Client::call_within`]).
pub const MAX_JSON: usize = 32 << 20;
/// Largest session document accepted.
const MAX_SESSION: usize = 1 << 20;
/// Longest session address accepted.
const MAX_SESSION_URL: usize = 2048;
/// Longest address, or address template, the session may name.
const MAX_TEMPLATE: usize = 4096;
/// Longest variable name in an address template, and longest value put in one (a file name, a type).
const MAX_VARIABLE_NAME: usize = 32;
const MAX_VARIABLE: usize = 512;
/// Longest address made out of a template.
const MAX_EXPANDED: usize = 16 * 1024;
/// Longest id accepted from a server (RFC 8620 §1.2 allows 255; a server may go past it).
pub const MAX_ID: usize = 512;
/// What a session that states no `maxObjectsInGet` is taken to allow (RFC 8620 §2: every server takes
/// 500), and the most any session is believed: it may state anything.
const DEFAULT_OBJECTS_IN_GET: usize = 500;
const MAX_OBJECTS_IN_GET: usize = 1000;
/// Likewise for `maxCallsInRequest` (every server takes 16 calls in a request).
const DEFAULT_CALLS_IN_REQUEST: usize = 16;
const MAX_CALLS_IN_REQUEST: usize = 64;
/// Longest user name and password, or access token, that one sign-in may carry.
const MAX_CREDENTIALS: usize = 32 * 1024;
/// Longest text kept of what a server says in an error, and largest body read for it.
const MAX_ERROR_TEXT: usize = 300;
const MAX_PROBLEM: usize = 64 * 1024;
/// Longest wait, in seconds, that a server can ask for.
const MAX_RETRY_AFTER: u32 = 3600;

/// What can go wrong, in the categories the callers act on.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// The network failed (try again later).
    #[error("network: {0}")]
    Io(String),
    /// An address, or a way of getting there, that would have sent the credentials unencrypted, or to
    /// another server than the one the session is served from.
    #[error("secure connection: {0}")]
    Tls(String),
    /// The server broke the protocol: not JSON, not the shape the standard has, an id that is none.
    #[error("unexpected server response: {0}")]
    Protocol(String),
    /// The server answered a method call, or one object of it, with an error: JMAP's `type` and
    /// `description` (RFC 8620 §3.6.2), as the server worded them.
    #[error("the server refused: {}", said(.kind, .description))]
    Refused { kind: String, description: String },
    /// Any other HTTP status that is not a success: the request was not understood or not allowed, or a
    /// gateway failed.
    #[error("the server rejected the request: {0}")]
    Bad(String),
    /// 401 and 403: the credentials were refused, or have expired.
    #[error("sign-in failed: {0}")]
    Auth(String),
    /// 429 and 503: the server is overloaded or down; try again after `retry_after` seconds when it
    /// says how long (one second to an hour, whatever it asked for).
    #[error("the server is busy (HTTP {status})")]
    Busy { status: u16, retry_after: Option<u32> },
    /// A request that is over a limit of the server's (one it stated in the session, or one it refused the
    /// request for), or an answer over [`MAX_JSON`] or the size the request set.
    #[error("too large: {0}")]
    TooLarge(String),
}

fn said(kind: &str, description: &str) -> String {
    format!("{kind} {description}").trim().to_string()
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// This error with the secrets taken out of any text a transport put in it.
    fn hidden(self, secrets: &Secrets) -> Error {
        match self {
            Error::Io(t) => Error::Io(secrets.hide(&t)),
            Error::Tls(t) => Error::Tls(secrets.hide(&t)),
            Error::Protocol(t) => Error::Protocol(secrets.hide(&t)),
            Error::Refused { kind, description } => Error::Refused { kind: secrets.hide(&kind), description: secrets.hide(&description) },
            Error::Bad(t) => Error::Bad(secrets.hide(&t)),
            Error::Auth(t) => Error::Auth(secrets.hide(&t)),
            Error::TooLarge(t) => Error::TooLarge(secrets.hide(&t)),
            busy @ Error::Busy { .. } => busy,
        }
    }
}

/// An HTTP request.
#[derive(Clone, Default)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Largest answer body accepted, in bytes.
    pub max: usize,
}

impl Request {
    pub fn get(url: &str, max: usize) -> Request {
        Request { method: "GET".into(), url: url.into(), headers: Vec::new(), body: Vec::new(), max }
    }

    pub fn post(url: &str, content_type: &str, body: Vec<u8>, max: usize) -> Request {
        Request { method: "POST".into(), url: url.into(), headers: vec![("Content-Type".into(), content_type.into())], body, max }
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// What may be shown of a request: not the credentials in its headers, not the query of its address
/// (it can hold a person's address), not the mail in its body.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = Url::parse(&self.url).map_or_else(|_| "(not a web address)".to_string(), |u| u.origin());
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("origin", &origin)
            .field("headers", &names)
            .field("body_bytes", &self.body.len())
            .field("max", &self.max)
            .finish()
    }
}

/// An HTTP response.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// The seconds a `Retry-After` header asked to wait, when it gave a number of seconds.
    pub retry_after: Option<u32>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

/// The HTTP client JMAP runs over. What an implementation owes it:
///
/// - Redirects are followed for a GET, never from https to http, and the `Authorization` header never
///   goes to another origin than the one it was set for.
/// - No cookies are sent or kept.
/// - An address it is given is plain ASCII on one line (the [`Client`] sends nothing else), and the text of
///   an [`Error`] it makes holds neither the address nor the headers of the request.
/// - An answer whose body is over [`Request::max`] is refused as soon as that is known, rather than after
///   it is all read (the [`Client`] checks again what it is handed).
/// - The seconds a `Retry-After` header gives go into [`Response::retry_after`].
#[allow(async_fn_in_trait)]
pub trait Http {
    async fn send(&self, req: Request) -> Result<Response>;
}

/// How to sign in. Neither the user name nor the password nor the token shows in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// A user name and a password (RFC 7617).
    Basic { user: String, password: String },
    /// An OAuth access token (RFC 6750).
    Bearer(String),
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Auth::Basic { .. } => "Auth::Basic(..)",
            Auth::Bearer(_) => "Auth::Bearer(..)",
        })
    }
}

impl Auth {
    pub fn basic(user: impl Into<String>, password: impl Into<String>) -> Auth {
        Auth::Basic { user: user.into(), password: password.into() }
    }

    pub fn bearer(token: impl Into<String>) -> Auth {
        Auth::Bearer(token.into())
    }

    /// The value of the `Authorization` header. Credentials that would break out of the header, or
    /// that are none, are refused rather than cleaned.
    fn header(&self) -> Result<String> {
        let unsendable = Error::Auth("the user name, password or access token has a character that can't be sent".into());
        let too_long = Error::Auth("the user name, password or access token is too long to be sent".into());
        match self {
            Auth::Basic { user, password } => {
                if user.is_empty() && password.is_empty() {
                    return Err(Error::Auth("there is no user name or password to sign in with".into()));
                }
                if user.contains(':') || user.chars().any(char::is_control) || password.chars().any(char::is_control) {
                    return Err(unsendable);
                }
                if user.len().saturating_add(password.len()) > MAX_CREDENTIALS {
                    return Err(too_long);
                }
                Ok(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))))
            }
            Auth::Bearer(token) => {
                if token.is_empty() {
                    return Err(Error::Auth("there is no access token to sign in with".into()));
                }
                if !token.chars().all(|c| c.is_ascii_graphic()) {
                    return Err(unsendable);
                }
                if token.len() > MAX_CREDENTIALS {
                    return Err(too_long);
                }
                Ok(format!("Bearer {token}"))
            }
        }
    }
}

/// What must never be shown: the credentials and every form they are sent in, a user name that is an
/// address, and the address of the session with the addresses in its query.
#[derive(Clone, Default)]
struct Secrets(Vec<String>);

impl Secrets {
    fn of(auth: &Auth, header: &str, addresses: &[&str]) -> Secrets {
        let mut secrets = Secrets::default();
        secrets.add(header);
        match auth {
            Auth::Basic { user, password } => {
                secrets.add(password);
                secrets.add(header.strip_prefix("Basic ").unwrap_or_default());
                // A user name that is not an address may be any word (`me`): it would garble every text.
                if user.contains('@') {
                    secrets.add(user);
                }
            }
            Auth::Bearer(token) => secrets.add(token),
        }
        for address in addresses {
            secrets.add_address(address);
        }
        secrets
    }

    /// The address as it is, and each value of its query that is a person's address, written either way.
    fn add_address(&mut self, address: &str) {
        self.add(address);
        let query = address.split_once('?').map_or("", |(_, query)| query.split('#').next().unwrap_or(""));
        for value in query.split('&').filter_map(|pair| pair.split_once('=').map(|(_, value)| value)) {
            let plain = pct_decode(value);
            if plain.contains('@') {
                self.add(value);
                self.add(&plain);
            }
        }
    }

    /// A secret, and the form of it a `pct`-encoded address holds.
    fn add(&mut self, secret: &str) {
        for form in [secret.to_string(), pct(secret)] {
            if !form.is_empty() && !self.0.contains(&form) {
                self.0.push(form);
            }
        }
        // Longest first, so that a secret inside another one can't leave a piece behind.
        self.0.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// `text` with every secret replaced by `***`. What a server said is hidden here first and cut or
    /// folded after, never the other way: a secret that is cut in two, or whose blanks are folded, is no
    /// longer found.
    fn hide(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.0 {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }
}

/// Server text made fit for an error message: one line, no control characters, not too long.
fn tidy(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for (n, c) in text.trim().chars().enumerate() {
        if n >= max_chars {
            out.push('…');
            break;
        }
        if c.is_control() || c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// Whether `id` is one a server may hand out: 1 to [`MAX_ID`] characters of the URL-safe base64
/// alphabet without padding (`A-Z a-z 0-9 - _`, RFC 8620 §1.2). An id that is not goes nowhere: ids
/// are put into JSON keys (`mailboxIds/<id>`) and into addresses.
pub fn valid_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Whether `text` is the name of a scheme (`https`): a letter, then letters, digits, `+`, `-` and `.`.
fn is_scheme(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic()) && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The session address for what the owner typed: a bare host or origin gets the well-known path
/// (RFC 8620 §2.2), an address with a path of its own is left as it is.
pub fn session_url(input: &str) -> String {
    let typed = input.trim();
    // A `://` counts only after a scheme's own characters: the query of a bare host may hold one.
    let has_scheme = typed.split_once("://").is_some_and(|(scheme, _)| is_scheme(scheme));
    let full = if has_scheme { typed.to_string() } else { format!("https://{typed}") };
    let (scheme, rest) = full.split_once("://").unwrap_or(("https", ""));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (path, more) = tail.split_at(tail.find(['?', '#']).unwrap_or(tail.len()));
    if path.is_empty() || path == "/" { format!("{scheme}://{authority}{WELL_KNOWN}{more}") } else { format!("{scheme}://{authority}{path}{more}") }
}

/// The identifier of the method call numbered `index` in a request.
fn call_id(index: usize) -> String {
    format!("c{index}")
}

/// The number of the call that an identifier names, if it is one [`call_id`] gives (not `c01`, not `c+1`).
fn call_index(id: &str) -> Option<usize> {
    let index = id.strip_prefix('c')?.parse::<usize>().ok()?;
    (call_id(index) == id).then_some(index)
}

/// A reference to the result of the call numbered `call` (from 0, in the same request) for the
/// value of an argument whose name starts with `#` (RFC 8620 §3.7):
/// `json!({"#ids": result_of(0, "Email/query", "/ids")})` hands the ids a query found to a `get`.
pub fn result_of(call: usize, method: &str, path: &str) -> Value {
    serde_json::json!({"resultOf": call_id(call), "name": method, "path": path})
}

/// The answers to the method calls of one request, in the order of the calls.
pub struct Answers(Vec<Result<Value>>);

impl Answers {
    /// The answer to call number `call`: its result, or the error that call alone ended in
    /// ([`Error::Refused`] when the server said so, [`Error::Protocol`] when it gave no answer).
    pub fn get(&self, call: usize) -> Result<&Value> {
        match self.0.get(call) {
            Some(Ok(value)) => Ok(value),
            Some(Err(e)) => Err(e.clone()),
            None => Err(Error::Protocol("JMAP: there is no such call".into())),
        }
    }

    /// The answer to call number `call`, taken out: the same as [`Answers::get`], without a copy. The
    /// answer is not there afterwards.
    pub fn take(&mut self, call: usize) -> Result<Value> {
        match self.0.get_mut(call) {
            Some(slot) => std::mem::replace(slot, Err(Error::Protocol("JMAP: that answer was taken".into()))),
            None => Err(Error::Protocol("JMAP: there is no such call".into())),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What may be shown of answers: how many, never what they hold (mail).
impl fmt::Debug for Answers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Answers({} of which {} failed)", self.0.len(), self.0.iter().filter(|a| a.is_err()).count())
    }
}

/// Send a request and hold the answer to the size the request allowed.
async fn send<H: Http>(http: &H, req: Request) -> Result<Response> {
    let max = req.max;
    let resp = http.send(req).await?;
    if resp.body.len() > max {
        return Err(Error::TooLarge(format!("the answer is larger than {max} bytes")));
    }
    Ok(resp)
}

fn json_body(resp: &Response) -> Result<Value> {
    serde_json::from_slice(&resp.body).map_err(|e| Error::Protocol(format!("JMAP: {e}")))
}

/// Turn an HTTP failure into the error the caller knows how to handle: 401 and 403 are
/// [`Error::Auth`], 429 and 503 are [`Error::Busy`] (with the wait the server asked for), a request that
/// is over a limit of the server's (RFC 8620 §3.6.1) is [`Error::TooLarge`], and any other status that is
/// not a success is [`Error::Bad`] with the reason the server gave, when it gave one as RFC 7807 problem
/// details.
pub fn check(resp: &Response) -> Result<()> {
    check_hiding(resp, &Secrets::default())
}

/// [`check`], with the secrets taken out of what the server says before it is cut to size.
fn check_hiding(resp: &Response, secrets: &Secrets) -> Result<()> {
    match resp.status {
        200..=299 => Ok(()),
        401 | 403 => Err(Error::Auth("the server refused the credentials".into())),
        429 | 503 => Err(Error::Busy { status: resp.status, retry_after: resp.retry_after.map(|s| s.clamp(1, MAX_RETRY_AFTER)) }),
        status => {
            let problem = if resp.body.len() <= MAX_PROBLEM { serde_json::from_slice::<Value>(&resp.body).ok() } else { None };
            let text = |key: &str| problem.as_ref().and_then(|p| p.get(key)).and_then(Value::as_str).map(|s| tidy(&secrets.hide(s), MAX_ERROR_TEXT));
            if status == 400 && text("type").as_deref() == Some(LIMIT_PROBLEM) {
                return Err(Error::TooLarge(
                    format!("the request is over the server's limit {}", text("limit").unwrap_or_default()).trim().to_string(),
                ));
            }
            let detail = text("detail").or_else(|| text("type")).unwrap_or_default();
            Err(Error::Bad(format!("HTTP {status} {detail}").trim().to_string()))
        }
    }
}

/// `{accountId}`-style template variables (RFC 6570 level 1), percent-encoded; a variable that is not
/// given is empty. The address is built in one pass and is never allowed to pass [`MAX_EXPANDED`], so that
/// a template full of variables and a long value can't make a large one.
fn expand(template: &str, vars: &[(&str, &str)]) -> Result<String> {
    let too_long = || Error::Protocol("a JMAP address is too long".into());
    if vars.iter().any(|(_, value)| value.len() > MAX_VARIABLE) {
        return Err(too_long());
    }
    let encoded: Vec<(&str, String)> = vars.iter().map(|(name, value)| (*name, pct(value))).collect();
    let mut out = String::with_capacity(template.len());
    let mut push = |piece: &str| -> Result<()> {
        if out.len().saturating_add(piece.len()) > MAX_EXPANDED {
            return Err(too_long());
        }
        out.push_str(piece);
        Ok(())
    };
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        push(before)?;
        let Some((name, after)) = from_brace.get(1..).and_then(|inner| inner.split_once('}')) else {
            // (An open brace that is never closed: a template that `resolve` would not have let in.)
            push(from_brace)?;
            return Ok(out);
        };
        if let Some((_, value)) = encoded.iter().find(|(known, _)| *known == name) {
            push(value)?;
        }
        rest = after;
    }
    push(rest)?;
    Ok(out)
}

/// Whether the braces of an address are the variables of a template and nothing else: `{name}` with a name
/// of letters, digits and underscores, never nested, and none at all in an address that is no template.
fn well_formed(raw: &str, templated: bool) -> bool {
    let mut rest = raw;
    while let Some(at) = rest.find(['{', '}']) {
        let Some(inner) = rest.get(at..).and_then(|from| from.strip_prefix('{')).filter(|_| templated) else {
            return false;
        };
        let Some((name, after)) = inner.split_once('}') else {
            return false;
        };
        if name.is_empty() || name.len() > MAX_VARIABLE_NAME || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return false;
        }
        rest = after;
    }
    true
}

/// An address the session names, absolute or relative, resolved against the session's own; the
/// `{variables}` of a template stay as they are. It must be on the server the session is served from, and
/// reached the way the session was (encrypted, unless that was this computer): the credentials go there.
fn resolve(base: &Url, raw: &str, key: &str) -> Result<String> {
    let templated = key != "apiUrl";
    if raw.len() > MAX_TEMPLATE || !raw.bytes().all(|b| b.is_ascii_graphic()) || !well_formed(raw, templated) {
        return Err(Error::Protocol(format!("the JMAP {key} can't be used")));
    }
    let cut = raw.find('{').unwrap_or(raw.len());
    let (head, variables) = raw.split_at(cut);
    let target = base.join(head)?;
    if base.https && !target.https {
        return Err(Error::Tls(format!("the JMAP {key} isn't https")));
    }
    if target.origin() != base.origin() {
        return Err(Error::Tls(format!("the JMAP {key} isn't on the server the session is served from")));
    }
    Ok(format!("{}{variables}", target.full()))
}

/// A limit the session states, held between 1 and `cap`; `default` when it states none, or no number.
fn limit(core: Option<&Value>, key: &str, default: usize, cap: usize) -> usize {
    let stated = core.and_then(|c| c.get(key)).and_then(Value::as_u64);
    stated.map_or(default, |n| usize::try_from(n.min(cap as u64)).unwrap_or(cap)).clamp(1, cap)
}

/// A signed-in JMAP session.
pub struct Client<H> {
    http: H,
    /// The value of the `Authorization` header: a secret.
    auth: String,
    secrets: Secrets,
    account: String,
    api: String,
    download: String,
    upload: String,
    capabilities: Vec<String>,
    max_get: usize,
    max_calls: usize,
}

/// What may be shown of a client: sizes, never an address or a credential.
impl<H> fmt::Debug for Client<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("capabilities", &self.capabilities.len())
            .field("max_objects_in_get", &self.max_get)
            .field("max_calls_in_request", &self.max_calls)
            .finish_non_exhaustive()
    }
}

impl<H: Http> Client<H> {
    /// Fetch the session at `session` (what the owner typed: [`session_url`] gives it the well-known
    /// path if it has none) and check it offers mail. The session's addresses must be on the server it is
    /// served from: see the crate documentation.
    pub async fn connect(http: H, session: &str, auth: &Auth) -> Result<Client<H>> {
        let header = auth.header()?;
        let address = session_url(session);
        if address.len() > MAX_SESSION_URL {
            return Err(Error::Protocol("the session address is too long".into()));
        }
        let base = Url::parse(&address)?;
        if !base.https && !base.is_loopback() {
            return Err(Error::Tls("the session address isn't https".into()));
        }
        // What is sent is the address that was parsed, in plain ASCII, not what was typed.
        let sent = base.full();
        let secrets = Secrets::of(auth, &header, &[session, address.as_str(), sent.as_str()]);
        let request = Request::get(&sent, MAX_SESSION).header("Authorization", &header).header("Accept", "application/json");
        let resp = send(&http, request).await.map_err(|e| e.hidden(&secrets))?;
        check_hiding(&resp, &secrets)?;
        let doc = json_body(&resp)?;
        let client = Client::from_session(http, &base, &doc, header, secrets)?;
        log::debug!(
            "jmap: connected, {} capabilities, up to {} objects per get and {} calls per request",
            client.capabilities.len(),
            client.max_get,
            client.max_calls
        );
        Ok(client)
    }

    fn from_session(http: H, base: &Url, doc: &Value, auth: String, secrets: Secrets) -> Result<Client<H>> {
        let capabilities =
            doc.get("capabilities").and_then(Value::as_object).ok_or_else(|| Error::Protocol("the JMAP session has no capabilities".into()))?;
        if !capabilities.contains_key(MAIL) {
            return Err(Error::Protocol("this JMAP server doesn't offer mail".into()));
        }
        let account = doc
            .get("primaryAccounts")
            .and_then(|p| p.get(MAIL))
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
            .ok_or_else(|| Error::Protocol("no mail account in the JMAP session".into()))?;
        let address = |key: &str| -> Result<String> {
            let raw = doc.get(key).and_then(Value::as_str).ok_or_else(|| Error::Protocol(format!("the JMAP session has no `{key}`")))?;
            resolve(base, raw, key)
        };
        let core = capabilities.get(CORE);
        Ok(Client {
            api: address("apiUrl")?,
            download: address("downloadUrl")?,
            upload: address("uploadUrl")?,
            account: account.to_string(),
            // (As many as the session document holds, which is bounded by its size.)
            capabilities: capabilities.keys().cloned().collect(),
            max_get: limit(core, "maxObjectsInGet", DEFAULT_OBJECTS_IN_GET, MAX_OBJECTS_IN_GET),
            max_calls: limit(core, "maxCallsInRequest", DEFAULT_CALLS_IN_REQUEST, MAX_CALLS_IN_REQUEST),
            auth,
            secrets,
            http,
        })
    }

    /// The id of the account that holds the mail: the session's primary account for mail.
    pub fn account_id(&self) -> &str {
        &self.account
    }

    /// The most ids one `get` may be given: what the server takes, never more than 1000.
    pub fn max_objects_in_get(&self) -> usize {
        self.max_get
    }

    /// The most method calls one request may hold: what the server takes, never more than 64.
    pub fn max_calls_in_request(&self) -> usize {
        self.max_calls
    }

    /// Whether the session names a capability (`urn:ietf:params:jmap:submission`…).
    pub fn has_capability(&self, uri: &str) -> bool {
        self.capabilities.iter().any(|c| c == uri)
    }

    /// The address a blob is downloaded from: the session's template with the account, the blob (an
    /// id of the alphabet of [`valid_id`]), the type to answer with and a file name filled in (these two
    /// at most 512 bytes each).
    pub fn download_url(&self, blob_id: &str, content_type: &str, name: &str) -> Result<String> {
        if !valid_id(blob_id) {
            return Err(Error::Protocol("not a blob id".into()));
        }
        expand(&self.download, &[("accountId", &self.account), ("blobId", blob_id), ("type", content_type), ("name", name)])
    }

    /// The address a blob is uploaded to.
    pub fn upload_url(&self) -> Result<String> {
        expand(&self.upload, &[("accountId", &self.account)])
    }

    /// Run a batch of method calls in one request, and give their answers in order. A method that
    /// fails (`["error", …]`, no answer at all, an answer to another method) fails its own answer
    /// only. A request that fails as a whole, or holds more calls than the server takes (which is
    /// not sent), is an error.
    pub async fn call(&self, using: &[&str], calls: Vec<(&str, Value)>) -> Result<Answers> {
        self.call_within(using, calls, MAX_JSON).await
    }

    /// [`Client::call`] with an answer of at most `max` bytes (never more than [`MAX_JSON`]): one that is
    /// larger is [`Error::TooLarge`], and is not read to its end by an [`Http`] that keeps to
    /// [`Request::max`]. A caller that knows how big an answer can be asks for no more than that, so that a
    /// server can not make it hold a great deal of memory.
    pub async fn call_within(&self, using: &[&str], calls: Vec<(&str, Value)>, max: usize) -> Result<Answers> {
        if calls.is_empty() {
            return Ok(Answers(Vec::new()));
        }
        if calls.len() > self.max_calls {
            return Err(Error::TooLarge(format!("{} method calls in one request, and the server takes {}", calls.len(), self.max_calls)));
        }
        let names: Vec<&str> = calls.iter().map(|(name, _)| *name).collect();
        let method_calls: Vec<Value> =
            calls.into_iter().enumerate().map(|(i, (name, args))| Value::Array(vec![Value::from(name), args, Value::from(call_id(i))])).collect();
        let body = serde_json::json!({ "using": using, "methodCalls": method_calls });
        let bytes = serde_json::to_vec(&body).map_err(|e| Error::Protocol(e.to_string()))?;
        let request = Request::post(&self.api, "application/json", bytes, max.min(MAX_JSON))
            .header("Authorization", &self.auth)
            .header("Accept", "application/json");
        let resp = send(&self.http, request).await.map_err(|e| e.hidden(&self.secrets))?;
        check_hiding(&resp, &self.secrets)?;
        let responses = match json_body(&resp)? {
            Value::Object(mut top) => match top.remove("methodResponses") {
                Some(Value::Array(responses)) => responses,
                _ => return Err(Error::Protocol("JMAP: no methodResponses".into())),
            },
            _ => return Err(Error::Protocol("JMAP: the answer isn't an object".into())),
        };
        log::debug!("jmap: {} method call(s) in one request", names.len());
        Ok(Answers(answers_to(responses, &names, &self.secrets)))
    }

    /// [`Client::call`] with the capabilities of mail named in `using`.
    pub async fn mail(&self, calls: Vec<(&str, Value)>) -> Result<Answers> {
        self.call(&[CORE, MAIL], calls).await
    }

    /// [`Client::call_within`] with the capabilities of mail named in `using`.
    pub async fn mail_within(&self, calls: Vec<(&str, Value)>, max: usize) -> Result<Answers> {
        self.call_within(&[CORE, MAIL], calls, max).await
    }
}

/// The answers to the calls of a request, whose methods are `names`, found in one pass over everything the
/// server answered: for each call, the first answer that has its call id and is named for its method, or
/// `error`.
fn answers_to(responses: Vec<Value>, names: &[&str], secrets: &Secrets) -> Vec<Result<Value>> {
    let mut found: Vec<Option<Vec<Value>>> = names.iter().map(|_| None).collect();
    for response in responses {
        let Value::Array(parts) = response else { continue };
        let index = parts.get(2).and_then(Value::as_str).and_then(call_index);
        let name = parts.first().and_then(Value::as_str);
        let (Some(index), Some(name)) = (index, name) else { continue };
        let wanted = names.get(index).is_some_and(|method| name == *method || name == "error");
        if let Some(slot) = found.get_mut(index).filter(|slot| wanted && slot.is_none()) {
            *slot = Some(parts);
        }
    }
    names.iter().zip(found).map(|(method, parts)| answer(parts, method, secrets)).collect()
}

/// The answer to one call: its result, or the error it ended in.
fn answer(parts: Option<Vec<Value>>, method: &str, secrets: &Secrets) -> Result<Value> {
    let Some(mut parts) = parts else {
        return Err(Error::Protocol(format!("JMAP: no answer to {method}")));
    };
    let failed = parts.first().and_then(Value::as_str) == Some("error");
    let args = parts.get_mut(1).map(std::mem::take).unwrap_or_default();
    if failed {
        let text = |key: &str, max| args.get(key).and_then(Value::as_str).map(|s| tidy(&secrets.hide(s), max)).unwrap_or_default();
        return Err(Error::Refused { kind: text("type", 64), description: text("description", MAX_ERROR_TEXT) });
    }
    Ok(args)
}

/// Run a future whose every wait is already over, as a blocking HTTP client's are: it
/// completes on the first poll. A future that does wait is polled again and again until it is done, which
/// is never if what it waits for needs this thread: it is no executor, and it is not for those.
#[cfg(not(target_arch = "wasm32"))]
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = f.as_mut().poll(&mut cx) {
            return value;
        }
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests;
