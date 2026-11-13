// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One item of a .pst file as a message of the mailbox (#815): its properties ([MS-OXPROPS],
//! [MS-OXCMSG], [MS-OXOMSG]) read into a [`Message`], its recipients, body and attachments, an
//! attached message written as `message/rfc822`.
//!
//! What the file holds is hostile: every text is decoded within its size, every list is capped,
//! and what an item brings in all (its text and its attachments, with those of the messages
//! attached to it) is within [`MAX_ITEM_BYTES`]. What can't be brought is said in the item's
//! notes, which the import report lists; the item itself still comes in. A body kept as rich
//! text alone comes in as its text, with the rich text as an attachment.

use std::io::{Read, Seek};

use mailcraft_model::{Address, Attachment, FlagStatus, Importance, Message};
use mailcraft_pst::{AttachmentContent, Error as PstError, MessageRef, PropValue, Properties, Pst, RecipientKind};

/// Most bytes one item brings in: its text, its HTML and its attachments, with those of the
/// messages attached to it.
pub const MAX_ITEM_BYTES: u64 = 256 << 20;
/// Most bytes of one body (its text or its HTML) read from the file.
pub const MAX_BODY: u64 = 32 << 20;
/// Most bytes a body kept as rich text opens out to.
const MAX_RTF: usize = 64 << 20;
/// Most bytes of text one import takes from bodies kept as rich text.
pub const RTF_TEXT_BUDGET: u64 = if cfg!(test) { 40 << 20 } else { 256 << 20 };
/// Most memory one import's events and to-dos may take: unlike mail, they aren't moved out of it.
pub const PLAN_TEXT_BUDGET: u64 = if cfg!(test) { 8 << 20 } else { 512 << 20 };
/// Most bytes one import's notes and journal entries may take. They are written with the rest of
/// the mailbox at each save, not as items of their own: ponytail: fine for what a person writes;
/// past this they are left out and named, and they would want records of their own.
pub const KEPT_TEXT_BUDGET: u64 = if cfg!(test) { 2 << 20 } else { 32 << 20 };
/// Most a mailbox may hold of them before an import, as they are written (JSON). One import then
/// adds at most six times its budget (a control character is six bytes written): together they
/// stay within the largest record a mailbox can save.
pub const KEPT_TOTAL_BUDGET: u64 = if cfg!(test) { 4 << 20 } else { 192 << 20 };
/// The name of the attachment that keeps a body's rich text.
const RTF_FILE: &str = "message body.rtf";
/// Most bytes of the transport headers kept.
pub const MAX_HEADERS: usize = 1 << 20;
/// Most characters of a short text (a name, an address, a subject) kept.
const MAX_SHORT: usize = 4096;
/// Most characters of a file name kept.
const MAX_FILE_NAME: usize = 255;
/// Most categories an item keeps.
const MAX_CATEGORIES: usize = 100;
/// Most message ids an item's `References` keeps.
const MAX_REFERENCES: usize = 200;
/// Most notes kept of one item (the rest are counted in the last).
const MAX_NOTES: usize = 20;

/// What a FILETIME (100-nanosecond intervals since 1601) is in seconds since 1970, when it is a
/// date a message can have (after 1970, before the year 10000).
pub fn filetime(ft: u64) -> Option<i64> {
    let seconds = i64::try_from(ft / 10_000_000).ok()?.checked_sub(11_644_473_600)?;
    (seconds > 0 && seconds < 253_402_300_800).then_some(seconds)
}

pub use mailcraft_pst::code_page_label as code_page;

/// How the 8-bit text of one item is read: its own code page, else the file's, else
/// Windows-1252 (what an ANSI file in Western Europe and America holds).
#[derive(Clone, Copy, Debug)]
pub struct Text {
    pub charset: &'static str,
}

impl Text {
    /// The code page of an item (`p`), falling back on `fallback` (the file's).
    pub fn of(p: &Properties, fallback: Option<&'static str>) -> Text {
        let own = [0x3FFD, 0x3FDE].into_iter().filter_map(|id| p.int(id)).find_map(code_page);
        Text { charset: own.or(fallback).unwrap_or("windows-1252") }
    }

