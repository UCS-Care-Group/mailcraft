// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The writing half of the stand-in (#252, E6): `UpdateItem`, `MoveItem`, `CopyItem`, `DeleteItem`,
//! `CreateItem` (`SaveOnly`, `SendAndSaveCopy`), `FindItem`, `CreateAttachment`,
//! `DeleteAttachment`, `CreateFolder`, `UpdateFolder`, `MoveFolder` and `DeleteFolder`, held to
//! the schemas of [MS-OXWSCORE], [MS-OXWSFOLD] and [MS-OXWSATT]: the attributes each operation
//! has, the children it has in the order of the schema, the delete types the service knows.
//!
//! Item ids change when an item is moved, as the service's do; folder ids don't. A change that
//! is made is one more revision, so that a later read tells of it.

use crate::soap::{MESSAGES, TYPES};
use crate::standin::{Check, Folder, Refusal, State, error_message, invalid, response};
use crate::standin_mail::{Item, ItemChange, ItemLog};
use crate::xml::{Node, escape};

/// The operations this module answers.
pub const HANDLED: &[&str] = &[
    "UpdateItem",
    "MoveItem",
    "CopyItem",
    "DeleteItem",
    "CreateItem",
    "FindItem",
    "CreateAttachment",
    "DeleteAttachment",
    "CreateFolder",
    "UpdateFolder",
    "MoveFolder",
    "DeleteFolder",
];

type Answer = Result<(u16, String, String), Refusal>;

/// An answer that says no with a response code (HTTP 200, as the service does).
fn no(operation: &str, code: &str) -> Answer {
    Ok((200, code.to_owned(), response(operation, &error_message(operation, code))))
}

fn ok_message(operation: &str, inner: &str) -> String {
    format!("<m:{operation}ResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>{inner}</m:{operation}ResponseMessage>")
}

fn yes(operation: &str, inner: &str) -> Answer {
    Ok((200, "NoError".into(), response(operation, &ok_message(operation, inner))))
}

fn item_id_xml(item: &Item) -> String {
    format!("<t:ItemId Id=\"{}\" ChangeKey=\"{}\"/>", escape(&item.id), escape(&item.change_key))
}

fn people_of(node: &Node) -> Vec<(String, String)> {
    node.children.iter().map(|m| (m.text_of("Name").trim().to_owned(), m.text_of("EmailAddress").trim().to_owned())).collect()
}

impl State {
    pub(crate) fn write(&mut self, name: &str, operation: &Node) -> Answer {
        match name {
            "UpdateItem" => self.update_item(operation),
            "MoveItem" | "CopyItem" => self.move_item(name, operation),
            "DeleteItem" => self.delete_item(operation),
            "CreateItem" => self.create_item(operation),
            "FindItem" => self.find_item(operation),
            "CreateAttachment" => self.create_attachment(operation),
            "DeleteAttachment" => self.delete_attachment(operation),
            "CreateFolder" => self.create_folder(operation),
            "UpdateFolder" => self.update_folder(operation),
            "MoveFolder" => self.move_folder(operation),
            _ => self.delete_folder_op(operation),
        }
    }

    /// The folder a `FolderId` or `DistinguishedFolderId` child of `holder` names.
    fn folder_of(&self, holder: &Node) -> Result<Option<Folder>, Refusal> {
        Check::no_text(holder)?;
        let [one] = holder.children.as_slice() else { return Err(invalid(format!("{} names one folder", holder.name))) };
        if one.is(TYPES, "DistinguishedFolderId") {
            Check::is(one, TYPES, "DistinguishedFolderId", &["Id", "ChangeKey"])?;
            Ok(self.folder(Check::attribute(one, "Id")?, true).cloned())
        } else {
            Check::is(one, TYPES, "FolderId", &["Id", "ChangeKey"])?;
            Ok(self.folder(Check::attribute(one, "Id")?, false).cloned())
        }
    }

