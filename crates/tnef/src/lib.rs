// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's reader of TNEF streams: the `winmail.dat` (`application/ms-tnef`) that Outlook and
//! Exchange attach to mail in place of the files the sender attached. Written from Microsoft's
//! published specification, [MS-OXTNEF], with property ids from [MS-OXPROPS] and [MS-OXCMSG],
//! and nothing else.
//!
//! [`read`] gives the files inside a stream ([`File`]) and the text of its message ([`Tnef::body`]),
//! for when the message around it has none. What it doesn't turn into a file (an attached
//! message, an OLE object, an attachment with no data, an attribute whose checksum is wrong, a
//! list of properties it can't read to its end) is named in [`Tnef::left_out`], so that a caller
//! can keep the stream itself.
//!
//! A TNEF stream is hostile input. Nothing here panics on any bytes: every length is checked
//! against what is left, nothing is opened inside another stream (an attached message is its own
//! TNEF stream, and stays shut), and the caps of [`Limits`] are checked before anything is kept.
//! The files are copies of slices of the input, at most one per attachment, so what comes out is
//! never more than what went in.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]

mod bytes;
mod props;
/// The test writer: synthetic TNEF streams, for this crate's tests and, through the
/// `test-writer` feature (a dev-dependency only, never in a build that ships), for the tests of
/// the crates that read them.
#[cfg(any(test, feature = "test-writer"))]
#[doc(hidden)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, dead_code)]
pub mod test_writer;
#[cfg(test)]
mod tests;

use std::fmt;

use bytes::Cursor;
use props::{Props, Value};

/// The first four bytes of every TNEF stream (`TNEFSignature`, 2.1.3.2).
pub const SIGNATURE: [u8; 4] = [0x78, 0x9F, 0x3E, 0x22];

/// The only `attTnefVersion` a reader may take (2.3.3.1).
const VERSION: [u8; 4] = [0x00, 0x00, 0x01, 0x00];

/// Attribute ids (the low word of an attribute's id, 2.1.3.2): the type in the high word is
/// fixed by the id, and isn't relied on.
mod id {
    pub const TNEF_VERSION: u16 = 0x9006;
    pub const OEM_CODEPAGE: u16 = 0x9007;
    pub const MESSAGE_CLASS: u16 = 0x8008;
    pub const ORIGINAL_MESSAGE_CLASS: u16 = 0x0006;
    pub const BODY: u16 = 0x800C;
    pub const MSG_PROPS: u16 = 0x9003;
    pub const ATTACH_REND_DATA: u16 = 0x9002;
    pub const ATTACH_DATA: u16 = 0x800F;
    pub const ATTACH_TITLE: u16 = 0x8010;
    pub const ATTACHMENT: u16 = 0x9005;
}

/// Property ids ([MS-OXPROPS]).
mod pid {
    /// PidTagBody.
    pub const BODY: u16 = 0x1000;
    /// PidTagRtfCompressed.
    pub const RTF_COMPRESSED: u16 = 0x1009;
    /// PidTagInternetCodepage.
    pub const INTERNET_CODEPAGE: u16 = 0x3FDE;
    /// PidTagAttachDataBinary, or PidTagAttachDataObject for an attached message or object.
    pub const ATTACH_DATA: u16 = 0x3701;
    /// PidTagAttachFilename (8.3).
    pub const ATTACH_FILENAME: u16 = 0x3704;
    /// PidTagAttachMethod.
    pub const ATTACH_METHOD: u16 = 0x3705;
    /// PidTagAttachLongFilename.
    pub const ATTACH_LONG_FILENAME: u16 = 0x3707;
    /// PidTagAttachMimeTag.
    pub const ATTACH_MIME_TAG: u16 = 0x370E;
    /// PidTagAttachContentId.
    pub const ATTACH_CONTENT_ID: u16 = 0x3712;
    /// PidTagDisplayName.
    pub const DISPLAY_NAME: u16 = 0x3001;
}

/// PidTagAttachMethod values ([MS-OXCMSG] 2.2.2.9).
mod method {
    pub const BY_VALUE: i64 = 1;
    pub const EMBEDDED_MESSAGE: i64 = 5;
    pub const OLE: i64 = 6;
}

/// Whether bytes start as a TNEF stream does.
pub fn is_tnef(bytes: &[u8]) -> bool {
    bytes.starts_with(&SIGNATURE)
}

