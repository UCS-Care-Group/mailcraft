// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's half of reading an account's calendars from a CalDAV server (#209, spec C5 and C9):
//! `PROPFIND`, `REPORT` and `GET` over the crate's TLS (`https`), the requests and answers
//! of `mailcraft-dav`, and the decisions of `sync.davRequest` (what to ask) carried out; what came
//! back is a [`DavDownload`], which the engine takes in (`sync.applyDav`).
//!
//! The rules, each held where a request is made ([`Run::call`]):
//!
//! - **The password is per origin and goes only in answer to that origin's own `Basic` challenge.**
//!   Every request starts without a credential. A 401 that offers `Basic` is answered, once, with
//!   the password the owner's settings hold for *that origin* (the caller's closure is asked for it
//!   by origin, and only when a challenge came); a 401 to the credential is "wrong password" and
//!   stops the run (no other host is tried: lockouts). After a challenge the same origin gets the
//!   credential up front for the rest of the run. An origin that is not one of the account's homes
//!   is never asked, so never given anything.
//! - **TLS, and the pinned certificate.** An `https://` address (a plain `http://` one only to this
//!   machine, for tests), with the one certificate the owner trusted when there is one
//!   ([`crate::https`]); a certificate the system doesn't trust is refused before any request.
//! - **Redirects** are followed one at a time, at most [`MAX_REDIRECTS`], to an address on the
//!   same origin only (the address is resolved against it, [`mailcraft_dav::href::Origin::resolve`]):
//!   a redirect to another origin ends the run with [`DavProblem::Redirected`] and the credential
//!   goes nowhere.
//! - **Every address the server names** is resolved against the collection it came in, must be on
//!   the origin and inside the collection, or it is left unread and counted.
//! - **Microsoft's and Google's hosts** are never contacted ([`mailcraft_model::DavServer::check_home`],
//!   checked for every request).
//! - **Everything is capped:** requests and raw bytes for a run, the time it takes, pages for a
//!   collection, resources for a collection, documents per request. A run that reaches a limit
//!   returns what it has, says what was cut short and goes on at the next.
//!
//! Nothing of an address, a name, a user name or a password is logged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mailcraft_dav::answer::{Collections, Entry, Multistatus};
use mailcraft_dav::href::{Origin, TrustedOrigin};
use mailcraft_dav::http::{HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};
use mailcraft_dav::request::{Endpoint, Kind, MAX_MULTIGET, MAX_PAGE, RequestError, Stamp};
use mailcraft_dav::status::{self, Verdict};
use mailcraft_model::dav_down::{MAX_CHANGES, MAX_READ, MAX_RESOURCES};
use mailcraft_model::{DavAsk, DavAskCollection, DavCalendar, DavChange, DavDownload, DavListing, DavProblem, DavRead, DavReadState, DavServer, Rgb};

use crate::NetError;
use crate::cert::CertificateProblem;
use crate::https;

/// Most redirects followed for one request.
pub const MAX_REDIRECTS: usize = 5;

/// What one run may spend.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Requests, redirects and the answer to a challenge included.
    pub requests: usize,
    /// Bytes of documents taken in.
    pub raw_bytes: usize,
    /// Pages of one collection's changes.
    pub pages: usize,
    /// Documents asked for one by one, for a collection (what a multiget left out).
    pub gets: usize,
    /// The whole run, from the first request.
    pub patience: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { requests: 1_500, raw_bytes: 48 * 1024 * 1024, pages: 50, gets: 100, patience: Duration::from_secs(300) }
    }
}

/// A password for an origin (`mailcraft_model::origin_of` form), asked for only when that origin
/// has challenged.
pub type Passwords<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The real wire: `https::send` with the owner's pinned certificate. A certificate that isn't
/// accepted is kept for the window to show ([`Wire::certificate`]).
pub struct Wire {
    trusted: String,
    certificate: Mutex<Option<CertificateProblem>>,
}

impl Wire {
    /// A wire that accepts the certificates the system does and the one `trusted` names (a SHA-256
    /// fingerprint), or only the system's when it is empty.
    pub fn new(trusted: &str) -> Wire {
        Wire { trusted: trusted.trim().to_string(), certificate: Mutex::new(None) }
    }

