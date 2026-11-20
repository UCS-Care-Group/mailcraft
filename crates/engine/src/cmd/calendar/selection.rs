// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The selected set of calendar items: typed selection, Select All, pasting it (a copy, or a
//! move), and deleting it as one step.
//!
//! A typed item names one whole event, or one occurrence of a series by the day the series
//! itself names it by ([`mailcraft_model::Occurrence::day`]) — never by the date it happens to be
//! shown on here. The distinction matters for a series that repeats in another zone: across the
//! weeks the two zones' clocks disagree, an occurrence's own day and its displayed date differ by
//! a day, and taking the displayed date would address its neighbour.
//!
//! Nothing here reads a target from the current selection when the caller named one: a payload
//! that is malformed is refused, never quietly turned into "what is selected". A batch that can't
//! be done whole is refused whole, before anything of it is changed.

use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

use mailcraft_model::zoned::instant_at;
use mailcraft_model::{Event, Id, Response, Rights};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use crate::cmd::{bad, fail, remote};
use crate::{CmdError, CmdResult, CommandSpec, MAX_SELECTED_EVENTS, SelectedEvent, Session, p};

#[cfg(test)]
mod selection_tests;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.selectAll", "Select All", "Calendar", select_all).reads().key("Mod+A").params(
            r#"{} — every distinct appointment shown in the range in view and in the calendars shown, up to 200; answers {"selected": n, "capped": bool} (capped says the range holds more than 200)"#,
        ),
        CommandSpec::new("calendar.paste", "Paste", "Calendar", paste).params(
            r#"{"items": [{"id": number, "day"?: "YYYY-MM-DD"}, …], "start": "YYYY-MM-DDTHH:MM", "cut"?: bool, "calendar"?: number, "send"?: bool} — pastes the named items at `start`: the earliest timed one there, the others by their own day and time-of-day offsets from it, whole-day ones as whole days. "cut": true moves them (a series' occurrence as that occurrence changed, never a cancellation); "calendar" is where they go (default: each one's own; a series' occurrence stays in its series' calendar). Answers {"ids": [...], "cut": bool}, the ids in the order of the items. Items are required, 1 to 200, and the same item twice is refused, and so is a copy (not a cut) of an item with files attached, as Copy Event refuses one (only its server keeps them). "send": true is required when cutting makes the account's server write to the people invited"#,
        ),
    ]
}

/// How strictly a list of items is read: [`Allow::Occurrences`] needs a day for anything that
/// repeats; [`Allow::Series`] takes a recurring item without one to mean all of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Allow {
    Occurrences,
    Series,
}

/// One named item as it actually is: the event, and the occurrence it names when it repeats.
struct Resolved {
    /// The item as it is stored in a selection: a whole event, or a day of a series.
    item: SelectedEvent,
    /// The event as the mailbox holds it, shared rather than copied: nothing here changes it.
    event: Arc<Event>,
    /// When that occurrence starts and ends here; the event's own times for one that doesn't
    /// repeat.
    start: DateTime,
    end: DateTime,
}

/// How a payload names what it acts on: the typed `items` list, the older single `id` (with the
/// day it is shown on here), nothing at all — the selection stands — or, as `calendar.select`
/// alone takes it, a null `id`, which selects nothing.
enum Named {
    Items(Vec<SelectedEvent>),
    Legacy {
        id: Id,
        day: Option<Date>,
    },
    /// `"id": null`: a target form that names nothing.
    Null,
    /// No target form: whatever is selected.
    Unnamed,
}

/// The target a payload names, read strictly: one form only, never `ids`, and never an id or a
/// day beside a typed list. Nothing here falls back to the selection when a form is malformed —
/// a payload that names something the command can't read is refused for what it is.
fn named(v: &Value) -> Result<Named, CmdError> {
    if !v.is_object() && !v.is_null() {
        return Err(bad("the parameters are an object"));
    }
    if v.get("ids").is_some() {
        return Err(bad("calendar items are named by `items` (a list) or by `id` (one), not by `ids`"));
    }
    let has_items = v.get("items").is_some();
    let id = v.get("id");
    // (`day`, and its older name `occurrence`, are the day of one event: beside a typed list they
    // say nothing, and are refused rather than quietly dropped.)
    let day = v.get("day").is_some() || v.get("occurrence").is_some();
    if has_items && (id.is_some() || day) {
        return Err(bad("name the calendar items by `items`, or by `id` and `day`, not both"));
    }
    if has_items {
        return Ok(Named::Items(parse_items(v)?));
    }
    match id {
        None if day => Err(bad("`day` is the day of the event `id` names, and no `id` was given")),
        None => Ok(Named::Unnamed),
        Some(Value::Null) if day => Err(bad("`id` is null, which names nothing, so `day` is no day of anything")),
        Some(Value::Null) => Ok(Named::Null),
        Some(_) => {
            // (An id that isn't a whole number above 0 — `null`, a fraction, text — is refused
            // here, never read as the selection.)
            let id = p::id_strict(v, "id")?.ok_or_else(|| bad("`id` is null, which names nothing"))?;
            let day = p::date(v, "day")?.or(p::date(v, "occurrence")?);
            Ok(Named::Legacy { id, day })
        }
    }
}

/// The selection a batch command works on: `items` when they are given (strictly, and never
/// falling back to the selection), else the legacy `id`/`day`, else what is selected. The typed
/// set is read item by item; a member that no longer exists or no longer happens is refused, not
/// dropped.
pub fn selected_items(s: &Session, params: &Value) -> Result<Vec<SelectedEvent>, CmdError> {
    match named(params)? {
        Named::Items(items) => read(s, &items, Allow::Occurrences),
        Named::Legacy { id, day } => Ok(vec![legacy_item(s, id, day)?]),
        // `null` names nothing, and is never the selection (#553).
        Named::Null => Err(bad("`id` is null, which names nothing")),
        Named::Unnamed => {
            if !s.sel.events.is_empty() {
                return read(s, &s.sel.events, Allow::Occurrences);
            }
            let id = s.sel.event.ok_or_else(|| fail("select an appointment first"))?;
            Ok(vec![legacy_item(s, id, s.sel.event_day)?])
        }
    }
}

/// The typed items as they are stored, in order, each resolved so that a stale member is refused.
fn read(s: &Session, items: &[SelectedEvent], allow: Allow) -> Result<Vec<SelectedEvent>, CmdError> {
    Ok(resolve(s, items, allow)?.into_iter().map(|r| r.item).collect())
}

/// The legacy single target as the item it names: one whole event (a series without a day is all
/// of it, as it is for `calendar.delete` without a scope), or one occurrence when a day is given.
fn legacy_item(s: &Session, id: Id, day: Option<Date>) -> Result<SelectedEvent, CmdError> {
    let event = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
    match (event.recurrence.is_some(), day) {
        (false, _) | (true, None) => Ok(SelectedEvent { id, day: None }),
        (true, Some(day)) => {
            let here = remote::offset_of(s);
            let day = event
                .series_day_on(day, &here)
                .ok_or_else(|| failed!("the series \"{title}\" doesn't happen on {date}", title = event.title, date = day.iso()))?;
            occurrence_times(event, day, &here).ok_or_else(|| failed!("the series doesn't happen on {date}", date = day.iso()))?;
            Ok(SelectedEvent { id, day: Some(day) })
        }
    }
}