    /// 8-bit text as text: in its code page, without the NULs that may end it.
    pub fn decode(&self, bytes: &[u8]) -> String {
        let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i.saturating_add(1));
        mailcraft_charset::decode(bytes.get(..end).unwrap_or_default(), self.charset)
    }

    /// A text property, whole (Unicode, or 8-bit text in the item's code page).
    pub fn long(&self, p: &Properties, id: u16) -> Option<String> {
        match p.get(id)? {
            PropValue::String(s) => Some(s.trim_end_matches('\0').to_string()),
            PropValue::String8(b) => Some(self.decode(b)),
            _ => None,
        }
    }

    /// A short text property (a name, an address): cut to [`MAX_SHORT`] characters, without
    /// control characters, trimmed.
    pub fn short(&self, p: &Properties, id: u16) -> Option<String> {
        let s = self.long(p, id)?;
        let s: String = s.chars().filter(|c| !c.is_control()).take(MAX_SHORT).collect();
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// The values of a multi-valued text property.
    pub fn list(&self, p: &Properties, id: u16) -> Vec<String> {
        match p.get(id) {
            Some(PropValue::MultiString(v)) => v.clone(),
            Some(PropValue::MultiString8(v)) => v.iter().map(|b| self.decode(b)).collect(),
            Some(PropValue::String(s)) => vec![s.clone()],
            Some(PropValue::String8(b)) => vec![self.decode(b)],
            _ => Vec::new(),
        }
    }
}

/// What every item of a file is read with: the ids its named properties have in that file.
#[derive(Clone, Debug, Default)]
pub struct Named {
    /// PidNameKeywords: the categories.
    pub keywords: Option<u16>,
    /// PidLidTaskDueDate: when a flagged item is due.
    pub due: Option<u16>,
    /// The file's own code page, for an item without one.
    pub charset: Option<&'static str>,
    /// The ids of the named properties of contacts.
    pub contact: super::contact::ContactIds,
    /// Bytes of text this import may still take from rich text.
    pub rtf_room: std::cell::Cell<u64>,
    /// Bytes of memory this import's events and to-dos may still take.
    pub plan_room: std::cell::Cell<u64>,
    /// Bytes this import's notes and journal entries may still take.
    pub kept_room: std::cell::Cell<u64>,
    /// The ids of the named properties of calendar items and tasks.
    pub calendar: super::calendar::CalendarIds,
}

/// PSETID_Task, as a .pst file keeps a property set (the first three fields little-endian).
pub const PSETID_TASK: [u8; 16] = [0x03, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

/// Whether an error ends the import rather than leaving one part out: the reader under the file
/// failing, or the file's read allowance spent.
pub fn ends_import(e: &PstError) -> bool {
    matches!(e, PstError::Io(_) | PstError::Cap(mailcraft_pst::Cap::TotalRead))
}

/// One item read into a message, and what of it couldn't be.
pub struct Built {
    pub message: Message,
    pub notes: Vec<String>,
}

/// Bytes counted for each text, list entry and attachment besides what it holds.
const OVERHEAD: u64 = 32;

/// About how much memory `m` holds: every text, list and attachment of it, for the import's
/// batches (a message of a few bytes of body may carry thousands of recipients or headers).
pub fn held(m: &Message) -> u64 {
    let text = |s: &str| (s.len() as u64).saturating_add(OVERHEAD);
    let address = |a: &mailcraft_model::Address| text(&a.name).saturating_add(text(&a.email));
    let addresses = |list: &[mailcraft_model::Address]| list.iter().map(address).fold(0u64, u64::saturating_add);
    let texts = |list: &[String]| list.iter().map(|s| text(s)).fold(0u64, u64::saturating_add);
    [
        text(&m.message_id),
        text(&m.in_reply_to),
        texts(&m.references),
        address(&m.from),
        addresses(&m.reply_to),
        addresses(&m.to),
        addresses(&m.cc),
        addresses(&m.bcc),
        text(&m.subject),
        text(&m.body.text),
        m.body.html.as_deref().map_or(0, text),
        text(&m.hidden_text),
        texts(&m.categories),
        texts(&m.warnings_dismissed),
        m.headers.iter().map(|(k, v)| text(k).saturating_add(text(v))).fold(0u64, u64::saturating_add),
        m.attachments
            .iter()
            .map(|a| text(&a.name).saturating_add(text(&a.mime)).saturating_add(text(&a.content_id)).saturating_add(a.data.len() as u64))
            .fold(0u64, u64::saturating_add),
    ]
    .into_iter()
    .fold(OVERHEAD, u64::saturating_add)
}

/// About how much memory a contact holds: its texts and lists.
pub fn held_contact(c: &mailcraft_model::Contact) -> u64 {
    let text = |s: &str| (s.len() as u64).saturating_add(OVERHEAD);
    let labeled = |l: &[mailcraft_model::Labeled]| l.iter().map(|l| text(&l.label).saturating_add(text(&l.value))).fold(0u64, u64::saturating_add);
    let texts = |l: &[String]| l.iter().map(|s| text(s)).fold(0u64, u64::saturating_add);
    let places = c.addresses.iter().map(|a| (a.one_line().len() as u64).saturating_add(6 * OVERHEAD)).fold(0u64, u64::saturating_add);
    [
        text(&c.display_name),
        text(&c.first),
        text(&c.middle),
        text(&c.last),
        text(&c.nickname),
        text(&c.company),
        text(&c.title),
        text(&c.department),
        text(&c.notes),
        text(&c.imported),
        labeled(&c.emails),
        labeled(&c.phones),
        texts(&c.websites),
        texts(&c.categories),
        places,
    ]
    .into_iter()
    .fold(OVERHEAD, u64::saturating_add)
}

/// What reading one item has left: bytes it may still bring, and its notes.
struct Item<'a> {
    named: &'a Named,
    left: u64,
    notes: Vec<String>,
    more_notes: usize,
}

