// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `SyncFolderItems` and `GetItem` ([MS-OXWSSYNC], [MS-OXWSCORE], [MS-OXWSMSG]): a folder's
//! messages as headers and a preview first, and one message whole as its MIME content.
//!
//! **An item's id** is its `Id`, which stays the same while the item is where it is; the `ChangeKey`
//! changes with every edit, so it is kept apart (`ServerMessage::change_key`) and a message is
//! matched by its id alone. An item's server id changes when it is moved to another folder, so
//! a message moved on the server comes as a deletion and a new message.
//!
//! What a server doesn't say is left as it was (E7): an answer without a preview, a flag or the
//! categories reads as a message without them. Everything is capped as the answer is read.

use mailcraft_model::sync::{GraphMessage, ReadState, ServerMessage};
use mailcraft_model::{Address, FlagStatus, Importance};

use crate::b64;
use crate::error::Error;
use crate::folder::valid_id;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::{MAX_MIME_TEXT, Node};

/// Most changes one page asks for (the schema's own limit is 512).
pub const MAX_CHANGES: u32 = 512;
/// The most changes one page holds that are taken in: what the model takes in of a page.
const MAX_PAGE: usize = mailcraft_model::sync::MAX_EWS_PAGE_MESSAGES;
/// Longest subject, `Message-ID`, preview and name kept, in characters; the most people a
/// message names in one list.
const MAX_SUBJECT: usize = 998;
const MAX_PREVIEW: usize = 256;
const MAX_NAME: usize = 256;
const MAX_ADDRESS: usize = 320;
const MAX_PEOPLE: usize = 200;
const MAX_CATEGORIES: usize = 50;
const MAX_CATEGORY: usize = 255;

/// Which properties `SyncFolderItems` asks for besides the default ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The recipients, the categories, the preview and the flag.
    Full,
    /// The recipients and the categories alone: for a server that doesn't know the preview and the
    /// flag (it answers `ErrorInvalidPropertyRequest`, say), which are then left out.
    Lean,
}

/// The request `SyncFolderItems` of one folder.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncFolderItems {
    folder: String,
    state: String,
    max: u32,
    shape: Shape,
}

impl std::fmt::Debug for SyncFolderItems {
    /// Nothing the server said: counts and flags.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncFolderItems").field("max", &self.max).field("shape", &self.shape).field("state", &!self.state.is_empty()).finish()
    }
}

/// One page of a folder's changes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ItemsPage {
    /// The state to go on from.
    pub state: String,
    /// The page reaches the last change the server has (`IncludesLastItemInRange`).
    pub last: bool,
    /// Messages created or updated, whole as a list shows them (the folder is not said: the caller
    /// knows it).
    pub messages: Vec<GraphMessage>,
    /// The ids of the items deleted.
    pub removed: Vec<String>,
    /// Items whose read state alone changed.
    pub read: Vec<ReadState>,
    /// How many `ReadFlagChange`s said no read state and were passed over (they carry nothing to lose).
    pub read_unsaid: usize,
}

impl std::fmt::Debug for ItemsPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemsPage")
            .field("last", &self.last)
            .field("messages", &self.messages.len())
            .field("removed", &self.removed.len())
            .field("read", &self.read.len())
            .finish()
    }
}

impl SyncFolderItems {
    /// Reads folder `folder` (its server id) from `state` (empty: from its start), `max` changes
    /// at the most (1 to 512).
    pub fn new(folder: &str, state: &str, max: u32, shape: Shape) -> Self {
        Self { folder: folder.to_owned(), state: state.to_owned(), max: max.clamp(1, MAX_CHANGES), shape }
    }