/// A legacy focus (an event, and the date it is shown on here) as the typed item it stands for.
fn focus_item(s: &Session, id: Id, day: Option<Date>) -> Result<SelectedEvent, CmdError> {
    let event = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
    if event.recurrence.is_none() {
        return Ok(SelectedEvent { id, day: None });
    }
    let here = remote::offset_of(s);
    // (`day` is the date it is shown on here; the typed day is the series' own — the one mapping
    // that may go through the displayed date is this legacy one.)
    let day = match day {
        Some(day) => event
            .series_day_on(day, &here)
            .ok_or_else(|| failed!("the series \"{title}\" doesn't happen on {date}", title = event.title, date = day.iso()))?,
        None => event
            .series_days(event.start.date, event.start.date.add_days(366), &here)
            .into_iter()
            .find(|day| occurrence_times(event, *day, &here).is_some())
            .ok_or_else(|| fail("that series has no day to select"))?,
    };
    occurrence_times(event, day, &here).ok_or_else(|| failed!("the series doesn't happen on {date}", date = day.iso()))?;
    Ok(SelectedEvent { id, day: Some(day) })
}

/// `items` read strictly: 1 to [`MAX_SELECTED_EVENTS`] objects of `id` and an optional strict ISO
/// `day`; nothing else, never `null` under either, and the same item never twice.
fn parse_items(v: &Value) -> Result<Vec<SelectedEvent>, CmdError> {
    let wrong = || bad(format!("`items` is a list of 1 to {MAX_SELECTED_EVENTS} calendar items, each {{\"id\": number, \"day\"?: \"YYYY-MM-DD\"}}"));
    let list = v.get("items").and_then(Value::as_array).ok_or_else(wrong)?;
    if list.is_empty() || list.len() > MAX_SELECTED_EVENTS {
        return Err(wrong());
    }
    let mut out: Vec<SelectedEvent> = Vec::with_capacity(list.len());
    for raw in list {
        let fields = raw.as_object().ok_or_else(wrong)?;
        if fields.keys().any(|k| k.as_str() != "id" && k.as_str() != "day") {
            return Err(bad("a calendar item is {\"id\": number, \"day\"?: \"YYYY-MM-DD\"}, and nothing else"));
        }
        let id = p::id(raw, "id").ok_or_else(|| bad("`items[*].id` is an id: a whole number above 0"))?;
        let day = match raw.get("day") {
            None => None,
            Some(Value::String(text)) if text.trim().is_empty() => return Err(bad("`items[*].day` is a date like 2026-10-11")),
            Some(Value::String(_)) => p::date(raw, "day")?,
            Some(_) => return Err(bad("`items[*].day` is a date like 2026-10-11")),
        };
        if out.iter().any(|item| item.id == id && item.day == day) {
            return Err(fail("the same calendar item is named twice; nothing was changed"));
        }
        out.push(SelectedEvent { id, day });
    }
    Ok(out)
}

/// The items as they are: each held event, and for one that repeats the occurrence its own day
/// names. An item the mailbox doesn't have, one that no longer happens, and the same one twice are
/// refused.
fn resolve(s: &Session, items: &[SelectedEvent], allow: Allow) -> Result<Vec<Resolved>, CmdError> {
    if items.len() > MAX_SELECTED_EVENTS {
        return Err(fail(format!("no more than {MAX_SELECTED_EVENTS} calendar items can be named at once")));
    }
    let here = remote::offset_of(s);
    let mut out: Vec<Resolved> = Vec::with_capacity(items.len());
    for item in items {
        let event = s.mailbox.events.get(&item.id).cloned().ok_or_else(|| failed!("no event with id {id}", id = item.id))?;
        if event.cancelled {
            return Err(failed!("\"{title}\" is cancelled, and can't be selected", title = event.title));
        }
        let (item, start, end) = match (event.recurrence.is_some(), item.day) {
            (false, _) => (SelectedEvent { id: event.id, day: None }, event.start, event.end),
            (true, Some(day)) => {
                let (start, end) = occurrence_times(&event, day, &here)
                    .ok_or_else(|| failed!("the series \"{title}\" doesn't happen on {date}", title = event.title, date = day.iso()))?;
                (SelectedEvent { id: event.id, day: Some(day) }, start, end)
            }
            (true, None) if allow == Allow::Series => (SelectedEvent { id: event.id, day: None }, event.start, event.end),
            (true, None) => {
                return Err(bad("a day of the series is named for an event that repeats: `items[*].day`, as YYYY-MM-DD"));
            }
        };
        if out.iter().any(|other| other.item == item) {
            return Err(fail("the same calendar item is named twice; nothing was changed"));
        }
        out.push(Resolved { item, event, start, end });
    }
    Ok(out)
}

/// When a series' own day `day` happens, and how long it lasts here: `None` when the rule skips it
/// or the series has ended. The day is the series' key, exactly as an occurrence carries it — it
/// is never mapped through the date the occurrence is shown on, which can be its neighbour's.
fn occurrence_times(event: &Event, day: Date, here: &dyn Fn(Timestamp) -> i32) -> Option<(DateTime, DateTime)> {
    if event.exceptions.contains(&day) || !on_rule(event, day, here) {
        return None;
    }
    Some(event.occurrence_times(day, here))
}

/// Whether the series' rule gives its own day `day`, cancelled or not.
fn on_rule(event: &Event, day: Date, here: &dyn Fn(Timestamp) -> i32) -> bool {
    event.series_days(day, day, here).contains(&day)
}

/// Which of `days` (the series' own days, as the typed selection names them) the series `event`
/// no longer has: cancelled, or no day of its rule. The same test as [`occurrence_times`], with the
/// cancelled days read once for all of them rather than once per day.
pub(crate) fn days_gone(event: &Event, days: &[Date], here: &dyn Fn(Timestamp) -> i32) -> HashSet<Date> {
    let mut asked = days.to_vec();
    asked.sort_unstable();
    asked.dedup();
    let mut gone: HashSet<Date> = event.exceptions.iter().copied().filter(|day| asked.binary_search(day).is_ok()).collect();
    for day in asked {
        if !gone.contains(&day) && !on_rule(event, day, here) {
            gone.insert(day);
        }
    }
    gone
}

