// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The list of Favorites inside the cached configuration of Outlook on the web: an XML document
//! that nobody documents, read for one entry of one dictionary and nothing else.
//!
//! The document is a dictionary of pairs, each an element with a `Key` and a `Value` in it. The
//! pair whose key reads `OWA.OutlookFavorites` holds the list: under its `Value`, one
//! `OutlookFavorite` element for each favourite, with an `Index` (its place), a `Type` (`Folder`
//! for a folder) and, under `SingleValueSettings`, pairs (`KeyValueType`: a `Key` and a `Value`)
//! of which the one called `FolderId` names the folder. Elements are told by their local names:
//! the prefixes are the document's own choice, and the pair's own element has a name with a
//! generated end to it, which is not looked at.
//!
//! Hostile twice over: it comes from a server, and its shape is known from looking only. It is
//! read as a stream by a reader that takes no DTD and expands no entity (a document with a
//! `<!DOCTYPE` is not read at all, and a reference that isn't a character or one of XML's own
//! five spoils the text it stands in); the depth, every text that is kept and the number of
//! entries are capped; and nothing but the three fields above is ever taken out of it. The
//! names the favourites are shown by in the document are not read: a folder is called what its
//! mailbox calls it.

use quick_xml::Reader;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesRef, Event};

/// The key of the pair that holds the list, and of the setting that names a favourite's folder.
const FAVORITES_KEY: &str = "OWA.OutlookFavorites";
const FOLDER_ID_KEY: &str = "FolderId";
/// Deepest the elements may nest, longest text kept of one field (bytes), and most entries read.
const MAX_DEPTH: usize = 64;
const MAX_FIELD: usize = 2_048;
pub(super) const MAX_ENTRIES: usize = 500;

/// One entry of the list, as the document has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Entry {
    /// Its place in the list; `None` when it says none that is a number.
    pub(super) index: Option<i64>,
    /// What kind of thing it is (`Folder`), as written.
    pub(super) kind: String,
    /// What its `FolderId` setting says; empty when it has none.
    pub(super) folder_id: String,
}

/// What a document says of the Favorites.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Listed {
    /// The entries, in the order the document has them.
    pub(super) entries: Vec<Entry>,
    /// How many more there were than are read.
    pub(super) more: usize,
}

/// The document isn't one that can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Malformed;

/// The elements that are told apart; any other is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Key,
    Value,
    Favorite,
    Index,
    Type,
    Settings,
    Setting,
    Other,
}

fn tag_of(local: &[u8]) -> Tag {
    match local {
        b"Key" => Tag::Key,
        b"Value" => Tag::Value,
        b"OutlookFavorite" => Tag::Favorite,
        b"Index" => Tag::Index,
        b"Type" => Tag::Type,
        b"SingleValueSettings" => Tag::Settings,
        b"KeyValueType" => Tag::Setting,
        _ => Tag::Other,
    }
}

/// How far the reading has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The pair with the key is yet to come.
    Seeking,
    /// The key was read, in a pair that is this deep: its `Value` is next.
    Paired(usize),
    /// Inside the `Value`, which is this deep.
    Inside(usize),
    /// The `Value` is read to its end.
    Done,
}

struct Walk {
    phase: Phase,
    stack: Vec<Tag>,
    /// The text of the field being read, and how deep its element is.
    field: Option<(String, usize)>,
    /// That text holds something that isn't taken (too long, a reference to who knows what).
    spoiled: bool,
    entry: Option<Entry>,
    setting: (String, String),
    listed: Listed,
}

impl Walk {
    /// Whether the innermost elements are `tail`, outermost first.
    fn under(&self, tail: &[Tag]) -> bool {
        self.stack.ends_with(tail)
    }

