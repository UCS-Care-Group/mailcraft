// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of keeping an account's calendars and contacts in step with a CalDAV and
//! CardDAV server (#209): what to ask (`sync.davRequest`), what came back (`sync.applyDav`), what
//! was changed here and goes up (`sync.davChanges`) and what the server made of it
//! (`sync.applyDavUploaded`). As for every server the engine does no I/O with it: a host with
//! sockets (`mailcraft_net::dav`) does the part in between.
//!
//! **Calendars down (D5).** `sync.davRequest` says per collection what the host asks: the sync
//! token or change tag kept at the last complete read, the stretch for a server that can't say
//! what changed, and, for a read that has no token to go on, what is here already. `sync.applyDav`
//! takes the answer in: the calendars of the account's homes (each a calendar here, read-only until
//! changes are sent up, D6) and each collection's changed resources. A resource is one `VCALENDAR`;
//! `mailcraft_ical` reads it into events (a single event, or a series with the days it skips and
//! its changed occurrences as events of their own, as for a subscribed calendar), each kept under
//! the resource's address and ETag (`Origin::id`, `Origin::change_key`; a changed occurrence under
//! the address and `#` and its own mark). The document itself is kept as it came
//! ([`crate::dav_raw`], ADR 0026).
//!
//! Everything the answer says is hostile: an address must be on an origin of the account's homes
//! and inside the collection it was read from; counts, sizes and texts are capped; a deletion is
//! believed only from a complete listing or the server's own "gone"; a token or change tag moves
//! only with a read that went to its end (or, for changes, to the end of a page that was taken in).
//! Where the server's copy and a change here meet, the server's wins (v1): nothing here is
//! changed yet, so nothing waits.
//!
//! The commands are the host's, never an agent's, and ship switched off (`MAILCRAFT_DAV=1`).

use std::collections::{HashMap, HashSet};

use mailcraft_model::dav_down::{MAX_ACCOUNT_RESOURCES, MAX_CHANGES, MAX_READ, MAX_RESOURCES};
use mailcraft_model::remote::MAX_KEY;
use mailcraft_model::{
    Calendar, DavAsk, DavAskCollection, DavCollectionState, DavDownload, DavListing, DavProblem, DavReadState, DavServer, Event, Id, Origin, Rights,
    origin_of,
};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

/// What the commands that send changes up say while they have nothing to do yet.
const NOT_YET: &str = "Changes to calendars and contacts on a server are not sent yet in this version.";