impl Item<'_> {
    fn note(&mut self, note: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(note);
        } else {
            self.more_notes = self.more_notes.saturating_add(1);
        }
    }

    /// Takes `n` bytes from what the item may bring; `false` (and nothing taken) past it.
    fn take(&mut self, n: u64) -> bool {
        match self.left.checked_sub(n) {
            Some(left) => {
                self.left = left;
                true
            }
            None => false,
        }
    }
}

/// Reads the item `id` (read already as `m`) into a message: never fails for what one item gets
/// wrong, only for what ends the import ([`ends_import`]).
pub fn build<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, named: &Named) -> Result<Built, PstError> {
    let mut item = Item { named, left: MAX_ITEM_BYTES, notes: Vec::new(), more_notes: 0 };
    let message = message_of(pst, m, &mut item, 0)?;
    if item.more_notes > 0 {
        item.notes.push(format!("and {} more notes", item.more_notes));
    }
    Ok(Built { message, notes: item.notes })
}

fn message_of<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, item: &mut Item, depth: usize) -> Result<Message, PstError> {
    let p = &m.properties;
    let text = Text::of(p, item.named.charset);
    let mut out = Message::default();
    let class = m.class.clone().unwrap_or_default();

    // What the reader couldn't read of it.
    for part in &m.left_out {
        item.note(format!("{}: {}", part_name(&part.part), part.reason));
    }

    // The headers it was sent with, when it kept them.
    let headers = match p.get(0x007D) {
        Some(PropValue::Large { .. }) => match pst.read_property(&m.id, 0x007D, MAX_HEADERS as u64) {
            Ok(Some(v)) => value_text(&v, &text),
            Ok(None) => None,
            Err(e) if ends_import(&e) => return Err(e),
            Err(e) => {
                item.note(format!("its Internet headers: {e}"));
                None
            }
        },
        _ => text.long(p, 0x007D),
    };
    let headers = headers.filter(|h| !h.trim().is_empty());
    if let Some(h) = &headers {
        let cut = cut_at(h, MAX_HEADERS);
        if cut.len() < h.len() {
            item.note("its Internet headers were longer than the limit and are cut".to_string());
        }
        let parsed = mailcraft_mime_model::parse(format!("{}\r\n\r\n", cut.trim_end()).as_bytes());
        out.from = parsed.from;
        out.reply_to = parsed.reply_to;
        out.to = parsed.to;
        out.cc = parsed.cc;
        out.bcc = parsed.bcc;
        out.subject = parsed.subject;
        out.date = parsed.date;
        out.message_id = parsed.message_id;
        out.in_reply_to = parsed.in_reply_to;
        out.references = parsed.references;
        out.importance = parsed.importance;
        out.categories = parsed.categories;
        out.headers = parsed.headers;
    }

    // Who it is from: who it was sent for, else who sent it.
    let from = person(&text, p, [0x0042, 0x5D02, 0x0065]).or_else(|| person(&text, p, [0x0C1A, 0x5D01, 0x0C1F]));
    let sender = person(&text, p, [0x0C1A, 0x5D01, 0x0C1F]);
    if out.from.email.is_empty()
        && out.from.name.is_empty()
        && let Some(from) = from.clone()
    {
        out.from = from;
    }
    if headers.is_none()
        && let (Some(sender), Some(from)) = (&sender, &from)
        && !sender.email.is_empty()
        && !sender.email.eq_ignore_ascii_case(&from.email)
    {
        out.headers.push(("Sender".to_string(), mailbox_text(sender)));
    }

    // Recipients: To and Cc from the headers when they had them; Bcc, which the headers never
    // have, from the recipient table.
    let (mut to, mut cc, mut bcc) = (Vec::new(), Vec::new(), Vec::new());
    for r in &m.recipients {
        let rp = &r.properties;
        let rtext = Text { charset: text.charset };
        let Some(a) = person(&rtext, rp, [0x3001, 0x39FE, 0x3003]) else { continue };
        match r.kind() {
            RecipientKind::To => to.push(a),
            RecipientKind::Cc => cc.push(a),
            RecipientKind::Bcc => bcc.push(a),
            RecipientKind::Originator | RecipientKind::Other(_) => {}
        }
    }
    if out.to.is_empty() && out.cc.is_empty() {
        out.to = to;
        out.cc = cc;
    }
    if out.bcc.is_empty() {
        out.bcc = bcc;
    }

    // Subject, ids, dates.
    if out.subject.is_empty() {
        out.subject = text.short(p, 0x0037).unwrap_or_default();
    }
    if out.message_id.is_empty() {
        out.message_id = text.short(p, 0x1035).and_then(|v| ids(&v).into_iter().next()).unwrap_or_default();
    }
    if out.in_reply_to.is_empty() {
        out.in_reply_to = text.short(p, 0x1042).and_then(|v| ids(&v).into_iter().next()).unwrap_or_default();
    }
    if out.references.is_empty() {
        out.references = text.long(p, 0x1039).map(|v| ids(&v)).unwrap_or_default();
    }
    let sent = p.time(0x0039).and_then(filetime);
    let delivered = p.time(0x0E06).and_then(filetime);
    if out.date == 0 {
        out.date = sent.or(delivered).unwrap_or(0);
    }
    out.received = delivered.or(sent).unwrap_or(out.date);

    // State: read, flag and when it is due, importance, sensitivity, categories, answered.
    let flags = p.int(0x0E07).unwrap_or(0);
    out.read = flags & 0x01 != 0;
    out.flag = match p.int(0x1090) {
        Some(2) => FlagStatus::Flagged,
        Some(1) => FlagStatus::Complete,
        _ => FlagStatus::None,
    };
    if out.flag == FlagStatus::Flagged {
        out.flag_due = item.named.due.and_then(|id| p.time(id)).and_then(filetime);
    }
    if let Some(i) = p.int(0x0017) {
        out.importance = match i {
            0 => Importance::Low,
            2 => Importance::High,
            _ => Importance::Normal,
        };
    }
    let sensitivity = match p.int(0x0036) {
        Some(1) => Some("Personal"),
        Some(2) => Some("Private"),
        Some(3) => Some("Company-Confidential"),
        _ => None,
    };
    if let Some(s) = sensitivity
        && !out.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("sensitivity"))
    {
        out.headers.push(("Sensitivity".to_string(), s.to_string()));
    }
    if let Some(id) = item.named.keywords {
        let own: Vec<String> = text.list(p, id).iter().filter_map(|c| mailcraft_model::clean_name(c)).take(MAX_CATEGORIES).collect();
        if !own.is_empty() {
            out.categories = own;
        }
    }
    out.categories.truncate(MAX_CATEGORIES);
    match p.int(0x1081) {
        Some(102 | 103) => out.answered = true,
        Some(104) => out.forwarded = true,
        _ => {}
    }

    // The body.
    body(pst, m, &text, item, &mut out)?;

    // Attachments, and the pictures the HTML shows by content id (looked for once).
    let cids = mailcraft_model::shown::shown_ids(out.body.html.as_deref().unwrap_or(""));
    for a in &m.attachments {
        attachment(pst, a, &text, item, depth, &cids, &mut out)?;
    }
    if class.to_ascii_lowercase().starts_with("ipm.note.smime") {
        item.note("signed or encrypted (S/MIME): kept as it was, can't be read yet".to_string());
    }
    Ok(out)
}

