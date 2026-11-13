// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header lines in two parts: the few the program reads by name, kept on the message
//! ([`Message::headers`]), and all the others, its **header block** ([`Message::header_block`]):
//! the trace, the spam filters' reports, the signatures and every other line nothing here reads,
//! which are most of what a message's header takes (Exchange writes several kilobytes of them on
//! every message). The block is kept as one piece, in order, with where each line stood among all
//! of them, so that the header is written out again as it came: View Source, Save As, a copy on
//! another account's server and a forward as an attachment carry every line, in its place.
//!
//! A mailbox kept in a file with a content folder beside it keeps a large block there, read only
//! when the message is written out (the engine's store); in memory it is then only named
//! ([`HeaderBlock::kept`]).
//!
//! What is read by name is [`WORKING`] (and MailCraft's own `X-MailCraft-…` notes, and the hop at
//! which Exchange Online took the mail in, which a sender check places its verdict by): a name
//! read anywhere that isn't in it would never be found, which a test holds to.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ContentRef, Message};

/// The header names the program reads by name, which stay on the message ([`is_working`]).
pub const WORKING: &[&str] = &[
    // The server's verdict on the sender (`auth_results`, `sender_warnings`), and what is left
    // behind when mail goes to another account (`auth_results::forget_stamps`).
    "Authentication-Results",
    "ARC-Authentication-Results",
    crate::auth_results::AUTH_AS,
    crate::auth_results::DIRECTIONALITY,
    // A mailing list, whose name a member's name may carry (`sender_warnings`).
    "List-Id",
    "List-Post",
    // A request for a read receipt, which an answer never makes.
    "Disposition-Notification-To",
    // What a .pst file's item said of who sent it and how sensitive it is.
    "Sender",
    "Sensitivity",
    // The id a message had before Microsoft Graph moved it to another account.
    "X-Microsoft-Original-Message-ID",
    // A sharing invitation for a calendar ([MS-OXSHARE]).
    "X-Sharing-Provider-Guid",
    "X-Sharing-Remote-Type",
    "X-Sharing-Local-Type",
];

/// The start of the name of every note MailCraft keeps on a message itself (the file it was
/// opened from, its place in a .pst file, an answer to an invitation, a message too large to
/// download, the copy put on a server): all read by name, all on the message.
pub const OWN_PREFIX: &str = "X-MailCraft-";

/// Most lines of a header block kept (a message read by `mailcraft-mime` keeps at most a few
/// hundred).
pub const MAX_BLOCK_LINES: usize = 1024;

/// Most bytes of a header block as it is kept ([`HeaderBlock::data`]); one read back that is
/// longer is no block.
pub const MAX_HEADER_BLOCK: usize = 256 * 1024;

/// Whether a header line with this name (and value) stays on the message: a name in [`WORKING`],
/// one of MailCraft's own notes, or a `Received` hop at Exchange Online's edge, above which its
/// own `Authentication-Results` is read (`auth_results`).
pub fn is_working(name: &str, value: &str) -> bool {
    is_working_name(name) || (name.eq_ignore_ascii_case("Received") && crate::auth_results::is_exchange_online_edge(value))
}

/// Whether every header line with this name stays on the message ([`is_working`]).
pub fn is_working_name(name: &str) -> bool {
    WORKING.iter().any(|w| w.eq_ignore_ascii_case(name)) || name.get(..OWN_PREFIX.len()).is_some_and(|start| start.eq_ignore_ascii_case(OWN_PREFIX))
}

/// One line of a header block: where it stood among all the message's header lines it kept, its
/// name, its value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderLine {
    pub at: usize,
    pub name: String,
    pub value: String,
}

/// The header lines of a message that nothing reads by name ([`Message::header_block`]): in
/// memory as one piece of bytes (`data`), or kept outside it, named by `kept`, and read back
/// when the message is written out.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HeaderBlock {
    /// The lines, when they are in memory, as `[[at,"name","value"],…]` (JSON). Empty while they
    /// are kept outside (`kept`), and when there are none.
    pub data: Arc<Vec<u8>>,
    /// Where the bytes are when `data` doesn't hold them (as `Attachment::kept`). Not written.
    pub kept: Option<ContentRef>,
}

