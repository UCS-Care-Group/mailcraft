// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an Exchange server's Exchange Web Services address from an e-mail address: the POX
//! Autodiscover of [MS-OXDSCLI], as a state machine that the host drives.
//!
//! The machine does no I/O. It says what to do next ([`Autodiscover::step`]): perform this
//! HTTP request, look this SRV name up, ask the owner about this host, or here is the end of it
//! ([`Outcome`]). The host does that and tells the machine what came of it
//! ([`Autodiscover::response`], [`Autodiscover::srv`], [`Autodiscover::agree`],
//! [`Autodiscover::decline`]). So the host owns the sockets, the name server and the question
//! to the owner, and the rules live here and are tested here.
//!
//! The rules (spec E3, Q10, Q19, Q22):
//!
//! - In this order, over HTTPS only: `https://<domain>/autodiscover/autodiscover.xml`, then
//!   `https://autodiscover.<domain>/autodiscover/autodiscover.xml`, then the hosts of the
//!   `_autodiscover._tcp.<domain>` SRV record. The plain-HTTP redirect step of the specification
//!   is never taken.
//! - A redirect (HTTP 301, 302, 303, 307 or 308 with a `Location`, or an answer whose `Action` is
//!   `redirectUrl` or `redirectAddr`) is followed one step at a time, at most
//!   [`MAX_REDIRECTS`] in a row, and only to an `https://` address.
//! - The user name and password (the [`Secret`] the machine was made with) go only to a host
//!   inside the owner's own mail domain, or to one the owner agreed to when the machine asked
//!   ([`Step::NeedsYes`], naming the host). That covers a redirect to another host, the host of
//!   an SRV record, another mail domain named by `redirectAddr`, and the Exchange Web Services
//!   address the answer gives, which is where the password goes next.
//! - A mailbox the answer places in Exchange Online (a Microsoft 365 host: the redirect, the
//!   address or the server) ends the search with [`Outcome::ExchangeOnline`] and nothing is
//!   sent there: Exchange Online is a Microsoft account (ADR 0002), not an Exchange server account.
//! - The answer is hostile XML ([`crate::xml`]): capped in size, no DTD, and only the Exchange
//!   Web Services address (internal and external) and the mailbox's server are taken from it.
//! - Failure is one of a few reasons the owner can act on ([`Failure`]); each one offers
//!   Server settings, where the address can be typed.
//!
//! A user name and password that are refused (401) end the search at once: asking other hosts
//! with the same wrong password is how an account gets locked.
//!
//! Nothing here is logged, and the [`Debug`](std::fmt::Debug) of everything that holds a host
//! shows nothing of it: the names of an organisation's servers are its own.

use std::collections::VecDeque;
use std::fmt;

pub use mailcraft_model::is_exchange_online;
use mailcraft_model::provider;
use mailcraft_model::{is_microsoft_domain, is_within};

use crate::http::{HttpExchange, HttpRequest, HttpResponse, Method, Secret, TransportError, check_endpoint};
use crate::xml::{self, Node};

/// Most redirects followed in a row.
pub const MAX_REDIRECTS: u8 = 5;
/// Most requests one search makes: a loop of agreed redirects between candidates ends here too.
const MAX_REQUESTS: u8 = 16;
/// Most hosts taken from an SRV record.
const MAX_SRV_HOSTS: usize = 3;
/// The largest answer read: Autodiscover answers weigh a few kilobytes.
pub const MAX_ANSWER: usize = 256 * 1024;
/// Longest address kept.
const MAX_URL: usize = 2_048;
/// Longest server name kept.
const MAX_HOST: usize = 253;

/// The namespace of the request.
pub(crate) const REQUEST_SCHEMA: &str = "http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006";
/// The namespace of the answer's document element.
pub(crate) const ANSWER_SCHEMA: &str = "http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006";
/// The schema asked for, and the namespace of the answer's `Response`.
pub(crate) const RESPONSE_SCHEMA: &str = "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";

