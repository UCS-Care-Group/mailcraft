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
    properties_end: usize,
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
    let mut first_nested = None;
    for line in lines(text)? {
        if line.text.is_empty() && stack.is_empty() {
            continue;
        }
        let (head, value) = parts(&line.text)?;
        // Lenient readers can treat parameterised delimiters as structure. Never
        // preserve them as ordinary properties while reasoning about another tree.
        let name = head.split(';').next().ok_or(InvalidDocument)?;
        if (name.eq_ignore_ascii_case("BEGIN") || name.eq_ignore_ascii_case("END")) && name != head {
            return Err(InvalidDocument);
        }
        if head.eq_ignore_ascii_case("BEGIN") {
            if stack.last().is_some_and(|s| s == component.name()) {
                nested_start = Some(line.span.start);
                first_nested.get_or_insert(line.span.start);
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
            if key.eq_ignore_ascii_case("BEGIN") || key.eq_ignore_ascii_case("END") {
                return Err(InvalidDocument);
            }
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
    Ok(Document { properties, properties_end: first_nested.unwrap_or(end), meeting, zones, nested })
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

/// Two card property lines that say the same thing. The vCard writer normalises parameters
/// (`TYPE=work` becomes `TYPE=INTERNET,WORK`, tokens are reordered and uppercased), so the
/// source's bytes and the writer's copy of the same property may differ while the card has not
/// changed. The value and the meaningful parameters are compared; a real difference still
/// refuses the edit.
fn card_line_same(a: &str, b: &str) -> bool {
    fn parsed(line: &str) -> Option<(String, String, BTreeMap<String, BTreeSet<String>>)> {
        let (head, value) = parts(line).ok()?;
        let mut pieces = head.split(';');
        let key = pieces.next()?.trim().to_ascii_uppercase();
        let mut params: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for piece in pieces {
            let (name, value) = piece.split_once('=')?;
            let name = name.trim().to_ascii_uppercase();
            let value = value.trim().trim_matches('"');
            let values: BTreeSet<String> = if name == "TYPE" {
                value.split(',').map(|token| token.trim().to_ascii_uppercase()).filter(|token| !token.is_empty()).collect()
            } else {
                BTreeSet::from([value.to_string()])
            };
            params.entry(name).or_default().extend(values);
        }
        Some((key, value.to_string(), params))
    }
    let Some((key_a, value_a, mut params_a)) = parsed(a) else { return false };
    let Some((key_b, value_b, mut params_b)) = parsed(b) else { return false };
    if key_a != key_b || value_a != value_b {
        return false;
    }
    // `INTERNET` is the type every e-mail address has; the writer adds it where a source left it
    // out.
    if key_a == "EMAIL" {
        if let Some(types) = params_a.get_mut("TYPE") {
            types.remove("INTERNET");
        }
        if let Some(types) = params_b.get_mut("TYPE") {
            types.remove("INTERNET");
        }
    }
    params_a == params_b
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

// Recognise compact RFC 5545 DATE / DATE-TIME values before comparing their forms.
// No offsets or partially parsed timestamps: an unknown representation stays read-only.
fn compact_time_form(value: &str) -> Result<(bool, bool), InvalidDocument> {
    let utc = value.ends_with('Z');
    let local = value.strip_suffix('Z').unwrap_or(value);
    let (date, clock) = local.split_once('T').map_or((local, None), |(date, clock)| (date, Some(clock)));
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InvalidDocument);
    }
    let number = |range: Range<usize>| date.get(range).and_then(|s| s.parse::<u32>().ok()).ok_or(InvalidDocument);
    let year = number(0..4)?;
    let month = number(4..6)?;
    let day = number(6..8)?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(InvalidDocument),
    };
    if year == 0 || day == 0 || day > days {
        return Err(InvalidDocument);
    }
    if let Some(clock) = clock {
        if clock.len() != 6 || !clock.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidDocument);
        }
        let number = |range: Range<usize>| clock.get(range).and_then(|s| s.parse::<u8>().ok()).ok_or(InvalidDocument);
        if number(0..2)? > 23 || number(2..4)? > 59 || number(4..6)? > 60 {
            return Err(InvalidDocument);
        }
        Ok((false, utc))
    } else if utc {
        Err(InvalidDocument)
    } else {
        Ok((true, false))
    }
}

fn validate_rule_until(start: &str, rule: &str) -> Result<(), InvalidDocument> {
    let (zone, date, utc) = time_form(start)?;
    let (head, start_value) = parts(start)?;
    let mut value_type = false;
    let mut has_zone = false;
    for parameter in head.split(';').skip(1) {
        if let Some((name, value)) = parameter.split_once('=') {
            if name.eq_ignore_ascii_case("VALUE") {
                if value_type || !(value.eq_ignore_ascii_case("DATE") && date || value.eq_ignore_ascii_case("DATE-TIME") && !date) {
                    return Err(InvalidDocument);
                }
                value_type = true;
            } else if name.eq_ignore_ascii_case("TZID") {
                if has_zone {
                    return Err(InvalidDocument);
                }
                has_zone = true;
            }
        }
    }
    if compact_time_form(start_value)? != (date, utc) || zone.as_ref().is_some_and(|z| z.is_empty()) {
        return Err(InvalidDocument);
    }
    let (_, value) = parts(rule)?;
    let mut seen = false;
    for part in value.split(';') {
        let (name, value) = part.split_once('=').ok_or(InvalidDocument)?;
        if name.eq_ignore_ascii_case("UNTIL") {
            if seen || compact_time_form(value)? != (date, !date && (utc || zone.is_some())) {
                return Err(InvalidDocument);
            }
            seen = true;
        }
    }
    Ok(())
}

