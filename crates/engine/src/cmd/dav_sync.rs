// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of keeping an account's calendars and contacts in step with a CalDAV and
//! CardDAV server (#209): what to ask (`sync.davRequest`), what came back (`sync.applyDav`), what
//! was changed here and goes up (`sync.davChanges`) and what the server made of it
//! (`sync.applyDavUploaded`). As for every server the engine does no I/O with it: a host with
//! sockets (`mailcraft_net::dav`) does the part in between.
//!
//! **Calendars down (D5).** `sync.davRequest` says per collection what the host asks: the sync
//! token or change tag kept at the last complete read, and what is here already (by address and
//! ETag), so that only what is new or changed is fetched, and a read that stopped short goes on
//! where it stopped. `sync.applyDav`
//! takes the answer in: the calendars of the account's homes (each a calendar here, read-only until
//! changes are sent up, D6) and each collection's changed resources. A resource is one `VCALENDAR`;
//! `mailcraft_ical` reads it into events (a single event, or a series with the days it skips and
//! its changed occurrences as events of their own, as for a subscribed calendar), each kept under
//! the resource's address and ETag (`Origin::id`, `Origin::change_key`; a changed occurrence under
//! the address and `#` and its own mark). The document itself is kept as it came
//! ([`crate::dav_raw`], ADR 0026).
//!
//! Everything the answer says is hostile: an address must be on an origin of the account's homes
//! and inside the collection it was read from; counts, sizes and texts are capped; a token or
//! change tag moves only with a read that went to its end (or, for changes, to the end of a page
//! that was taken in whole), and only when every change in it was taken in here; and a read made
//! under settings that have changed since is not taken in at all.
//!
//! **Nothing the server still has is removed here.** A resource goes only on the server's own
//! word: a change that says it is gone, or a whole listing of its collection
//! ([`DavRead::members`](mailcraft_model::DavRead::members)) that doesn't name it; a calendar only
//! when a whole listing of the homes doesn't. A read that stopped short, a time range, a
//! recurrence worked out here or a document that isn't a whole calendar never removes anything.
//! Where the server's copy and a change here meet, the server's wins (v1): nothing here is
//! changed yet, so nothing waits.
//!
//! The commands are the host's, never an agent's, and ship switched off (`MAILCRAFT_DAV=1`).

use std::collections::{HashMap, HashSet};

use mailcraft_model::dav_down::{MAX_ACCOUNT_RESOURCES, MAX_ACCOUNT_TEXT, MAX_CHANGES, MAX_KNOWN, MAX_READ, MAX_RESOURCES};
use mailcraft_model::remote::MAX_KEY;
use mailcraft_model::{
    Calendar, DavAsk, DavAskCollection, DavChange, DavCollectionState, DavDownload, DavKnown, DavListing, DavProblem, DavRead, DavReadState,
    DavServer, Event, Id, Origin, Rights, origin_of,
};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

/// What the commands that send changes up say while they have nothing to do yet.
const NOT_YET: &str = "Changes to calendars and contacts on a server are not sent yet in this version.";

/// A look at the account's calendars (which it has) is repeated after this many seconds.
const RELIST_AFTER: i64 = 3_600;
/// A resource the server gave no ETag for is read again after this many seconds: nothing else
/// shows that it changed.
const REREAD_AFTER: i64 = 86_400;
/// At most this many resources an account is remembered by in [`Session::dav_seen`].
const MAX_SEEN: usize = 50_000;

/// A resource read in that no event here names by its ETag ([`Session::dav_seen`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The collection it is in.
    pub collection: String,
    /// The ETag it came with (empty: none).
    pub etag: String,
    /// When it was read (Unix seconds).
    pub at: i64,
    /// It gave no event here (a to-do, or a document that isn't a calendar).
    pub bare: bool,
}
/// Most events one resource is taken to hold (a series with its changed occurrences).
const MAX_PER_RESOURCE: usize = 1_000;
/// Most calendars a mailbox has (as for those made elsewhere).
const MAX_CALENDARS: usize = 200;
/// Longest line (a title, a place, a name, a category) and note kept of a server's event, and
/// most categories, attendees and skipped days: as for a subscribed calendar.
const MAX_LINE: usize = 500;
const MAX_NOTE: usize = 100_000;
const MAX_CATEGORIES: usize = 50;
const MAX_ATTENDEES: usize = 500;
const MAX_EXCEPTIONS: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.davRequest", "Send/Receive Calendars and Contacts Server", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "skip"?: [number] (accounts to pass over), "relist"?: bool (look for calendars that came or went now)} — answers what a host should read from an account's calendar server: its homes, the calendars known with their sync token, change tag and what is here (by address and ETag), and the settings the read is made under (`server`, handed back in the download). Answers {"ask": DavAsk} or {"ask": null} when no account has one."#)
            .when(available),
        CommandSpec::new("sync.applyDav", "Apply Calendars and Contacts Server", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": DavDownload} — takes in what a host read from a calendar server: the calendars its homes list, and each collection's changed or gone resources. Refused, and nothing taken in, when the download was read under settings that have changed since. Answers {"added", "updated", "removed", "calendars", "skipped", "problem", "wait"}."#)
            .when(super::dav_account::switched_on),
        CommandSpec::new("sync.davChanges", "Calendar and Contacts Changes to Send", "Send / Receive", changes)
            .pure()
            .params(r#"{"account"?: number} — answers what was changed here that a host should send to an account's calendar and contacts server. Not built yet: it refuses."#)
            .when(available),
        CommandSpec::new("sync.applyDavUploaded", "Apply Uploaded Calendar and Contacts Changes", "Send / Receive", uploaded)
            .pure()
            .params(r#"{"result": DavUploaded} — takes in what a calendar and contacts server made of changes sent to it. Not built yet: it refuses."#)
            .when(super::dav_account::switched_on),
    ]
}

/// Why the commands are out of reach, when they are: the feature is switched off, or no account
/// has a calendar and contacts server.
fn available(s: &Session) -> Option<&'static str> {
    super::dav_account::switched_on(s)
        .or_else(|| if s.mailbox.accounts.iter().any(|a| a.dav.is_some()) { None } else { Some("no account has a calendar and contacts server") })
}

