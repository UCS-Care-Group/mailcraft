// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mail half of the stand-in (#252, E6): `SyncFolderHierarchy`, `SyncFolderItems` and
//! `GetItem`, held to the schemas of [MS-OXWSSYNC] and [MS-OXWSCORE], over a mailbox the test
//! fills with made-up mail (`example.org` addresses and invented people).
//!
//! The service's sync states are modelled as the documents describe them: opaque to the client,
//! carrying the folder they were given for, and the "epoch" of the server's memory. A state of
//! another folder, one the server never gave, or one from before [`State::epoch`] was raised is
//! `ErrorInvalidSyncStateData`. A first read pages through the folder in the order the mail came
//! (`MaxChangesReturned`), and a later one brings what changed since as `Create`, `Update`,
//! `Delete` and `ReadFlagChange`.

use crate::basic::base64;
use crate::soap::{MESSAGES, TYPES};
use crate::standin::{Check, Folder, Hostile, Refusal, State, error_message, invalid, response};
use crate::xml::{Node, escape};

/// The `FieldURI`s a request may ask for besides the shape's own (the ones this stand-in knows).
const FIELD_URIS: &[&str] = &[
    "message:ToRecipients",
    "message:CcRecipients",
    "message:From",
    "message:IsRead",
    "item:Categories",
    "item:Importance",
    "item:Subject",
    "item:Preview",
    "item:Flag",
    "item:DateTimeReceived",
    "item:HasAttachments",
    "item:Attachments",
];

/// A message of the mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its id.
    pub id: String,
    /// Its change key; changes with every edit.
    pub change_key: String,
    /// The id of the folder it lies in.
    pub folder: String,
    /// Its subject.
    pub subject: String,
    /// Who it is from: name and address.
    pub from: (String, String),
    /// Who it is to.
    pub to: Vec<(String, String)>,
    /// Who it is copied to.
    pub cc: Vec<(String, String)>,
    /// When it arrived and when it was sent, as ISO 8601 in UTC.
    pub received: String,
    pub sent: String,
    /// Read or not.
    pub read: bool,
    /// Flagged for follow-up.
    pub flagged: bool,
    /// A draft.
    pub draft: bool,
    /// Has attachments.
    pub attachments: bool,
    /// Its categories.
    pub categories: Vec<String>,
    /// `Low`, `Normal` or `High`.
    pub importance: String,
    /// The `Message-ID` it carries, with its angle brackets.
    pub message_id: String,
    /// The preview.
    pub preview: String,
    /// Who it is blind-copied to.
    pub bcc: Vec<(String, String)>,
    /// The body, and whether it is HTML.
    pub body: (bool, String),
    /// The flag's status as the server has it: `NotFlagged`, `Flagged` or `Complete`.
    pub flag: String,
    /// MailCraft's marker, when the item carries one.
    pub marker: String,
    /// Its files: id, name and bytes.
    pub files: Vec<(String, String, Vec<u8>)>,
    /// The message as the server holds it, MIME.
    pub mime: Vec<u8>,
}

/// What happened to a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLog {
    /// The revision it happened at.
    pub rev: u64,
    /// The folder.
    pub id: String,
    /// `Create`, `Update` or `Delete`.
    pub kind: &'static str,
}

/// What happened to an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemLog {
    /// The revision it happened at.
    pub rev: u64,
    /// The folder it happened in.
    pub folder: String,
    /// The item.
    pub id: String,
    /// What.
    pub kind: ItemChange,
}

/// The kinds of change `SyncFolderItems` tells of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemChange {
    /// New in the folder.
    Create,
    /// Edited.
    Update,
    /// Gone from the folder.
    Delete,
    /// Read or unread, and nothing else.
    Read,
}

impl State {
    pub(crate) fn next_id(&mut self, what: &str) -> String {
        self.issued = self.issued.saturating_add(1);
        format!("AAMkAGV4YW1wbGUtb3JnL2V3cy17{what}fQ{:06}AAA=", self.issued)
    }

