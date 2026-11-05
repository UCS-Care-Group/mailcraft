// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! CalDAV and CardDAV, for an account on a Linchange server whose mail goes over IMAP and SMTP.
//!
//! Linchange serves both on the same HTTPS listener as its web application: calendars under
//! `/caldav/` and address books under `/carddav/`, authenticated with the account's mail
//! password (HTTP Basic). [`probe`] asks a domain's mail exchanger whether it is such a server,
//! without sending anything of the owner's; [`sync`] walks the principal to the calendar and
//! address-book homes, lists every collection there (shared calendars and shared address books
//! are listed under the owner's own home too), and brings their events and cards back as the
//! model's [`EventsDownload`](mailcraft_model::remote::EventsDownload) and
//! [`ContactsDownload`](mailcraft_model::remote::ContactsDownload).
//!
//! Everything is as hostile as any other server answer: the body is capped by the HTTP layer,
//! the XML is read without a DTD and without expanding entities, its depth and size are bounded,
//! and iCalendar and vCard text goes through the readers written for exactly that.
//!
//! Only `https://` is spoken to (`http://` to this machine alone, as everywhere else). Nothing
//! of the request is logged, and a password never reaches an error.

use std::time::Duration;

use mailcraft_charset::base64::STANDARD as BASE64;
use mailcraft_dav::document::{Component, patch};
use mailcraft_model::dav::DavServer;
use mailcraft_model::remote::{
    CalendarsDownload, ContactChangeKind, ContactOp, ContactOutcome, ContactsDownload, ContactsUpload, ContactsUploaded, EventChangeKind,
    EventFields, EventOp, EventOutcome, EventResult, EventsUpload, EventsUploaded, RemoteAddressBook, RemoteCalendar, RemoteContact, RemoteEvent,
    RemoteEventKind,
};
use mailcraft_model::{CalendarOwner, Contact, Event, Rgb, Rights};
use mailcraft_time::{DateTime, Timestamp, Zone};
use quick_xml::Reader;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesRef, Event as XmlEvent};

use crate::NetError;
use crate::http::{self, Request as HttpRequest, Url};

/// The bodies sent with the requests.
const XML: &str = "application/xml; charset=utf-8";
const CALENDAR: &str = "text/calendar; charset=utf-8";
const VCARD: &str = "text/vcard; charset=utf-8";

/// How long one DAV request may take: the server is usually close, and an answer is a page of XML.
const PATIENCE: Duration = Duration::from_secs(30);
/// Largest answer body read of one request. A calendar query can hold a month of events.
const MAX_BODY: usize = 16 * 1024 * 1024;
/// Deepest XML the reader walks, and most elements and text it keeps.
const MAX_DEPTH: usize = 96;
const MAX_NODES: usize = 200_000;
const MAX_TEXT: usize = 16 * 1024 * 1024;
/// Most calendars and most objects of one collection taken from one answer.
const MAX_COLLECTIONS: usize = 200;
const MAX_OBJECTS: usize = 20_000;

/// Where a Linchange server keeps its CalDAV and CardDAV services.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub caldav: String,
    pub carddav: String,
}

/// What one run of [`sync`] brought.
#[derive(Clone, Debug, Default)]
pub struct DavSync {
    /// Every calendar the account may read, including the ones shared with it.
    pub calendars: CalendarsDownload,
    /// The events of each calendar in the stretch that was asked for.
    pub events: Vec<mailcraft_model::remote::EventsDownload>,
    /// Every contact of every address book the account may read, including shared books and the
    /// organization directory. Each says which book it came from.
    pub contacts: ContactsDownload,
    /// What one part of the run couldn't do, in words for the owner. An error here doesn't stop
    /// the rest: a calendar that can't be read doesn't hide the contacts.
    pub errors: Vec<String>,
}

/// Ask `host` (one of a domain's mail exchangers) whether it is a Linchange server: both services
/// answer an unauthenticated request with a challenge that names Linchange. Nothing of the
/// owner's is sent, and a server that answers otherwise is not one.
///
/// This blocks; a host calls it on a background thread.
pub fn probe(host: &str) -> Result<Option<Endpoints>, NetError> {
    let Some(endpoints) = DavServer::for_host(host) else { return Ok(None) };
    let Some(caldav) = discover_service(host, &endpoints.caldav, "CalDAV")? else { return Ok(None) };
    let Some(carddav) = discover_service(host, &endpoints.carddav, "CardDAV")? else { return Ok(None) };
    Ok(Some(Endpoints { caldav, carddav }))
}

