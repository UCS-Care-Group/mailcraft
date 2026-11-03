// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `GetFolder` ([MS-OXWSFOLD]): ask for folders by their well-known names, read what the server
//! says of each. It is how an account is tested: if the Inbox can be had, the address, the
//! credentials and the service all work.

use crate::error::Error;
use crate::reply::Reply;
use crate::soap::{Element, TYPES};
use crate::xml::Node;

/// The longest folder id kept: real ones are about 150 characters.
const MAX_ID: usize = 1_024;
/// The most folders one request asks for.
pub const MAX_FOLDERS: usize = 100;

/// A folder every mailbox has under a name of its own (`DistinguishedFolderIdNameType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistinguishedFolder {
    /// The top of the mailbox (`root`).
    Root,
    /// The top of the mail folders (`msgfolderroot`).
    MessageRoot,
    /// Inbox.
    Inbox,
    /// Drafts.
    Drafts,
    /// Sent Items.
    SentItems,
    /// Deleted Items.
    DeletedItems,
    /// Junk E-mail.
    JunkEmail,
    /// Outbox.
    Outbox,
}

impl DistinguishedFolder {
    /// The name the schema gives it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::MessageRoot => "msgfolderroot",
            Self::Inbox => "inbox",
            Self::Drafts => "drafts",
            Self::SentItems => "sentitems",
            Self::DeletedItems => "deleteditems",
            Self::JunkEmail => "junkemail",
            Self::Outbox => "outbox",
        }
    }
}

/// Whether `text` may be a server's id (of a folder or an item) or a change key: short, and with
/// no control character. An id is opaque: nothing else is assumed of it (the model holds the same
/// rule, `id_ok`), and it only ever travels as an escaped attribute.
pub(crate) fn valid_id(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_ID && !text.chars().any(char::is_control)
}

/// A server's id for a folder: opaque, short, and with no control character. It stays inside the
/// crate's users and out of every log, so `Debug` hides it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FolderId(String);

impl FolderId {
    /// Checks an id received from a server.
    pub fn new(text: &str) -> Option<Self> {
        valid_id(text).then(|| Self(text.to_owned()))
    }

    /// The id, for a request.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for FolderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FolderId(..)")
    }
}

/// What the server says of a folder (`BaseShape` of `Default`).
#[derive(Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// The server's id.
    pub id: FolderId,
    /// The server's change key for it, if given.
    pub change_key: Option<String>,
    /// The folder's name.
    pub display_name: String,
    /// How many items it holds, if the server says.
    pub total_count: Option<u32>,
    /// How many are unread, if the server says.
    pub unread_count: Option<u32>,
    /// How many folders lie in it, if the server says.
    pub child_folder_count: Option<u32>,
}

impl std::fmt::Debug for FolderInfo {
    /// The counts: the id, the change key and the name are the server's and the owner's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderInfo")
            .field("total_count", &self.total_count)
            .field("unread_count", &self.unread_count)
            .field("child_folder_count", &self.child_folder_count)
            .finish()
    }
}

/// The request `GetFolder` for folders by their distinguished names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetFolder {
    folders: Vec<DistinguishedFolder>,
}

impl GetFolder {
    /// Asks for `folders` (at most [`MAX_FOLDERS`]; the rest are left out).
    pub fn new(folders: &[DistinguishedFolder]) -> Self {
        Self { folders: folders.iter().copied().take(MAX_FOLDERS).collect() }
    }

    /// The folders asked for, in order.
    pub fn folders(&self) -> &[DistinguishedFolder] {
        &self.folders
    }

    /// The operation as the schema has it.
    pub fn element(&self) -> Element {
        let shape = Element::new("m:FolderShape").child(Element::new("t:BaseShape").text("Default"));
        let ids = self
            .folders
            .iter()
            .fold(Element::new("m:FolderIds"), |ids, folder| ids.child(Element::new("t:DistinguishedFolderId").attr("Id", folder.as_str())));
        Element::new("m:GetFolder").child(shape).child(ids)
    }

    /// The server's answer: one result for each folder asked for, in order. An answer with more
    /// or fewer messages than there were questions is not the answer.
    pub fn answer(&self, reply: &Reply) -> Result<Vec<Result<FolderInfo, Error>>, Error> {
        let messages = reply.messages("GetFolder")?;
        if messages.len() != self.folders.len() {
            return Err(Error::Unexpected);
        }
        Ok(messages.iter().map(|message| message.ok().and_then(read_folder)).collect())
    }
}

/// The folder in a response message's `Folders`: the first child, whatever kind of folder it is
/// (`Folder`, `CalendarFolder`, `ContactsFolder`, `SearchFolder`, `TasksFolder`).
fn read_folder(message: &Node) -> Result<FolderInfo, Error> {
    let folders = message.child_in(crate::soap::MESSAGES, "Folders").ok_or(Error::Unexpected)?;
    let folder = folders.children.iter().find(|child| *child.namespace == *TYPES).ok_or(Error::Unexpected)?;
    let id_element = folder.child_in(TYPES, "FolderId").ok_or(Error::Unexpected)?;
    let id = id_element.attribute("Id").and_then(FolderId::new).ok_or(Error::Unexpected)?;
    let change_key = id_element.attribute("ChangeKey").filter(|key| FolderId::new(key).is_some()).map(str::to_owned);
    let count = |name: &str| folder.child_in(TYPES, name).and_then(|n| n.text.trim().parse::<u32>().ok());
    Ok(FolderInfo {
        id,
        change_key,
        display_name: folder.child_in(TYPES, "DisplayName").map(|name| name.text.trim().to_owned()).unwrap_or_default(),
        total_count: count("TotalCount"),
        unread_count: count("UnreadCount"),
        child_folder_count: count("ChildFolderCount"),
    })
}
