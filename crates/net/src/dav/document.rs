// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded property edits for RFC 5545 events and RFC 6350 cards. Untouched source bytes stay
//! untouched, including folds, photos, time zones, extension properties and nested components.
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_LINES: usize = 65_536;
const MAX_LINE: usize = 256 * 1024;
const MAX_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Event,
    Card,
}
impl Component {
    fn name(self) -> &'static str {
        match self {
            Self::Event => "VEVENT",
            Self::Card => "VCARD",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDocument;
impl std::fmt::Display for InvalidDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the DAV document cannot be edited without losing information")
    }
}
impl std::error::Error for InvalidDocument {}

struct Line {
    text: String,
    span: Range<usize>,
}
struct Property {
    key: String,
    group: Option<String>,
    line: Line,
}
struct Document {
    properties: Vec<Property>,
    end: usize,
    meeting: bool,
    zones: BTreeSet<String>,
    nested: Vec<String>,
}

fn lines(text: &str) -> Result<Vec<Line>, InvalidDocument> {
    if text.len() > MAX_BYTES || text.contains('\0') {
        return Err(InvalidDocument);
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut start = 0;
    let mut physical = 0usize;
    for raw in text.split_inclusive('\n') {
        physical += 1;
        if physical > MAX_LINES {
            return Err(InvalidDocument);
        }
        let end = start + raw.len();
        let value = raw.trim_end_matches('\n').trim_end_matches('\r');
        if let Some(tail) = value.strip_prefix([' ', '\t']) {
            let previous = lines.last_mut().ok_or(InvalidDocument)?;
            if previous.text.len().saturating_add(tail.len()) > MAX_LINE {
                return Err(InvalidDocument);
            }
            previous.text.push_str(tail);
            previous.span.end = end;
        } else {
            if value.len() > MAX_LINE {
                return Err(InvalidDocument);
            }
            lines.push(Line { text: value.into(), span: start..end });
        }
        start = end;
    }
    Ok(lines)
}

// A colon inside a quoted parameter is not the property's value delimiter.
fn parts(line: &str) -> Result<(&str, &str), InvalidDocument> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if c == ':' && !quoted {
            return Ok((line.get(..i).ok_or(InvalidDocument)?, line.get(i + 1..).ok_or(InvalidDocument)?));
        }
    }
    Err(InvalidDocument)
}

fn parse(text: &str, component: Component) -> Result<Document, InvalidDocument> {
    let mut stack: Vec<String> = Vec::new();
    let mut properties = Vec::new();
    let mut targets = 0;
    let mut roots = 0;
    let mut end = 0;
    let mut meeting = false;
    let mut zones = BTreeSet::new();
    let mut nested = Vec::new();
    let mut nested_start = None;
    for line in lines(text)? {
        if line.text.is_empty() && stack.is_empty() {
            continue;
        }
        let (head, value) = parts(&line.text)?;
        if head.eq_ignore_ascii_case("BEGIN") {
            if stack.last().is_some_and(|s| s == component.name()) {
                nested_start = Some(line.span.start);
            }
            if stack.is_empty() {
                roots += 1;
            }
            if roots > 1 || stack.len() >= MAX_DEPTH {
                return Err(InvalidDocument);
            }
            let name = value.to_ascii_uppercase();
            if name == component.name() {
                targets += 1;
                let valid_parent = stack.is_empty() || (component == Component::Event && stack.as_slice() == ["VCALENDAR"]);
                if !valid_parent || targets > 1 {
                    return Err(InvalidDocument);
                }
            }
            stack.push(name);
        } else if head.eq_ignore_ascii_case("END") {
            let name = stack.pop().ok_or(InvalidDocument)?;
            if !name.eq_ignore_ascii_case(value) {
                return Err(InvalidDocument);
            }
            if stack.last().is_some_and(|s| s == component.name())
                && let Some(start) = nested_start.take()
            {
                nested.push(text.get(start..line.span.end).ok_or(InvalidDocument)?.to_string());
            }
            if name == component.name() {
                end = line.span.start;
            }
        } else {
            if stack.is_empty() {
                return Err(InvalidDocument);
            }
            let name = head.split(';').next().ok_or(InvalidDocument)?;
            let (group, key) = name.split_once('.').map_or((None, name), |(g, k)| (Some(g.to_ascii_uppercase()), k));
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err(InvalidDocument);
            }
            let key = key.to_ascii_uppercase();
            if stack.as_slice() == ["VCALENDAR", "VTIMEZONE"] && key == "TZID" {
                zones.insert(value.to_string());
            }
            if stack.last().is_some_and(|s| s == component.name()) {
                meeting |= matches!(key.as_str(), "ORGANIZER" | "ATTENDEE");
                properties.push(Property { key, group, line });
            }
        }
    }
    if !stack.is_empty() || targets != 1 {
        return Err(InvalidDocument);
    }
    Ok(Document { properties, end, meeting, zones, nested })
}

fn timezone(line: &str) -> Result<Option<String>, InvalidDocument> {
    let (head, _) = parts(line)?;
    Ok(head
        .split(';')
        .skip(1)
        .find_map(|p| p.split_once('=').filter(|(name, _)| name.eq_ignore_ascii_case("TZID")).map(|(_, value)| value.trim_matches('"').to_string())))
}

fn values(document: &Document) -> BTreeMap<&str, Vec<&str>> {
    let mut values: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in &document.properties {
        values.entry(&p.key).or_default().push(&p.line.text);
    }
    values
}