    /// The problem with the last certificate that was refused, once.
    pub fn certificate(&self) -> Option<CertificateProblem> {
        self.certificate.lock().ok().and_then(|mut held| held.take())
    }
}

impl HttpExchange for Wire {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let fields: Vec<(&str, &str)> = request.fields().collect();
        let sent = https::Request {
            method: request.method().as_str(),
            url: request.url(),
            fields: &fields,
            body: request.body(),
            max_response: request.max_response_bytes(),
            trusted: &self.trusted,
        };
        match https::send(&sent) {
            Ok(answer) => Ok(response_of(answer)),
            Err(NetError::Certificate(problem)) => {
                if let Ok(mut held) = self.certificate.lock() {
                    *held = Some(*problem);
                }
                Err(TransportError::Certificate)
            }
            Err(NetError::Connect { .. }) => Err(TransportError::Unreachable),
            Err(other) if crate::http::is_too_long(&other) => Err(TransportError::BodyTooLarge),
            // (Nothing of the error: it may name the server.)
            Err(_) => Err(TransportError::Failed),
        }
    }
}

fn response_of(answer: https::Response) -> HttpResponse {
    let mut response = HttpResponse::new(answer.status, answer.body);
    if let Some(location) = &answer.location {
        response = response.with_field("location", location);
    }
    for challenge in &answer.challenges {
        response = response.with_field("www-authenticate", challenge);
    }
    if let Some(wait) = &answer.retry_after {
        response = response.with_field("retry-after", wait);
    }
    if !answer.dav.is_empty() {
        response = response.with_field("dav", &answer.dav.join(", "));
    }
    if let Some(etag) = &answer.etag {
        response = response.with_field("etag", etag);
    }
    response
}

/// Read what `ask` says from the real server: the job a host runs on a thread of its own. Besides
/// the download, the certificate that was refused, when that is what stopped it.
pub fn look(ask: &DavAsk, passwords: Passwords<'_>) -> (DavDownload, Option<CertificateProblem>) {
    let wire = Wire::new(&ask.trusted);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let download = run(ask, passwords, &wire, now, &Limits::default());
    log::info!("calendar server: {} collections read, {:?}", download.collections.len(), download.problem);
    (download, wire.certificate())
}

/// Why a step stopped.
enum Fail {
    /// The run ends: what it has is returned.
    Stop(DavProblem),
    /// This collection (or home) can't be read just now; the run goes on.
    Skip,
}

/// One origin the account's homes are on.
struct Site {
    key: String,
    origin: Origin,
    trusted: TrustedOrigin,
    anon: Endpoint,
    authed: Option<Endpoint>,
    challenged: bool,
}

/// What a listing says of one calendar besides what the engine keeps.
struct Found {
    calendar: DavCalendar,
    ctag: Option<String>,
    token: Option<String>,
    /// Whether the server offers `sync-collection` for it, when it said what it offers (a
    /// listing without `supported-report-set` leaves it to be tried).
    sync: Option<bool>,
}

/// How a pass over a collection's changes ended.
enum Pass {
    /// To its end.
    Done,
    /// The token isn't known to the server any more.
    TokenInvalid,
    /// The server doesn't do `sync-collection` for it.
    NoSync,
    /// Stopped short (pages, resources or a document that couldn't be had): what was read stands.
    Short,
}

struct Run<'a> {
    x: &'a dyn HttpExchange,
    user: String,
    passwords: Passwords<'a>,
    limits: Limits,
    started: Instant,
    now: i64,
    sites: Vec<Site>,
    requests: usize,
    raw: usize,
    signed_in: Vec<String>,
    skipped: u32,
    wait: u64,
}