/// Ask the RFC 6764 well-known address first, then the service root the server publishes. A
/// redirect from either is followed; the address the challenge came from is the service root.
fn discover_service(host: &str, base: &str, service: &str) -> Result<Option<String>, NetError> {
    if let Some(root) = service_root(host) {
        let well_known = format!("{root}.well-known/{}", service.to_ascii_lowercase());
        let (answer, final_url) = send_following(&well_known, "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024)?;
        if names_linchange(&answer, service) {
            return Ok(Some(service_url(&final_url)));
        }
    }
    let (answer, final_url) = send_following(base, "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024)?;
    Ok(names_linchange(&answer, service).then(|| service_url(&final_url)))
}

/// `https://host[:port]/` from the service root `DavServer` builds.
fn service_root(host: &str) -> Option<String> {
    let base = DavServer::for_host(host)?.caldav;
    base.rsplit_once("/caldav/").map(|(root, _)| format!("{root}/"))
}

/// The service root, with the trailing slash the paths below it are resolved against.
fn service_url(url: &str) -> String {
    if url.ends_with('/') { url.to_string() } else { format!("{url}/") }
}

/// Whether an unauthenticated DAV answer asks for a sign-in in Linchange's own realm.
fn names_linchange(answer: &http::Reply, service: &str) -> bool {
    // A server that lets a stranger read a collection is not the one this looks for.
    answer.status == 401
        && answer.challenges.iter().take(16).any(|line| line.to_ascii_lowercase().contains(&format!("linchange {service}").to_ascii_lowercase()))
}

/// Bring the calendars, events and contacts of an account down. `from` and `to` are the stretch
/// of the calendar to ask for (Unix seconds, UTC).
///
/// This blocks; a host calls it on a background thread.
pub fn sync(caldav: &str, carddav: &str, username: &str, password: &str, from: Timestamp, to: Timestamp) -> Result<DavSync, NetError> {
    let servers = DavServer { caldav: caldav.trim().to_string(), carddav: carddav.trim().to_string() };
    if let Some(why) = servers.problem() {
        let word = if DavServer::check_url(&servers.caldav).is_err() { "caldav" } else { "carddav" };
        return Err(NetError::Config(format!("the {word} address {why}")));
    }
    let auth = basic(username, password);
    let mut out = DavSync::default();
    match calendars(&servers.caldav, &auth, username, from, to) {
        Ok((list, mut events, mut errors)) => {
            out.calendars = list;
            out.events.append(&mut events);
            out.errors.append(&mut errors);
        }
        Err(e) => out.errors.push(format!("the calendars couldn't be read: {e}")),
    }
    match contacts(&servers.carddav, &auth) {
        Ok(list) => out.contacts = list,
        Err(e) => out.errors.push(format!("the address books couldn't be read: {e}")),
    }
    Ok(out)
}

/// `Basic` for `username:password`.
fn basic(username: &str, password: &str) -> String {
    format!("Basic {}", BASE64.encode(format!("{username}:{password}").as_bytes()))
}

fn send(
    url: &str,
    method: &str,
    depth: Option<&str>,
    body: Option<&str>,
    auth: &str,
    content_type: &str,
    max_body: usize,
) -> Result<http::Reply, NetError> {
    Ok(send_following(url, method, depth, body, auth, content_type, &[], max_body)?.0)
}

/// Send one request, following the redirects a DAV service may answer with (a service root
/// without its trailing slash, or the address a well-known name leads to). A redirect may not
/// leave the account's server: the credential goes with every request.
fn send_following(
    url: &str,
    method: &str,
    depth: Option<&str>,
    body: Option<&str>,
    auth: &str,
    content_type: &str,
    extra: &[(&str, &str)],
    max_body: usize,
) -> Result<(http::Reply, String), NetError> {
    const MAX_REDIRECTS: usize = 5;
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let reply = send_once(&current, method, depth, body, auth, content_type, extra, max_body)?;
        if matches!(reply.status, 301 | 302 | 303 | 307 | 308)
            && let Some(location) = reply.location.as_deref()
        {
            let Some(next) = resolve(&current, location) else {
                return Err(NetError::Forbidden("a DAV redirect led to another server".into()));
            };
            current = next;
            continue;
        }
        return Ok((reply, current));
    }
    Err(NetError::Forbidden("too many DAV redirects".into()))
}

fn send_once(
    url: &str,
    method: &str,
    depth: Option<&str>,
    body: Option<&str>,
    auth: &str,
    content_type: &str,
    extra: &[(&str, &str)],
    max_body: usize,
) -> Result<http::Reply, NetError> {
    let url = Url::parse(url)?;
    let mut headers: Vec<(&str, &str)> = vec![("Accept", "application/xml, text/xml")];
    if !auth.is_empty() {
        headers.push(("Authorization", auth));
    }
    if let Some(depth) = depth {
        headers.push(("Depth", depth));
    }
    if body.is_some() {
        headers.push(("Content-Type", content_type));
    }
    headers.extend(extra.iter().copied());
    let request = HttpRequest { method, headers: &headers, body: body.map(str::as_bytes), patience: PATIENCE, max_body };
    http::request(&url, &request)
}

/// PROPFIND `url` and read its multistatus.
fn propfind(url: &str, depth: &str, body: &str, auth: &str) -> Result<Vec<Response>, NetError> {
    let reply = send(url, "PROPFIND", Some(depth), Some(body), auth, XML, MAX_BODY)?;
    if reply.status != 207 {
        return Err(NetError::Forbidden(format!("the server answered {} to a request for its collections", reply.status)));
    }
    multistatus(&reply.body)
}

/// The collection list of a CalDAV service, and the events of each calendar.
fn calendars(
    base: &str,
    auth: &str,
    username: &str,
    from: Timestamp,
    to: Timestamp,
) -> Result<(CalendarsDownload, Vec<mailcraft_model::remote::EventsDownload>, Vec<String>), NetError> {
    let root = propfind(base, "0", DISCOVERY_CALDAV, auth)?;
    let home = root
        .iter()
        .find_map(|r| r.href_of("calendar-home-set"))
        .or_else(|| {
            root.iter()
                .find_map(|r| r.href_of("current-user-principal"))
                .and_then(|principal| principal_home(base, &principal, "calendar-home-set", auth))
        })
        .unwrap_or_else(|| base.to_string());
    let home = resolve(base, &home).ok_or_else(|| NetError::Forbidden("the server's calendar home is not on the account's server".into()))?;
    let listing = propfind(&home, "1", LIST_CALENDARS, auth)?;
    let mut out = CalendarsDownload { complete: true, ..Default::default() };
    let mut events = Vec::new();
    let mut errors = Vec::new();
    let mut chosen_main = false;
    if listing.len() > MAX_COLLECTIONS {
        out.complete = false;
        errors.push("more calendars were listed than MailCraft reads".into());
    }
    for response in listing.iter().take(MAX_COLLECTIONS) {
        if !response.has_resource_type("calendar") {
            continue;
        }
        let Some(href) = resolve(&home, &response.href) else {
            out.complete = false;
            errors.push("a calendar's address is not on the account's server".into());
            continue;
        };
        let name = response.prop_text("displayname").filter(|name| !name.trim().is_empty()).unwrap_or_else(|| slug(&href));
        let rights = response.rights();
        let color = response.prop_text("calendar-color").and_then(|color| color_of(&color));
        let owner = calendar_owner(response);
        // The account's own calendar is the one `DAV:owner` names, never just the first one the
        // server lists. If the server names none, no calendar is the main one and an event that
        // names no calendar waits until a target is known.
        let is_main = !chosen_main && response_owner_is(response, username);
        chosen_main |= is_main;
        out.calendars.push(RemoteCalendar {
            id: href.clone(),
            name: name.clone(),
            color,
            group: String::new(),
            rights,
            owner,
            main: is_main,
            online_providers: None,
        });
        match events_of(&href, auth, from, to, if is_main { "" } else { &href }) {
            Ok(download) => {
                out.complete &= download.complete;
                events.push(download);
            }
            // A calendar that can't be read doesn't hide the others; its events are simply not
            // brought this time, and the owner is told which calendar it was.
            Err(e) => {
                out.complete = false;
                errors.push(format!("the events of \"{name}\" couldn't be read: {e}"));
            }
        }
    }
    // A server with one unnamed collection, or a calendar not marked as one: use the home itself.
    if out.calendars.is_empty() {
        out.complete = false;
    }
    Ok((out, events, errors))
}

/// A principal's home set, read from its resource.
fn principal_home(base: &str, principal: &str, property: &str, auth: &str) -> Option<String> {
    let principal = resolve(base, principal)?;
    let body = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:A="urn:ietf:params:xml:ns:carddav"><D:prop><C:calendar-home-set/><A:addressbook-home-set/></D:prop></D:propfind>"#;
    let list = propfind(&principal, "0", body, auth).ok()?;
    list.iter().find_map(|r| r.href_of(property))
}

/// The events of one calendar in the stretch asked for.
fn events_of(
    calendar: &str,
    auth: &str,
    from: Timestamp,
    to: Timestamp,
    calendar_id: &str,
) -> Result<mailcraft_model::remote::EventsDownload, NetError> {
    let start = utc_text(from);
    let end = utc_text(to);
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:getetag/><C:calendar-data/></D:prop><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{start}" end="{end}"/></C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
    );
    let reply = send(calendar, "REPORT", Some("1"), Some(&body), auth, XML, MAX_BODY)?;
    if reply.status != 207 {
        return Err(NetError::Forbidden(format!("the server answered {} to a calendar query", reply.status)));
    }
    let responses = multistatus(&reply.body)?;
    let mut out = mailcraft_model::remote::EventsDownload { from, to, complete: true, calendar: calendar_id.to_string(), ..Default::default() };
    if responses.len() > MAX_OBJECTS {
        out.complete = false;
    }
    for response in responses.iter().take(MAX_OBJECTS) {
        let Some(text) = response.prop_text("calendar-data") else {
            // A listing that names an object but gives no document is not whole; removing what
            // is missing here would delete a calendar the server still has.
            out.complete = false;
            continue;
        };
        let etag = response.prop_text("getetag").unwrap_or_default();
        let parsed = mailcraft_ical::parse_events(&text, &|_| 0, 500);
        if parsed.events.is_empty() || parsed.events.len() >= 500 {
            out.complete = false;
            if parsed.events.is_empty() {
                continue;
            }
        }
        let many = parsed.events.len() > 1;
        for event in parsed.events {
            let uid = if event.uid.trim().is_empty() { slug(&response.href) } else { event.uid.clone() };
            let id = if many { format!("{}#{uid}", response.href) } else { response.href.clone() };
            out.events.push(remote_event(event, id, uid, etag.clone()));
        }
    }
    Ok(out)
}