    /// The id of the folder with this distinguished name.
    pub fn folder_id(&self, distinguished: &str) -> Option<String> {
        self.folders.iter().find(|f| f.distinguished == distinguished).map(|f| f.id.clone())
    }

    /// Makes a message arrive in the folder with this distinguished name, from Zoe Park, to Ada;
    /// its id.
    pub fn deliver(&mut self, folder: &str, subject: &str, read: bool) -> String {
        let id = self.next_id("item");
        // (A distinguished name, or the id of a folder made with `add_folder`.)
        let folder_id = self.folder_id(folder).or_else(|| self.folders.iter().find(|f| f.id == folder).map(|f| f.id.clone())).unwrap_or_default();
        self.rev = self.rev.saturating_add(1);
        let n = self.issued;
        let when = format!("2026-10-14T{:02}:{:02}:00Z", 6 + (n / 60) % 12, n % 60);
        let mime = format!(
            "From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{n}@example.org>\r\nDate: Wed, 14 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n"
        )
        .into_bytes();
        self.items.push(Item {
            id: id.clone(),
            change_key: "CQAAABYAAAAx".into(),
            folder: folder_id.clone(),
            subject: subject.into(),
            from: ("Zoe Park".into(), "zoe@example.org".into()),
            to: vec![("Ada".into(), "ada@example.org".into())],
            cc: Vec::new(),
            received: when.clone(),
            sent: when,
            read,
            flagged: false,
            draft: false,
            attachments: false,
            categories: Vec::new(),
            importance: "Normal".into(),
            message_id: format!("<{n}@example.org>"),
            preview: format!("Hello about {subject}."),
            bcc: Vec::new(),
            body: (false, String::new()),
            flag: "NotFlagged".into(),
            marker: String::new(),
            files: Vec::new(),
            mime,
        });
        self.item_log.push(ItemLog { rev: self.rev, folder: folder_id.clone(), id: id.clone(), kind: ItemChange::Create });
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            f.total = f.total.saturating_add(1);
            f.unread = f.unread.saturating_add(u32::from(!read));
        }
        id
    }

    /// The item with this id.
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub(crate) fn touch(&mut self, id: &str, kind: ItemChange, edit: impl FnOnce(&mut Item)) -> bool {
        self.rev = self.rev.saturating_add(1);
        let rev = self.rev;
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { return false };
        edit(item);
        item.change_key = format!("CQAAABYAAAA{rev}");
        let folder = item.folder.clone();
        self.item_log.push(ItemLog { rev, folder, id: id.to_owned(), kind });
        true
    }

    /// Changes an item's subject: an `Update`.
    pub fn edit_subject(&mut self, id: &str, subject: &str) -> bool {
        self.touch(id, ItemChange::Update, |i| i.subject = subject.to_owned())
    }

    /// Reads or unreads an item: a `ReadFlagChange`.
    pub fn set_read(&mut self, id: &str, read: bool) -> bool {
        self.touch(id, ItemChange::Read, |i| i.read = read)
    }

    /// Takes an item away: a `Delete`.
    pub fn remove(&mut self, id: &str) -> bool {
        self.rev = self.rev.saturating_add(1);
        let Some(at) = self.items.iter().position(|i| i.id == id) else { return false };
        let item = self.items.remove(at);
        self.item_log.push(ItemLog { rev: self.rev, folder: item.folder, id: id.to_owned(), kind: ItemChange::Delete });
        true
    }

    /// Makes a folder under the top of the mail; its id.
    pub fn add_folder(&mut self, name: &str) -> String {
        let id = self.next_id("folder");
        self.rev = self.rev.saturating_add(1);
        let parent = self.folder_id("msgfolderroot").unwrap_or_default();
        self.folders.push(Folder {
            distinguished: String::new(),
            id: id.clone(),
            name: name.into(),
            total: 0,
            unread: 0,
            parent,
            class: "IPF.Note".into(),
        });
        self.folder_log.push(FolderLog { rev: self.rev, id: id.clone(), kind: "Create" });
        id
    }

    /// Renames a folder: an `Update`.
    pub fn rename_folder(&mut self, id: &str, name: &str) -> bool {
        self.rev = self.rev.saturating_add(1);
        let Some(folder) = self.folders.iter_mut().find(|f| f.id == id) else { return false };
        folder.name = name.to_owned();
        self.folder_log.push(FolderLog { rev: self.rev, id: id.to_owned(), kind: "Update" });
        true
    }

    /// Takes a folder away, and the mail in it: a `Delete`.
    pub fn delete_folder(&mut self, id: &str) -> bool {
        self.rev = self.rev.saturating_add(1);
        let Some(at) = self.folders.iter().position(|f| f.id == id) else { return false };
        self.folders.remove(at);
        self.items.retain(|i| i.folder != id);
        self.folder_log.push(FolderLog { rev: self.rev, id: id.to_owned(), kind: "Delete" });
        true
    }

    /// Makes the server forget every sync state it gave: the next one it is handed is
    /// `ErrorInvalidSyncStateData`.
    pub fn forget_sync_states(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
    }

    fn state_text(&self, parts: &[&str]) -> String {
        format!("S{}|{}", self.epoch, parts.join("|"))
    }

    /// The parts of a sync state this server gave and still knows; none otherwise.
    fn state_parts<'s>(&self, text: &'s str) -> Option<Vec<&'s str>> {
        let rest = text.strip_prefix('S')?;
        let (epoch, rest) = rest.split_once('|')?;
        (epoch.parse::<u32>().ok()? == self.epoch).then(|| rest.split('|').collect())
    }
}