/// Read what `ask` says through `x`. The passwords are asked for by origin, and only when that
/// origin challenges ([`Passwords`]). `now` is Unix seconds (for a `Retry-After` date).
pub fn run(ask: &DavAsk, passwords: Passwords<'_>, x: &dyn HttpExchange, now: i64, limits: &Limits) -> DavDownload {
    let mut out = DavDownload { account: ask.account, email: ask.email.clone(), ..Default::default() };
    let mut run = Run {
        x,
        user: ask.username.clone(),
        passwords,
        limits: limits.clone(),
        started: Instant::now(),
        now,
        sites: Vec::new(),
        requests: 0,
        raw: 0,
        signed_in: Vec::new(),
        skipped: 0,
        wait: 0,
    };
    // A site for each origin a home of the settings is on, and for none else.
    let mut homes: Vec<&String> = Vec::new();
    // Whether a home was left out: what is listed is then not all there is.
    let mut left_out = ask.homes.len() > DavServer::MAX_HOMES;
    for home in ask.homes.iter().take(DavServer::MAX_HOMES) {
        if DavServer::check_home(home).is_err() {
            run.skipped = run.skipped.saturating_add(1);
            left_out = true;
            continue;
        }
        if run.add_site(home) {
            homes.push(home);
        } else {
            left_out = true;
        }
    }
    if homes.is_empty() {
        out.problem = DavProblem::NotCalendarServer;
        return out;
    }
    let ended = run.work(ask, &homes, left_out, &mut out);
    if let Err(Fail::Stop(problem)) = ended {
        out.problem = problem;
    }
    out.wait = run.wait;
    out.signed_in = run.signed_in;
    out.skipped = run.skipped;
    out
}

