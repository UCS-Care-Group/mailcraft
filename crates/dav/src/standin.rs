// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for a CalDAV and CardDAV server that holds a client to the RFCs, for the tests.
//!
//! It runs inside the test: [`StandIn`] is an [`HttpExchange`] that answers at once, and nothing
//! here opens a socket (the host crates wrap it in their test TLS server when they need a wire).
//! It is not in a build that isn't a test's (`stand-in` feature).
//!
//! What it holds a client to:
//!
//! - **Credentials.** Every request needs the `Authorization` it was set up with; without it the
//!   answer is 401 with `WWW-Authenticate: Basic` and a `DAV` header naming the classes (RFC 4918
//!   §10.1), as a DAV server answers a probe. The quirk [`Quirks::bare_401`] drops the `DAV`
//!   header, as a mail filter's web front does.
//! - **Requests.** `PROPFIND` and `REPORT` bodies are XML of the expected root; `Depth` is given
//!   (`infinity` is refused with `propfind-finite-depth`, RFC 4918 §9.1; `sync-collection` needs
//!   `0`); a `REPORT` is one the collection supports (`supported-report`, RFC 3253 §3.6).
//! - **Writes.** A `PUT` of an existing resource needs `If-Match`, a create needs
//!   `If-None-Match: *` (428 otherwise, and the request is noted in [`StandIn::violations`]); a
//!   stale `If-Match` or a create over an existing name is 412; a read-only collection is 403;
//!   data that is not a calendar object or a vCard is 403 (`valid-calendar-data`).
//! - **Multiget.** An `href` outside the collection is answered 404 and noted as a violation.
//! - **Sync.** `sync-collection` (RFC 6578) with tokens, `507` truncation and `valid-sync-token`.
//!
//! And it can misbehave as real servers do ([`Quirks`]).

use crate::href::Origin;
use crate::http::{HttpExchange, HttpRequest, HttpResponse, Method, Secret, TransportError};
use crate::request::{APPLE, CALSERVER, escape};
use crate::xml::{self, CALDAV, CARDDAV, DAV, Node};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

/// The `DAV` header of a server that does CalDAV and CardDAV.
pub const DAV_HEADER: &str = "1, 2, 3, calendar-access, addressbook, extended-mkcol";

/// Ways real servers differ.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quirks {
    /// No `sync-collection` in `supported-report-set`, and a `REPORT` of it is refused.
    pub no_sync_collection: bool,
    /// Google's layout: `/caldav/v2/user%40example.org/…` with an escaped `@`, absolute `href`s,
    /// no `getctag`, no address books.
    pub google: bool,
    /// A plain resource (`personal.ics`, not a collection) in the calendar home, as SOGo keeps.
    pub sogo_plain_in_home: bool,
    /// A sync answers at most this many changes, then `507` for the collection.
    pub page_size: Option<usize>,
    /// Every sync token but the empty one is unknown (`valid-sync-token`).
    pub token_expiry: bool,
    /// The 401 has no `DAV` header: a web front that is not a DAV server.
    pub bare_401: bool,
}

#[derive(Clone, Debug)]
struct Member {
    etag: String,
    body: String,
}

#[derive(Clone, Debug)]
struct Coll {
    name: String,
    display: String,
    address_book: bool,
    read_only: bool,
    tasks_only: bool,
    members: BTreeMap<String, Member>,
    /// (revision, name, present afterwards)
    log: Vec<(u64, String, bool)>,
    rev: u64,
}

/// One request the stand-in saw, for the tests to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Method name.
    pub method: String,
    /// Path as asked.
    pub path: String,
    /// Whether the credential it was set up with came.
    pub authorized: bool,
    /// Whether any `Authorization` came.
    pub credential: bool,
}

struct State {
    origin: Origin,
    quirks: Quirks,
    host: String,
    user: String,
    password: String,
    colls: Vec<Coll>,
    next_etag: u64,
    violations: Vec<String>,
    seen: Vec<Seen>,
    writes: usize,
}