impl std::fmt::Debug for HeaderBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeaderBlock({} bytes{})", self.size(), if self.is_loaded() { "" } else { ", kept" })
    }
}

impl HeaderBlock {
    /// A block of `lines`, at most [`MAX_BLOCK_LINES`] of them and [`MAX_HEADER_BLOCK`] bytes:
    /// the lines past either are left out.
    pub fn of(lines: &[HeaderLine]) -> HeaderBlock {
        // `[`, `]`, and a comma between lines.
        let mut size = 2usize;
        let mut taken = 0usize;
        for line in lines.iter().take(MAX_BLOCK_LINES) {
            let one = serde_json::to_vec(&(line.at, &line.name, &line.value)).map_or(usize::MAX, |b| b.len());
            size = size.saturating_add(one).saturating_add(usize::from(taken > 0));
            if size > MAX_HEADER_BLOCK {
                break;
            }
            taken += 1;
        }
        let lines = lines.get(..taken).unwrap_or(&[]);
        if lines.is_empty() {
            return HeaderBlock::default();
        }
        HeaderBlock { data: Arc::new(encode(lines)), kept: None }
    }

    /// The bytes read back as a block: `None` when they are more than [`MAX_HEADER_BLOCK`] or
    /// aren't one. At most [`MAX_BLOCK_LINES`] lines are taken.
    pub fn read(bytes: &[u8]) -> Option<HeaderBlock> {
        let lines = decode(bytes)?;
        Some(HeaderBlock::of(&lines))
    }

    /// The lines in memory, in order; none while they are kept outside (or when the bytes are
    /// damaged).
    pub fn lines(&self) -> Vec<HeaderLine> {
        if self.data.is_empty() { Vec::new() } else { decode(&self.data).unwrap_or_default() }
    }

    /// Whether there is no block: nothing in memory and nothing kept outside.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() && self.kept.is_none()
    }

    /// Whether none of its lines are in memory (there are none, or they are kept outside).
    pub fn none_in_memory(&self) -> bool {
        self.data.is_empty()
    }

    /// Whether its lines are in memory, or there is nothing kept outside to bring in.
    pub fn is_loaded(&self) -> bool {
        self.kept.is_none() || !self.data.is_empty()
    }

    /// How many bytes it is, in memory or where it is kept.
    pub fn size(&self) -> u64 {
        match self.kept {
            Some(k) if self.data.is_empty() => k.len,
            _ => self.data.len() as u64,
        }
    }
}

/// `lines` as [`HeaderBlock::data`] has them.
fn encode(lines: &[HeaderLine]) -> Vec<u8> {
    let wire: Vec<(usize, &str, &str)> = lines.iter().map(|l| (l.at, l.name.as_str(), l.value.as_str())).collect();
    serde_json::to_vec(&wire).unwrap_or_default()
}

/// [`HeaderBlock::data`] read back, within [`MAX_HEADER_BLOCK`] bytes and [`MAX_BLOCK_LINES`]
/// lines.
fn decode(bytes: &[u8]) -> Option<Vec<HeaderLine>> {
    if bytes.len() > MAX_HEADER_BLOCK {
        return None;
    }
    let wire: Vec<(u64, String, String)> = serde_json::from_slice(bytes).ok()?;
    Some(
        wire.into_iter()
            .take(MAX_BLOCK_LINES)
            .map(|(at, name, value)| HeaderLine { at: usize::try_from(at).unwrap_or(usize::MAX), name, value })
            .collect(),
    )
}

/// The lines as a list of `[at, name, value]`, which is what [`HeaderBlock::data`] holds.
impl Serialize for HeaderBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let lines = self.lines();
        serializer.collect_seq(lines.iter().map(|l| (l.at, &l.name, &l.value)))
    }
}