pub(crate) fn ok_message(operation: &str, inner: &str) -> String {
    format!("<m:{operation}ResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>{inner}</m:{operation}ResponseMessage>")
}

fn invalid_state(operation: &str) -> (u16, String, String) {
    (200, "ErrorInvalidSyncStateData".into(), response(operation, &error_message(operation, "ErrorInvalidSyncStateData")))
}

fn people(name: &str, list: &[(String, String)]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let all: String = list
        .iter()
        .map(|(n, a)| {
            format!("<t:Mailbox><t:Name>{}</t:Name><t:EmailAddress>{}</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType><t:MailboxType>Mailbox</t:MailboxType></t:Mailbox>", escape(n), escape(a))
        })
        .collect();
    format!("<t:{name}>{all}</t:{name}>")
}

impl State {
    /// The `t:Message` of an item as the shape asks for it.
    fn message_xml(&self, item: &Item, shape: &str, extra: &[String]) -> String {
        let hostile =
            self.hostile == Some(Hostile::BadItems) && (self.bad_items_in.is_empty() || self.bad_items_in.split(',').any(|f| f == item.folder));
        let long = "x".repeat(2_000);
        let id = if hostile { long.as_str() } else { item.id.as_str() };
        let mut xml = format!("<t:Message><t:ItemId Id=\"{}\" ChangeKey=\"{}\"/>", escape(id), escape(&item.change_key));
        let wants = |uri: &str| extra.iter().any(|e| e == uri);
        if shape != "IdOnly" {
            let subject = if hostile { "x".repeat(20_000) } else { item.subject.clone() };
            xml.push_str(&format!(
                "<t:ParentFolderId Id=\"{}\" ChangeKey=\"AQAAAA==\"/><t:ItemClass>IPM.Note</t:ItemClass><t:Subject>{}</t:Subject><t:DateTimeReceived>{}</t:DateTimeReceived><t:Size>{}</t:Size><t:Importance>{}</t:Importance><t:IsSubmitted>false</t:IsSubmitted><t:IsDraft>{}</t:IsDraft><t:DateTimeSent>{}</t:DateTimeSent><t:HasAttachments>{}</t:HasAttachments>",
                escape(&item.folder),
                escape(&subject),
                item.received,
                item.mime.len(),
                item.importance,
                item.draft,
                item.sent,
                item.attachments
            ));
            xml.push_str(&people("From", std::slice::from_ref(&item.from)));
            xml.push_str(&format!("<t:InternetMessageId>{}</t:InternetMessageId><t:IsRead>{}</t:IsRead>", escape(&item.message_id), item.read));
        }
        if wants("message:ToRecipients") {
            xml.push_str(&people("ToRecipients", &item.to));
        }
        if wants("message:CcRecipients") {
            xml.push_str(&people("CcRecipients", &item.cc));
        }
        if wants("item:Categories") && !item.categories.is_empty() {
            let all: String = item.categories.iter().map(|c| format!("<t:String>{}</t:String>", escape(c))).collect();
            xml.push_str(&format!("<t:Categories>{all}</t:Categories>"));
        }
        if wants("item:Preview") {
            xml.push_str(&format!("<t:Preview>{}</t:Preview>", escape(&item.preview)));
        }
        if wants("item:Flag") {
            xml.push_str(&format!("<t:Flag><t:FlagStatus>{}</t:FlagStatus></t:Flag>", if item.flagged { item.flag.as_str() } else { "NotFlagged" }));
        }
        if wants("item:Attachments") && !item.files.is_empty() {
            let all: String = item
                .files
                .iter()
                .map(|(id, name, bytes)| {
                    format!(
                        "<t:FileAttachment><t:AttachmentId Id=\"{}\"/><t:Name>{}</t:Name><t:Size>{}</t:Size></t:FileAttachment>",
                        escape(id),
                        escape(name),
                        bytes.len()
                    )
                })
                .collect();
            xml.push_str(&format!("<t:Attachments>{all}</t:Attachments>"));
        }
        xml.push_str("</t:Message>");
        xml
    }