/// Point the selection at the typed items, the last of them having the focus: its own day, and the
/// date it is shown on here for the legacy focus. Nothing selected clears both.
fn set_selection(s: &mut Session, items: &[SelectedEvent]) {
    let here = remote::offset_of(s);
    let focus = items.last().and_then(|item| {
        let event = s.mailbox.event(item.id)?;
        let date = item.day.map_or(event.start.date, |day| event.occurrence_times(day, &here).0.date);
        Some((item.id, date))
    });
    s.sel.events = items.to_vec();
    match focus {
        Some((id, date)) => {
            s.sel.event = Some(id);
            s.sel.event_day = Some(date);
        }
        None => {
            s.sel.event = None;
            s.sel.event_day = None;
        }
    }
}

// ---- select ----

/// `calendar.select`: typed `items` (replacing the set, or toggling with `extend`), or the legacy
/// `id`/`day` (`id: null` selects nothing). A legacy selection replaces the typed set.
pub(super) fn select(s: &mut Session, v: &Value) -> CmdResult {
    let extend = p::opt_bool(v, "extend")?.unwrap_or(false);
    // `id: null` selects nothing, as it always did, whatever older `day` is beside it (#553);
    // beside a typed list it is refused by `named`, as any other id is.
    if v.get("id").is_some_and(Value::is_null) && v.get("items").is_none() && v.get("ids").is_none() {
        s.sel.event = None;
        s.sel.event_day = None;
        s.sel.events.clear();
        return Ok(json!({}));
    }
    match named(v)? {
        Named::Items(items) => {
            let resolved = resolve(s, &items, Allow::Occurrences)?;
            let mut chosen: Vec<SelectedEvent> = if extend { current_set(s)? } else { Vec::new() };
            for item in resolved {
                if let Some(at) = chosen.iter().position(|other| *other == item.item) {
                    chosen.remove(at);
                } else {
                    chosen.push(item.item);
                }
            }
            // (The cap is the set's once every toggle is in: one taken away makes room for one
            // added, whichever of the two is named first. The set held meanwhile is at most twice
            // the cap, both lists being capped.)
            if chosen.len() > MAX_SELECTED_EVENTS {
                return Err(fail(format!("no more than {MAX_SELECTED_EVENTS} calendar items can be selected at once")));
            }
            set_selection(s, &chosen);
        }
        Named::Legacy { id, day } => {
            let e = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
            s.sel.event_day = day.or(Some(e.start.date));
            s.sel.event = Some(id);
            // An older single-event selection may not leave an unrelated set standing.
            s.sel.events.clear();
        }
        // (`id: null`, or no target at all: nothing is selected.)
        Named::Null | Named::Unnamed => {
            s.sel.event = None;
            s.sel.event_day = None;
            s.sel.events.clear();
        }
    }
    Ok(json!({}))
}

/// What `extend` starts from: the typed set, or the legacy focus as the one item it stands for.
fn current_set(s: &Session) -> Result<Vec<SelectedEvent>, CmdError> {
    if !s.sel.events.is_empty() {
        return read(s, &s.sel.events, Allow::Occurrences);
    }
    match s.sel.event {
        Some(id) => Ok(vec![focus_item(s, id, s.sel.event_day)?]),
        None => Ok(Vec::new()),
    }
}

// ---- select all ----

/// `calendar.selectAll`: every distinct item the calendar shows in the range in view, in the
/// calendars that are shown. A calendar that only says when somebody is busy is left out (a
/// block in it is not an appointment), so is a cancelled appointment the calendar never shows,
/// and so is nothing beyond the cap, which the answer says.
///
/// The range can hold far more occurrences than a selection may, and a calendar far more events
/// than the range shows, so nothing is listed whole here: each event is read by itself and only
/// the earliest [`MAX_SELECTED_EVENTS`] distinct items are kept as the scan goes.
fn select_all(s: &mut Session, _: &Value) -> CmdResult {
    let (from, to) = crate::cmd::view::calendar_range(s);
    let here = remote::offset_of(s);
    let shown: HashSet<Id> = s.mailbox.calendars.iter().filter(|c| c.visible && c.rights != Rights::FreeBusy).map(|c| c.id).collect();
    // The earliest items, by the order the calendar lists them in: the heap's largest is the
    // last one kept, so one more than a selection may take is the most it ever holds.
    let mut earliest: BinaryHeap<(DateTime, DateTime, Id, Option<Date>)> = BinaryHeap::with_capacity(MAX_SELECTED_EVENTS.saturating_add(1));
    let mut total = 0usize;
    for event in s.mailbox.events.values() {
        if event.cancelled || !shown.contains(&event.calendar) {
            continue;
        }
        // (A series' occurrence is its own day; an event that doesn't repeat is one item.)
        let repeats = event.recurrence.is_some();
        for occurrence in event.occurrences_in(from, to, &here) {
            total = total.saturating_add(1);
            let item = (occurrence.start, occurrence.end, occurrence.event, repeats.then_some(occurrence.day));
            // (Later than the last of the earliest kept, once a full set is held: it isn't one of
            // them, and nothing of it is kept.)
            if let Some(last) = earliest.peek()
                && earliest.len() >= MAX_SELECTED_EVENTS
                && item >= *last
            {
                continue;
            }
            earliest.push(item);
            if earliest.len() > MAX_SELECTED_EVENTS {
                earliest.pop();
            }
        }
    }
    let mut items: Vec<SelectedEvent> = earliest.into_sorted_vec().into_iter().map(|(_, _, id, day)| SelectedEvent { id, day }).collect();
    items.dedup();
    set_selection(s, &items);
    Ok(json!({"selected": items.len(), "capped": total > items.len()}))
}

// ---- paste ----

/// The people a paste as a move would make the account's servers write to, split by what they are
/// told. One kind for a batch, so that one question is accurate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MailSummary {
    invitations: usize,
    updates: usize,
}

impl MailSummary {
    fn people(self) -> usize {
        self.invitations.saturating_add(self.updates)
    }

    /// The one kind of mail the batch makes, when it makes only one.
    fn kind(self) -> Option<super::Mail> {
        // (A batch that would both invite and update is refused, and isn't described as one.)
        if self.invitations > 0 && self.updates > 0 {
            None
        } else if self.invitations > 0 {
            Some(super::Mail::Invitation(self.invitations))
        } else if self.updates > 0 {
            Some(super::Mail::Update(self.updates))
        } else {
            None
        }
    }
}

/// The people a cut's plan ([`cut_plan`]) writes to, by what they are told: each person once.
fn mail_summary(plan: &[Notice]) -> MailSummary {
    let of = |kind: NoticeKind| reached(plan.iter().filter(|notice| notice.kind == kind));
    MailSummary { invitations: of(NoticeKind::Invitation), updates: of(NoticeKind::Update) }
}