/// A text value, from what `read_property` gave.
fn value_text(v: &PropValue, text: &Text) -> Option<String> {
    match v {
        PropValue::String(s) => Some(s.clone()),
        PropValue::String8(b) => Some(text.decode(b)),
        _ => None,
    }
}

/// `s` cut to at most `max` bytes, at a character boundary.
fn cut_at(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default()
}

/// A person from their name and addresses (`[name, SMTP address, address]`): the SMTP address,
/// else the address when it is one (an Exchange address is a name in a directory, not one).
pub(super) fn person(text: &Text, p: &Properties, [name, smtp, address]: [u16; 3]) -> Option<Address> {
    let name = text.short(p, name).unwrap_or_default();
    let email = text.short(p, smtp).filter(|e| e.contains('@')).or_else(|| text.short(p, address).filter(|e| e.contains('@'))).unwrap_or_default();
    (!name.is_empty() || !email.is_empty()).then(|| Address::new(name, email))
}

/// `Name <address>` for a header.
fn mailbox_text(a: &Address) -> String {
    if a.name.is_empty() { format!("<{}>", a.email) } else { format!("\"{}\" <{}>", a.name.replace(['"', '\\'], ""), a.email) }
}

/// The message ids of a `Message-ID`, `In-Reply-To` or `References` value, without brackets.
fn ids(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let after = rest.get(open.saturating_add(1)..).unwrap_or_default();
        let Some(close) = after.find('>') else { break };
        let id: String = after.get(..close).unwrap_or_default().trim().chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect();
        if !id.is_empty() && out.len() < MAX_REFERENCES {
            out.push(id);
        }
        rest = after.get(close.saturating_add(1)..).unwrap_or_default();
    }
    if out.is_empty() && !value.contains('<') {
        out.extend(value.split(|c: char| c.is_whitespace() || c == ',').filter(|t| t.contains('@')).take(MAX_REFERENCES).map(str::to_string));
    }
    out
}

