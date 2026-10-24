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
            meeting |= matches!(key.as_str(), "ORGANIZER" | "ATTENDEE");
            if stack.last().is_some_and(|s| s == component.name()) {
                properties.push(Property { key, group, line });
            }
        }
    }
    if !stack.is_empty() || targets != 1 {
        return Err(InvalidDocument);
    }
    Ok(Document { properties, end, meeting, nested })
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
    // Keep the original VTIMEZONE definitions. Introducing a different zone needs a future
    // operation that can supply its definition; never silently write an undefined TZID.
    for p in new.properties.iter().filter(|p| matches!(p.key.as_str(), "DTSTART" | "DTEND") && changed.contains(p.key.as_str())) {
        if let Some(zone) = timezone(&p.line.text)? {
            let mut known = false;
            for old in &source.properties {
                known |= timezone(&old.line.text)?.as_ref() == Some(&zone);
            }
            if !known {
                return Err(InvalidDocument);
            }
        }
    }
    // Every replacement consists of bytes already present in the bounded `after` document.
    let mut output = String::new();
    let mut cursor = 0;
    for p in source.properties.iter().filter(|p| changed.contains(p.key.as_str())) {
        output.push_str(original.get(cursor..p.line.span.start).ok_or(InvalidDocument)?);
        cursor = p.line.span.end;
    }
    output.push_str(original.get(cursor..source.end).ok_or(InvalidDocument)?);
    for p in &new.properties {
        if changed.contains(p.key.as_str()) {
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