/// The server. Share it by reference; it locks inside.
pub struct StandIn {
    origin: Origin,
    state: Mutex<State>,
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "HTTP/1.1 200 OK",
        404 => "HTTP/1.1 404 Not Found",
        507 => "HTTP/1.1 507 Insufficient Storage",
        _ => "HTTP/1.1 500 Internal Server Error",
    }
}

enum Res<'a> {
    Root,
    Principal,
    Home(bool),
    Coll(&'a Coll),
    Member(&'a Coll, &'a str, &'a Member),
    Plain,
}

impl StandIn {
    /// A server at `origin` (`https://dav.example.org`) that takes `user` and `password` (the
    /// tests' own, never a real one) and has no collections.
    pub fn new(origin: &str, user: &str, password: &str) -> Result<Self, crate::http::EndpointError> {
        let parsed = Origin::parse(origin)?;
        let state = State {
            origin: parsed.clone(),
            quirks: Quirks::default(),
            host: parsed.base(),
            user: user.into(),
            password: password.into(),
            colls: Vec::new(),
            next_etag: 1,
            violations: Vec::new(),
            seen: Vec::new(),
            writes: 0,
        };
        Ok(Self { origin: parsed, state: Mutex::new(state) })
    }

    /// The origin, as an endpoint takes it.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    /// The credential the stand-in takes.
    pub fn credential(&self) -> Secret {
        let s = self.lock();
        Secret::basic(&s.user, &s.password)
    }
    /// Sets the quirks.
    pub fn set_quirks(&self, quirks: Quirks) {
        self.lock().quirks = quirks;
    }
    /// Adds a calendar of events; its address is [`StandIn::collection_url`].
    pub fn add_calendar(&self, name: &str, display: &str, read_only: bool) {
        self.add(name, display, false, read_only, false);
    }
    /// Adds a calendar that holds tasks only (`VTODO`).
    pub fn add_task_list(&self, name: &str, display: &str) {
        self.add(name, display, false, false, true);
    }
    /// Adds an address book.
    pub fn add_address_book(&self, name: &str, display: &str, read_only: bool) {
        self.add(name, display, true, read_only, false);
    }
    fn add(&self, name: &str, display: &str, address_book: bool, read_only: bool, tasks_only: bool) {
        self.lock().colls.push(Coll {
            name: name.into(),
            display: display.into(),
            address_book,
            read_only,
            tasks_only,
            members: BTreeMap::new(),
            log: Vec::new(),
            rev: 0,
        });
    }
    /// Puts a resource in a collection as if somebody else had (no checks); returns its ETag.
    pub fn put_directly(&self, collection: &str, name: &str, body: &str) -> Option<String> {
        let mut s = self.lock();
        let etag = format!("\"{}\"", s.next_etag);
        s.next_etag += 1;
        let c = s.colls.iter_mut().find(|c| c.name == collection)?;
        c.rev += 1;
        let rev = c.rev;
        c.members.insert(name.into(), Member { etag: etag.clone(), body: body.into() });
        c.log.push((rev, name.into(), true));
        Some(etag)
    }
    /// Removes a resource as if somebody else had.
    pub fn remove_directly(&self, collection: &str, name: &str) {
        let mut s = self.lock();
        if let Some(c) = s.colls.iter_mut().find(|c| c.name == collection)
            && c.members.remove(name).is_some()
        {
            c.rev += 1;
            let rev = c.rev;
            c.log.push((rev, name.into(), false));
        }
    }
    /// The body of a resource.
    pub fn body_of(&self, collection: &str, name: &str) -> Option<String> {
        self.lock().colls.iter().find(|c| c.name == collection)?.members.get(name).map(|m| m.body.clone())
    }
    /// The ETag of a resource.
    pub fn etag_of(&self, collection: &str, name: &str) -> Option<String> {
        self.lock().colls.iter().find(|c| c.name == collection)?.members.get(name).map(|m| m.etag.clone())
    }
    /// How many resources a collection holds.
    pub fn count(&self, collection: &str) -> usize {
        self.lock().colls.iter().find(|c| c.name == collection).map_or(0, |c| c.members.len())
    }
    /// The sync token a collection has now.
    pub fn token(&self, collection: &str) -> Option<String> {
        self.lock().colls.iter().find(|c| c.name == collection).map(|c| token(c.rev))
    }
    /// What the stand-in found wrong with the requests it got (empty: none).
    pub fn violations(&self) -> Vec<String> {
        self.lock().violations.clone()
    }
    /// Every request seen, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.lock().seen.clone()
    }
    /// How many `PUT` and `DELETE` were accepted.
    pub fn writes(&self) -> usize {
        self.lock().writes
    }
    /// The address of the root a client starts at (`/.well-known/caldav` redirects to it).
    pub fn root_url(&self) -> String {
        format!("{}{}", self.origin.base(), self.layout().root)
    }
    /// The address of a collection.
    pub fn collection_url(&self, collection: &str) -> String {
        let l = self.layout();
        let book = self.lock().colls.iter().find(|c| c.name == collection).is_some_and(|c| c.address_book);
        format!("{}{}{collection}/", self.origin.base(), if book { l.card_home } else { l.cal_home })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn layout(&self) -> Layout {
        Layout::of(self.lock().quirks.google)
    }
}

fn token(rev: u64) -> String {
    format!("http://example.org/ns/sync/{rev}")
}

struct Layout {
    root: &'static str,
    principal: &'static str,
    cal_home: &'static str,
    card_home: &'static str,
}

impl Layout {
    fn of(google: bool) -> Self {
        if google {
            Self {
                root: "/caldav/v2/",
                principal: "/caldav/v2/owner%40example.org/user",
                cal_home: "/caldav/v2/owner%40example.org/",
                card_home: "/carddav/unsupported/",
            }
        } else {
            Self { root: "/dav/", principal: "/dav/principals/owner/", cal_home: "/dav/cal/owner/", card_home: "/dav/card/owner/" }
        }
    }
}

fn reply(code: u16, ctype: &str, body: String) -> HttpResponse {
    HttpResponse::new(code, body.into_bytes()).with_field("Content-Type", ctype).with_field("DAV", DAV_HEADER)
}

fn error_body(code: u16, element: &str) -> HttpResponse {
    reply(
        code,
        "application/xml; charset=utf-8",
        format!("<?xml version=\"1.0\"?><d:error xmlns:d=\"DAV:\" xmlns:c=\"{CALDAV}\"><{element}/></d:error>"),
    )
}

impl HttpExchange for StandIn {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        Ok(self.handle(request))
    }
}