/// A body property (`id`) whole, read on its own when it was too large to come with the rest,
/// within [`MAX_BODY`] and what the item may still bring.
fn body_value<R: Read + Seek>(
    pst: &mut Pst<R>,
    m: &mailcraft_pst::Message,
    id: u16,
    item: &mut Item,
    what: &str,
) -> Result<Option<PropValue>, PstError> {
    let v = match m.properties.get(id) {
        None => return Ok(None),
        Some(PropValue::Large { size }) => {
            let max = MAX_BODY.min(item.left);
            if *size > max {
                item.note(format!("its {what} ({}) is larger than the limit and is left out", mailcraft_model::size_label(*size)));
                return Ok(None);
            }
            match pst.read_property(&m.id, id, max) {
                Ok(Some(PropValue::Damaged { .. })) => {
                    item.note(format!("its {what} is damaged in the file and is left out"));
                    return Ok(None);
                }
                Ok(v) => v,
                Err(e) if ends_import(&e) => return Err(e),
                Err(e) => {
                    item.note(format!("its {what}: {e}"));
                    return Ok(None);
                }
            }
        }
        Some(PropValue::Damaged { .. }) => return Ok(None),
        Some(v) => Some(v.clone()),
    };
    let size = match &v {
        Some(PropValue::String(s)) => s.len(),
        Some(PropValue::String8(b) | PropValue::Binary(b)) => b.len(),
        _ => 0,
    } as u64;
    if !item.take(size) {
        item.note(format!("its {what} is larger than what one item may bring and is left out"));
        return Ok(None);
    }
    Ok(v)
}

/// The charset an HTML body says it is in (`<meta charset>` or `charset=`), from its first bytes.
fn html_charset(bytes: &[u8]) -> Option<String> {
    let head = bytes.get(..bytes.len().min(2048)).unwrap_or_default();
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let at = lower.windows(8).position(|w| w == b"charset=")?.saturating_add(8);
    let rest = lower.get(at..)?;
    let rest = rest.strip_prefix(b"\"").or_else(|| rest.strip_prefix(b"'")).unwrap_or(rest);
    let end = rest.iter().position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))).unwrap_or(rest.len());
    let label = std::str::from_utf8(rest.get(..end)?).ok()?;
    (!label.is_empty()).then(|| label.to_string())
}

