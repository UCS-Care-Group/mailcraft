// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changes going up (#252, E6): `UpdateItem`, `MoveItem` and `DeleteItem` for messages, and
//! `CreateFolder`, `UpdateFolder`, `MoveFolder` and `DeleteFolder` for folders, each a request and
//! a reader of its answer ([MS-OXWSCORE], [MS-OXWSFOLD]).
//!
//! **Nothing here deletes for good.** A [`DeleteMode`] is `MoveToDeletedItems` or `SoftDelete`
//! (what Outlook's Delete and "Delete for good" do: recoverable from the dumpster); `HardDelete`
//! has no variant, so it cannot be asked for.
//!
//! An item's id changes when it is moved ([`Moved`] says the new one); a change key changes with
//! every edit ([`Saved`]). Both are the server's and stay opaque.

use mailcraft_model::sync::FlagUp;
use mailcraft_model::{Address, FlagStatus, Importance};

use crate::error::Error;
use crate::folder::valid_id;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// What a delete does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Into Deleted Items.
    MoveToDeletedItems,
    /// Into the dumpster, recoverable by the owner (Outlook's "Delete for good").
    SoftDelete,
}

impl DeleteMode {
    fn name(self) -> &'static str {
        match self {
            Self::MoveToDeletedItems => "MoveToDeletedItems",
            Self::SoftDelete => "SoftDelete",
        }
    }
}

/// The ids of a saved item: the id and the change key it has now.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Saved {
    /// The item's id.
    pub id: String,
    /// Its change key.
    pub change_key: String,
}

impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Saved(..)")
    }
}

/// Whether `error` is the response code `name` (a code this crate has no variant for).
pub fn is_code(error: &Error, name: &str) -> bool {
    matches!(error, Error::Server(code) if code.to_string() == name)
}

pub(crate) fn id_element(name: &str, id: &str, change_key: Option<&str>) -> Element {
    let element = Element::new(name).attr("Id", id);
    match change_key {
        Some(key) if !key.is_empty() => element.attr("ChangeKey", key),
        _ => element,
    }
}

/// `text` for a value that is one line (a subject, a display name, a category, a folder's name, a
/// file's name): CR, LF and NUL taken out, as the MIME writer does for a header value, so that a
/// value never carries a line break into the server's headers or its folder names.
pub(crate) fn line(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect()
}

fn mailbox(address: &Address) -> Element {
    let mut element = Element::new("t:Mailbox");
    let name = line(address.name.trim());
    if !name.trim().is_empty() {
        element = element.child(Element::new("t:Name").text(name.trim()));
    }
    element.child(Element::new("t:EmailAddress").text(line(address.email.trim()).trim()))
}

/// A recipient list element (`t:ToRecipients`) of `people`.
pub(crate) fn recipients(name: &str, people: &[Address]) -> Element {
    people.iter().filter(|a| !a.email.trim().is_empty()).fold(Element::new(name), |list, a| list.child(mailbox(a)))
}

fn importance_name(importance: Importance) -> &'static str {
    match importance {
        Importance::Low => "Low",
        Importance::Normal => "Normal",
        Importance::High => "High",
    }
}

/// The first `ItemId` an answer's `Items` holds (the item as the server has it after the change).
fn first_item(node: &Node) -> Option<Saved> {
    let items = node.child_in(MESSAGES, "Items")?;
    let item = items.children.iter().find(|c| *c.namespace == *TYPES)?;
    let id = item.child_in(TYPES, "ItemId")?;
    let take = |name: &str| id.attribute(name).filter(|v| valid_id(v)).map(str::to_owned);
    Some(Saved { id: take("Id")?, change_key: take("ChangeKey").unwrap_or_default() })
}