/// A parsed iCalendar event as a server's event.
fn remote_event(event: mailcraft_model::Event, id: String, uid: String, etag: String) -> RemoteEvent {
    let start = event.start.to_timestamp(0);
    let end = event.end.max(event.start).to_timestamp(0);
    let series = event.recurrence.is_some();
    RemoteEvent {
        id,
        change_key: etag,
        uid,
        title: event.title,
        location: event.location,
        description: event.description,
        start,
        end: end.max(start),
        all_day: event.all_day,
        start_zone: event.start_zone,
        end_zone: event.end_zone,
        organizer: event.organizer,
        attendees: event.attendees,
        response: Default::default(),
        show_as: event.show_as,
        private: event.private,
        cancelled: event.cancelled,
        kind: if series { RemoteEventKind::SeriesMaster } else { RemoteEventKind::Single },
        series: String::new(),
        reminder_minutes: event.reminder_minutes,
        modified: 0,
        is_organizer: None,
        token: String::new(),
        recurrence: event.recurrence,
        series_day: series.then_some(event.start.date),
        original: None,
        categories: event.categories,
        importance: event.importance,
        options: event.response_options,
        online: event.online,
        proposals: Vec::new(),
        attachments: event.has_attachments,
    }
}

/// The address books of a CardDAV service, and every card in each of them.
fn contacts(base: &str, auth: &str) -> Result<ContactsDownload, NetError> {
    let root = propfind(base, "0", DISCOVERY_CARDDAV, auth)?;
    let home = root
        .iter()
        .find_map(|r| r.href_of("addressbook-home-set"))
        .or_else(|| {
            root.iter()
                .find_map(|r| r.href_of("current-user-principal"))
                .and_then(|principal| principal_home(base, &principal, "addressbook-home-set", auth))
        })
        .unwrap_or_else(|| base.to_string());
    let home = resolve(base, &home).ok_or_else(|| NetError::Forbidden("the server's address-book home is not on the account's server".into()))?;
    let listing = propfind(&home, "1", LIST_ADDRESSBOOKS, auth)?;
    let mut out = ContactsDownload { complete: true, ..Default::default() };
    let mut books = 0usize;
    if listing.len() > MAX_COLLECTIONS {
        out.complete = false;
    }
    for response in listing.iter().take(MAX_COLLECTIONS) {
        if !response.has_resource_type("addressbook") {
            continue;
        }
        books = books.saturating_add(1);
        let Some(href) = resolve(&home, &response.href) else {
            out.complete = false;
            continue;
        };
        let name = response.prop_text("displayname").filter(|name| !name.trim().is_empty()).unwrap_or_else(|| slug(&href));
        out.books.push(RemoteAddressBook { id: href.clone(), name: name.clone(), rights: response.rights() });
        match cards(&href, &href, &name, auth) {
            Ok((mut cards, complete)) => {
                out.complete &= complete;
                out.contacts.append(&mut cards);
            }
            Err(_) => out.complete = false,
        }
    }
    if books == 0 {
        // Every Linchange account has its personal book: none at all is a home that wasn't
        // found, and nothing is removed from the mailbox for it.
        out.complete = false;
    }
    if out.contacts.is_empty() && !out.complete {
        return Err(NetError::Forbidden("no address book could be read".into()));
    }
    Ok(out)
}

