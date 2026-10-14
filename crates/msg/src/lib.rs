//! MailCraft Outlook `.msg` files: a message saved from Outlook, or dragged out of it.
//!
//! [`parse()`] turns the bytes of a `.msg` file into the same [`mailcraft_model::Message`] an
//! `.eml` file becomes. Written from Microsoft's public open specifications only: [MS-CFB] (the
//! Compound File Binary format the file is), [MS-OXMSG] (how a message lies in it), [MS-OXPROPS]
//! (the property tags), [MS-OXCMSG] (what the properties mean), [MS-OXRTFCP] (compressed RTF) and
//! [MS-OXRTFEX] (HTML encapsulated in RTF).
//!
//! A `.msg` file is hostile input like any message: every size, chain, table, count and nesting
//! is capped, nothing panics, and the work is bounded by the size of the file. Nothing in it is
//! run or fetched: an attachment by reference (a link to a file elsewhere) is left out.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod cfb;
mod codepage;
mod rtf;

#[cfg(any(test, feature = "writer"))]
pub mod writer;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, FlagStatus, Importance, Message};

use crate::cfb::{Cfb, Kind};

pub use cfb::SIGNATURE;

/// Why a file couldn't be read as an Outlook message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MsgError {
    #[error("this isn't an Outlook message file")]
    NotCompound,
    #[error("this is an Office file, but not an Outlook message")]
    NotMessage,
    #[error("this Outlook message file is damaged ({0})")]
    Damaged(String),
}

/// Whether `bytes` start like a compound file (which every `.msg` file is; so are old Word and
/// Excel files, which [`parse()`] then refuses).
pub fn is_compound(bytes: &[u8]) -> bool {
    bytes.get(..8) == Some(&SIGNATURE[..])
}

/// Deepest nesting of attached messages opened; one deeper is left out.
const MAX_DEPTH: usize = 8;

/// Most attachments read from one file (attached messages' own included).
const MAX_ATTACHMENTS: usize = 1000;

/// Most bytes of attachments kept from one file.
const MAX_ATTACHMENT_BYTES: usize = 384 << 20;

/// Most recipients read from one message.
const MAX_RECIPIENTS: usize = 10_000;

/// Most attachment and recipient storages opened in one file.
const MAX_OPENED: usize = 50_000;

// Property ids ([MS-OXPROPS]).
const SUBJECT: u16 = 0x0037;
const SUBJECT_PREFIX: u16 = 0x003D;
const NORMALIZED_SUBJECT: u16 = 0x0E1D;
const IMPORTANCE: u16 = 0x0017;
const SENT_REPRESENTING_NAME: u16 = 0x0042;
const SENT_REPRESENTING_ADDRESS_TYPE: u16 = 0x0064;
const SENT_REPRESENTING_EMAIL: u16 = 0x0065;
const SENT_REPRESENTING_SMTP: u16 = 0x5D02;
const TRANSPORT_HEADERS: u16 = 0x007D;
const SENDER_NAME: u16 = 0x0C1A;
const SENDER_ADDRESS_TYPE: u16 = 0x0C1E;
const SENDER_EMAIL: u16 = 0x0C1F;
const SENDER_SMTP: u16 = 0x5D01;
const CLIENT_SUBMIT_TIME: u16 = 0x0039;
const MESSAGE_DELIVERY_TIME: u16 = 0x0E06;
const CREATION_TIME: u16 = 0x3007;
const MESSAGE_FLAGS: u16 = 0x0E07;
const INTERNET_MESSAGE_ID: u16 = 0x1035;
const INTERNET_REFERENCES: u16 = 0x1039;
const IN_REPLY_TO: u16 = 0x1042;
const LAST_VERB_EXECUTED: u16 = 0x1081;
const FLAG_STATUS: u16 = 0x1090;
const BODY: u16 = 0x1000;
const RTF_COMPRESSED: u16 = 0x1009;
const HTML: u16 = 0x1013;
const INTERNET_CODEPAGE: u16 = 0x3FDE;
const MESSAGE_CODEPAGE: u16 = 0x3FFD;
const RECIPIENT_TYPE: u16 = 0x0C15;
const DISPLAY_NAME: u16 = 0x3001;
const ADDRESS_TYPE: u16 = 0x3002;
const EMAIL_ADDRESS: u16 = 0x3003;
const SMTP_ADDRESS: u16 = 0x39FE;
const RECIPIENT_DISPLAY_NAME: u16 = 0x5FF6;
const ATTACH_DATA: u16 = 0x3701;
const ATTACH_FILENAME: u16 = 0x3704;
const ATTACH_METHOD: u16 = 0x3705;
const ATTACH_LONG_FILENAME: u16 = 0x3707;
const ATTACH_MIME_TAG: u16 = 0x370E;
const ATTACH_CONTENT_ID: u16 = 0x3712;
const ATTACH_FLAGS: u16 = 0x3714;
const ATTACHMENT_HIDDEN: u16 = 0x7FFE;