fn changes(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}

fn uploaded(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}

/// The account has no calendar server any more, or another one: what the old one gave goes (its
/// calendars, named by `collections`, their events and the documents kept as they came), and the
/// homes are listed afresh. Nothing else of the account is touched: what a Graph sign-in gave it
/// (calendars, events, to-dos, contacts) and what waits to be sent to it stay, as does whatever
/// was made here.
pub(super) fn forget_account(s: &mut Session, account: Id, collections: &[String]) {
    // (A read asked for before this is not taken in, even when the settings come back as they were.)
    let generation = s.dav_generation.entry(account).or_default();
    *generation = generation.wrapping_add(1);
    let hrefs: HashSet<&str> = collections.iter().map(String::as_str).collect();
    let followed: Vec<Id> =
        s.mailbox.calendars.iter().filter(|c| c.origin.account == account && hrefs.contains(c.origin.id.as_str())).map(|c| c.id).collect();
    let followed_set: HashSet<Id> = followed.iter().copied().collect();
    let events: HashSet<Id> = s
        .mailbox
        .events
        .values()
        .filter(|e| followed_set.contains(&e.calendar) && e.origin.account == account && !e.origin.id.is_empty())
        .map(|e| e.id)
        .collect();
    for id in &events {
        let _ = s.mailbox.remove_event(*id);
    }
    s.mailbox.event_changes.retain(|c| !(c.account == account && events.contains(&c.event)));
    // A calendar with something of the owner's left in it becomes the mailbox's own. And one
    // calendar of the mailbox's own always stays, for new events to go in (as `calendar.remove`
    // keeps one): a subscribed copy of a calendar on the web is not one.
    let in_use: HashSet<Id> = s.mailbox.events.values().map(|e| e.calendar).filter(|c| followed_set.contains(c)).collect();
    let mut stays: Vec<Id> = followed.iter().copied().filter(|id| in_use.contains(id)).collect();
    let own_elsewhere = s
        .mailbox
        .calendars
        .iter()
        .any(|c| !followed_set.contains(&c.id) && super::remote::keeper(&s.mailbox, &c.origin).is_none() && !c.is_subscribed());
    if stays.is_empty()
        && !own_elsewhere
        && let Some(first) = followed.first()
    {
        stays.push(*first);
    }
    for c in s.mailbox.calendars.iter_mut().filter(|c| stays.contains(&c.id)) {
        c.origin = Origin::default();
        c.rights = Rights::default();
    }
    s.mailbox.calendars.retain(|c| !followed_set.contains(&c.id) || stays.contains(&c.id));
    s.dav_listed.remove(&account);
    s.dav_asked.remove(&account);
    s.dav_seen.remove(&account);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(file) = s.path.clone() {
        crate::dav_raw::drop_all(&file, account);
    }
    if !followed.is_empty() {
        s.dirty = true;
        s.mailbox_changed();
    }
}

/// The part of an item's key that is its resource's address.
fn resource_of(key: &str) -> &str {
    key.split('#').next().unwrap_or(key)
}

