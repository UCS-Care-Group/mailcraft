// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `FindItem` ([MS-OXWSSRCH]): one folder's messages that match a search, newest first, a page
//! at a time (#1725: More results on the server).
//!
//! **What is asked.** The search is the part of the owner's query a server can be asked
//! ([`mailcraft_model::search::Query::for_server`]), written one of two ways:
//!
//! - a `QueryString` ([`query_string`]): the terms in the syntax Microsoft documents for it
//!   (Advanced Query Syntax), `from:`, `to:`, `cc:`, `subject:`, `body:`, `attachment:`,
//!   `hasattachment:true` and `importance:high` or `low`, joined by `AND`, which the server
//!   answers from its content index (Exchange Server 2010 and later);
//! - a `Restriction` ([`restriction`]) for a server that has no `QueryString`, or takes one and
//!   doesn't look at it: `Contains` on the subject, the body and the people a message names,
//!   `IsEqualTo` for the two marks. The body and the importance are named by their MAPI
//!   properties (`PidTagBody` and `PidTagImportance` of [MS-OXPROPS], as an `ExtendedFieldURI`),
//!   which every server has, where Grommunio knows neither `item:Body` nor the importance by
//!   its name in a restriction (#1725). It can't say "in an attachment", so a search with
//!   `attachment:` is not one it is asked.
//!
//! Every word goes as text: a word that could be read as syntax is written in quotes, with its
//! own quotes, backslashes and control characters taken out, and a `Constant` is an attribute's
//! value, escaped as every attribute is.
//!
//! **What comes back** is a list's fields of each item ([`crate::items::read_item`]), without
//! the recipients (`FindItem` doesn't give them), capped as it is read; an item that can't be
//! read is passed over, since nothing is lost by it. Nothing of the search or of what was found
//! is in debug output.

use mailcraft_model::Importance;
use mailcraft_model::search::{ServerField, ServerTerm};
use mailcraft_model::sync::GraphMessage;

use crate::error::Error;
use crate::items::{Shape, read_item};
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};

/// Most items one page asks for, and most that are read of one whatever the server sends.
pub const MAX_FOUND: u32 = 100;
/// The longest `QueryString` sent, in bytes.
const MAX_QUERY: usize = 2_000;
/// The longest text of one `Constant`, in characters.
const MAX_CONSTANT: usize = 256;

/// How the search is written.
#[derive(Clone, PartialEq, Eq)]
pub enum Search {
    /// A `QueryString`.
    Query(String),
    /// A `Restriction`: the element inside `m:Restriction`.
    Restriction(Element),
}

/// The request `FindItem` of one folder.
#[derive(Clone, PartialEq, Eq)]
pub struct FindItem {
    folder: String,
    search: Search,
    offset: u32,
    max: u32,
    shape: Shape,
}

impl std::fmt::Debug for FindItem {
    /// Nothing of the search or the folder: how it is asked.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindItem")
            .field("query", &matches!(self.search, Search::Query(_)))
            .field("offset", &self.offset)
            .field("max", &self.max)
            .field("shape", &self.shape)
            .finish()
    }
}

/// One page of what a folder has for a search.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FoundPage {
    /// The messages, in the server's order (the folder is not said: the caller knows it).
    pub messages: Vec<GraphMessage>,
    /// The page reaches the last item the folder has for the search.
    pub last: bool,
    /// Where the next page begins.
    pub next_offset: u32,
}

impl std::fmt::Debug for FoundPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoundPage").field("messages", &self.messages.len()).field("last", &self.last).finish()
    }
}

impl FindItem {
    /// Searches folder `folder` (its server id) from the item at `offset`, `max` items at the most
    /// (1 to [`MAX_FOUND`]).
    pub fn new(folder: &str, search: Search, offset: u32, max: u32, shape: Shape) -> Self {
        Self { folder: folder.to_owned(), search, offset, max: max.clamp(1, MAX_FOUND), shape }
    }