/// A look at the account's calendars (which it has) is repeated after this many seconds.
const RELIST_AFTER: i64 = 3_600;
/// The stretch asked for by a server that can't say what changed: a month back, a year ahead.
const BACK: i64 = 31 * 86_400;
const AHEAD: i64 = 366 * 86_400;
/// Most events one resource is taken to hold (a series with its changed occurrences).
const MAX_PER_RESOURCE: usize = 1_000;
/// Most calendars a mailbox has (as for those made elsewhere).
const MAX_CALENDARS: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.davRequest", "Send/Receive Calendars and Contacts Server", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "skip"?: [number] (accounts to pass over), "relist"?: bool (look for calendars that came or went now)} — answers what a host should read from an account's calendar server: its homes, the calendars known with their sync token, change tag and (for one without a token) what is here, and the stretch to ask for. Answers {"ask": DavAsk} or {"ask": null} when no account has one."#)
            .when(available),
        CommandSpec::new("sync.applyDav", "Apply Calendars and Contacts Server", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": DavDownload} — takes in what a host read from a calendar server: the calendars its homes list, and each collection's changed or gone resources. Answers {"added", "updated", "removed", "calendars", "skipped", "problem", "wait"}."#)
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
    let hrefs: HashSet<&str> = collections.iter().map(String::as_str).collect();
    let followed: Vec<Id> =
        s.mailbox.calendars.iter().filter(|c| c.origin.account == account && hrefs.contains(c.origin.id.as_str())).map(|c| c.id).collect();
    let events: Vec<Id> = s
        .mailbox
        .events
        .values()
        .filter(|e| followed.contains(&e.calendar) && e.origin.account == account && !e.origin.id.is_empty())
        .map(|e| e.id)
        .collect();
    for id in &events {
        let _ = s.mailbox.remove_event(*id);
    }
    s.mailbox.event_changes.retain(|c| !(c.account == account && events.contains(&c.event)));
    // A calendar with something of the owner's left in it becomes the mailbox's own. And one
    // calendar of the mailbox's own always stays, for new events to go in (as `calendar.remove`
    // keeps one): a subscribed copy of a calendar on the web is not one.
    let mut stays: Vec<Id> = followed.iter().copied().filter(|id| s.mailbox.events.values().any(|e| e.calendar == *id)).collect();
    let own_elsewhere =
        s.mailbox.calendars.iter().any(|c| !followed.contains(&c.id) && super::remote::keeper(&s.mailbox, &c.origin).is_none() && !c.is_subscribed());
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
    s.mailbox.calendars.retain(|c| !followed.contains(&c.id) || stays.contains(&c.id));
    s.dav_listed.remove(&account);
    s.dav_asked.remove(&account);
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
    let skip: Vec<u64> =
        v.get("skip").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_u64).take(1_000).collect()).unwrap_or_default();
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
    let calendars: Vec<&DavCollectionState> = server.collections.iter().filter(|c| !c.contacts).collect();
    let list_homes = relist || calendars.is_empty() || now.saturating_sub(listed) >= RELIST_AFTER || listed > now;
    let mut collections: Vec<DavAskCollection> = Vec::new();
    for state in calendars {
        let mut ask =
            DavAskCollection { href: state.href.clone(), sync_token: state.sync_token.clone(), ctag: state.ctag.clone(), known: Vec::new() };
        if state.sync_token.is_empty()
            && let Some(calendar) = s.mailbox.calendars.iter().find(|c| c.origin.account == account.id && c.origin.id == state.href)
        {
            let mut seen: HashSet<&str> = HashSet::new();
            for event in s.mailbox.events.values().filter(|e| e.calendar == calendar.id && e.origin.account == account.id) {
                let href = resource_of(&event.origin.id);
                if !href.is_empty() && seen.insert(href) && ask.known.len() < MAX_RESOURCES {
                    ask.known.push(mailcraft_model::DavKnown { href: href.to_string(), etag: event.origin.change_key.clone() });
                }
            }
        }
        collections.push(ask);
    }
    let ask = DavAsk {
        account: account.id,
        email: account.email.trim().to_string(),
        username: if server.username.trim().is_empty() { account.email.trim().to_string() } else { server.username.trim().to_string() },
        trusted: server.trusted.clone(),
        homes: server.calendar_homes.clone(),
        list_homes,
        from: now.saturating_sub(BACK),
        to: now.saturating_add(AHEAD),
        collections,
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

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let download = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let download: DavDownload = serde_json::from_value(download)
        .map_err(|e| bad(format!("`result`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(200).collect::<String>())))?;
    let took = take_in(s, &download)?;
    Ok(json!({
        "added": took.added, "updated": took.updated, "removed": took.removed, "calendars": took.calendars,
        "skipped": took.skipped.saturating_add(download.skipped as usize),
        "problem": serde_json::to_value(download.problem).unwrap_or(Value::Null),
        "wait": download.wait.min(600),
        "signedIn": download.signed_in,
    }))
}

fn take_in(s: &mut Session, download: &DavDownload) -> Result<Took, CmdError> {
    let account = super::remote::owner(&s.mailbox, download.account, &download.email)?.id;
    let server = s.mailbox.account(account).and_then(|a| a.dav.clone()).ok_or_else(|| fail("this account has no calendar server"))?;
    let mut took = Took::default();
    let offset_at = super::remote::offset_of(s);
    let now = s.now();
    let file = s.path.clone();
    let mut changed = false;
    let homes = server.home_origins();
    let ok_collection = |href: &str| DavServer::check_home(href).is_ok() && origin_of(href).is_some_and(|o| homes.contains(&o));

    if let Some(listing) = &download.listing {
        changed |= take_listing(s, account, listing, &ok_collection, &mut took, file.as_deref());
        if listing.complete {
            s.dav_listed.insert(account, now);
        }
    }
    let mut reads = 0usize;
    for read in &download.collections {
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
        changed |= take_read(s, account, calendar, &state.href, read, &offset_at, file.as_deref(), &mut took);
        // The state moves with what was taken in, in the same step.
        let token = if read.sync_token.chars().count() <= DavServer::MAX_TOKEN { read.sync_token.clone() } else { String::new() };
        let tag = if read.ctag.chars().count() <= DavServer::MAX_TOKEN { read.ctag.clone() } else { String::new() };
        if let Some(d) = s.mailbox.account_mut(account)?.dav.as_mut()
            && let Some(c) = d.collections.iter_mut().find(|c| c.href == read.href)
        {
            let new_ctag = if read.complete { tag } else { c.ctag.clone() };
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
        log::info!("calendar server (account {account}): {:?}", download.problem);
    }
    Ok(took)
}

/// The calendars the homes list: each is one here, read-only. Whether anything changed.
fn take_listing(
    s: &mut Session,
    account: Id,
    listing: &DavListing,
    ok_collection: &dyn Fn(&str) -> bool,
    took: &mut Took,
    file: Option<&std::path::Path>,
) -> bool {
    let mut changed = false;
    let mut listed: Vec<&mailcraft_model::DavCalendar> = Vec::new();
    for calendar in listing.calendars.iter().take(DavServer::MAX_COLLECTIONS) {
        let events = calendar.components.is_empty() || calendar.components.iter().any(|c| c == "VEVENT");
        if ok_collection(&calendar.href) && events && listed.iter().all(|l| l.href != calendar.href) {
            listed.push(calendar);
        } else {
            took.skipped += 1;
        }
    }
    for item in &listed {
        let name: String = item.name.chars().filter(|c| !c.is_control()).take(mailcraft_model::MAX_NAME).collect();
        let name = if name.trim().is_empty() { "Calendar".to_string() } else { name.trim().to_string() };
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
            None => continue,
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
    // What the server no longer lists goes, with its events and their documents, but only from a
    // listing that went to its end.
    if listing.complete && !listing.capped {
        let keep: HashSet<&str> = listed.iter().map(|l| l.href.as_str()).collect();
        let gone: Vec<String> = s
            .mailbox
            .account(account)
            .and_then(|a| a.dav.as_ref())
            .map(|d| d.collections.iter().filter(|c| !c.contacts && !keep.contains(c.href.as_str())).map(|c| c.href.clone()).collect())
            .unwrap_or_default();
        for href in gone {
            let ids: Vec<Id> = s.mailbox.calendars.iter().filter(|c| c.origin.account == account && c.origin.id == href).map(|c| c.id).collect();
            for id in &ids {
                let events: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == *id).map(|e| e.id).collect();
                for event in events {
                    let _ = s.mailbox.remove_event(event);
                    took.removed += 1;
                }
            }
            s.mailbox.calendars.retain(|c| !ids.contains(&c.id));
            if let Ok(a) = s.mailbox.account_mut(account)
                && let Some(d) = a.dav.as_mut()
            {
                d.collections.retain(|c| c.href != href);
            }
            took.calendars += 1;
            changed = true;
        }
        sweep(s, account, file);
    }
    changed
}

/// Takes in what one collection's read brought. Whether anything changed.
#[allow(clippy::too_many_arguments)]
fn take_read(
    s: &mut Session,
    account: Id,
    calendar: Id,
    collection: &str,
    read: &mailcraft_model::DavRead,
    offset_at: &dyn Fn(i64) -> i32,
    file: Option<&std::path::Path>,
    took: &mut Took,
) -> bool {
    let owner = s.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
    let pending: HashSet<Id> = s.mailbox.event_changes.iter().map(|c| c.event).collect();
    // What is here of this collection, by key, and the keys of each resource (so that a change
    // looks at its own resource's and not at all).
    let mut here: HashMap<String, Id> = HashMap::new();
    let mut of_resource: HashMap<String, Vec<String>> = HashMap::new();
    for event in s.mailbox.events.values().filter(|e| e.calendar == calendar && e.origin.account == account) {
        here.insert(event.origin.id.clone(), event.id);
        of_resource.entry(resource_of(&event.origin.id).to_string()).or_default().push(event.origin.id.clone());
    }
    let mut total = s.mailbox.events.values().filter(|e| e.origin.account == account && !e.origin.id.is_empty()).count();
    // The documents' folder is listed once for the whole read.
    let mut store = file.map(|file| raw::Store::open(file, account));
    let before = (took.added, took.updated, took.removed);
    let mut seen: HashSet<&str> = HashSet::new();
    for change in read.changes.iter().take(MAX_CHANGES) {
        if !within(collection, &change.href) {
            took.skipped += 1;
            continue;
        }
        seen.insert(change.href.as_str());
        let mut fresh: Vec<(String, Event)> = Vec::new();
        if !change.gone {
            let Some(raw) = change.raw.as_deref().filter(|r| r.len() <= MAX_READ && change.etag.len() <= MAX_KEY) else {
                took.skipped += 1;
                continue;
            };
            fresh = keyed(&change.href, mailcraft_ical::parse(raw, offset_at).events);
        }
        let old: Vec<(String, Id)> =
            of_resource.get(change.href.as_str()).into_iter().flatten().filter_map(|k| here.get(k).map(|i| (k.clone(), *i))).collect();
        let new_keys: HashSet<String> = fresh.iter().map(|(k, _)| k.clone()).collect();
        for (key, id) in old.iter().filter(|(k, _)| !new_keys.contains(k)) {
            if !pending.contains(id) && s.mailbox.remove_event(*id).is_ok() {
                here.remove(key);
                if let Some(keys) = of_resource.get_mut(change.href.as_str()) {
                    keys.retain(|k| k != key);
                }
                took.removed += 1;
            }
        }
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
            match here.get(&key).copied() {
                Some(id) if pending.contains(&id) => {}
                Some(id) => {
                    event.id = id;
                    if s.mailbox.event(id) != Some(&event)
                        && let Ok(slot) = s.mailbox.event_mut(id)
                    {
                        *slot = event;
                        took.updated += 1;
                    }
                }
                None if total < MAX_ACCOUNT_RESOURCES => {
                    if let Ok(id) = s.mailbox.add_event(event) {
                        of_resource.entry(change.href.clone()).or_default().push(key.clone());
                        here.insert(key, id);
                        total += 1;
                        took.added += 1;
                    }
                }
                None => took.skipped += 1,
            }
        }
        if let Some(store) = store.as_mut() {
            store.drop_one(&change.href);
            if let Some(doc) = change.raw.as_deref().filter(|_| !change.gone) {
                // (What is asked of the events when there is no room is how they are now.)
                store.keep(&change.href, doc, &|| ends_of(s, account, offset_at));
            }
        }
    }
    // What a full read that went to its end didn't find is gone (of the stretch it was of): a
    // resource goes whole, its series, its changed occurrences and all, when it is none of what
    // the server named and, for a read of a stretch, something of it is in that stretch (an event
    // that starts in it, a series with an occurrence in it).
    if read.full && read.complete && !read.capped {
        let present: HashSet<&str> = read.present.iter().map(String::as_str).chain(seen.iter().copied()).collect();
        // The server compares exact instants (RFC 4791 §9.9), so a day-wise look at where a series
        // falls would take an occurrence that ended just before the stretch for one inside it: a
        // series is believed gone only when one of its occurrences lies, whole, a day or more
        // inside both edges of the stretch (and only over the first five years of it: the stretch
        // is a command's parameter too).
        let window = read.window.map(|(from, to)| {
            let (a, b) = (from.saturating_add(86_400), to.saturating_sub(86_400));
            let reach = b.min(a.saturating_add(5 * 366 * 86_400));
            let inner =
                (a < b).then(|| (mailcraft_time::DateTime::from_timestamp(a, 0).date, mailcraft_time::DateTime::from_timestamp(reach, 0).date, a, b));
            (from, to, inner)
        });
        let mut gone: Vec<String> = Vec::new();
        for (resource, keys) in &of_resource {
            if present.contains(resource.as_str()) {
                continue;
            }
            let ids: Vec<Id> = keys.iter().filter_map(|k| here.get(k).copied()).collect();
            if ids.is_empty() || ids.iter().any(|id| pending.contains(id)) {
                continue;
            }
            let inside = |id: &Id| match (window, s.mailbox.event(*id)) {
                (None, Some(_)) => true,
                (Some((from, to, inner)), Some(e)) => {
                    if e.recurrence.is_some() {
                        inner.is_some_and(|(from_day, to_day, a, b)| {
                            e.occurrences_in(from_day, to_day, offset_at).iter().any(|o| {
                                // (Wall clock to instant: a day's margin covers the offset's own.)
                                let off = offset_at(o.start.to_timestamp(0));
                                o.start.to_timestamp(off) >= a && o.end.to_timestamp(off) <= b
                            })
                        })
                    } else {
                        (from..to).contains(&e.instants(offset_at).0)
                    }
                }
                _ => false,
            };
            if ids.iter().any(inside) {
                gone.push(resource.clone());
            }
        }
        for resource in gone {
            for key in of_resource.get(&resource).into_iter().flatten() {
                if let Some(id) = here.get(key)
                    && s.mailbox.remove_event(*id).is_ok()
                {
                    took.removed += 1;
                }
            }
            if let Some(store) = store.as_mut() {
                store.drop_one(&resource);
            }
        }
    }
    (took.added, took.updated, took.removed) != before
}

/// The end of each of the account's events that has a kept document, by the document's name, as
/// the events are now (a series that never ends is as late as can be).
fn ends_of(s: &Session, account: Id, offset_at: &dyn Fn(i64) -> i32) -> HashMap<String, i64> {
    let mut ends: HashMap<String, i64> = HashMap::new();
    for e in s.mailbox.events.values().filter(|e| e.origin.account == account && !e.origin.id.is_empty()) {
        let end = e.instants(offset_at).1;
        let slot = ends.entry(raw::name_of(resource_of(&e.origin.id))).or_insert(i64::MIN);
        *slot = (*slot).max(if e.recurrence.is_some() { i64::MAX } else { end });
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
    for (n, event) in main.into_iter().enumerate() {
        let key = if n == 0 { href.to_string() } else { format!("{href}#{}", if event.uid.is_empty() { n.to_string() } else { event.uid.clone() }) };
        if key.len() <= MAX_KEY && out.iter().all(|(k, _)| *k != key) {
            out.push((key, event));
        }
    }
    out
}

/// A server's event cut to what the mailbox keeps.
fn tidy(event: &mut Event) {
    let line = |text: &str, max: usize| -> String { text.chars().filter(|c| !c.is_control() || *c == '\t').take(max).collect() };
    event.title = line(&event.title, 500);
    event.location = line(&event.location, 500);
    event.description = event.description.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).take(100_000).collect();
    event.attendees.truncate(500);
    event.categories.truncate(50);
    event.exceptions.truncate(5_000);
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
        pub(super) fn keep(&mut self, _: &str, _: &str, _: &dyn Fn() -> HashMap<String, i64>) -> bool {
            false
        }
    }
    pub(super) fn sweep(_: &Path, _: u64, _: &HashSet<String>) {}
}