    fn item_ids_of(node: &Node, tag: &str) -> Result<Vec<(String, Option<String>)>, Refusal> {
        Check::is(node, MESSAGES, tag, &[])?;
        Check::no_text(node)?;
        if node.children.is_empty() {
            return Err(invalid(format!("{tag} names at least one item")));
        }
        node.children
            .iter()
            .map(|one| {
                Check::is(one, TYPES, "ItemId", &["Id", "ChangeKey"])?;
                Check::empty(one)?;
                Ok((Check::attribute(one, "Id")?.to_owned(), one.attribute("ChangeKey").map(str::to_owned)))
            })
            .collect()
    }

    fn at(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    fn update_item(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "UpdateItem", &["MessageDisposition", "ConflictResolution"])?;
        if operation.attribute("MessageDisposition") != Some("SaveOnly") {
            return Err(invalid("MessageDisposition is SaveOnly here"));
        }
        let resolution = operation.attribute("ConflictResolution").ok_or_else(|| invalid("ConflictResolution is required"))?;
        if !["NeverOverwrite", "AutoResolve", "AlwaysOverwrite"].contains(&resolution) {
            return Err(invalid("ConflictResolution is NeverOverwrite, AutoResolve or AlwaysOverwrite"));
        }
        let parts = Check::children(operation, MESSAGES, &["SavedItemFolderId", "ItemChanges"])?;
        let changes = Check::required(operation, parts.get(1).copied().flatten(), "ItemChanges")?;
        Check::is(changes, MESSAGES, "ItemChanges", &[])?;
        let mut messages = String::new();
        let mut first = String::new();
        'changes: for change in &changes.children {
            Check::is(change, TYPES, "ItemChange", &[])?;
            let inside = Check::children(change, TYPES, &["ItemId", "Updates"])?;
            let id = Check::required(change, inside.first().copied().flatten(), "ItemId")?;
            Check::is(id, TYPES, "ItemId", &["Id", "ChangeKey"])?;
            let updates = Check::required(change, inside.get(1).copied().flatten(), "Updates")?;
            Check::is(updates, TYPES, "Updates", &[])?;
            let Some(at) = self.at(Check::attribute(id, "Id")?) else {
                messages.push_str(&error_message("UpdateItem", "ErrorItemNotFound"));
                if first.is_empty() {
                    first = "ErrorItemNotFound".into();
                }
                continue;
            };
            if resolution == "NeverOverwrite"
                && let Some(key) = id.attribute("ChangeKey")
                && self.items.get(at).is_some_and(|i| i.change_key != key)
            {
                messages.push_str(&error_message("UpdateItem", "ErrorIrresolvableConflict"));
                if first.is_empty() {
                    first = "ErrorIrresolvableConflict".into();
                }
                continue;
            }
            let mut item = self.items.get(at).cloned().ok_or_else(|| invalid("no item"))?;
            let mut only_read = true;
            for update in &updates.children {
                let set = update.is(TYPES, "SetItemField");
                if !set && !update.is(TYPES, "DeleteItemField") {
                    return Err(invalid(format!("Updates has no place for {}", update.name)));
                }
                Check::no_text(update)?;
                let uri = update.child_in(TYPES, "FieldURI").ok_or_else(|| invalid("an update needs a FieldURI"))?;
                Check::is(uri, TYPES, "FieldURI", &["FieldURI"])?;
                let uri = Check::attribute(uri, "FieldURI")?.to_owned();
                let holder = update.children.get(1);
                if set != holder.is_some() || update.children.len() > 2 {
                    return Err(invalid("a SetItemField has a FieldURI and the item with the value; a DeleteItemField has the FieldURI only"));
                }
                if let Some(holder) = holder {
                    Check::is(holder, TYPES, "Message", &[])?;
                }
                let value = |name: &str| holder.and_then(|h| h.child_in(TYPES, name));
                if uri != "message:IsRead" {
                    only_read = false;
                }
                match uri.as_str() {
                    "message:IsRead" => {
                        let v = value("IsRead").ok_or_else(|| invalid("IsRead is missing"))?;
                        item.read = match v.text.trim() {
                            "true" => true,
                            "false" => false,
                            _ => return Err(invalid("IsRead is true or false")),
                        };
                    }
                    "item:Importance" => {
                        let v = value("Importance").ok_or_else(|| invalid("Importance is missing"))?.text.trim().to_owned();
                        if !["Low", "Normal", "High"].contains(&v.as_str()) {
                            return Err(invalid("Importance is Low, Normal or High"));
                        }
                        item.importance = v;
                    }
                    "item:Flag" => {
                        if self.lacks_flag {
                            messages.push_str(&error_message("UpdateItem", "ErrorInvalidPropertyForOperation"));
                            if first.is_empty() {
                                first = "ErrorInvalidPropertyForOperation".into();
                            }
                            continue 'changes;
                        }
                        let flag = value("Flag").ok_or_else(|| invalid("Flag is missing"))?;
                        let parts = Check::children(flag, TYPES, &["FlagStatus", "StartDate", "DueDate", "CompleteDate"])?;
                        let status = Check::required(flag, parts.first().copied().flatten(), "FlagStatus")?.text.trim().to_owned();
                        if !["NotFlagged", "Flagged", "Complete"].contains(&status.as_str()) {
                            return Err(invalid("FlagStatus is NotFlagged, Flagged or Complete"));
                        }
                        if parts.get(1).copied().flatten().is_some() != parts.get(2).copied().flatten().is_some() {
                            return Err(invalid("StartDate and DueDate come together"));
                        }
                        item.flagged = status != "NotFlagged";
                        item.flag = status;
                    }
                    "item:Categories" => {
                        item.categories = match value("Categories") {
                            Some(list) => {
                                Check::no_text(list)?;
                                list.children.iter().map(|s| s.text.trim().to_owned()).collect()
                            }
                            None => Vec::new(),
                        };
                    }
                    "item:Subject" => item.subject = value("Subject").ok_or_else(|| invalid("Subject is missing"))?.text.clone(),
                    "item:Body" => {
                        let body = value("Body").ok_or_else(|| invalid("Body is missing"))?;
                        let html = match body.attribute("BodyType") {
                            Some("HTML") => true,
                            Some("Text") => false,
                            _ => return Err(invalid("BodyType is HTML or Text")),
                        };
                        item.body = (html, body.text.clone());
                    }
                    "message:ToRecipients" => item.to = value("ToRecipients").map(people_of).unwrap_or_default(),
                    "message:CcRecipients" => item.cc = value("CcRecipients").map(people_of).unwrap_or_default(),
                    "message:BccRecipients" => item.bcc = value("BccRecipients").map(people_of).unwrap_or_default(),
                    "message:ReplyTo" => item.reply_to = value("ReplyTo").map(people_of).unwrap_or_default(),
                    other => return Err(invalid(format!("{other} is no property this stand-in updates"))),
                }
            }
            let kind = if only_read { ItemChange::Read } else { ItemChange::Update };
            let id = item.id.clone();
            let replaced = item;
            self.touch(&id, kind, |i| {
                let (folder, key) = (i.folder.clone(), i.change_key.clone());
                *i = replaced;
                i.folder = folder;
                i.change_key = key;
            });
            let now = self.item(&id).map(item_id_xml).unwrap_or_default();
            messages.push_str(&ok_message(
                "UpdateItem",
                &format!("<m:Items><t:Message>{now}</t:Message></m:Items><m:ConflictResults><t:Count>0</t:Count></m:ConflictResults>"),
            ));
            if first.is_empty() {
                first = "NoError".into();
            }
        }
        Ok((200, first, response("UpdateItem", &messages)))
    }

    /// Moves the item at `at` into `folder` under a new id, as the service does.
    fn relocate(&mut self, at: usize, folder: &Folder, copy: bool) -> Option<Item> {
        let mut item = self.items.get(at)?.clone();
        let old = item.id.clone();
        if !copy {
            self.rev = self.rev.saturating_add(1);
            self.items.remove(at);
            let log = ItemLog { rev: self.rev, folder: item.folder.clone(), id: old, kind: ItemChange::Delete };
            self.item_log.push(log);
        }
        item.id = self.next_id("item");
        item.folder = folder.id.clone();
        self.rev = self.rev.saturating_add(1);
        item.change_key = format!("CQAAABYAAAA{}", self.rev);
        self.item_log.push(ItemLog { rev: self.rev, folder: folder.id.clone(), id: item.id.clone(), kind: ItemChange::Create });
        self.items.push(item.clone());
        Some(item)
    }

    fn move_item(&mut self, name: &str, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, name, &[])?;
        let parts = Check::children(operation, MESSAGES, &["ToFolderId", "ItemIds", "ReturnNewItemIds"])?;
        let to = Check::required(operation, parts.first().copied().flatten(), "ToFolderId")?;
        Check::is(to, MESSAGES, "ToFolderId", &[])?;
        let ids = Self::item_ids_of(Check::required(operation, parts.get(1).copied().flatten(), "ItemIds")?, "ItemIds")?;
        if let Some(r) = parts.get(2).copied().flatten() {
            Check::is(r, MESSAGES, "ReturnNewItemIds", &[])?;
        }
        let Some(folder) = self.folder_of(to)? else { return no(name, "ErrorFolderNotFound") };
        let mut messages = String::new();
        let mut first = String::new();
        for (id, _) in ids {
            let (code, inner) = match self.at(&id).and_then(|at| self.relocate(at, &folder, name == "CopyItem")) {
                Some(item) => ("NoError", ok_message(name, &format!("<m:Items><t:Message>{}</t:Message></m:Items>", item_id_xml(&item)))),
                None => ("ErrorItemNotFound", error_message(name, "ErrorItemNotFound")),
            };
            messages.push_str(&inner);
            if first.is_empty() {
                first = code.into();
            }
        }
        Ok((200, first, response(name, &messages)))
    }

    fn delete_item(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "DeleteItem", &["DeleteType", "SendMeetingCancellations", "AffectedTaskOccurrences", "SuppressReadReceipts"])?;
        let kind = Check::attribute(operation, "DeleteType")?.to_owned();
        if !["HardDelete", "SoftDelete", "MoveToDeletedItems"].contains(&kind.as_str()) {
            return Err(invalid("DeleteType is HardDelete, SoftDelete or MoveToDeletedItems"));
        }
        let parts = Check::children(operation, MESSAGES, &["ItemIds"])?;
        let ids = Self::item_ids_of(Check::required(operation, parts.first().copied().flatten(), "ItemIds")?, "ItemIds")?;
        self.deletes.push(kind.clone());
        let mut messages = String::new();
        let mut first = String::new();
        for (id, _) in ids {
            let found = self.at(&id);
            let (code, inner) = match found {
                None => ("ErrorItemNotFound", error_message("DeleteItem", "ErrorItemNotFound")),
                Some(at) => {
                    if kind == "MoveToDeletedItems" {
                        if let Some(deleted) = self.folder("deleteditems", true).cloned() {
                            self.relocate(at, &deleted, false);
                        }
                    } else {
                        self.remove(&id);
                    }
                    ("NoError", ok_message("DeleteItem", ""))
                }
            };
            messages.push_str(&inner);
            if first.is_empty() {
                first = code.into();
            }
        }
        Ok((200, first, response("DeleteItem", &messages)))
    }

    fn attachments_of(&mut self, list: &Node) -> Result<Vec<(String, String, Vec<u8>)>, Refusal> {
        Check::no_text(list)?;
        let mut out = Vec::new();
        for file in &list.children {
            Check::is(file, TYPES, "FileAttachment", &[])?;
            let order = ["Name", "ContentType", "ContentId", "IsInline", "Content"];
            let parts = Check::children(file, TYPES, &order)?;
            let name = Check::required(file, parts.first().copied().flatten(), "Name")?.text.trim().to_owned();
            let content = Check::required(file, parts.get(4).copied().flatten(), "Content")?;
            let bytes = crate::b64::decode(&content.text, 64 << 20).ok_or_else(|| invalid("Content is Base64"))?;
            out.push((self.next_id("attachment"), name, bytes));
        }
        Ok(out)
    }

    fn create_item(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "CreateItem", &["MessageDisposition", "SendMeetingInvitations"])?;
        let disposition = Check::attribute(operation, "MessageDisposition")?.to_owned();
        if !["SaveOnly", "SendOnly", "SendAndSaveCopy"].contains(&disposition.as_str()) {
            return Err(invalid("MessageDisposition is SaveOnly, SendOnly or SendAndSaveCopy"));
        }
        let parts = Check::children(operation, MESSAGES, &["SavedItemFolderId", "Items"])?;
        let items = Check::required(operation, parts.get(1).copied().flatten(), "Items")?;
        Check::is(items, MESSAGES, "Items", &[])?;
        let [message] = items.children.as_slice() else { return Err(invalid("Items holds one message here")) };
        Check::is(message, TYPES, "Message", &[])?;
        let saved_in = match parts.first().copied().flatten() {
            Some(holder) => self.folder_of(holder)?,
            None => None,
        };
        let sending = disposition != "SaveOnly";
        if sending && saved_in.as_ref().is_none_or(|f| f.distinguished != "sentitems") && disposition == "SendAndSaveCopy" {
            return no("CreateItem", "ErrorFolderNotFound");
        }
        let mut item = Item {
            id: String::new(),
            change_key: String::new(),
            folder: String::new(),
            subject: String::new(),
            from: ("Ada".into(), "ada@example.org".into()),
            to: Vec::new(),
            cc: Vec::new(),
            reply_to: Vec::new(),
            received: "2026-10-14T16:00:00Z".into(),
            sent: "2026-10-14T16:00:00Z".into(),
            read: true,
            flagged: false,
            draft: !sending,
            attachments: false,
            categories: Vec::new(),
            importance: "Normal".into(),
            message_id: String::new(),
            preview: String::new(),
            bcc: Vec::new(),
            body: (false, String::new()),
            flag: "NotFlagged".into(),
            marker: String::new(),
            files: Vec::new(),
            mime: Vec::new(),
        };
        if let Some(mime) = message.child_in(TYPES, "MimeContent") {
            let bytes = crate::b64::decode(&mime.text, 64 << 20).ok_or_else(|| invalid("MimeContent is Base64"))?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let header = |name: &str| {
                text.lines()
                    .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned()))
                    .unwrap_or_default()
            };
            item.subject = header("Subject");
            item.message_id = header("Message-ID");
            item.mime = bytes;
        } else {
            let order =
                ["Subject", "Body", "Attachments", "Importance", "ExtendedProperty", "ToRecipients", "CcRecipients", "BccRecipients", "ReplyTo"];
            let parts = Check::children(message, TYPES, &order)?;
            item.subject = parts.first().copied().flatten().map(|n| n.text.clone()).unwrap_or_default();
            if let Some(body) = parts.get(1).copied().flatten() {
                item.body = (
                    match body.attribute("BodyType") {
                        Some("HTML") => true,
                        Some("Text") => false,
                        _ => return Err(invalid("BodyType is HTML or Text")),
                    },
                    body.text.clone(),
                );
            }
            if let Some(list) = parts.get(2).copied().flatten() {
                item.files = self.attachments_of(list)?;
                item.attachments = !item.files.is_empty();
            }
            if let Some(v) = parts.get(3).copied().flatten() {
                item.importance = v.text.trim().to_owned();
            }
            if let Some(property) = parts.get(4).copied().flatten() {
                let uri = property.child_in(TYPES, "ExtendedFieldURI").ok_or_else(|| invalid("an ExtendedProperty names its property"))?;
                Check::is(uri, TYPES, "ExtendedFieldURI", &["DistinguishedPropertySetId", "PropertyName", "PropertyType"])?;
                item.marker = property.text_of("Value").trim().to_owned();
            }
            item.to = parts.get(5).copied().flatten().map(people_of).unwrap_or_default();
            item.cc = parts.get(6).copied().flatten().map(people_of).unwrap_or_default();
            item.bcc = parts.get(7).copied().flatten().map(people_of).unwrap_or_default();
            item.reply_to = parts.get(8).copied().flatten().map(people_of).unwrap_or_default();
            item.preview = item.body.1.chars().take(200).collect();
            item.message_id = format!("<{}@example.org>", self.issued.saturating_add(1));
        }
        let folder =
            if sending { self.folder("sentitems", true).cloned() } else { saved_in.clone().or_else(|| self.folder("drafts", true).cloned()) };
        let Some(folder) = folder else { return no("CreateItem", "ErrorFolderNotFound") };
        item.id = self.next_id("item");
        item.folder = folder.id.clone();
        self.rev = self.rev.saturating_add(1);
        item.change_key = format!("CQAAABYAAAA{}", self.rev);
        if sending {
            // What went out: the MIME as it was, or the message as fields written as MIME.
            let out = if item.mime.is_empty() { format!("Subject: {}\r\n\r\n{}", item.subject, item.body.1).into_bytes() } else { item.mime.clone() };
            self.outgoing.push(out);
            item.draft = false;
        }
        let xml = item_id_xml(&item);
        self.item_log.push(ItemLog { rev: self.rev, folder: folder.id, id: item.id.clone(), kind: ItemChange::Create });
        self.items.push(item);
        let inner = if sending { "<m:Items/>".to_owned() } else { format!("<m:Items><t:Message>{xml}</t:Message></m:Items>") };
        yes("CreateItem", &inner)
    }

    fn find_item(&self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "FindItem", &["Traversal"])?;
        if !["Shallow", "SoftDeleted", "Associated"].contains(&Check::attribute(operation, "Traversal")?) {
            return Err(invalid("Traversal is Shallow, SoftDeleted or Associated"));
        }
        let parts = Check::children(operation, MESSAGES, &["ItemShape", "IndexedPageItemView", "Restriction", "SortOrder", "ParentFolderIds"])?;
        Check::required(operation, parts.first().copied().flatten(), "ItemShape")?;
        let restriction = Check::required(operation, parts.get(2).copied().flatten(), "Restriction")?;
        let equal = restriction.child_in(TYPES, "IsEqualTo").ok_or_else(|| invalid("this stand-in restricts by IsEqualTo"))?;
        let value = equal
            .child_in(TYPES, "FieldURIOrConstant")
            .and_then(|c| c.child_in(TYPES, "Constant"))
            .and_then(|c| c.attribute("Value"))
            .ok_or_else(|| invalid("IsEqualTo needs a Constant"))?
            .to_owned();
        let parents = Check::required(operation, parts.get(4).copied().flatten(), "ParentFolderIds")?;
        let newest_first = parts.get(3).copied().flatten().is_some();
        let Some(folder) = self.folder_of(parents)? else { return no("FindItem", "ErrorFolderNotFound") };
        let mut matching: Vec<_> = self.items.iter().filter(|i| i.folder == folder.id && !value.is_empty() && i.marker == value).collect();
        if newest_first {
            matching.reverse();
        }
        let found: String = matching.into_iter().map(|i| format!("<t:Message>{}</t:Message>", item_id_xml(i))).collect();
        yes("FindItem", &format!("<m:RootFolder TotalItemsInView=\"1\" IncludesLastItemInRange=\"true\"><t:Items>{found}</t:Items></m:RootFolder>"))
    }

    fn create_attachment(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "CreateAttachment", &[])?;
        let parts = Check::children(operation, MESSAGES, &["ParentItemId", "Attachments"])?;
        let parent = Check::required(operation, parts.first().copied().flatten(), "ParentItemId")?;
        Check::is(parent, MESSAGES, "ParentItemId", &["Id", "ChangeKey"])?;
        let list = Check::required(operation, parts.get(1).copied().flatten(), "Attachments")?;
        Check::is(list, MESSAGES, "Attachments", &[])?;
        let Some(at) = self.at(Check::attribute(parent, "Id")?) else { return no("CreateAttachment", "ErrorItemNotFound") };
        let files = self.attachments_of(list)?;
        let id = self.items.get(at).map(|i| i.id.clone()).unwrap_or_default();
        let mut messages = String::new();
        let count = files.len();
        for (n, file) in files.into_iter().enumerate() {
            let attachment = file.0.clone();
            let last = n.saturating_add(1) == count;
            if let Some(item) = self.items.get_mut(at) {
                item.files.push(file);
                item.attachments = true;
            }
            let key = if last {
                self.touch(&id, ItemChange::Update, |_| {});
                self.item(&id).map(|i| i.change_key.clone()).unwrap_or_default()
            } else {
                self.item(&id).map(|i| i.change_key.clone()).unwrap_or_default()
            };
            messages.push_str(&ok_message(
                "CreateAttachment",
                &format!(
                    "<m:Attachments><t:FileAttachment><t:AttachmentId Id=\"{}\" RootItemId=\"{}\" RootItemChangeKey=\"{}\"/></t:FileAttachment></m:Attachments>",
                    escape(&attachment),
                    escape(&id),
                    escape(&key)
                ),
            ));
        }
        Ok((200, "NoError".into(), response("CreateAttachment", &messages)))
    }

    fn delete_attachment(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "DeleteAttachment", &[])?;
        let parts = Check::children(operation, MESSAGES, &["AttachmentIds"])?;
        let ids = Check::required(operation, parts.first().copied().flatten(), "AttachmentIds")?;
        Check::is(ids, MESSAGES, "AttachmentIds", &[])?;
        let mut messages = String::new();
        let mut first = String::new();
        for one in &ids.children {
            Check::is(one, TYPES, "AttachmentId", &["Id"])?;
            let wanted = Check::attribute(one, "Id")?.to_owned();
            let owner = self.items.iter().find(|i| i.files.iter().any(|f| f.0 == wanted)).map(|i| i.id.clone());
            let (code, inner) = match owner {
                None => ("ErrorItemNotFound", error_message("DeleteAttachment", "ErrorItemNotFound")),
                Some(owner) => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == owner) {
                        item.files.retain(|f| f.0 != wanted);
                        item.attachments = !item.files.is_empty();
                    }
                    self.touch(&owner, ItemChange::Update, |_| {});
                    let key = self.item(&owner).map(|i| i.change_key.clone()).unwrap_or_default();
                    (
                        "NoError",
                        ok_message(
                            "DeleteAttachment",
                            &format!("<m:RootItemId RootItemId=\"{}\" RootItemChangeKey=\"{}\"/>", escape(&owner), escape(&key)),
                        ),
                    )
                }
            };
            messages.push_str(&inner);
            if first.is_empty() {
                first = code.into();
            }
        }
        Ok((200, first, response("DeleteAttachment", &messages)))
    }

    fn create_folder(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "CreateFolder", &[])?;
        let parts = Check::children(operation, MESSAGES, &["ParentFolderId", "Folders"])?;
        let parent = Check::required(operation, parts.first().copied().flatten(), "ParentFolderId")?;
        Check::is(parent, MESSAGES, "ParentFolderId", &[])?;
        let list = Check::required(operation, parts.get(1).copied().flatten(), "Folders")?;
        Check::is(list, MESSAGES, "Folders", &[])?;
        let [folder] = list.children.as_slice() else { return Err(invalid("Folders holds one folder here")) };
        Check::is(folder, TYPES, "Folder", &[])?;
        let inside = Check::children(folder, TYPES, &["FolderClass", "DisplayName"])?;
        let name = Check::required(folder, inside.get(1).copied().flatten(), "DisplayName")?.text.trim().to_owned();
        let Some(parent) = self.folder_of(parent)? else { return no("CreateFolder", "ErrorFolderNotFound") };
        if self.folders.iter().any(|f| f.parent == parent.id && f.name.eq_ignore_ascii_case(&name)) {
            return no("CreateFolder", "ErrorFolderExists");
        }
        let id = self.add_folder(&name);
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.parent = parent.id;
        }
        yes("CreateFolder", &format!("<m:Folders><t:Folder><t:FolderId Id=\"{}\" ChangeKey=\"AQAAABYA\"/></t:Folder></m:Folders>", escape(&id)))
    }

    fn update_folder(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "UpdateFolder", &[])?;
        let parts = Check::children(operation, MESSAGES, &["FolderChanges"])?;
        let changes = Check::required(operation, parts.first().copied().flatten(), "FolderChanges")?;
        let [change] = changes.children.as_slice() else { return Err(invalid("FolderChanges holds one change here")) };
        Check::is(change, TYPES, "FolderChange", &[])?;
        let inside = Check::children(change, TYPES, &["FolderId", "Updates"])?;
        let id = Check::required(change, inside.first().copied().flatten(), "FolderId")?;
        Check::is(id, TYPES, "FolderId", &["Id", "ChangeKey"])?;
        let updates = Check::required(change, inside.get(1).copied().flatten(), "Updates")?;
        let [set] = updates.children.as_slice() else { return Err(invalid("one update here")) };
        Check::is(set, TYPES, "SetFolderField", &[])?;
        let uri = set.child_in(TYPES, "FieldURI").and_then(|u| u.attribute("FieldURI")).ok_or_else(|| invalid("an update needs a FieldURI"))?;
        if uri != "folder:DisplayName" {
            return Err(invalid("this stand-in renames folders only"));
        }
        let name = set.child_in(TYPES, "Folder").map(|f| f.text_of("DisplayName").trim().to_owned()).ok_or_else(|| invalid("Folder is missing"))?;
        let id = Check::attribute(id, "Id")?.to_owned();
        if !self.rename_folder(&id, &name) {
            return no("UpdateFolder", "ErrorFolderNotFound");
        }
        yes("UpdateFolder", &format!("<m:Folders><t:Folder><t:FolderId Id=\"{}\" ChangeKey=\"AQAAABYA\"/></t:Folder></m:Folders>", escape(&id)))
    }

    fn move_folder(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "MoveFolder", &[])?;
        let parts = Check::children(operation, MESSAGES, &["ToFolderId", "FolderIds"])?;
        let to = Check::required(operation, parts.first().copied().flatten(), "ToFolderId")?;
        Check::is(to, MESSAGES, "ToFolderId", &[])?;
        let ids = Check::required(operation, parts.get(1).copied().flatten(), "FolderIds")?;
        let [one] = ids.children.as_slice() else { return Err(invalid("FolderIds names one folder here")) };
        Check::is(one, TYPES, "FolderId", &["Id", "ChangeKey"])?;
        let id = Check::attribute(one, "Id")?.to_owned();
        let Some(parent) = self.folder_of(to)? else { return no("MoveFolder", "ErrorFolderNotFound") };
        self.rev = self.rev.saturating_add(1);
        let rev = self.rev;
        let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) else { return no("MoveFolder", "ErrorFolderNotFound") };
        folder.parent = parent.id;
        self.folder_log.push(crate::standin_mail::FolderLog { rev, id: id.clone(), kind: "Update" });
        yes("MoveFolder", &format!("<m:Folders><t:Folder><t:FolderId Id=\"{}\" ChangeKey=\"AQAAABYA\"/></t:Folder></m:Folders>", escape(&id)))
    }

    fn delete_folder_op(&mut self, operation: &Node) -> Answer {
        Check::is(operation, MESSAGES, "DeleteFolder", &["DeleteType"])?;
        let kind = Check::attribute(operation, "DeleteType")?.to_owned();
        if !["HardDelete", "SoftDelete", "MoveToDeletedItems"].contains(&kind.as_str()) {
            return Err(invalid("DeleteType is HardDelete, SoftDelete or MoveToDeletedItems"));
        }
        let parts = Check::children(operation, MESSAGES, &["FolderIds"])?;
        let ids = Check::required(operation, parts.first().copied().flatten(), "FolderIds")?;
        let [one] = ids.children.as_slice() else { return Err(invalid("FolderIds names one folder here")) };
        Check::is(one, TYPES, "FolderId", &["Id", "ChangeKey"])?;
        let id = Check::attribute(one, "Id")?.to_owned();
        self.deletes.push(kind);
        if self.folders.iter().any(|f| f.id == id && !f.distinguished.is_empty()) {
            return no("DeleteFolder", "ErrorDeleteDistinguishedFolder");
        }
        if !self.delete_folder(&id) {
            return no("DeleteFolder", "ErrorFolderNotFound");
        }
        yes("DeleteFolder", "")
    }
}