    /// The operation as the schema has it, in the schema's order: `ItemShape`, the page
    /// (`IndexedPageItemView`), `Restriction`, `SortOrder` (newest first), `ParentFolderIds`,
    /// `QueryString`.
    pub fn element(&self) -> Element {
        let uri = |name: &str| Element::new("t:FieldURI").attr("FieldURI", name);
        let mut extra = FIELDS.iter().fold(Element::new("t:AdditionalProperties"), |all, name| all.child(uri(name)));
        if self.shape == Shape::Full {
            extra = extra.child(uri("item:Preview")).child(uri("item:Flag"));
        }
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(extra);
        let page = Element::new("m:IndexedPageItemView")
            .attr("MaxEntriesReturned", &self.max.to_string())
            .attr("Offset", &self.offset.to_string())
            .attr("BasePoint", "Beginning");
        let mut operation = Element::new("m:FindItem").attr("Traversal", "Shallow").child(shape).child(page);
        if let Search::Restriction(restriction) = &self.search {
            operation = operation.child(Element::new("m:Restriction").child(restriction.clone()));
        }
        let newest = Element::new("t:FieldOrder").attr("Order", "Descending").child(uri("item:DateTimeReceived"));
        operation = operation
            .child(Element::new("m:SortOrder").child(newest))
            .child(Element::new("m:ParentFolderIds").child(Element::new("t:FolderId").attr("Id", &self.folder)));
        match &self.search {
            Search::Query(query) => operation.child(Element::new("m:QueryString").text(query)),
            Search::Restriction(_) => operation,
        }
    }

    /// The server's answer: the items of `RootFolder` that can be read, at most [`MAX_FOUND`], and
    /// where the next page begins. A server that doesn't say whether this was the last page is
    /// taken to have more only when it gave a full page.
    pub fn answer(&self, reply: &Reply) -> Result<FoundPage, Error> {
        let messages = reply.messages("FindItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let root = node.child_in(MESSAGES, "RootFolder").ok_or(Error::Unexpected)?;
        let most = usize::try_from(MAX_FOUND).unwrap_or(usize::MAX);
        let (mut found, mut listed) = (Vec::new(), 0u32);
        if let Some(items) = root.child_in(TYPES, "Items") {
            for item in items.children.iter().filter(|c| *c.namespace == *TYPES).take(most) {
                listed = listed.saturating_add(1);
                found.extend(read_item(item));
            }
        }
        // (An `xs:boolean`: "1" and "0" as well.)
        let last = match root.attribute("IncludesLastItemInRange").map(str::trim) {
            Some("true" | "1") => true,
            Some("false" | "0") => false,
            _ => listed < self.max,
        };
        // Where the server says the next page begins, when that is further on; else past what
        // this page listed. A page that listed nothing has nothing after it.
        let after = self.offset.saturating_add(listed);
        let said = root.attribute("IndexedPagingOffset").and_then(|n| n.trim().parse::<u32>().ok()).filter(|next| *next > self.offset);
        Ok(FoundPage { messages: found, last: last || listed == 0, next_offset: said.unwrap_or(after) })
    }
}

/// What a list shows of an item, asked for by name: `FindItem`'s own shapes differ from one
/// kind of item and one server to the next (#1457).
const FIELDS: [&str; 11] = [
    "item:Subject",
    "item:DateTimeReceived",
    "item:DateTimeSent",
    "item:Size",
    "item:Importance",
    "item:HasAttachments",
    "item:Categories",
    "item:IsDraft",
    "message:From",
    "message:IsRead",
    "message:InternetMessageId",
];

/// A word as it is sent: no quotes, backslashes or control characters, which are syntax or
/// nothing, and at most [`MAX_CONSTANT`] characters.
fn clean(word: &str) -> String {
    word.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '\\')).take(MAX_CONSTANT).collect()
}

/// The words of a term as one text.
fn text_of(words: &[String]) -> String {
    let words: Vec<String> = words.iter().map(|word| clean(word)).filter(|word| !word.trim().is_empty()).collect();
    words.join(" ").chars().take(MAX_CONSTANT).collect()
}

/// The `QueryString` for `terms`; `None` when there is nothing of them to send, or more than a
/// query takes.
pub fn query_string(terms: &[ServerTerm]) -> Option<String> {
    let clauses: Vec<String> = terms.iter().filter_map(clause).collect();
    let query = clauses.join(" AND ");
    (!clauses.is_empty() && clauses.len() == terms.len() && query.len() <= MAX_QUERY).then_some(query)
}