/// `item:Flag` as the schema has it: the status, and the dates when there is a due date (a flag
/// with a start and no due date is not one Outlook makes).
fn flag_element(flag: &FlagUp) -> Element {
    let mut inner = Element::new("t:Flag");
    let status = match flag.status {
        FlagStatus::None => "NotFlagged",
        FlagStatus::Flagged => "Flagged",
        FlagStatus::Complete => "Complete",
    };
    inner = inner.child(Element::new("t:FlagStatus").text(status));
    let when = |ts| mailcraft_time::format_iso_utc(ts);
    match flag.status {
        FlagStatus::Flagged => {
            if let Some((start, due)) = flag.start.zip(flag.due) {
                inner = inner.child(Element::new("t:StartDate").text(&when(start))).child(Element::new("t:DueDate").text(&when(due)));
            }
        }
        FlagStatus::Complete => {
            if let Some(done) = flag.completed {
                inner = inner.child(Element::new("t:CompleteDate").text(&when(done)));
            }
        }
        FlagStatus::None => {}
    }
    inner
}

/// The properties of a message an `UpdateItem` sets; what is `None` is left as it is.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ItemUpdate {
    /// `message:IsRead`.
    pub read: Option<bool>,
    /// `item:Flag` (where the server has it).
    pub flag: Option<FlagUp>,
    /// `item:Categories`: the whole list (empty clears it).
    pub categories: Option<Vec<String>>,
    /// `item:Importance`.
    pub importance: Option<Importance>,
    /// `item:Subject` (a draft).
    pub subject: Option<String>,
    /// `item:Body`: whether it is HTML, and the content (a draft).
    pub body: Option<(bool, String)>,
    /// `message:ToRecipients`, `CcRecipients`, `BccRecipients` (a draft).
    pub to: Option<Vec<Address>>,
    /// See `to`.
    pub cc: Option<Vec<Address>>,
    /// See `to`.
    pub bcc: Option<Vec<Address>>,
}

impl ItemUpdate {
    fn set(uri: &str, holder: &str, value: Element) -> Element {
        Element::new("t:SetItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri)).child(Element::new(holder).child(value))
    }

    fn clear(uri: &str) -> Element {
        Element::new("t:DeleteItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri))
    }

    /// The `t:Updates` element, or none when nothing is set.
    fn element(&self) -> Option<Element> {
        let mut updates = Element::new("t:Updates");
        let mut any = false;
        let mut put = |element: Element| {
            updates = std::mem::take(&mut updates).child(element);
            any = true;
        };
        if let Some(subject) = &self.subject {
            put(Self::set("item:Subject", "t:Message", Element::new("t:Subject").text(&line(subject))));
        }
        if let Some((html, content)) = &self.body {
            let body = Element::new("t:Body").attr("BodyType", if *html { "HTML" } else { "Text" }).text(content);
            put(Self::set("item:Body", "t:Message", body));
        }
        for (uri, name, list) in [
            ("message:ToRecipients", "t:ToRecipients", &self.to),
            ("message:CcRecipients", "t:CcRecipients", &self.cc),
            ("message:BccRecipients", "t:BccRecipients", &self.bcc),
        ] {
            if let Some(people) = list {
                if people.iter().all(|a| a.email.trim().is_empty()) {
                    put(Self::clear(uri));
                } else {
                    put(Self::set(uri, "t:Message", recipients(name, people)));
                }
            }
        }
        if let Some(importance) = self.importance {
            put(Self::set("item:Importance", "t:Message", Element::new("t:Importance").text(importance_name(importance))));
        }
        if let Some(read) = self.read {
            put(Self::set("message:IsRead", "t:Message", Element::new("t:IsRead").text(if read { "true" } else { "false" })));
        }
        if let Some(flag) = &self.flag {
            put(Self::set("item:Flag", "t:Message", flag_element(flag)));
        }
        if let Some(categories) = &self.categories {
            if categories.is_empty() {
                put(Self::clear("item:Categories"));
            } else {
                let list = categories.iter().fold(Element::new("t:Categories"), |list, c| list.child(Element::new("t:String").text(&line(c))));
                put(Self::set("item:Categories", "t:Message", list));
            }
        }
        any.then_some(updates)
    }
}

/// `UpdateItem` of one message.
#[derive(Clone, PartialEq, Eq)]
pub struct UpdateItem {
    id: String,
    change_key: Option<String>,
    updates: ItemUpdate,
    /// Written over whatever the server has (marks), or refused when the item changed since
    /// `change_key` (a draft: `ErrorIrresolvableConflict`).
    overwrite: bool,
}

impl std::fmt::Debug for UpdateItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateItem").field("overwrite", &self.overwrite).finish()
    }
}