/// Whether `host`, one of a domain's mail exchangers (`example-org.mail.protection.outlook.com`),
/// is Microsoft's: the domain's mail is in Exchange Online. A host that has looked up the MX
/// records before it starts tells the search so with [`Autodiscover::note_mail_exchangers`].
pub fn is_exchange_online_mx(host: &str) -> bool {
    provider::by_mx(host).is_some_and(|preset| preset.oauth.is_some())
}

/// Whether `text` is a host or domain name with a dot in it: labels of letters, digits and
/// hyphens, none empty, none over 63, none starting or ending with a hyphen. (A name in other
/// letters is written as its ASCII form, which is what an address on the wire has.)
fn is_name(text: &str) -> bool {
    text.len() <= MAX_HOST
        && text.contains('.')
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The host of an `https://` address, lower case, without port and the dot it may end in.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("https://")).and_then(|_| url.get(8..))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(inside) => inside.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let host = host.trim_end_matches('.');
    // Letters, digits, hyphens, dots (and colons of an IPv6 address): a host that any parser
    // reads the same way, so that a backslash is not taken for part of a name under a domain.
    let plain = host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':'));
    (!host.is_empty() && plain).then(|| host.to_ascii_lowercase())
}

/// An address the owner gave that is not an e-mail address a domain can be taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressError;

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("that is not an e-mail address a server can be looked for from; type the server's address in Server settings")
    }
}

impl std::error::Error for AddressError {}

/// `address` as `(address, domain)`: one `@`, a local part without space, control character or
/// angle bracket, a domain that is a name.
fn split_address(address: &str) -> Option<(String, String)> {
    let address = address.trim();
    let (local, domain) = address.rsplit_once('@')?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let local_ok =
        !local.is_empty() && local.len() <= 64 && !local.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | '@'));
    (local_ok && is_name(&domain)).then(|| (format!("{local}@{domain}"), domain))
}

/// What the owner is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentReason {
    /// The search was sent on to another host (a redirect, an SRV record): the password would go there.
    Redirect,
    /// The answer asks to start again with an address in another mail domain.
    OtherDomain,
    /// The answer gives an Exchange Web Services address on another host: the password would go there.
    MailServer,
}

/// A question for the owner: may the sign-in go to `host`? The host asks, naming it, and
/// calls [`Autodiscover::agree`] or [`Autodiscover::decline`].
#[derive(Clone, PartialEq, Eq)]
pub struct Consent {
    host: String,
    reason: ConsentReason,
}

impl Consent {
    /// The host, or the mail domain, in question (lower case).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Why it is asked.
    pub fn reason(&self) -> ConsentReason {
        self.reason
    }
}

impl fmt::Debug for Consent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Consent").field("reason", &self.reason).finish()
    }
}

/// The Exchange Web Services addresses an answer gave, and the server it names.
#[derive(Clone, PartialEq, Eq)]
pub struct Discovered {
    external: Option<String>,
    internal: Option<String>,
    server: Option<String>,
}

impl Discovered {
    /// The `https://` addresses of the service to try, in order: the external one (Outlook
    /// Anywhere's, which any network reaches) and then the internal one (the organisation's
    /// own network's). At least one; never twice the same.
    pub fn urls(&self) -> Vec<&str> {
        let mut urls: Vec<&str> = Vec::new();
        for url in [self.external.as_deref(), self.internal.as_deref()].into_iter().flatten() {
            if !urls.iter().any(|have| have.eq_ignore_ascii_case(url)) {
                urls.push(url);
            }
        }
        urls
    }

    /// The external address, when the answer gave one.
    pub fn external_url(&self) -> Option<&str> {
        self.external.as_deref()
    }

    /// The internal address, when the answer gave one.
    pub fn internal_url(&self) -> Option<&str> {
        self.internal.as_deref()
    }

    /// The name of the mailbox's server, when the answer gave one that is a host name.
    pub fn server(&self) -> Option<&str> {
        self.server.as_deref()
    }

    fn hosts(&self) -> Vec<String> {
        self.urls().into_iter().filter_map(host_of).chain(self.server.iter().cloned()).collect()
    }
}