impl StandIn {
    /// Answers one request.
    pub fn handle(&self, request: &HttpRequest) -> HttpResponse {
        let mut s = self.lock();
        let layout = Layout::of(s.quirks.google);
        let url = request.url().to_owned();
        let on_origin = Origin::parse(&url).is_ok_and(|o| o == self.origin);
        let path = if on_origin { self.origin.path(&url).unwrap_or_default() } else { String::new() };
        let sent = request.field("authorization");
        let expected = Secret::basic(&s.user, &s.password);
        let authorized = sent == Some(expected.expose());
        s.seen.push(Seen { method: request.method().as_str().into(), path: path.clone(), authorized, credential: sent.is_some() });
        if !on_origin {
            return HttpResponse::new(421, Vec::new());
        }
        if path == "/.well-known/caldav" || path == "/.well-known/carddav" {
            return HttpResponse::new(301, Vec::new()).with_field("Location", layout.root);
        }
        if !authorized {
            let mut r = HttpResponse::new(401, Vec::new()).with_field("WWW-Authenticate", "Basic realm=\"dav\", charset=\"UTF-8\"");
            if !s.quirks.bare_401 {
                r = r.with_field("DAV", DAV_HEADER);
            }
            return r;
        }
        match request.method() {
            Method::Options => {
                HttpResponse::new(200, Vec::new()).with_field("DAV", DAV_HEADER).with_field("Allow", "OPTIONS, GET, PUT, DELETE, PROPFIND, REPORT")
            }
            Method::Propfind => propfind(&mut s, &layout, request, &path),
            Method::Report => report(&mut s, &layout, request, &path),
            Method::Get => match locate(&s, &layout, &path) {
                Some(Res::Member(_, _, m)) => reply(200, "text/calendar", m.body.clone()).with_field("ETag", &m.etag),
                _ => HttpResponse::new(404, Vec::new()),
            },
            Method::Put => put(&mut s, &layout, request, &path),
            Method::Delete => delete(&mut s, &layout, request, &path),
        }
    }
}