/// `calendar.paste`: copies `items` to `start`, or with `cut` moves them there. A copy is a new
/// appointment of its own (no rule, no people, no UID); a move keeps each item, and an occurrence
/// of a series is changed on its own day, never cancelled and made again.
fn paste(s: &mut Session, v: &Value) -> CmdResult {
    let Checked { cut, agreed, target, resolved, destinations, placements, stays } = checked(s, v)?;
    let moving = || resolved.iter().zip(&stays).filter(|(_, stays)| !**stays).map(|(r, _)| r);
    let plan = if cut { cut_plan(s, moving(), target) } else { Vec::new() };
    let mail = mail_summary(&plan);
    if mail.invitations > 0 && mail.updates > 0 {
        return Err(fail(
            "Some of these invite the people to a new meeting and some tell them of a change: cut them one at a time, so that each is asked about on its own. Nothing was changed.",
        ));
    }
    if mail.people() > 0 && !agreed {
        let tells = if mail.invitations > 0 { super::TELLS_INVITE } else { super::TELLS_UPDATE };
        return Err(super::unsent(mail.people(), tells));
    }
    let send = agreed && mail.people() > 0;
    room_for_batch(s, changes_needed(s, &resolved, &destinations, &stays, cut))?;

    let mut chosen: Vec<SelectedEvent> = Vec::with_capacity(resolved.len());
    for (((r, destination), (from, to)), stays) in resolved.iter().zip(destinations.iter()).zip(placements.iter().copied()).zip(&stays) {
        if *stays {
            // (An item the paste leaves where it is, as an all-day one on its day beside a timed one
            // moved within the day: nothing of it changes, and nobody is written to about it.)
            chosen.push(r.item);
        } else if cut {
            // The owner's one yes is for the mail the plan names: an item that writes to nobody (a
            // meeting somebody else organizes) never has mail of its own approved, or rewritten.
            let tells = send && plan.iter().any(|notice| notice.item == r.item);
            chosen.push(SelectedEvent { id: cut_one(s, r, *destination, from, to, tells)?, day: None });
        } else {
            let copy = Event {
                calendar: *destination,
                title: r.event.title.clone(),
                location: r.event.location.clone(),
                description: r.event.description.clone(),
                start: from,
                end: to,
                all_day: r.event.all_day,
                reminder_minutes: r.event.reminder_minutes,
                show_as: r.event.show_as,
                categories: r.event.categories.clone(),
                private: r.event.private,
                importance: r.event.importance,
                ..Default::default()
            };
            let made = s.mailbox.add_event(copy)?;
            remote::note_created(s, made)?;
            chosen.push(SelectedEvent { id: made, day: None });
        }
    }
    // The pasted items are what is selected now, once, in the order they were pasted.
    let ids: Vec<Id> = chosen.iter().map(|item| item.id).collect();
    set_selection(s, &chosen);
    Ok(json!({"ids": ids, "cut": cut}))
}

/// A paste read and checked whole, as [`paste`] runs it and as the question about it reads it.
struct Checked {
    cut: bool,
    /// The owner's yes to the mail (`send`).
    agreed: bool,
    target: Option<Id>,
    resolved: Vec<Resolved>,
    /// Where each item goes: `target`, or its own calendar.
    destinations: Vec<Id>,
    /// When each item starts and ends there.
    placements: Vec<(DateTime, DateTime)>,
    /// Which items a cut leaves exactly where they are (never one of a copy).
    stays: Vec<bool>,
}

/// `calendar.paste`'s reading of `v`, and every refusal it can meet before it changes anything:
/// the one reader of the command and of the question about it, so that what can't go whole is
/// refused before anybody is asked about mail it would never send.
fn checked(s: &Session, v: &Value) -> Result<Checked, CmdError> {
    let Named::Items(items) = named(v)? else {
        return Err(bad(format!(
            "paste takes `items`: the calendar items to paste, 1 to {MAX_SELECTED_EVENTS}, each {{\"id\": number, \"day\"?: \"YYYY-MM-DD\"}}"
        )));
    };
    let cut = p::opt_bool(v, "cut")?.unwrap_or(false);
    let agreed = p::opt_bool(v, "send")?.unwrap_or(false);
    let start = p::datetime(v, "start")?.ok_or_else(|| bad("`start` is required"))?;
    // (`calendar`, when named, is honoured or refused: a malformed one never falls back to each
    // item's own calendar, which would report a move into the wrong one as done.)
    let target = match p::id_strict(v, "calendar")? {
        Some(c) => Some(s.mailbox.calendar(c).map(|calendar| calendar.id).ok_or_else(|| failed!("no calendar with id {id}", id = c))?),
        None => None,
    };
    let resolved = resolve(s, &items, Allow::Occurrences)?;
    let destinations: Vec<Id> = resolved.iter().map(|r| target.unwrap_or(r.event.calendar)).collect();
    let placements = placements(&resolved, start)?;
    let mut stays = Vec::with_capacity(resolved.len());
    // Everything a batch could be refused for is checked before any of it is changed.
    for (r, (destination, (from, to))) in resolved.iter().zip(destinations.iter().zip(placements.iter().copied())) {
        if !cut {
            // A copy is refused for whatever Copy Event refuses one for (#1664: files that only
            // the server keeps, which the copy would be without).
            crate::cmd::calendar_extra::copyable(s, &r.event)?;
        } else if free_busy(s, r.event.calendar) {
            return Err(fail(
                "A calendar that only shows when people are busy has no appointment to copy: open one of the calendars that can be read.",
            ));
        }
        if cut {
            super::writable(s, r.event.calendar)?;
            if r.event.recurrence.is_some() && *destination != r.event.calendar {
                return Err(fail(
                    "One occurrence of a series can't be moved to another calendar than the rest of the series. Cut the whole series, or copy this day instead.",
                ));
            }
            if r.event.recurrence.is_some() {
                // Every day of this series that the batch takes needs a place in its cancelled
                // days, against the series as it stands — the limit is one per series, not one
                // per item, so two cuts of a series with one place left are refused whole.
                let taking = resolved.iter().filter(|other| other.event.id == r.event.id && other.item.day.is_some()).count();
                if r.event.exceptions.len().saturating_add(taking) > super::MAX_SERIES_EXCEPTIONS {
                    return Err(fail("this series has too many changed days"));
                }
            }
            // (The same time in another calendar is a move into that calendar.)
            let stay = (from, to) == (r.start, r.end) && *destination == r.event.calendar;
            // A move out of a server's calendar into another is refused for what the server can't
            // move (a meeting, files attached, a series), here rather than halfway through.
            if !stay
                && r.event.recurrence.is_none()
                && *destination != r.event.calendar
                && let Some(who) = super::kept_by(s, &r.event)
            {
                let after = Event { calendar: *destination, start: from, end: to, ..(*r.event).clone() };
                super::meeting::movable(s, &r.event, &after, &who)?;
            }
            stays.push(stay);
        } else {
            stays.push(false);
        }
        super::writable(s, *destination)?;
    }
    // A cut of which nothing would move moves nothing; one where some items stay where they are
    // (all-day ones on their days while the timed ones move within the day) moves the rest.
    if cut && stays.iter().all(|stays| *stays) {
        return Err(fail("An item would land where it already is; nothing was moved."));
    }
    Ok(Checked { cut, agreed, target, resolved, destinations, placements, stays })
}

