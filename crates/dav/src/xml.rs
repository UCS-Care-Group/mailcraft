// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Namespace-aware RFC 4918 multistatus documents, with finite size, depth and node budgets.
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 32;
const MAX_NAME: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidXml;
impl std::fmt::Display for InvalidXml {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid or oversized DAV XML")
    }
}
impl std::error::Error for InvalidXml {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub namespace: String,
    pub name: String,
    pub component_name: Option<String>,
    pub text: String,
    pub children: Vec<Node>,
}
impl Node {
    pub fn is(&self, namespace: &str, name: &str) -> bool {
        self.namespace == namespace && self.name == name
    }
    pub fn child(&self, namespace: &str, name: &str) -> Option<&Self> {
        self.children.iter().find(|n| n.is(namespace, name))
    }
    pub fn value(&self, namespace: &str, name: &str) -> Option<&str> {
        self.child(namespace, name).map(|n| n.text.trim())
    }
}

fn append(stack: &mut [Node], text: &str) -> Result<(), InvalidXml> {
    if !valid_characters(text) {
        return Err(InvalidXml);
    }
    let node = stack.last_mut().ok_or(InvalidXml)?;
    // Text outside the single document element must be XML whitespace.
    if node.name.is_empty() && !text.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n')) {
        return Err(InvalidXml);
    }
    node.text.push_str(text);
    Ok(())
}

fn valid_characters(text: &str) -> bool {
    text.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
}

/// Parse one UTF-8 DAV multistatus. DTDs, unknown entities, undefined prefixes, malformed
/// trees and exceeded budgets fail the whole response; a partial tree is never returned.
pub fn parse(bytes: &[u8]) -> Result<Node, InvalidXml> {
    if bytes.len() > MAX_BYTES {
        return Err(InvalidXml);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| InvalidXml)?;
    if !valid_characters(text) {
        return Err(InvalidXml);
    }
    let mut reader = NsReader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut stack = vec![Node::default()];
    let mut count = 0usize;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(|_| InvalidXml)?;
        let namespace = match namespace {
            ResolveResult::Bound(n) => {
                let name = AsRef::<str>::as_ref(&n);
                if name.len() > MAX_NAME {
                    return Err(InvalidXml);
                }
                name.to_string()
            }
            ResolveResult::Unbound => String::new(),
            ResolveResult::Unknown(_) => return Err(InvalidXml),
        };
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                count += 1;
                if count > MAX_NODES || stack.len() > MAX_DEPTH {
                    return Err(InvalidXml);
                }
                let mut component_name = None;
                for a in e.attributes() {
                    let a = a.map_err(|_| InvalidXml)?;
                    let value = a.normalized_value(XmlVersion::Implicit1_0).map_err(|_| InvalidXml)?;
                    if !valid_characters(&value) {
                        return Err(InvalidXml);
                    }
                    if a.key.as_ref() == "name" {
                        if value.len() > MAX_NAME {
                            return Err(InvalidXml);
                        }
                        component_name = Some(value.into_owned());
                    }
                }
                let local = e.local_name();
                let name = AsRef::<str>::as_ref(&local);
                if name.len() > MAX_NAME {
                    return Err(InvalidXml);
                }
                let node = Node { namespace, name: name.to_string(), component_name, ..Default::default() };
                if stack.len() == 1 && !stack.first().ok_or(InvalidXml)?.children.is_empty() {
                    return Err(InvalidXml);
                }
                if empty {
                    stack.last_mut().ok_or(InvalidXml)?.children.push(node);
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(InvalidXml);
                }
                let node = stack.pop().ok_or(InvalidXml)?;
                stack.last_mut().ok_or(InvalidXml)?.children.push(node);
            }
            Event::Text(t) => append(&mut stack, &t.xml10_content())?,
            Event::CData(t) => {
                if stack.len() == 1 {
                    return Err(InvalidXml);
                }
                append(&mut stack, &t.xml10_content())?;
            }
            Event::GeneralRef(r) => {
                let value = if let Some(c) = r.resolve_char_ref().map_err(|_| InvalidXml)? {
                    c.to_string()
                } else {
                    resolve_xml_entity(&r).ok_or(InvalidXml)?.to_string()
                };
                append(&mut stack, &value)?;
            }
            Event::DocType(_) => return Err(InvalidXml),
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
        }
    }
    if stack.len() != 1 {
        return Err(InvalidXml);
    }
    let mut document = stack.pop().ok_or(InvalidXml)?;
    if document.children.len() != 1 {
        return Err(InvalidXml);
    }
    let root = document.children.pop().ok_or(InvalidXml)?;
    if !root.is(DAV, "multistatus") {
        return Err(InvalidXml);
    }
    Ok(root)
}

/// Properties explicitly returned in a successful propstat, never failed/missing properties.
/// A missing property says nothing about whether a resource was deleted.
pub fn properties(response: &Node) -> impl Iterator<Item = &Node> {
    response
        .children
        .iter()
        .filter(|n| {
            n.is(DAV, "propstat")
                && n.value(DAV, "status").is_some_and(|s| {
                    let mut words = s.split_whitespace();
                    words.next().is_some_and(|v| v.starts_with("HTTP/")) && words.next() == Some("200")
                })
        })
        .filter_map(|n| n.child(DAV, "prop"))
        .flat_map(|n| &n.children)
}

#[cfg(test)]
#[path = "xml_tests.rs"]
mod tests;