/// Splits `path` below a home into (home is cards, collection, member).
fn split<'p>(layout: &Layout, path: &'p str) -> Option<(bool, &'p str, Option<&'p str>)> {
    let (book, rest) = match path.strip_prefix(layout.cal_home) {
        Some(r) => (false, r),
        None => (true, path.strip_prefix(layout.card_home)?),
    };
    let mut parts = rest.split('/');
    let c = parts.next().filter(|c| !c.is_empty())?;
    let m = parts.next().filter(|m| !m.is_empty());
    if parts.next().is_some_and(|x| !x.is_empty()) {
        return None;
    }
    Some((book, c, m))
}

fn locate<'a>(s: &'a State, layout: &Layout, path: &'a str) -> Option<Res<'a>> {
    let p = path.trim_end_matches('/');
    if p == layout.root.trim_end_matches('/') {
        return Some(Res::Root);
    }
    if p == layout.principal.trim_end_matches('/') {
        return Some(Res::Principal);
    }
    if p == layout.cal_home.trim_end_matches('/') {
        return Some(Res::Home(false));
    }
    if p == layout.card_home.trim_end_matches('/') && !s.quirks.google {
        return Some(Res::Home(true));
    }
    if s.quirks.sogo_plain_in_home && p == format!("{}personal.ics", layout.cal_home) {
        return Some(Res::Plain);
    }
    let (book, c, m) = split(layout, p)?;
    let coll = s.colls.iter().find(|x| x.name == c && x.address_book == book)?;
    match m {
        None => Some(Res::Coll(coll)),
        Some(m) => coll.members.get_key_value(m).map(|(k, v)| Res::Member(coll, k.as_str(), v)),
    }
}

fn href_for(s: &State, path: &str) -> String {
    if s.quirks.google { format!("{}{path}", s.host) } else { path.to_owned() }
}

fn path_of(layout: &Layout, res: &Res) -> String {
    match res {
        Res::Root => layout.root.into(),
        Res::Principal => layout.principal.into(),
        Res::Home(false) => layout.cal_home.into(),
        Res::Home(true) => layout.card_home.into(),
        Res::Coll(c) => format!("{}{}/", if c.address_book { layout.card_home } else { layout.cal_home }, c.name),
        Res::Member(c, n, _) => format!("{}{}/{n}", if c.address_book { layout.card_home } else { layout.cal_home }, c.name),
        Res::Plain => format!("{}personal.ics", layout.cal_home),
    }
}

fn parse_body(request: &HttpRequest, root: &[(&str, &str)]) -> Result<Node, HttpResponse> {
    let bad = |c: u16| Err(HttpResponse::new(c, Vec::new()));
    let ctype = request.field("content-type").unwrap_or("");
    if !(ctype.starts_with("application/xml") || ctype.starts_with("text/xml")) {
        return bad(415);
    }
    let Ok(node) = xml::parse_document(request.body()) else { return bad(400) };
    if root.iter().any(|(ns, n)| node.is(ns, n)) { Ok(node) } else { bad(400) }
}

fn multistatus(responses: &str, extra: &str) -> HttpResponse {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<d:multistatus xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\" xmlns:card=\"{CARDDAV}\" xmlns:a=\"{APPLE}\" xmlns:cs=\"{CALSERVER}\">{responses}{extra}</d:multistatus>"
    );
    reply(207, "application/xml; charset=utf-8", body)
}