/// Where each item lands: the earliest one that isn't a whole day takes the requested time, and
/// every other item keeps the wall-clock minutes (or, for a whole day, the days) between its own
/// start and that anchor's. An all-day batch lands on the requested date.
fn placements(resolved: &[Resolved], start: DateTime) -> Result<Vec<(DateTime, DateTime)>, CmdError> {
    let timed = resolved.iter().filter(|r| !r.event.all_day).min_by_key(|r| (r.start, r.end, r.event.id));
    let anchor = match timed {
        Some(anchor) => anchor,
        None => resolved.iter().min_by_key(|r| (r.start.date, r.event.id)).ok_or_else(|| bad("`items` names no item"))?,
    };
    let mut out = Vec::with_capacity(resolved.len());
    for r in resolved {
        let (from, to) = if r.event.all_day {
            let offset = anchor.start.date.days_until(r.start.date);
            let first = on_day(start.date, offset)?;
            let days = r.start.date.days_until(r.end.date).max(1);
            let last = on_day(first, days)?;
            (DateTime::at_midnight(first), DateTime::at_midnight(last))
        } else {
            let offset = anchor.start.minutes_until(r.start);
            let at = moved(start, offset)?;
            let length = r.start.minutes_until(r.end).max(0);
            (at, moved(at, length)?)
        };
        out.push((from, to));
    }
    Ok(out)
}

/// A time `minutes` after `at` on the same wall clock, refused beyond the years a calendar has.
fn moved(at: DateTime, minutes: i64) -> Result<DateTime, CmdError> {
    let outside = || fail("the pasted items would land outside the years a calendar can hold");
    let civil = at.civil_seconds().checked_add(minutes.saturating_mul(60)).ok_or_else(outside)?;
    let moved = DateTime::from_civil_seconds(civil);
    if moved.civil_seconds() != civil {
        return Err(outside());
    }
    Ok(moved)
}

/// The day `offset` days from `base`, refused beyond the years a calendar has.
fn on_day(base: Date, offset: i64) -> Result<Date, CmdError> {
    let day = base.add_days(offset);
    if base.days_until(day) != offset {
        return Err(fail("the pasted items would land outside the years a calendar can hold"));
    }
    Ok(day)
}

/// One item of the batch moved: an appointment keeps its identity and everything of it but where
/// it is; an occurrence of a series is changed on its own day, with the series' existing update
/// path. `send` is the owner's one yes for the whole batch.
fn cut_one(s: &mut Session, source: &Resolved, calendar: Id, start: DateTime, end: DateTime, send: bool) -> Result<Id, CmdError> {
    let before: &Event = &source.event;
    if before.recurrence.is_some() {
        let day = source.item.day.ok_or_else(|| fail("a day of the series is needed to move one of its occurrences"))?;
        let changed = shifted_series(before, day, start, end, &remote::offset_of(s))?;
        let params = if send { json!({"send": true}) } else { json!({}) };
        let made = super::change_one(s, &params, before, day, changed)?;
        let id = made.get("id").and_then(Value::as_u64).ok_or_else(|| fail("the changed occurrence has no id"))?;
        if id == before.id {
            return Err(fail("the occurrence was not changed; nothing was moved"));
        }
        // The day is where it was asked to go, or nothing is: a wall-clock time this calendar
        // can't place exactly (an hour its clocks skip, a repeated one) is refused whole by the
        // command's own rollback rather than reported as moved to a time it isn't at.
        let placed = s.mailbox.event(id).ok_or_else(|| fail("the changed occurrence is gone"))?;
        if (placed.start, placed.end) != (start, end) {
            return Err(fail("That day of the series can't be put exactly where it was asked to go; nothing was moved."));
        }
        return Ok(id);
    }
    let mut after = Event { calendar, start, end, ..before.clone() };
    (after.start_repeated, after.end_repeated) = (false, false);
    super::meeting::online_fits(s, &after, Some(before))?;
    let moved = calendar != before.calendar;
    match super::kept_by(s, before) {
        Some(who) => {
            if moved {
                super::meeting::movable(s, before, &after, &who)?;
                *s.mailbox.event_mut(before.id)? = after;
                remote::note_moved(s, before.id, before)?;
            } else {
                *s.mailbox.event_mut(before.id)? = after;
                super::changed(s, before)?;
                if send && !before.origin.id.is_empty() {
                    remote::approve_mail(&mut s.mailbox, before.origin.account, before.id, &before.origin.id, "");
                }
            }
        }
        None if moved && super::served(s, calendar) => {
            super::fits_server(s, &after)?;
            if !after.attendees.is_empty() {
                // The account whose calendar it goes into organizes it from now on.
                let owner = s.mailbox.calendar(calendar).and_then(|c| remote::keeper(&s.mailbox, &c.origin)).map(|a| a.address());
                after.organizer = owner.or_else(|| after.organizer.take());
            }
            let account = s.mailbox.calendar(calendar).and_then(|c| remote::keeper(&s.mailbox, &c.origin)).map(|a| a.id);
            *s.mailbox.event_mut(before.id)? = after;
            remote::note_created(s, before.id)?;
            if send && let Some(account) = account {
                remote::approve_mail(&mut s.mailbox, account, before.id, "", "");
            }
        }
        None => *s.mailbox.event_mut(before.id)? = after,
    }
    Ok(before.id)
}

/// The series as the fields of a move leave it: moved by as much as the occurrence it changes,
/// which is where [`super::change_one`] moves that one occurrence from.
///
/// A series that repeats by a clock of its own is moved by *instants*: `change_one` reads the
/// changed master as the difference between the instant it names and the series' own start, and
/// puts the occurrence at that difference from where the occurrence really is. A count of
/// wall-clock minutes of the master is not that difference across a change of clocks, and lands
/// the moved day an hour from where it was asked to go. A series without a clock of its own, and
/// a whole-day one, are days on a wall clock: there the master moves by the wall clock, which is
/// exactly what `change_one` reads back.
fn shifted_series(series: &Event, day: Date, start: DateTime, end: DateTime, here: &dyn Fn(Timestamp) -> i32) -> Result<Event, CmdError> {
    let mut moved = series.clone();
    if series.zone().is_some() && !series.all_day {
        let (begins, ends) = series.instants(here);
        let occurrence = series.occurrence_instant(day, here);
        let occurrence_end = occurrence.saturating_add(ends.saturating_sub(begins));
        let (from, to) = (instant_at(start, here), instant_at(end, here));
        let (moved_from, moved_to) = (begins.saturating_add(from.saturating_sub(occurrence)), ends.saturating_add(to.saturating_sub(occurrence_end)));
        moved.set_instants(moved_from, moved_to, here);
        // `change_one` reads the master's times back as instants: where this calendar shows a
        // wall-clock time twice, or skips one, the master it would read isn't the one that was
        // put there, and the day would land an hour from where it was asked to go.
        if (instant_at(moved.start, here), instant_at(moved.end, here)) != (moved_from, moved_to) {
            return Err(fail("That day of the series can't be put exactly where it was asked to go; nothing was moved."));
        }
    } else {
        // (Days are days, and a wall clock is a wall clock: the occurrence's own times plus as
        // long as this move takes it.)
        let (begins, ends) = series.occurrence_times(day, here);
        moved.start = series.start.add_minutes(begins.minutes_until(start));
        moved.end = series.end.add_minutes(ends.minutes_until(end));
    }
    Ok(moved)
}