impl<'de> Deserialize<'de> for HeaderBlock {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Lines;
        impl<'de> serde::de::Visitor<'de> for Lines {
            type Value = Vec<HeaderLine>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a list of header lines")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while out.len() < MAX_BLOCK_LINES {
                    match seq.next_element::<(u64, String, String)>()? {
                        Some((at, name, value)) => out.push(HeaderLine { at: usize::try_from(at).unwrap_or(usize::MAX), name, value }),
                        None => return Ok(out),
                    }
                }
                // The rest is skipped without being kept.
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(out)
            }
        }
        d.deserialize_seq(Lines).map(|lines| HeaderBlock::of(&lines))
    }
}

/// `all` (a message's header lines, in order) as the lines that stay on the message and its
/// header block.
pub fn split(all: Vec<(String, String)>) -> (Vec<(String, String)>, HeaderBlock) {
    let mut working = Vec::new();
    let mut block = Vec::new();
    for (at, (name, value)) in all.into_iter().enumerate() {
        if is_working(&name, &value) {
            working.push((name, value));
        } else {
            block.push(HeaderLine { at, name, value });
        }
    }
    (working, HeaderBlock::of(&block))
}

/// The lines that stayed on a message and those of its block as one list, each block line back in
/// its place: what [`split`] was given, for a message unchanged since. A line put on the message
/// since goes where its place now falls; a place past the end is the end.
pub fn merge(working: &[(String, String)], block: &[HeaderLine]) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(working.len().saturating_add(block.len()));
    let mut rest = working.iter();
    for line in block {
        while out.len() < line.at {
            match rest.next() {
                Some(h) => out.push(h.clone()),
                None => break,
            }
        }
        out.push((line.name.clone(), line.value.clone()));
    }
    out.extend(rest.cloned());
    out
}

impl Message {
    /// The value of the first header line named `name` that stays on the message (the name is one
    /// of [`WORKING`], or MailCraft's own), at most `auth_results::MAX_HEADERS_READ` looked at.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .take(crate::auth_results::MAX_HEADERS_READ)
            .find(|(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The values of every header line named `name` that stays on the message, in order.
    pub fn header_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .take(crate::auth_results::MAX_HEADERS_READ)
            .filter(move |(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether a header line named `name` stays on the message.
    pub fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    /// Take every header line named `name` off the message (a name that stays on it). With its
    /// block in memory, the block's lines keep their places among the rest.
    pub fn remove_header(&mut self, name: &str) {
        if !self.headers.iter().any(|(have, _)| have.eq_ignore_ascii_case(name)) {
            return;
        }
        if self.header_block.data.is_empty() {
            self.headers.retain(|(have, _)| !have.eq_ignore_ascii_case(name));
            return;
        }
        let mut all = self.all_headers();
        all.retain(|(have, _)| !have.eq_ignore_ascii_case(name));
        (self.headers, self.header_block) = split(all);
    }

    /// Every header line of the message kept, in order: those on it and those of its block in
    /// memory. A block kept outside memory is not among them: whoever writes the message out
    /// brings it in first (`Session::loaded_message` in the engine).
    pub fn all_headers(&self) -> Vec<(String, String)> {
        if self.header_block.data.is_empty() { self.headers.clone() } else { merge(&self.headers, &self.header_block.lines()) }
    }

    /// Move the header lines that nothing reads by name from [`Message::headers`] into the
    /// message's block ([`split`]): for a message read before the two were kept apart, or one a
    /// line was added to. A block kept outside memory stays as it is, and so does the rest.
    pub fn split_headers(&mut self) {
        if !self.header_block.is_loaded() || self.headers.iter().all(|(name, value)| is_working(name, value)) {
            return;
        }
        let all = merge(&std::mem::take(&mut self.headers), &self.header_block.lines());
        (self.headers, self.header_block) = split(all);
    }

    /// [`Message::all_headers`] put back on the message, with no block: as a file written for a
    /// MailCraft from before the two were kept apart has it.
    pub fn join_headers(&mut self) {
        if self.header_block.data.is_empty() {
            return;
        }
        self.headers = self.all_headers();
        self.header_block = HeaderBlock::default();
    }
}