fn value(res: &Res, s: &State, layout: &Layout, ns: &str, name: &str) -> Option<String> {
    let write_priv = |ro: bool| {
        if ro {
            "<d:privilege><d:read/></d:privilege>"
        } else {
            "<d:privilege><d:read/></d:privilege><d:privilege><d:write-content/></d:privilege><d:privilege><d:bind/></d:privilege><d:privilege><d:unbind/></d:privilege>"
        }
    };
    let url = |p: &str| format!("<d:href>{}</d:href>", escape(&href_for(s, p)));
    Some(match (ns, name, res) {
        (DAV, "resourcetype", Res::Coll(c)) => format!("<d:collection/>{}", if c.address_book { "<card:addressbook/>" } else { "<c:calendar/>" }),
        (DAV, "resourcetype", Res::Plain | Res::Member(..)) => String::new(),
        (DAV, "resourcetype", Res::Principal) => "<d:principal/>".into(),
        (DAV, "resourcetype", _) => "<d:collection/>".into(),
        (DAV, "displayname", Res::Coll(c)) => escape(&c.display),
        (DAV, "displayname", Res::Principal) => "Owner".into(),
        (DAV, "getetag", Res::Member(_, _, m)) => escape(&m.etag),
        (DAV, "getcontenttype", Res::Member(c, ..)) => if c.address_book { "text/vcard" } else { "text/calendar" }.into(),
        (CALSERVER, "getctag", Res::Coll(c)) if !s.quirks.google => escape(&format!("ctag-{}", c.rev)),
        (DAV, "sync-token", Res::Coll(c)) if !s.quirks.no_sync_collection => token(c.rev),
        (DAV, "current-user-principal", _) => url(layout.principal),
        (CALDAV, "calendar-home-set", Res::Principal) => url(layout.cal_home),
        (CARDDAV, "addressbook-home-set", Res::Principal) if !s.quirks.google => url(layout.card_home),
        (CALDAV, "supported-calendar-component-set", Res::Coll(c)) if !c.address_book => {
            if c.tasks_only { "<c:comp name=\"VTODO\"/>" } else { "<c:comp name=\"VEVENT\"/><c:comp name=\"VTODO\"/>" }.into()
        }
        (CARDDAV, "supported-address-data", Res::Coll(c)) if c.address_book => "<card:address-data-type content-type=\"text/vcard\" version=\"3.0\"/><card:address-data-type content-type=\"text/vcard\" version=\"4.0\"/>".into(),
        (DAV, "supported-report-set", Res::Coll(c)) => {
            let mut r = if c.address_book { "<d:supported-report><d:report><card:addressbook-multiget/></d:report></d:supported-report>".to_owned() } else { "<d:supported-report><d:report><c:calendar-query/></d:report></d:supported-report><d:supported-report><d:report><c:calendar-multiget/></d:report></d:supported-report>".to_owned() };
            if !s.quirks.no_sync_collection {
                r.push_str("<d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report>");
            }
            r
        }
        (DAV, "current-user-privilege-set", Res::Coll(c)) => write_priv(c.read_only).into(),
        (APPLE, "calendar-color", Res::Coll(c)) if !c.address_book => "#3366CCFF".into(),
        (CALDAV, "calendar-data", Res::Member(c, _, m)) if !c.address_book => escape(&m.body),
        (CARDDAV, "address-data", Res::Member(c, _, m)) if c.address_book => escape(&m.body),
        _ => return None,
    })
}

fn response_for(s: &State, layout: &Layout, res: &Res, wanted: &[(String, String)]) -> String {
    let (mut found, mut missing) = (String::new(), String::new());
    for (ns, name) in wanted {
        let prefix = match ns.as_str() {
            DAV => "d",
            CALDAV => "c",
            CARDDAV => "card",
            APPLE => "a",
            CALSERVER => "cs",
            _ => {
                continue;
            }
        };
        match value(res, s, layout, ns, name) {
            Some(v) if v.is_empty() => found.push_str(&format!("<{prefix}:{name}/>")),
            Some(v) => found.push_str(&format!("<{prefix}:{name}>{v}</{prefix}:{name}>")),
            None => missing.push_str(&format!("<{prefix}:{name}/>")),
        }
    }
    let mut out = format!("<d:response><d:href>{}</d:href>", escape(&href_for(s, &path_of(layout, res))));
    if !found.is_empty() || missing.is_empty() {
        out.push_str(&format!("<d:propstat><d:prop>{found}</d:prop><d:status>{}</d:status></d:propstat>", status_line(200)));
    }
    if !missing.is_empty() {
        out.push_str(&format!("<d:propstat><d:prop>{missing}</d:prop><d:status>{}</d:status></d:propstat>", status_line(404)));
    }
    out.push_str("</d:response>");
    out
}

