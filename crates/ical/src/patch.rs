// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An event changed here, written into the document a calendar server sent (#209 D6, ADR 0026):
//! only the properties MailCraft changes are replaced; every other line of the document (alarms,
//! attachments, another program's own properties, the zones it names) is kept as it came, folded
//! as it came (RFC 5545 section 3.1).

/// The properties of the event that are written anew: what the owner can change from here, and
/// the stamps of a change.
const REPLACED: [&str; 8] = ["SUMMARY", "LOCATION", "DESCRIPTION", "DTSTART", "DTEND", "DURATION", "DTSTAMP", "LAST-MODIFIED"];

/// Most lines a document may have to be changed here (a document is at most a megabyte).
const MAX_LINES: usize = 100_000;

/// One content line with the lines folded into it, as they came, and its name in upper case.
struct Group<'a> {
    lines: Vec<&'a str>,
    name: String,
}

fn groups(doc: &str) -> Option<Vec<Group<'_>>> {
    let mut out: Vec<Group<'_>> = Vec::new();
    for (n, line) in doc.split('\n').enumerate() {
        if n >= MAX_LINES {
            return None;
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with([' ', '\t']) {
            // (A folded line goes with the one before; one with none before is no document.)
            out.last_mut()?.lines.push(line);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let name = line.split([':', ';']).next().unwrap_or_default().trim().to_ascii_uppercase();
        out.push(Group { lines: vec![line], name });
    }
    Some(out)
}

/// The value of a group's first line after its name and parameters (`BEGIN:VEVENT` → `VEVENT`).
fn value<'a>(g: &Group<'a>) -> &'a str {
    g.lines.first().and_then(|l| l.split_once(':')).map(|(_, v)| v.trim()).unwrap_or_default()
}

/// The components a document's groups are in, for each group: the path of `BEGIN`s it is under.
fn paths(groups: &[Group<'_>]) -> Option<Vec<Vec<String>>> {
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        match g.name.as_str() {
            "BEGIN" => {
                out.push(stack.clone());
                if stack.len() >= 16 {
                    return None;
                }
                stack.push(value(g).to_ascii_uppercase());
            }
            "END" => {
                let ends = value(g).to_ascii_uppercase();
                if stack.last() != Some(&ends) {
                    return None;
                }
                stack.pop();
                out.push(stack.clone());
            }
            _ => out.push(stack.clone()),
        }
    }
    stack.is_empty().then_some(out)
}

fn is_event(path: &[String]) -> bool {
    path.len() == 2 && path.first().is_some_and(|c| c == "VCALENDAR") && path.get(1).is_some_and(|c| c == "VEVENT")
}

fn events(groups: &[Group<'_>]) -> usize {
    groups.iter().filter(|g| g.name == "BEGIN" && value(g).eq_ignore_ascii_case("VEVENT")).count()
}

/// A document's zones: each one's `TZID` and the groups of its block.
fn zones(groups: &[Group<'_>], paths: &[Vec<String>]) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < groups.len() {
        let top = paths.get(at).is_some_and(|p| p.len() == 1);
        if top && groups.get(at).is_some_and(|g| g.name == "BEGIN" && value(g).eq_ignore_ascii_case("VTIMEZONE")) {
            let start = at;
            let mut tzid = String::new();
            while at < groups.len() {
                let Some(g) = groups.get(at) else { break };
                if g.name == "TZID" && paths.get(at).is_some_and(|p| p.len() == 2) {
                    tzid = value(g).to_string();
                }
                at += 1;
                if g.name == "END" && value(g).eq_ignore_ascii_case("VTIMEZONE") && paths.get(at - 1).is_some_and(|p| p.len() == 1) {
                    break;
                }
            }
            out.push((tzid, start..at));
            continue;
        }
        at += 1;
    }
    out
}

/// `raw` (the server's document of one event) with the event's [`REPLACED`] properties taken from
/// `fresh` (a document [`crate::write_in`] made of the event as it is now), its `SEQUENCE` one
/// higher, and the zones `fresh` names that `raw` lacks added; everything else as it came. `None`
/// when either is not a whole calendar of exactly one event (a series with changed occurrences is
/// not changed this way).
pub fn patch_event(raw: &str, fresh: &str) -> Option<String> {
    let (old, new) = (groups(raw)?, groups(fresh)?);
    let (old_paths, new_paths) = (paths(&old)?, paths(&new)?);
    if events(&old) != 1 || events(&new) != 1 {
        return None;
    }
    let taken: Vec<&Group<'_>> =
        new.iter().zip(&new_paths).filter(|(g, p)| is_event(p) && REPLACED.contains(&g.name.as_str())).map(|(g, _)| g).collect();
    let have: Vec<String> = zones(&old, &old_paths).into_iter().map(|(tzid, _)| tzid).collect();
    let missing: Vec<std::ops::Range<usize>> =
        zones(&new, &new_paths).into_iter().filter(|(tzid, _)| !tzid.is_empty() && !have.contains(tzid)).map(|(_, r)| r).collect();
    let sequence = old
        .iter()
        .zip(&old_paths)
        .find(|(g, p)| is_event(p) && g.name == "SEQUENCE")
        .and_then(|(g, _)| value(g).parse::<u32>().ok())
        .map_or(1, |n| n.saturating_add(1));
    let mut out = String::with_capacity(raw.len().saturating_add(fresh.len()));
    let mut push = |line: &str| {
        out.push_str(line);
        out.push_str("\r\n");
    };
    for (g, path) in old.iter().zip(&old_paths) {
        let opens_event = g.name == "BEGIN" && value(g).eq_ignore_ascii_case("VEVENT");
        if opens_event {
            for range in &missing {
                for line in new.get(range.clone()).into_iter().flatten().flat_map(|g| g.lines.iter()) {
                    push(line);
                }
            }
        }
        // (The END of the event is the event's own line: its path is the calendar's.)
        let closes_event = g.name == "END" && value(g).eq_ignore_ascii_case("VEVENT") && path.len() == 1;
        if closes_event {
            for line in taken.iter().flat_map(|g| g.lines.iter()) {
                push(line);
            }
            push(&format!("SEQUENCE:{sequence}"));
        }
        let replaced = is_event(path) && (REPLACED.contains(&g.name.as_str()) || g.name == "SEQUENCE");
        if !replaced {
            for line in &g.lines {
                push(line);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