impl fmt::Debug for Discovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discovered")
            .field("external", &self.external.is_some())
            .field("internal", &self.internal.is_some())
            .field("server", &self.server.is_some())
            .finish()
    }
}

/// Why nothing was found. Each is a sentence the owner can act on, and each offers Server
/// settings; none holds a word a server wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// Nobody answered at the places looked at.
    Unreachable,
    /// There is a server, but it has no Autodiscover (404 and the like).
    NoAutodiscover,
    /// A secure connection could not be set up: the certificate, for example.
    SecureConnection,
    /// The answer was not an Autodiscover answer that can be read.
    Unreadable,
    /// A redirect to an address that is not `https://`.
    InsecureRedirect,
    /// The server or the organisation refused (403, an error in the answer).
    ServerRefused,
    /// The answer has no Exchange Web Services address.
    NoExchangeAddress,
    /// The server offers to sign in only in ways that are not built yet (NTLM, Negotiate), and
    /// not with a user name and password over TLS (Basic).
    SignInNotSupported,
    /// The server offers NTLM and not Basic: the search can't sign in that way (it speaks Basic
    /// only), but a typed server address is signed in to with NTLM.
    SignInNtlmOnly,
    /// The owner did not agree to a host.
    Declined,
    /// The user name or password was not accepted (401). Nothing else was asked.
    SignInRefused,
    /// More redirects in a row than [`MAX_REDIRECTS`].
    TooManyRedirects,
}

impl Failure {
    /// Which of two reasons says more: the one kept when several steps failed.
    fn rank(self) -> u8 {
        match self {
            Self::Unreachable => 0,
            Self::NoAutodiscover => 1,
            Self::SecureConnection => 2,
            Self::Unreadable => 3,
            Self::InsecureRedirect => 4,
            Self::ServerRefused => 5,
            Self::SignInNotSupported => 6,
            Self::SignInNtlmOnly => 6,
            Self::NoExchangeAddress => 7,
            Self::Declined => 8,
            Self::SignInRefused => 9,
            Self::TooManyRedirects => 10,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "No server answered at the usual places for this address. Check the network connection, or type the server's address in Server settings.",
            Self::NoAutodiscover => "The domain's servers have no automatic setup. Type the server's address in Server settings.",
            Self::SecureConnection => "A secure connection to the domain's server could not be set up, for example because its certificate is not trusted. Type the server's address in Server settings.",
            Self::Unreadable => "The server's answer for automatic setup could not be read. Type the server's address in Server settings.",
            Self::InsecureRedirect => "The server sent the setup somewhere that is not encrypted, which is never followed. Type the server's address in Server settings.",
            Self::ServerRefused => "The server or the organisation would not give its settings. Ask your administrator for the server's address and type it in Server settings.",
            Self::NoExchangeAddress => "The server answered without the address of Exchange Web Services. Ask your administrator for it and type it in Server settings.",
            Self::SignInNotSupported => "The server wants a way of signing in (Kerberos) that this program cannot do yet. Ask your administrator whether the server can take a user name and password over an encrypted connection.",
            Self::SignInNtlmOnly => "Automatic setup can't sign in to this server. Type its address in Server settings, and MailCraft will sign in with NTLM.",
            Self::Declined => "You did not agree to send your sign-in to the server the setup led to. Type the server's address in Server settings.",
            Self::SignInRefused => "The server did not accept the user name and password. Check them, or type the user name in Server settings (DOMAIN\\user or user@example.org).",
            Self::TooManyRedirects => "The setup was sent from one server to another too many times. Type the server's address in Server settings.",
        })
    }
}

impl std::error::Error for Failure {}

/// How a search ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Found an Exchange Web Services address.
    Found(Discovered),
    /// The mailbox is in Exchange Online (Microsoft 365): offer to add it as a Microsoft
    /// account. No password was sent there.
    ExchangeOnline,
    /// Nothing found, and why.
    Failed(Failure),
}