fn wanted(node: &Node) -> Vec<(String, String)> {
    node.child(DAV, "prop").map(|p| p.children.iter().map(|c| (c.namespace.clone(), c.name.clone())).collect()).unwrap_or_default()
}

fn propfind(s: &mut State, layout: &Layout, request: &HttpRequest, path: &str) -> HttpResponse {
    let depth = match request.field("depth") {
        Some("0") => 0,
        Some("1") => 1,
        _ => return error_body(403, "d:propfind-finite-depth"),
    };
    let body = match parse_body(request, &[(DAV, "propfind")]) {
        Ok(b) => b,
        Err(r) => {
            s.violations.push("PROPFIND body is not a propfind document".into());
            return r;
        }
    };
    let want = wanted(&body);
    let Some(res) = locate(s, layout, path) else { return HttpResponse::new(404, Vec::new()) };
    let mut out = response_for(s, layout, &res, &want);
    if depth == 1 {
        let children: Vec<Res> = match &res {
            Res::Home(book) => {
                let mut v: Vec<Res> = s.colls.iter().filter(|c| c.address_book == *book).map(Res::Coll).collect();
                if !book && s.quirks.sogo_plain_in_home {
                    v.push(Res::Plain);
                }
                v
            }
            Res::Coll(c) => c.members.iter().map(|(n, m)| Res::Member(c, n.as_str(), m)).collect(),
            Res::Root => vec![Res::Principal],
            _ => Vec::new(),
        };
        for child in &children {
            out.push_str(&response_for(s, layout, child, &want));
        }
    }
    multistatus(&out, "")
}

fn hrefs_of(body: &Node) -> Vec<String> {
    body.children.iter().filter(|c| c.is(DAV, "href")).map(|c| c.text.trim().to_owned()).collect()
}

fn dtstart(body: &str) -> Option<String> {
    let line = body.lines().find(|l| l.starts_with("DTSTART"))?;
    let v = line.split_once(':')?.1.trim();
    Some(if v.len() == 8 { format!("{v}T000000Z") } else { v.to_owned() })
}

fn report(s: &mut State, layout: &Layout, request: &HttpRequest, path: &str) -> HttpResponse {
    let mut found = Vec::new();
    let answer = report_inner(s, layout, request, path, &mut found);
    s.violations.extend(found);
    answer
}

fn report_inner(s: &State, layout: &Layout, request: &HttpRequest, path: &str, violations: &mut Vec<String>) -> HttpResponse {
    let body = match parse_body(
        request,
        &[(CALDAV, "calendar-query"), (CALDAV, "calendar-multiget"), (CARDDAV, "addressbook-multiget"), (DAV, "sync-collection")],
    ) {
        Ok(b) => b,
        Err(r) => {
            violations.push("REPORT body is not a known report".into());
            return r;
        }
    };
    let Some(Res::Coll(coll)) = locate(s, layout, path) else { return HttpResponse::new(404, Vec::new()) };
    let want = wanted(&body);
    let depth = request.field("depth");
    let is = |ns, n| body.is(ns, n);
    if (is(DAV, "sync-collection") && s.quirks.no_sync_collection)
        || ((is(CALDAV, "calendar-query") || is(CALDAV, "calendar-multiget")) && coll.address_book)
        || (is(CARDDAV, "addressbook-multiget") && !coll.address_book)
    {
        return error_body(403, "d:supported-report");
    }
    if is(DAV, "sync-collection") {
        if depth != Some("0") {
            violations.push("sync-collection without Depth: 0".into());
            return HttpResponse::new(400, Vec::new());
        }
        return sync(s, layout, coll, &body, &want, path);
    }
    if depth != Some("0") && depth != Some("1") {
        violations.push("REPORT without a finite Depth".into());
        return HttpResponse::new(400, Vec::new());
    }
    let mut out = String::new();
    if is(CALDAV, "calendar-query") {
        let range = body
            .child(CALDAV, "filter")
            .and_then(|f| f.child(CALDAV, "comp-filter"))
            .and_then(|f| f.child(CALDAV, "comp-filter"))
            .and_then(|f| f.child(CALDAV, "time-range"));
        let (start, end) = range.map_or((None, None), |r| (r.attribute("start").map(str::to_owned), r.attribute("end").map(str::to_owned)));
        for (n, m) in &coll.members {
            let d = dtstart(&m.body);
            let inside = match (&d, &start, &end) {
                (Some(d), Some(a), Some(b)) => d.as_str() >= a.as_str() && d.as_str() < b.as_str(),
                _ => true,
            };
            if inside {
                out.push_str(&response_for(s, layout, &Res::Member(coll, n, m), &want));
            }
        }
    } else {
        for href in hrefs_of(&body) {
            let abs = format!("{}{href}", s.host);
            let inside = s.origin.is_within(&path_of(layout, &Res::Coll(coll)), &abs);
            if !inside {
                violations.push("multiget names an href outside the collection".into());
                out.push_str(&format!("<d:response><d:href>{}</d:href><d:status>{}</d:status></d:response>", escape(&href), status_line(404)));
                continue;
            }
            let p = s.origin.path(&abs).unwrap_or_default();
            match locate(s, layout, &p) {
                Some(Res::Member(c, n, m)) => out.push_str(&response_for(s, layout, &Res::Member(c, n, m), &want)),
                _ => out.push_str(&format!("<d:response><d:href>{}</d:href><d:status>{}</d:status></d:response>", escape(&href), status_line(404))),
            }
        }
    }
    multistatus(&out, "")
}