    /// The operation as the schema has it: `ItemShape`, `SyncFolderId`, `SyncState`,
    /// `MaxChangesReturned`, `SyncScope`.
    pub fn element(&self) -> Element {
        let uri = |name: &str| Element::new("t:FieldURI").attr("FieldURI", name);
        let mut extra = Element::new("t:AdditionalProperties")
            .child(uri("message:ToRecipients"))
            .child(uri("message:CcRecipients"))
            .child(uri("item:Categories"));
        if self.shape == Shape::Full {
            extra = extra.child(uri("item:Preview")).child(uri("item:Flag"));
        }
        let shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("Default")).child(extra);
        let folder = Element::new("m:SyncFolderId").child(Element::new("t:FolderId").attr("Id", &self.folder));
        let mut operation = Element::new("m:SyncFolderItems").child(shape).child(folder);
        if !self.state.is_empty() {
            operation = operation.child(Element::new("m:SyncState").text(&self.state));
        }
        operation.child(Element::new("m:MaxChangesReturned").text(&self.max.to_string())).child(Element::new("m:SyncScope").text("NormalItems"))
    }

    /// The server's answer. A state the server no longer knows is [`Error::Server`] of
    /// `InvalidSyncState`.
    ///
    /// **A page is whole or it is an error.** The answer's new state says "everything up to here
    /// has been taken in", so a change that was left out would be lost for good. Every change is
    /// taken in, however many the server gave (the size of the answer and its nodes are held
    /// already, as the answer is parsed, and a server that ignores `MaxChangesReturned` has said its
    /// state covers them all); one that can't be taken in makes the page [`Error::Unreadable`], with
    /// nothing of it handed on and no state taken from it, so the folder stays where it was and is
    /// read again next time. Only a change with no usable id (a `Delete`, a `ReadFlagChange`, an
    /// item) or no item (a `Create` or an `Update` with nothing in it) can't be taken in: whatever
    /// else an item lacks (its read state, importance, categories, preview, times) is read as
    /// nothing said, as a message without it. A `ReadFlagChange` that doesn't say which state it
    /// is carries nothing to lose, and is passed over. A kind of change this program doesn't know
    /// is passed over too (E7).
    pub fn answer(&self, reply: &Reply) -> Result<ItemsPage, Error> {
        let messages = reply.messages("SyncFolderItems")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let state = node.child_in(MESSAGES, "SyncState").map(|n| n.text.trim().to_owned()).filter(|s| !s.is_empty()).ok_or(Error::Unexpected)?;
        let last = node.child_in(MESSAGES, "IncludesLastItemInRange").is_none_or(|n| n.text.trim() != "false");
        let mut page = ItemsPage { state, last, ..ItemsPage::default() };
        let Some(changes) = node.child_in(MESSAGES, "Changes") else { return Ok(page) };
        // (What the model takes in of one page: more than that is a page that can't be taken in.)
        if changes.children.len() > MAX_PAGE {
            return Err(Error::Unreadable);
        }
        for change in changes.children.iter().filter(|c| *c.namespace == *TYPES) {
            let id = || change.child_in(TYPES, "ItemId").and_then(|id| id.attribute("Id")).filter(|id| valid_id(id));
            match change.name.as_str() {
                "Create" | "Update" => {
                    let item = change.children.iter().find(|c| *c.namespace == *TYPES).and_then(read_item);
                    page.messages.push(item.ok_or(Error::Unreadable)?);
                }
                "Delete" => page.removed.push(id().ok_or(Error::Unreadable)?.to_owned()),
                "ReadFlagChange" => {
                    let id = id().ok_or(Error::Unreadable)?;
                    match change.child_in(TYPES, "IsRead").map(|n| n.text.trim().eq_ignore_ascii_case("true")) {
                        Some(read) => page.read.push(ReadState { id: id.to_owned(), read }),
                        None => page.read_unsaid = page.read_unsaid.saturating_add(1),
                    }
                }
                _ => {}
            }
        }
        Ok(page)
    }
}

/// `text` on one line, `most` characters at the most.
fn line(text: &str, most: usize) -> String {
    let mut out = String::new();
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= most {
            break;
        }
    }
    out.chars().take(most).collect()
}

/// The time an element says (`2013-08-29T15:22:10Z`); 0 when it says none that can be read.
fn time(node: &Node, name: &str) -> i64 {
    node.child_in(TYPES, name).and_then(|n| mailcraft_time::iso_to_timestamp(n.text.trim(), 0)).unwrap_or(0)
}

fn flag(node: &Node, name: &str) -> bool {
    node.child_in(TYPES, name).is_some_and(|n| n.text.trim().eq_ignore_ascii_case("true"))
}

/// A `Mailbox` element as an address. An address in the server's own directory (routing type
/// `EX`) is not an address anybody can write to: only the name is kept.
fn mailbox(node: &Node) -> Option<Address> {
    let name = line(node.child_in(TYPES, "Name").map_or("", |n| n.text.as_str()), MAX_NAME);
    let routing = node.child_in(TYPES, "RoutingType").map_or("SMTP", |n| n.text.trim());
    let email = if routing.eq_ignore_ascii_case("SMTP") {
        line(node.child_in(TYPES, "EmailAddress").map_or("", |n| n.text.as_str()), MAX_ADDRESS)
    } else {
        String::new()
    };
    (!name.is_empty() || !email.is_empty()).then_some(Address { name, email })
}

/// The people in a list element (`ToRecipients`), or the one in `From`.
fn people(node: &Node, name: &str) -> Vec<Address> {
    let Some(list) = node.child_in(TYPES, name) else { return Vec::new() };
    let one = |holder: &Node| if holder.is(TYPES, "Mailbox") { mailbox(holder) } else { holder.child_in(TYPES, "Mailbox").and_then(mailbox) };
    list.children.iter().take(MAX_PEOPLE).filter_map(one).collect()
}