impl Run<'_> {
    fn add_site(&mut self, url: &str) -> bool {
        let Some(key) = mailcraft_model::origin_of(url) else { return false };
        if self.sites.iter().any(|site| site.key == key) {
            return true;
        }
        let (Ok(origin), Ok(trusted)) = (Origin::parse(url), TrustedOrigin::from_account_settings(url)) else { return false };
        let anon = Endpoint::new(trusted.clone(), None);
        self.sites.push(Site { key, origin, trusted, anon, authed: None, challenged: false });
        true
    }

    fn site_of(&self, url: &str) -> Option<usize> {
        let key = mailcraft_model::origin_of(url)?;
        self.sites.iter().position(|site| site.key == key)
    }

    fn work(&mut self, ask: &DavAsk, homes: &[&String], left_out: bool, out: &mut DavDownload) -> Result<(), Fail> {
        // The calendars of the homes, when they are to be listed.
        let mut found: Vec<Found> = Vec::new();
        let mut stopped = None;
        if ask.list_homes {
            let mut listing = DavListing { complete: !left_out, ..Default::default() };
            for home in homes {
                match self.list_home(home) {
                    Ok(listed) => {
                        listing.capped |= listed.capped;
                        for item in listed.found {
                            if found.iter().all(|have| have.calendar.href != item.calendar.href) {
                                if found.len() >= DavServer::MAX_COLLECTIONS {
                                    listing.capped = true;
                                } else {
                                    found.push(item);
                                }
                            }
                        }
                    }
                    Err(Fail::Skip) => listing.complete = false,
                    Err(Fail::Stop(problem)) => {
                        listing.complete = false;
                        stopped = Some(problem);
                        break;
                    }
                }
            }
            listing.complete &= !listing.capped;
            listing.calendars = found.iter().map(|f| f.calendar.clone()).collect();
            out.listing = Some(listing);
        }
        if let Some(problem) = stopped {
            return Err(Fail::Stop(problem));
        }
        // The collections to read: those listed, else those that are known.
        let known: HashMap<&str, &DavAskCollection> = ask.collections.iter().map(|c| (c.href.as_str(), c)).collect();
        let targets: Vec<(DavAskCollection, Option<&Found>)> = if ask.list_homes {
            found
                .iter()
                .map(|f| {
                    (
                        known
                            .get(f.calendar.href.as_str())
                            .map(|c| (*c).clone())
                            .unwrap_or_else(|| DavAskCollection { href: f.calendar.href.clone(), ..Default::default() }),
                        Some(f),
                    )
                })
                .collect()
        } else {
            ask.collections.iter().take(DavServer::MAX_COLLECTIONS).map(|c| (c.clone(), None)).collect()
        };
        for (collection, listed) in &targets {
            let mut read = DavRead { href: collection.href.clone(), complete: true, ..Default::default() };
            let ended = self.read_collection(ask, collection, *listed, &mut read);
            let stop = match ended {
                Ok(()) => None,
                Err(Fail::Skip) => {
                    read.complete = false;
                    None
                }
                Err(Fail::Stop(problem)) => {
                    read.complete = false;
                    Some(problem)
                }
            };
            if read.state != DavReadState::Skipped || !read.changes.is_empty() {
                out.collections.push(read);
            }
            if let Some(problem) = stop {
                return Err(Fail::Stop(problem));
            }
        }
        Ok(())
    }

    /// One request, with its challenge and its redirects. `build` makes the request for an address
    /// on the endpoint it is given.
    fn call(&mut self, site: usize, url: &str, build: &dyn Fn(&Endpoint, &str) -> Result<HttpRequest, RequestError>) -> Result<HttpResponse, Fail> {
        let mut target = url.to_string();
        let mut redirects = 0;
        let mut retried = false;
        loop {
            if self.requests >= self.limits.requests || self.started.elapsed() >= self.limits.patience {
                return Err(Fail::Stop(DavProblem::Limits));
            }
            self.requests += 1;
            let (built, sent_credential) = {
                let Some(held) = self.sites.get(site) else { return Err(Fail::Skip) };
                let endpoint = if held.challenged { held.authed.as_ref().unwrap_or(&held.anon) } else { &held.anon };
                (build(endpoint, &target), held.challenged && held.authed.is_some())
            };
            let request = match built {
                Ok(request) => request,
                Err(_) => {
                    // An address off the origin, or one that is none: left alone.
                    self.skipped = self.skipped.saturating_add(1);
                    return Err(Fail::Skip);
                }
            };
            // Never to Microsoft's or Google's hosts, whatever an answer said.
            if DavServer::check_home(request.url()).is_err() {
                self.skipped = self.skipped.saturating_add(1);
                return Err(Fail::Skip);
            }
            let response = match self.x.exchange(&request) {
                Ok(response) => response,
                Err(TransportError::Certificate) => return Err(Fail::Stop(DavProblem::Certificate)),
                Err(TransportError::Unreachable) => return Err(Fail::Stop(DavProblem::Unreachable)),
                Err(TransportError::BodyTooLarge) => return Err(Fail::Skip),
                // (One more try of what may be asked twice.)
                Err(TransportError::Failed) if request.is_repeatable() && !retried => {
                    retried = true;
                    continue;
                }
                Err(_) => return Err(Fail::Stop(DavProblem::Unreachable)),
            };
            match status::classify(&response, self.now) {
                Verdict::Unauthorized => {
                    if sent_credential {
                        return Err(Fail::Stop(DavProblem::WrongPassword));
                    }
                    if !status::challenges(&response).iter().any(|scheme| scheme == "basic") {
                        return Err(Fail::Stop(DavProblem::UnsupportedSignIn));
                    }
                    self.answer_challenge(site)?;
                }
                Verdict::Redirect(_) => {
                    redirects += 1;
                    let Some(held) = self.sites.get(site) else { return Err(Fail::Skip) };
                    let next = response.header("location").and_then(|location| held.origin.resolve(&target, location).ok());
                    match next {
                        Some(next) if redirects <= MAX_REDIRECTS => target = next,
                        // Another origin, an address that is none, or too many: nothing goes there.
                        _ => return Err(Fail::Stop(DavProblem::Redirected)),
                    }
                }
                Verdict::Wait(seconds) => {
                    self.wait = seconds;
                    return Err(Fail::Stop(DavProblem::Busy));
                }
                _ => {
                    if sent_credential {
                        let key = self.sites.get(site).map(|s| s.key.clone()).unwrap_or_default();
                        if !self.signed_in.contains(&key) {
                            self.signed_in.push(key);
                        }
                    }
                    return Ok(response);
                }
            }
        }
    }

    /// The origin challenged with `Basic`: the credential for it, asked for now and for this origin
    /// alone.
    fn answer_challenge(&mut self, site: usize) -> Result<(), Fail> {
        let Some(held) = self.sites.get(site) else { return Err(Fail::Skip) };
        let Some(password) = (self.passwords)(&held.key) else { return Err(Fail::Stop(DavProblem::NeedsPassword)) };
        let Ok(secret) = Secret::basic(&self.user, &password) else { return Err(Fail::Stop(DavProblem::WrongPassword)) };
        let authed = Endpoint::new(held.trusted.clone(), Some(secret));
        if let Some(held) = self.sites.get_mut(site) {
            held.authed = Some(authed);
            held.challenged = true;
        }
        Ok(())
    }

    /// The multistatus of a 207 answer, or why not.
    fn multistatus(&self, site: usize, response: &HttpResponse, base: &str) -> Result<Multistatus, Fail> {
        let origin = &self.sites.get(site).ok_or(Fail::Skip)?.origin;
        match status::classify(response, self.now) {
            Verdict::Multistatus => Multistatus::parse(response.body(), origin, base).map_err(|_| Fail::Skip),
            Verdict::Forbidden | Verdict::Other(_) | Verdict::PreconditionFailed | Verdict::Gone => Err(Fail::Skip),
            _ => Err(Fail::Skip),
        }
    }

    // ---- listing --------------------------------------------------------------------------

    fn list_home(&mut self, home: &str) -> Result<Listed, Fail> {
        let Some(site) = self.site_of(home) else { return Err(Fail::Skip) };
        let response = self.call(site, home, &|ep, url| ep.propfind_collections(url))?;
        let ms = self.multistatus(site, &response, home)?;
        let skipped = ms.skipped;
        self.skipped = self.skipped.saturating_add(u32::try_from(skipped).unwrap_or(u32::MAX));
        let capped = ms.capped;
        let listed = Collections::from_listing(ms);
        let mut out = Listed { found: Vec::new(), capped: capped || listed.capped };
        for entry in listed.calendars {
            out.found.push(found_of(&entry));
        }
        Ok(out)
    }

    // ---- one collection -------------------------------------------------------------------

    fn read_collection(&mut self, ask: &DavAsk, coll: &DavAskCollection, listed: Option<&Found>, out: &mut DavRead) -> Result<(), Fail> {
        let Some(site) = self.site_of(&coll.href) else {
            // (Not on an origin of the homes: never asked.)
            self.skipped = self.skipped.saturating_add(1);
            return Err(Fail::Skip);
        };
        let (ctag, token) = match listed {
            Some(found) => (found.ctag.clone(), found.token.clone()),
            None => self.state_of(site, &coll.href)?,
        };
        let same_token = !coll.sync_token.is_empty() && token.as_deref() == Some(coll.sync_token.as_str());
        let same_ctag = !coll.ctag.is_empty() && ctag.as_deref() == Some(coll.ctag.as_str());
        if same_token || same_ctag {
            out.state = DavReadState::Unchanged;
            return Ok(());
        }
        out.state = DavReadState::Read;
        out.ctag = String::new();
        let known: HashMap<&str, &str> = coll.known.iter().map(|k| (k.href.as_str(), k.etag.as_str())).collect();
        let sync_known = listed.and_then(|f| f.sync);
        let mut ended = Pass::NoSync;
        if sync_known != Some(false) {
            if !coll.sync_token.is_empty() {
                ended = self.sync_pass(site, coll, &known, Some(coll.sync_token.clone()), out)?;
                if matches!(ended, Pass::TokenInvalid) {
                    // Read afresh and match by address: what came before is not trusted.
                    *out = DavRead { href: coll.href.clone(), complete: true, state: DavReadState::Read, ..Default::default() };
                    ended = self.sync_pass(site, coll, &known, None, out)?;
                }
            } else {
                ended = self.sync_pass(site, coll, &known, None, out)?;
            }
        }
        if matches!(ended, Pass::TokenInvalid | Pass::NoSync) {
            *out = DavRead { href: coll.href.clone(), complete: true, state: DavReadState::Read, ..Default::default() };
            self.query_pass(site, ask, coll, &known, out)?;
        }
        if out.complete {
            out.ctag = ctag.unwrap_or_default();
        }
        Ok(())
    }

    /// The change tag and sync token of a collection, asked for by themselves.
    fn state_of(&mut self, site: usize, href: &str) -> Result<(Option<String>, Option<String>), Fail> {
        let response = self.call(site, href, &|ep, url| ep.propfind_state(url))?;
        // (A server that can't be asked this way is read as if it had changed.)
        let Ok(ms) = self.multistatus(site, &response, href) else { return Ok((None, None)) };
        let own = ms.entries.iter().find(|e| e.is_base).or_else(|| ms.entries.first());
        Ok((own.and_then(|e| e.ctag.clone()), own.and_then(|e| e.sync_token.clone())))
    }

    /// Whether `href` is a resource inside `collection` on its origin: what a member must be.
    fn inside(&self, site: usize, collection: &str, href: &str) -> bool {
        self.sites.get(site).is_some_and(|s| s.origin.is_within(collection, href))
    }

    /// Reads a collection's changes by `sync-collection` (RFC 6578) from `token` (none: the first
    /// read, of everything), page by page.
    fn sync_pass(
        &mut self,
        site: usize,
        coll: &DavAskCollection,
        known: &HashMap<&str, &str>,
        start: Option<String>,
        out: &mut DavRead,
    ) -> Result<Pass, Fail> {
        let delta = start.is_some();
        // (A pass that goes on from a token keeps it until a page is taken in.)
        out.sync_token = start.clone().unwrap_or_default();
        let mut token = start;
        let mut present: BTreeMap<String, String> = BTreeMap::new();
        let mut total = 0usize;
        let mut pages = 0usize;
        let mut gets = 0usize;
        loop {
            // (Whether a deletion of this page found no room: the token then stays where it was.)
            let mut overflow = false;
            pages += 1;
            if pages > self.limits.pages {
                out.complete = false;
                return self.short(out, delta, &present);
            }
            let asked = token.clone();
            let response = self.call(site, &coll.href, &|ep, url| ep.sync_collection(url, asked.as_deref(), Some(MAX_PAGE)))?;
            match status::classify(&response, self.now) {
                Verdict::Multistatus => {}
                Verdict::SyncTokenInvalid if delta || pages > 1 => return Ok(Pass::TokenInvalid),
                // (A server that doesn't do the report, and says so: another way is tried.)
                Verdict::Forbidden | Verdict::Other(_) | Verdict::PreconditionFailed if token.is_none() && pages == 1 => return Ok(Pass::NoSync),
                _ => return Err(Fail::Skip),
            }
            let ms = self.multistatus(site, &response, &coll.href)?;
            self.skipped = self.skipped.saturating_add(u32::try_from(ms.skipped).unwrap_or(u32::MAX));
            let mut page: Vec<(String, String)> = Vec::new();
            for entry in &ms.entries {
                if entry.is_base {
                    continue;
                }
                if !self.inside(site, &coll.href, &entry.href) {
                    self.skipped = self.skipped.saturating_add(1);
                    continue;
                }
                if entry.is_gone() {
                    present.remove(&entry.href);
                    if out.changes.len() < MAX_CHANGES {
                        out.changes.push(DavChange { href: entry.href.clone(), gone: true, ..Default::default() });
                    } else {
                        overflow = true;
                    }
                    continue;
                }
                // (A member without a usable ETag is still a member: it is kept, and asked for.)
                let etag = usable(entry.etag.as_deref());
                total += 1;
                if total > MAX_RESOURCES {
                    // (The first ones stand; what is over is said once, never taken in.)
                    out.capped = true;
                    continue;
                }
                if !delta {
                    present.insert(entry.href.clone(), etag.clone());
                }
                if known.get(entry.href.as_str()) != Some(&etag.as_str()) {
                    page.push((entry.href.clone(), etag.clone()));
                }
            }
            out.capped |= ms.capped;
            let fetched = self.fetch(site, &coll.href, &page, &mut gets, out)?;
            if !fetched || out.capped || overflow {
                out.complete = false;
                return self.short(out, delta, &present);
            }
            if ms.truncated {
                match ms.sync_token {
                    Some(next) if Some(&next) != token.as_ref() => {
                        // (A token between pages is good to go on from only for changes: a first read
                        // that stops short starts over, so that what is gone is found.)
                        if delta {
                            out.sync_token = next.clone();
                        }
                        token = Some(next);
                    }
                    _ => {
                        out.complete = false;
                        return self.short(out, delta, &present);
                    }
                }
                continue;
            }
            out.sync_token = ms.sync_token.unwrap_or_default();
            if !delta {
                out.full = true;
                out.present = present.into_keys().collect();
            }
            return Ok(Pass::Done);
        }
    }

    /// A pass that stopped short: a first read keeps nothing to go on from.
    fn short(&mut self, out: &mut DavRead, delta: bool, present: &BTreeMap<String, String>) -> Result<Pass, Fail> {
        if !delta {
            out.sync_token = String::new();
            out.full = true;
            out.present = present.keys().cloned().collect();
        }
        Ok(Pass::Short)
    }

    /// The fallback for a server that can't say what changed: the events of the stretch, by ETag,
    /// then what is new or different (`calendar-query`, RFC 4791 §7.8).
    fn query_pass(&mut self, site: usize, ask: &DavAsk, coll: &DavAskCollection, known: &HashMap<&str, &str>, out: &mut DavRead) -> Result<(), Fail> {
        let (Some(start), Some(end)) = (stamp(ask.from), stamp(ask.to)) else { return Err(Fail::Skip) };
        let response = self.call(site, &coll.href, &|ep, url| ep.calendar_query(url, &start, &end))?;
        let ms = self.multistatus(site, &response, &coll.href)?;
        self.skipped = self.skipped.saturating_add(u32::try_from(ms.skipped).unwrap_or(u32::MAX));
        let mut present: BTreeMap<String, String> = BTreeMap::new();
        for entry in ms.entries.iter().filter(|e| !e.is_base && !e.is_gone()) {
            if !self.inside(site, &coll.href, &entry.href) {
                self.skipped = self.skipped.saturating_add(1);
                continue;
            }
            let etag = usable(entry.etag.as_deref());
            if present.len() >= MAX_RESOURCES {
                out.capped = true;
                out.complete = false;
                break;
            }
            present.insert(entry.href.clone(), etag);
        }
        out.capped |= ms.capped;
        out.full = true;
        out.window = Some((ask.from, ask.to));
        out.sync_token = String::new();
        out.present = present.keys().cloned().collect();
        if out.capped {
            out.complete = false;
        }
        let wanted: Vec<(String, String)> =
            present.iter().filter(|(href, etag)| known.get(href.as_str()) != Some(&etag.as_str())).map(|(h, e)| (h.clone(), e.clone())).collect();
        let mut gets = 0usize;
        if !self.fetch(site, &coll.href, &wanted, &mut gets, out)? {
            out.complete = false;
        }
        Ok(())
    }

    /// The documents of `wanted` (address, ETag), into `out.changes`: by multiget in batches, and
    /// one by one what a multiget left out or sent too large to hold. False when one couldn't be had.
    fn fetch(&mut self, site: usize, collection: &str, wanted: &[(String, String)], gets: &mut usize, out: &mut DavRead) -> Result<bool, Fail> {
        let mut all = true;
        let etags: HashMap<&str, &str> = wanted.iter().map(|(h, e)| (h.as_str(), e.as_str())).collect();
        let mut single: Vec<String> = Vec::new();
        for batch in wanted.chunks(MAX_MULTIGET) {
            let hrefs: Vec<&str> = batch.iter().map(|(h, _)| h.as_str()).collect();
            let response = self.call(site, collection, &|ep, url| ep.multiget(Kind::Calendar, url, &hrefs))?;
            let ms = match self.multistatus(site, &response, collection) {
                Ok(ms) => ms,
                // (The batch is asked for one by one instead.)
                Err(Fail::Skip) => {
                    single.extend(batch.iter().map(|(h, _)| h.clone()));
                    continue;
                }
                Err(stop) => return Err(stop),
            };
            self.skipped = self.skipped.saturating_add(u32::try_from(ms.skipped).unwrap_or(u32::MAX));
            let mut got: HashSet<String> = HashSet::new();
            for entry in ms.entries.iter().filter(|e| !e.is_base && !e.is_gone()) {
                // Only what was asked for: a server may name others, and none of them is taken.
                let Some(asked) = etags.get(entry.href.as_str()) else {
                    self.skipped = self.skipped.saturating_add(1);
                    continue;
                };
                if let Some(data) = &entry.data {
                    self.take(out, &entry.href, entry.etag.as_deref().unwrap_or(asked), data.clone())?;
                    got.insert(entry.href.clone());
                } else if entry.data_too_large {
                    single.push(entry.href.clone());
                    got.insert(entry.href.clone());
                }
            }
            single.extend(batch.iter().filter(|(h, _)| !got.contains(h)).map(|(h, _)| h.clone()));
        }
        for href in single {
            if *gets >= self.limits.gets {
                all = false;
                break;
            }
            *gets += 1;
            match self.get(site, collection, &href) {
                Ok(Some((etag, body))) => {
                    self.take(out, &href, etag.as_deref().or_else(|| etags.get(href.as_str()).copied()).unwrap_or_default(), body)?
                }
                Ok(None) | Err(Fail::Skip) => all = false,
                Err(stop) => return Err(stop),
            }
        }
        Ok(all)
    }

    /// One document, by `GET`: its ETag and text, when it is one that is read.
    fn get(&mut self, site: usize, collection: &str, href: &str) -> Result<Option<(Option<String>, String)>, Fail> {
        if !self.inside(site, collection, href) {
            return Ok(None);
        }
        let response = self.call(site, href, &|ep, url| ep.get(url))?;
        if !matches!(status::classify(&response, self.now), Verdict::Done) || response.body().len() > MAX_READ {
            return Ok(None);
        }
        let etag = response.header("etag").map(str::to_owned);
        Ok(Some((etag, String::from_utf8_lossy(response.body()).into_owned())))
    }

    /// Keeps one document, within what a run may take in.
    fn take(&mut self, out: &mut DavRead, href: &str, etag: &str, raw: String) -> Result<(), Fail> {
        // (A collection that has more to bring than one run takes goes on at the next, which is
        // told what is here already; the whole run's bytes end it.)
        if out.changes.len() >= MAX_CHANGES {
            return Err(Fail::Skip);
        }
        self.raw = self.raw.saturating_add(raw.len());
        if self.raw > self.limits.raw_bytes {
            return Err(Fail::Stop(DavProblem::Limits));
        }
        out.changes.push(DavChange { href: href.to_string(), etag: etag.to_string(), raw: Some(raw), gone: false });
        Ok(())
    }
}