fn sync(s: &State, layout: &Layout, coll: &Coll, body: &Node, want: &[(String, String)], _path: &str) -> HttpResponse {
    let given = body.value(DAV, "sync-token").unwrap_or("");
    let from = if given.is_empty() {
        0
    } else if s.quirks.token_expiry {
        return error_body(403, "d:valid-sync-token");
    } else {
        match given.strip_prefix("http://example.org/ns/sync/").and_then(|n| n.parse::<u64>().ok()) {
            Some(n) if n <= coll.rev => n,
            _ => return error_body(403, "d:valid-sync-token"),
        }
    };
    // The last event of each name after `from`; for a first sync, what exists.
    let mut latest: BTreeMap<&str, (u64, bool)> = BTreeMap::new();
    for (rev, name, present) in coll.log.iter().filter(|(r, ..)| *r > from) {
        latest.insert(name.as_str(), (*rev, *present));
    }
    let mut changes: Vec<(&str, u64, bool)> = latest.into_iter().map(|(n, (r, p))| (n, r, p)).collect();
    if from == 0 {
        changes.retain(|(_, _, present)| *present);
    }
    changes.sort_by_key(|(_, r, _)| *r);
    let limit = body.child(DAV, "limit").and_then(|l| l.value(DAV, "nresults")).and_then(|n| n.parse::<usize>().ok());
    let cap = [s.quirks.page_size, limit].into_iter().flatten().min();
    let truncated = cap.is_some_and(|c| changes.len() > c);
    if let Some(c) = cap {
        changes.truncate(c);
    }
    let end = if truncated { changes.last().map_or(from, |(_, r, _)| *r) } else { coll.rev };
    let mut out = String::new();
    for (name, _, present) in &changes {
        match coll.members.get_key_value(*name) {
            Some((n, m)) if *present => out.push_str(&response_for(s, layout, &Res::Member(coll, n.as_str(), m), want)),
            _ => {
                let p = format!("{}{name}", path_of(layout, &Res::Coll(coll)));
                out.push_str(&format!(
                    "<d:response><d:href>{}</d:href><d:status>{}</d:status></d:response>",
                    escape(&href_for(s, &p)),
                    status_line(404)
                ));
            }
        }
    }
    if truncated {
        out.push_str(&format!(
            "<d:response><d:href>{}</d:href><d:status>{}</d:status></d:response>",
            escape(&href_for(s, &path_of(layout, &Res::Coll(coll)))),
            status_line(507)
        ));
    }
    multistatus(&out, &format!("<d:sync-token>{}</d:sync-token>", token(end)))
}