/// How many server changes a paste notes, so that a mailbox with no room for them refuses it
/// before it is made.
fn changes_needed(s: &Session, resolved: &[Resolved], destinations: &[Id], stays: &[bool], cut: bool) -> usize {
    let mut adds = 0usize;
    for ((r, destination), stays) in resolved.iter().zip(destinations).zip(stays) {
        if *stays {
            // (Left where it is: nothing is noted for its server.)
        } else if cut {
            if r.event.recurrence.is_some() {
                if super::kept_by(s, &r.event).is_some() {
                    adds = adds.saturating_add(1);
                }
            } else if super::kept_by(s, &r.event).is_some() {
                // A move queues its create now; the old copy is deleted after that succeeds.
                adds = adds.saturating_add(1);
            } else if *destination != r.event.calendar && super::served(s, *destination) {
                adds = adds.saturating_add(1);
            }
        } else if super::served(s, *destination) {
            adds = adds.saturating_add(1);
        }
    }
    adds
}

fn room_for_batch(s: &Session, adds: usize) -> Result<(), CmdError> {
    if s.mailbox.event_changes.len().saturating_add(adds) > mailcraft_model::remote::MAX_EVENT_CHANGES {
        return Err(fail("Too many calendar changes are waiting to be sent to the server. Let Send/Receive get them through first."));
    }
    Ok(())
}

/// A calendar that only says when its owner is busy: a block in it is not an appointment.
fn free_busy(s: &Session, calendar: Id) -> bool {
    s.mailbox.calendar(calendar).is_some_and(|c| c.rights == Rights::FreeBusy)
}

// ---- delete ----

/// How a batch `calendar.delete` reads a recurring item that names no day: `"scope": "series"`
/// takes the whole of it; nothing else does, and a batch never takes a series whole by default.
fn delete_allow(v: &Value) -> Result<Allow, CmdError> {
    match p::str(v, "scope") {
        None | Some("occurrence") => Ok(Allow::Occurrences),
        Some("series") => Ok(Allow::Series),
        Some(_) => Err(bad("`scope` is \"occurrence\" or \"series\"")),
    }
}

/// The items a batch `calendar.delete` names, read as the command reads them: the typed `items`
/// when they are given (strictly, with each item's own day), else the owner's typed selection.
/// A batch names its days per item, so one `day` or `occurrence` for the whole list is refused;
/// the legacy single `id` and its focus are the command's own one-event path, never a batch.
///
/// This is the one reader of a deletion batch's targets: [`delete_selected`] executes it,
/// [`items_mail_of`] counts its mail, and the agent gate asks about and pins the same set, so
/// that none of them can act on what another can't see.
fn batch_targets(s: &Session, v: &Value) -> Result<(Vec<SelectedEvent>, Allow), CmdError> {
    // (A payload that isn't an object names nothing: it is refused, never read as the selection.)
    if !v.is_object() && !v.is_null() {
        return Err(bad("the parameters are an object"));
    }
    // (`occurrence`, and the `day` of an item among several, are refused here: each item of a
    // batch names its own day, and one day for the whole of it would retarget what was typed.)
    if v.get("occurrence").is_some() || v.get("day").is_some() {
        return Err(bad("`occurrence` names one day: with several items, name each day in its item"));
    }
    if v.get("ids").is_some() {
        return Err(bad("calendar items are named by `items` (a list) or by `id` (one), not by `ids`"));
    }
    if v.get("items").is_some() && v.get("id").is_some() {
        return Err(bad("name the events by `items` or by `id`, not both"));
    }
    let allow = delete_allow(v)?;
    let items = match v.get("items") {
        Some(_) => parse_items(v)?,
        None => {
            if s.sel.events.is_empty() {
                return Err(fail("select an appointment first"));
            }
            s.sel.events.clone()
        }
    };
    Ok((items, allow))
}

/// The set a batch `calendar.delete` works on, each item resolved as the command resolves it.
///
/// `pub` so that the agent gate — the question's words and the binding of the owner's yes — reads
/// exactly the set the command would delete: the typed `items` form with `"scope": "series"` for
/// a whole series named without a day included. [`selected_items`] stays the strict concrete
/// reader for paste and for reading a selection, where a recurring item must name its own day.
pub fn delete_items(s: &Session, v: &Value) -> Result<Vec<SelectedEvent>, CmdError> {
    let (items, allow) = batch_targets(s, v)?;
    read(s, &items, allow)
}

