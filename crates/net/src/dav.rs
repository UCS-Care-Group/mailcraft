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
//! How a collection is read, two ways and no third:
//!
//! - **From its token** (RFC 6578 `sync-collection`), page by page. A page is taken in whole or not
//!   at all, and the token moves past a page only when everything it names is in the download, so
//!   that nothing it said (a deletion least of all) is passed over. A page that could never be
//!   taken whole, a token the server forgot and a report it refuses send the read to the other way.
//! - **By listing it** (`PROPFIND`, Depth 1, each member's ETag), then fetching what is new or
//!   changed by `calendar-multiget` (one by one what that couldn't bring). Only a listing that is
//!   whole says what the collection holds ([`DavRead::members`]): that, and the server's own "gone",
//!   are the only things that ever remove an event here. No time range is asked for, so nothing
//!   depends on how the server or this program places a recurrence or a floating time.
//!
//! Each run is told what is here already ([`DavAskCollection::known`]), so one that stopped short
//! (on its limits, its changes or a document that couldn't be had) goes on where it stopped: every
//! run takes in at least one document or ends for a reason it says.
//!
//! Nothing of an address, a name, a user name or a password is logged.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mailcraft_dav::answer::{Collections, Entry, Multistatus};
use mailcraft_dav::href::{Origin, TrustedOrigin};
use mailcraft_dav::http::{HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};
use mailcraft_dav::request::{Endpoint, Kind, MAX_MULTIGET, MAX_PAGE, RequestError};
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
    /// The answer was larger than a request may bring: asking the same again brings the same.
    TooLarge,
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

/// How a pass over a collection's changes from its token ended.
enum Delta {
    /// Every page was taken in: `changes` is all that changed, and the token is the last page's.
    Done,
    /// Stopped after the last page that was taken in whole (or before the first): what was read
    /// stands, the token is that page's, and the next run goes on from it.
    Short,
    /// The token can't be gone on from (the server forgot it, or refused the report, or a page
    /// came that could never be taken in whole): the collection is listed afresh. `sync`: whether
    /// a token is still worth keeping for the next time.
    Afresh { sync: bool },
}

/// What came of fetching a set of documents.
enum Fetched {
    /// Each was taken in, or can never be had (gone since, too large to read, off the collection).
    All,
    /// Some could not be had now; they are asked for again at the next run.
    Partly,
}

/// One document asked for by itself.
enum Got {
    Doc(Option<String>, String),
    /// It is not there any more, is too large to read, or is not on the collection: asking again
    /// brings the same.
    Never,
    /// Not now (an error, or an answer that was no document).
    NotNow,
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
    /// The last request ([`Run::call`]) was answered at another address than the one it was
    /// made to.
    redirected: bool,
    /// Something couldn't be read for an error of the server's (a home, a collection, a
    /// document): said as [`DavProblem::ServerErrors`] when nothing worse ended the run.
    errors: bool,
}