/// Every card of one address book.
fn cards(book: &str, book_id: &str, name: &str, auth: &str) -> Result<(Vec<RemoteContact>, bool), NetError> {
    let body = r#"<?xml version="1.0" encoding="utf-8"?><C:addressbook-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav"><D:prop><D:getetag/><C:address-data/></D:prop></C:addressbook-query>"#;
    let reply = send(book, "REPORT", Some("1"), Some(body), auth, XML, MAX_BODY)?;
    if reply.status != 207 {
        return Err(NetError::Forbidden(format!("the server answered {} to an address-book query", reply.status)));
    }
    let responses = multistatus(&reply.body)?;
    let mut out = Vec::new();
    let mut complete = responses.len() <= MAX_OBJECTS;
    for response in responses.iter().take(MAX_OBJECTS) {
        let Some(text) = response.prop_text("address-data") else {
            complete = false;
            continue;
        };
        let etag = response.prop_text("getetag").unwrap_or_default();
        let Some(card) = mailcraft_vcard::parse(&text).into_iter().next() else {
            complete = false;
            continue;
        };
        out.push(remote_contact(card, response.href.clone(), etag, name, book_id));
    }
    Ok((out, complete))
}

/// A parsed vCard as a server's contact.
fn remote_contact(card: mailcraft_model::Contact, id: String, etag: String, book: &str, book_id: &str) -> RemoteContact {
    RemoteContact {
        id,
        change_key: etag,
        uid: card.uid,
        display_name: card.display_name,
        first: card.first,
        middle: card.middle,
        last: card.last,
        nickname: card.nickname,
        company: card.company,
        title: card.title,
        department: card.department,
        emails: card.emails,
        phones: card.phones,
        addresses: card.addresses,
        websites: card.websites,
        birthday: card.birthday,
        notes: card.notes,
        book: book.to_string(),
        book_id: book_id.to_string(),
        modified: 0,
    }
}

/// `#RRGGBBAA` or `#RRGGBB` as a colour.
fn color_of(text: &str) -> Option<Rgb> {
    let text = text.trim();
    let six = text.chars().take(7).collect::<String>();
    Rgb::parse(&six)
}

/// The UTC text of an instant, as CalDAV writes it (`20260101T090000Z`).
fn utc_text(instant: Timestamp) -> String {
    let date = mailcraft_time::DateTime::from_timestamp(instant, 0);
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", date.date.year, date.date.month, date.date.day, date.hour, date.minute, date.second)
}

/// The last path segment of a URL, without its trailing slash.
fn slug(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    path.rsplit('/').next().unwrap_or_default().to_string()
}

/// Whose calendar a listed collection is, from the `DAV:owner` href a server sends.
///
/// A personal calendar names its owner's principal, a shared one the principal of the person who
/// shared it, and a team calendar names the group. Only the first two are a person, and the
/// address is what tells a calendar shared with the account from one of its own.
fn calendar_owner(response: &Response) -> Option<CalendarOwner> {
    let href = response.href_of("owner")?;
    let path = href.split(['?', '#']).next()?.trim_end_matches('/');
    // A team calendar is owned by its workgroup, which is not a person: there is no address to
    // read. It is still somebody else's calendar, so it keeps an owner (and is listed among the
    // shared ones) instead of looking like a feed nobody owns.
    if let Some((_, group)) = path.split_once("/workgroup/") {
        let id = group.trim_end_matches('/').split('/').next().unwrap_or_default();
        if !id.is_empty() {
            return CalendarOwner { address: format!("workgroup-{id}"), name: "Workgroup".into() }.cleaned();
        }
    }
    let address = percent_decoded(path.rsplit('/').next()?);
    (address.contains('@')).then(|| CalendarOwner { address, name: String::new() }.cleaned()).flatten()
}

/// Whether the principal in a `DAV:owner` href is the account's own. Some servers name an owner
/// by its login without the domain while the account signs in with an address.
fn response_owner_is(response: &Response, username: &str) -> bool {
    let Some(href) = response.href_of("owner") else { return false };
    let path = href.split(['?', '#']).next().unwrap_or(&href).trim_end_matches('/');
    let Some(last) = path.rsplit('/').next() else { return false };
    same_principal(&percent_decoded(last), username)
}

fn same_principal(owner: &str, username: &str) -> bool {
    let owner = owner.trim();
    let username = username.trim();
    if owner.eq_ignore_ascii_case(username) {
        return true;
    }
    let (owner_has_domain, username_has_domain) = (owner.contains('@'), username.contains('@'));
    if owner_has_domain && username_has_domain {
        return false;
    }
    let owner_local = owner.split('@').next().unwrap_or(owner);
    let username_local = username.split('@').next().unwrap_or(username);
    !owner_local.is_empty() && owner_local.eq_ignore_ascii_case(username_local)
}

/// A URL path segment with its percent escapes read back into text.
fn percent_decoded(text: &str) -> String {
    let hex = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1).copied(), bytes.get(at + 2).copied()) {
            (b'%', Some(high), Some(low)) if hex(high).is_some() && hex(low).is_some() => {
                out.push(hex(high).unwrap_or_default() * 16 + hex(low).unwrap_or_default());
                at += 3;
            }
            (byte, ..) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An absolute address from one a server wrote, which may be absolute, root-relative or relative.
fn resolve(base: &str, href: &str) -> Option<String> {
    // A resource may be named with the occurrence after `#`: the request goes to the resource.
    let href = href.split('#').next().unwrap_or(href).trim();
    if href.is_empty() {
        return None;
    }
    let base = Url::parse(base).ok()?;
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        let url = Url::parse(href).ok()?;
        // A server may name one of its own resources by the whole address, but nothing it says
        // may send the account's credentials to another server.
        return url.same_server(&base).then(|| canonical(&url));
    }
    let origin = format!(
        "{}://{}{}",
        if base.secure { "https" } else { "http" },
        if base.host.contains(':') { format!("[{}]", base.host) } else { base.host.clone() },
        if base.port == 0 { String::new() } else { format!(":{}", base.port) }
    );
    if href.starts_with('/') {
        return Url::parse(&format!("{origin}{href}")).ok().map(|url| canonical(&url));
    }
    let dir = base.path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    Url::parse(&format!("{origin}{dir}/{href}")).ok().map(|url| canonical(&url))
}

/// The path of an event or card on its server, without scheme or host. A write's answer may name
/// the whole URL while a download names only the path, and the two are the same resource.
fn resource_path(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if let Some((_, rest)) = path.split_once("://") {
        return rest.find('/').map(|at| rest[at..].to_string()).unwrap_or_else(|| "/".to_string());
    }
    path.to_string()
}

/// A stored resource URL made absolute for a request: one written by an older client may be only
/// the path the server lists, while the transport needs an address. `None` when the address is on
/// another server, so the account's credentials never go there.
fn request_url(base: &str, url: &str) -> Option<String> {
    let url = url.split('#').next().unwrap_or(url).trim();
    resolve(base, url)
}