fn body<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, text: &Text, item: &mut Item, out: &mut Message) -> Result<(), PstError> {
    let p = &m.properties;
    let html = match body_value(pst, m, 0x1013, item, "HTML")? {
        Some(PropValue::Binary(b)) => {
            let label = p.int(0x3FDE).and_then(code_page).map(str::to_string).or_else(|| html_charset(&b));
            Some(match label {
                Some(label) => mailcraft_charset::decode(&b, &label),
                None => mailcraft_charset::guess(&b),
            })
        }
        Some(v) => value_text(&v, text),
        None => None,
    }
    .filter(|h| !h.trim().is_empty());
    let plain = body_value(pst, m, 0x1000, item, "text")?.and_then(|v| value_text(&v, text));
    let plain = plain.map(|t| t.trim_end_matches('\0').replace("\r\n", "\n"));
    let (from_html, hidden) = html.as_deref().map(mailcraft_html::text_and_hidden).unzip();
    out.hidden_text = cut_at(&hidden.unwrap_or_default(), mailcraft_model::MAX_HIDDEN_TEXT).to_string();
    out.body.text = match (plain, from_html) {
        (Some(t), Some(h)) if t.trim().is_empty() => h,
        (Some(t), _) => t,
        (None, Some(h)) => h,
        (None, None) => String::new(),
    };
    // A body that is rich text alone (no HTML, no text): its text. One with a text as well (as
    // Outlook keeps most) has its text, and its rich text is let be.
    let mut html = html;
    if html.is_none() && out.body.text.trim().is_empty() {
        match body_value(pst, m, 0x1009, item, "rich text")? {
            Some(PropValue::Binary(stored)) => html = rich_text(stored, item, out),
            Some(_) => item.note("its rich text is not of the kind rich text is kept as and is left out".to_string()),
            None => {}
        }
    }
    out.body.html = html;
    Ok(())
}

/// The categories an item keeps (PidNameKeywords).
pub(super) fn own_categories(p: &Properties, named: &Named, text: &Text) -> Vec<String> {
    named.keywords.map_or_else(Vec::new, |id| text.list(p, id).iter().filter_map(|c| mailcraft_model::clean_name(c)).take(MAX_CATEGORIES).collect())
}

/// The text of an item that isn't mail (an appointment's notes, a task's): its plain text, else
/// the text of its rich text, else of its HTML, within `max` characters; and what couldn't be.
pub(super) fn plain_body<R: Read + Seek>(
    pst: &mut Pst<R>,
    m: &mailcraft_pst::Message,
    named: &Named,
    max: usize,
) -> Result<(String, Vec<String>), PstError> {
    let text = Text::of(&m.properties, named.charset);
    let mut item = Item { named, left: MAX_BODY.saturating_mul(2), notes: Vec::new(), more_notes: 0 };
    let mut out = body_value(pst, m, 0x1000, &mut item, "text")?
        .and_then(|v| value_text(&v, &text))
        .map(|t| t.trim_end_matches('\0').replace("\r\n", "\n"))
        .unwrap_or_default();
    if out.trim().is_empty()
        && let Some(PropValue::Binary(stored)) = body_value(pst, m, 0x1009, &mut item, "rich text")?
    {
        let room = named.rtf_room.get();
        let most = usize::try_from(room).unwrap_or(usize::MAX).min(max.saturating_add(8));
        match mailcraft_pst::decompress_rtf(&stored, MAX_RTF).and_then(|raw| mailcraft_pst::rtf_text(&raw, most)) {
            Ok((t, cut)) => {
                named.rtf_room.set(room.saturating_sub(t.len() as u64));
                out = t;
                if cut && most <= max {
                    item.note("its notes (rich text) are cut: this import has opened as much rich text as one import may".to_string());
                }
            }
            Err(e) => item.note(format!("its notes (rich text) could not be read: {e}")),
        }
    }
    if out.trim().is_empty()
        && let Some(PropValue::Binary(b)) = body_value(pst, m, 0x1013, &mut item, "HTML")?
    {
        out = mailcraft_html::text_and_hidden(&mailcraft_charset::guess(&b)).0;
    }
    let kept = cut_at(&out, max);
    if kept.len() < out.len() {
        item.note("its notes were longer than the limit and are cut".to_string());
    }
    let kept = kept.to_string();
    Ok((kept, item.notes))
}