/// Read what `ask` says through `x`. The passwords are asked for by origin, and only when that
/// origin challenges ([`Passwords`]). `now` is Unix seconds (for a `Retry-After` date).
pub fn run(ask: &DavAsk, passwords: Passwords<'_>, x: &dyn HttpExchange, now: i64, limits: &Limits) -> DavDownload {
    let mut out =
        DavDownload { account: ask.account, email: ask.email.clone(), server: ask.server.clone(), generation: ask.generation, ..Default::default() };
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
        redirected: false,
        errors: false,
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
    } else if run.errors {
        // (So that the window says it, rather than the run going round the same failure quietly.)
        out.problem = DavProblem::ServerErrors;
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

    fn skip(&mut self, count: usize) {
        self.skipped = self.skipped.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
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
                        listing.complete &= listed.whole;
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
                    Err(Fail::Stop(problem)) => {
                        listing.complete = false;
                        stopped = Some(problem);
                        break;
                    }
                    Err(_) => {
                        listing.complete = false;
                        self.errors = true;
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
                // (A calendar the engine has no room for is not read: it would be read whole at
                // every run, and none of it kept.)
                .filter(|f| !ask.no_room || known.contains_key(f.calendar.href.as_str()))
                .map(|f| {
                    let ask = known
                        .get(f.calendar.href.as_str())
                        .map(|c| (*c).clone())
                        .unwrap_or_else(|| DavAskCollection { href: f.calendar.href.clone(), ..Default::default() });
                    (ask, Some(f))
                })
                .collect()
        } else {
            ask.collections.iter().take(DavServer::MAX_COLLECTIONS).map(|c| (c.clone(), None)).collect()
        };
        for (collection, listed) in &targets {
            let mut read = DavRead { href: collection.href.clone(), ..Default::default() };
            let ended = self.read_collection(collection, *listed, &mut read);
            if ended.is_err() {
                read.complete = false;
            }
            if let Err(Fail::Skip | Fail::TooLarge) = ended {
                self.errors = true;
            }
            if read.state != DavReadState::Skipped || !read.changes.is_empty() {
                out.collections.push(read);
            }
            if let Err(Fail::Stop(problem)) = ended {
                return Err(Fail::Stop(problem));
            }
        }
        Ok(())
    }

    /// One request, with its challenge and its redirects. `build` makes the request for an address
    /// on the endpoint it is given. [`Run::redirected`] says afterwards whether the answer came
    /// from another address than `url`.
    fn call(&mut self, site: usize, url: &str, build: &dyn Fn(&Endpoint, &str) -> Result<HttpRequest, RequestError>) -> Result<HttpResponse, Fail> {
        let mut target = url.to_string();
        let mut redirects = 0;
        let mut retried = false;
        self.redirected = false;
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
                    self.skip(1);
                    return Err(Fail::Skip);
                }
            };
            // Never to Microsoft's or Google's hosts, whatever an answer said.
            if DavServer::check_home(request.url()).is_err() {
                self.skip(1);
                return Err(Fail::Skip);
            }
            let response = match self.x.exchange(&request) {
                Ok(response) => response,
                Err(TransportError::Certificate) => return Err(Fail::Stop(DavProblem::Certificate)),
                Err(TransportError::Unreachable) => return Err(Fail::Stop(DavProblem::Unreachable)),
                Err(TransportError::BodyTooLarge) => return Err(Fail::TooLarge),
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
                    self.redirected = redirects > 0;
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
            _ => Err(Fail::Skip),
        }
    }

    /// Whether `href` is a resource inside `collection` on its origin: what a member must be.
    fn inside(&self, site: usize, collection: &str, href: &str) -> bool {
        self.sites.get(site).is_some_and(|s| s.origin.is_within(collection, href))
    }

    // ---- listing --------------------------------------------------------------------------

    /// A home's calendars. `whole` only when every child of the home was understood: one whose
    /// properties failed, whose address was left out or that was past a cap may be a calendar, and
    /// a calendar that is not listed is taken for one that is gone.
    fn list_home(&mut self, home: &str) -> Result<Listed, Fail> {
        let Some(site) = self.site_of(home) else { return Err(Fail::Skip) };
        let response = self.call(site, home, &|ep, url| ep.propfind_collections(url))?;
        // (Answered at another address: what it lists is no proof of what this home holds.)
        let redirected = self.redirected;
        let mut ms = self.multistatus(site, &response, home)?;
        self.skip(ms.skipped);
        let understood = ms.entries.iter().filter(|e| !e.is_base).all(|e| (200..300).contains(&e.status) && e.typed);
        let mut whole = !redirected && understood && has_base(&ms) && ms.skipped == 0 && !ms.capped && !ms.truncated;
        // Only what is inside the home is one of its calendars.
        let before = ms.entries.len();
        ms.entries.retain(|e| e.is_base || self.inside(site, home, &e.href));
        if ms.entries.len() != before {
            self.skip(before - ms.entries.len());
            whole = false;
        }
        let listed = Collections::from_listing(ms);
        whole &= !listed.capped && listed.skipped == 0;
        Ok(Listed { found: listed.calendars.iter().map(found_of).collect(), whole })
    }

    // ---- one collection -------------------------------------------------------------------

    fn read_collection(&mut self, coll: &DavAskCollection, listed: Option<&Found>, out: &mut DavRead) -> Result<(), Fail> {
        let Some(site) = self.site_of(&coll.href) else {
            // (Not on an origin of the homes: never asked.)
            self.skip(1);
            return Err(Fail::Skip);
        };
        // (Asked before anything is read: a change made while the collection is read is after
        // them, and is seen again at the next run.)
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
        let known: HashMap<&str, &str> = coll.known.iter().map(|k| (k.href.as_str(), k.etag.as_str())).collect();
        let mut sync = listed.and_then(|f| f.sync) != Some(false);
        if sync && !coll.sync_token.is_empty() {
            match self.delta(site, coll, &known, out)? {
                Delta::Done => {
                    out.complete = true;
                    out.ctag = ctag.unwrap_or_default();
                    return Ok(());
                }
                Delta::Short => return Ok(()),
                Delta::Afresh { sync: still } => {
                    sync = still;
                    *out = DavRead { href: coll.href.clone(), state: DavReadState::Read, ..Default::default() };
                }
            }
        }
        self.full(site, coll, &known, if sync { token } else { None }, out)?;
        if out.complete {
            out.ctag = ctag.unwrap_or_default();
        }
        Ok(())
    }

    /// The change tag and sync token of a collection, asked for by themselves.
    fn state_of(&mut self, site: usize, href: &str) -> Result<(Option<String>, Option<String>), Fail> {
        let response = match self.call(site, href, &|ep, url| ep.propfind_state(url)) {
            Err(Fail::TooLarge) => return Ok((None, None)),
            other => other?,
        };
        // (A server that can't be asked this way is read as if it had changed.)
        let Ok(ms) = self.multistatus(site, &response, href) else { return Ok((None, None)) };
        let own = ms.entries.iter().find(|e| e.is_base).or_else(|| ms.entries.first());
        Ok((own.and_then(|e| e.ctag.clone()), own.and_then(|e| e.sync_token.clone())))
    }

    /// Every member of a collection (`PROPFIND`, Depth 1), and then each one that is new or
    /// changed. `out.members` is set only from a listing that is whole; `out.complete` (and the
    /// token, `token`, taken before the listing) only when every member that can be held was
    /// fetched.
    fn full(
        &mut self,
        site: usize,
        coll: &DavAskCollection,
        known: &HashMap<&str, &str>,
        token: Option<String>,
        out: &mut DavRead,
    ) -> Result<(), Fail> {
        out.complete = false;
        out.sync_token = String::new();
        let response = match self.call(site, &coll.href, &|ep, url| ep.propfind_members(url)) {
            Err(Fail::TooLarge) => return Err(Fail::Skip),
            other => other?,
        };
        // (Answered at another address: its members are not this collection's, as far as can be
        // told. Nothing is read, and nothing is taken to be gone.)
        if self.redirected {
            return Err(Fail::Skip);
        }
        let ms = self.multistatus(site, &response, &coll.href)?;
        self.skip(ms.skipped);
        // Whether the listing is all there is (what isn't in it is gone), and whether every member
        // that can be held is in it (the change tag and token may be kept once they are fetched).
        // An answer that doesn't describe the collection itself is no listing of it at all.
        let described = has_base(&ms);
        let mut whole = described && ms.skipped == 0 && !ms.capped && !ms.truncated;
        let mut all = described && !ms.capped && !ms.truncated;
        let mut members: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in ms.entries.iter().filter(|e| !e.is_base) {
            if !self.inside(site, &coll.href, &entry.href) {
                // (Never to be held here: it doesn't stop the read, but the listing is not whole.)
                self.skip(1);
                whole = false;
                continue;
            }
            if !(200..300).contains(&entry.status) {
                // (A member the server couldn't describe now: not read, so the read is not all.)
                whole = false;
                all = false;
                continue;
            }
            if entry.typed && entry.is_collection {
                continue;
            }
            // (A member whose kind didn't come is read as a resource; but it might have been
            // something else, so the listing is not proof of anything.)
            whole &= entry.typed;
            if !seen.insert(entry.href.as_str()) {
                continue;
            }
            if members.len() >= MAX_RESOURCES {
                // (The first ones stand; what is over is said, and the listing is not all.)
                whole = false;
                all = false;
                break;
            }
            members.push((entry.href.clone(), usable(entry.etag.as_deref())));
        }
        if whole {
            out.members = Some(members.iter().map(|(h, _)| h.clone()).collect());
        }
        let wanted: Vec<(String, String)> = members.into_iter().filter(|(href, etag)| changed(known, href, etag)).collect();
        let mut gets = 0usize;
        let fetched = self.fetch(site, &coll.href, &wanted, &mut gets, out)?;
        out.complete = all && matches!(fetched, Fetched::All);
        if out.complete {
            out.sync_token = token.unwrap_or_default();
        }
        Ok(())
    }

    /// Reads a collection's changes by `sync-collection` (RFC 6578) from its token, page by page.
    /// A page is taken in whole or not at all: the token moves past it only when everything it
    /// names is in `out.changes` (or can never be had).
    fn delta(&mut self, site: usize, coll: &DavAskCollection, known: &HashMap<&str, &str>, out: &mut DavRead) -> Result<Delta, Fail> {
        out.sync_token = coll.sync_token.clone();
        let mut token = coll.sync_token.clone();
        let mut gets = 0usize;
        let mut pages = 0usize;
        loop {
            // (A page that can never be taken in whole, the first: by token there is no way on.)
            let stuck = |pages: usize| if pages == 0 { Delta::Afresh { sync: true } } else { Delta::Short };
            if pages >= self.limits.pages.max(1) {
                return Ok(Delta::Short);
            }
            let asked = token.clone();
            let response = match self.call(site, &coll.href, &|ep, url| ep.sync_collection(url, Some(&asked), Some(MAX_PAGE))) {
                Err(Fail::TooLarge) => return Ok(stuck(pages)),
                other => other?,
            };
            if self.redirected {
                return Err(Fail::Skip);
            }
            match status::classify(&response, self.now) {
                Verdict::Multistatus => {}
                Verdict::SyncTokenInvalid => return Ok(Delta::Afresh { sync: true }),
                // A server that refuses the report (400, 403, 405, 501 and the like) is read by
                // listing from now on; one that failed (5xx) may do it again later.
                Verdict::Other(code) if code >= 500 && code != 501 => return Ok(Delta::Afresh { sync: true }),
                _ => return Ok(Delta::Afresh { sync: false }),
            }
            let Ok(ms) = self.multistatus(site, &response, &coll.href) else { return Ok(stuck(pages)) };
            self.skip(ms.skipped);
            if ms.capped {
                return Ok(stuck(pages));
            }
            let mut gone: Vec<String> = Vec::new();
            let mut wanted: Vec<(String, String)> = Vec::new();
            for entry in ms.entries.iter().filter(|e| !e.is_base) {
                if !self.inside(site, &coll.href, &entry.href) {
                    // (Never held here, so nothing here can be missing it.)
                    self.skip(1);
                    continue;
                }
                if entry.is_gone() {
                    gone.push(entry.href.clone());
                    continue;
                }
                let etag = usable(entry.etag.as_deref());
                // (One the server couldn't describe now is a change all the same: the page is
                // taken in only once it has been read.)
                let undescribed = !(200..300).contains(&entry.status);
                if undescribed || changed(known, &entry.href, &etag) {
                    wanted.push((entry.href.clone(), etag));
                }
            }
            if gone.len().saturating_add(wanted.len()) > MAX_CHANGES.saturating_sub(out.changes.len()) {
                return Ok(stuck(pages));
            }
            out.changes.extend(gone.into_iter().map(|href| DavChange { href, gone: true, ..Default::default() }));
            if let Fetched::Partly = self.fetch(site, &coll.href, &wanted, &mut gets, out)? {
                return Ok(Delta::Short);
            }
            // The page is in, whole.
            pages += 1;
            match ms.sync_token {
                Some(next) if next != token => {
                    out.sync_token = next.clone();
                    token = next;
                }
                // (A page without a new token: there is nothing to go on from.)
                _ if ms.truncated => return Ok(stuck(pages.saturating_sub(1))),
                _ => {}
            }
            if !ms.truncated {
                return Ok(Delta::Done);
            }
        }
    }

    /// The documents of `wanted` (address, ETag), into `out.changes`: by multiget in batches, and
    /// one by one what a multiget left out, sent too large to hold, or couldn't bring at all.
    fn fetch(&mut self, site: usize, collection: &str, wanted: &[(String, String)], gets: &mut usize, out: &mut DavRead) -> Result<Fetched, Fail> {
        let etags: HashMap<&str, &str> = wanted.iter().map(|(h, e)| (h.as_str(), e.as_str())).collect();
        let mut single: Vec<String> = Vec::new();
        for batch in wanted.chunks(MAX_MULTIGET) {
            if out.changes.len() >= MAX_CHANGES {
                return Ok(Fetched::Partly);
            }
            let hrefs: Vec<&str> = batch.iter().map(|(h, _)| h.as_str()).collect();
            let ms = match self.call(site, collection, &|ep, url| ep.multiget(Kind::Calendar, url, &hrefs)) {
                Ok(response) if !self.redirected => self.multistatus(site, &response, collection).ok(),
                Ok(_) | Err(Fail::Skip | Fail::TooLarge) => None,
                Err(stop) => return Err(stop),
            };
            // (A batch that couldn't be had together is asked for one by one.)
            let Some(ms) = ms else {
                single.extend(batch.iter().map(|(h, _)| h.clone()));
                continue;
            };
            self.skip(ms.skipped);
            let mut got: HashSet<String> = HashSet::new();
            for entry in ms.entries.iter().filter(|e| !e.is_base && !e.is_gone()) {
                // Only what was asked for: a server may name others, and none of them is taken.
                let Some(asked) = etags.get(entry.href.as_str()) else {
                    self.skip(1);
                    continue;
                };
                if got.contains(&entry.href) {
                    continue;
                }
                if let Some(data) = &entry.data {
                    if !self.take(out, &entry.href, entry.etag.as_deref().unwrap_or(asked), data.clone())? {
                        return Ok(Fetched::Partly);
                    }
                    got.insert(entry.href.clone());
                }
            }
            single.extend(batch.iter().filter(|(h, _)| !got.contains(h)).map(|(h, _)| h.clone()));
        }
        let mut all = true;
        for href in single {
            if out.changes.len() >= MAX_CHANGES {
                return Ok(Fetched::Partly);
            }
            if *gets >= self.limits.gets {
                all = false;
                break;
            }
            *gets += 1;
            match self.get(site, collection, &href)? {
                Got::Doc(etag, body) => {
                    let etag = etag.as_deref().or_else(|| etags.get(href.as_str()).copied()).unwrap_or_default().to_string();
                    if !self.take(out, &href, &etag, body)? {
                        return Ok(Fetched::Partly);
                    }
                }
                // (Said, without its document, so that it is not asked for at every run.)
                Got::Never => {
                    let etag = etags.get(href.as_str()).copied().unwrap_or_default().to_string();
                    out.changes.push(DavChange { href, etag, raw: None, gone: false });
                }
                Got::NotNow => {
                    // lean: said as a problem each run, not counted per document; a count kept by
                    // the engine would let one that keeps failing be passed over.
                    self.errors = true;
                    all = false;
                }
            }
        }
        Ok(if all { Fetched::All } else { Fetched::Partly })
    }

    /// One document, by `GET`.
    fn get(&mut self, site: usize, collection: &str, href: &str) -> Result<Got, Fail> {
        if !self.inside(site, collection, href) {
            return Ok(Got::Never);
        }
        let response = match self.call(site, href, &|ep, url| ep.get(url)) {
            Ok(response) => response,
            Err(Fail::TooLarge) => return Ok(Got::Never),
            Err(Fail::Skip) => return Ok(Got::NotNow),
            Err(stop) => return Err(stop),
        };
        match status::classify(&response, self.now) {
            Verdict::Done if response.body().len() > MAX_READ => Ok(Got::Never),
            Verdict::Done => {
                let etag = response.header("etag").map(str::to_owned);
                Ok(Got::Doc(etag, String::from_utf8_lossy(response.body()).into_owned()))
            }
            // (Not there, or refused for good: asking again brings the same.)
            Verdict::Gone | Verdict::Forbidden | Verdict::Other(405) => Ok(Got::Never),
            _ => Ok(Got::NotNow),
        }
    }

    /// Keeps one document. False when the read holds as many changes as it may (the document is
    /// not kept, and is asked for again at the next run). The run's bytes end the run, after this
    /// document: each run takes at least one.
    fn take(&mut self, out: &mut DavRead, href: &str, etag: &str, raw: String) -> Result<bool, Fail> {
        if out.changes.len() >= MAX_CHANGES {
            return Ok(false);
        }
        self.raw = self.raw.saturating_add(raw.len());
        out.changes.push(DavChange { href: href.to_string(), etag: etag.to_string(), raw: Some(raw), gone: false });
        if self.raw > self.limits.raw_bytes {
            return Err(Fail::Stop(DavProblem::Limits));
        }
        Ok(true)
    }
}