fn canonical(url: &Url) -> String {
    format!(
        "{}://{}{}{}",
        if url.secure { "https" } else { "http" },
        if url.host.contains(':') { format!("[{}]", url.host) } else { url.host.clone() },
        if url.port == 0 { String::new() } else { format!(":{}", url.port) },
        url.path
    )
}

// --- what changes here go up --------------------------------------------------------------

/// Tell a CalDAV server what was made, changed or deleted here. `main_calendar` is the calendar
/// an event that says no calendar of its own belongs in (the account's main one). What couldn't
/// be done is in the outcomes, never an error as a whole.
///
/// This blocks; a host calls it on a background thread.
pub fn upload_events(caldav: &str, username: &str, password: &str, upload: &EventsUpload, main_calendar: &str) -> EventsUploaded {
    let auth = basic(username, password);
    let mut out = EventsUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    for op in upload.ops.iter().take(mailcraft_model::remote::MAX_EVENT_OPS) {
        out.outcomes.push(one_change(&auth, op, main_calendar, caldav));
    }
    out
}

/// A document the server gave us, with the bytes and validator needed to change it safely.
struct FetchedEvent {
    raw: String,
    etag: String,
    event: Event,
}

struct FetchedCard {
    raw: String,
    etag: String,
    card: Contact,
}

fn event_text(event: &Event) -> String {
    let options = mailcraft_ical::WriteOptions { now: mailcraft_time::now(), ..Default::default() };
    mailcraft_ical::write_in(&[event], &[], &options, Some(Zone::UTC))
}

fn condition(etag: &str, create: bool) -> Vec<(&'static str, &str)> {
    if create {
        vec![("If-None-Match", "*")]
    } else if etag.trim().is_empty() {
        Vec::new()
    } else {
        vec![("If-Match", etag.trim())]
    }
}

/// One change, as the server made of it.
fn one_change(auth: &str, op: &EventOp, main_calendar: &str, caldav: &str) -> EventOutcome {
    let mut outcome = EventOutcome { op: op.clone(), ..Default::default() };
    let calendar = if op.calendar.trim().is_empty() { main_calendar.trim() } else { op.calendar.trim() };
    // An occurrence or an override lives in the same resource as its series. Until the whole
    // series document can be changed, never delete or rewrite it from the wrong address.
    if (op.kind == EventChangeKind::Update || op.kind == EventChangeKind::Delete) && (op.of_occurrence() || op.remote.contains('#')) {
        outcome.result = EventResult::Refused;
        outcome.error = "one occurrence of a repeating event cannot be changed here yet".into();
        return outcome;
    }
    match op.kind {
        EventChangeKind::Create | EventChangeKind::Update => {
            let mut event = Event::default();
            let mut original = None;
            let mut expected = op.etag.trim().to_string();
            if op.kind == EventChangeKind::Update {
                let Some(url) = request_url(caldav, &op.remote) else {
                    outcome.result = EventResult::Failed;
                    outcome.error = "the event's address can't be used".into();
                    return outcome;
                };
                match fetch_event(&url, auth) {
                    Ok(Some(fetched)) => {
                        if !expected.is_empty() && !fetched.etag.is_empty() && expected != fetched.etag {
                            outcome.result = EventResult::Refused;
                            outcome.error = "the event changed on the server since it was read here".into();
                            return outcome;
                        }
                        if fetched.event.organizer.is_some() || !fetched.event.attendees.is_empty() {
                            outcome.result = EventResult::Refused;
                            outcome.error = "meetings are not changed through CalDAV yet".into();
                            return outcome;
                        }
                        event = fetched.event;
                        original = Some(fetched.raw);
                        if expected.is_empty() {
                            expected = fetched.etag;
                        }
                    }
                    Ok(None) => {
                        outcome.result = EventResult::Gone;
                        return outcome;
                    }
                    Err(e) => {
                        outcome.result = EventResult::Failed;
                        outcome.error = e.to_string();
                        return outcome;
                    }
                }
            } else if op.fields.attendees.as_ref().is_some_and(|people| !people.is_empty()) {
                outcome.result = EventResult::Refused;
                outcome.error = "meetings are not made through CalDAV yet".into();
                return outcome;
            }
            let uid = if op.kind == EventChangeKind::Create {
                if op.token.trim().is_empty() { format!("mailcraft-{}", op.event) } else { op.token.trim().to_string() }
            } else {
                let from_path = op.remote.rsplit('/').next().unwrap_or_default();
                let from_path = from_path.strip_suffix(".ics").unwrap_or(from_path).to_string();
                if event.uid.trim().is_empty() { from_path } else { event.uid.clone() }
            };
            event.uid.clone_from(&uid);
            let before = event_text(&event);
            apply_fields(&mut event, &op.fields);
            let after = event_text(&event);
            let text = match &original {
                Some(raw) => match patch(raw, &before, &after, Component::Event) {
                    Ok(text) => text,
                    Err(_) => {
                        outcome.result = EventResult::Refused;
                        outcome.error = "this event can't be changed without losing information".into();
                        return outcome;
                    }
                },
                None => after,
            };
            let url = if op.kind == EventChangeKind::Update && !op.remote.trim().is_empty() {
                let Some(url) = request_url(caldav, &op.remote) else {
                    outcome.result = EventResult::Failed;
                    outcome.error = "the event's address can't be used".into();
                    return outcome;
                };
                url
            } else {
                let base = if calendar.ends_with('/') { calendar.to_string() } else { format!("{calendar}/") };
                format!("{base}{}.ics", http::encode(&uid))
            };
            if calendar.is_empty() && op.kind == EventChangeKind::Create {
                outcome.result = EventResult::Failed;
                outcome.error = "the calendar is not known yet".into();
                return outcome;
            }
            let write_condition = condition(&expected, op.kind == EventChangeKind::Create);
            match send_following(&url, "PUT", None, Some(&text), auth, CALENDAR, &write_condition, 64 * 1024) {
                Ok((reply, _)) if matches!(reply.status, 200 | 201 | 204) => {
                    outcome.result = EventResult::Done;
                    outcome.remote = resource_path(&url);
                    outcome.uid = uid;
                    outcome.change_key = reply.etag.unwrap_or_default();
                    outcome.modified = mailcraft_time::now();
                    outcome.status = reply.status;
                }
                Ok((reply, _)) => {
                    outcome.result = match reply.status {
                        403 => EventResult::Denied,
                        404 | 410 => EventResult::Gone,
                        412 => EventResult::Refused,
                        409 | 428 => EventResult::Failed,
                        _ => EventResult::Refused,
                    };
                    if reply.status == 412 {
                        outcome.error = "the event changed on the server since it was read here".into();
                    }
                    outcome.status = reply.status;
                }
                Err(e) => {
                    outcome.result = EventResult::Failed;
                    outcome.error = e.to_string();
                }
            }
        }
        EventChangeKind::Delete => {
            if op.remote.trim().is_empty() {
                outcome.result = EventResult::Failed;
                outcome.error = "the event has no address on the server".into();
                return outcome;
            }
            let Some(url) = request_url(caldav, &op.remote) else {
                outcome.result = EventResult::Failed;
                outcome.error = "the event's address can't be used".into();
                return outcome;
            };
            let fetched = match fetch_event(&url, auth) {
                Ok(Some(fetched)) => fetched,
                Ok(None) => {
                    outcome.result = EventResult::Gone;
                    return outcome;
                }
                Err(e) => {
                    outcome.result = EventResult::Failed;
                    outcome.error = e.to_string();
                    return outcome;
                }
            };
            if !op.etag.trim().is_empty() && !fetched.etag.is_empty() && op.etag.trim() != fetched.etag {
                outcome.result = EventResult::Refused;
                outcome.error = "the event changed on the server since it was read here".into();
                return outcome;
            }
            let write_condition = condition(&fetched.etag, false);
            match send_following(&url, "DELETE", None, None, auth, XML, &write_condition, 64 * 1024) {
                Ok((reply, _)) if matches!(reply.status, 200 | 204) => outcome.result = EventResult::Done,
                Ok((reply, _)) if reply.status == 404 || reply.status == 410 => outcome.result = EventResult::Gone,
                Ok((reply, _)) if reply.status == 403 => outcome.result = EventResult::Denied,
                Ok((reply, _)) if reply.status == 412 => {
                    outcome.result = EventResult::Refused;
                    outcome.error = "the event changed on the server since it was read here".into();
                }
                Ok((reply, _)) => {
                    outcome.result = EventResult::Failed;
                    outcome.status = reply.status;
                }
                Err(e) => {
                    outcome.result = EventResult::Failed;
                    outcome.error = e.to_string();
                }
            }
        }
        // Answers to invitations and forwards are not a calendar write: the server has nothing
        // here that says it, and asking again would not help.
        EventChangeKind::Respond | EventChangeKind::Forward => {
            outcome.result = EventResult::Refused;
            outcome.error = "this server takes answers to invitations through mail".into();
        }
    }
    outcome
}