/// The caps that keep a hostile stream bounded. Past one, what is left is not read, and
/// [`LeftOut::Cap`] says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Most attributes read from one stream.
    pub max_attributes: usize,
    /// Most files given from one stream.
    pub max_files: usize,
    /// Most properties read from one list (`attMsgProps`, `attAttachment`).
    pub max_properties: usize,
    /// Most values read from one multi-valued property.
    pub max_values: usize,
    /// Most characters kept of a file's name, media type or content id.
    pub max_name: usize,
    /// Most bytes of the message's text, and of the rich text it is read from.
    pub max_body: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_attributes: 10_000, max_files: 1_000, max_properties: 4_096, max_values: 4_096, max_name: 1_024, max_body: 16 * 1024 * 1024 }
    }
}

/// What a TNEF stream holds that MailCraft keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tnef {
    /// The files attached to the message, in the stream's order.
    pub files: Vec<File>,
    /// The message's plain text (PidTagBody, else `attBody`, else the text of its compressed
    /// rich text), with no NUL characters; `None` when it has none. Line ends are as written.
    pub body: Option<String>,
    /// What wasn't read or turned into a file: each kind once, in the order first met.
    pub left_out: Vec<LeftOut>,
}

impl Tnef {
    /// Whether every attachment in the stream is among [`Tnef::files`] and nothing else was
    /// skipped.
    pub fn is_complete(&self) -> bool {
        self.left_out.is_empty()
    }
}

/// One attached file.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct File {
    /// Its name (PidTagAttachLongFilename, else PidTagAttachFilename, else `attAttachTitle`,
    /// else PidTagDisplayName), without control characters; `None` when it has none.
    pub name: Option<String>,
    /// The media type it says it is (PidTagAttachMimeTag), as written.
    pub mime_type: Option<String>,
    /// Its content id (PidTagAttachContentId), without angle brackets.
    pub content_id: Option<String>,
    /// Its bytes.
    pub data: Vec<u8>,
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("name", &self.name)
            .field("mime_type", &self.mime_type)
            .field("content_id", &self.content_id)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .finish()
    }
}

/// Something in a stream that wasn't read or turned into a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeftOut {
    /// An attached message (ATTACH_EMBEDDED_MSG): a TNEF stream of its own, not opened.
    AttachedMessage,
    /// An OLE object (ATTACH_OLE), or an attachment by reference: no file to give.
    Object,
    /// An attachment with no data.
    NoData,
    /// An attribute whose checksum doesn't match its data: skipped.
    BadChecksum,
    /// A list of properties that couldn't be read to its end (a type of no known size, or a
    /// value past the end of the list).
    Properties,
    /// The stream ends inside an attribute.
    Truncated,
    /// Past one of the [`Limits`].
    Cap,
}

impl fmt::Display for LeftOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LeftOut::AttachedMessage => "an attached message",
            LeftOut::Object => "an embedded object",
            LeftOut::NoData => "an attachment with no data",
            LeftOut::BadChecksum => "an attribute whose checksum is wrong",
            LeftOut::Properties => "properties that couldn't be read",
            LeftOut::Truncated => "the end of a stream that was cut short",
            LeftOut::Cap => "more than a stream may hold",
        })
    }
}

/// Why a stream can't be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The bytes don't start with [`SIGNATURE`].
    NotTnef,
    /// An `attTnefVersion` other than the one a reader may take (2.3.3.1).
    UnsupportedVersion,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::NotTnef => "not a TNEF stream",
            Error::UnsupportedVersion => "a TNEF stream of a version that can't be read",
        })
    }
}

impl std::error::Error for Error {}

/// One attribute (2.1.3.3): its level, id, data and whether its checksum matches.
struct Attribute<'a> {
    id: u16,
    data: &'a [u8],
    checksum_ok: bool,
}

/// One attachment's attributes, gathered until the stream's code page is known.
#[derive(Default)]
struct Pending<'a> {
    /// `attAttachRendData` said it is an OLE object.
    ole: bool,
    title: Option<&'a [u8]>,
    data: Option<&'a [u8]>,
    props: Props<'a>,
}