/// A body kept as rich text alone ([MS-OXRTFCP]): its text becomes the body and the RTF is kept
/// as an attachment, for the fonts and pictures the text leaves behind. What can't be opened
/// stays as it is in the file, and is named; nothing is lost either way. Compression lets a few
/// bytes open out to many and a body stays in memory, so an import's text from rich text has a
/// budget of its own ([`Named::rtf_room`]) beside the item's.
fn rich_text(stored: Vec<u8>, item: &mut Item, out: &mut Message) -> Option<String> {
    let room = item.named.rtf_room.get();
    if room == 0 {
        keep_stored(stored, "this import has opened as much rich text as one import may", item, out);
        return None;
    }
    let mut raw = match mailcraft_pst::decompress_rtf(&stored, MAX_RTF.min(usize::try_from(item.left).unwrap_or(usize::MAX))) {
        Ok(raw) => raw,
        Err(e) => {
            keep_stored(stored, &e.to_string(), item, out);
            return None;
        }
    };
    // What was stored is let go of: what is kept instead is counted.
    item.left = item.left.saturating_add(stored.len() as u64).saturating_sub(raw.len() as u64);
    drop(stored);
    raw.shrink_to_fit();
    let most = MAX_BODY.min(item.left).min(room);
    // HTML kept in rich text comes in as the HTML it was, read when shown as any other's. (Cut,
    // it wouldn't be the HTML it was: its text and the rich text are kept, as for any other.)
    if let Ok(Some((html, false))) = mailcraft_pst::rtf_html(&raw, usize::try_from(most).unwrap_or(usize::MAX))
        && !html.trim().is_empty()
    {
        // Its text and what it hides are the HTML's, as for any other's: the rich text's own
        // text holds what the HTML hides and what only the rich text shows.
        let (whole, hidden) = mailcraft_html::text_and_hidden(&html);
        let text = cut_at(&whole, usize::try_from(most.saturating_sub(html.len() as u64)).unwrap_or(usize::MAX)).to_string();
        if text.len() < whole.len() {
            item.note("its rich text was longer than a body can be and its text is cut".to_string());
        }
        out.hidden_text = cut_at(&hidden, mailcraft_model::MAX_HIDDEN_TEXT).to_string();
        let size = (html.len() as u64).saturating_add(text.len() as u64).saturating_add(out.hidden_text.len() as u64);
        item.left = item.left.saturating_add(raw.len() as u64).saturating_sub(size);
        item.named.rtf_room.set(room.saturating_sub(size));
        out.body.text = text;
        return Some(html);
    }
    let read = mailcraft_pst::rtf_text(&raw, usize::try_from(most).unwrap_or(usize::MAX));
    match read {
        Ok((text, cut)) => {
            if text.trim().is_empty() && !cut && !raw.windows(5).any(|w| w == b"\\pict") && !raw.windows(7).any(|w| w == b"\\object") {
                // Nothing in it: no text, no picture. It is no more than an empty body.
                item.left = item.left.saturating_add(raw.len() as u64);
                return None;
            }
            let size = text.len() as u64;
            item.left = item.left.saturating_sub(size);
            item.named.rtf_room.set(room.saturating_sub(size));
            if text.trim().is_empty() {
                item.note(format!("its rich text has no text of its own: it is kept as the attachment {RTF_FILE}"));
            } else {
                item.note(format!("its body is rich text alone: its text is brought in without its formatting, and the rich text is kept as the attachment {RTF_FILE}"));
            }
            out.body.text = text;
            if cut {
                item.note("its rich text was longer than a body can be and its text is cut".to_string());
            }
        }
        Err(e) => item.note(format!("its rich text could not be read ({e}) and is kept as the attachment {RTF_FILE}")),
    }
    out.attachments.push(Attachment::new(RTF_FILE.to_string(), "application/rtf", raw));
    None
}

/// Rich text that can't be opened, kept as it is in the file.
fn keep_stored(stored: Vec<u8>, why: &str, item: &mut Item, out: &mut Message) {
    let name = "message body (compressed rich text).bin";
    out.attachments.push(Attachment::new(name.to_string(), "application/octet-stream", stored));
    item.note(format!("its rich text could not be read ({why}) and is kept as the attachment {name}"));
}

/// A file name from the file: its last part, without control characters, cut to size.
fn file_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let s: String = last.chars().filter(|c| !c.is_control()).take(MAX_FILE_NAME).collect();
    s.trim().to_string()
}