/// The event a resource holds now, for a change to be merged into it.
fn fetch_event(url: &str, auth: &str) -> Result<Option<FetchedEvent>, NetError> {
    if url.trim().is_empty() {
        return Ok(None);
    }
    let reply = send(url.trim(), "GET", None, None, auth, XML, MAX_BODY)?;
    match reply.status {
        200 => {
            let raw = String::from_utf8_lossy(&reply.body).into_owned();
            let event = mailcraft_ical::parse(&raw, &|_| 0)
                .events
                .into_iter()
                .next()
                .ok_or_else(|| NetError::Proto(mailcraft_proto::ProtoError::Protocol("the server's event isn't an iCalendar document".into())))?;
            Ok(Some(FetchedEvent { raw, etag: reply.etag.unwrap_or_default(), event }))
        }
        404 | 410 => Ok(None),
        status => Err(NetError::Forbidden(format!("the server answered {status} for the event"))),
    }
}

/// The card a resource holds now, for a change to be merged into it.
fn fetch_card(url: &str, auth: &str) -> Result<Option<FetchedCard>, NetError> {
    if url.trim().is_empty() {
        return Ok(None);
    }
    let reply = send(url.trim(), "GET", None, None, auth, XML, MAX_BODY)?;
    match reply.status {
        200 => {
            let raw = String::from_utf8_lossy(&reply.body).into_owned();
            let card = mailcraft_vcard::parse(&raw)
                .into_iter()
                .next()
                .ok_or_else(|| NetError::Proto(mailcraft_proto::ProtoError::Protocol("the server's card isn't a vCard document".into())))?;
            Ok(Some(FetchedCard { raw, etag: reply.etag.unwrap_or_default(), card }))
        }
        404 | 410 => Ok(None),
        status => Err(NetError::Forbidden(format!("the server answered {status} for the card"))),
    }
}

/// What a change says of an event, applied to the event as it is.
fn apply_fields(event: &mut Event, fields: &EventFields) {
    if let Some(title) = &fields.title {
        event.title.clone_from(title);
    }
    if let Some(location) = &fields.location {
        event.location.clone_from(location);
    }
    if let Some(description) = &fields.description {
        event.description.clone_from(description);
    }
    if let Some(time) = fields.time {
        event.all_day = time.all_day;
        event.start = DateTime::from_timestamp(time.start, 0);
        event.end = DateTime::from_timestamp(time.end.max(time.start), 0);
        if time.all_day {
            event.start_zone.clear();
            event.end_zone.clear();
        }
    }
    // A change that names its zones keeps them; one that doesn't leaves the event's own zone
    // alone, so a recurring nine o'clock does not become an instant that moves at a DST change.
    if let Some(zones) = &fields.zones {
        if event.all_day {
            event.start_zone.clear();
            event.end_zone.clear();
        } else {
            event.start_zone.clone_from(&zones.start);
            event.end_zone = if zones.end == zones.start { String::new() } else { zones.end.clone() };
        }
    }
    if let Some(show) = fields.show_as {
        event.show_as = show;
    }
    if let Some(private) = fields.private {
        event.private = private;
    }
    if let Some(reminder) = fields.reminder {
        event.reminder_minutes = reminder.on.then_some(reminder.minutes);
    }
    if let Some(repeat) = &fields.repeat {
        event.recurrence.clone_from(&repeat.rule);
    }
    if let Some(attendees) = &fields.attendees {
        event.attendees.clone_from(attendees);
    }
    if let Some(categories) = &fields.categories {
        event.categories.clone_from(categories);
    }
    if let Some(importance) = fields.importance {
        event.importance = importance;
    }
}