// ---- asking ---------------------------------------------------------------------------------

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let wanted = v.get("account").and_then(Value::as_u64);
    // Accounts the host is not to be asked about now (waiting out a back-off, or refused).
    let skip: HashSet<u64> =
        v.get("skip").and_then(Value::as_array).map(|list| list.iter().take(1_000).filter_map(Value::as_u64).collect()).unwrap_or_default();
    // The account whose server was asked longest ago (or never): each has its turn.
    let account = s
        .mailbox
        .accounts
        .iter()
        .filter(|a| a.dav.as_ref().is_some_and(|d| !d.calendar_homes.is_empty()) && wanted.is_none_or(|id| id == a.id) && !skip.contains(&a.id))
        .min_by_key(|a| (s.dav_asked.get(&a.id).copied().unwrap_or(i64::MIN), a.id))
        .ok_or_else(|| fail("no account has a calendar server"))?;
    let Some(server) = account.dav.as_ref() else { return Err(fail("no account has a calendar server")) };
    let now = s.now();
    let listed = s.dav_listed.get(&account.id).copied().unwrap_or(0);
    let relist = v.get("relist").and_then(Value::as_bool).unwrap_or(false);
    let calendars: Vec<&DavCollectionState> = server.collections.iter().filter(|c| !c.contacts).take(DavServer::MAX_COLLECTIONS).collect();
    let list_homes = relist || calendars.is_empty() || now.saturating_sub(listed) >= RELIST_AFTER || listed > now;
    // (Only a collection with a calendar here is read: one that found no room is never fetched.)
    let held: HashSet<&str> = s.mailbox.calendars.iter().filter(|c| c.origin.account == account.id).map(|c| c.origin.id.as_str()).collect();
    let mut collections: Vec<DavAskCollection> = calendars
        .iter()
        .filter(|state| held.contains(state.href.as_str()))
        .map(|state| DavAskCollection { href: state.href.clone(), sync_token: state.sync_token.clone(), ctag: state.ctag.clone(), known: Vec::new() })
        .collect();
    // What is here, by collection, in one look at the events: a read (and one that goes on after a
    // read that stopped short) fetches only what is new or changed. A resource the server gave no
    // ETag for is left out once a day, so that it is read again.
    let by_calendar: HashMap<Id, usize> = s
        .mailbox
        .calendars
        .iter()
        .filter(|c| c.origin.account == account.id)
        .filter_map(|c| collections.iter().position(|ask| ask.href == c.origin.id).map(|at| (c.id, at)))
        .collect();
    let read_in = s.dav_seen.get(&account.id);
    // (Without an ETag, a resource is read again once a day.)
    let due = |href: &str, etag: &str| {
        etag.is_empty() && read_in.and_then(|f| f.get(href)).is_none_or(|r| now.saturating_sub(r.at) >= REREAD_AFTER || r.at > now)
    };
    let mut seen: Vec<HashSet<&str>> = vec![HashSet::new(); collections.len()];
    // What was read in without giving an event here: it is here too, as far as fetching goes. It
    // is named first: a new version that gave no event (a document that can't be read) is what
    // the server has now, not the event kept of the version before, which would otherwise be
    // fetched again at every run.
    let by_href: HashMap<String, usize> = collections.iter().enumerate().map(|(at, c)| (c.href.clone(), at)).collect();
    for (href, r) in read_in.into_iter().flatten().filter(|(_, r)| r.bare) {
        let Some(&at) = by_href.get(r.collection.as_str()) else { continue };
        let (Some(ask), Some(seen)) = (collections.get_mut(at), seen.get_mut(at)) else { continue };
        if ask.known.len() < MAX_KNOWN && seen.insert(href.as_str()) && !due(href, &r.etag) {
            ask.known.push(DavKnown { href: href.clone(), etag: r.etag.clone() });
        }
    }
    for event in s.mailbox.events.values().filter(|e| e.origin.account == account.id && !e.origin.id.is_empty()) {
        let (Some(&at), href) = (by_calendar.get(&event.calendar), resource_of(&event.origin.id)) else { continue };
        let (Some(ask), Some(seen)) = (collections.get_mut(at), seen.get_mut(at)) else { continue };
        if href.is_empty() || ask.known.len() >= MAX_KNOWN || !seen.insert(href) {
            continue;
        }
        let etag = &event.origin.change_key;
        if !due(href, etag) {
            ask.known.push(DavKnown { href: href.to_string(), etag: etag.clone() });
        }
    }
    let ask = DavAsk {
        account: account.id,
        email: account.email.trim().to_string(),
        username: if server.username.trim().is_empty() { account.email.trim().to_string() } else { server.username.trim().to_string() },
        trusted: server.trusted.clone(),
        homes: server.calendar_homes.clone(),
        list_homes,
        server: server.identity(&account.email),
        collections,
        no_room: s.mailbox.calendars.len() >= MAX_CALENDARS,
        generation: s.dav_generation.get(&account.id).copied().unwrap_or(0),
    };
    // (After every other, even within one second.)
    let turn = now.max(s.dav_asked.values().copied().max().map_or(i64::MIN, |last| last.saturating_add(1)));
    s.dav_asked.insert(ask.account, turn);
    Ok(json!({"ask": ask}))
}

// ---- taking in ------------------------------------------------------------------------------

/// What a download came to.
#[derive(Default)]
struct Took {
    added: usize,
    updated: usize,
    removed: usize,
    calendars: usize,
    skipped: usize,
    /// Something found no room here (events, their text, or a calendar).
    full: bool,
}

/// Whether `href` is a resource inside `collection` on the same origin: the collection's address
/// is a prefix of it at a segment, with something after, and no dot segment (plain or escaped) in
/// what follows. (The host resolves addresses already; this is the engine's own look, as a
/// download is a command's parameter too.)
pub(crate) fn within(collection: &str, href: &str) -> bool {
    let (Some(a), Some(b)) = (origin_of(collection), origin_of(href)) else { return false };
    if a != b || href.len() > MAX_KEY || href.contains(['#', '\\', ' ']) || href.chars().any(char::is_control) {
        return false;
    }
    let prefix = if collection.ends_with('/') { collection.to_string() } else { format!("{collection}/") };
    let Some(rest) = href.strip_prefix(prefix.as_str()) else { return false };
    let path = rest.split('?').next().unwrap_or("");
    !path.is_empty()
        && path.split('/').all(|seg| {
            let seg = seg.to_ascii_lowercase();
            seg != "." && seg != ".." && seg != "%2e" && seg != "%2e%2e" && seg != ".%2e" && seg != "%2e."
        })
}

