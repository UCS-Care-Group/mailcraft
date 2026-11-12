// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One contact or distribution list of a .pst file as a contact of the mailbox (#815), from the
//! properties of [MS-OXOCNTC] and [MS-OXPROPS]: the names, the company, the three email
//! addresses, the telephone numbers, the work, home and other addresses, the web pages, the
//! birthday, the notes and the categories. The properties in the property set PSETID_Address
//! are named ones: the file says which id each has ([`ContactIds`]).
//!
//! A personal distribution list ([MS-OXOCNTC] 2.2.2) becomes a contact whose notes list its
//! members, by name and address, as the one-off entry ids it keeps them in ([MS-OXCDATA]
//! 2.2.5.1) say, from the list's stream (2.2.2.2.4) when it has one; the model has no groups
//! yet.
//!
//! The model's contact has no picture, so a contact's picture is left out and named; what
//! Outlook keeps that the model has no place for (office, profession, spouse, anniversary,
//! instant messaging address) is named as well. What the file holds is hostile: every text is
//! cut, every list capped.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use mailcraft_model::{Contact, Labeled, PostalAddress};
use mailcraft_pst::{Error as PstError, NamedMap, PropName, PropValue, Properties, Pst};
use mailcraft_time::Date;

use super::item::{Named, Text, ends_import};