impl UpdateItem {
    /// Sets `updates` on the item `id`, whatever the server has now (read, flag, categories,
    /// importance).
    pub fn marks(id: &str, updates: ItemUpdate) -> Self {
        Self { id: id.to_owned(), change_key: None, updates, overwrite: true }
    }

    /// Sets `updates` on the draft `id` as it was at `change_key`: if it was changed since, the
    /// server says `ErrorIrresolvableConflict` and nothing is written.
    pub fn draft(id: &str, change_key: &str, updates: ItemUpdate) -> Self {
        Self { id: id.to_owned(), change_key: Some(change_key.to_owned()), updates, overwrite: change_key.is_empty() }
    }

    /// Whether there is anything to set.
    pub fn is_empty(&self) -> bool {
        self.updates.element().is_none()
    }

    /// The operation as the schema has it.
    pub fn element(&self) -> Element {
        let change = Element::new("t:ItemChange").child(id_element("t:ItemId", &self.id, self.change_key.as_deref()));
        let change = match self.updates.element() {
            Some(updates) => change.child(updates),
            None => change,
        };
        Element::new("m:UpdateItem")
            .attr("MessageDisposition", "SaveOnly")
            .attr("ConflictResolution", if self.overwrite { "AlwaysOverwrite" } else { "NeverOverwrite" })
            .child(Element::new("m:ItemChanges").child(change))
    }

    /// The item's ids after the change.
    pub fn answer(&self, reply: &Reply) -> Result<Saved, Error> {
        let messages = reply.messages("UpdateItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        first_item(message.ok()?).ok_or(Error::Unexpected)
    }
}

/// `MoveItem` of one message into a folder.
#[derive(Clone, PartialEq, Eq)]
pub struct MoveItem {
    id: String,
    to: String,
    copy: bool,
}

impl std::fmt::Debug for MoveItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MoveItem(..)")
    }
}

impl MoveItem {
    /// Moves item `id` into the folder `to` (a server id).
    pub fn new(id: &str, to: &str) -> Self {
        Self { id: id.to_owned(), to: to.to_owned(), copy: false }
    }

    /// Copies item `id` into the folder `to` instead (`CopyItem`: the same shape and answer).
    pub fn copy(id: &str, to: &str) -> Self {
        Self { id: id.to_owned(), to: to.to_owned(), copy: true }
    }

    /// The operation: `ToFolderId`, `ItemIds`, `ReturnNewItemIds`.
    pub fn element(&self) -> Element {
        Element::new(if self.copy { "m:CopyItem" } else { "m:MoveItem" })
            .child(Element::new("m:ToFolderId").child(id_element("t:FolderId", &self.to, None)))
            .child(Element::new("m:ItemIds").child(id_element("t:ItemId", &self.id, None)))
            .child(Element::new("m:ReturnNewItemIds").text("true"))
    }

    /// The item's id in its new folder.
    pub fn answer(&self, reply: &Reply) -> Result<Saved, Error> {
        let messages = reply.messages(if self.copy { "CopyItem" } else { "MoveItem" })?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        // (A server that doesn't say the new id leaves the old one: it is found again by the next read.)
        let node = message.ok()?;
        Ok(first_item(node).unwrap_or_else(|| Saved { id: self.id.clone(), change_key: String::new() }))
    }
}

/// `DeleteItem` of one message.
#[derive(Clone, PartialEq, Eq)]
pub struct DeleteItem {
    id: String,
    mode: DeleteMode,
}

impl std::fmt::Debug for DeleteItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteItem").field("mode", &self.mode).finish()
    }
}

impl DeleteItem {
    /// Deletes item `id` the way `mode` says.
    pub fn new(id: &str, mode: DeleteMode) -> Self {
        Self { id: id.to_owned(), mode }
    }

    /// The operation: `DeleteType` and `ItemIds`.
    pub fn element(&self) -> Element {
        Element::new("m:DeleteItem")
            .attr("DeleteType", self.mode.name())
            .child(Element::new("m:ItemIds").child(id_element("t:ItemId", &self.id, None)))
    }