/// Whether `raw` is a whole `VCALENDAR`: it begins with one and ends it. A document cut short, an
/// error page or nothing at all says nothing of what the resource holds, and is not taken to say
/// that its events are gone.
fn whole_calendar(raw: &str) -> bool {
    let mut lines = raw.trim_start_matches('\u{feff}').lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next().is_some_and(|l| l.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
        && lines.next_back().is_some_and(|l| l.eq_ignore_ascii_case("END:VCALENDAR"))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let download = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    // (Nothing of the parameter in the error: it is the server's text, and may be logged.)
    let download: DavDownload = serde_json::from_value(download).map_err(|_| bad("`result` is not what a calendar server's read brings"))?;
    let took = take_in(s, &download)?;
    // (What the engine found full is said, unless the host has something worse to say.)
    let problem = if took.full && matches!(download.problem, DavProblem::None | DavProblem::Limits) { DavProblem::Full } else { download.problem };
    Ok(json!({
        "added": took.added, "updated": took.updated, "removed": took.removed, "calendars": took.calendars,
        "skipped": took.skipped.saturating_add(download.skipped as usize),
        "problem": serde_json::to_value(problem).unwrap_or(Value::Null),
        "wait": download.wait.min(600),
    }))
}

/// The documents kept as they came, for one taking-in: the account's folder is listed at the
/// first need, and once.
struct Docs<'a> {
    file: Option<&'a std::path::Path>,
    account: Id,
    store: Option<raw::Store>,
}

impl Docs<'_> {
    fn store(&mut self) -> Option<&mut raw::Store> {
        let file = self.file?;
        Some(self.store.get_or_insert_with(|| raw::Store::open(file, self.account)))
    }
}

fn take_in(s: &mut Session, download: &DavDownload) -> Result<Took, CmdError> {
    let owner = super::remote::owner(&s.mailbox, download.account, &download.email)?;
    let account = owner.id;
    let server = owner.dav.clone().ok_or_else(|| fail("this account has no calendar server"))?;
    // A read made under other settings (another user name, server or certificate) is the old
    // server's: nothing of it is taken in, before anything is touched.
    if download.server != server.identity(&owner.email) {
        return Err(fail("this read was made under the calendar server's earlier settings"));
    }
    // And one asked for before the settings were put away or changed, even if they are as they
    // were again: what it knew of the account was forgotten since.
    if download.generation != s.dav_generation.get(&account).copied().unwrap_or(0) {
        return Err(fail("this read was made under the calendar server's earlier settings"));
    }
    let mut took = Took::default();
    let offset_at = super::remote::offset_of(s);
    let now = s.now();
    let file = s.path.clone();
    let mut docs = Docs { file: file.as_deref(), account, store: None };
    let mut changed = false;
    let homes = server.home_origins();
    let ok_collection = |href: &str| DavServer::check_home(href).is_ok() && origin_of(href).is_some_and(|o| homes.contains(&o));

    if let Some(listing) = &download.listing {
        changed |= take_listing(s, account, listing, &ok_collection, &mut took, file.as_deref());
        if listing.complete {
            s.dav_listed.insert(account, now);
        }
    }
    // (Counted once: each read adds to it. What waits to be sent is looked at once, too.)
    let mut room = s
        .mailbox
        .events
        .values()
        .filter(|e| e.origin.account == account && !e.origin.id.is_empty())
        .fold(Room::default(), |r, e| Room { total: r.total.saturating_add(1), text: r.text.saturating_add(text_size(e)) });
    let pending: HashSet<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
    let mut reads = 0usize;
    let mut done: HashSet<&str> = HashSet::new();
    for read in &download.collections {
        // (A collection read twice in one download is taken in once.)
        if !done.insert(read.href.as_str()) {
            took.skipped += 1;
            continue;
        }
        let Some(state) =
            s.mailbox.account(account).and_then(|a| a.dav.as_ref()).and_then(|d| d.collections.iter().find(|c| c.href == read.href)).cloned()
        else {
            took.skipped += 1;
            continue;
        };
        if state.contacts || !ok_collection(&state.href) || read.state != DavReadState::Read {
            continue;
        }
        reads += 1;
        if reads > DavServer::MAX_COLLECTIONS {
            break;
        }
        let Some(calendar) = s.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id == state.href).map(|c| c.id) else {
            took.skipped += 1;
            continue;
        };
        let to = Target { account, calendar, collection: &state.href, now, pending: &pending };
        let read_in = take_read(s, to, read, &offset_at, &mut docs, &mut room, &mut took);
        changed |= read_in.changed;
        // The state moves with what was taken in, and only when all of it was: a change that
        // found no room here is asked for again, from where the token was.
        if !read_in.all {
            continue;
        }
        let token = if read.sync_token.chars().count() <= DavServer::MAX_TOKEN { read.sync_token.clone() } else { String::new() };
        let tag = if read.ctag.chars().count() <= DavServer::MAX_TOKEN { read.ctag.clone() } else { String::new() };
        if let Some(d) = s.mailbox.account_mut(account)?.dav.as_mut()
            && let Some(c) = d.collections.iter_mut().find(|c| c.href == read.href)
        {
            let new_ctag = if read.complete { tag } else { c.ctag.clone() };
            // (A read that stopped short with nothing to go on from leaves the token where it
            // was: asked again from there, the server answers as it did, and nothing is lost.)
            let token = if read.complete || !token.is_empty() { token } else { c.sync_token.clone() };
            if c.sync_token != token || c.ctag != new_ctag {
                (c.sync_token, c.ctag) = (token, new_ctag);
                changed = true;
            }
        }
    }
    if changed || took.added + took.updated + took.removed + took.calendars > 0 {
        s.dirty = true;
        s.mailbox_changed();
    }
    if download.problem != DavProblem::None {
        // (Nothing that names the account or the server.)
        log::info!("a calendar server's read ended early: {:?}", download.problem);
    }
    Ok(took)
}