/// One term as a clause of the query syntax.
fn clause(term: &ServerTerm) -> Option<String> {
    match term {
        ServerTerm::Words { field, words } => {
            let property = match field {
                None => "",
                Some(ServerField::From) => "from:",
                Some(ServerField::To) => "to:",
                Some(ServerField::Cc) => "cc:",
                Some(ServerField::Subject) => "subject:",
                Some(ServerField::Body) => "body:",
                Some(ServerField::Attachment) => "attachment:",
            };
            let text = text_of(words);
            if text.is_empty() {
                return None;
            }
            // One word of letters and digits goes as it is, unless it is an operator of the
            // syntax; anything else is text in quotes.
            let plain = text.chars().all(char::is_alphanumeric) && !matches!(text.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT" | "NEAR");
            Some(if plain { format!("{property}{text}") } else { format!("{property}\"{text}\"") })
        }
        ServerTerm::HasAttachment => Some("hasattachment:true".into()),
        ServerTerm::Importance(Importance::High) => Some("importance:high".into()),
        ServerTerm::Importance(Importance::Low) => Some("importance:low".into()),
        ServerTerm::Importance(Importance::Normal) => None,
    }
}

/// The element inside `Restriction` for `terms`; `None` when one of them can't be said this way
/// (`attachment:`), or there is nothing to say.
pub fn restriction(terms: &[ServerTerm]) -> Option<Element> {
    let mut parts: Vec<Element> = terms.iter().map(part).collect::<Option<_>>()?;
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(parts.into_iter().fold(Element::new("t:And"), Element::child)),
    }
}

/// A property a restriction looks at: by its name in the schema, or by its MAPI tag and type.
#[derive(Clone, Copy)]
enum Property {
    Named(&'static str),
    Tagged(&'static str, &'static str),
}

/// The text of a message (`PidTagBody`) and its importance (`PidTagImportance`: 0 low, 1 normal,
/// 2 high), [MS-OXPROPS].
const BODY: Property = Property::Tagged("0x1000", "String");
const IMPORTANCE: Property = Property::Tagged("0x0017", "Integer");

impl Property {
    fn element(self) -> Element {
        match self {
            Property::Named(name) => Element::new("t:FieldURI").attr("FieldURI", name),
            Property::Tagged(tag, kind) => Element::new("t:ExtendedFieldURI").attr("PropertyTag", tag).attr("PropertyType", kind),
        }
    }
}

fn contains(property: Property, text: &str) -> Element {
    Element::new("t:Contains")
        .attr("ContainmentMode", "Substring")
        .attr("ContainmentComparison", "IgnoreCase")
        .child(property.element())
        .child(Element::new("t:Constant").attr("Value", text))
}

fn equals(property: Property, value: &str) -> Element {
    Element::new("t:IsEqualTo")
        .child(property.element())
        .child(Element::new("t:FieldURIOrConstant").child(Element::new("t:Constant").attr("Value", value)))
}

const FROM: Property = Property::Named("message:From");
const TO: Property = Property::Named("item:DisplayTo");
const CC: Property = Property::Named("item:DisplayCc");

/// One term as a part of a restriction.
fn part(term: &ServerTerm) -> Option<Element> {
    match term {
        ServerTerm::Words { field, words } => {
            let text = text_of(words);
            if text.is_empty() {
                return None;
            }
            match field {
                // Wherever a search looks by itself: the subject, the body and the people.
                None => Some(
                    [Property::Named("item:Subject"), BODY, FROM, TO, CC]
                        .into_iter()
                        .fold(Element::new("t:Or"), |any, property| any.child(contains(property, &text))),
                ),
                Some(ServerField::From) => Some(contains(FROM, &text)),
                Some(ServerField::To) => Some(contains(TO, &text)),
                Some(ServerField::Cc) => Some(contains(CC, &text)),
                Some(ServerField::Subject) => Some(contains(Property::Named("item:Subject"), &text)),
                Some(ServerField::Body) => Some(contains(BODY, &text)),
                Some(ServerField::Attachment) => None,
            }
        }
        ServerTerm::HasAttachment => Some(equals(Property::Named("item:HasAttachments"), "true")),
        ServerTerm::Importance(Importance::High) => Some(equals(IMPORTANCE, "2")),
        ServerTerm::Importance(Importance::Low) => Some(equals(IMPORTANCE, "0")),
        ServerTerm::Importance(Importance::Normal) => None,
    }
}