/// PSETID_Address, as a .pst file keeps a property set (the first three fields little-endian).
pub const PSETID_ADDRESS: [u8; 16] = [0x04, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

/// Most characters of a short text kept.
const MAX_SHORT: usize = 4096;
/// Most email addresses, telephone numbers, web pages and categories a contact keeps (as a
/// vCard's reader does).
const MAX_VALUES: usize = 100;
/// Most members of a distribution list kept.
pub const MAX_MEMBERS: usize = 1000;
/// Most bytes of the notes of a contact read; what is kept is cut to fit a vCard's longest line
/// ([`fit`]).
const MAX_NOTES: u64 = 1 << 20;
/// Most bytes of rich text a contact's notes are unpacked to.
const MAX_RTF: usize = 4 << 20;
const MAX_MEMBER_BYTES: u64 = 4 << 20;
/// Most entries of a list's stream looked at: more than the members kept, since some have no
/// internet address or name one already listed.
const STREAM_ENTRIES: usize = 4 * MAX_MEMBERS;
/// Longest a member entry may be (one-off entry ids are small).
const MAX_ENTRY: usize = 16 * 1024;

// The long ids (LIDs) of the named properties read.
const FILE_UNDER: u32 = 0x8005;
const EMAIL_SLOTS: [(u32, u32, u32); 3] = [(0x8082, 0x8083, 0x8084), (0x8092, 0x8093, 0x8094), (0x80A2, 0x80A3, 0x80A4)];
const HTML: u32 = 0x802B;
const WORK_ADDRESS: [u32; 5] = [0x8045, 0x8046, 0x8047, 0x8048, 0x8049];
const BIRTHDAY_LOCAL: u32 = 0x80DE;
const ANNIVERSARY_LOCAL: u32 = 0x80DF;
const LIST_NAME: u32 = 0x8053;
const LIST_ONE_OFF: u32 = 0x8054;
const LIST_MEMBERS: u32 = 0x8055;
const INSTANT_MESSAGING: u32 = 0x8062;
const LIST_STREAM: u32 = 0x8064;

fn wanted() -> Vec<u32> {
    let mut v = vec![FILE_UNDER, HTML, BIRTHDAY_LOCAL, ANNIVERSARY_LOCAL, LIST_NAME, LIST_ONE_OFF, LIST_MEMBERS, INSTANT_MESSAGING, LIST_STREAM];
    v.extend(WORK_ADDRESS);
    for (a, b, c) in EMAIL_SLOTS {
        v.extend([a, b, c]);
    }
    v
}

/// The ids a file gives the named contact properties.
#[derive(Clone, Debug, Default)]
pub struct ContactIds(HashMap<u32, u16>);

impl ContactIds {
    pub fn find(map: &NamedMap) -> ContactIds {
        let mut ids = HashMap::new();
        for lid in wanted() {
            if let Some(id) = map.find(&PSETID_ADDRESS, &PropName::Id(lid)) {
                ids.insert(lid, id);
            }
        }
        ContactIds(ids)
    }

    fn id(&self, lid: u32) -> Option<u16> {
        self.0.get(&lid).copied()
    }
}

/// What the telephone numbers are, by property: (id, label). Several are lists of numbers.
const PHONES: [(u16, &str); 18] = [
    (0x3A08, "work"),
    (0x3A1B, "work"),
    (0x3A09, "home"),
    (0x3A2F, "home"),
    (0x3A1C, "mobile"),
    (0x3A1A, "main"),
    (0x3A57, "company"),
    (0x3A24, "work fax"),
    (0x3A25, "home fax"),
    (0x3A1F, "other"),
    (0x3A21, "pager"),
    (0x3A1E, "car"),
    (0x3A2E, "assistant"),
    (0x3A02, "callback"),
    (0x3A1D, "radio"),
    (0x3A2C, "telex"),
    (0x3A4B, "text phone"),
    (0x3A2D, "ISDN"),
];

/// Street, city, state, postal code and country of the home and the other address, and of the
/// business address when the file has no named work address.
const HOME_ADDRESS: [u16; 5] = [0x3A5D, 0x3A59, 0x3A5C, 0x3A5B, 0x3A5A];
const OTHER_ADDRESS: [u16; 5] = [0x3A63, 0x3A5F, 0x3A62, 0x3A61, 0x3A60];
const BUSINESS_ADDRESS: [u16; 5] = [0x3A29, 0x3A27, 0x3A28, 0x3A2A, 0x3A26];

/// One contact or list read, and what of it couldn't be.
pub struct Built {
    pub contact: Contact,
    pub notes: Vec<String>,
    /// For a distribution list: how many members it lists here.
    pub members: Option<usize>,
}

/// Whether a message class is a contact (`Some(false)`) or a distribution list (`Some(true)`).
pub fn kind(class: &str) -> Option<bool> {
    let c = class.to_ascii_lowercase();
    let family = |prefix: &str| c == prefix || c.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'));
    if family("ipm.contact") {
        Some(false)
    } else if family("ipm.distlist") {
        Some(true)
    } else {
        None
    }
}

/// Whether `s` is an address: something before and after an `@`.
fn is_address(s: &str) -> bool {
    s.split_once('@').is_some_and(|(local, domain)| !local.trim().is_empty() && !domain.trim().is_empty())
}

/// How many bytes of the notes' vCard line `s` takes: the escapes count (a line break, comma,
/// semicolon or backslash is written as two).
fn cost(s: &str) -> usize {
    s.chars().map(|c| c.len_utf8() + usize::from(matches!(c, '\\' | ',' | ';' | '\n'))).fold(0usize, usize::saturating_add)
}

/// What the notes may take, in bytes of the vCard line: the longest line the vCard reader keeps
/// (`people_merge::MAX_NOTES`), less `NOTE:`.
fn room() -> usize {
    crate::cmd::people_merge::MAX_NOTES.saturating_sub(8)
}

/// `s` cut, at a character, to what takes at most `room` bytes of the notes' vCard line; whether
/// it was cut.
fn fit(s: &str, room: usize) -> (String, bool) {
    let mut used = 0usize;
    let mut end = s.len();
    for (at, c) in s.char_indices() {
        used = used.saturating_add(cost(c.encode_utf8(&mut [0u8; 4])));
        if used > room {
            end = at;
            break;
        }
    }
    (s.get(..end).unwrap_or_default().to_string(), end < s.len())
}

/// A text of one line: no control characters, trimmed, cut.
fn one_line(s: &str) -> Option<String> {
    let s: String = s.chars().filter(|c| !c.is_control()).take(MAX_SHORT).collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A text of lines (a street), joined with commas.
fn lines(s: &str) -> Option<String> {
    let parts: Vec<String> = s.lines().filter_map(one_line).collect();
    let joined: String = parts.join(", ").chars().take(MAX_SHORT).collect();
    (!joined.is_empty()).then_some(joined)
}

struct Reader<'a> {
    text: Text,
    p: &'a Properties,
    ids: &'a ContactIds,
}

impl Reader<'_> {
    fn tag(&self, id: u16) -> Option<String> {
        self.text.long(self.p, id).as_deref().and_then(one_line)
    }

    fn lid(&self, lid: u32) -> Option<String> {
        self.tag(self.ids.id(lid)?)
    }

    fn tag_lines(&self, id: u16) -> Option<String> {
        self.text.long(self.p, id).as_deref().and_then(lines)
    }

    fn lid_lines(&self, lid: u32) -> Option<String> {
        self.tag_lines(self.ids.id(lid)?)
    }

    fn many(&self, id: u16) -> Vec<String> {
        self.text.list(self.p, id).iter().filter_map(|s| one_line(s)).take(MAX_VALUES).collect()
    }

    fn lid_time(&self, lid: u32) -> Option<u64> {
        self.p.time(self.ids.id(lid)?)
    }
}