/// The name of an SRV record to look up (`_autodiscover._tcp.example.org`). Its `Debug` shows
/// nothing: the domain is the owner's.
#[derive(Clone, PartialEq, Eq)]
pub struct SrvName(String);

impl SrvName {
    /// The name to look up.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SrvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SrvName(<redacted>)")
    }
}

/// What to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Perform this request, then call [`Autodiscover::response`] with what came of it. It
    /// follows no redirect itself.
    Request(HttpRequest),
    /// Look up the SRV record with this name (`_autodiscover._tcp.example.org`), then call
    /// [`Autodiscover::srv`] with the targets it names (host and port), best first, or `None` if
    /// the lookup failed. Only the domain goes out.
    LookupSrv(SrvName),
    /// Ask the owner, then call [`Autodiscover::agree`] or [`Autodiscover::decline`].
    NeedsYes(Consent),
    /// Finished.
    Done(Outcome),
}

/// What the machine waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Waiting {
    Idle,
    Request(HttpRequest),
    Srv(SrvName),
    Consent(Consent, Pending),
    Done(Outcome),
}

/// What an agreement lets go on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Request(String),
    Domain,
    Settings(Discovered),
}

/// A place to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Url(String),
    Srv,
}

/// The search for an Exchange server's address. See the [module](self).
pub struct Autodiscover {
    authorization: Secret,
    email: String,
    domain: String,
    /// The owner's own mail domain: hosts within it get the sign-in.
    own: String,
    /// Mail domains and hosts the owner agreed to.
    domains: Vec<String>,
    hosts: Vec<String>,
    queue: VecDeque<Target>,
    waiting: Waiting,
    chain: u8,
    requests: u8,
    reason: Option<Failure>,
}

impl fmt::Debug for Autodiscover {
    /// Nothing: an address, a domain and the sign-in are all the owner's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Autodiscover").field("requests", &self.requests).finish()
    }
}

impl Autodiscover {
    /// A search for the server of `email`, signing in with `authorization`
    /// ([`Secret::basic`]). Nothing is sent until the host performs the first [`Step`].
    pub fn new(email: &str, authorization: Secret) -> Result<Self, AddressError> {
        let (email, domain) = split_address(email).ok_or(AddressError)?;
        let queue = candidates(&domain);
        // Microsoft's own mail is no Exchange server's: nothing is sent to it.
        let waiting = if is_microsoft_domain(&domain) { Waiting::Done(Outcome::ExchangeOnline) } else { Waiting::Idle };
        Ok(Self {
            authorization,
            email,
            own: domain.clone(),
            domain,
            domains: Vec::new(),
            hosts: Vec::new(),
            queue,
            waiting,
            chain: 0,
            requests: 0,
            reason: None,
        })
    }

    /// The host has looked up the domain's mail exchangers before starting and they are
    /// Microsoft's ([`is_exchange_online_mx`]): the mailbox is in Exchange Online, a Microsoft
    /// account on its own domain, and the search ends with that said. Nothing is sent. (Ignored
    /// once the search has begun.)
    pub fn note_mail_exchangers(&mut self, hosts: &[String]) {
        if self.requests == 0
            && matches!(self.waiting, Waiting::Idle)
            && hosts.iter().any(|host| is_exchange_online_mx(host.trim().trim_end_matches('.')))
        {
            self.finish(Outcome::ExchangeOnline);
        }
    }

    /// The SRV name the third step looks up, for the domain being asked about.
    fn srv_name(&self) -> SrvName {
        SrvName(format!("_autodiscover._tcp.{}", self.domain))
    }

    /// What to do next. Asked again before the host has answered, it says the same.
    pub fn step(&mut self) -> Step {
        if matches!(self.waiting, Waiting::Idle) {
            self.advance();
        }
        match &self.waiting {
            Waiting::Request(request) => Step::Request(request.clone()),
            Waiting::Srv(name) => Step::LookupSrv(name.clone()),
            Waiting::Consent(consent, _) => Step::NeedsYes(consent.clone()),
            Waiting::Done(outcome) => Step::Done(outcome.clone()),
            // (`advance` always leaves the machine waiting for something.)
            Waiting::Idle => Step::Done(Outcome::Failed(Failure::Unreachable)),
        }
    }