/// The timing facts of a document's nested components: an alarm's `TRIGGER` and the repeats it
/// takes (`DURATION`, `REPEAT`). What else a nested component says (an alarm's description, say)
/// is not timing, and a writer may word it its own way.
fn nested_timing(document: &Document) -> Result<Vec<String>, InvalidDocument> {
    let mut timing = Vec::new();
    for nested in &document.nested {
        for line in lines(nested)? {
            let (head, _) = parts(&line.text)?;
            let key = head.split(';').next().unwrap_or_default();
            if ["TRIGGER", "DURATION", "REPEAT"].iter().any(|name| key.eq_ignore_ascii_case(name)) {
                timing.push(line.text);
            }
        }
    }
    Ok(timing)
}

/// What an edit writes over the source: a fresh `DTSTAMP`, the `SEQUENCE` after the one the
/// source had, and a `LAST-MODIFIED` that matches the stamp. The first line is the writer's
/// stamp; `superseded` names the source properties it stands in for, which go rather than
/// keeping their old values beside the new ones.
fn revision(
    component: Component,
    changed: &BTreeSet<&str>,
    source: &BTreeMap<&str, Vec<&str>>,
    writer: &BTreeMap<&str, Vec<&str>>,
) -> Result<(Vec<String>, BTreeSet<&'static str>), InvalidDocument> {
    if component != Component::Event {
        return Ok((Vec::new(), BTreeSet::new()));
    }
    let Some(stamp) = writer.get("DTSTAMP").and_then(|lines| lines.first()).copied() else {
        return Ok((Vec::new(), BTreeSet::new()));
    };
    let (stamp_head, stamp_value) = parts(stamp)?;
    if !stamp_head.eq_ignore_ascii_case("DTSTAMP") || stamp_value.is_empty() {
        return Err(InvalidDocument);
    }
    let mut lines = vec![stamp.to_string()];
    let mut superseded = BTreeSet::from(["DTSTAMP"]);
    // A sequence the writer did not say for itself is moved on from the source's, when that one
    // is a number; one that isn't is left as it is rather than guessed at.
    if !changed.contains("SEQUENCE")
        && let Some(sequence) = source.get("SEQUENCE").and_then(|lines| lines.first()).and_then(|line| bumped_sequence(line))
    {
        lines.push(sequence);
        superseded.insert("SEQUENCE");
    }
    // A last-modified the source has says the same as the new stamp: it is when this edit was
    // made.
    if !changed.contains("LAST-MODIFIED")
        && let Some(last) = source.get("LAST-MODIFIED").and_then(|lines| lines.first())
    {
        let (head, _) = parts(last)?;
        lines.push(format!("{head}:{stamp_value}"));
        superseded.insert("LAST-MODIFIED");
    }
    Ok((lines, superseded))
}