/// A postal address from its parts (street, city, state, postal code, country), when it has any.
fn postal(label: &str, parts: [Option<String>; 5]) -> Option<PostalAddress> {
    let [street, city, region, postal_code, country] = parts.map(Option::unwrap_or_default);
    let a = PostalAddress { label: label.to_string(), street, city, region, postal_code, country };
    (!a.is_empty()).then_some(a)
}

/// The date of a FILETIME read as a calendar day, years 1000 to 3000 (Outlook's "none" is the
/// year 4501).
fn day(ft: u64) -> Option<Date> {
    let seconds = i64::try_from(ft / 10_000_000).ok()?.checked_sub(11_644_473_600)?;
    let date = Date::from_days(seconds.div_euclid(86_400));
    (1000..=3000).contains(&date.year).then_some(date)
}

/// A property read whole, on its own when it was too large to come with the rest, within `max`.
pub(super) fn value<R: Read + Seek>(
    pst: &mut Pst<R>,
    m: &mailcraft_pst::Message,
    id: u16,
    max: u64,
    what: &str,
    notes: &mut Vec<String>,
) -> Result<Option<PropValue>, PstError> {
    match m.properties.get(id) {
        None | Some(PropValue::Damaged { .. }) => Ok(None),
        Some(PropValue::Large { size }) => {
            if *size > max {
                notes.push(format!("its {what} is larger than the limit and is left out"));
                return Ok(None);
            }
            match pst.read_property(&m.id, id, max) {
                Ok(v) => Ok(v),
                Err(e) if ends_import(&e) => Err(e),
                Err(e) => {
                    notes.push(format!("its {what}: {e}"));
                    Ok(None)
                }
            }
        }
        Some(v) => Ok(Some(v.clone())),
    }
}