/// The calendars the homes list: each is one here, read-only. Whether anything changed. A
/// calendar the server no longer lists goes only when the listing is whole: it said it went to
/// its end, nothing was capped, and every calendar in it was taken in here.
fn take_listing(
    s: &mut Session,
    account: Id,
    listing: &DavListing,
    ok_collection: &dyn Fn(&str) -> bool,
    took: &mut Took,
    file: Option<&std::path::Path>,
) -> bool {
    let mut changed = false;
    let mut whole = listing.complete && !listing.capped && listing.calendars.len() <= DavServer::MAX_COLLECTIONS;
    let mut listed: Vec<&mailcraft_model::DavCalendar> = Vec::new();
    for calendar in listing.calendars.iter().take(DavServer::MAX_COLLECTIONS) {
        let events = calendar.components.is_empty() || calendar.components.iter().any(|c| c == "VEVENT");
        if !ok_collection(&calendar.href) {
            // (Never to be held here: but it is a calendar, so the listing is not all there is.)
            took.skipped += 1;
            whole = false;
        } else if events && listed.iter().all(|l| l.href != calendar.href) {
            listed.push(calendar);
        }
    }
    if whole {
        // What the server no longer lists goes (first, so that a calendar that replaces one finds
        // its room), with its events and their documents.
        let keep: HashSet<&str> = listed.iter().map(|l| l.href.as_str()).collect();
        let gone: Vec<String> = s
            .mailbox
            .account(account)
            .and_then(|a| a.dav.as_ref())
            .map(|d| d.collections.iter().filter(|c| !c.contacts && !keep.contains(c.href.as_str())).map(|c| c.href.clone()).collect())
            .unwrap_or_default();
        let pending: HashSet<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
        for href in gone {
            let ids: HashSet<Id> = s.mailbox.calendars.iter().filter(|c| c.origin.account == account && c.origin.id == href).map(|c| c.id).collect();
            // (Only what the server gave goes, as when the server is put away: anything else in
            // the calendar stays, and the calendar with it, as the mailbox's own.)
            let events: Vec<Id> = s
                .mailbox
                .events
                .values()
                .filter(|e| ids.contains(&e.calendar) && e.origin.account == account && !e.origin.id.is_empty())
                .map(|e| e.id)
                .collect();
            // (A calendar with something in it that waits to be sent stays until that is settled.)
            if events.iter().any(|e| pending.contains(e)) {
                continue;
            }
            for event in events {
                let _ = s.mailbox.remove_event(event);
                took.removed += 1;
            }
            let in_use: HashSet<Id> = s.mailbox.events.values().map(|e| e.calendar).filter(|c| ids.contains(c)).collect();
            for c in s.mailbox.calendars.iter_mut().filter(|c| in_use.contains(&c.id)) {
                c.origin = Origin::default();
                c.rights = Rights::default();
            }
            s.mailbox.calendars.retain(|c| !ids.contains(&c.id) || in_use.contains(&c.id));
            if let Ok(a) = s.mailbox.account_mut(account)
                && let Some(d) = a.dav.as_mut()
            {
                d.collections.retain(|c| c.href != href);
            }
            if let Some(read_in) = s.dav_seen.get_mut(&account) {
                read_in.retain(|_, r| r.collection != href);
            }
            took.calendars += 1;
            changed = true;
        }
    }
    for item in &listed {
        // (No control or format character, as for an event's title.)
        let name: String = mailcraft_names::incoming(&item.name, mailcraft_model::MAX_NAME).unwrap_or_default();
        let name = if name.trim().is_empty() { "Calendar".to_string() } else { name.trim().to_string() };
        // A calendar the mailbox has no room for is not kept, so never read (`DavAsk::no_room`),
        // and that is said.
        let here = s.mailbox.calendars.iter().any(|c| c.origin.account == account && c.origin.id == item.href);
        if !here && s.mailbox.calendars.len() >= MAX_CALENDARS {
            took.skipped += 1;
            took.full = true;
            continue;
        }
        let Ok(a) = s.mailbox.account_mut(account) else { return changed };
        let Some(d) = a.dav.as_mut() else { return changed };
        let room = d.collections.len() < DavServer::MAX_COLLECTIONS;
        match d.collections.iter_mut().find(|c| c.href == item.href) {
            Some(c) => {
                let components: Vec<String> = item.components.iter().take(16).cloned().collect();
                if (&c.name, c.color, c.read_only, &c.components) != (&name, item.color, item.read_only, &components) {
                    (c.name, c.color, c.read_only, c.components) = (name.clone(), item.color, item.read_only, components);
                    changed = true;
                }
            }
            None if room => {
                d.collections.push(DavCollectionState {
                    href: item.href.clone(),
                    name: name.clone(),
                    color: item.color,
                    read_only: item.read_only,
                    components: item.components.iter().take(16).cloned().collect(),
                    ..Default::default()
                });
                changed = true;
            }
            None => {
                took.skipped += 1;
                continue;
            }
        }
        // The calendar here. Read-only for now: nothing is sent up yet.
        let mailbox = &mut s.mailbox;
        match mailbox.calendars.iter().position(|c| c.origin.account == account && c.origin.id == item.href) {
            Some(at) => {
                if let Some(c) = mailbox.calendars.get_mut(at) {
                    let color = item.color.unwrap_or(c.color);
                    if (&c.name, c.color, c.rights) != (&name, color, Rights::Read) {
                        (c.name, c.color, c.rights) = (name.clone(), color, Rights::Read);
                        took.calendars += 1;
                    }
                }
            }
            None if mailbox.calendars.len() < MAX_CALENDARS => {
                let color = item.color.unwrap_or_else(|| super::calendar::next_color(mailbox));
                let id = mailbox.alloc_id();
                let origin = Origin { account, id: item.href.clone(), ..Default::default() };
                mailbox.calendars.push(Calendar { id, name, color, visible: true, origin, rights: Rights::Read, ..Default::default() });
                took.calendars += 1;
            }
            None => took.skipped += 1,
        }
    }
    if whole {
        sweep(s, account, file);
    }
    changed
}