/// Read a TNEF stream: the files it holds and its message's text.
///
/// An error only for bytes that aren't a TNEF stream or are one of a version that can't be
/// read; anything else that goes wrong leaves out what it touches, named in
/// [`Tnef::left_out`], and the rest is read.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Tnef, Error> {
    let mut c = Cursor::new(bytes);
    if c.take(SIGNATURE.len()) != Some(&SIGNATURE[..]) {
        return Err(Error::NotTnef);
    }
    let mut left_out = Vec::new();
    // LegacyKey: any number.
    if c.u16().is_none() {
        note(&mut left_out, LeftOut::Truncated);
    }

    let mut oem_codepage = None;
    let mut body_attr = None;
    let mut message = Props::default();
    let mut pending: Vec<Pending<'_>> = Vec::new();
    let mut attributes = 0usize;
    // As many attachments as a stream may give have been started: the rest are left out.
    let mut full = false;
    while !c.is_empty() {
        if attributes >= limits.max_attributes {
            note(&mut left_out, LeftOut::Cap);
            break;
        }
        attributes = attributes.saturating_add(1);
        let Some(attr) = attribute(&mut c) else {
            note(&mut left_out, LeftOut::Truncated);
            break;
        };
        // Some old writers got the checksum of the message class wrong (2.3.3.4): it is read
        // for nothing here, so any of its checksums will do.
        let class = matches!(attr.id, id::MESSAGE_CLASS | id::ORIGINAL_MESSAGE_CLASS);
        if !attr.checksum_ok && !class {
            if attr.id == id::TNEF_VERSION {
                return Err(Error::UnsupportedVersion);
            }
            note(&mut left_out, LeftOut::BadChecksum);
            // The attachment this attribute belonged to may be missing it now: a new group
            // begins anyway, so that what follows isn't taken for the one before.
            if attr.id == id::ATTACH_REND_DATA {
                full |= !start(&mut pending, false, limits, &mut left_out);
            }
            continue;
        }
        match attr.id {
            id::TNEF_VERSION => {
                if attr.data != VERSION {
                    return Err(Error::UnsupportedVersion);
                }
            }
            id::OEM_CODEPAGE => oem_codepage = Cursor::new(attr.data).u32().filter(|cp| *cp != 0),
            id::BODY => body_attr = Some(attr.data),
            id::MSG_PROPS => {
                let (props, complete) = props::read(attr.data, limits);
                if !complete {
                    note(&mut left_out, LeftOut::Properties);
                }
                if !message.merge(props) {
                    note(&mut left_out, LeftOut::Cap);
                }
            }
            id::ATTACH_REND_DATA => {
                // AttachType: 1 a file, 2 an OLE object.
                let ole = Cursor::new(attr.data).u16() == Some(2);
                full |= !start(&mut pending, ole, limits, &mut left_out);
            }
            id::ATTACH_DATA | id::ATTACH_TITLE | id::ATTACHMENT => {
                // An attachment's attributes start with attAttachRendData; one that doesn't is
                // taken as starting its own. Past the cap of files, the rest are left alone.
                if full {
                    continue;
                }
                if pending.is_empty() {
                    full |= !start(&mut pending, false, limits, &mut left_out);
                }
                let Some(current) = pending.last_mut() else { continue };
                match attr.id {
                    id::ATTACH_DATA => current.data = Some(attr.data),
                    id::ATTACH_TITLE => current.title = Some(attr.data),
                    _ => {
                        let (props, complete) = props::read(attr.data, limits);
                        if !complete {
                            note(&mut left_out, LeftOut::Properties);
                        }
                        if !current.props.merge(props) {
                            note(&mut left_out, LeftOut::Cap);
                        }
                    }
                }
            }
            // The rest says nothing MailCraft keeps of a message that comes as MIME: its sender,
            // subject and dates are the MIME header's (2.1.3.5.2).
            _ => {}
        }
    }

    // The code page of 8-bit strings (2.3.3.2): attOemCodepage, else PidTagInternetCodepage,
    // else the most common one.
    let label = oem_codepage
        .map(i64::from)
        .or_else(|| message.int(pid::INTERNET_CODEPAGE))
        .and_then(mailcraft_pst::code_page_label)
        .unwrap_or("windows-1252");

    let mut files = Vec::new();
    for p in pending {
        if let Some(file) = file(p, label, limits, &mut left_out) {
            files.push(file);
        }
    }
    let (body, cut) = body(&message, body_attr, label, limits);
    if cut {
        note(&mut left_out, LeftOut::Cap);
    }
    Ok(Tnef { files, body, left_out })
}

/// The next attribute: `level id length data checksum`. `None` when the stream ends inside it.
fn attribute<'a>(c: &mut Cursor<'a>) -> Option<Attribute<'a>> {
    // The level (message or attachment) is taken from the id; a reader needn't trust both.
    let _level = c.u8()?;
    let id = c.u32()?;
    let length = usize::try_from(c.u32()?).ok()?;
    let data = c.take(length)?;
    let checksum = c.u16()?;
    let sum = data.iter().fold(0u16, |s, b| s.wrapping_add(u16::from(*b)));
    // The low word is the attribute's id, the high word its type.
    let [low, high, _, _] = id.to_le_bytes();
    let id = u16::from_le_bytes([low, high]);
    Some(Attribute { id, data, checksum_ok: sum == checksum })
}