    /// `ItemShape` of `SyncFolderItems`/`GetItem`: the base shape, whether the MIME content is
    /// asked for, and the extra `FieldURI`s.
    fn item_shape(shape: &Node, mime_allowed: bool) -> Result<(String, bool, Vec<String>), Refusal> {
        Check::is(shape, MESSAGES, "ItemShape", &[])?;
        let order: &[&str] =
            if mime_allowed { &["BaseShape", "IncludeMimeContent", "AdditionalProperties"] } else { &["BaseShape", "AdditionalProperties"] };
        let inside = Check::children(shape, TYPES, order)?;
        let base = Check::required(shape, inside.first().copied().flatten(), "BaseShape")?;
        Check::is(base, TYPES, "BaseShape", &[])?;
        Check::no_children(base)?;
        let base = base.text.trim();
        if !["IdOnly", "Default", "AllProperties"].contains(&base) {
            return Err(invalid("BaseShape is IdOnly, Default or AllProperties"));
        }
        let mut mime = false;
        if mime_allowed && let Some(node) = inside.get(1).copied().flatten() {
            Check::is(node, TYPES, "IncludeMimeContent", &[])?;
            Check::no_children(node)?;
            mime = match node.text.trim() {
                "true" => true,
                "false" => false,
                _ => return Err(invalid("IncludeMimeContent is true or false")),
            };
        }
        let mut extra = Vec::new();
        if let Some(node) = inside.get(order.len().saturating_sub(1)).copied().flatten() {
            Check::is(node, TYPES, "AdditionalProperties", &[])?;
            Check::no_text(node)?;
            for one in &node.children {
                Check::is(one, TYPES, "FieldURI", &["FieldURI"])?;
                Check::empty(one)?;
                let uri = Check::attribute(one, "FieldURI")?;
                if !FIELD_URIS.contains(&uri) {
                    return Err(invalid(format!("{uri} is no property this server knows")));
                }
                extra.push(uri.to_owned());
            }
        }
        Ok((base.to_owned(), mime, extra))
    }