/// What the account's events from its server take up here: how many, and their text in bytes
/// ([`text_size`]). Both are capped ([`MAX_ACCOUNT_RESOURCES`], [`MAX_ACCOUNT_TEXT`]).
#[derive(Default)]
struct Room {
    total: usize,
    text: usize,
}

impl Room {
    /// An event of `size` went.
    fn gone(&mut self, size: usize) {
        self.total = self.total.saturating_sub(1);
        self.text = self.text.saturating_sub(size);
    }
}

/// The collection a read is of, and where its events go.
struct Target<'a> {
    account: Id,
    calendar: Id,
    collection: &'a str,
    now: i64,
    /// The events with something that waits to be sent: never changed or removed by a read.
    pending: &'a HashSet<Id>,
}

/// What taking in one read came to.
struct ReadIn {
    /// Something here changed.
    changed: bool,
    /// Every change was taken in, or can never be (an address off the collection, a document
    /// that is too large or no calendar): the read's token may be kept. Not when a change found
    /// no room here.
    all: bool,
}

/// Takes in what one collection's read brought.
///
/// A resource is removed here only on the server's word: a change that says it is gone, or a whole
/// listing of the collection ([`DavRead::members`]) that doesn't name it. Never for anything else.
#[allow(clippy::too_many_arguments)]
fn take_read(
    s: &mut Session,
    to: Target<'_>,
    read: &DavRead,
    offset_at: &dyn Fn(i64) -> i32,
    docs: &mut Docs<'_>,
    room: &mut Room,
    took: &mut Took,
) -> ReadIn {
    let Target { account, calendar, collection, now, pending } = to;
    let owner = s.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
    // What is here of this collection, by key, and the keys of each resource (so that a change
    // looks at its own resource's and not at all).
    let mut here: HashMap<String, Id> = HashMap::new();
    let mut of_resource: HashMap<String, Vec<String>> = HashMap::new();
    // (Only what the server gave: an event without an address is the mailbox's own, and no
    // listing of the server's can say it is gone.)
    for event in s.mailbox.events.values().filter(|e| e.calendar == calendar && e.origin.account == account && !e.origin.id.is_empty()) {
        here.insert(event.origin.id.clone(), event.id);
        of_resource.entry(resource_of(&event.origin.id).to_string()).or_default().push(event.origin.id.clone());
    }
    let before = (took.added, took.updated, took.removed);
    let mut all = read.changes.len() <= MAX_CHANGES;
    let mut named: HashSet<&str> = HashSet::new();
    for change in read.changes.iter().take(MAX_CHANGES) {
        if !within(collection, &change.href) {
            took.skipped += 1;
            continue;
        }
        named.insert(change.href.as_str());
        // What it brings, and whether that is all of it (when not, nothing of it is removed).
        let mut fresh: Vec<(String, Event)> = Vec::new();
        let mut whole = true;
        if !change.gone {
            let raw = change.raw.as_deref().filter(|r| r.len() <= MAX_READ && change.etag.len() <= MAX_KEY);
            let Some(raw) = raw.filter(|r| whole_calendar(r)) else {
                // (Not a calendar this can read: what is here of it stays, and it is not
                // fetched again until it changes.)
                took.skipped += 1;
                remember(s, account, collection, change, now, true);
                continue;
            };
            let events = mailcraft_ical::parse(raw, offset_at).events;
            let parsed = events.len();
            fresh = keyed(&change.href, events);
            // (Each event the document holds came out of it: a part the reader couldn't make out is
            // no word that what was here of it went.)
            let written = raw.lines().filter(|l| l.trim().eq_ignore_ascii_case("BEGIN:VEVENT")).count();
            whole = parsed <= MAX_PER_RESOURCE && fresh.len() == parsed && parsed >= written;
        }
        let old: Vec<(String, Id)> =
            of_resource.get(change.href.as_str()).into_iter().flatten().filter_map(|k| here.get(k).map(|i| (k.clone(), *i))).collect();
        let new_keys: HashSet<String> = fresh.iter().map(|(k, _)| k.clone()).collect();
        for (key, id) in old.iter().filter(|(k, _)| (change.gone || whole) && !new_keys.contains(k)) {
            let size = s.mailbox.event(*id).map_or(0, text_size);
            if !pending.contains(id) && s.mailbox.remove_event(*id).is_ok() {
                here.remove(key);
                if let Some(keys) = of_resource.get_mut(change.href.as_str()) {
                    keys.retain(|k| k != key);
                }
                room.gone(size);
                took.removed += 1;
            }
        }
        // (When its events end, for the documents' room: a series is as late as can be.)
        let end = fresh.iter().map(|(_, e)| if e.recurrence.is_some() { i64::MAX } else { e.instants(offset_at).1 }).max().unwrap_or(i64::MIN);
        let gives = !fresh.is_empty();
        let mut partly = false;
        for (key, mut event) in fresh {
            event.calendar = calendar;
            event.origin = Origin {
                account,
                id: key.clone(),
                change_key: change.etag.clone(),
                series: if key == change.href { String::new() } else { change.href.clone() },
                guest: !owner.is_empty()
                    && event.organizer.as_ref().is_some_and(|o| !o.email.trim().is_empty() && !o.email.trim().eq_ignore_ascii_case(&owner)),
                occurrence: None,
            };
            tidy(&mut event);
            let size = text_size(&event);
            match here.get(&key).copied() {
                Some(id) if pending.contains(&id) => {}
                Some(id) => {
                    event.id = id;
                    let old = s.mailbox.event(id).map_or(0, text_size);
                    if s.mailbox.event(id) == Some(&event) {
                        // (As it was.)
                    } else if room.text.saturating_sub(old).saturating_add(size) > MAX_ACCOUNT_TEXT {
                        // (No room for the new version: the one here stays as it is, and the new
                        // one is asked for again.)
                        took.skipped += 1;
                        took.full = true;
                        all = false;
                        partly = true;
                    } else if let Ok(slot) = s.mailbox.event_mut(id) {
                        *slot = event;
                        room.text = room.text.saturating_sub(old).saturating_add(size);
                        took.updated += 1;
                    }
                }
                None if room.total < MAX_ACCOUNT_RESOURCES && room.text.saturating_add(size) <= MAX_ACCOUNT_TEXT => {
                    if let Ok(id) = s.mailbox.add_event(event) {
                        of_resource.entry(change.href.clone()).or_default().push(key.clone());
                        here.insert(key, id);
                        room.total = room.total.saturating_add(1);
                        room.text = room.text.saturating_add(size);
                        took.added += 1;
                    }
                }
                None => {
                    // (No room: asked for again, so the token stays; and said.)
                    took.skipped += 1;
                    took.full = true;
                    all = false;
                    partly = true;
                }
            }
        }
        if change.gone {
            forget_seen(s, account, &change.href);
        } else if partly {
            // Not all of it is here: it isn't named as here under its ETag, so that it is fetched
            // again (and nothing of it remembered as read).
            for key in of_resource.get(change.href.as_str()).into_iter().flatten() {
                if let Some(id) = here.get(key)
                    && let Ok(event) = s.mailbox.event_mut(*id)
                {
                    event.origin.change_key.clear();
                }
            }
            forget_seen(s, account, &change.href);
        } else {
            remember(s, account, collection, change, now, !gives);
        }
        if let Some(store) = docs.store() {
            store.drop_one(&change.href);
            // (Kept for an event to be edited by: a document that gave none is not kept.)
            if let Some(doc) = change.raw.as_deref().filter(|_| !change.gone && gives) {
                // (What is asked of the events when there is no room is how they are now, once.)
                store.keep(&change.href, doc, end, &|| ends_of(s, account, offset_at));
            }
        }
    }
    // What a whole listing of the collection doesn't name is gone: a resource goes whole, its
    // series, its changed occurrences and all, unless something here waits to be sent for it.
    if let Some(members) = read.members.as_ref().filter(|m| m.len() <= MAX_RESOURCES) {
        let present: HashSet<&str> = members.iter().map(String::as_str).chain(named.iter().copied()).collect();
        // (What is held of the collection now: a listing that takes all of it is believed, but its
        // token and change tag are not kept, so that the next run looks again.)
        let held: usize = of_resource.values().map(Vec::len).sum();
        let mut swept = 0usize;
        let gone: Vec<String> = of_resource
            .iter()
            .filter(|(resource, keys)| {
                !present.contains(resource.as_str()) && !keys.is_empty() && keys.iter().all(|k| here.get(k).is_some_and(|id| !pending.contains(id)))
            })
            .map(|(resource, _)| resource.clone())
            .collect();
        for resource in gone {
            for key in of_resource.get(&resource).into_iter().flatten() {
                let Some(id) = here.get(key) else { continue };
                let size = s.mailbox.event(*id).map_or(0, text_size);
                if s.mailbox.remove_event(*id).is_ok() {
                    room.gone(size);
                    took.removed += 1;
                    swept += 1;
                }
            }
            if let Some(store) = docs.store() {
                store.drop_one(&resource);
            }
        }
        if held > 0 && swept == held {
            all = false;
        }
        if let Some(read_in) = s.dav_seen.get_mut(&account) {
            read_in.retain(|href, r| r.collection != collection || present.contains(href.as_str()));
        }
    }
    ReadIn { changed: (took.added, took.updated, took.removed) != before, all }
}

