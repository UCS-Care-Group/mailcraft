// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar, what the list of calendars and the mailbox's own settings add to `calendar.rs`: the
//! calendars by group as the side pane lists them; which of them lie over one another (overlay)
//! and which stand side by side; showing one alone; a colour chosen here; and the working hours,
//! which are the mailbox's own (as a host brought them from the account's server) unless the
//! owner would rather have the local options.

use mailcraft_model::{Account, Calendar, Id, Rgb};
use mailcraft_time::Weekday;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{bad, fail, remote};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What the account's own calendars are listed under, and the calendars of other people and
/// places it follows. (What is kept here is listed under [`remote::LOCAL_CALENDAR`].)
pub const MY_CALENDARS: &str = "My Calendars";
pub const OTHER_CALENDARS: &str = "Other Calendars";

/// Most calendars that are listed, most groups of one account, and most blocks of calendars side
/// by side (the calendars beyond that share the last block).
const MAX_LISTED: usize = 200;
const MAX_GROUPS: usize = 30;
pub const MAX_PANES: usize = 8;
/// Longest name of a time zone that is kept.
const MAX_ZONE: usize = 64;
const DAY_MINUTES: i64 = 24 * 60;

/// The colours a calendar can be given here, by the names they go by. Our own colours: the same
/// ones `mailcraft-net` shows a Microsoft 365 calendar's named colour in, so a colour chosen here
/// and the same one chosen on the server look alike.
pub const CALENDAR_COLORS: [(&str, Rgb); 9] = [
    ("Blue", Rgb(0x4A, 0x90, 0xD9)),
    ("Green", Rgb(0x4C, 0xAF, 0x6A)),
    ("Orange", Rgb(0xE8, 0x8F, 0x3C)),
    ("Gray", Rgb(0x9A, 0xA0, 0xA6)),
    ("Yellow", Rgb(0xD9, 0xB5, 0x2B)),
    ("Teal", Rgb(0x2F, 0xA8, 0xA0)),
    ("Pink", Rgb(0xD9, 0x6C, 0xA6)),
    ("Brown", Rgb(0xA6, 0x7C, 0x52)),
    ("Red", Rgb(0xD9, 0x53, 0x4F)),
];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.calendars", "List Calendars", "Calendar", list)
            .pure()
            .params(r##"{} — the calendars as the side pane lists them, and how those that are shown are arranged: {"groups": [{"title", "account": number | null, "calendars": [{"id", "name", "color": "#RRGGBB" (as shown), "ownColor": bool, "visible", "readOnly", "current"}]}], "panes": [{"calendars": [number], "front": number}], "overlay": bool}"##),
        CommandSpec::new("calendar.showOnly", "Show Only This Calendar", "Home › Manage Calendars", show_only)
            .params(r#"{"id": number} — that calendar is shown and every other one hidden"#),
        CommandSpec::new("calendar.color", "Calendar Colour", "Home › Manage Calendars", color)
            .params(r##"{"id": number, "color": "#RRGGBB" | null} — the colour that calendar is shown in here, whatever its server says; null takes the choice back"##),
        CommandSpec::new("view.overlay", "Overlay", "View › Arrangement", overlay)
            .pure()
            .params(r#"{"calendar"?: number, "value"?: bool} — with a calendar: lay it over the calendars that are overlaid already (over the first one shown when none is), or with "value": false put it back beside them (default: the other way than it is); without: every calendar that is shown lies over the others (default), or none does"#),
        CommandSpec::new("view.sideBySide", "Side-by-Side", "View › Arrangement", side_by_side)
            .pure()
            .params("{} — every calendar that is shown stands beside the others, each in a block of its own"),
        CommandSpec::new("view.frontCalendar", "Bring Calendar to Front", "View › Arrangement", front)
            .pure()
            .params(r#"{"calendar": number} — of the calendars that lie over one another, the one whose items are in front; it is also the one being worked in, when it can be changed"#),
        CommandSpec::new("view.workHours", "Working Hours", "File › Options", work_hours_command)
            .pure()
            .params(r#"{"own"?: bool} — "own": true uses the working hours of the local options even when the account's mailbox has its own, false the mailbox's when it has (default: as it is). Answers the hours in effect: {"days": ["Mon", …], "start": "HH:MM", "end": "HH:MM", "from": "mailbox" | "options", "own": bool}"#),
        CommandSpec::new("sync.applyMailboxSettings", "Apply Downloaded Mailbox Settings", "Send / Receive", apply_settings_command)
            .pure()
            .params(r#"{"account": number, "email"?: string, "workingHours": {"days": ["monday", …], "start": "HH:MM", "end": "HH:MM", "zone"?: string (what the mailbox calls the time zone they are in), "offset"?: minutes east of UTC of that zone now (without it the times are taken as this computer's)} | null (none: the local options are used)} — the working hours an account's mailbox has, as a host brought them from its server (`mailcraft-net`'s `graph::mailbox_settings`)"#),
    ]
}

// ---- the list ----

/// A heading of the list of calendars with the calendars under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarGroup {
    /// What the group is called: [`MY_CALENDARS`], [`OTHER_CALENDARS`] and "On this computer" in
    /// English (a front end translates those), any other as the account's server calls it. With
    /// more accounts than one it says whose it is.
    pub title: String,
    /// The account whose server keeps its calendars; 0 for those kept on this computer.
    pub account: Id,
    pub calendars: Vec<Id>,
}

/// The calendars as the side pane lists them: for each account whose server keeps calendars, its
/// own first (the group its main calendar is in, the main calendar at the top), then its other
/// groups by their names, the calendars of other people and places last; and after them what is
/// kept on this computer. A mailbox no server keeps a calendar for has them all under
/// [`MY_CALENDARS`].
pub fn calendar_groups(s: &Session) -> Vec<CalendarGroup> {
    let mailbox = &s.mailbox;
    let keeper = |c: &Calendar| remote::keeper(mailbox, &c.origin).map(|account| account.id);
    let listed: Vec<&Calendar> = mailbox.calendars.iter().take(MAX_LISTED).collect();
    let accounts: Vec<&Account> = mailbox.accounts.iter().take(50).filter(|a| listed.iter().any(|c| keeper(c) == Some(a.id))).collect();
    let group_of = |c: &Calendar| if c.group.trim().is_empty() { MY_CALENDARS.to_string() } else { c.group.trim().to_string() };
    let mut groups: Vec<CalendarGroup> = Vec::new();
    for account in &accounts {
        let own: Vec<&Calendar> = listed.iter().copied().filter(|c| keeper(c) == Some(account.id)).collect();
        let mut names: Vec<String> = Vec::new();
        for name in own.iter().map(|c| group_of(c)) {
            if names.len() < MAX_GROUPS && !names.iter().any(|have| have.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
        let holds = |name: &str, c: &Calendar| group_of(c).eq_ignore_ascii_case(name);
        // The account's own first, whatever its server calls them; other people's last.
        let rank = |name: &str| {
            if name.eq_ignore_ascii_case(MY_CALENDARS) || own.iter().any(|c| c.origin.id.is_empty() && holds(name, c)) {
                0
            } else if name.eq_ignore_ascii_case(OTHER_CALENDARS) {
                2
            } else {
                1
            }
        };
        names.sort_by_key(|name| (rank(name), name.to_lowercase()));
        for name in names {
            let mut calendars: Vec<&Calendar> = own.iter().copied().filter(|c| holds(&name, c)).collect();
            // The main calendar at the top; the rest as the mailbox has them.
            calendars.sort_by_key(|c| !c.origin.id.is_empty());
            let whose: String = remote::who(account).chars().take(100).collect();
            let title = if accounts.len() > 1 { format!("{name} ({whose})") } else { name };
            groups.push(CalendarGroup { title, account: account.id, calendars: calendars.iter().map(|c| c.id).collect() });
        }
    }
    let here: Vec<Id> = listed.iter().filter(|c| keeper(c).is_none()).map(|c| c.id).collect();
    if !here.is_empty() {
        let title = if accounts.is_empty() { MY_CALENDARS } else { remote::LOCAL_CALENDAR };
        groups.push(CalendarGroup { title: title.to_string(), account: 0, calendars: here });
    }
    groups
}

/// The calendars that are shown, in the order of the list.
pub fn shown_calendars(s: &Session) -> Vec<Id> {
    let shown = |id: &Id| s.mailbox.calendar(*id).is_some_and(|c| c.visible);
    calendar_groups(s).iter().flat_map(|group| group.calendars.iter().copied()).filter(shown).collect()
}

/// One block of the calendar: a calendar by itself, or calendars that lie over one another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    /// The calendars in it, in the order of the list. None when no calendar is shown at all.
    pub calendars: Vec<Id>,
    /// The one whose items are in front, which is the one that is worked in there; 0 for none.
    pub front: Id,
}

/// The calendars that lie over one another: those of the shown ones that were overlaid
/// (`view.overlay`), when there are two of them at least.
fn stack(s: &Session, shown: &[Id]) -> Vec<Id> {
    let stack: Vec<Id> = shown.iter().copied().filter(|id| s.view.overlay.contains(id)).collect();
    if stack.len() < 2 { Vec::new() } else { stack }
}

/// How the calendars that are shown are arranged, from left to right: the ones that lie over
/// one another first, as one block, then each of the others in a block of its own. There is
/// always one block (an empty one when nothing is shown), and never more than [`MAX_PANES`].
pub fn panes(s: &Session) -> Vec<Pane> {
    let shown = shown_calendars(s);
    let stack = stack(s, &shown);
    let mut panes: Vec<Pane> = Vec::new();
    for id in shown.iter().copied().filter(|id| !stack.contains(id)) {
        panes.push(Pane { calendars: vec![id], front: id });
    }
    if let Some(first) = stack.first().copied() {
        let front = if stack.contains(&s.view.overlay_front) { s.view.overlay_front } else { first };
        panes.insert(0, Pane { calendars: stack, front });
    }
    if panes.len() > MAX_PANES {
        // More than there is room for side by side: the rest share the last block.
        let rest = panes.split_off(MAX_PANES - 1);
        let calendars: Vec<Id> = rest.into_iter().flat_map(|pane| pane.calendars).collect();
        let front = calendars.first().copied().unwrap_or(0);
        panes.push(Pane { calendars, front });
    }
    if panes.is_empty() {
        panes.push(Pane::default());
    }
    panes
}

/// Whether calendars lie over one another now.
pub fn overlaid(s: &Session) -> bool {
    !stack(s, &shown_calendars(s)).is_empty()
}

fn panes_json(s: &Session) -> Value {
    let panes: Vec<Value> = panes(s).iter().map(|pane| json!({"calendars": pane.calendars, "front": pane.front})).collect();
    json!({"panes": panes, "overlay": overlaid(s)})
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let current = super::calendar::current_calendar(s);
    let calendar = |id: &Id| {
        let c = s.mailbox.calendar(*id)?;
        Some(json!({
            "id": c.id, "name": c.name, "color": c.shown_color().hex(), "ownColor": c.own_color.is_some(),
            "visible": c.visible, "readOnly": remote::writable(&s.mailbox, c.id).is_err(), "current": c.id == current,
        }))
    };
    let groups: Vec<Value> = calendar_groups(s)
        .iter()
        .map(|group| {
            let account = if group.account == 0 { Value::Null } else { json!(group.account) };
            json!({"title": group.title, "account": account, "calendars": group.calendars.iter().filter_map(calendar).collect::<Vec<_>>()})
        })
        .collect();
    let mut answer = panes_json(s);
    if let Some(fields) = answer.as_object_mut() {
        fields.insert("groups".into(), Value::Array(groups));
    }
    Ok(answer)
}

fn calendar_named(s: &Session, v: &Value, key: &str) -> Result<Id, CmdError> {
    let id = p::req_id(v, key)?;
    s.mailbox.calendar(id).map(|c| c.id).ok_or_else(|| failed!("no calendar with id {id}", id))
}

fn show_only(s: &mut Session, v: &Value) -> CmdResult {
    let id = calendar_named(s, v, "id")?;
    for calendar in &mut s.mailbox.calendars {
        calendar.visible = calendar.id == id;
    }
    Ok(json!({"id": id, "visible": true}))
}

fn color(s: &mut Session, v: &Value) -> CmdResult {
    let id = calendar_named(s, v, "id")?;
    let chosen = match v.get("color") {
        None => return Err(bad("`color` is a colour like #2E8B57, or null")),
        Some(Value::Null) => None,
        Some(_) => Some(p::str(v, "color").and_then(Rgb::parse).ok_or_else(|| bad("`color` looks like #2E8B57"))?),
    };
    let calendar = s.mailbox.calendars.iter_mut().find(|c| c.id == id).ok_or_else(|| failed!("no calendar with id {id}", id))?;
    calendar.own_color = chosen;
    Ok(json!({"id": id, "color": calendar.shown_color().hex(), "ownColor": chosen.is_some()}))
}

// ---- overlay and side by side ----

fn overlay(s: &mut Session, v: &Value) -> CmdResult {
    let shown = shown_calendars(s);
    // (Calendars the mailbox no longer has are forgotten as it goes.)
    let mut over: Vec<Id> = s.view.overlay.iter().copied().filter(|id| s.mailbox.calendar(*id).is_some()).take(MAX_LISTED).collect();
    let lying = |over: &[Id]| shown.iter().filter(|id| over.contains(id)).count();
    match v.get("calendar").filter(|calendar| !calendar.is_null()) {
        None => {
            if p::bool(v, "value").unwrap_or(true) {
                // All of them, the one being worked in in front.
                let current = super::calendar::current_calendar(s);
                s.view.overlay_front = if shown.contains(&current) { current } else { 0 };
                over = shown.iter().copied().take(MAX_LISTED).collect();
            } else {
                s.view.overlay_front = 0;
                over.clear();
            }
        }
        Some(_) => {
            let id = calendar_named(s, v, "calendar")?;
            let lies = over.contains(&id) && lying(&over) >= 2;
            if p::bool(v, "value").unwrap_or(!lies) {
                if !shown.contains(&id) {
                    return Err(fail("That calendar isn't shown: tick it in the list of calendars first."));
                }
                if !over.contains(&id) {
                    over.push(id);
                }
                // Nothing is overlaid yet: it lies over the first calendar that is shown.
                if lying(&over) < 2 {
                    let base = shown.iter().copied().find(|other| *other != id).ok_or_else(|| fail("There is no other calendar to lay it over."))?;
                    over.push(base);
                }
                // It lies in front, and is the one being worked in (unless it can only be read).
                s.view.overlay_front = id;
                if remote::writable(&s.mailbox, id).is_ok() {
                    s.view.in_calendar = id;
                }
            } else {
                over.retain(|other| *other != id);
                // One calendar doesn't lie over anything.
                if lying(&over) < 2 {
                    over.clear();
                }
                if s.view.overlay_front == id || over.is_empty() {
                    s.view.overlay_front = 0;
                }
            }
        }
    }
    s.view.overlay = over;
    Ok(panes_json(s))
}

fn side_by_side(s: &mut Session, _: &Value) -> CmdResult {
    s.view.overlay.clear();
    s.view.overlay_front = 0;
    Ok(panes_json(s))
}

fn front(s: &mut Session, v: &Value) -> CmdResult {
    let id = calendar_named(s, v, "calendar")?;
    s.view.overlay_front = id;
    // It is the one being worked in too, unless it can only be read.
    if remote::writable(&s.mailbox, id).is_ok() {
        s.view.in_calendar = id;
    }
    Ok(panes_json(s))
}

// ---- working hours ----

/// The working hours an account's mailbox has (Microsoft 365 keeps them with the mailbox's
/// settings), as a host brought them. Kept with the preferences, not in the mailbox file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailboxHours {
    /// The address of the account whose mailbox they are.
    pub email: String,
    pub days: Vec<Weekday>,
    /// From and up to, in minutes of the day, by the clock of the zone the mailbox has them in.
    pub start: u16,
    pub end: u16,
    /// What the mailbox calls that zone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// That zone's offset from UTC (minutes east) when the hours were brought, when a host could
    /// tell; without it the times are taken as this computer's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
}

impl MailboxHours {
    /// The hours when they are hours: at least one day, each once, and an end after the start
    /// within the day. `None` for anything else.
    pub fn tidy(&self) -> Option<MailboxHours> {
        let mut days: Vec<Weekday> = Vec::new();
        for day in self.days.iter().take(64) {
            if !days.contains(day) {
                days.push(*day);
            }
        }
        days.sort();
        let fits = !days.is_empty() && self.start < self.end && i64::from(self.end) <= DAY_MINUTES;
        fits.then(|| MailboxHours {
            email: self.email.trim().chars().filter(|c| !c.is_control()).take(254).collect(),
            days,
            start: self.start,
            end: self.end,
            zone: self.zone.trim().chars().filter(|c| !c.is_control()).take(MAX_ZONE).collect(),
            // No zone is further from UTC than a day's worth of hours.
            offset: self.offset.filter(|offset| (-18 * 60..=18 * 60).contains(offset)),
        })
    }
}

/// The working hours in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkHours {
    pub days: Vec<Weekday>,
    /// From and up to, in minutes of the day, by the clock of this computer.
    pub start: i64,
    pub end: i64,
    /// They are the mailbox's own, not the local options.
    pub from_mailbox: bool,
}

/// The working hours the mailbox has, by the clock of this computer; `None` when it has none,
/// or the account they came for is not in the mailbox (any more).
fn mailbox_hours(s: &Session) -> Option<WorkHours> {
    let hours = s.prefs.mailbox_hours.as_ref()?.tidy()?;
    let whose = |account: &&Account| !hours.email.is_empty() && account.email.trim().eq_ignore_ascii_case(&hours.email);
    s.mailbox.accounts.iter().take(50).find(whose)?;
    let (start, length) = (i64::from(hours.start), i64::from(hours.end) - i64::from(hours.start));
    // A mailbox whose hours are in another zone: when it is that time there, it is this time
    // here. (What then runs past midnight stops there.) The zone is the one the mailbox names,
    // when that is one that is known (it is looked up as it is now, so the hours stay right
    // when only one of the two zones changes its clocks); else the offset a host gave; else
    // the times are taken as they are.
    let now = s.now();
    let theirs = mailcraft_time::Zone::named(&hours.zone).map(|zone| zone.offset_at(now)).or(hours.offset);
    let shift = theirs.map_or(0, |offset| i64::from(s.offset_at(now)).saturating_sub(i64::from(offset)));
    let start = start.saturating_add(shift).rem_euclid(DAY_MINUTES);
    Some(WorkHours { days: hours.days, start, end: (start + length).min(DAY_MINUTES), from_mailbox: true })
}

/// The working hours in effect: the mailbox's own when it has them, unless the owner would
/// rather have the ones of the local options (`view.workHours`), which is also what there is
/// without a mailbox that says.
pub fn work_hours(s: &Session) -> WorkHours {
    if !s.prefs.own_work_hours
        && let Some(hours) = mailbox_hours(s)
    {
        return hours;
    }
    let mut days: Vec<Weekday> = Vec::new();
    for day in s.prefs.work_days.iter().take(64) {
        if !days.contains(day) {
            days.push(*day);
        }
    }
    WorkHours { days, start: i64::from(s.prefs.work_start.min(24)) * 60, end: i64::from(s.prefs.work_end.min(24)) * 60, from_mailbox: false }
}

fn clock(minutes: i64) -> String {
    let minutes = minutes.clamp(0, DAY_MINUTES);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn work_hours_json(s: &Session) -> Value {
    let hours = work_hours(s);
    json!({
        "days": hours.days, "start": clock(hours.start), "end": clock(hours.end),
        "from": if hours.from_mailbox { "mailbox" } else { "options" }, "own": s.prefs.own_work_hours,
    })
}

fn work_hours_command(s: &mut Session, v: &Value) -> CmdResult {
    match v.get("own") {
        None | Some(Value::Null) => {}
        Some(_) => s.prefs.own_work_hours = p::bool(v, "own").ok_or_else(|| bad("`own` is true or false"))?,
    }
    Ok(work_hours_json(s))
}

/// Take in the working hours of an account's mailbox (`None`: it has none, and what was kept
/// goes). What isn't working hours (no days, an end before the start) is refused, and what was
/// kept stays. Not an undo step, and nothing of the mailbox changes: they are a preference.
pub fn apply_mailbox_hours(s: &mut Session, account: Id, hours: Option<&MailboxHours>) -> CmdResult {
    let owner = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    let email = owner.email.trim().to_string();
    match hours {
        None => {
            // Only what came for this account goes.
            if s.prefs.mailbox_hours.as_ref().is_some_and(|kept| kept.email.trim().eq_ignore_ascii_case(&email)) {
                s.prefs.mailbox_hours = None;
            }
        }
        Some(hours) => {
            if !hours.email.trim().is_empty() && !hours.email.trim().eq_ignore_ascii_case(&email) {
                return Err(failed!("these settings are for another account, not for {who}", who = remote::who(owner)));
            }
            let tidy = hours.tidy().ok_or_else(|| bad("`workingHours` needs at least one day, and an end after the start on the same day"))?;
            s.prefs.mailbox_hours = Some(MailboxHours { email, ..tidy });
        }
    }
    Ok(work_hours_json(s))
}

/// `08:00`, `08:00:00.0000000`: minutes of the day, midnight at the end of it included.
fn minutes_of(text: &str) -> Option<u16> {
    if text.len() > 32 {
        return None;
    }
    let mut parts = text.trim().split(':');
    let number =
        |part: Option<&str>| part.filter(|part| !part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit()))?.parse::<u16>().ok();
    let (hour, minute) = (number(parts.next())?, number(parts.next())?);
    // Seconds, when they are there, are seconds.
    let seconds = parts.next().is_none_or(|rest| rest.len() <= 12 && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.'));
    let fits = seconds && parts.next().is_none() && minute < 60 && (hour < 24 || (hour == 24 && minute == 0));
    fits.then_some(hour * 60 + minute)
}

fn apply_settings_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    let stated = p::str(v, "email").unwrap_or("").trim().to_string();
    // Settings for another account (the mailbox was replaced while they were on their way) are
    // nobody's here.
    let owner = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !stated.is_empty() && !owner.email.trim().eq_ignore_ascii_case(&stated) {
        return Err(failed!("these settings are for another account, not for {who}", who = remote::who(owner)));
    }
    let hours = match v.get("workingHours") {
        None => return Err(bad("`workingHours` is required (null for none)")),
        Some(Value::Null) => None,
        Some(given) if given.is_object() => {
            let named = p::strings(given, "days").ok_or_else(|| bad("`days` is a list of days of the week"))?;
            let days = named.iter().take(64).filter_map(|day| Weekday::parse(day).or_else(|| Weekday::from_ical(day))).collect();
            let time = |key: &str| p::str(given, key).and_then(minutes_of).ok_or_else(|| bad(format!("`{key}` is a time of day like 08:30")));
            let offset = p::i64(given, "offset").and_then(|offset| i32::try_from(offset).ok());
            Some(MailboxHours {
                email: stated,
                days,
                start: time("start")?,
                end: time("end")?,
                zone: p::str(given, "zone").unwrap_or("").to_string(),
                offset,
            })
        }
        Some(_) => return Err(bad("`workingHours` is an object, or null")),
    };
    apply_mailbox_hours(s, account, hours.as_ref())
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Mailbox, Origin};

    use super::*;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    fn session() -> Session {
        Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
    }

    /// The sample with a second account, and for each a few calendars as a server lists them.
    fn with_server_calendars() -> (Session, Id, Id) {
        let mut s = session();
        let first = s.mailbox.accounts.first().map(|a| a.id).unwrap();
        let second = s.run("account.add", &json!({"email": "bo@mail.example", "name": "Bo"})).unwrap()["id"].as_u64().unwrap();
        let add = |s: &mut Session, account: Id, remote: &str, name: &str, group: &str, read_only: bool| {
            let id = s.mailbox.alloc_id();
            let origin = Origin { account, id: remote.into(), ..Default::default() };
            s.mailbox.calendars.push(Calendar { id, name: name.into(), visible: true, origin, read_only, group: group.into(), ..Default::default() });
            id
        };
        add(&mut s, first, "c-hol", "Holidays", "Other Calendars", true);
        add(&mut s, first, "c-team", "Team", "Projects", false);
        add(&mut s, first, "c-side", "Side", "My Calendars", false);
        add(&mut s, first, "", "Calendar", "My Calendars", false);
        add(&mut s, first, "c-zoo", "Zoo", "aardvarks", false);
        add(&mut s, second, "", "Calendar", "", false);
        (s, first, second)
    }

    fn names(s: &Session, group: &CalendarGroup) -> Vec<String> {
        group.calendars.iter().filter_map(|id| s.mailbox.calendar(*id)).map(|c| c.name.clone()).collect()
    }

    #[test]
    fn the_list_has_the_accounts_own_first_then_its_groups_then_what_is_kept_here() {
        // No server keeps a calendar: they are all the owner's.
        let s = session();
        let groups = calendar_groups(&s);
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].title.as_str(), groups[0].account, names(&s, &groups[0])), ("My Calendars", 0, vec!["Calendar".to_string()]));

        let (s, first, second) = with_server_calendars();
        let groups = calendar_groups(&s);
        let whose = |account: Id| remote::who(s.mailbox.account(account).unwrap()).to_string();
        let listed: Vec<(String, Id, Vec<String>)> = groups.iter().map(|g| (g.title.clone(), g.account, names(&s, g))).collect();
        assert_eq!(
            listed,
            [
                (format!("My Calendars ({})", whose(first)), first, vec!["Calendar".to_string(), "Side".to_string()]),
                (format!("aardvarks ({})", whose(first)), first, vec!["Zoo".to_string()]),
                (format!("Projects ({})", whose(first)), first, vec!["Team".to_string()]),
                (format!("Other Calendars ({})", whose(first)), first, vec!["Holidays".to_string()]),
                (format!("My Calendars ({})", whose(second)), second, vec!["Calendar".to_string()]),
                ("On this computer".to_string(), 0, vec!["Calendar".to_string()]),
            ]
        );
        // With one account the headings need not say whose they are; and the group the main
        // calendar is in comes first whatever the server calls it.
        let mut s = s;
        s.mailbox.calendars.retain(|c| c.origin.account != second);
        for calendar in s.mailbox.calendars.iter_mut().filter(|c| c.group == "My Calendars") {
            calendar.group = "Zu Hause".into();
        }
        let titles: Vec<String> = calendar_groups(&s).iter().map(|g| g.title.clone()).collect();
        assert_eq!(titles, ["Zu Hause", "aardvarks", "Projects", "Other Calendars", "On this computer"]);
        // The calendars that are shown, in that order.
        let hidden = s.mailbox.calendars.iter().find(|c| c.name == "Zoo").map(|c| c.id).unwrap();
        s.run("calendar.showCalendar", &json!({"id": hidden, "value": false})).unwrap();
        let shown: Vec<String> = shown_calendars(&s).iter().filter_map(|id| s.mailbox.calendar(*id)).map(|c| c.name.clone()).collect();
        assert_eq!(shown, ["Calendar", "Side", "Team", "Holidays", "Calendar"]);
    }

    #[test]
    fn calendars_stand_side_by_side_until_they_are_laid_over_one_another() {
        let (mut s, ..) = with_server_calendars();
        let shown = shown_calendars(&s);
        assert_eq!(shown.len(), 7);
        let blocks = |s: &Session| panes(s).iter().map(|pane| (pane.calendars.clone(), pane.front)).collect::<Vec<_>>();
        // Each in a block of its own, in the order of the list.
        assert_eq!(blocks(&s), shown.iter().map(|id| (vec![*id], *id)).collect::<Vec<_>>());
        assert!(!overlaid(&s));
        // The third is laid over the first: they are one block, on the left, the third in front.
        let answer = s.run("view.overlay", &json!({"calendar": shown[2]})).unwrap();
        assert_eq!(answer["overlay"], true);
        assert_eq!(blocks(&s)[0], (vec![shown[0], shown[2]], shown[2]));
        assert_eq!(blocks(&s).len(), 6);
        assert_eq!(blocks(&s)[1], (vec![shown[1]], shown[1]));
        // One more joins them, and is in front; a click on a tab brings another to the front,
        // and that is the calendar being worked in from then on.
        s.run("view.overlay", &json!({"calendar": shown[3], "value": true})).unwrap();
        assert_eq!(blocks(&s)[0], (vec![shown[0], shown[2], shown[3]], shown[3]));
        s.run("view.frontCalendar", &json!({"calendar": shown[0]})).unwrap();
        assert_eq!((blocks(&s)[0].1, s.view.in_calendar), (shown[0], shown[0]));
        // One that can only be read comes to the front, and is not worked in.
        let holidays = s.mailbox.calendars.iter().find(|c| c.name == "Holidays").map(|c| c.id).unwrap();
        assert_eq!(holidays, shown[4]);
        s.run("view.overlay", &json!({"calendar": holidays})).unwrap();
        assert_eq!(blocks(&s)[0], (vec![shown[0], shown[2], shown[3], holidays], holidays));
        s.run("view.frontCalendar", &json!({"calendar": holidays})).unwrap();
        assert_eq!((panes(&s)[0].front, s.view.in_calendar), (holidays, shown[0]));
        s.run("view.overlay", &json!({"calendar": holidays})).unwrap();
        assert_eq!(blocks(&s)[0], (vec![shown[0], shown[2], shown[3]], shown[0]));
        // Taken out again one by one: the last two part together.
        s.run("view.overlay", &json!({"calendar": shown[2], "value": false})).unwrap();
        assert_eq!(blocks(&s)[0], (vec![shown[0], shown[3]], shown[0]));
        s.run("view.overlay", &json!({"calendar": shown[0]})).unwrap();
        assert!(!overlaid(&s) && s.view.overlay.is_empty() && s.view.overlay_front == 0);
        assert_eq!(blocks(&s).len(), 7);
        // All of them at once, the one being worked in in front; and back.
        s.run("calendar.setCurrent", &json!({"id": shown[1]})).unwrap();
        s.run("view.overlay", &json!({})).unwrap();
        assert_eq!(blocks(&s), [(shown.clone(), shown[1])]);
        // A calendar that is hidden leaves the block; the last one left is simply a calendar.
        for id in &shown[1..] {
            s.run("calendar.showCalendar", &json!({"id": id, "value": false})).unwrap();
        }
        assert_eq!(blocks(&s), [(vec![shown[0]], shown[0])]);
        assert!(!overlaid(&s));
        s.run("calendar.showCalendar", &json!({"id": shown[0], "value": false})).unwrap();
        assert_eq!(blocks(&s), [(Vec::new(), 0)]);
        for id in &shown {
            s.run("calendar.showCalendar", &json!({"id": id, "value": true})).unwrap();
        }
        assert_eq!(s.run("view.sideBySide", &json!({})).unwrap()["overlay"], false);
        assert_eq!(blocks(&s).len(), 7);
        // None of it is a change to the mailbox that Undo would know of.
        let before = s.undo_label();
        s.run("view.overlay", &json!({"calendar": shown[3]})).unwrap();
        assert_eq!(s.undo_label(), before);
        // What can't be: a calendar that isn't there, one that is hidden, one with nothing beside it.
        assert!(s.run("view.overlay", &json!({"calendar": 999_999})).is_err());
        assert!(s.run("view.frontCalendar", &json!({})).is_err());
        s.run("calendar.showOnly", &json!({"id": shown[0]})).unwrap();
        assert!(s.run("view.overlay", &json!({"calendar": shown[1], "value": true})).is_err());
        assert!(s.run("view.overlay", &json!({"calendar": shown[0], "value": true})).is_err());
    }

    #[test]
    fn more_calendars_than_blocks_share_the_last_one() {
        let mut s = session();
        for n in 0..40 {
            s.run("calendar.addCalendar", &json!({"name": format!("Extra {n}")})).unwrap();
        }
        let blocks = panes(&s);
        assert_eq!(blocks.len(), MAX_PANES);
        assert!(blocks.iter().take(MAX_PANES - 1).all(|pane| pane.calendars.len() == 1));
        assert_eq!(blocks.last().map(|pane| pane.calendars.len()), Some(41 - (MAX_PANES - 1)));
        // A view from a file with nonsense in it.
        s.view.overlay = vec![0, 7, u64::MAX, 7, 7];
        s.view.overlay_front = u64::MAX;
        assert_eq!(panes(&s).len(), MAX_PANES);
        let _ = s.run("view.overlay", &json!({"calendar": u64::MAX}));
        s.mailbox = Mailbox::with_local_account("", "");
        s.mailbox.calendars.clear();
        assert_eq!(panes(&s), [Pane::default()]);
        assert!(calendar_groups(&s).is_empty());
    }

    #[test]
    fn one_calendar_is_shown_alone_and_a_colour_chosen_here_wins() {
        let (mut s, ..) = with_server_calendars();
        let team = s.mailbox.calendars.iter().find(|c| c.name == "Team").map(|c| c.id).unwrap();
        s.run("calendar.showOnly", &json!({"id": team})).unwrap();
        assert_eq!(shown_calendars(&s), [team]);
        assert_eq!(s.undo_label(), Some("Show Only This Calendar"));
        assert!(s.undo() && shown_calendars(&s).len() == 7);
        // A colour chosen here is what it is shown in; the server's own colour is kept under it.
        let before = s.mailbox.calendar(team).map(|c| c.color).unwrap();
        let answer = s.run("calendar.color", &json!({"id": team, "color": "#D9534F"})).unwrap();
        assert_eq!((answer["color"].as_str(), answer["ownColor"].as_bool()), (Some("#D9534F"), Some(true)));
        let calendar = s.mailbox.calendar(team).cloned().unwrap();
        assert_eq!((calendar.color, calendar.own_color, calendar.shown_color()), (before, Some(Rgb(0xD9, 0x53, 0x4F)), Rgb(0xD9, 0x53, 0x4F)));
        // What the server says its colour is next changes what is under it, not what is shown.
        let account = calendar.origin.account;
        let download = json!({"account": account, "complete": false, "calendars": [
            {"id": "c-main", "name": "Calendar", "main": true, "readOnly": false},
            {"id": "c-team", "name": "Team", "color": [1, 2, 3], "group": "Projects", "readOnly": false, "main": false},
        ]});
        s.run("sync.applyCalendars", &json!({"download": download})).unwrap();
        let calendar = s.mailbox.calendar(team).cloned().unwrap();
        assert_eq!((calendar.color, calendar.shown_color()), (Rgb(1, 2, 3), Rgb(0xD9, 0x53, 0x4F)));
        // Taken back: the server's again.
        s.run("calendar.color", &json!({"id": team, "color": null})).unwrap();
        assert_eq!(s.mailbox.calendar(team).map(|c| (c.own_color, c.shown_color())), Some((None, Rgb(1, 2, 3))));
        for params in [
            json!({}),
            json!({"id": team}),
            json!({"id": team, "color": "red"}),
            json!({"id": team, "color": 7}),
            json!({"id": 999_999, "color": null}),
        ] {
            assert!(s.run("calendar.color", &params).is_err(), "{params}");
        }
        assert!(s.run("calendar.showOnly", &json!({"id": 999_999})).is_err());
        // The list as a script reads it.
        let listed = s.run("calendar.calendars", &json!({})).unwrap();
        assert_eq!(listed["groups"].as_array().map(Vec::len), Some(6));
        assert_eq!(listed["panes"].as_array().map(Vec::len), Some(7));
        let holidays = &listed["groups"][3]["calendars"][0];
        assert_eq!(
            (holidays["name"].as_str(), holidays["readOnly"].as_bool(), holidays["current"].as_bool()),
            (Some("Holidays"), Some(true), Some(false))
        );
        // Every colour on offer is one a calendar can be given.
        for (_, rgb) in CALENDAR_COLORS {
            s.run("calendar.color", &json!({"id": team, "color": rgb.hex()})).unwrap();
        }
    }

    /// A mailbox file from before a calendar could be given a colour here loads as it was, and
    /// one in which none was given is written as it was.
    #[test]
    fn calendars_saved_before_load_unchanged() {
        let old = json!({"id": 4, "name": "Team", "color": [1, 2, 3], "visible": true, "group": "Projects"});
        let calendar: Calendar = serde_json::from_value(old.clone()).unwrap();
        assert_eq!((calendar.own_color, calendar.shown_color(), calendar.group.as_str()), (None, Rgb(1, 2, 3), "Projects"));
        assert_eq!(serde_json::to_value(&calendar).unwrap(), old);
        let chosen = Calendar { own_color: Some(Rgb(9, 8, 7)), ..calendar };
        let saved = serde_json::to_value(&chosen).unwrap();
        assert_eq!(saved["own_color"], json!([9, 8, 7]));
        assert_eq!(serde_json::from_value::<Calendar>(saved).unwrap(), chosen);
        // The whole sample mailbox, as it is saved and read back.
        let s = session();
        let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("own_color"));
    }

    /// Settings saved before these were added (the owner's `ui.json`) load as they were.
    #[test]
    fn settings_saved_before_these_were_added_still_load() {
        let prefs: crate::Prefs = serde_json::from_value(json!({"twelveHour": false, "workStart": 9, "workEnd": 18, "timeScale": 15})).unwrap();
        assert_eq!((prefs.own_work_hours, prefs.mailbox_hours.clone()), (false, None));
        assert_eq!((prefs.twelve_hour, prefs.work_start, prefs.work_end, prefs.time_scale), (false, 9, 18, 15));
        let view: crate::ViewState = serde_json::from_value(json!({"calendar": "month", "inCalendar": 7, "days": 3})).unwrap();
        assert_eq!((view.overlay.len(), view.overlay_front, view.in_calendar, view.days), (0, 0, 7, 3));
        assert_eq!(serde_json::from_value::<crate::Prefs>(json!({})).unwrap(), crate::Prefs::default());
        assert_eq!(serde_json::from_value::<crate::ViewState>(json!({})).unwrap(), crate::ViewState::default());
        // Without mailbox hours nothing new is written; with them they come back.
        let saved = serde_json::to_value(crate::Prefs::default()).unwrap();
        assert!(saved.get("mailboxHours").is_none());
        assert_eq!(saved["ownWorkHours"], false);
        let hours = MailboxHours {
            email: "avery@mail.example".into(),
            days: vec![Weekday::Mon, Weekday::Sat],
            start: 420,
            end: 900,
            zone: "W. Europe Standard Time".into(),
            offset: Some(60),
        };
        let saved = serde_json::to_value(crate::Prefs { mailbox_hours: Some(hours.clone()), own_work_hours: true, ..Default::default() }).unwrap();
        assert_eq!(saved["mailboxHours"]["start"], 420);
        let back: crate::Prefs = serde_json::from_value(saved).unwrap();
        assert_eq!((back.mailbox_hours, back.own_work_hours), (Some(hours), true));
        let saved = serde_json::to_value(crate::ViewState { overlay: vec![3, 5], overlay_front: 5, ..Default::default() }).unwrap();
        assert_eq!((saved["overlay"].clone(), saved["overlayFront"].clone()), (json!([3, 5]), json!(5)));
        assert_eq!(serde_json::from_value::<crate::ViewState>(saved).unwrap().overlay, [3, 5]);
        // Nonsense where they are kept is refused by the file's reader, or comes to nothing.
        assert!(serde_json::from_value::<crate::Prefs>(json!({"mailboxHours": "nine to five"})).is_err());
        let odd: crate::Prefs = serde_json::from_value(json!({"mailboxHours": {"days": [], "start": 900, "end": 60, "offset": 99999}})).unwrap();
        let mut s = session();
        s.prefs = odd;
        assert!(!work_hours(&s).from_mailbox);
    }

    #[test]
    fn the_mailboxs_working_hours_are_used_unless_the_owner_has_others() {
        let mut s = session();
        let account = s.mailbox.accounts.first().map(|a| a.id).unwrap();
        let email = s.mailbox.accounts.first().map(|a| a.email.clone()).unwrap();
        let hours = |s: &Session| {
            let h = work_hours(s);
            (h.days.clone(), h.start, h.end, h.from_mailbox)
        };
        let weekdays = vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri];
        // The local options to begin with.
        assert_eq!(hours(&s), (weekdays.clone(), 480, 1020, false));
        // The mailbox says Sunday to Thursday, 7:30 to 15:45, as Graph says a time.
        let settings = json!({"account": account, "email": email, "workingHours": {
            "days": ["sunday", "monday", "tuesday", "wednesday", "thursday", "monday", "someday"],
            "start": "07:30:00.0000000", "end": "15:45:00.0000000", "zone": "Customized Time Zone",
        }});
        let answer = s.run("sync.applyMailboxSettings", &settings).unwrap();
        assert_eq!((answer["from"].as_str(), answer["start"].as_str(), answer["end"].as_str()), (Some("mailbox"), Some("07:30"), Some("15:45")));
        let sunday_to_thursday = vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Sun];
        assert_eq!(hours(&s), (sunday_to_thursday.clone(), 450, 945, true));
        assert!(!s.dirty && s.undo_label().is_none(), "a preference, not a change to the mailbox");
        // The Work Week shows the mailbox's days: Sunday to Thursday, in a week that starts on
        // Sunday (as this one does).
        s.run("view.workWeek", &json!({})).unwrap();
        let (from, to) = crate::cmd::view::calendar_range(&s);
        assert_eq!((from.iso(), to.iso()), ("2026-10-11".to_string(), "2026-10-15".to_string()));
        // "Use my own working hours": the local options again, and the mailbox's are kept for later.
        let answer = s.run("view.workHours", &json!({"own": true})).unwrap();
        assert_eq!((answer["from"].as_str(), answer["own"].as_bool()), (Some("options"), Some(true)));
        assert_eq!(hours(&s), (weekdays.clone(), 480, 1020, false));
        let (from, to) = crate::cmd::view::calendar_range(&s);
        assert_eq!((from.iso(), to.iso()), ("2026-10-12".to_string(), "2026-10-16".to_string()));
        assert_eq!(s.run("view.workHours", &json!({"own": false})).unwrap()["from"], "mailbox");
        assert_eq!(s.run("view.workHours", &json!({})).unwrap()["start"], "07:30");

        // In a zone two hours east of this computer's: when it is 7:30 there it is 5:30 here.
        let mut zoned = settings.clone();
        zoned["workingHours"]["offset"] = json!(120);
        s.run("sync.applyMailboxSettings", &zoned).unwrap();
        assert_eq!(hours(&s), (sunday_to_thursday.clone(), 330, 825, true));
        // Far to the west: what runs past midnight here stops at midnight.
        zoned["workingHours"]["offset"] = json!(-600);
        s.run("sync.applyMailboxSettings", &zoned).unwrap();
        assert_eq!(hours(&s), (sunday_to_thursday.clone(), 1050, 1440, true));
        zoned["workingHours"]["offset"] = json!(0);
        s.run("sync.applyMailboxSettings", &zoned).unwrap();

        // What isn't working hours is refused, and what was kept stays.
        let kept = s.prefs.mailbox_hours.clone();
        for odd in [
            json!({"days": [], "start": "08:00", "end": "17:00"}),
            json!({"days": ["someday", 5, null], "start": "08:00", "end": "17:00"}),
            json!({"days": ["monday"], "start": "25:00", "end": "26:00"}),
            json!({"days": ["monday"], "start": "17:00", "end": "08:00"}),
            json!({"days": ["monday"], "start": "08:00", "end": "08:00"}),
            json!({"days": ["monday"], "start": "8 o'clock", "end": "17:00"}),
            json!({"days": ["monday"], "start": "08:60", "end": "17:00"}),
            json!({"days": ["monday"], "start": "08:00", "end": "24:01"}),
            json!({"days": ["monday"], "start": "-8:00", "end": "17:00"}),
            json!({"days": "x".repeat(1_000_000), "start": "08:00".repeat(100_000), "end": "17:00"}),
            json!({"days": ["monday"], "start": 480, "end": 1020}),
            json!("nine to five"),
            json!([]),
        ] {
            let params = json!({"account": account, "workingHours": odd});
            assert!(s.run("sync.applyMailboxSettings", &params).is_err(), "{}", params.to_string().chars().take(200).collect::<String>());
            assert_eq!(s.prefs.mailbox_hours, kept);
        }
        // A zone nobody has heard of, megabytes of it, and an offset that is none: the times are
        // taken as this computer's.
        let odd = json!({"days": ["monday"], "start": "08:00", "end": "24:00", "zone": "Z\u{0}one\r\n".repeat(200_000), "offset": 1e12});
        s.run("sync.applyMailboxSettings", &json!({"account": account, "workingHours": odd})).unwrap();
        assert_eq!(hours(&s), (vec![Weekday::Mon], 480, 1440, true));
        let kept = s.prefs.mailbox_hours.clone().unwrap();
        assert!(kept.zone.len() <= 64 && !kept.zone.contains('\r') && kept.offset.is_none());
        // For another account, or none: refused.
        assert!(s.run("sync.applyMailboxSettings", &json!({"account": account, "email": "eve@example.org", "workingHours": null})).is_err());
        assert!(s.run("sync.applyMailboxSettings", &json!({"account": 999_999, "workingHours": null})).is_err());
        assert!(s.run("sync.applyMailboxSettings", &json!({"account": account})).is_err());
        // The mailbox has none (any more): the local options.
        s.run("sync.applyMailboxSettings", &json!({"account": account, "workingHours": null})).unwrap();
        assert_eq!((s.prefs.mailbox_hours.clone(), hours(&s)), (None, (weekdays.clone(), 480, 1020, false)));
        // Hours that were kept for an account the mailbox doesn't have are not used.
        s.prefs.mailbox_hours = Some(MailboxHours { email: "gone@example.org".into(), ..kept });
        assert_eq!(hours(&s), (weekdays, 480, 1020, false));
        // Local options nobody would set.
        s.prefs.work_days = vec![Weekday::Sat; 500];
        s.prefs.work_start = 200;
        s.prefs.work_end = 3;
        assert_eq!(hours(&s), (vec![Weekday::Sat], 1440, 180, false));
    }

    #[test]
    fn times_of_day_are_read_as_graph_writes_them() {
        assert_eq!(minutes_of("08:00"), Some(480));
        assert_eq!(minutes_of("08:00:00.0000000"), Some(480));
        assert_eq!(minutes_of(" 17:30:00 "), Some(1050));
        assert_eq!(minutes_of("0:5"), Some(5));
        assert_eq!(minutes_of("24:00"), Some(1440));
        for odd in [
            "",
            "8",
            "24:01",
            "25:00",
            "08:60",
            "08:00:00:00",
            "08:00:xx",
            "-1:00",
            "0x8:00",
            "８:00",
            "08:00:00.0000000000000000000000000000",
            "999:00",
        ] {
            assert_eq!(minutes_of(odd), None, "{odd}");
        }
    }
}