/// Put the contacts made here into the address books chosen for them.
///
/// A contact is new, so its URL on the server is the one its `UID` makes in that book: a second
/// run of an upload whose answer was lost writes the same card again rather than a second one.
///
/// This blocks; a host calls it on a background thread.
pub fn upload_contacts(carddav: &str, username: &str, password: &str, upload: &ContactsUpload) -> ContactsUploaded {
    let auth = basic(username, password);
    let mut out = ContactsUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    for op in upload.ops.iter().take(mailcraft_model::remote::MAX_CONTACT_OPS) {
        out.outcomes.push(one_contact(&auth, op, carddav));
    }
    out
}

/// One contact change, as the server made of it.
fn one_contact(auth: &str, op: &ContactOp, carddav: &str) -> ContactOutcome {
    let mut outcome = ContactOutcome { op: op.clone(), ..Default::default() };
    if op.kind == ContactChangeKind::Delete {
        if op.remote.trim().is_empty() {
            outcome.result = EventResult::Failed;
            outcome.error = "the contact has no address on the server".into();
            return outcome;
        }
        let Some(url) = request_url(carddav, &op.remote) else {
            outcome.result = EventResult::Failed;
            outcome.error = "the contact's address can't be used".into();
            return outcome;
        };
        outcome.remote = resource_path(&url);
        let fetched = match fetch_card(&url, auth) {
            Ok(Some(fetched)) => fetched,
            Ok(None) => {
                outcome.result = EventResult::Gone;
                return outcome;
            }
            Err(e) => {
                outcome.result = EventResult::Failed;
                outcome.error = e.to_string();
                return outcome;
            }
        };
        if !op.etag.trim().is_empty() && !fetched.etag.is_empty() && op.etag.trim() != fetched.etag {
            outcome.result = EventResult::Refused;
            outcome.error = "the contact changed on the server since it was read here".into();
            return outcome;
        }
        let write_condition = condition(&fetched.etag, false);
        match send_following(&url, "DELETE", None, None, auth, XML, &write_condition, 64 * 1024) {
            Ok((reply, _)) if matches!(reply.status, 200 | 204) => {
                outcome.result = EventResult::Done;
                outcome.status = reply.status;
            }
            Ok((reply, _)) if reply.status == 404 || reply.status == 410 => {
                outcome.result = EventResult::Gone;
                outcome.status = reply.status;
            }
            Ok((reply, _)) if reply.status == 403 => {
                outcome.result = EventResult::Denied;
                outcome.status = reply.status;
            }
            Ok((reply, _)) if reply.status == 412 => {
                outcome.result = EventResult::Refused;
                outcome.error = "the contact changed on the server since it was read here".into();
            }
            Ok((reply, _)) => {
                outcome.result = EventResult::Failed;
                outcome.status = reply.status;
            }
            Err(e) => {
                outcome.result = EventResult::Failed;
                outcome.error = e.to_string();
            }
        }
        return outcome;
    }

    let book = op.book.trim();
    if op.uid.trim().is_empty() {
        outcome.result = EventResult::Failed;
        outcome.error = "the contact has no UID to be saved under".into();
        return outcome;
    }
    let mut expected = op.etag.trim().to_string();
    let mut original = None;
    let url = if op.kind == ContactChangeKind::Update && !op.remote.trim().is_empty() {
        let Some(url) = request_url(carddav, &op.remote) else {
            outcome.result = EventResult::Failed;
            outcome.error = "the contact's address can't be used".into();
            return outcome;
        };
        match fetch_card(&url, auth) {
            Ok(Some(fetched)) => {
                if !expected.is_empty() && !fetched.etag.is_empty() && expected != fetched.etag {
                    outcome.result = EventResult::Refused;
                    outcome.error = "the contact changed on the server since it was read here".into();
                    return outcome;
                }
                original = Some((fetched.raw, fetched.card));
                if expected.is_empty() {
                    expected = fetched.etag;
                }
                url
            }
            Ok(None) => {
                outcome.result = EventResult::Gone;
                return outcome;
            }
            Err(e) => {
                outcome.result = EventResult::Failed;
                outcome.error = e.to_string();
                return outcome;
            }
        }
    } else {
        if book.is_empty() {
            outcome.result = EventResult::Failed;
            outcome.error = "the address book is not known yet".into();
            return outcome;
        }
        let base = if book.ends_with('/') { book.to_string() } else { format!("{book}/") };
        format!("{base}{}.vcf", http::encode(op.uid.trim()))
    };
    let text = match original {
        Some((raw, before)) => {
            let before = mailcraft_vcard::write(&[&before]);
            match patch(&raw, &before, &op.card, Component::Card) {
                Ok(text) => text,
                Err(_) => {
                    outcome.result = EventResult::Refused;
                    outcome.error = "this contact can't be changed without losing information".into();
                    return outcome;
                }
            }
        }
        None => op.card.clone(),
    };
    outcome.remote = resource_path(&url);
    let write_condition = condition(&expected, op.kind == ContactChangeKind::Create);
    match send_following(&url, "PUT", None, Some(&text), auth, VCARD, &write_condition, 64 * 1024) {
        Ok((reply, _)) if matches!(reply.status, 200 | 201 | 204) => {
            outcome.result = EventResult::Done;
            outcome.status = reply.status;
        }
        Ok((reply, _)) => {
            outcome.result = match reply.status {
                403 => EventResult::Denied,
                404 | 410 => EventResult::Gone,
                412 => EventResult::Refused,
                _ => EventResult::Failed,
            };
            if reply.status == 412 {
                outcome.error = "the contact changed on the server since it was read here".into();
            }
            outcome.status = reply.status;
        }
        Err(e) => {
            outcome.result = EventResult::Failed;
            outcome.error = e.to_string();
        }
    }
    outcome
}

// --- the multistatus document -------------------------------------------------------------

/// One `response` of a multistatus: the resource's address and the properties it has.
#[derive(Clone, Debug, Default)]
struct Response {
    href: String,
    props: Vec<Node>,
}

impl Response {
    fn prop(&self, name: &str) -> Option<&Node> {
        self.props.iter().find(|node| node.name == name)
    }

    fn prop_text(&self, name: &str) -> Option<String> {
        self.props.iter().filter(|node| node.name == name).find_map(|node| {
            let text = node.text.trim();
            (!text.is_empty()).then(|| text.to_string())
        })
    }