/// Whether a multistatus describes the address that was asked, with a status that says it is
/// there (a Depth 1 listing names the collection itself first, RFC 4918 §9.1). Without it, an
/// answer with no members is no word that the collection is empty.
fn has_base(ms: &Multistatus) -> bool {
    ms.entries.iter().any(|e| e.is_base && (200..300).contains(&e.status))
}

/// Whether a member is to be read: it isn't here, or is here under another ETag. One the server
/// gave no ETag for can't be compared: it is read when it isn't here, and the engine leaves it out
/// of what is here once a day, so that it is read again. That holds in a change pass too: a page
/// cut short is asked for again from its start, and reading again what it named without an ETag
/// would take the same documents at every run and never get past them. (A server that keeps to
/// RFC 4791 gives every calendar resource an ETag; one that doesn't has a change seen within a day.)
///
/// Only what is here *without* an ETag holds back one that came without: what is here under an
/// ETag and now comes without one (its properties failed, the server stopped giving them) can't be
/// said to be the same, and is read.
fn changed(known: &HashMap<&str, &str>, href: &str, etag: &str) -> bool {
    if etag.is_empty() { known.get(href).is_none_or(|have| !have.is_empty()) } else { known.get(href) != Some(&etag) }
}

/// An ETag as the server wrote it, or none when it wrote none that can be compared.
fn usable(etag: Option<&str>) -> String {
    etag.map(str::trim).filter(|e| !e.is_empty()).unwrap_or_default().to_string()
}

struct Listed {
    found: Vec<Found>,
    /// Every child of the home was understood, and none was left out.
    whole: bool,
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

#[cfg(test)]
#[path = "dav_tests.rs"]
mod tests;
