// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `GetFolder` ([MS-OXWSFOLD]): ask for folders by their well-known names, read what the server
//! says of each. It is how an account is tested: if the Inbox can be had, the address, the
//! credentials and the service all work.

use mailcraft_model::Rights;

use crate::error::Error;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
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
    /// Conversation History (`conversationhistory`), where a chat program files its talks.
    ConversationHistory,
    /// Sync Issues (`syncissues`), and the three folders in it.
    SyncIssues,
    /// Conflicts (`conflicts`), in Sync Issues.
    Conflicts,
    /// Local Failures (`localfailures`), in Sync Issues.
    LocalFailures,
    /// Server Failures (`serverfailures`), in Sync Issues.
    ServerFailures,
    /// The default calendar.
    Calendar,
    /// The default address book.
    Contacts,
    /// The default to-do list.
    Tasks,
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
            Self::ConversationHistory => "conversationhistory",
            Self::SyncIssues => "syncissues",
            Self::Conflicts => "conflicts",
            Self::LocalFailures => "localfailures",
            Self::ServerFailures => "serverfailures",
            Self::Calendar => "calendar",
            Self::Contacts => "contacts",
            Self::Tasks => "tasks",
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
    /// Its `FolderClass` (`IPF.Appointment` for a calendar, `IPF.Contact` for an address book,
    /// `IPF.Task` for a to-do list), when the server says.
    pub class: String,
    /// What the account may do with it, from `EffectiveRights`; `Rights::Edit` when the server
    /// didn't say.
    pub rights: Rights,
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
    mailbox: String,
    shape: Shape,
}

/// Which properties a folder's answer has to hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// The default ones.
    #[default]
    Default,
    /// Also its class and what may be done with it, which a listing of calendars, address books
    /// and to-do lists needs (`FolderShape` with added `folder:FolderClass` and
    /// `folder:EffectiveRights`).
    Full,
}

impl GetFolder {
    /// Asks for `folders` (at most [`MAX_FOLDERS`]; the rest are left out).
    pub fn new(folders: &[DistinguishedFolder]) -> Self {
        Self { folders: folders.iter().copied().take(MAX_FOLDERS).collect(), mailbox: String::new(), shape: Shape::Default }
    }

    /// Asks for them in the mailbox at `address` rather than the account's own: a calendar or an
    /// address book shared with the owner, read with the owner's sign-in.
    #[must_use]
    pub fn in_mailbox(mut self, address: &str) -> Self {
        self.mailbox = address.trim().to_owned();
        self
    }

    /// Asks for the class and the rights as well.
    #[must_use]
    pub fn full(mut self) -> Self {
        self.shape = Shape::Full;
        self
    }

    /// The folders asked for, in order.
    pub fn folders(&self) -> &[DistinguishedFolder] {
        &self.folders
    }

    /// The operation as the schema has it.
    pub fn element(&self) -> Element {
        let mut shape = Element::new("m:FolderShape").child(Element::new("t:BaseShape").text("Default"));
        if self.shape == Shape::Full {
            shape = shape.child(
                Element::new("t:AdditionalProperties")
                    .child(Element::new("t:FieldURI").attr("FieldURI", "folder:FolderClass"))
                    .child(Element::new("t:FieldURI").attr("FieldURI", "folder:EffectiveRights")),
            );
        }
        let ids = self.folders.iter().fold(Element::new("m:FolderIds"), |ids, folder| {
            let mut id = Element::new("t:DistinguishedFolderId").attr("Id", folder.as_str());
            if !self.mailbox.is_empty() {
                id = id.child(mailbox(&self.mailbox));
            }
            ids.child(id)
        });
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

/// A mailbox an operation is asked in (`t:Mailbox`), with its address and nothing else.
pub(crate) fn mailbox(address: &str) -> Element {
    Element::new("t:Mailbox").child(Element::new("t:EmailAddress").text(address.trim()))
}

/// The rights `EffectiveRights` says.
fn rights(folder: &Node) -> Rights {
    let Some(rights) = folder.child_in(TYPES, "EffectiveRights") else { return Rights::Edit };
    let yes = |name: &str| rights.child_in(TYPES, name).is_some_and(|n| n.text.trim().eq_ignore_ascii_case("true"));
    if yes("Modify") || yes("CreateContents") || yes("Delete") {
        Rights::Edit
    } else if yes("Read") {
        Rights::Read
    } else {
        Rights::Edit
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
        class: folder.child_in(TYPES, "FolderClass").map(|n| n.text.trim().to_owned()).unwrap_or_default(),
        rights: rights(folder),
    })
}

/// A folder whose class is one of the calendar, address-book and to-do kinds; the folders below
/// `msgfolderroot` are listed to find the ones that are not the default.
const DIRECTORY_CLASSES: [&str; 3] = ["IPF.Appointment", "IPF.Contact", "IPF.Task"];

/// The request `FindFolder`: every folder below one parent, or the account's whole tree (
/// `msgfolderroot`), with the class and the rights of each.
#[derive(Clone, PartialEq, Eq)]
pub struct FindFolder {
    parent: DistinguishedFolder,
    mailbox: String,
    deep: bool,
    offset: u32,
    max: u32,
}

impl std::fmt::Debug for FindFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindFolder").field("parent", &self.parent).field("deep", &self.deep).field("offset", &self.offset).finish()
    }
}