/// Remembers a resource read in that no event here can be told by: one without an ETag (so that
/// it is read again after a day) or one that gave no event (so that it is not fetched again until
/// it changes). Anything else is forgotten.
fn remember(s: &mut Session, account: Id, collection: &str, change: &DavChange, now: i64, bare: bool) {
    if !bare && !change.etag.is_empty() {
        forget_seen(s, account, &change.href);
        return;
    }
    let read_in = s.dav_seen.entry(account).or_default();
    if read_in.len() < MAX_SEEN || read_in.contains_key(&change.href) {
        // (An ETag past the cap is none: the resource is then read again after a day.)
        let etag = if change.etag.len() <= MAX_KEY { change.etag.clone() } else { String::new() };
        let seen = Seen { collection: collection.to_string(), etag, at: now, bare };
        read_in.insert(change.href.clone(), seen);
    }
}

fn forget_seen(s: &mut Session, account: Id, href: &str) {
    if let Some(read_in) = s.dav_seen.get_mut(&account) {
        read_in.remove(href);
    }
}

/// The end of each of the account's events that has a kept document, by the document's name, as
/// the events are now (a series that never ends is as late as can be).
fn ends_of(s: &Session, account: Id, offset_at: &dyn Fn(i64) -> i32) -> HashMap<String, i64> {
    let mut ends: HashMap<String, i64> = HashMap::new();
    for e in s.mailbox.events.values().filter(|e| e.origin.account == account && !e.origin.id.is_empty()) {
        let end = if e.recurrence.is_some() { i64::MAX } else { e.instants(offset_at).1 };
        let slot = ends.entry(raw::name_of(resource_of(&e.origin.id))).or_insert(i64::MIN);
        *slot = (*slot).max(end);
    }
    ends
}