fn attachment<R: Read + Seek>(
    pst: &mut Pst<R>,
    a: &mailcraft_pst::Attachment,
    text: &Text,
    item: &mut Item,
    depth: usize,
    cids: &mailcraft_model::shown::ShownIds,
    out: &mut Message,
) -> Result<(), PstError> {
    let p = &a.properties;
    let name = text.long(p, 0x3707).or_else(|| text.long(p, 0x3704)).or_else(|| text.long(p, 0x3001)).map(|n| file_name(&n)).unwrap_or_default();
    let mime = text.short(p, 0x370E).filter(|m| m.contains('/') && m.len() < 128).unwrap_or_else(|| "application/octet-stream".to_string());
    let content_id = text.short(p, 0x3712).map(|c| c.trim_matches(['<', '>']).to_string()).unwrap_or_default();
    let hidden = p.bool(0x7FFE).unwrap_or(false) || p.int(0x3714).is_some_and(|f| f & 0x4 != 0);
    let shown = if name.is_empty() { "an attachment".to_string() } else { format!("the attachment {name}") };
    match (&a.content, a.method) {
        (AttachmentContent::Message(inner), _) => {
            let read = match pst.message(inner) {
                Ok(m) => m,
                Err(e) if ends_import(&e) => return Err(e),
                Err(e) => {
                    item.note(format!("{shown}, an attached message: {e}"));
                    return Ok(());
                }
            };
            // What the attached message reads is charged while it is read, then given back: the
            // message as a file is what this item brings, and is charged once.
            let left = item.left;
            let attached = message_of(pst, &read, item, depth.saturating_add(1))?;
            item.left = left;
            let bytes = mailcraft_mime_model::write(&attached, &mailcraft_mime_model::WriteOptions { include_bcc: false, offset_minutes: 0 });
            if !item.take(bytes.len() as u64) {
                item.note(format!("{shown}, an attached message, is larger than what one item may bring and is left out"));
                return Ok(());
            }
            let stem = if attached.subject.trim().is_empty() { name.trim_end_matches(".msg").to_string() } else { attached.subject.clone() };
            let file = crate::opening::export_name_or(&stem, "eml", "attached message");
            out.attachments.push(Attachment::new(file, "message/rfc822", bytes));
        }
        (AttachmentContent::MessageTooDeep, _) => item.note(format!("{shown}, an attached message, is nested deeper than the limit and is left out")),
        (AttachmentContent::Data { size } | AttachmentContent::Object { size }, _) => {
            let object = matches!(a.content, AttachmentContent::Object { .. }) || a.method == Some(6);
            if !item.take(*size) {
                item.note(format!("{shown} ({}) is larger than what one item may bring and is left out", mailcraft_model::size_label(*size)));
                return Ok(());
            }
            let data = match pst.attachment_bytes(&a.id) {
                Ok(d) => d,
                Err(e) if ends_import(&e) => return Err(e),
                Err(e) => {
                    item.note(format!("{shown}: {e}"));
                    return Ok(());
                }
            };
            let (name, mime) = if object {
                ("embedded object.bin".to_string(), "application/octet-stream".to_string())
            } else if name.is_empty() {
                ("attachment.bin".to_string(), mime)
            } else {
                (name, mime)
            };
            let mut file = Attachment::new(name, mime, data);
            if !content_id.is_empty() {
                // Shown in place only as a picture an `<img>` asks for (or the file hides), by its
                // bytes: a file of any other kind is listed, whatever the body or the file says.
                file.inline = mailcraft_model::shown::is_picture(&file.data) && (hidden || cids.shows(&content_id));
                file.content_id = content_id;
            }
            out.attachments.push(file);
        }
        (AttachmentContent::None, Some(2..=4 | 7)) => {
            let path = text.short(p, 0x370D).or_else(|| text.short(p, 0x3708)).unwrap_or_else(|| name.clone());
            item.note(format!("a linked file, {path}, not fetched"));
        }
        (AttachmentContent::None, _) => {
            // An attachment with nothing in it: kept, so that its name is not lost.
            let name = if name.is_empty() { "attachment.bin".to_string() } else { name };
            out.attachments.push(Attachment::new(name, mime, Vec::new()));
        }
    }
    Ok(())
}

/// A part of an item the reader left out, for its note.
pub(super) fn part_name(part: &mailcraft_pst::Part) -> String {
    use mailcraft_pst::Part;
    match part {
        Part::Recipients { count } => format!("{count} recipients past the limit"),
        Part::Recipient { row } => format!("recipient {row}"),
        Part::RecipientTable => "its recipients".to_string(),
        Part::Attachments { count } => format!("{count} attachments past the limit"),
        Part::Attachment { id } => format!("attachment {:#x}", id.0),
        Part::AttachmentTable => "its attachments".to_string(),
        Part::Property { id } => format!("property {id:#06x}"),
        Part::RecipientProperty { row, id } => format!("property {id:#06x} of recipient {row}"),
        Part::AttachmentProperty { attachment, id } => format!("property {id:#06x} of attachment {:#x}", attachment.0),
    }
}

/// The ref of an item in a folder.
pub fn item_ref(nid: u32) -> MessageRef {
    MessageRef::item(mailcraft_pst::Nid(nid))
}
