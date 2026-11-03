// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What Exchange Web Services answers, read into a small tree: elements by their local names,
//! their attributes, and the text directly inside them.
//!
//! An answer is hostile like anything a server sends, and it may be anybody's: a SOAP fault, a
//! page that isn't the service at all, megabytes of nesting. So the reader takes no DTD and
//! expands no entity (a document with a `<!DOCTYPE` is not read at all; a reference that isn't a
//! character or one of XML's own five spoils the text or the attribute it stands in, which then
//! reads as empty), and everything is capped: how deep elements nest, how many there are, how
//! long a name, an attribute and a text may be, and how much memory the whole tree may cost (a
//! small multiple of the body: see [`budget`]). What is over a cap is left out or reads as empty;
//! nesting that is too deep, more elements than any answer has, a tree that costs too much, and
//! a prefix or a namespace declaration that is over its cap (which cannot be left out without
//! changing what the elements around it are) end the reading. Control characters XML forbids
//! never reach the tree. A namespace is kept once for the document and the nodes share it.
//!
//! Elements are told by their local names. The prefixes are the document's own choice (the
//! service writes `t:` and `m:`, `s:` for the envelope), and what is read is found by where it
//! is under the operation's own answer, never by a prefix.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};

/// Deepest the elements may nest.
pub const MAX_DEPTH: usize = 40;
/// Most elements in one answer.
pub const MAX_NODES: usize = 100_000;
/// Longest name of an element or an attribute that is kept (a longer one is no name anybody
/// asks for), most attributes read of one element, and the longest value of one.
const MAX_NAME: usize = 64;
const MAX_ATTRIBUTES: usize = 12;
const MAX_ATTRIBUTE: usize = 2_048;
/// Most attributes and declarations one start tag may have: more is no tag of the service.
const MAX_TAG_ATTRIBUTES: usize = 64;
/// Longest text kept of one element (bytes): an id, a name, a property in base64.
pub const MAX_TEXT: usize = 8_192;
/// Longest text kept of a `MimeContent` (bytes): a whole message in base64, which the answer to
/// `GetItem` carries in one element. (Charged at its real size, like any text.)
pub const MAX_MIME_TEXT: usize = 16 * 1024 * 1024;
/// Longest text kept of a `SyncState` (bytes): the server's own record of what a folder's client
/// has, which grows with the folder (a compressed list of the items it has told of): a large
/// folder's runs to some tens of kilobytes.
pub const MAX_SYNC_STATE: usize = 1024 * 1024;
/// What an element costs besides its name, attributes and text: the size of a node and the
/// allocator's share of the block that holds it in its parent's `children`. (The vectors of a
/// node are shrunk to fit when it is closed, so that a vector's spare room is no further cost.)
const NODE_COST: usize = std::mem::size_of::<Node>() + 48;
/// What the whole tree may cost, whatever the body weighs (bytes).
const MAX_BUDGET: usize = 64 * 1024 * 1024;

/// What a tree of a document of `body_len` bytes may cost: sixteen times the document and a
/// little more, never above 64 MiB. Every element costs a fixed amount and what it holds; a
/// namespace costs once however many elements it is declared on.
pub fn budget(body_len: usize) -> usize {
    body_len.saturating_mul(16).saturating_add(256 * 1024).min(MAX_BUDGET)
}

/// The answer isn't a document that can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the answer is not a document that can be read")
    }
}