/// The message in an item element, when it has an id.
fn read_item(item: &Node) -> Option<GraphMessage> {
    let id_node = item.child_in(TYPES, "ItemId")?;
    let id = id_node.attribute("Id").filter(|id| valid_id(id))?;
    let change_key = id_node.attribute("ChangeKey").filter(|key| valid_id(key)).unwrap_or_default();
    let importance = match item.child_in(TYPES, "Importance").map_or("", |n| n.text.trim()) {
        "Low" => Importance::Low,
        "High" => Importance::High,
        _ => Importance::Normal,
    };
    let flag_node = item.child_in(TYPES, "Flag");
    let status = match flag_node.and_then(|f| f.child_in(TYPES, "FlagStatus")).map_or("", |n| n.text.trim()) {
        "Flagged" => FlagStatus::Flagged,
        "Complete" => FlagStatus::Complete,
        _ => FlagStatus::None,
    };
    let when = |name: &str| flag_node.and_then(|f| f.child_in(TYPES, name)).and_then(|n| mailcraft_time::iso_to_timestamp(n.text.trim(), 0));
    let categories = item
        .child_in(TYPES, "Categories")
        .map(|c| c.children.iter().take(MAX_CATEGORIES).map(|s| line(&s.text, MAX_CATEGORY)).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let message_id = line(item.child_in(TYPES, "InternetMessageId").map_or("", |n| n.text.as_str()), MAX_SUBJECT);
    Some(GraphMessage {
        server: ServerMessage {
            id: id.to_owned(),
            read: flag(item, "IsRead"),
            flag: status,
            categories,
            importance,
            size: item.child_in(TYPES, "Size").and_then(|n| n.text.trim().parse().ok()).unwrap_or(0),
            flag_start: when("StartDate").filter(|_| status != FlagStatus::None),
            flag_due: when("DueDate").filter(|_| status != FlagStatus::None),
            flag_completed: when("CompleteDate").filter(|_| status == FlagStatus::Complete),
            change_key: change_key.to_owned(),
            ..ServerMessage::default()
        },
        message_id: message_id.trim_start_matches('<').trim_end_matches('>').to_owned(),
        subject: line(item.child_in(TYPES, "Subject").map_or("", |n| n.text.as_str()), MAX_SUBJECT),
        from: people(item, "From").into_iter().next().unwrap_or_default(),
        to: people(item, "ToRecipients"),
        cc: people(item, "CcRecipients"),
        sent: time(item, "DateTimeSent"),
        received: time(item, "DateTimeReceived"),
        preview: line(item.child_in(TYPES, "Preview").map_or("", |n| n.text.as_str()), MAX_PREVIEW),
        has_attachments: flag(item, "HasAttachments"),
        draft: item.child_in(TYPES, "IsDraft").map(|n| n.text.trim().eq_ignore_ascii_case("true")),
    })
}

/// The request `GetItem` for one message's MIME content.
#[derive(Clone, PartialEq, Eq)]
pub struct GetItemMime {
    id: String,
}

impl std::fmt::Debug for GetItemMime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GetItemMime(..)")
    }
}

impl GetItemMime {
    /// Asks for the item with this id (a server id from a sync).
    pub fn new(id: &str) -> Self {
        Self { id: id.to_owned() }
    }

    /// The operation as the schema has it: `ItemShape` (`IdOnly` and `IncludeMimeContent`), `ItemIds`.
    pub fn element(&self) -> Element {
        let shape =
            Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("IdOnly")).child(Element::new("t:IncludeMimeContent").text("true"));
        let ids = Element::new("m:ItemIds").child(Element::new("t:ItemId").attr("Id", &self.id));
        Element::new("m:GetItem").child(shape).child(ids)
    }

    /// The message as the server holds it (its MIME), or why not: [`Error::Server`] of
    /// `ItemNotFound` for an item it no longer has. A message the answer holds more of than
    /// can be read (over [`MAX_MIME_TEXT`] of base64) is [`Error::TooLarge`].
    pub fn answer(&self, reply: &Reply) -> Result<Vec<u8>, Error> {
        let messages = reply.messages("GetItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
        let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
        let content = item.child_in(TYPES, "MimeContent").ok_or(Error::Unexpected)?;
        if content.text.is_empty() {
            // (Either nothing, or more than the reader keeps.)
            return Err(Error::TooLarge);
        }
        let bytes = b64::decode(&content.text, MAX_MIME_TEXT).ok_or(Error::Unexpected)?;
        if bytes.is_empty() { Err(Error::Unexpected) } else { Ok(bytes) }
    }
}