    /// Whether the sign-in may go to `host`: it is the owner's own domain's, or the owner agreed.
    fn allowed(&self, host: &str) -> bool {
        is_within(host, &self.own) || self.domains.iter().any(|domain| is_within(host, domain)) || self.hosts.iter().any(|agreed| agreed == host)
    }

    fn finish(&mut self, outcome: Outcome) {
        self.waiting = Waiting::Done(outcome);
    }

    /// A step failed for `why`: remember the most telling reason, and go on to the next place.
    fn fail_step(&mut self, why: Failure) {
        if self.reason.is_none_or(|have| why.rank() > have.rank()) {
            self.reason = Some(why);
        }
        self.chain = 0;
        self.waiting = Waiting::Idle;
    }

    /// Set `waiting` to the next thing to do.
    fn advance(&mut self) {
        // Each round takes one place off the queue; the rounds are bounded by the requests.
        for _ in 0..=usize::from(MAX_REQUESTS).saturating_mul(4) {
            if self.requests >= MAX_REQUESTS {
                return self.finish(Outcome::Failed(self.reason.unwrap_or(Failure::TooManyRedirects)));
            }
            let Some(target) = self.queue.pop_front() else {
                return self.finish(Outcome::Failed(self.reason.unwrap_or(Failure::NoAutodiscover)));
            };
            match target {
                Target::Srv => {
                    self.waiting = Waiting::Srv(self.srv_name());
                    return;
                }
                Target::Url(url) => {
                    let Some(host) = host_of(&url) else {
                        self.fail_step(Failure::Unreadable);
                        continue;
                    };
                    if is_exchange_online(&host) {
                        return self.finish(Outcome::ExchangeOnline);
                    }
                    if !self.allowed(&host) {
                        self.waiting = Waiting::Consent(Consent { host, reason: ConsentReason::Redirect }, Pending::Request(url));
                        return;
                    }
                    match self.request_to(&url, false) {
                        Some(request) => {
                            self.requests = self.requests.saturating_add(1);
                            self.waiting = Waiting::Request(request);
                            return;
                        }
                        None => self.fail_step(Failure::Unreadable),
                    }
                }
            }
        }
        self.finish(Outcome::Failed(self.reason.unwrap_or(Failure::Unreachable)));
    }