/// Read the contact or list `m` into a contact of the mailbox. Never fails for what one item
/// gets wrong, only for what ends the import.
pub fn build<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, named: &Named, list: bool) -> Result<Built, PstError> {
    let ids = &named.contact;
    let mut notes: Vec<String> = Vec::new();
    for part in &m.left_out {
        notes.push(format!("a part of it couldn't be read: {}", part.reason));
    }
    let r = Reader { text: Text::of(&m.properties, named.charset), p: &m.properties, ids };
    let name = |id| r.tag(id).unwrap_or_default();
    let mut c = Contact {
        display_name: r.tag(0x3001).or_else(|| r.lid(FILE_UNDER)).unwrap_or_default(),
        first: name(0x3A06),
        middle: name(0x3A44),
        last: name(0x3A11),
        nickname: name(0x3A4F),
        company: name(0x3A16),
        title: name(0x3A17),
        department: name(0x3A18),
        ..Default::default()
    };

    // The email addresses: the internet address of each slot; an address in a directory (an
    // Exchange address is a name there, not an address) only by the address it also shows.
    let mut unusable = 0usize;
    for (kind, address, original) in EMAIL_SLOTS {
        let Some(shown) = r.lid(address) else { continue };
        let smtp = r.lid(kind).is_none_or(|k| k.eq_ignore_ascii_case("SMTP"));
        let email = if smtp && is_address(&shown) { Some(shown) } else { r.lid(original).filter(|o| is_address(o) && !o.contains(' ')) };
        match email {
            Some(e) if !c.has_email(&e) => c.emails.push(Labeled::new("email", e)),
            Some(_) => {}
            None => unusable += 1,
        }
    }
    if unusable > 0 {
        notes.push(format!("{unusable} email address(es) in a directory (Exchange) with no internet address are left out"));
    }

    let mut seen: HashSet<(&str, String)> = HashSet::new();
    for (id, label) in PHONES {
        for number in r.many(id) {
            if c.phones.len() < MAX_VALUES && seen.insert((label, number.clone())) {
                c.phones.push(Labeled::new(label, number));
            }
        }
    }

    let work = WORK_ADDRESS.map(|l| r.lid_lines(l));
    let work = if work.iter().any(Option::is_some) { work } else { BUSINESS_ADDRESS.map(|t| r.tag_lines(t)) };
    for (label, parts) in [("work", work), ("home", HOME_ADDRESS.map(|t| r.tag_lines(t))), ("other", OTHER_ADDRESS.map(|t| r.tag_lines(t)))] {
        c.addresses.extend(postal(label, parts));
    }

    for site in [r.lid(HTML), r.tag(0x3A51), r.tag(0x3A50)].into_iter().flatten() {
        if c.websites.len() < MAX_VALUES && !c.websites.contains(&site) {
            c.websites.push(site);
        }
    }

    c.birthday = r.lid_time(BIRTHDAY_LOCAL).or_else(|| r.p.time(0x3A42)).and_then(day);

    if let Some(id) = named.keywords {
        for name in r.text.list(r.p, id).iter().filter_map(|s| mailcraft_model::clean_name(s)) {
            if c.categories.len() < MAX_VALUES && !c.categories.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
                c.categories.push(name);
            }
        }
    }

    let mut body = match value(pst, m, 0x1000, MAX_NOTES, "notes", &mut notes)? {
        Some(PropValue::String(s)) => Some(s),
        Some(PropValue::String8(b)) => Some(r.text.decode(&b)),
        _ => None,
    }
    .filter(|b| !b.trim_end_matches('\0').trim().is_empty());
    let mut rtf_cut = false;
    // Outlook keeps a contact's notes as compressed rich text alone ([MS-OXRTFCP]): no plain text.
    if body.is_none()
        && let Some(PropValue::Binary(rtf)) = value(pst, m, 0x1009, MAX_NOTES, "notes (rich text)", &mut notes)?
    {
        match mailcraft_pst::decompress_rtf(&rtf, MAX_RTF).and_then(|raw| mailcraft_pst::rtf_text(&raw, room().saturating_add(8))) {
            Ok((text, cut)) => {
                body = Some(text);
                rtf_cut = cut;
            }
            Err(e) => notes.push(format!("its notes (rich text) could not be read: {e}")),
        }
    }
    if let Some(b) = body {
        // (A list's notes share the room with its members, when it has any.)
        let has_members = ids.id(LIST_ONE_OFF).and_then(|id| m.properties.get(id)).is_some_and(|v| match v {
            PropValue::MultiBinary(v) => !v.is_empty(),
            _ => true,
        }) || ids.id(LIST_STREAM).and_then(|id| m.properties.get(id)).is_some();
        let (text, cut) = fit(b.trim_end_matches('\0').replace("\r\n", "\n").as_str(), if list && has_members { room() / 2 } else { room() });
        c.notes = text;
        if cut || rtf_cut {
            notes.push("its notes were longer than a contact's notes can be and are cut".to_string());
        }
    }

    // What the model has no place for, named.
    let anniversary = r.lid_time(ANNIVERSARY_LOCAL).or_else(|| r.p.time(0x3A41)).and_then(day).is_some();
    let picture = m.attachments.iter().any(|a| a.properties.bool(0x7FFF).unwrap_or(false));
    let lost: Vec<&str> = [
        (r.tag(0x3A19).is_some(), "office"),
        (r.tag(0x3A46).is_some(), "profession"),
        (r.tag(0x3A48).is_some(), "spouse"),
        (anniversary, "anniversary"),
        (r.lid(INSTANT_MESSAGING).is_some(), "instant messaging address"),
        (picture, "picture"),
    ]
    .into_iter()
    .filter_map(|(has, name)| has.then_some(name))
    .collect();
    if !lost.is_empty() {
        notes.push(format!("not kept, as contacts here have no place for it yet: {}", lost.join(", ")));
    }

    let mut members = None;
    if list {
        if c.display_name.is_empty() {
            c.display_name = r.lid(LIST_NAME).unwrap_or_default();
        }
        let left = room().saturating_sub(cost(&c.notes)).saturating_sub(8);
        let (text, count) = members_text(pst, m, ids, left, &mut notes)?;
        members = Some(count);
        if !text.is_empty() {
            c.notes = if c.notes.trim().is_empty() { text } else { format!("{}\n\n{text}", c.notes.trim_end()) };
        }
    }
    // A contact needs a name to be edited: from what it has, else a plain one.
    if c.name().is_empty() {
        c.display_name = c.phones.first().map(|p| p.value.clone()).or_else(|| c.websites.first().cloned()).unwrap_or_else(|| "(no name)".to_string());
    }
    Ok(Built { contact: c, notes, members })
}