/// One element.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// Its local name; empty for one whose name is longer than any that is asked for.
    pub name: String,
    /// The prefix its name is written with (`t` of `t:Folder`); empty for none. Nothing goes
    /// by it; see `namespace`.
    pub prefix: String,
    /// The namespaces it declares, by prefix (`xmlns:t="…"`; the empty prefix for `xmlns="…"`).
    pub namespaces: Vec<(String, Arc<str>)>,
    /// The namespace its prefix stands for where it is written, as it or an element around it
    /// declares it; empty when nothing does. Whoever has to hold a document to its namespaces
    /// (the stand-in, and the readers of the answers where it matters) goes by this, never by
    /// the prefix. Shared with every other element of the document that has the same one.
    pub namespace: Arc<str>,
    /// Its other attributes, by their local names.
    pub attributes: Vec<(String, String)>,
    /// The text directly inside it; empty when there is none, or it is longer than is kept, or
    /// it holds a reference to who knows what.
    pub text: String,
    /// The elements inside it.
    pub children: Vec<Node>,
}

impl fmt::Debug for Node {
    /// How many elements lie inside: a name, an attribute or a text may be anything a server
    /// chose, and may be somebody's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node").field("children", &self.children.len()).finish()
    }
}

impl Node {
    /// Whether this element is `name` of the namespace `space`.
    pub fn is(&self, space: &str, name: &str) -> bool {
        self.name == name && *self.namespace == *space
    }

    /// The first element `name` of the namespace `space` directly inside this one.
    pub fn child_in(&self, space: &str, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.is(space, name))
    }

    /// The first element called `name` directly inside this one.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The element at the end of `path`, each name directly inside the one before it.
    pub fn find(&self, path: &[&str]) -> Option<&Node> {
        path.iter().try_fold(self, |at, name| at.child(name))
    }

    /// Every element called `name` directly inside this one.
    pub fn all<'n>(&'n self, name: &'n str) -> impl Iterator<Item = &'n Node> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// What its attribute `name` says.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    /// The text of the element called `name` directly inside this one; empty when it has none.
    pub fn text_of(&self, name: &str) -> &str {
        self.child(name).map_or("", |child| child.text.trim())
    }
}