    /// The POX request to `url`. Never to anything but `https://`. It carries the sign-in only
    /// when `authenticated`, which is only in answer to a host's own `401` offering Basic.
    fn request_to(&self, url: &str, authenticated: bool) -> Option<HttpRequest> {
        if !url.get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://")) {
            return None;
        }
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><Autodiscover xmlns=\"{REQUEST_SCHEMA}\"><Request><EMailAddress>{}</EMailAddress><AcceptableResponseSchema>{RESPONSE_SCHEMA}</AcceptableResponseSchema></Request></Autodiscover>",
            xml::escape_text(&self.email)
        );
        let mut fields = vec![("Content-Type", "text/xml; charset=utf-8"), ("Accept", "text/xml")];
        if authenticated {
            fields.push(("Authorization", self.authorization.expose()));
        }
        HttpRequest::new(Method::Post, url, &fields, body.into_bytes()).ok().map(|request| request.with_response_limit(MAX_ANSWER))
    }

    /// The host has performed the [`Step::Request`]: this is what came of it. (A call at any
    /// other time is ignored.)
    pub fn response(&mut self, result: Result<HttpResponse, TransportError>) {
        if !matches!(self.waiting, Waiting::Request(_)) {
            return;
        }
        let Waiting::Request(request) = std::mem::replace(&mut self.waiting, Waiting::Idle) else {
            return;
        };
        match result {
            Err(TransportError::Tls) => self.fail_step(Failure::SecureConnection),
            Err(TransportError::RedirectRefused) => self.fail_step(Failure::NoAutodiscover),
            Err(_) => self.fail_step(Failure::Unreachable),
            Ok(response) if response.status() == 401 => self.challenged(&request, &response),
            Ok(response) => self.judge(request.url(), &response),
        }
    }

    /// A host answered `401`. Asked without a sign-in, that is a challenge: when it offers Basic,
    /// the same request is made again with the sign-in, to that same host (which the owner's
    /// domain or the owner's yes has allowed). Asked with one, it is a refusal of the user name
    /// and password, and the search ends at once: asking other hosts with a wrong password is
    /// how an account gets locked.
    fn challenged(&mut self, request: &HttpRequest, response: &HttpResponse) {
        if request.field("authorization").is_some() {
            return self.finish(Outcome::Failed(Failure::SignInRefused));
        }
        // Each header offers one scheme or several, separated by commas: the first word of each piece.
        let offers = |scheme: &str| {
            response
                .headers_named("www-authenticate")
                .flat_map(|value| value.split(','))
                .any(|piece| piece.split_whitespace().next().is_some_and(|have| have.eq_ignore_ascii_case(scheme)))
        };
        if offers("basic") {
            match self.request_to(request.url(), true) {
                Some(again) if self.requests < MAX_REQUESTS => {
                    self.requests = self.requests.saturating_add(1);
                    self.waiting = Waiting::Request(again);
                }
                _ => self.fail_step(Failure::Unreadable),
            }
        } else if offers("ntlm") {
            self.fail_step(Failure::SignInNtlmOnly);
        } else if offers("negotiate") {
            self.fail_step(Failure::SignInNotSupported);
        } else {
            // A site that wants some other sign-in is not an Autodiscover that takes ours.
            self.fail_step(Failure::NoAutodiscover);
        }
    }

    fn judge(&mut self, url: &str, response: &HttpResponse) {
        match response.status() {
            200 => {
                if response.body().len() > MAX_ANSWER {
                    return self.fail_step(Failure::Unreadable);
                }
                match read_answer(response.body()) {
                    Answer::Settings(found) => self.settings(found),
                    Answer::RedirectUrl(next) => self.follow(&next),
                    Answer::RedirectAddr(email) => self.restart(&email),
                    Answer::Refused => self.fail_step(Failure::ServerRefused),
                    Answer::Unreadable => self.fail_step(Failure::Unreadable),
                }
            }
            301 | 302 | 303 | 307 | 308 => match response.header("location").and_then(|location| resolve(url, location)) {
                Some(next) => self.follow(&next),
                None => self.fail_step(Failure::InsecureRedirect),
            },
            403 => self.fail_step(Failure::ServerRefused),
            _ => self.fail_step(Failure::NoAutodiscover),
        }
    }

    /// One redirect to `url`, which is an `https://` address.
    fn follow(&mut self, url: &str) {
        if self.chain >= MAX_REDIRECTS {
            return self.finish(Outcome::Failed(Failure::TooManyRedirects));
        }
        self.chain = self.chain.saturating_add(1);
        if url.len() > MAX_URL || !url.get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://")) || check_endpoint(url).is_err() {
            return self.fail_step(Failure::InsecureRedirect);
        }
        self.queue.push_front(Target::Url(url.to_owned()));
        self.waiting = Waiting::Idle;
    }

    /// `redirectAddr`: start again with another address, which may be in another mail domain.
    fn restart(&mut self, email: &str) {
        if self.chain >= MAX_REDIRECTS {
            return self.finish(Outcome::Failed(Failure::TooManyRedirects));
        }
        let Some((email, domain)) = split_address(email) else {
            return self.fail_step(Failure::Unreadable);
        };
        self.chain = self.chain.saturating_add(1);
        self.queue = candidates(&domain);
        self.email = email;
        self.domain = domain.clone();
        self.waiting = if is_microsoft_domain(&domain) {
            Waiting::Done(Outcome::ExchangeOnline)
        } else if self.allowed(&domain) {
            Waiting::Idle
        } else {
            Waiting::Consent(Consent { host: domain, reason: ConsentReason::OtherDomain }, Pending::Domain)
        };
    }

    /// The answer gave settings: Exchange Online, or the owner's say about each new host, or done.
    fn settings(&mut self, found: Discovered) {
        if found.hosts().iter().any(|host| is_exchange_online(host)) {
            return self.finish(Outcome::ExchangeOnline);
        }
        if found.urls().is_empty() {
            return self.fail_step(Failure::NoExchangeAddress);
        }
        // The password goes to the addresses, not to the name of the mailbox's server.
        match found.urls().into_iter().filter_map(host_of).find(|host| !self.allowed(host)) {
            Some(host) => self.waiting = Waiting::Consent(Consent { host, reason: ConsentReason::MailServer }, Pending::Settings(found)),
            None => self.finish(Outcome::Found(found)),
        }
    }

    /// The host has looked up the SRV record asked for: the targets it names (host and port),
    /// best first, or `None` when the lookup failed. Whatever is given is checked: names only, at
    /// most three, and a port only if it is 443 or 1024 and above (the port of an SRV record
    /// names where the service listens: MS-OXDSCLI asks there).
    pub fn srv(&mut self, hosts: Option<Vec<(String, u16)>>) {
        if !matches!(self.waiting, Waiting::Srv(_)) {
            return;
        }
        self.waiting = Waiting::Idle;
        let mut named: Vec<(String, u16)> = Vec::new();
        for (host, port) in hosts.unwrap_or_default() {
            let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
            let port = if port == 0 { 443 } else { port };
            let target = (host, port);
            if is_name(&target.0) && (port == 443 || port >= 1024) && !named.contains(&target) && named.len() < MAX_SRV_HOSTS {
                named.push(target);
            }
        }
        if named.is_empty() {
            return self.fail_step(Failure::Unreachable);
        }
        for (host, port) in named.into_iter().rev() {
            let port = if port == 443 { String::new() } else { format!(":{port}") };
            self.queue.push_front(Target::Url(format!("https://{host}{port}/autodiscover/autodiscover.xml")));
        }
    }

    /// The owner agrees to the host the machine asked about. (A call at any other time is ignored.)
    pub fn agree(&mut self) {
        if !matches!(self.waiting, Waiting::Consent(..)) {
            return;
        }
        let Waiting::Consent(consent, pending) = std::mem::replace(&mut self.waiting, Waiting::Idle) else {
            return;
        };
        match consent.reason {
            ConsentReason::OtherDomain => self.domains.push(consent.host),
            ConsentReason::Redirect | ConsentReason::MailServer => self.hosts.push(consent.host),
        }
        match pending {
            Pending::Request(url) => self.queue.push_front(Target::Url(url)),
            Pending::Domain => {}
            Pending::Settings(found) => self.settings(found),
        }
    }

    /// The owner does not agree: nothing is sent to that host, and the search goes on elsewhere.
    pub fn decline(&mut self) {
        if let Waiting::Consent(consent, _) = &self.waiting {
            // The places left to ask were all in the domain that was declined.
            if consent.reason == ConsentReason::OtherDomain {
                self.queue.clear();
            }
            self.waiting = Waiting::Idle;
            self.fail_step(Failure::Declined);
        }
    }

    /// Drives the whole search with the host's ways of doing things: `exchange` performs a
    /// request, `srv` looks a name up (the hosts it names, or `None`), and `ask` asks the owner
    /// (true is yes). The host that must wait for a person calls [`step`](Self::step) itself
    /// instead.
    pub async fn run<E: HttpExchange>(
        &mut self,
        exchange: &E,
        mut srv: impl FnMut(&str) -> Option<Vec<(String, u16)>>,
        mut ask: impl FnMut(&Consent) -> bool,
    ) -> Outcome {
        // Every round is a request, an SRV lookup or a question, and each is bounded.
        for _ in 0..usize::from(MAX_REQUESTS).saturating_mul(8) {
            match self.step() {
                Step::Request(request) => {
                    let result = exchange.exchange(&request).await;
                    self.response(result);
                }
                Step::LookupSrv(name) => {
                    let hosts = srv(name.as_str());
                    self.srv(hosts);
                }
                Step::NeedsYes(consent) => {
                    if ask(&consent) {
                        self.agree();
                    } else {
                        self.decline();
                    }
                }
                Step::Done(outcome) => return outcome,
            }
        }
        Outcome::Failed(self.reason.unwrap_or(Failure::TooManyRedirects))
    }
}