    /// The folder a `SyncFolderId` names.
    fn sync_folder(&self, node: &Node) -> Result<Folder, Refusal> {
        Check::is(node, MESSAGES, "SyncFolderId", &[])?;
        Check::no_text(node)?;
        let [one] = node.children.as_slice() else { return Err(invalid("SyncFolderId names one folder")) };
        let found = if one.is(TYPES, "DistinguishedFolderId") {
            Check::is(one, TYPES, "DistinguishedFolderId", &["Id", "ChangeKey"])?;
            self.folder(Check::attribute(one, "Id")?, true)
        } else if one.is(TYPES, "FolderId") {
            Check::is(one, TYPES, "FolderId", &["Id", "ChangeKey"])?;
            self.folder(Check::attribute(one, "Id")?, false)
        } else {
            return Err(invalid("SyncFolderId has no place for that"));
        };
        Check::empty(one)?;
        found.cloned().ok_or(Refusal { code: "ErrorFolderNotFound", why: "no such folder".into() })
    }

    pub(crate) fn sync_hierarchy(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "SyncFolderHierarchy", &[])?;
        let parts = Check::children(operation, MESSAGES, &["FolderShape", "SyncFolderId", "SyncState"])?;
        let shape = Check::required(operation, parts.first().copied().flatten(), "FolderShape")?;
        Check::is(shape, MESSAGES, "FolderShape", &[])?;
        let inside = Check::children(shape, TYPES, &["BaseShape", "AdditionalProperties"])?;
        let base = Check::required(shape, inside.first().copied().flatten(), "BaseShape")?;
        Check::is(base, TYPES, "BaseShape", &[])?;
        if !["IdOnly", "Default", "AllProperties"].contains(&base.text.trim()) {
            return Err(invalid("BaseShape is IdOnly, Default or AllProperties"));
        }
        let asked_class = inside
            .get(1)
            .copied()
            .flatten()
            .is_some_and(|extra| extra.children.iter().any(|p| p.attribute("FieldURI") == Some("folder:FolderClass")));
        let with_class = !self.class_only_when_asked || asked_class || base.text.trim() == "AllProperties";
        let root = self.sync_folder(Check::required(operation, parts.get(1).copied().flatten(), "SyncFolderId")?)?;
        if root.distinguished != "msgfolderroot" {
            return Err(Refusal { code: "ErrorInvalidRequest", why: "this stand-in synchronises the top of the mail".into() });
        }
        let given = parts.get(2).copied().flatten();
        let since = match given {
            None => None,
            Some(node) => {
                Check::is(node, MESSAGES, "SyncState", &[])?;
                Check::no_children(node)?;
                match self.state_parts(node.text.trim()).as_deref() {
                    Some(["h", rev]) => Some(rev.parse::<u64>().map_err(|_| invalid("bad state"))?),
                    _ => return Ok(invalid_state("SyncFolderHierarchy")),
                }
            }
        };
        let folder_xml = |folder: &Folder, tag: &str| {
            let element = match folder.class.as_str() {
                c if c.starts_with("IPF.Appointment") => "CalendarFolder",
                c if c.starts_with("IPF.Contact") => "ContactsFolder",
                c if c.starts_with("IPF.Task") => "TasksFolder",
                _ => "Folder",
            };
            let class = if with_class { format!("<t:FolderClass>{}</t:FolderClass>", escape(&folder.class)) } else { String::new() };
            format!(
                "<t:{tag}><t:{element}><t:FolderId Id=\"{id}\" ChangeKey=\"AQAAABYA\"/><t:ParentFolderId Id=\"{parent}\" ChangeKey=\"AQAAAA==\"/>{class}<t:DisplayName>{name}</t:DisplayName><t:TotalCount>{total}</t:TotalCount><t:ChildFolderCount>0</t:ChildFolderCount><t:UnreadCount>{unread}</t:UnreadCount></t:{element}></t:{tag}>",
                id = escape(&folder.id),
                parent = escape(&folder.parent),
                name = escape(&folder.name),
                total = folder.total,
                unread = folder.unread
            )
        };
        let mut changes = String::new();
        match since {
            None => {
                for folder in self.folders.iter().filter(|f| f.distinguished != "root" && f.distinguished != "msgfolderroot") {
                    changes.push_str(&folder_xml(folder, "Create"));
                }
            }
            Some(since) => {
                for log in self.folder_log.iter().filter(|l| l.rev > since) {
                    match (log.kind, self.folders.iter().find(|f| f.id == log.id)) {
                        ("Delete", _) => {
                            changes.push_str(&format!("<t:Delete><t:FolderId Id=\"{}\" ChangeKey=\"AQAAAA==\"/></t:Delete>", escape(&log.id)))
                        }
                        (kind, Some(folder)) => changes.push_str(&folder_xml(folder, kind)),
                        _ => {}
                    }
                }
            }
        }
        let state = self.state_text(&["h", &self.rev.to_string()]);
        let inner = format!(
            "<m:SyncState>{state}</m:SyncState><m:IncludesLastFolderInRange>true</m:IncludesLastFolderInRange><m:Changes>{changes}</m:Changes>"
        );
        Ok((200, "NoError".into(), response("SyncFolderHierarchy", &ok_message("SyncFolderHierarchy", &inner))))
    }

    pub(crate) fn sync_items(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "SyncFolderItems", &[])?;
        let parts = Check::children(operation, MESSAGES, &["ItemShape", "SyncFolderId", "SyncState", "Ignore", "MaxChangesReturned", "SyncScope"])?;
        let shape = Check::required(operation, parts.first().copied().flatten(), "ItemShape")?;
        let (base, _, extra) = Self::item_shape(shape, false)?;
        let folder = self.sync_folder(Check::required(operation, parts.get(1).copied().flatten(), "SyncFolderId")?)?;
        let max = Check::required(operation, parts.get(4).copied().flatten(), "MaxChangesReturned")?;
        Check::is(max, MESSAGES, "MaxChangesReturned", &[])?;
        Check::no_children(max)?;
        let max: usize = max.text.trim().parse().map_err(|_| invalid("MaxChangesReturned is a number"))?;
        if !(1..=512).contains(&max) {
            return Err(invalid("MaxChangesReturned is from 1 to 512"));
        }
        let max = if self.ignores_max { usize::MAX } else { max };
        if let Some(scope) = parts.get(5).copied().flatten() {
            Check::is(scope, MESSAGES, "SyncScope", &[])?;
            if !["NormalItems", "NormalAndAssociatedItems"].contains(&scope.text.trim()) {
                return Err(invalid("SyncScope is NormalItems or NormalAndAssociatedItems"));
            }
        }
        if self.lacks_preview && extra.iter().any(|e| e == "item:Preview" || e == "item:Flag") {
            return Ok((
                200,
                "ErrorInvalidPropertyRequest".into(),
                response("SyncFolderItems", &error_message("SyncFolderItems", "ErrorInvalidPropertyRequest")),
            ));
        }
        // Where the read goes on from: the state it was given, or the start.
        enum From {
            Listing { done: usize, rev: u64 },
            Since(u64),
        }
        let from = match parts.get(2).copied().flatten() {
            None => From::Listing { done: 0, rev: self.rev },
            Some(node) => {
                Check::is(node, MESSAGES, "SyncState", &[])?;
                Check::no_children(node)?;
                match self.state_parts(node.text.trim()).as_deref() {
                    Some(["i", id, done, rev]) if *id == folder.id => {
                        From::Listing { done: done.parse().map_err(|_| invalid("bad state"))?, rev: rev.parse().map_err(|_| invalid("bad state"))? }
                    }
                    Some(["d", id, rev]) if *id == folder.id => From::Since(rev.parse().map_err(|_| invalid("bad state"))?),
                    _ => return Ok(invalid_state("SyncFolderItems")),
                }
            }
        };
        let in_folder: Vec<&Item> = self.items.iter().filter(|i| i.folder == folder.id).collect();
        let mut changes = String::new();
        let (state, last) = match from {
            From::Listing { done, rev } => {
                let page: Vec<&&Item> = in_folder.iter().skip(done).take(max).collect();
                for item in &page {
                    changes.push_str(&format!("<t:Create>{}</t:Create>", self.message_xml(item, &base, &extra)));
                }
                let reached = done.saturating_add(page.len());
                if reached >= in_folder.len() {
                    (self.state_text(&["d", &folder.id, &rev.to_string()]), true)
                } else {
                    (self.state_text(&["i", &folder.id, &reached.to_string(), &rev.to_string()]), false)
                }
            }
            From::Since(since) => {
                let log: Vec<&ItemLog> = self.item_log.iter().filter(|l| l.rev > since && l.folder == folder.id).collect();
                let mut reached = since;
                for entry in log.iter().take(max) {
                    reached = entry.rev;
                    let item = self.item(&entry.id);
                    match (entry.kind, item) {
                        (ItemChange::Delete, _) => {
                            changes.push_str(&format!("<t:Delete><t:ItemId Id=\"{}\" ChangeKey=\"CQAAAA==\"/></t:Delete>", escape(&entry.id)))
                        }
                        (ItemChange::Read, Some(item)) => changes.push_str(&format!(
                            "<t:ReadFlagChange><t:ItemId Id=\"{}\" ChangeKey=\"{}\"/><t:IsRead>{}</t:IsRead></t:ReadFlagChange>",
                            escape(&item.id),
                            escape(&item.change_key),
                            item.read
                        )),
                        (ItemChange::Create, Some(item)) => {
                            changes.push_str(&format!("<t:Create>{}</t:Create>", self.message_xml(item, &base, &extra)))
                        }
                        (ItemChange::Update, Some(item)) => {
                            changes.push_str(&format!("<t:Update>{}</t:Update>", self.message_xml(item, &base, &extra)))
                        }
                        _ => {}
                    }
                }
                let more = log.len() > max;
                if !more {
                    reached = self.rev.max(reached);
                }
                (self.state_text(&["d", &folder.id, &reached.to_string()]), !more)
            }
        };
        let inner = format!(
            "<m:SyncState>{state}</m:SyncState><m:IncludesLastItemInRange>{last}</m:IncludesLastItemInRange><m:Changes>{changes}</m:Changes>"
        );
        Ok((200, "NoError".into(), response("SyncFolderItems", &ok_message("SyncFolderItems", &inner))))
    }

    pub(crate) fn get_item(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "GetItem", &[])?;
        let parts = Check::children(operation, MESSAGES, &["ItemShape", "ItemIds"])?;
        let shape = Check::required(operation, parts.first().copied().flatten(), "ItemShape")?;
        let (base, mime, extra) = Self::item_shape(shape, true)?;
        let ids = Check::required(operation, parts.get(1).copied().flatten(), "ItemIds")?;
        Check::is(ids, MESSAGES, "ItemIds", &[])?;
        Check::no_text(ids)?;
        if ids.children.is_empty() {
            return Err(invalid("ItemIds names at least one item"));
        }
        let (mut messages, mut first) = (String::new(), String::new());
        for one in &ids.children {
            Check::is(one, TYPES, "ItemId", &["Id", "ChangeKey"])?;
            Check::empty(one)?;
            let id = Check::attribute(one, "Id")?;
            let code = match self.item(id) {
                Some(item) => {
                    let mut xml = self.message_xml(item, &base, &extra);
                    if mime {
                        let content = format!("<t:MimeContent CharacterSet=\"UTF-8\">{}</t:MimeContent>", base64(&item.mime));
                        xml = xml.replacen("<t:ItemId", &format!("{content}<t:ItemId"), 1);
                    }
                    messages.push_str(&ok_message("GetItem", &format!("<m:Items>{xml}</m:Items>")));
                    "NoError"
                }
                None => {
                    messages.push_str(&error_message("GetItem", "ErrorItemNotFound"));
                    "ErrorItemNotFound"
                }
            };
            if first.is_empty() {
                first = code.to_owned();
            }
        }
        Ok((200, first, response("GetItem", &messages)))
    }
}