/// One page of a folder listing.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FoldersPage {
    /// The folders in it.
    pub folders: Vec<FolderInfo>,
    /// There are no more folders after this page.
    pub last: bool,
}

impl std::fmt::Debug for FoldersPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldersPage").field("folders", &self.folders.len()).field("last", &self.last).finish()
    }
}

impl FindFolder {
    /// Lists the folders below `parent`: all of them when `deep`, else its own children, `max` at
    /// a time from `offset`.
    pub fn new(parent: DistinguishedFolder, deep: bool, offset: u32, max: u32) -> Self {
        Self { parent, mailbox: String::new(), deep, offset, max: max.clamp(1, 1_000) }
    }

    /// Lists them in the mailbox at `address`.
    #[must_use]
    pub fn in_mailbox(mut self, address: &str) -> Self {
        self.mailbox = address.trim().to_owned();
        self
    }

    /// The operation as the schema has it.
    pub fn element(&self) -> Element {
        let shape = Element::new("m:FolderShape").child(Element::new("t:BaseShape").text("Default")).child(
            Element::new("t:AdditionalProperties")
                .child(Element::new("t:FieldURI").attr("FieldURI", "folder:FolderClass"))
                .child(Element::new("t:FieldURI").attr("FieldURI", "folder:EffectiveRights")),
        );
        let page = Element::new("m:IndexedPageFolderView")
            .attr("MaxEntriesReturned", &self.max.to_string())
            .attr("Offset", &self.offset.to_string())
            .attr("BasePoint", "Beginning");
        let mut parent = Element::new("t:DistinguishedFolderId").attr("Id", self.parent.as_str());
        if !self.mailbox.is_empty() {
            parent = parent.child(mailbox(&self.mailbox));
        }
        Element::new("m:FindFolder")
            .attr("Traversal", if self.deep { "Deep" } else { "Shallow" })
            .child(shape)
            .child(page)
            .child(Element::new("m:ParentFolderIds").child(parent))
    }

    /// The server's answer: the folders of the page, and whether it is the last.
    pub fn answer(&self, reply: &Reply) -> Result<FoldersPage, Error> {
        let messages = reply.messages("FindFolder")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let root = node.child_in(MESSAGES, "RootFolder").ok_or(Error::Unexpected)?;
        let mut page = FoldersPage::default();
        if let Some(folders) = root.child_in(TYPES, "Folders") {
            for folder in &folders.children {
                if *folder.namespace != *TYPES {
                    continue;
                }
                // (A folder that can't be read as one is left out: it is no folder of this account's
                // calendars, books or to-dos either way.)
                if let Ok(folder) = read_folder_node(folder) {
                    page.folders.push(folder);
                }
            }
        }
        page.last = root.attribute("IncludesLastItemInRange").is_none_or(|last| !last.trim().eq_ignore_ascii_case("false"));
        Ok(page)
    }
}

/// A folder element (not inside a response message, as `GetFolder` has it).
fn read_folder_node(folder: &Node) -> Result<FolderInfo, Error> {
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
        class: folder.child_in(TYPES, "FolderClass").map(|n| n.text.trim().to_owned()).unwrap_or_default(),
        rights: rights(folder),
    })
}

/// Whether a folder's class is a calendar, an address book or a to-do list.
pub fn directory_class(class: &str) -> Option<&'static str> {
    DIRECTORY_CLASSES.iter().copied().find(|known| known.eq_ignore_ascii_case(class.trim()))
}