/// The places to ask for `domain`, in order.
fn candidates(domain: &str) -> VecDeque<Target> {
    VecDeque::from([
        Target::Url(format!("https://{domain}/autodiscover/autodiscover.xml")),
        Target::Url(format!("https://autodiscover.{domain}/autodiscover/autodiscover.xml")),
        Target::Srv,
    ])
}

/// Where a `Location` leads from `current`: an absolute `https://` address, or a path on the same
/// host. Anything else (plain `http://`, another scheme, a relative path without a slash, a
/// protocol-relative address) is not followed.
fn resolve(current: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.is_empty() || location.len() > MAX_URL || location.chars().any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii()) {
        return None;
    }
    let absolute = location.get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    if absolute {
        return check_endpoint(location).is_ok().then(|| location.to_owned());
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let rest = current.get(8..)?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        return Some(format!("https://{authority}{location}"));
    }
    None
}

/// What an Autodiscover answer says.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Settings(Discovered),
    RedirectUrl(String),
    RedirectAddr(String),
    /// An `Error` element: the service says no.
    Refused,
    Unreadable,
}

/// The text of `node`'s child `name`, trimmed; empty when there is none.
fn text(node: &Node, name: &str) -> String {
    node.text_of(name).to_owned()
}

/// An Exchange Web Services address from an answer: `https://` only, no user name in it.
fn ews_address(text: &str) -> Option<String> {
    let text = text.trim();
    let secure = text.get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    (secure && text.len() <= MAX_URL && check_endpoint(text).is_ok() && host_of(text).is_some()).then(|| text.to_owned())
}