/// `SEQUENCE:4` for a source's `SEQUENCE:3`, or none when the source's is not a number.
fn bumped_sequence(line: &str) -> Option<String> {
    let (head, value) = parts(line).ok()?;
    let sequence: u32 = value.trim().parse().ok()?;
    Some(format!("{head}:{}", sequence.saturating_add(1).min(2_147_483_647)))
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
    // The writer stamps every copy it writes, so a stamp that differs between its two copies is
    // no edit of its own. A real edit refreshes the source's, below.
    let changed: BTreeSet<&str> = changed.into_iter().filter(|key| *key != "DTSTAMP").collect();
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
    // Several readers accept alternate keys for one model field. An edit must
    // account for every alias, including one the writer no longer emits.
    let aliases: &[&[&str]] = match component {
        Component::Card => &[&["X-FAVORITE", "X-MAILCRAFT-FAVORITE"]],
        Component::Event => &[
            &["PRIORITY", "X-MICROSOFT-CDO-IMPORTANCE"],
            &["CONFERENCE", "X-MICROSOFT-SKYPETEAMSMEETINGURL"],
            &["TRANSP", "X-MICROSOFT-CDO-BUSYSTATUS"],
        ],
    };
    for keys in aliases {
        if keys.iter().any(|key| changed.contains(key)) && keys.iter().any(|key| source_values.get(key) != old_values.get(key)) {
            return Err(InvalidDocument);
        }
    }
    if component == Component::Event {
        // Recurrence and timing share DTSTART's representation. Refuse a partial edit
        // when the writer did not retain the source's complete timing information.
        let timing = ["DTSTART", "DTEND", "DURATION", "RRULE", "RDATE", "EXDATE", "EXRULE"];
        if timing.iter().any(|key| changed.contains(key))
            && (timing.iter().any(|key| source_values.get(key) != old_values.get(key)) || nested_timing(&source)? != nested_timing(&old)?)
        {
            return Err(InvalidDocument);
        }
        if changed.contains("RRULE")
            && let Some(rules) = new_values.get("RRULE")
        {
            let effective = if changed.contains("DTSTART") { &new_values } else { &source_values };
            let starts = effective.get("DTSTART").ok_or(InvalidDocument)?;
            if rules.len() != 1 || starts.len() != 1 {
                return Err(InvalidDocument);
            }
            validate_rule_until(starts.first().ok_or(InvalidDocument)?, rules.first().ok_or(InvalidDocument)?)?;
        }
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
            // A new DTEND must match the DTSTART that will actually survive in the output,
            // not a normalised start from a writer whose unchanged lines are never spliced in.
            if key == "DTEND" && source_lines.is_empty() {
                let start_values = if changed.contains("DTSTART") { &new_values } else { &source_values };
                if let (Some(start), Some(end)) = (start_values.get("DTSTART").and_then(|v| v.first()), new_lines.first())
                    && time_form(start)? != time_form(end)?
                {
                    return Err(InvalidDocument);
                }
            }
            if let (Some(before), Some(after)) = (source_lines.first(), new_lines.first()) {
                let (before, after) = (time_form(before)?, time_form(after)?);
                // A whole-day event becoming one with times (or the other way round) is an
                // intentional change of a single event, not a writer's normalisation: its date
                // form differs, and the new form is taken. A recurring series keeps its form,
                // and two timed forms that differ still refuse.
                let recurring = ["RRULE", "RDATE", "EXDATE"].iter().any(|key| source_values.contains_key(key) || old_values.contains_key(key));
                if before.1 == after.1 && before != after {
                    return Err(InvalidDocument);
                }
                if before.1 != after.1 && recurring {
                    return Err(InvalidDocument);
                }
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
        let same = if component == Component::Card {
            let source_lines = source_values.get(key).map(Vec::as_slice).unwrap_or_default();
            let old_lines = old_values.get(key).map(Vec::as_slice).unwrap_or_default();
            source_lines.len() == old_lines.len() && source_lines.iter().zip(old_lines).all(|(a, b)| card_line_same(a, b))
        } else {
            source_values.get(key) == old_values.get(key)
        };
        if !same {
            return Err(InvalidDocument);
        }
    }
    // Cancel equal occurrences, not whole property names. A surviving repeated line keeps
    // its original position, parameters and physical folding, even beside an edited line.
    let mut remove = difference(&source, &new, &changed);
    let mut add = difference(&new, &old, &changed);
    // An edit is a revision of what the server has: it carries a fresh stamp, and the sequence
    // and last-modified that say it came after.
    let (revision, superseded) = revision(component, &changed, &source_values, &new_values)?;
    // Insert property replacements before any nested alarms, preferably at the first
    // removed line. RFC 5545 §3.6.1 puts event properties before its nested components.
    let ending = if original.split_inclusive('\n').next().is_some_and(|line| line.ends_with("\r\n")) { "\r\n" } else { "\n" };
    let mut insertions = String::new();
    for p in &new.properties {
        if take(&mut add, &p.line.text) {
            for physical in after.get(p.line.span.clone()).ok_or(InvalidDocument)?.split_inclusive('\n') {
                let content = physical.strip_suffix("\r\n").or_else(|| physical.strip_suffix('\n')).unwrap_or(physical);
                insertions.push_str(content);
                insertions.push_str(ending);
            }
        }
    }
    for line in &revision {
        insertions.push_str(line);
        insertions.push_str(ending);
    }
    let removed: Vec<_> = source
        .properties
        .iter()
        .filter(|p| superseded.contains(p.key.as_str()) || take(&mut remove, &p.line.text))
        .map(|p| p.line.span.clone())
        .collect();
    let insertion = removed.first().map_or(source.properties_end, |span| span.start.min(source.properties_end));
    let mut output = String::new();
    let mut cursor = 0;
    let mut inserted = false;
    for span in removed {
        if !inserted && insertion <= span.start {
            output.push_str(original.get(cursor..insertion).ok_or(InvalidDocument)?);
            output.push_str(&insertions);
            cursor = insertion;
            inserted = true;
        }
        output.push_str(original.get(cursor..span.start).ok_or(InvalidDocument)?);
        cursor = span.end;
    }
    if !inserted {
        output.push_str(original.get(cursor..insertion).ok_or(InvalidDocument)?);
        output.push_str(&insertions);
        cursor = insertion;
    }
    output.push_str(original.get(cursor..).ok_or(InvalidDocument)?);
    if output.len() > MAX_BYTES {
        return Err(InvalidDocument);
    }
    Ok(output)
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