    fn href_of(&self, property: &str) -> Option<String> {
        self.props.iter().filter(|node| node.name == property).find_map(|node| {
            let href = node.child("href")?.text.trim();
            (!href.is_empty()).then(|| href.to_string())
        })
    }

    fn has_resource_type(&self, kind: &str) -> bool {
        self.props.iter().filter(|node| node.name == "resourcetype").any(|node| node.child(kind).is_some())
    }

    fn rights(&self) -> Rights {
        let Some(set) = self.prop("current-user-privilege-set") else { return Rights::Edit };
        if set.has("write") || set.has("write-content") || set.has("all") { Rights::Edit } else { Rights::Read }
    }
}

/// An element of the document, its text and the elements inside it.
#[derive(Clone, Debug, Default)]
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|node| node.name == name)
    }

    /// Whether an element of this name is here or anywhere below.
    fn has(&self, name: &str) -> bool {
        self.name == name || self.children.iter().any(|node| node.has(name))
    }
}

/// The `response` elements of a multistatus.
fn multistatus(body: &[u8]) -> Result<Vec<Response>, NetError> {
    let root = parse_xml(body)
        .ok_or_else(|| NetError::Proto(mailcraft_proto::ProtoError::Protocol("the server's answer isn't the XML it should be".into())))?;
    let responses = &root.children;
    let mut out = Vec::new();
    for response in responses.iter().filter(|node| node.name == "response").take(MAX_COLLECTIONS.saturating_mul(MAX_OBJECTS).min(MAX_OBJECTS * 2)) {
        let Some(href) = response.child("href").map(|node| node.text.trim().to_string()).filter(|href| !href.is_empty()) else { continue };
        let mut props = Vec::new();
        for propstat in response.children.iter().filter(|node| node.name == "propstat") {
            if let Some(prop) = propstat.child("prop") {
                props.extend(prop.children.iter().cloned());
            }
        }
        if let Some(prop) = response.child("prop") {
            props.extend(prop.children.iter().cloned());
        }
        out.push(Response { href, props });
    }
    Ok(out)
}

/// Read an XML document into elements, without a DTD and without expanding an entity that isn't
/// one of XML's own five or a character reference.
fn parse_xml(bytes: &[u8]) -> Option<Node> {
    let mut reader = Reader::from_reader(bytes);
    let config = reader.config_mut();
    (config.check_end_names, config.allow_unmatched_ends, config.expand_empty_elements) = (true, false, false);
    let mut stack: Vec<Node> = vec![Node { name: String::new(), ..Default::default() }];
    let mut nodes = 0usize;
    let mut text = 0usize;
    // Every event takes at least a byte of the document, which bounds the loop.
    for _ in 0..=bytes.len().saturating_mul(2).max(1) {
        match reader.read_event() {
            Ok(XmlEvent::Start(element)) => {
                if stack.len() > MAX_DEPTH || nodes >= MAX_NODES {
                    return None;
                }
                nodes += 1;
                stack.push(Node { name: local_name(&element), ..Default::default() });
            }
            Ok(XmlEvent::Empty(element)) => {
                if stack.len() > MAX_DEPTH || nodes >= MAX_NODES {
                    return None;
                }
                nodes += 1;
                let node = Node { name: local_name(&element), ..Default::default() };
                stack.last_mut()?.children.push(node);
            }
            Ok(XmlEvent::End(_)) => {
                if stack.len() <= 1 {
                    return None;
                }
                let node = stack.pop()?;
                stack.last_mut()?.children.push(node);
            }
            Ok(XmlEvent::Text(value)) => {
                let value = value.xml10_content();
                text = text.saturating_add(value.len());
                if text > MAX_TEXT {
                    return None;
                }
                stack.last_mut()?.text.push_str(&value);
            }
            Ok(XmlEvent::CData(value)) => {
                let value = value.xml10_content();
                text = text.saturating_add(value.len());
                if text > MAX_TEXT {
                    return None;
                }
                stack.last_mut()?.text.push_str(&value);
            }
            Ok(XmlEvent::GeneralRef(reference)) => {
                let decoded = decoded_reference(&reference)?;
                text = text.saturating_add(decoded.len());
                if text > MAX_TEXT {
                    return None;
                }
                stack.last_mut()?.text.push_str(&decoded);
            }
            Ok(XmlEvent::DocType(_)) => return None,
            Ok(XmlEvent::Comment(_) | XmlEvent::Decl(_) | XmlEvent::PI(_)) => {}
            Ok(XmlEvent::Eof) => break,
            Err(_) => return None,
        }
    }
    if stack.len() != 1 || stack.first()?.children.is_empty() {
        return None;
    }
    let mut root = stack.pop()?;
    // The multistatus itself (or, for a document that wraps it, the first element).
    root.children.first()?;
    Some(root.children.remove(0))
}

fn local_name(element: &quick_xml::events::BytesStart<'_>) -> String {
    element.local_name().as_ref().to_string()
}

fn decoded_reference(reference: &BytesRef<'_>) -> Option<String> {
    let mut one = [0u8; 4];
    match reference.resolve_char_ref() {
        Ok(Some(c)) => Some(c.encode_utf8(&mut one).to_string()),
        Ok(None) => resolve_xml_entity(reference).map(str::to_string),
        Err(_) => None,
    }
}

// --- the request bodies -------------------------------------------------------------------

const DISCOVERY_CALDAV: &str = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:current-user-principal/><C:calendar-home-set/><D:resourcetype/></D:prop></D:propfind>"#;
const DISCOVERY_CARDDAV: &str = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:" xmlns:A="urn:ietf:params:xml:ns:carddav"><D:prop><D:current-user-principal/><A:addressbook-home-set/><D:resourcetype/></D:prop></D:propfind>"#;
const LIST_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:A="http://apple.com/ns/ical/"><D:prop><D:displayname/><D:resourcetype/><D:current-user-privilege-set/><D:owner/><A:calendar-color/></D:prop></D:propfind>"#;
const LIST_ADDRESSBOOKS: &str = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav"><D:prop><D:displayname/><D:resourcetype/><D:current-user-privilege-set/></D:prop></D:propfind>"#;

// (The documents above are the whole of what is sent; nothing of the owner's is in them.)
#[allow(dead_code)]
fn _documents_are_sent_as_written() {
    let _ = (DISCOVERY_CALDAV, DISCOVERY_CARDDAV, LIST_CALENDARS, LIST_ADDRESSBOOKS);
}

#[cfg(test)]
mod tests;