// Property types.
const PT_I2: u16 = 0x0002;
const PT_LONG: u16 = 0x0003;
const PT_BOOLEAN: u16 = 0x000B;
const PT_SYSTIME: u16 = 0x0040;
const PT_STRING8: u16 = 0x001E;
const PT_UNICODE: u16 = 0x001F;
const PT_BINARY: u16 = 0x0102;
const PT_OBJECT: u16 = 0x000D;

const PROPERTIES: &str = "__PROPERTIES_VERSION1.0";
const RECIPIENT_PREFIX: &str = "__RECIP_VERSION1.0_#";
const ATTACHMENT_PREFIX: &str = "__ATTACH_VERSION1.0_#";

/// How long the header of a property stream is ([MS-OXMSG] 2.4.1).
#[derive(Clone, Copy)]
enum Header {
    /// The message of the file itself.
    TopMessage = 32,
    /// A message attached to another.
    Embedded = 24,
    /// A recipient or an attachment.
    Part = 8,
}

/// Read an Outlook `.msg` file.
///
/// Sets `subject`, `from` (the sender's SMTP address; for an address only an Exchange server
/// knows, the transport headers' `From`, else the name alone), `to`, `cc`, `bcc`, `date` (when it
/// was sent), `received` (when it arrived), `message_id`, `in_reply_to`, `references`,
/// `importance`, `read`, `flag`, `answered`, `forwarded`, `body` (text, and HTML when the message
/// has it, from the HTML property or from the RTF body that encapsulates it), `attachments` (an
/// attached message as `message/rfc822`) and, from the transport headers when the file has them,
/// `reply_to`, `categories` and the header fields MailCraft doesn't model. Leaves `id`,
/// `account`, `folder` and `uid` at 0.
pub fn parse(bytes: &[u8]) -> Result<Message, MsgError> {
    let cfb = Cfb::open(bytes)?;
    let mut budget = Budget { attachments: 0, bytes: 0, opened: 0 };
    read_message(&cfb, cfb.root(), Header::TopMessage, 0, &mut budget).ok_or(MsgError::NotMessage)
}

/// What one file may still cost, attached messages included.
struct Budget {
    attachments: usize,
    bytes: usize,
    /// Attachment and recipient storages opened (whether or not anything came of them).
    opened: usize,
}

/// A message, recipient or attachment object: its fixed-size properties, and its storage's
/// streams for the rest.
struct Object<'c, 'a> {
    cfb: &'c Cfb<'a>,
    children: HashMap<String, usize>,
    fixed: HashMap<u32, [u8; 8]>,
    codepage: u32,
}

impl<'c, 'a> Object<'c, 'a> {
    fn open(cfb: &'c Cfb<'a>, storage: usize, header: Header, codepage: u32) -> Option<Object<'c, 'a>> {
        let children = cfb.children(storage);
        let props = cfb.stream(*children.get(PROPERTIES)?);
        let mut fixed = HashMap::new();
        for entry in props.get(header as usize..).unwrap_or(&[]).as_chunks::<16>().0 {
            let [t0, t1, t2, t3, _, _, _, _, value @ ..] = *entry;
            fixed.insert(u32::from_le_bytes([t0, t1, t2, t3]), value);
        }
        let mut object = Object { cfb, children, fixed, codepage };
        if let Some(cp) = object.int(MESSAGE_CODEPAGE).and_then(|c| u32::try_from(c).ok()).filter(|c| codepage::label(*c).is_some()) {
            object.codepage = cp;
        }
        Some(object)
    }

    fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let id = *self.children.get(name)?;
        (self.cfb.entry(id)?.kind == Kind::Stream).then(|| self.cfb.stream(id))
    }

    fn value(&self, id: u16, ty: u16) -> Option<Vec<u8>> {
        self.stream(&format!("__SUBSTG1.0_{id:04X}{ty:04X}"))
    }

    fn string(&self, id: u16) -> Option<String> {
        let s = if let Some(b) = self.value(id, PT_UNICODE) {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect();
            String::from_utf16_lossy(&units)
        } else {
            codepage::decode(&self.value(id, PT_STRING8)?, self.codepage)
        };
        Some(s.trim_end_matches('\0').to_string())
    }

    /// A string that has something in it.
    fn text(&self, id: u16) -> Option<String> {
        self.string(id).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    fn binary(&self, id: u16) -> Option<Vec<u8>> {
        self.value(id, PT_BINARY)
    }

    fn fixed(&self, id: u16, ty: u16) -> Option<&[u8; 8]> {
        self.fixed.get(&((u32::from(id) << 16) | u32::from(ty)))
    }

    fn int(&self, id: u16) -> Option<i32> {
        if let Some(v) = self.fixed(id, PT_LONG) {
            return Some(i32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        }
        self.fixed(id, PT_I2).map(|v| i32::from(i16::from_le_bytes([v[0], v[1]])))
    }

    fn bool(&self, id: u16) -> Option<bool> {
        self.fixed(id, PT_BOOLEAN).map(|v| v[0] != 0 || v[1] != 0)
    }

    /// A time as seconds since 1970 (from a FILETIME: 100 ns since 1601).
    fn time(&self, id: u16) -> Option<i64> {
        let ft = i64::from_le_bytes(*self.fixed(id, PT_SYSTIME)?);
        (ft > 0).then(|| ft / 10_000_000 - 11_644_473_600)
    }

    /// The storages whose names start with `prefix`, in the order of their numbers.
    fn parts(&self, prefix: &str) -> Vec<usize> {
        let mut parts: Vec<(&String, usize)> = self
            .children
            .iter()
            .filter(|(name, id)| name.starts_with(prefix) && self.cfb.entry(**id).is_some_and(|e| e.kind == Kind::Storage))
            .map(|(name, id)| (name, *id))
            .collect();
        parts.sort();
        parts.into_iter().map(|(_, id)| id).collect()
    }
}

/// An SMTP address, not an Exchange one (`/O=EXAMPLE/OU=...`).
fn smtp(address: Option<String>) -> Option<String> {
    address.filter(|a| a.contains('@') && !a.starts_with('/') && !a.contains(char::is_whitespace))
}

/// The SMTP address of whoever an object names, from its SMTP property, else its address when
/// its type is SMTP.
fn smtp_of(o: &Object, smtp_id: u16, type_id: u16, address_id: u16) -> Option<String> {
    smtp(o.text(smtp_id)).or_else(|| {
        let ty = o.text(type_id).unwrap_or_default();
        if ty.eq_ignore_ascii_case("SMTP") || ty.is_empty() { smtp(o.text(address_id)) } else { None }
    })
}

fn read_message(cfb: &Cfb, storage: usize, header: Header, depth: usize, budget: &mut Budget) -> Option<Message> {
    let mut o = Object::open(cfb, storage, header, 1252)?;
    let internet_cp = o.int(INTERNET_CODEPAGE).and_then(|c| u32::try_from(c).ok()).filter(|c| codepage::label(*c).is_some());
    if o.int(MESSAGE_CODEPAGE).is_none()
        && let Some(cp) = internet_cp
    {
        o.codepage = cp;
    }
    let mut m = Message {
        subject: o
            .string(SUBJECT)
            .or_else(|| o.string(NORMALIZED_SUBJECT).map(|n| format!("{}{n}", o.string(SUBJECT_PREFIX).unwrap_or_default())))
            .unwrap_or_default()
            .trim()
            .to_string(),
        ..Message::default()
    };

    let name = o.text(SENDER_NAME).or_else(|| o.text(SENT_REPRESENTING_NAME)).unwrap_or_default();
    let email = smtp_of(&o, SENDER_SMTP, SENDER_ADDRESS_TYPE, SENDER_EMAIL)
        .or_else(|| smtp_of(&o, SENT_REPRESENTING_SMTP, SENT_REPRESENTING_ADDRESS_TYPE, SENT_REPRESENTING_EMAIL))
        .unwrap_or_default();
    m.from = Address::new(name, email);

    let recipients = o.parts(RECIPIENT_PREFIX);
    for r in recipients.iter().take(MAX_RECIPIENTS) {
        budget.opened += 1;
        if budget.opened > MAX_OPENED {
            break;
        }
        let Some(r) = Object::open(cfb, *r, Header::Part, o.codepage) else { continue };
        let name = r.text(DISPLAY_NAME).or_else(|| r.text(RECIPIENT_DISPLAY_NAME)).unwrap_or_default();
        let email = smtp_of(&r, SMTP_ADDRESS, ADDRESS_TYPE, EMAIL_ADDRESS).unwrap_or_default();
        if name.is_empty() && email.is_empty() {
            continue;
        }
        let address = Address::new(if name == email { String::new() } else { name }, email);
        match r.int(RECIPIENT_TYPE).unwrap_or(1) & 0x3 {
            2 => m.cc.push(address),
            3 => m.bcc.push(address),
            _ => m.to.push(address),
        }
    }

    m.date = o.time(CLIENT_SUBMIT_TIME).or_else(|| o.time(MESSAGE_DELIVERY_TIME)).or_else(|| o.time(CREATION_TIME)).unwrap_or(0);
    m.received = o.time(MESSAGE_DELIVERY_TIME).unwrap_or(m.date);
    m.message_id = o.text(INTERNET_MESSAGE_ID).map(|s| strip_brackets(&s)).unwrap_or_default();
    m.in_reply_to = o.text(IN_REPLY_TO).map(|s| strip_brackets(&s)).unwrap_or_default();
    m.references = o.text(INTERNET_REFERENCES).map(|s| ids(&s)).unwrap_or_default();
    m.importance = match o.int(IMPORTANCE) {
        Some(0) => Importance::Low,
        Some(2) => Importance::High,
        _ => Importance::Normal,
    };
    m.read = o.int(MESSAGE_FLAGS).is_some_and(|f| f & 0x1 != 0);
    m.flag = match o.int(FLAG_STATUS) {
        Some(1) => FlagStatus::Complete,
        Some(2) => FlagStatus::Flagged,
        _ => FlagStatus::None,
    };
    match o.int(LAST_VERB_EXECUTED) {
        Some(102 | 103) => m.answered = true,
        Some(104) => m.forwarded = true,
        _ => {}
    }

    // The body: HTML from its own property, else from an RTF body that encapsulates it.
    let mut rtf_text = None;
    let mut html = o.binary(HTML).map(|b| html_text(&b, internet_cp.unwrap_or(0))).or_else(|| o.string(HTML));
    if html.is_none()
        && let Some(rtf) = o.binary(RTF_COMPRESSED).and_then(|c| rtf::decompress(&c))
    {
        match rtf::read(&rtf) {
            rtf::Content::Html(h) => html = Some(h),
            rtf::Content::Text(t) => rtf_text = Some(t),
        }
    }
    let html = html.map(|h| h.trim_end_matches('\0').to_string()).filter(|h| !h.trim().is_empty());
    let text = o.string(BODY).filter(|t| !t.trim().is_empty());
    m.body.text = unix_lines(&match (text, &html) {
        (Some(t), _) => t,
        (None, Some(h)) => mailcraft_html::to_text(h),
        (None, None) => rtf_text.unwrap_or_default(),
    });
    m.body.html = html;

    for a in o.parts(ATTACHMENT_PREFIX) {
        budget.opened += 1;
        if budget.attachments >= MAX_ATTACHMENTS || budget.bytes >= MAX_ATTACHMENT_BYTES || budget.opened > MAX_OPENED {
            break;
        }
        if let Some(a) = read_attachment(cfb, a, &o, m.body.html.as_deref(), depth, budget) {
            budget.attachments += 1;
            budget.bytes = budget.bytes.saturating_add(a.size());
            if budget.bytes <= MAX_ATTACHMENT_BYTES {
                m.attachments.push(a);
            }
        }
    }

    if let Some(headers) = o.string(TRANSPORT_HEADERS).filter(|h| !h.trim().is_empty()) {
        from_headers(&mut m, &headers, recipients.is_empty());
    }
    Some(m)
}

/// What the transport headers add: what the properties didn't have, and the fields MailCraft
/// doesn't model.
fn from_headers(m: &mut Message, headers: &str, no_recipients: bool) {
    let mut raw = headers.trim_end().replace("\r\n", "\n").replace('\n', "\r\n");
    raw.push_str("\r\n\r\n");
    let h = mailcraft_mime::parse(raw.as_bytes());
    if m.message_id.is_empty() {
        m.message_id = h.message_id;
    }
    if m.in_reply_to.is_empty() {
        m.in_reply_to = h.in_reply_to;
    }
    if m.references.is_empty() {
        m.references = h.references;
    }
    if m.from.email.is_empty() && !h.from.email.is_empty() {
        let name = if m.from.name.is_empty() { h.from.name } else { std::mem::take(&mut m.from.name) };
        m.from = Address::new(name, h.from.email);
    }
    if no_recipients {
        m.to = h.to;
        m.cc = h.cc;
    }
    if m.date == 0 {
        m.date = h.date;
    }
    if m.received == 0 {
        m.received = m.date;
    }
    m.reply_to = h.reply_to;
    m.categories = h.categories;
    m.headers = h.headers;
}

fn read_attachment(cfb: &Cfb, storage: usize, message: &Object, html: Option<&str>, depth: usize, budget: &mut Budget) -> Option<Attachment> {
    let a = Object::open(cfb, storage, Header::Part, message.codepage)?;
    let name = a.text(ATTACH_LONG_FILENAME).or_else(|| a.text(ATTACH_FILENAME)).or_else(|| a.text(DISPLAY_NAME)).map(|n| clean_name(&n));
    match a.int(ATTACH_METHOD).unwrap_or(1) {
        // An attached message.
        5 => {
            if depth + 1 >= MAX_DEPTH {
                return None;
            }
            let inner = *a.children.get(&format!("__SUBSTG1.0_{ATTACH_DATA:04X}{PT_OBJECT:04X}"))?;
            let inner = read_message(cfb, inner, Header::Embedded, depth + 1, budget)?;
            let base = name.filter(|n| !n.is_empty()).unwrap_or_else(|| clean_name(&inner.subject));
            let base = if base.is_empty() { "message".to_string() } else { base };
            let name = if base.to_ascii_lowercase().ends_with(".eml") { base } else { format!("{base}.eml") };
            let data = mailcraft_mime::write(&inner, &mailcraft_mime::WriteOptions { include_bcc: true, offset_minutes: 0 });
            Some(Attachment::new(name, "message/rfc822", data))
        }
        // By value (0 is "none", which some writers use for a file anyway).
        0 | 1 => {
            let data = a.binary(ATTACH_DATA)?;
            let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| "attachment".to_string());
            let mime = a
                .text(ATTACH_MIME_TAG)
                .map(|t| t.to_ascii_lowercase())
                .filter(|t| t.split_once('/').is_some_and(|(x, y)| !x.is_empty() && !y.is_empty()) && t.bytes().all(|b| b.is_ascii_graphic()))
                .unwrap_or_else(|| mailcraft_mime::media_type_for(&name).to_string());
            let content_id = a.text(ATTACH_CONTENT_ID).map(|c| strip_brackets(&c)).unwrap_or_default();
            let inline = !content_id.is_empty()
                && (a.int(ATTACH_FLAGS).is_some_and(|f| f & 0x4 != 0)
                    || a.bool(ATTACHMENT_HIDDEN) == Some(true)
                    || html.is_some_and(|h| h.contains(&format!("cid:{content_id}"))));
            Some(Attachment { name, mime, content_id, inline, data: Arc::new(data) })
        }
        // By reference, by web reference, an OLE object: nothing that is the file itself, and
        // nothing is fetched.
        _ => None,
    }
}

/// HTML from its bytes: in the code page the message says, else the one the HTML declares, else
/// UTF-8 or Windows-1252 by what the bytes are.
fn html_text(bytes: &[u8], codepage: u32) -> String {
    if codepage != 0 {
        return codepage::decode(bytes, codepage);
    }
    let head = String::from_utf8_lossy(bytes.get(..bytes.len().min(4096)).unwrap_or(&[])).to_ascii_lowercase();
    let label = head.find("charset=").map(|at| {
        head.get(at + 8..)
            .unwrap_or("")
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
            .collect::<String>()
    });
    mailcraft_mime::decode_charset(bytes, label.as_deref().unwrap_or(""))
}

/// A file name without folders or control characters.
fn clean_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    name.chars().filter(|c| !c.is_control()).take(255).collect::<String>().trim().to_string()
}

fn strip_brackets(s: &str) -> String {
    s.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()
}

/// The ids of a `References` value.
fn ids(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || c == ',').map(strip_brackets).filter(|id| !id.is_empty()).take(1000).collect()
}

fn unix_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