/// `calendar.delete` over the selected set: each item is one whole event or one day of a series,
/// the whole is refused when any of it can't be done, and the mail to everyone affected is asked
/// about once. The answer says how many were deleted and which.
pub(super) fn delete_selected(s: &mut Session, v: &Value) -> CmdResult {
    let (items, allow) = batch_targets(s, v)?;
    let strict_send = p::opt_bool(v, "send")?;
    // Somebody else's meeting is declined rather than only taken out of the calendar, as one is.
    let decline = p::opt_bool(v, "decline")?.unwrap_or(false);
    let resolved = resolve(s, &items, allow)?;
    let here = remote::offset_of(s);
    // A series and one of its days are two operations on one thing: never both in one batch.
    let whole: Vec<Id> = resolved.iter().filter(|r| r.item.day.is_none() && r.event.recurrence.is_some()).map(|r| r.event.id).collect();
    if resolved.iter().any(|r| r.item.day.is_some() && whole.contains(&r.event.id)) {
        return Err(fail("A series and one of its days are both named; nothing was changed."));
    }
    for r in &resolved {
        if free_busy(s, r.event.calendar) {
            return Err(fail("A calendar that only shows when people are busy has nothing that can be deleted."));
        }
        // (As a single delete asks it: a calendar the account may only read, or a copy of one on
        // the web, #209.)
        super::may_alter(s, &r.event)?;
        if let Some(day) = r.item.day {
            // Every day of this series that the batch takes, not just this one, has to fit.
            let more = resolved.iter().filter(|other| other.event.id == r.event.id && other.item.day.is_some()).count();
            if r.event.exceptions.len().saturating_add(more) > super::MAX_SERIES_EXCEPTIONS {
                return Err(fail("this series has too many cancelled days"));
            }
            if r.event.exceptions.contains(&day) || occurrence_times(&r.event, day, &here).is_none() {
                return Err(failed!("the series doesn't happen on {date}", date = day.iso()));
            }
        }
    }
    // One question for everybody the batch writes to, and no mail without the yes.
    let plan = delete_plan(s, &resolved, decline);
    let people = reached(plan.iter().filter(|notice| notice.kind == NoticeKind::Cancellation));
    let send = match (people, strict_send) {
        (0, send) => send.unwrap_or(false),
        (_, Some(true)) => true,
        (people, _) => return Err(super::unsent(people, super::TELLS_CANCEL)),
    };
    let note = super::comment(v);
    // The cancellations the yes is for: exactly the items the plan asked about.
    let cancels: Vec<SelectedEvent> = plan.iter().filter(|notice| notice.kind == NoticeKind::Cancellation).map(|notice| notice.item).collect();
    let mut deleted = 0usize;
    let mut said: Vec<Value> = Vec::with_capacity(resolved.len());
    // (Each resolved event is the one shared before the batch began: it stands as it was while
    // the mailbox changes, and the approvals read it after.)
    let mut approvals: Vec<&Event> = Vec::new();
    for r in &resolved {
        let before: &Event = &r.event;
        let answering = decline && super::kept_by(s, before).is_some() && before.origin.guest && !before.origin.id.is_empty();
        match r.item.day {
            Some(day) => {
                s.mailbox.event_mut(before.id)?.exceptions.push(day);
                // Its server is to have that day gone too; a series not there yet arrives without it.
                if super::kept_by(s, before).is_some() && !before.origin.id.is_empty() {
                    let response = if answering { Response::Declined } else { Response::None };
                    remote::note_occurrence_gone(s, before, day, response, send, &note)?;
                }
                if answering {
                    said.push(json!({"id": before.id, "occurrence": day.iso(), "response": Response::Declined}));
                } else {
                    said.push(json!({"id": before.id, "occurrence": day.iso()}));
                }
            }
            None => {
                s.mailbox.remove_event(before.id)?;
                if answering {
                    // (A declined meeting is taken out of the calendar, and its organizer hears
                    // of the no only when the owner said it was meant.)
                    remote::note_responded(&mut s.mailbox, before, Response::Declined, send, &note)?;
                    said.push(json!({"id": before.id, "response": Response::Declined}));
                } else {
                    remote::note_deleted(&mut s.mailbox, before)?;
                    if send && cancels.contains(&r.item) {
                        approvals.push(before);
                    }
                    said.push(json!({"id": before.id}));
                }
            }
        }
        deleted += 1;
    }
    for event in approvals {
        remote::approve_mail(&mut s.mailbox, event.origin.account, event.id, &event.origin.id, &note);
    }
    // What is left of the set is what is selected, once, with the last survivor in focus.
    let left = survivors(s, &resolved);
    set_selection(s, &left);
    Ok(json!({"deleted": deleted, "items": said}))
}

/// The items of a batch that are still there after it: the event, and the day of a series that
/// wasn't cancelled.
fn survivors(s: &Session, resolved: &[Resolved]) -> Vec<SelectedEvent> {
    resolved
        .iter()
        .filter(|r| match s.mailbox.event(r.event.id) {
            None => false,
            Some(event) => match r.item.day {
                None => event.recurrence.is_none(),
                Some(day) => !event.exceptions.contains(&day),
            },
        })
        .map(|r| r.item)
        .collect()
}

// ---- what a batch asks about ----

/// The mail `calendar.paste` would make a server send, for `calendar::mail_of`: what cutting each
/// named item into `calendar` (when named) writes to, counted once for all of them. `None` when the
/// payload is malformed (it is then refused, never described from the selection) or writes to
/// nobody.
pub(super) fn paste_mail_of(s: &Session, v: &Value) -> Option<super::Mail> {
    let cut = p::opt_bool(v, "cut").ok()??;
    if !cut {
        return None;
    }
    // (Nor does one without a `start` the command can read: it is refused before anything is
    // changed or sent.)
    p::datetime(v, "start").ok().flatten()?;
    // (A payload with no readable list, or a destination that isn't one, asks about nothing:
    // the command refuses it whole rather than acting on what happens to be selected.)
    mail_summary(&paste_notices(s, v)?).kind()
}

/// The mail `calendar.delete` over a batch would make a server send, counted once for all of them.
///
/// The implicit form is the typed selection when the command reads it as a set
/// ([`super::delete_form`]): two items or more, or one that isn't the item in focus. A selection
/// of one item that has the focus is the older one-event Delete and never reaches here, nor do
/// the legacy `id`/focus and an empty selection.
pub(super) fn items_mail_of(s: &Session, id: &str, v: &Value) -> Option<super::Mail> {
    if id != "calendar.delete" || v.get("occurrence").is_some() {
        return None;
    }
    // (The set selected when the question is asked is pinned as it is now; a named list is read
    // strictly by the same reader the command and the gate use, and a malformed one is no
    // question at all. A set of one gets here only when it isn't the item in focus, and the
    // command deletes it as a set.)
    if v.get("items").is_none() && s.sel.events.is_empty() {
        return None;
    }
    let (items, allow) = batch_targets(s, v).ok()?;
    let resolved = resolve(s, &items, allow).ok()?;
    let plan = delete_plan(s, &resolved, false);
    let people = reached(plan.iter().filter(|notice| notice.kind == NoticeKind::Cancellation));
    (people > 0).then_some(super::Mail::Cancellation(people))
}

// ---- who a batch writes to ----

/// What an account's server sends about one item of a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// The meeting is cancelled: to the people invited to a meeting the owner organizes.
    Cancellation,
    /// The meeting's new time: to the people invited, for a cut that moves it where its server
    /// keeps it.
    Update,
    /// An invitation: an appointment of this computer's with people in it, cut into a calendar a
    /// server keeps.
    Invitation,
    /// The owner's no: to the organizer of somebody else's meeting that is declined.
    Decline,
}

/// What a batch makes an account's server write about one of its items, and to whom.
///
/// The one plan of a batch's mail: the command counts the people it asks about from it, the
/// agent gate builds its question and binds the owner's yes from it, and the window's question
/// names its people and accounts from it, so that none of them can say another number or other
/// people than the command writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The item, as the batch names it.
    pub item: SelectedEvent,
    pub kind: NoticeKind,
    /// The account whose server sends it.
    pub account: Id,
    /// The addresses it goes to, as the event writes them: trimmed, none empty, each once. Empty
    /// for a decline whose organizer has no address here, and for an item whose people all have
    /// none ([`Notice::unnamed`] counts those).
    pub to: Vec<String>,
    /// How many of the people invited have no address here: still people the server may write
    /// to (it keeps its own list of who is invited), as the host's rule counts them
    /// (`remote::permitted`), so each is counted on their own and never named.
    pub unnamed: usize,
}

/// The addresses everything in `notices` goes to, each person once (by address, whatever its
/// case), in the order they come.
pub fn recipients<'a>(notices: impl IntoIterator<Item = &'a Notice>) -> Vec<&'a str> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<&str> = Vec::new();
    for notice in notices {
        for address in &notice.to {
            if seen.insert(address.to_ascii_lowercase()) {
                out.push(address.as_str());
            }
        }
    }
    out
}