/// Reads an answer body. Takes `Action`, `RedirectUrl`, `RedirectAddr`, and from `Protocol`
/// elements of type `EXCH` (internal) and `EXPR` (external) `EwsUrl` and `Server`: nothing else.
fn read_answer(body: &[u8]) -> Answer {
    let Ok(root) = xml::parse(body) else { return Answer::Unreadable };
    if root.name != "Autodiscover" || *root.namespace != *ANSWER_SCHEMA {
        return Answer::Unreadable;
    }
    let Some(response) = root.child("Response") else { return Answer::Unreadable };
    let account = response.child("Account");
    if response.child("Error").is_some() || account.is_some_and(|account| account.child("Error").is_some()) {
        return Answer::Refused;
    }
    let Some(account) = account.filter(|_| *response.namespace == *RESPONSE_SCHEMA) else { return Answer::Unreadable };
    match text(account, "Action").as_str() {
        "redirectUrl" => {
            let url = text(account, "RedirectUrl");
            return if url.is_empty() { Answer::Unreadable } else { Answer::RedirectUrl(url) };
        }
        "redirectAddr" => {
            let address = text(account, "RedirectAddr");
            return if address.is_empty() { Answer::Unreadable } else { Answer::RedirectAddr(address) };
        }
        "settings" => {}
        _ => return Answer::Unreadable,
    }
    let (mut internal, mut external, mut server) = (None, None, None);
    for protocol in account.all("Protocol") {
        let slot = match protocol.text_of("Type") {
            "EXCH" => &mut internal,
            "EXPR" => &mut external,
            _ => continue,
        };
        if slot.is_none() {
            *slot = ews_address(&text(protocol, "EwsUrl"));
        }
        if server.is_none() {
            let name = text(protocol, "Server").trim_end_matches('.').to_ascii_lowercase();
            server = is_name(&name).then_some(name);
        }
    }
    Answer::Settings(Discovered { external, internal, server })
}

#[cfg(test)]
#[path = "autodiscover_tests.rs"]
mod autodiscover_tests;
