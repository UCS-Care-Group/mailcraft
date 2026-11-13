// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The view: which module is in front, how the message list is sorted and filtered, the search,
//! the calendar's range, the panes.

use mailcraft_model::search::Suggestion;
use mailcraft_model::{AccountChoice, Address};
use serde_json::{Value, json};

use super::people;
use super::tasks_remote::keeps_tasks;
use super::{bad, done, mac_or};
use crate::session::{enum_code, parse_enum};
use crate::{CalView, CmdResult, CommandSpec, Filter, Module, Pane, Scope, Session, SortBy, TaskFilter, ViewState, p};

pub mod display;

pub fn specs() -> Vec<CommandSpec> {
    let mut specs = vec![
        CommandSpec::new("view.reminders", "Reminders Window", "View › Window", reminders)
            .pure()
            .params("{} — answers the reminders that are due now: events inside their reminder time, or that ended less than a day ago"),
        CommandSpec::new("view.mail", "Mail", "Navigation", mail).reads().key("Mod+1"),
        CommandSpec::new("view.calendar", "Calendar", "Navigation", calendar).reads().key("Mod+2"),
        CommandSpec::new("view.people", "People", "Navigation", people).reads().key("Mod+3"),
        CommandSpec::new("view.tasks", "Tasks", "Navigation", tasks).reads().key("Mod+4"),
        CommandSpec::new("view.readingPane", "Reading Pane", "View › Layout", reading_pane).reads().params(r#"{"value": "right"|"bottom"|"off"}"#),
        CommandSpec::new("view.gmailCategories", "Gmail Categories", "View › Layout", gmail_categories)
            .reads()
            .when(crate::gmail::offered)
            .params(r#"{"value": "folders"|"tabs"} — Gmail's inbox categories as five folders under a Gmail account's Inbox in the folder pane, or as a row of tabs above its message list (#911)"#),
        CommandSpec::new("view.folderPane", "Folder Pane", "View › Layout", folder_pane).reads().params(TOGGLE),
        CommandSpec::new("view.conversations", "Show as Conversations", "View › Messages", conversations).reads().params(TOGGLE),
        CommandSpec::new("view.sort", "Arrange By", "View › Arrangement", sort)
            .reads()
            .params(r#"{"by": "date"|"from"|"subject"|"size"|"importance"|"categories", "descending"?: bool}"#),
        CommandSpec::new("view.reverseSort", "Reverse Sort", "View › Arrangement", reverse_sort).reads(),
        CommandSpec::new("view.filter", "Filter Email", "Home › Find", filter)
            .reads()
            .params(r#"{"value": "all"|"unread"|"flagged"|"attachments"|"toMe"}"#),
        CommandSpec::new("view.search", "Search", "Search", search)
            .reads()
            .key("Mod+E / F3")
            .params(r#"{"query": string, "scope"?: "folder"|"subfolders"|"mailbox"|"everywhere"}"#),
        CommandSpec::new("view.clearSearch", "Close Search", "Search", clear_search).reads(),
        CommandSpec::new("view.searchSuggestions", "Search Suggestions", "Search", search_suggestions_command).reads().params(
            r#"{"query": string, "cursor"?: number (characters; default: the end)} — answers what the search box offers for the word the cursor is at the end of: keywords, categories, what is: and has: take, and after from:, to: and cc: people from the contacts and the mail on this computer. Each with "name" (what the list shows), "text" (what goes in place of characters "start" to "end"), and "email" (a person's address) or "color" (a category)"#,
        ),
        CommandSpec::new("view.previewLines", "Message Preview", "View › Arrangement", preview_lines).reads().params(r#"{"value": 0-3}"#),
        CommandSpec::new("view.groups", "Show in Groups", "View › Arrangement", groups).reads().params(TOGGLE),
        CommandSpec::new("view.collapseGroup", "Collapse This Group", "View › Arrangement", collapse_group)
            .reads()
            .key("ArrowLeft")
            .selection(can_collapse_group)
            .params(GROUP),
        CommandSpec::new("view.expandGroup", "Expand This Group", "View › Arrangement", expand_group)
            .reads()
            .key("ArrowRight")
            .selection(can_expand_group)
            .params(GROUP),
        CommandSpec::new("view.olderByMonth", "Older Mail by Month and Year", "View › Arrangement", older_by_month).reads().params(TOGGLE),
        CommandSpec::new("view.collapseAllGroups", "Collapse All Groups", "View › Arrangement", collapse_all_groups).reads().when(can_collapse_all).params("{}"),
        CommandSpec::new("view.expandAllGroups", "Expand All Groups", "View › Arrangement", expand_all_groups).reads().when(can_expand_all).params("{}"),
        CommandSpec::new("view.oneLine", "Single Line", "View › Arrangement", one_line).reads().params(TOGGLE),
        CommandSpec::new("view.day", "Day", "Home › Arrange", day).reads().key(mac_or("Mod+Ctrl+1", "Mod+Alt+1")),
        CommandSpec::new("view.workWeek", "Work Week", "Home › Arrange", work_week).reads().key(mac_or("Mod+Ctrl+2", "Mod+Alt+2")),
        CommandSpec::new("view.week", "Week", "Home › Arrange", week).reads().key(mac_or("Mod+Ctrl+3", "Mod+Alt+3")),
        CommandSpec::new("view.month", "Month", "Home › Arrange", month).reads().key(mac_or("Mod+Ctrl+4", "Mod+Alt+4")),
        CommandSpec::new("view.today", "Today", "Home › Go To", today).reads(),
        CommandSpec::new("view.next", "Forward", "Home › Go To", forward).reads(),
        CommandSpec::new("view.previous", "Back", "Home › Go To", back).reads(),
        CommandSpec::new("view.goToDate", "Go to Date", "Home › Go To", go_to_date).reads().key("Mod+G").params(r#"{"date": "YYYY-MM-DD"}"#),
        CommandSpec::new("view.taskFilter", "Filter Tasks", "Home › Current View", task_filter)
            .reads()
            .params(r#"{"value": "active"|"today"|"overdue"|"completed"|"all"}"#),
        CommandSpec::new("view.taskAccount", "Show Tasks Of", "Tasks", task_account)
            .reads()
            .params(r#"{"value": "all" | "here" | number} — every account's tasks, those kept on this computer (0 is the same), or one account's"#),
        CommandSpec::new("view.peopleAccount", "Show Contacts Of", "People", people_account)
            .reads()
            .params(r#"{"value": "all" | "here" | number} — every account's contacts, those kept on this computer (0 is the same), or one account's"#),
        CommandSpec::new("view.reset", "Reset View", "View › Current View", reset).pure(),
        CommandSpec::new("view.inspect", "Inspect View", "View", inspect).reads(),
    ];
    specs.extend(display::specs());
    specs
}

const TOGGLE: &str = r#"{"value"?: bool} (default: toggle)"#;
/// For collapsing and expanding the message list's groups (#929).
const GROUP: &str = r#"{"group"?: string (a heading as the list names it: "Last Month", "Older", a sender)} (default: the group the focus is in) — in the folder in view, kept for it; while something is searched for, every group shows whole"#;

fn module(s: &mut Session, m: Module) -> CmdResult {
    s.view.module = m;
    Ok(json!({"module": m}))
}

fn mail(s: &mut Session, _: &Value) -> CmdResult {
    module(s, Module::Mail)
}

fn calendar(s: &mut Session, _: &Value) -> CmdResult {
    module(s, Module::Calendar)
}

fn people(s: &mut Session, _: &Value) -> CmdResult {
    module(s, Module::People)
}

fn tasks(s: &mut Session, _: &Value) -> CmdResult {
    module(s, Module::Tasks)
}

fn reading_pane(s: &mut Session, v: &Value) -> CmdResult {
    s.view.reading_pane = parse_enum::<Pane>(p::req_str(v, "value")?).ok_or_else(|| bad("`value` is right, bottom or off"))?;
    Ok(json!({"value": s.view.reading_pane}))
}

fn gmail_categories(s: &mut Session, v: &Value) -> CmdResult {
    s.view.gmail_tabs = match p::req_str(v, "value")? {
        "folders" => false,
        "tabs" => true,
        _ => return Err(bad("`value` is folders or tabs")),
    };
    Ok(json!({"value": if s.view.gmail_tabs { "tabs" } else { "folders" }}))
}

fn folder_pane(s: &mut Session, v: &Value) -> CmdResult {
    s.view.folder_pane = p::bool(v, "value").unwrap_or(!s.view.folder_pane);
    Ok(json!({"value": s.view.folder_pane}))
}

fn conversations(s: &mut Session, v: &Value) -> CmdResult {
    s.view.conversations = p::bool(v, "value").unwrap_or(!s.view.conversations);
    Ok(json!({"value": s.view.conversations}))
}

fn sort(s: &mut Session, v: &Value) -> CmdResult {
    let by = parse_enum::<SortBy>(p::req_str(v, "by")?).ok_or_else(|| bad("`by` is date, from, subject, size, importance or categories"))?;
    // Names start at A; dates, sizes and importance start at the top.
    let natural = by.starts_high();
    s.view.sort_descending = p::bool(v, "descending").unwrap_or(if s.view.sort == by { s.view.sort_descending } else { natural });
    s.view.sort = by;
    Ok(json!({"by": by, "descending": s.view.sort_descending}))
}

fn reverse_sort(s: &mut Session, _: &Value) -> CmdResult {
    s.view.sort_descending = !s.view.sort_descending;
    Ok(json!({"by": s.view.sort, "descending": s.view.sort_descending}))
}

fn filter(s: &mut Session, v: &Value) -> CmdResult {
    s.view.filter = parse_enum::<Filter>(p::req_str(v, "value")?).ok_or_else(|| bad("`value` is all, unread, flagged, attachments or toMe"))?;
    Ok(json!({"value": s.view.filter, "shown": crate::list::listed(&s.rows())}))
}

fn search(s: &mut Session, v: &Value) -> CmdResult {
    let query: String = p::req_str(v, "query")?.chars().filter(|c| !c.is_control()).take(2000).collect();
    if let Some(scope) = p::str(v, "scope") {
        s.view.scope = parse_enum::<Scope>(scope).ok_or_else(|| bad("`scope` is folder, subfolders, mailbox or everywhere"))?;
    }
    // A new search forgets what the servers found for the last (R6.2).
    if s.view.search != query {
        s.server_search.forget();
    }
    s.view.search = query;
    s.view.module = Module::Mail;
    Ok(json!({"query": s.view.search, "scope": s.view.scope, "found": crate::list::listed(&s.rows())}))
}

/// What the search box offers for the word being typed in `query`, the cursor at byte `cursor`
/// (#308): keywords, the mailbox's categories, what `is:` and `has:` take, and after `from:`, `to:`
/// and `cc:` the people a new message's recipients are suggested from
/// ([`compose::suggest`](super::compose::suggest)): the contacts and the mail on this computer,
/// nothing asked of a server; then, while the list has room, the owner's own addresses, which a new
/// message leaves out (`from:` one finds what was sent from it, `to:` what came to it). The
/// window's list and `view.searchSuggestions` both ask this. With nothing typed, the searches run
/// lately (`view.recentSearches`).
pub fn search_suggestions(s: &Session, query: &str, cursor: usize) -> Vec<Suggestion> {
    // An empty box offers the searches run lately, to run again.
    if query.trim().is_empty() {
        let recent = s.prefs.recent_searches();
        return recent.map(|q| Suggestion { label: q.to_string(), range: 0..query.len(), text: q.to_string(), color: None, address: None }).collect();
    }
    mailcraft_model::search::suggestions_with(query, cursor, &s.mailbox.every_category(), |typed| {
        let mut people: Vec<Address> = super::compose::suggest(s, typed, 20).into_iter().map(|who| Address::new(who.name, who.email)).collect();
        if let Some(typed) = super::compose::Typed::new(typed) {
            let own = s.mailbox.accounts.iter().filter(|a| typed.fits(&a.display_name, &a.email));
            people.extend(own.map(|a| Address::new(a.display_name.as_str(), a.email.as_str())));
        }
        people
    })
}

fn search_suggestions_command(s: &mut Session, v: &Value) -> CmdResult {
    let query: String = p::req_str(v, "query")?.chars().take(2000).collect();
    // The cursor and the answer's places in characters, as the search box counts them.
    let chars = query.chars().count();
    let cursor = p::u64(v, "cursor").map_or(chars, |c| usize::try_from(c).unwrap_or(usize::MAX).min(chars));
    let byte = query.char_indices().nth(cursor).map_or(query.len(), |(b, _)| b);
    let at = |b: usize| query.get(..b).map_or(0, |before| before.chars().count());
    let found: Vec<Value> = search_suggestions(s, &query, byte)
        .into_iter()
        .map(|x| {
            let mut one = json!({"name": x.label, "text": x.text, "start": at(x.range.start), "end": at(x.range.end)});
            if let Some(address) = x.address {
                one["email"] = json!(address);
            }
            if let Some(color) = x.color {
                one["color"] = json!(color.hex());
            }
            one
        })
        .collect();
    Ok(json!({"suggestions": found}))
}

fn clear_search(s: &mut Session, _: &Value) -> CmdResult {
    s.clear_search();
    done()
}

fn preview_lines(s: &mut Session, v: &Value) -> CmdResult {
    let n = p::u64(v, "value").ok_or_else(|| bad("`value` (0-3) is required"))?;
    s.prefs.preview_lines = n.min(3) as u8;
    Ok(json!({"value": s.prefs.preview_lines}))
}

fn groups(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.show_groups = p::bool(v, "value").unwrap_or(!s.prefs.show_groups);
    Ok(json!({"value": s.prefs.show_groups}))
}

/// The heading of the group the row at `at` is in: the nearest above it, itself included.
fn group_at(rows: &[crate::list::Row], at: usize) -> Option<String> {
    rows.get(..=at)?.iter().rev().find_map(|r| r.group.clone())
}

/// The list's rows as the window last had them, and where the focus is in them; nothing while
/// the list has no groups or something is searched for (#929).
fn grouped_focus(s: &Session) -> Option<(std::sync::Arc<Vec<crate::list::Row>>, usize)> {
    if !collapsible(s) || !mailcraft_model::search::Query::parse(&s.view.search).is_empty() {
        return None;
    }
    let rows = s.rows_in_view()?;
    let at = s.sel.focus_in(&rows)?;
    Some((rows, at))
}

/// Whether the list in view has groups that collapse: shown in groups, and not arranged by
/// category, where a message is listed under each of its categories and one heading can't stand
/// for its messages alone (#929).
fn collapsible(s: &Session) -> bool {
    s.prefs.show_groups && s.view.sort != SortBy::Categories
}

/// Collapse All Groups: while the list shows groups that collapse, and nothing is searched for
/// (a search shows every group whole, and isn't what is kept for the folder).
fn can_collapse_all(s: &Session) -> Option<&'static str> {
    // (Written out at each state: the catalogue takes a reason from where it is given.)
    if s.view.sort == SortBy::Categories {
        Some("the groups of a list arranged by category don't collapse")
    } else if !s.prefs.show_groups {
        Some("the list isn't shown in groups")
    } else if !mailcraft_model::search::Query::parse(&s.view.search).is_empty() {
        Some("while something is searched for, every group shows whole")
    } else {
        None
    }
}

/// Expand All Groups: while a group of the folder in view is collapsed.
fn can_expand_all(s: &Session) -> Option<&'static str> {
    if s.prefs.collapsed_in(s.view.folder).next().is_none() { Some("no group of this folder is collapsed") } else { None }
}

/// Left collapses the group the focus is in, when it is open (#929).
fn can_collapse_group(s: &Session) -> Option<&'static str> {
    match grouped_focus(s) {
        Some((rows, at)) if rows.get(at).is_some_and(|r| r.folded.is_none()) && group_at(&rows, at).is_some() => None,
        Some(_) => Some("the group is collapsed"),
        None => Some("select a message in a list arranged in groups first"),
    }
}

/// Right expands the collapsed group the focus is on (#929).
fn can_expand_group(s: &Session) -> Option<&'static str> {
    match grouped_focus(s) {
        Some((rows, at)) if rows.get(at).is_some_and(|r| r.folded.is_some()) => None,
        Some(_) => Some("the group is open"),
        None => Some("select a collapsed group first"),
    }
}

/// The group `v` names (`group`), else the one the focus is in.
fn named_group(s: &mut Session, v: &Value) -> Result<String, crate::CmdError> {
    if let Some(group) = v.get("group") {
        let group = group.as_str().map(str::trim).filter(|g| !g.is_empty()).ok_or_else(|| bad("`group` is a heading of the list"))?;
        if group.chars().count() > crate::Prefs::GROUP_CHARS {
            return Err(bad("`group` is a heading of the list, at most 254 characters"));
        }
        return Ok(group.to_string());
    }
    // The group is the row's, not the selected message's: a conversation is listed under the
    // heading of the message on top of it, which needn't be the one selected (a newer reply came,
    // #503), and a row under an open conversation is shown under that conversation's heading.
    // Taking the selected message's own date would collapse a group the owner isn't looking at.
    let rows = s.rows();
    let at = s.sel.focus_in(&rows).ok_or_else(|| bad("select a message in a group first, or name the `group`"))?;
    let row = rows.get(at).ok_or_else(|| bad("select a message in a group first, or name the `group`"))?;
    crate::list::heading_of(s, row.under.unwrap_or(row.id)).ok_or_else(|| bad("the message selected is in no group"))
}

/// What `view.collapseGroup` and `view.expandGroup` answer: the folder and its collapsed groups.
fn collapsed_now(s: &Session) -> CmdResult {
    let folder = s.view.folder;
    Ok(json!({"folder": folder, "collapsed": s.prefs.collapsed_in(folder).collect::<Vec<_>>()}))
}

/// Keeps `groups` collapsed in the folder in view, as many as may be kept (#929).
fn collapse(s: &mut Session, groups: Vec<String>) -> CmdResult {
    // Every state the menu refuses in, refused here too: naming the `group` makes the command run
    // while it is disabled (a command that takes a target isn't held back by the selection), so
    // an agent or the control channel would otherwise collapse a group while a search shows every
    // group whole, or with the groups turned off, and the heading would be kept for a list that
    // doesn't fold.
    if let Some(why) = can_collapse_all(s) {
        return Err(bad(why));
    }
    let folder = s.view.folder;
    let prefs = &mut s.prefs;
    let kept = prefs.collapsed_groups.get(&folder);
    if kept.is_none() && prefs.collapsed_groups.len() >= crate::Prefs::COLLAPSED_FOLDERS {
        return Err(bad("too many folders have collapsed groups: expand the groups of another folder first"));
    }
    // A heading too long to be kept would be stored and then skipped, and would take one of the
    // places all the same: it is refused here, wherever the headings came from (#929).
    if let Some(long) = groups.iter().find(|g| g.chars().count() > crate::Prefs::GROUP_CHARS) {
        return Err(failed!("the heading \"{group}\" is longer than 254 characters", group = long.chars().take(40).collect::<String>()));
    }
    // All or nothing: a group that can't be kept is said so, and none is collapsed.
    let new: std::collections::BTreeSet<&String> = groups.iter().filter(|g| kept.is_none_or(|k| !k.contains(*g))).collect();
    if kept.map_or(0, std::collections::BTreeSet::len).saturating_add(new.len()) > crate::Prefs::COLLAPSED_GROUPS {
        return Err(bad("at most 200 groups of a folder stay collapsed: expand some of them first"));
    }
    let new: std::collections::BTreeSet<String> = new.into_iter().cloned().collect();
    if new.is_empty() {
        return collapsed_now(s);
    }
    s.prefs.collapsed_groups.entry(folder).or_default().extend(groups);
    // (What the groups now hide leaves the selection in `Session::drop_folded_selection`, which
    // `normalize` runs after this command as it does after every other, #929. Running it here as
    // well would walk the list twice for every collapse.)
    collapsed_now(s)
}

/// `view.collapseGroup`: the group shows as its heading alone, with how many messages it has (#929).
fn collapse_group(s: &mut Session, v: &Value) -> CmdResult {
    let group = named_group(s, v)?;
    collapse(s, vec![group])
}

/// `view.collapseAllGroups`: every group of the list in view (#929).
fn collapse_all_groups(s: &mut Session, _: &Value) -> CmdResult {
    let groups = s.rows().iter().filter_map(|r| r.group.clone()).collect();
    collapse(s, groups)
}

/// `view.expandGroup`: the group's messages show again (#929).
fn expand_group(s: &mut Session, v: &Value) -> CmdResult {
    let folder = s.view.folder;
    let group = named_group(s, v)?;
    if let Some(kept) = s.prefs.collapsed_groups.get_mut(&folder) {
        kept.remove(&group);
        if kept.is_empty() {
            s.prefs.collapsed_groups.remove(&folder);
        }
    }
    collapsed_now(s)
}

/// `view.expandAllGroups`: every group of the folder in view shows whole again (#929).
fn expand_all_groups(s: &mut Session, _: &Value) -> CmdResult {
    let folder = s.view.folder;
    s.prefs.collapsed_groups.remove(&folder);
    collapsed_now(s)
}

/// `view.olderByMonth`: arranged by date, mail older than last month in a group for each month of
/// this year and then for each year, not in one Older group (#929).
fn older_by_month(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.older_by_month = p::bool(v, "value").unwrap_or(!s.prefs.older_by_month);
    Ok(json!({"value": s.prefs.older_by_month}))
}

fn one_line(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.one_line = p::bool(v, "value").unwrap_or(!s.prefs.one_line);
    Ok(json!({"value": s.prefs.one_line}))
}

fn cal(s: &mut Session, view: CalView) -> CmdResult {
    s.view.calendar = view;
    // The view as its button has it: Day is the one day again (`view.days` shows more).
    s.view.days = 0;
    s.view.module = Module::Calendar;
    Ok(json!({"view": view, "range": range_json(s)}))
}

fn day(s: &mut Session, _: &Value) -> CmdResult {
    cal(s, CalView::Day)
}

fn work_week(s: &mut Session, _: &Value) -> CmdResult {
    cal(s, CalView::WorkWeek)
}

fn week(s: &mut Session, _: &Value) -> CmdResult {
    cal(s, CalView::Week)
}

fn month(s: &mut Session, _: &Value) -> CmdResult {
    cal(s, CalView::Month)
}

/// The days the calendar shows for the view and date in effect (inclusive).
pub fn calendar_range(s: &Session) -> (mailcraft_time::Date, mailcraft_time::Date) {
    let d = s.view.date;
    match s.view.calendar {
        // One day, or as many from it on as `view.days` asked for.
        CalView::Day => (d, d.add_days(super::calendar_extra::days_shown(s) - 1)),
        CalView::Week => {
            let start = d.start_of_week(s.prefs.week_start);
            (start, start.add_days(6))
        }
        CalView::WorkWeek => {
            // The first to the last working day of the week in view, as the week is counted
            // (Monday to Friday; Sunday to Thursday where those are the working days and the
            // week starts on Sunday). The working days are the ones in effect: the mailbox's
            // own, or the local options'.
            let start = d.start_of_week(s.prefs.week_start);
            let first = i64::from(s.prefs.week_start.index());
            let days: Vec<i64> = super::calendar_settings::work_hours(s).days.iter().map(|w| (i64::from(w.index()) - first).rem_euclid(7)).collect();
            match (days.iter().min(), days.iter().max()) {
                (Some(lo), Some(hi)) => (start.add_days(*lo), start.add_days(*hi)),
                _ => (start.add_days((7 - first).rem_euclid(7)), start.add_days((7 - first).rem_euclid(7) + 4)),
            }
        }
        CalView::Month => {
            // Whole weeks covering the month, as a month grid shows them.
            let first = d.start_of_month().start_of_week(s.prefs.week_start);
            let last = d.end_of_month().start_of_week(s.prefs.week_start).add_days(6);
            (first, last)
        }
    }
}

fn range_json(s: &Session) -> Value {
    let (from, to) = calendar_range(s);
    json!({"from": from.iso(), "to": to.iso(), "date": s.view.date.iso()})
}

fn today(s: &mut Session, _: &Value) -> CmdResult {
    s.view.date = s.today();
    Ok(range_json(s))
}

fn shift(s: &mut Session, sign: i32) -> CmdResult {
    s.view.date = match s.view.calendar {
        CalView::Day => s.view.date.add_days(i64::from(sign) * super::calendar_extra::days_shown(s)),
        CalView::Week | CalView::WorkWeek => s.view.date.add_days(7 * i64::from(sign)),
        CalView::Month => s.view.date.add_months(sign),
    };
    Ok(range_json(s))
}

fn forward(s: &mut Session, _: &Value) -> CmdResult {
    shift(s, 1)
}

fn back(s: &mut Session, _: &Value) -> CmdResult {
    shift(s, -1)
}

fn go_to_date(s: &mut Session, v: &Value) -> CmdResult {
    s.view.date = p::date(v, "date")?.ok_or_else(|| bad("`date` (YYYY-MM-DD) is required"))?;
    s.view.module = Module::Calendar;
    Ok(range_json(s))
}

fn task_filter(s: &mut Session, v: &Value) -> CmdResult {
    s.view.task_filter =
        parse_enum::<TaskFilter>(p::req_str(v, "value")?).ok_or_else(|| bad("`value` is active, today, overdue, completed or all"))?;
    Ok(json!({"value": s.view.task_filter}))
}

fn task_account(s: &mut Session, v: &Value) -> CmdResult {
    let value = v.get("value").and_then(AccountChoice::parse).ok_or_else(|| bad("`value` is all, here or an account's id"))?;
    if matches!(value, AccountChoice::Account(account) if !keeps_tasks(&s.mailbox, account)) {
        return Err(bad("that account doesn't keep its to-dos with Microsoft"));
    }
    s.view.task_account = value;
    // The selected task stays selected only where it is listed.
    s.sel.task = s.sel.task.filter(|id| s.mailbox.task(*id).is_some_and(|t| value.shows(&t.origin)));
    Ok(json!({"value": value.code()}))
}

fn people_account(s: &mut Session, v: &Value) -> CmdResult {
    let value = v.get("value").and_then(AccountChoice::parse).ok_or_else(|| bad("`value` is all, here or an account's id"))?;
    if matches!(value, AccountChoice::Account(account) if !people::keeps_contacts(&s.mailbox, account)) {
        return Err(bad("that account's contacts aren't read from its server"));
    }
    s.view.people_account = value;
    // The selected contact stays selected only where it is listed.
    let shown = people::shown_account(s);
    s.sel.contact = s.sel.contact.filter(|id| s.mailbox.contact(*id).is_some_and(|c| shown.shows(&c.origin)));
    Ok(json!({"value": value.code()}))
}

fn reset(s: &mut Session, _: &Value) -> CmdResult {
    let keep = (s.view.module, s.view.folder, s.view.date);
    s.view = ViewState::default();
    (s.view.module, s.view.folder, s.view.date) = keep;
    done()
}

fn inspect(s: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({
        "view": s.view,
        "selection": s.sel,
        "composing": s.compose.as_ref().map(|c| json!({"mode": c.mode, "to": c.to, "cc": c.cc, "bcc": c.bcc, "subject": c.subject, "body": c.body.plain(), "rich": c.body.text(), "format": c.format, "quoted": c.quote.is_some(), "selection": c.body.selection(), "attachments": c.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), "importance": c.importance, "dirty": c.dirty, "draft": c.draft})),
        "folder": s.mailbox.folder(s.view.folder).map(|f| json!({"id": f.id, "path": s.mailbox.folder_path(f.id), "kind": enum_code(&f.kind)})),
        "calendarRange": range_json(s),
        "today": s.today().iso(),
        "canUndo": s.undo_label(),
        "canRedo": s.redo_label(),
        "dirty": s.dirty,
    }))
}

/// A reminder whose time has come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub event: mailcraft_model::Id,
    /// The occurrence it is for.
    pub start: mailcraft_time::DateTime,
    pub end: mailcraft_time::DateTime,
    pub all_day: bool,
    pub title: String,
    pub location: String,
    /// Minutes until it starts; negative once it has.
    pub minutes: i64,
}

/// The reminders that are due at `now` (local time): occurrences that are inside their reminder
/// time, and those that ended less than a day ago (a reminder stays until someone dismisses it,
/// which the window that shows them remembers). Soonest first, a hundred at most.
pub fn due_reminders(s: &Session, now: mailcraft_time::DateTime) -> Vec<Reminder> {
    // A reminder is at most four weeks ahead of its event.
    let (from, to) = (now.date.add_days(-1), now.date.add_days(29));
    let mut due: Vec<Reminder> = Vec::new();
    // How long until it starts is so many minutes that pass, not so many on the clock: the two
    // differ on a night the clocks change. (A whole-day event is by the clock: it has no instant.)
    let here = super::remote::offset_of(s);
    let instant = |wall: mailcraft_time::DateTime| mailcraft_model::zoned::instant_at(wall, &here);
    let at = instant(now);
    // Only the owner's own calendars remind (#70): not a shared mailbox's, not one somebody shares.
    let not_theirs: std::collections::HashSet<mailcraft_model::Id> =
        s.mailbox.calendars.iter().filter(|c| !super::remote::is_own_calendar(&s.mailbox, c)).map(|c| c.id).collect();
    for e in s.mailbox.events.values() {
        let Some(ahead) = e.reminder_minutes else { continue };
        if e.cancelled || not_theirs.contains(&e.calendar) {
            continue;
        }
        for o in e.occurrences_in(from, to, &here) {
            let minutes = if e.all_day { now.minutes_until(o.start) } else { instant(o.start).saturating_sub(at).div_euclid(60) };
            if minutes <= i64::from(ahead) && o.end.minutes_until(now) < 1440 {
                due.push(Reminder {
                    event: e.id,
                    start: o.start,
                    end: o.end,
                    all_day: e.all_day,
                    title: e.title.clone(),
                    location: e.location.clone(),
                    minutes,
                });
            }
        }
    }
    due.sort_by_key(|r| (r.start, r.event));
    due.truncate(100);
    due
}

fn reminders(s: &mut Session, _: &Value) -> CmdResult {
    let items: Vec<Value> = due_reminders(s, s.local_now())
        .into_iter()
        .map(|r| json!({"event": r.event, "occurrence": r.start.iso(), "start": r.start.iso(), "end": r.end.iso(), "allDay": r.all_day, "title": r.title, "location": r.location, "minutes": r.minutes}))
        .collect();
    Ok(json!({"reminders": items}))
}