/// Start an attachment's attributes; false, and nothing started, when there are already as
/// many as a stream may give.
fn start(pending: &mut Vec<Pending<'_>>, ole: bool, limits: &Limits, left_out: &mut Vec<LeftOut>) -> bool {
    if pending.len() >= limits.max_files {
        note(left_out, LeftOut::Cap);
        return false;
    }
    pending.push(Pending { ole, ..Pending::default() });
    true
}

/// The file an attachment's attributes make, or why there is none.
fn file(p: Pending<'_>, label: &str, limits: &Limits, left_out: &mut Vec<LeftOut>) -> Option<File> {
    let method = p.props.int(pid::ATTACH_METHOD);
    if method == Some(method::EMBEDDED_MESSAGE) {
        note(left_out, LeftOut::AttachedMessage);
        return None;
    }
    if p.ole || method == Some(method::OLE) {
        note(left_out, LeftOut::Object);
        return None;
    }
    if method.is_some_and(|m| m != method::BY_VALUE) {
        // By reference: a path or a web address on the sender's side, never fetched.
        note(left_out, LeftOut::Object);
        return None;
    }
    // PidTagAttachDataBinary, else attAttachData; an object here is not a file.
    let data = match p.props.get(pid::ATTACH_DATA) {
        Some(Value::Binary(data)) => Some(*data),
        Some(Value::Object) => {
            note(left_out, LeftOut::Object);
            return None;
        }
        _ => p.data,
    };
    let Some(data) = data else {
        note(left_out, LeftOut::NoData);
        return None;
    };
    let name = p
        .props
        .text(pid::ATTACH_LONG_FILENAME, label)
        .or_else(|| p.props.text(pid::ATTACH_FILENAME, label))
        .or_else(|| p.title.map(|t| mailcraft_charset::decode(t, label)))
        .or_else(|| p.props.text(pid::DISPLAY_NAME, label))
        .map(|n| clean(&n, limits.max_name))
        .filter(|n| !n.is_empty());
    let mime_type = p.props.text(pid::ATTACH_MIME_TAG, label).map(|t| clean(&t, limits.max_name)).filter(|t| !t.is_empty());
    let content_id = p
        .props
        .text(pid::ATTACH_CONTENT_ID, label)
        .map(|t| clean(t.trim().trim_start_matches('<').trim_end_matches('>'), limits.max_name))
        .filter(|t| !t.is_empty());
    Some(File { name, mime_type, content_id, data: data.to_vec() })
}

/// The message's plain text: PidTagBody, else `attBody`, else the text of its compressed rich
/// text; and whether it was cut at [`Limits::max_body`].
fn body(message: &Props<'_>, body_attr: Option<&[u8]>, label: &str, limits: &Limits) -> (Option<String>, bool) {
    let plain = message.text(pid::BODY, label).or_else(|| body_attr.map(|b| mailcraft_charset::decode(b, label))).map(|t| (t, false));
    let mut too_big = false;
    let found = plain.or_else(|| {
        let Some(Value::Binary(rtf)) = message.get(pid::RTF_COMPRESSED) else { return None };
        let rtf = match mailcraft_pst::decompress_rtf(rtf, limits.max_body) {
            Ok(rtf) => rtf,
            Err(mailcraft_pst::Error::Cap(_)) => {
                too_big = true;
                return None;
            }
            Err(_) => return None,
        };
        mailcraft_pst::rtf_text(&rtf, limits.max_body).ok()
    });
    let Some((text, rtf_cut)) = found else { return (None, too_big) };
    let text: String = text.chars().filter(|c| *c != '\0').collect();
    let cut = rtf_cut || text.len() > limits.max_body;
    let text = truncate(text, limits.max_body);
    ((!text.trim().is_empty()).then_some(text), cut)
}

/// A name without NULs and other control characters, trimmed, at most `max` characters.
fn clean(name: &str, max: usize) -> String {
    name.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// `text` cut to at most `max` bytes, at a character's start.
fn truncate(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
    }
    text
}

/// Add a reason to what was left out, once.
fn note(left_out: &mut Vec<LeftOut>, what: LeftOut) {
    if !left_out.contains(&what) {
        left_out.push(what);
    }
}