    fn open(&mut self, local: &[u8]) -> Result<(), Malformed> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(Malformed);
        }
        let mut tag = tag_of(local);
        // A favourite inside a favourite is nothing that is known.
        if tag == Tag::Favorite && self.entry.is_some() {
            tag = Tag::Other;
        }
        match (self.phase, tag) {
            (Phase::Paired(depth), Tag::Value) if self.stack.len() == depth => self.phase = Phase::Inside(depth + 1),
            (Phase::Inside(_), Tag::Favorite) => self.entry = Some(Entry::default()),
            _ => {}
        }
        self.stack.push(tag);
        let wanted = match self.phase {
            Phase::Seeking => tag == Tag::Key,
            Phase::Inside(_) if self.entry.is_some() => {
                self.under(&[Tag::Favorite, Tag::Index])
                    || self.under(&[Tag::Favorite, Tag::Type])
                    || self.under(&[Tag::Favorite, Tag::Settings, Tag::Setting, Tag::Key])
                    || self.under(&[Tag::Favorite, Tag::Settings, Tag::Setting, Tag::Value])
            }
            _ => false,
        };
        // (An element inside a field is no part of its text: the field goes on once it has ended.)
        if wanted && self.field.is_none() {
            self.field = Some((String::new(), self.stack.len()));
            self.spoiled = false;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), Malformed> {
        let depth = self.stack.len();
        let Some(tag) = self.stack.last().copied() else { return Err(Malformed) };
        if self.field.as_ref().is_some_and(|(_, at)| *at == depth) {
            let text = self.field.take().map(|(text, _)| text).unwrap_or_default();
            let text = if self.spoiled { "" } else { text.trim() };
            match (self.phase, tag) {
                (Phase::Seeking, Tag::Key) if text == FAVORITES_KEY => self.phase = Phase::Paired(depth.saturating_sub(1)),
                (Phase::Inside(_), Tag::Index) => {
                    if let Some(entry) = &mut self.entry {
                        entry.index = text.parse::<i64>().ok();
                    }
                }
                (Phase::Inside(_), Tag::Type) => {
                    if let Some(entry) = &mut self.entry {
                        entry.kind = text.to_string();
                    }
                }
                (Phase::Inside(_), Tag::Key) => self.setting.0 = text.to_string(),
                (Phase::Inside(_), Tag::Value) => self.setting.1 = text.to_string(),
                _ => {}
            }
        }
        match (self.phase, tag) {
            // One setting is read: the one that names the folder is kept, the first of them.
            (Phase::Inside(_), Tag::Setting) if self.under(&[Tag::Favorite, Tag::Settings, Tag::Setting]) => {
                let (key, value) = std::mem::take(&mut self.setting);
                if let Some(entry) = self.entry.as_mut().filter(|entry| key == FOLDER_ID_KEY && entry.folder_id.is_empty()) {
                    entry.folder_id = value;
                }
            }
            (Phase::Inside(_), Tag::Favorite) => {
                self.setting = Default::default();
                if let Some(entry) = self.entry.take() {
                    if self.listed.entries.len() < MAX_ENTRIES {
                        self.listed.entries.push(entry);
                    } else {
                        self.listed.more = self.listed.more.saturating_add(1);
                    }
                }
            }
            _ => {}
        }
        self.stack.pop();
        match self.phase {
            // The pair ended without a value: the key was no more than a text.
            Phase::Paired(pair) if self.stack.len() < pair => self.phase = Phase::Seeking,
            Phase::Inside(value) if self.stack.len() < value => self.phase = Phase::Done,
            _ => {}
        }
        Ok(())
    }

    /// Text of the element that is innermost now.
    fn text(&mut self, text: &str) {
        let depth = self.stack.len();
        if let Some((field, _)) = self.field.as_mut().filter(|(_, at)| *at == depth) {
            if field.len().saturating_add(text.len()) > MAX_FIELD {
                self.spoiled = true;
            } else {
                field.push_str(text);
            }
        }
    }

    /// A reference in the text: a character by its number, or one of XML's own five. Any other
    /// is not looked up anywhere, and the field it stands in is not taken.
    fn reference(&mut self, reference: &BytesRef<'_>) {
        if self.field.is_none() {
            return;
        }
        let mut one = [0u8; 4];
        match reference.resolve_char_ref() {
            Ok(Some(c)) => self.text(c.encode_utf8(&mut one)),
            Ok(None) => match resolve_xml_entity(reference) {
                Some(text) => self.text(text),
                None => self.spoiled = true,
            },
            Err(_) => self.spoiled = true,
        }
    }
}

/// The list of Favorites in the document `xml`: its entries as they are written, in their
/// order. `Ok(None)` when the document has no such list.
pub(super) fn favorites_of(xml: &str) -> Result<Option<Listed>, Malformed> {
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    (config.check_end_names, config.allow_unmatched_ends, config.expand_empty_elements) = (true, false, false);
    let mut walk = Walk {
        phase: Phase::Seeking,
        stack: Vec::new(),
        field: None,
        spoiled: false,
        entry: None,
        setting: Default::default(),
        listed: Listed::default(),
    };
    // Every event takes at least a byte of the document, which bounds the loop.
    for _ in 0..=xml.len() {
        match reader.read_event().map_err(|_| Malformed)? {
            Event::Start(element) => walk.open(AsRef::<str>::as_ref(&element.local_name()).as_bytes())?,
            Event::Empty(element) => {
                walk.open(AsRef::<str>::as_ref(&element.local_name()).as_bytes())?;
                walk.close()?;
            }
            Event::End(_) => walk.close()?,
            Event::Text(text) => walk.text(&text.xml10_content()),
            Event::CData(text) => walk.text(&text.xml10_content()),
            Event::GeneralRef(reference) => walk.reference(&reference),
            // A document that brings a DTD along is not one this reads.
            Event::DocType(_) => return Err(Malformed),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
        if walk.phase == Phase::Done {
            return Ok(Some(walk.listed));
        }
    }
    // A list that the document ends in the middle of is not the list.
    if matches!(walk.phase, Phase::Inside(_)) { Err(Malformed) } else { Ok(None) }
}