// Keep date, floating, UTC and named-zone forms separate. A recurring wall-clock series
// cannot be converted to instants here without changing its future daylight-saving behaviour.
fn time_form(line: &str) -> Result<(Option<String>, bool, bool), InvalidDocument> {
    let (head, value) = parts(line)?;
    let zone = timezone(line)?;
    let date =
        head.split(';').skip(1).any(|p| p.eq_ignore_ascii_case("VALUE=DATE")) || (value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit()));
    let utc = value.ends_with('Z');
    if (zone.is_some() && (date || utc)) || (date && utc) {
        return Err(InvalidDocument);
    }
    Ok((zone, date, utc))
}

fn difference<'a>(left: &'a Document, right: &Document, changed: &BTreeSet<&str>) -> BTreeMap<&'a str, usize> {
    let mut counts = BTreeMap::new();
    for p in left.properties.iter().filter(|p| changed.contains(p.key.as_str())) {
        *counts.entry(p.line.text.as_str()).or_default() += 1;
    }
    for p in &right.properties {
        take(&mut counts, &p.line.text);
    }
    counts
}

fn take(counts: &mut BTreeMap<&str, usize>, line: &str) -> bool {
    if let Some(count) = counts.get_mut(line)
        && *count > 0
    {
        *count -= 1;
        return true;
    }
    false
}

/// Change only properties that differ between the model's `before` and `after` documents.
/// Refuses changed identities, meetings, multiple events/cards, and ambiguous group labels.
/// No-op edits still validate the source. Nothing here sends a document to a server.
pub fn patch(original: &str, before: &str, after: &str, component: Component) -> Result<String, InvalidDocument> {
    let source = parse(original, component)?;
    let old = parse(before, component)?;
    let new = parse(after, component)?;
    if old.nested != new.nested {
        return Err(InvalidDocument);
    }
    if component == Component::Event && (source.meeting || old.meeting || new.meeting) {
        return Err(InvalidDocument);
    }
    let old_values = values(&old);
    let new_values = values(&new);
    for immutable in ["UID", "VERSION"] {
        if old_values.get(immutable) != new_values.get(immutable) {
            return Err(InvalidDocument);
        }
    }
    let changed: BTreeSet<&str> = old_values.keys().chain(new_values.keys()).copied().filter(|k| old_values.get(k) != new_values.get(k)).collect();
    if changed.is_empty() {
        return Ok(original.into());
    }
    // Group labels and grouped values form one unit. Until an edit can identify that unit
    // unambiguously, refuse it rather than detach a label or overwrite a photo's group.
    if source
        .properties
        .iter()
        .chain(&old.properties)
        .chain(&new.properties)
        .any(|p| (p.group.is_some() && changed.contains(p.key.as_str())) || (p.key == "X-ABLABEL" && changed.contains("X-ABLABEL")))
    {
        return Err(InvalidDocument);
    }
    if values(&source).get("UID") != old_values.get("UID") {
        return Err(InvalidDocument);
    }
    // Never mix a writer's normalised values into a different source representation.
    // In particular, a vCard 3 writer cannot safely patch a vCard 4 source.
    let source_values = values(&source);
    if source_values.get("VERSION") != old_values.get("VERSION") {
        return Err(InvalidDocument);
    }
    if component == Component::Event {
        for document in [&source_values, &old_values, &new_values] {
            if document.contains_key("DTEND") && document.contains_key("DURATION") {
                return Err(InvalidDocument);
            }
        }
        if changed.iter().any(|k| matches!(*k, "DTSTART" | "DTEND" | "DURATION")) {
            // A model that has converted DURATION to DTEND has lost the original form.
            // Refuse that edit rather than preserve an incompatible length or end.
            for key in ["DTEND", "DURATION"] {
                if source_values.contains_key(key) != old_values.contains_key(key) {
                    return Err(InvalidDocument);
                }
            }
        }
        for key in ["DTSTART", "DTEND"] {
            if !changed.contains(key) {
                continue;
            }
            let source_lines = source_values.get(key).map(Vec::as_slice).unwrap_or_default();
            let new_lines = new_values.get(key).map(Vec::as_slice).unwrap_or_default();
            if source_lines.len() > 1 || new_lines.len() > 1 {
                return Err(InvalidDocument);
            }
            if let (Some(before), Some(after)) = (source_lines.first(), new_lines.first())
                && time_form(before)? != time_form(after)?
            {
                return Err(InvalidDocument);
            }
            for line in new_lines {
                if let Some(zone) = timezone(line)?
                    && !source.zones.contains(&zone)
                {
                    return Err(InvalidDocument);
                }
            }
        }
    }
    for key in &changed {
        if source_values.get(key) != old_values.get(key) {
            return Err(InvalidDocument);
        }
    }
    // Cancel equal occurrences, not whole property names. A surviving repeated line keeps
    // its original position, parameters and physical folding, even beside an edited line.
    let mut remove = difference(&source, &new, &changed);
    let mut add = difference(&new, &old, &changed);
    // Every replacement consists of bytes already present in the bounded `after` document.
    let mut output = String::new();
    let mut cursor = 0;
    for p in &source.properties {
        if !take(&mut remove, &p.line.text) {
            continue;
        }
        output.push_str(original.get(cursor..p.line.span.start).ok_or(InvalidDocument)?);
        cursor = p.line.span.end;
    }
    output.push_str(original.get(cursor..source.end).ok_or(InvalidDocument)?);
    for p in &new.properties {
        if take(&mut add, &p.line.text) {
            output.push_str(after.get(p.line.span.clone()).ok_or(InvalidDocument)?);
            if !output.ends_with('\n') {
                output.push_str("\r\n");
            }
        }
    }
    output.push_str(original.get(source.end..).ok_or(InvalidDocument)?);
    if output.len() > MAX_BYTES {
        return Err(InvalidDocument);
    }
    Ok(output)
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