    /// Whether it was done.
    pub fn answer(&self, reply: &Reply) -> Result<(), Error> {
        let messages = reply.messages("DeleteItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        message.ok().map(|_| ())
    }
}

/// A folder as an answer to a change of folders names it: its id.
fn first_folder_id(node: &Node) -> Option<String> {
    let folders = node.child_in(MESSAGES, "Folders")?;
    let folder = folders.children.iter().find(|c| *c.namespace == *TYPES)?;
    folder.child_in(TYPES, "FolderId")?.attribute("Id").filter(|id| valid_id(id)).map(str::to_owned)
}

/// Where a folder is made or moved to: the top of the mail, or a folder by id.
fn folder_target(name: &str, parent: &str) -> Element {
    let holder = Element::new(name);
    if parent.is_empty() {
        holder.child(Element::new("t:DistinguishedFolderId").attr("Id", "msgfolderroot"))
    } else {
        holder.child(id_element("t:FolderId", parent, None))
    }
}

/// `CreateFolder` of a folder of mail.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateFolder {
    parent: String,
    name: String,
}

impl std::fmt::Debug for CreateFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreateFolder(..)")
    }
}

impl CreateFolder {
    /// Makes a folder called `name` under `parent` (a server id; empty: the top of the mail).
    pub fn new(parent: &str, name: &str) -> Self {
        Self { parent: parent.to_owned(), name: name.to_owned() }
    }

    /// The operation: `ParentFolderId` and `Folders`.
    pub fn element(&self) -> Element {
        let folder = Element::new("t:Folder")
            .child(Element::new("t:FolderClass").text("IPF.Note"))
            .child(Element::new("t:DisplayName").text(&line(&self.name)));
        Element::new("m:CreateFolder").child(folder_target("m:ParentFolderId", &self.parent)).child(Element::new("m:Folders").child(folder))
    }

    /// The new folder's id.
    pub fn answer(&self, reply: &Reply) -> Result<String, Error> {
        let messages = reply.messages("CreateFolder")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        first_folder_id(message.ok()?).ok_or(Error::Unexpected)
    }
}

/// `UpdateFolder`: a new display name.
#[derive(Clone, PartialEq, Eq)]
pub struct RenameFolder {
    id: String,
    name: String,
}

impl std::fmt::Debug for RenameFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RenameFolder(..)")
    }
}

impl RenameFolder {
    /// Calls folder `id` `name`.
    pub fn new(id: &str, name: &str) -> Self {
        Self { id: id.to_owned(), name: name.to_owned() }
    }

    /// The operation: `FolderChanges`.
    pub fn element(&self) -> Element {
        let set = Element::new("t:SetFolderField")
            .child(Element::new("t:FieldURI").attr("FieldURI", "folder:DisplayName"))
            .child(Element::new("t:Folder").child(Element::new("t:DisplayName").text(&line(&self.name))));
        let change = Element::new("t:FolderChange").child(id_element("t:FolderId", &self.id, None)).child(Element::new("t:Updates").child(set));
        Element::new("m:UpdateFolder").child(Element::new("m:FolderChanges").child(change))
    }

    /// Whether it was done.
    pub fn answer(&self, reply: &Reply) -> Result<(), Error> {
        let messages = reply.messages("UpdateFolder")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        message.ok().map(|_| ())
    }
}

/// `MoveFolder`.
#[derive(Clone, PartialEq, Eq)]
pub struct MoveFolder {
    id: String,
    to: String,
}

impl std::fmt::Debug for MoveFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MoveFolder(..)")
    }
}

impl MoveFolder {
    /// Moves folder `id` under `to` (a server id; empty: the top of the mail).
    pub fn new(id: &str, to: &str) -> Self {
        Self { id: id.to_owned(), to: to.to_owned() }
    }

    /// The operation: `ToFolderId` and `FolderIds`.
    pub fn element(&self) -> Element {
        Element::new("m:MoveFolder").child(folder_target("m:ToFolderId", &self.to)).child(Element::new("m:FolderIds").child(id_element(
            "t:FolderId",
            &self.id,
            None,
        )))
    }

    /// The folder's id after the move.
    pub fn answer(&self, reply: &Reply) -> Result<String, Error> {
        let messages = reply.messages("MoveFolder")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        Ok(first_folder_id(message.ok()?).unwrap_or_else(|| self.id.clone()))
    }
}