fn put(s: &mut State, layout: &Layout, request: &HttpRequest, path: &str) -> HttpResponse {
    let Some((book, c, Some(name))) = split(layout, path.trim_end_matches('/')) else { return HttpResponse::new(409, Vec::new()) };
    let (c, name) = (c.to_owned(), name.to_owned());
    let Some(index) = s.colls.iter().position(|x| x.name == c && x.address_book == book) else { return HttpResponse::new(409, Vec::new()) };
    let exists = s.colls.get(index).is_some_and(|x| x.members.contains_key(&name));
    let if_match = request.field("if-match").map(str::to_owned);
    let none_match = request.field("if-none-match").map(str::to_owned);
    let ext = if book { ".vcf" } else { ".ics" };
    if !name.ends_with(ext) {
        s.violations.push("a resource name does not end as its kind does".into());
    }
    if s.colls.get(index).is_some_and(|x| x.read_only) {
        return error_body(403, "d:need-privileges");
    }
    let data = String::from_utf8_lossy(request.body()).into_owned();
    let (begin, ctype) = if book { ("BEGIN:VCARD", "text/vcard") } else { ("BEGIN:VCALENDAR", "text/calendar") };
    if !request.field("content-type").is_some_and(|t| t.starts_with(ctype)) || !data.trim_start().starts_with(begin) {
        return error_body(403, if book { "card:valid-address-data" } else { "c:valid-calendar-data" });
    }
    if data.len() > crate::request::MAX_RESOURCE {
        return HttpResponse::new(413, Vec::new());
    }
    if exists {
        match &if_match {
            None if none_match.as_deref() == Some("*") => return HttpResponse::new(412, Vec::new()),
            None => {
                s.violations.push("PUT of an existing resource without If-Match".into());
                return HttpResponse::new(428, Vec::new());
            }
            Some(tag) => {
                let current = s.colls.get(index).and_then(|x| x.members.get(&name)).map(|m| m.etag.clone());
                if tag != "*" && current.as_deref() != Some(tag.as_str()) {
                    return HttpResponse::new(412, Vec::new());
                }
            }
        }
    } else if none_match.as_deref() != Some("*") {
        s.violations.push("PUT that creates without If-None-Match: *".into());
        return HttpResponse::new(428, Vec::new());
    } else if if_match.is_some() {
        return HttpResponse::new(412, Vec::new());
    }
    let etag = format!("\"{}\"", s.next_etag);
    s.next_etag += 1;
    s.writes += 1;
    if let Some(x) = s.colls.get_mut(index) {
        x.rev += 1;
        let rev = x.rev;
        x.members.insert(name.clone(), Member { etag: etag.clone(), body: data });
        x.log.push((rev, name, true));
    }
    HttpResponse::new(if exists { 204 } else { 201 }, Vec::new()).with_field("ETag", &etag)
}

fn delete(s: &mut State, layout: &Layout, request: &HttpRequest, path: &str) -> HttpResponse {
    let Some((book, c, Some(name))) = split(layout, path.trim_end_matches('/')) else { return HttpResponse::new(404, Vec::new()) };
    let (c, name) = (c.to_owned(), name.to_owned());
    let Some(index) = s.colls.iter().position(|x| x.name == c && x.address_book == book) else { return HttpResponse::new(404, Vec::new()) };
    let current = s.colls.get(index).and_then(|x| x.members.get(&name)).map(|m| m.etag.clone());
    let Some(current) = current else { return HttpResponse::new(404, Vec::new()) };
    if s.colls.get(index).is_some_and(|x| x.read_only) {
        return error_body(403, "d:need-privileges");
    }
    match request.field("if-match") {
        None => {
            s.violations.push("DELETE without If-Match".into());
            return HttpResponse::new(428, Vec::new());
        }
        Some(tag) if tag != "*" && tag != current => return HttpResponse::new(412, Vec::new()),
        Some(_) => {}
    }
    s.writes += 1;
    if let Some(x) = s.colls.get_mut(index) {
        x.members.remove(&name);
        x.rev += 1;
        let rev = x.rev;
        x.log.push((rev, name, false));
    }
    HttpResponse::new(204, Vec::new())
}

#[cfg(test)]
#[path = "standin_tests.rs"]
mod tests;