/// Lets go of the documents no event names any more.
fn sweep(s: &Session, account: Id, file: Option<&std::path::Path>) {
    let Some(file) = file else { return };
    let named: HashSet<String> = s
        .mailbox
        .events
        .values()
        .filter(|e| e.origin.account == account && !e.origin.id.is_empty())
        .map(|e| resource_of(&e.origin.id).to_string())
        .collect();
    raw::sweep(file, account, &named);
}

/// The events of one resource under the keys they are kept by: the first that is not a changed
/// occurrence under the resource's address (the series, or the event), the others under it and
/// `#` and their own uid. At most [`MAX_PER_RESOURCE`], none twice, none whose key is too long.
fn keyed(href: &str, events: Vec<Event>) -> Vec<(String, Event)> {
    let uids: HashSet<String> = events.iter().map(|e| e.uid.clone()).collect();
    let (mut main, mut others): (Vec<Event>, Vec<Event>) =
        events.into_iter().take(MAX_PER_RESOURCE).partition(|e| !e.uid.split_once('#').is_some_and(|(base, _)| uids.contains(base)));
    main.append(&mut others);
    let mut out: Vec<(String, Event)> = Vec::new();
    let mut keys: HashSet<String> = HashSet::new();
    for (n, event) in main.into_iter().enumerate() {
        let key = if n == 0 { href.to_string() } else { format!("{href}#{}", if event.uid.is_empty() { n.to_string() } else { event.uid.clone() }) };
        if key.len() <= MAX_KEY && keys.insert(key.clone()) {
            out.push((key, event));
        }
    }
    out
}

/// A server's event cut to what the mailbox keeps: no control or format character (a
/// right-to-left override makes a title read as something else), every text capped, as for a
/// subscribed calendar.
fn tidy(event: &mut Event) {
    let line = |text: &str| mailcraft_names::incoming(text, MAX_LINE).unwrap_or_default();
    let plain =
        |text: &str, max: usize| -> String { text.chars().filter(|c| !c.is_control() && !mailcraft_names::is_format(*c)).take(max).collect() };
    event.title = line(&event.title);
    event.location = line(&event.location);
    event.description = event
        .description
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || !(c.is_control() || mailcraft_names::is_format(*c)))
        .take(MAX_NOTE)
        .collect();
    event.uid = plain(&event.uid, MAX_KEY);
    event.start_zone = plain(&event.start_zone, MAX_LINE);
    event.end_zone = plain(&event.end_zone, MAX_LINE);
    event.categories.truncate(MAX_CATEGORIES);
    event.categories = event.categories.iter().filter_map(|c| mailcraft_names::incoming(c, MAX_LINE)).collect();
    event.attendees.truncate(MAX_ATTENDEES);
    for person in event.organizer.iter_mut().chain(event.attendees.iter_mut().map(|a| &mut a.address)) {
        person.name = line(&person.name);
        person.email = plain(&person.email, MAX_LINE);
    }
    if let Some(online) = event.online.as_mut() {
        online.join_url = plain(&online.join_url, MAX_KEY);
    }
    event.exceptions.truncate(MAX_EXCEPTIONS);
}

/// The bytes of text an event of the server's takes up here, counted against [`MAX_ACCOUNT_TEXT`]
/// (each skipped day as the size of a date).
pub(crate) fn text_size(e: &Event) -> usize {
    let people = e
        .organizer
        .iter()
        .chain(e.attendees.iter().map(|a| &a.address))
        .fold(0usize, |n, p| n.saturating_add(p.name.len()).saturating_add(p.email.len()));
    let categories = e.categories.iter().fold(0usize, |n, c| n.saturating_add(c.len()));
    [
        e.title.len(),
        e.location.len(),
        e.description.len(),
        e.uid.len(),
        e.start_zone.len(),
        e.end_zone.len(),
        e.origin.id.len(),
        e.origin.change_key.len(),
        e.origin.series.len(),
        e.online.as_ref().map_or(0, |o| o.join_url.len()),
        e.exceptions.len().saturating_mul(std::mem::size_of::<mailcraft_time::Date>()),
        people,
        categories,
    ]
    .iter()
    .fold(0usize, |n, part| n.saturating_add(*part))
}

/// The kept documents, where there is a place to keep them.
#[cfg(not(target_arch = "wasm32"))]
mod raw {
    pub(super) use crate::dav_raw::{Store, name_of, sweep};
}

/// Without a folder to keep them in (the browser): none is kept.
#[cfg(target_arch = "wasm32")]
mod raw {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    pub(super) fn name_of(href: &str) -> String {
        href.to_string()
    }
    pub(super) struct Store;
    impl Store {
        pub(super) fn open(_: &Path, _: u64) -> Store {
            Store
        }
        pub(super) fn drop_one(&mut self, _: &str) {}
        pub(super) fn keep(&mut self, _: &str, _: &str, _: i64, _: &dyn Fn() -> HashMap<String, i64>) -> bool {
            false
        }
    }
    pub(super) fn sweep(_: &Path, _: u64, _: &HashSet<String>) {}
}