/// Whether XML 1.0 has a place for `c`: not the C0 controls but the tab, the line feed and the
/// carriage return, and not U+FFFE and U+FFFF.
fn allowed(c: char) -> bool {
    !((c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')) && c != '\u{FFFE}' && c != '\u{FFFF}'
}

/// `text` without what XML has no place for.
fn clean(text: &str) -> Cow<'_, str> {
    if text.chars().all(allowed) { Cow::Borrowed(text) } else { Cow::Owned(text.chars().filter(|c| allowed(*c)).collect()) }
}

/// An element that is being read: the node so far, and whether its text is still good.
struct Open {
    node: Node,
    spoiled: bool,
}

/// What a string of `len` bytes costs on the heap: a small block is not smaller than the
/// allocator's least (32 bytes), and nothing costs nothing but the empty string.
fn heap(len: usize) -> usize {
    if len == 0 { 0 } else { len.max(32) }
}

fn name_of(local: &str) -> String {
    if local.len() <= MAX_NAME { local.to_string() } else { String::new() }
}

struct Walk {
    stack: Vec<Open>,
    root: Option<Node>,
    nodes: usize,
    /// What the tree has cost so far, and what it may.
    spent: usize,
    budget: usize,
    /// The namespaces of the document, each once.
    spaces: HashSet<Arc<str>>,
}

impl Walk {
    fn charge(&mut self, bytes: usize) -> Result<(), Malformed> {
        self.spent = self.spent.saturating_add(bytes);
        if self.spent > self.budget { Err(Malformed) } else { Ok(()) }
    }

    /// The one `Arc` this document has for `uri`.
    fn space(&mut self, uri: &str) -> Result<Arc<str>, Malformed> {
        if let Some(have) = self.spaces.get(uri) {
            return Ok(Arc::clone(have));
        }
        self.charge(heap(uri.len()))?;
        let new: Arc<str> = Arc::from(uri);
        self.spaces.insert(Arc::clone(&new));
        Ok(new)
    }

    /// The element a start tag opens, with the attributes it has.
    fn opened(&mut self, start: &BytesStart<'_>) -> Result<Open, Malformed> {
        let prefix = match start.name().prefix() {
            // (A prefix over the cap would be taken for none, and an element in no namespace
            // for one in the default.)
            Some(prefix) if AsRef::<str>::as_ref(&prefix).len() > MAX_NAME => return Err(Malformed),
            Some(prefix) => AsRef::<str>::as_ref(&prefix).to_string(),
            None => String::new(),
        };
        let mut node = Node { name: name_of(AsRef::<str>::as_ref(&start.local_name())), prefix, ..Default::default() };
        self.charge(NODE_COST.saturating_add(heap(node.name.len())).saturating_add(heap(node.prefix.len())))?;
        for (seen, attribute) in start.attributes().enumerate() {
            if seen >= MAX_TAG_ATTRIBUTES {
                return Err(Malformed);
            }
            // A tag whose attributes can't be told apart is no tag.
            let attribute = attribute.map_err(|_| Malformed)?;
            let name = name_of(AsRef::<str>::as_ref(&attribute.key.local_name()));
            let declares = match attribute.key.as_ref() {
                "xmlns" => Some(String::new()),
                _ if attribute.key.prefix().is_some_and(|prefix| AsRef::<str>::as_ref(&prefix) == "xmlns") => {
                    Some(AsRef::<str>::as_ref(&attribute.key.local_name()).to_string())
                }
                _ => None,
            };
            let value = attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0);
            if let Some(declared) = declares {
                // `xmlns="…"` and `xmlns:t="…"` say what a prefix stands for. One that can't be
                // kept whole can't be left out: the elements below would be in another namespace.
                let value = value.map_err(|_| Malformed)?;
                if value.len() > MAX_ATTRIBUTE || declared.len() > MAX_NAME {
                    return Err(Malformed);
                }
                self.charge(heap(declared.len()).saturating_add(16))?;
                let uri = self.space(&clean(&value))?;
                node.namespaces.push((declared, uri));
                continue;
            }
            // (A value with a reference to who knows what in it is no value: left out.)
            let Ok(value) = value else { continue };
            if value.len() > MAX_ATTRIBUTE
                || name.is_empty()
                || node.attributes.len() >= MAX_ATTRIBUTES
                || node.attributes.iter().any(|(have, _)| *have == name)
            {
                continue;
            }
            let value = clean(&value).into_owned();
            self.charge(heap(name.len()).saturating_add(heap(value.len())).saturating_add(48))?;
            node.attributes.push((name, value));
        }
        Ok(Open { node, spoiled: false })
    }

    fn open(&mut self, start: &BytesStart<'_>) -> Result<(), Malformed> {
        self.nodes = self.nodes.saturating_add(1);
        // (One element at the top: a second one makes it no document.)
        if self.stack.len() >= MAX_DEPTH || self.nodes > MAX_NODES || (self.stack.is_empty() && self.root.is_some()) {
            return Err(Malformed);
        }
        let mut open = self.opened(start)?;
        let prefix = open.node.prefix.clone();
        let declared = |namespaces: &[(String, Arc<str>)]| namespaces.iter().find(|(have, _)| *have == prefix).map(|(_, uri)| Arc::clone(uri));
        let found = declared(&open.node.namespaces).or_else(|| self.stack.iter().rev().find_map(|above| declared(&above.node.namespaces)));
        open.node.namespace = match found {
            Some(uri) => uri,
            None => self.space("")?,
        };
        self.stack.push(open);
        Ok(())
    }

    fn close(&mut self) -> Result<(), Malformed> {
        let Some(mut done) = self.stack.pop() else { return Err(Malformed) };
        if done.spoiled {
            done.node.text.clear();
        }
        // (The room a vector grew by is not kept: a node is what it holds.)
        done.node.children.shrink_to_fit();
        done.node.namespaces.shrink_to_fit();
        done.node.attributes.shrink_to_fit();
        done.node.text.shrink_to_fit();
        match self.stack.last_mut() {
            Some(parent) => parent.node.children.push(done.node),
            None => self.root = Some(done.node),
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), Malformed> {
        let text = clean(text);
        let Some(open) = self.stack.last_mut() else { return Ok(()) };
        let cap = match open.node.name.as_str() {
            "MimeContent" => MAX_MIME_TEXT,
            "SyncState" => MAX_SYNC_STATE,
            _ => MAX_TEXT,
        };
        if open.node.text.len().saturating_add(text.len()) > cap {
            open.spoiled = true;
        } else if !open.spoiled {
            let before = heap(open.node.text.len());
            open.node.text.push_str(&text);
            let more = heap(open.node.text.len()).saturating_sub(before);
            self.charge(more)?;
        }
        Ok(())
    }

    /// A reference in the text: a character by its number, or one of XML's own five. Any other
    /// is not looked up anywhere, and the text it stands in is not taken.
    fn reference(&mut self, reference: &BytesRef<'_>) -> Result<(), Malformed> {
        let mut one = [0u8; 4];
        match reference.resolve_char_ref() {
            Ok(Some(c)) => self.text(c.encode_utf8(&mut one)),
            Ok(None) => match resolve_xml_entity(reference) {
                Some(text) => self.text(text),
                None => {
                    self.spoil();
                    Ok(())
                }
            },
            Err(_) => {
                self.spoil();
                Ok(())
            }
        }
    }

    fn spoil(&mut self) {
        if let Some(open) = self.stack.last_mut() {
            open.spoiled = true;
        }
    }
}

/// The document in `body` (UTF-8, with or without a byte order mark) as a tree: its one element
/// at the top.
pub fn parse(body: &[u8]) -> Result<Node, Malformed> {
    parse_with_cost(body).map(|(tree, _)| tree)
}

/// Like [`parse`], and what the tree cost against its [`budget`]: `(tree, cost)`.
pub fn parse_with_cost(body: &[u8]) -> Result<(Node, usize), Malformed> {
    let body = body.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(body);
    let xml = std::str::from_utf8(body).map_err(|_| Malformed)?;
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    (config.check_end_names, config.allow_unmatched_ends, config.expand_empty_elements) = (true, false, false);
    let mut walk = Walk { stack: Vec::new(), root: None, nodes: 0, spent: 0, budget: budget(body.len()), spaces: HashSet::new() };
    // Every event takes at least a byte of the document, which bounds the loop.
    for _ in 0..=xml.len() {
        match reader.read_event().map_err(|_| Malformed)? {
            Event::Start(element) => walk.open(&element)?,
            Event::Empty(element) => {
                walk.open(&element)?;
                walk.close()?;
            }
            Event::End(_) => walk.close()?,
            Event::Text(text) => walk.text(&text.xml10_content())?,
            Event::CData(text) => walk.text(&text.xml10_content())?,
            Event::GeneralRef(reference) => walk.reference(&reference)?,
            // A document that brings a DTD along is not one this reads.
            Event::DocType(_) => return Err(Malformed),
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    // A document that ends in the middle of an element is not the answer.
    if !walk.stack.is_empty() {
        return Err(Malformed);
    }
    let spent = walk.spent;
    walk.root.map(|tree| (tree, spent)).ok_or(Malformed)
}

/// `text` as it is written inside an attribute (or anywhere a line of a protocol could end): the
/// five characters XML has names for by those names, and nothing that could end a line or a
/// header: every control character (CR, LF, NUL and the tab too), and the characters XML 1.0
/// forbids (U+FFFE, U+FFFF).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// `text` as it is written inside an element: as [`escape`], except that the tab and the line
/// feed stay (a body of text keeps its lines) and a carriage return is written as `&#13;`, which
/// is not one on the wire. NUL and the other C0 controls, U+FFFE and U+FFFF are dropped.
/// (A `&str` holds no unpaired surrogate.)
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c if allowed(c) => out.push(c),
            _ => {}
        }
    }
    out
}