/// How many people everything in `notices` writes to: each address once ([`recipients`]), and
/// each person without an address on their own, as nobody can tell two of them apart. The count
/// a batch asks about, so that it asks exactly when the host would hold its change back for want
/// of the owner's yes.
pub fn reached<'a>(notices: impl IntoIterator<Item = &'a Notice> + Clone) -> usize {
    let nameless = notices.clone().into_iter().fold(0usize, |n, notice| n.saturating_add(notice.unnamed));
    recipients(notices).len().saturating_add(nameless)
}

/// How many of the people invited to `event` have no address here.
fn nameless(event: &Event) -> usize {
    event.attendees.iter().filter(|attendee| attendee.address.email.trim().is_empty()).count()
}

/// The people invited to `event`, as addresses: trimmed, none empty, each once.
fn invitees(event: &Event) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    event
        .attendees
        .iter()
        .map(|attendee| attendee.address.email.trim())
        .filter(|email| !email.is_empty() && seen.insert(email.to_ascii_lowercase()))
        .map(str::to_string)
        .collect()
}

/// What deleting `event` (`item`, of a batch or as the one event of the older form) makes a
/// server write: a cancellation to the people invited to a meeting the owner organizes that a
/// server keeps; with `decline`, the owner's no to the organizer of somebody else's meeting (sent
/// only on the owner's yes, which the caller reads); and nothing for an appointment, for one not on
/// a server yet, or for anything this computer keeps.
pub fn delete_notice(s: &Session, event: &Event, item: SelectedEvent, decline: bool) -> Option<Notice> {
    if event.origin.id.is_empty() {
        return None;
    }
    let account = remote::keeper(&s.mailbox, &event.origin)?.id;
    if event.origin.guest {
        if !decline {
            return None;
        }
        let organizer = event.organizer.as_ref().map(|organizer| organizer.email.trim()).filter(|email| !email.is_empty());
        return Some(Notice { item, kind: NoticeKind::Decline, account, to: organizer.map(str::to_string).into_iter().collect(), unnamed: 0 });
    }
    // (Everybody invited counts, an attendee without an address included: so does the host.)
    (!event.attendees.is_empty()).then(|| Notice { item, kind: NoticeKind::Cancellation, account, to: invitees(event), unnamed: nameless(event) })
}

/// What cutting `r` into `target` (when named; else its own calendar) makes a server write. One
/// occurrence of a series moves where it is, and a meeting moved in its own calendar is changed
/// there: the people invited to one the owner organizes hear of the new time. Into another
/// calendar of the same account an appointment only moves, which writes to nobody; an appointment
/// of this computer's with people in it, cut into a calendar a server keeps, is an invitation that
/// server sends.
fn cut_notice(s: &Session, r: &Resolved, target: Option<Id>) -> Option<Notice> {
    let event: &Event = &r.event;
    let destination = target.unwrap_or(event.calendar);
    let kept = remote::keeper(&s.mailbox, &event.origin);
    let (kind, account) = if event.recurrence.is_some() || (kept.is_some() && destination == event.calendar) {
        (NoticeKind::Update, kept.filter(|_| !event.origin.guest)?.id)
    } else if kept.is_none() && destination != event.calendar {
        let keeper = s.mailbox.calendar(destination).and_then(|calendar| remote::keeper(&s.mailbox, &calendar.origin))?;
        (NoticeKind::Invitation, keeper.id)
    } else {
        return None;
    };
    (!event.attendees.is_empty()).then(|| Notice { item: r.item, kind, account, to: invitees(event), unnamed: nameless(event) })
}

/// A batch delete's plan: what each item makes a server write.
fn delete_plan(s: &Session, resolved: &[Resolved], decline: bool) -> Vec<Notice> {
    resolved.iter().filter_map(|r| delete_notice(s, &r.event, r.item, decline)).collect()
}

/// A cut's plan: what each item makes a server write.
fn cut_plan<'a>(s: &Session, resolved: impl IntoIterator<Item = &'a Resolved>, target: Option<Id>) -> Vec<Notice> {
    resolved.into_iter().filter_map(|r| cut_notice(s, r, target)).collect()
}

/// What a `calendar.delete` of a set would make servers write, item by item, read as the command
/// reads it (with its `decline`). `None` when it is no set ([`super::delete_form`]) or when the
/// command would refuse it: then nothing is described.
pub fn delete_notices(s: &Session, v: &Value) -> Option<Vec<Notice>> {
    if super::delete_form(&s.sel, v).ok()? != super::DeleteForm::Batch {
        return None;
    }
    let decline = p::opt_bool(v, "decline").ok()?.unwrap_or(false);
    let (items, allow) = batch_targets(s, v).ok()?;
    let resolved = resolve(s, &items, allow).ok()?;
    Some(delete_plan(s, &resolved, decline))
}

/// Where a `calendar.paste` would put the start of each item, read as the command reads its
/// payload and places them: for the window's question, which says where each one goes. `None`
/// when the command would refuse the payload.
pub fn paste_places(s: &Session, v: &Value) -> Option<Vec<(SelectedEvent, DateTime)>> {
    let start = p::datetime(v, "start").ok().flatten()?;
    let Named::Items(items) = named(v).ok()? else { return None };
    let resolved = resolve(s, &items, Allow::Occurrences).ok()?;
    let places = placements(&resolved, start).ok()?;
    Some(resolved.iter().zip(places).map(|(r, (from, _))| (r.item, from)).collect())
}

/// When each of `items` starts by the clock here, as the commands that act on it resolve it: a day
/// of a series at the time that day happens here, by the series' own zone (which can be on the
/// date before or after the series' own day, at another hour than its first start's); a whole
/// event, or a whole series named without a day, at its own start. For the window's question,
/// which says when each item is. An item a command would refuse (gone, cancelled, a day the
/// series doesn't have) is left out, as the command never gets as far as placing it.
pub fn item_starts(s: &Session, items: &[SelectedEvent]) -> Vec<(SelectedEvent, DateTime)> {
    items.iter().filter_map(|item| resolve(s, std::slice::from_ref(item), Allow::Series).ok()).flatten().map(|r| (r.item, r.start)).collect()
}

/// What a `calendar.paste` would make servers write, item by item: nothing for a copy (a new
/// appointment with nobody in it); for a cut, what each item's move writes, and nothing for an
/// item the cut leaves where it is. `None` when the command would refuse it before changing
/// anything (no readable `cut`, `start`, `items` or `calendar`, or an item that can't go there):
/// nobody is asked about mail that a refused paste never sends.
pub fn paste_notices(s: &Session, v: &Value) -> Option<Vec<Notice>> {
    if !p::opt_bool(v, "cut").ok()?.unwrap_or(false) {
        return Some(Vec::new());
    }
    let checked = checked(s, v).ok()?;
    let moving = checked.resolved.iter().zip(&checked.stays).filter(|(_, stays)| !**stays).map(|(r, _)| r);
    Some(cut_plan(s, moving, checked.target))
}