/// The notes of a distribution list: its members, one to a line (`Name <address>`), and how
/// many there are. Past 15,000 bytes Outlook keeps them in a stream of their own
/// (PidLidDistributionListStream, [MS-OXOCNTC] 2.2.2.2.4), and then the two lists of members are
/// to be ignored: the stream is read when there is one, the two lists only when it can't be.
fn members_text<R: Read + Seek>(
    pst: &mut Pst<R>,
    m: &mailcraft_pst::Message,
    ids: &ContactIds,
    room: usize,
    notes: &mut Vec<String>,
) -> Result<(String, usize), PstError> {
    let stream_id = ids.id(LIST_STREAM).filter(|id| m.properties.get(*id).is_some());
    let stream = match stream_id {
        Some(id) => Some(value(pst, m, id, MAX_MEMBER_BYTES, "list's stream of members", notes)?),
        None => None,
    };
    let mut unreadable = 0usize;
    let mut more = 0usize;
    // Each member as its name and address, when it has an internet address.
    let mut found: Vec<Option<(String, String)>> = Vec::new();
    // Whether members may be missing because the stream couldn't be read, or only in part.
    let mut missing = stream.is_some();
    let read = match &stream {
        Some(Some(PropValue::Binary(b))) => match mailcraft_pst::list_stream(b, STREAM_ENTRIES) {
            // A stream that breaks before its first member is no better than none.
            Ok(s) if s.members.is_empty() && s.damaged > 0 => {
                notes.push("its list's stream of members couldn't be read: it ends before its first member".to_string());
                None
            }
            Ok(s) => Some(s),
            Err(e) => {
                notes.push(format!("its list's stream of members couldn't be read: {e}"));
                None
            }
        },
        Some(Some(_)) => {
            notes.push("its list's stream of members is not a stream and couldn't be read".to_string());
            None
        }
        // (Too large: said by `value`.)
        Some(None) => {
            if stream_id.and_then(|id| m.properties.get(id)).is_some_and(|v| matches!(v, PropValue::Damaged { .. })) {
                notes.push("its list's stream of members is damaged and couldn't be read".to_string());
            }
            None
        }
        None => None,
    };
    if let Some(s) = read {
        more = s.more;
        missing = s.damaged > 0;
        if s.damaged > 0 {
            notes.push(format!("{} member(s) of the list's stream couldn't be read: it ends before them", s.damaged));
        }
        if s.totals_differ {
            notes.push("its list's stream doesn't add up to what its header says; the members were read as they come".to_string());
        }
        found.extend(s.members.iter().map(|e| stream_member(e.entry_id, e.one_off)));
    } else {
        let one_off = match ids.id(LIST_ONE_OFF) {
            Some(id) => value(pst, m, id, MAX_MEMBER_BYTES, "list of members", notes)?,
            None => None,
        };
        let entries = match one_off {
            Some(PropValue::MultiBinary(v)) => v,
            _ => Vec::new(),
        };
        // (OneOffMembers is kept in step with Members, one for one [MS-OXOCNTC] 2.2.2.2.2: a
        // member that is a contact or another list has an entry there with no address, and is
        // counted and named as not kept.)
        let listed = match ids.id(LIST_MEMBERS).and_then(|id| m.properties.get(id)) {
            Some(PropValue::MultiBinary(v)) => v.len(),
            _ => 0,
        }
        .max(entries.len());
        unreadable = listed.saturating_sub(entries.len());
        found.extend(entries.iter().map(|e| one_off_member(e)));
    }
    let mut lines: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // (The line that says members may be missing takes its room first.)
    let room = if missing { room.saturating_sub(cost(MISSING)) } else { room };
    let mut used = cost("Members:\n");
    for member in found {
        let Some((name, address)) = member else {
            unreadable += 1;
            continue;
        };
        if !seen.insert(address.to_lowercase()) {
            continue;
        }
        let line = if name.is_empty() || name == address { address } else { format!("{name} <{address}>") };
        // (Each line, and the line break after it, takes this much of the notes' room.)
        let take = cost(&line).saturating_add(2);
        if lines.len() >= MAX_MEMBERS || used.saturating_add(take) > room {
            more += 1;
            continue;
        }
        used += take;
        lines.push(line);
    }
    if unreadable > 0 {
        notes.push(format!("{unreadable} member(s) of the list name no internet address and are left out"));
    }
    if more > 0 {
        notes.push(format!("{more} member(s) past the limit of {MAX_MEMBERS} are left out"));
    }
    let count = lines.len();
    let text = if !lines.is_empty() {
        format!("Members:\n{}{}", lines.join("\n"), if missing { MISSING } else { "" })
    } else if missing {
        "Members: the list's stream of members couldn't be read.".to_string()
    } else {
        String::new()
    };
    Ok((text, count))
}