/// An ETag as the server wrote it, or none when it wrote none that can be compared.
fn usable(etag: Option<&str>) -> String {
    etag.map(str::trim).filter(|e| !e.is_empty()).unwrap_or_default().to_string()
}

struct Listed {
    found: Vec<Found>,
    capped: bool,
}

fn found_of(entry: &Entry) -> Found {
    Found {
        calendar: DavCalendar {
            href: entry.href.clone(),
            name: entry.display_name.clone().unwrap_or_default(),
            color: entry.color.map(|[r, g, b]| Rgb(r, g, b)),
            // (A server that says nothing of the owner's rights is taken as read-only.)
            read_only: entry.read_only.unwrap_or(true),
            components: entry.components.clone().unwrap_or_default(),
        },
        ctag: entry.ctag.clone(),
        token: entry.sync_token.clone(),
        sync: (!entry.reports.is_empty()).then(|| entry.supports_sync()),
    }
}

/// `YYYYMMDDTHHMMSSZ` for a Unix time (RFC 4791 §9.9).
fn stamp(at: i64) -> Option<Stamp> {
    let when = mailcraft_time::DateTime::from_timestamp(at, 0);
    Stamp::parse(&format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", when.date.year, when.date.month, when.date.day, when.hour, when.minute, when.second))
        .ok()
}

#[cfg(test)]
#[path = "dav_tests.rs"]
mod tests;