/// `DeleteFolder`.
#[derive(Clone, PartialEq, Eq)]
pub struct DeleteFolder {
    id: String,
    mode: DeleteMode,
}

impl std::fmt::Debug for DeleteFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteFolder").field("mode", &self.mode).finish()
    }
}

impl DeleteFolder {
    /// Deletes folder `id` the way `mode` says.
    pub fn new(id: &str, mode: DeleteMode) -> Self {
        Self { id: id.to_owned(), mode }
    }

    /// The operation: `DeleteType` and `FolderIds`.
    pub fn element(&self) -> Element {
        Element::new("m:DeleteFolder").attr("DeleteType", self.mode.name()).child(Element::new("m:FolderIds").child(id_element(
            "t:FolderId",
            &self.id,
            None,
        )))
    }

    /// Whether it was done.
    pub fn answer(&self, reply: &Reply) -> Result<(), Error> {
        let messages = reply.messages("DeleteFolder")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        message.ok().map(|_| ())
    }
}

/// What `GetItem` says of a message for a change: its categories, its change key and its
/// attachments (by id and name).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ItemNow {
    /// The change key now.
    pub change_key: String,
    /// The categories now.
    pub categories: Vec<String>,
    /// The attachments the item holds.
    pub attachments: Vec<NowFile>,
}

/// A file attached to an item, as the server says it is.
#[derive(Clone, PartialEq, Eq)]
pub struct NowFile {
    /// The server's id of it.
    pub id: String,
    /// Its file name.
    pub name: String,
    /// Its size in bytes; 0 when the server said none.
    pub size: u64,
    /// Its content id, for an inline one.
    pub content_id: String,
}

impl std::fmt::Debug for ItemNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemNow").field("attachments", &self.attachments.len()).finish()
    }
}

/// `GetItem` for [`ItemNow`].
#[derive(Clone, PartialEq, Eq)]
pub struct GetItemNow {
    id: String,
}

impl std::fmt::Debug for GetItemNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GetItemNow(..)")
    }
}

const MAX_NOW_CATEGORIES: usize = 200;
const MAX_NOW_ATTACHMENTS: usize = 500;

impl GetItemNow {
    /// Reads item `id`.
    pub fn new(id: &str) -> Self {
        Self { id: id.to_owned() }
    }

    /// The operation: an `IdOnly` shape with the categories and the attachments.
    pub fn element(&self) -> Element {
        let uri = |name: &str| Element::new("t:FieldURI").attr("FieldURI", name);
        let shape = Element::new("m:ItemShape")
            .child(Element::new("t:BaseShape").text("IdOnly"))
            .child(Element::new("t:AdditionalProperties").child(uri("item:Categories")).child(uri("item:Attachments")));
        Element::new("m:GetItem").child(shape).child(Element::new("m:ItemIds").child(id_element("t:ItemId", &self.id, None)))
    }

    /// The item now, or `ItemNotFound`.
    pub fn answer(&self, reply: &Reply) -> Result<ItemNow, Error> {
        let messages = reply.messages("GetItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
        let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
        let saved = first_item(node).unwrap_or_default();
        let categories = item
            .child_in(TYPES, "Categories")
            .map(|c| c.children.iter().take(MAX_NOW_CATEGORIES).map(|s| s.text.trim().to_owned()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let attachments = item
            .child_in(TYPES, "Attachments")
            .map(|list| {
                list.children
                    .iter()
                    .take(MAX_NOW_ATTACHMENTS)
                    .filter_map(|a| {
                        let id = a.child_in(TYPES, "AttachmentId")?.attribute("Id").filter(|id| valid_id(id))?.to_owned();
                        Some(NowFile {
                            id,
                            name: a.child_in(TYPES, "Name").map(|n| n.text.trim().to_owned()).unwrap_or_default(),
                            size: a.child_in(TYPES, "Size").and_then(|n| n.text.trim().parse().ok()).unwrap_or(0),
                            content_id: a.child_in(TYPES, "ContentId").map(|n| n.text.trim().to_owned()).unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ItemNow { change_key: saved.change_key, categories, attachments })
    }
}