/// What a list's notes end with when the stream of its members couldn't be read, or only in part.
const MISSING: &str = "\nSome members may be missing: the list's stream of members couldn't be read whole.";

/// The provider id of a WrappedEntryId ([MS-OXOCNTC] 2.2.2.2.4.1.1).
const WRAPPED: [u8; 16] = [0xC0, 0x91, 0xAD, 0xD3, 0x51, 0x9D, 0xCF, 0x11, 0xA4, 0xA9, 0x00, 0xAA, 0x00, 0x47, 0xFA, 0xA4];

/// The name and the internet address of a member of a list's stream: from its one-off entry id
/// (which a member that is a contact has too), else from its entry id when that is a one-off,
/// bare or wrapped. A contact, a list or a directory entry with no one-off has none.
fn stream_member(entry_id: &[u8], one_off: &[u8]) -> Option<(String, String)> {
    one_off_member(one_off).or_else(|| one_off_member(entry_id)).or_else(|| {
        let wrapped = entry_id.get(..4)? == [0; 4] && entry_id.get(4..20)? == WRAPPED && entry_id.get(20)? & 0x0F == 0;
        if wrapped { one_off_member(entry_id.get(21..)?) } else { None }
    })
}

/// The provider id of a one-off entry id ([MS-OXCDATA] 2.2.5.1).
const ONE_OFF: [u8; 16] = [0x81, 0x2B, 0x1F, 0xA4, 0xBE, 0xA3, 0x10, 0x19, 0x9D, 0x6E, 0x00, 0xDD, 0x01, 0x0F, 0x54, 0x02];

/// The name and the internet address a one-off entry id holds: flags (4), the provider (16), a
/// version (2) and flags (2, 0x8000 when the texts are Unicode), then the display name, the
/// address type and the address, each ending in a NUL.
fn one_off_member(entry: &[u8]) -> Option<(String, String)> {
    if entry.len() > MAX_ENTRY || entry.get(4..20)? != ONE_OFF {
        return None;
    }
    let flags = u16::from_le_bytes(<[u8; 2]>::try_from(entry.get(22..24)?).ok()?);
    let rest = entry.get(24..)?;
    let texts: Vec<String> = if flags & 0x8000 != 0 {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes).collect();
        units.split(|u| *u == 0).take(3).map(String::from_utf16_lossy).collect()
    } else {
        rest.split(|b| *b == 0).take(3).map(|b| mailcraft_charset::decode(b, "windows-1252")).collect()
    };
    let [name, kind, address] = <[String; 3]>::try_from(texts).ok()?;
    let address = one_line(&address)?;
    (kind.eq_ignore_ascii_case("SMTP") && is_address(&address)).then(|| (one_line(&name).unwrap_or_default(), address))
}
