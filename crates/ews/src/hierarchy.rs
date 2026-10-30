// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `SyncFolderHierarchy` ([MS-OXWSSYNC]): the folders of the mailbox, and what changed in them
//! since the sync state the server gave last time.
//!
//! The mail folders lie under `msgfolderroot`; that is what is synchronised. The first read
//! (no state) brings every folder as a `Create`; later reads bring the `Create`, `Update` and
//! `Delete` of the folders that changed. A folder that is not a mail folder (a calendar, contacts,
//! tasks, notes: its `FolderClass` is not `IPF.Note`) is skipped and counted: it is not this
//! slice's, and the count is told once per run (E7), never its name.

use mailcraft_model::FolderKind;
use mailcraft_model::sync::GraphFolder;

use crate::error::Error;
use crate::folder::{FolderId, valid_id};
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// Most folders one answer is read for.
pub const MAX_FOLDERS: usize = 20_000;
/// The longest folder name kept, in characters.
const MAX_NAME: usize = 255;

/// The request `SyncFolderHierarchy` of the mail folders, from `state` (empty: from the start).
#[derive(Clone, PartialEq, Eq)]
pub struct SyncFolderHierarchy {
    state: String,
}

impl std::fmt::Debug for SyncFolderHierarchy {
    /// Whether there is a state; the state is the server's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncFolderHierarchy").field("state", &!self.state.is_empty()).finish()
    }
}

/// What the server said in answer.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HierarchyPage {
    /// The state to go on from.
    pub state: String,
    /// The answer reaches the last folder (`IncludesLastFolderInRange`).
    pub last: bool,
    /// Folders created or updated, as the server says them. `kind` is `Custom` and `parent` is the
    /// server's id of the folder above (the root's, for a folder at the top): the caller knows
    /// which are which.
    pub changed: Vec<GraphFolder>,
    /// The ids of the folders deleted.
    pub deleted: Vec<String>,
    /// How many folders were passed over for not being mail folders, or for not being readable.
    pub skipped: usize,
}

impl std::fmt::Debug for HierarchyPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HierarchyPage")
            .field("last", &self.last)
            .field("changed", &self.changed.len())
            .field("deleted", &self.deleted.len())
            .field("skipped", &self.skipped)
            .finish()
    }
}

impl SyncFolderHierarchy {
    /// Reads from `state` (the server's; empty to begin).
    pub fn new(state: &str) -> Self {
        Self { state: state.to_owned() }
    }

    /// The operation as the schema has it: `FolderShape`, `SyncFolderId`, `SyncState`.
    pub fn element(&self) -> Element {
        // (The class is asked for by name: some servers leave it out of the Default shape, and a
        // folder with none would be taken for a folder of mail.)
        let shape = Element::new("m:FolderShape")
            .child(Element::new("t:BaseShape").text("Default"))
            .child(Element::new("t:AdditionalProperties").child(Element::new("t:FieldURI").attr("FieldURI", "folder:FolderClass")));
        let root = Element::new("m:SyncFolderId").child(Element::new("t:DistinguishedFolderId").attr("Id", "msgfolderroot"));
        let mut operation = Element::new("m:SyncFolderHierarchy").child(shape).child(root);
        if !self.state.is_empty() {
            operation = operation.child(Element::new("m:SyncState").text(&self.state));
        }
        operation
    }

    /// The server's answer. A state the server no longer knows is [`Error::Server`] of
    /// `InvalidSyncState`: the caller reads afresh.
    pub fn answer(&self, reply: &Reply) -> Result<HierarchyPage, Error> {
        let messages = reply.messages("SyncFolderHierarchy")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let state = node.child_in(MESSAGES, "SyncState").map(|n| n.text.trim().to_owned()).filter(|s| !s.is_empty()).ok_or(Error::Unexpected)?;
        // (Absent, the server has no more to say.)
        let last = node.child_in(MESSAGES, "IncludesLastFolderInRange").is_none_or(|n| n.text.trim() != "false");
        let mut page = HierarchyPage { state, last, ..HierarchyPage::default() };
        let Some(changes) = node.child_in(MESSAGES, "Changes") else { return Ok(page) };
        for change in changes.children.iter().filter(|c| *c.namespace == *TYPES) {
            if page.changed.len().saturating_add(page.deleted.len()).saturating_add(page.skipped) >= MAX_FOLDERS {
                return Err(Error::Unexpected);
            }
            match change.name.as_str() {
                "Create" | "Update" => match read_folder(change) {
                    Some(folder) => page.changed.push(folder),
                    None => page.skipped = page.skipped.saturating_add(1),
                },
                "Delete" => {
                    if let Some(id) = change.child_in(TYPES, "FolderId").and_then(|id| id.attribute("Id")).filter(|id| valid_id(id)) {
                        page.deleted.push(id.to_owned());
                    }
                }
                _ => {}
            }
        }
        Ok(page)
    }
}

/// The folder in a `Create` or `Update`, when it is a mail folder that can be taken in.
fn read_folder(change: &Node) -> Option<GraphFolder> {
    let folder = change.children.iter().find(|child| *child.namespace == *TYPES)?;
    // A calendar, contacts, tasks or search folder is no folder of mail, whatever class it says
    // (or doesn't: Grommunio's listing gave none without being asked).
    if ["CalendarFolder", "ContactsFolder", "TasksFolder", "SearchFolder"].contains(&&*folder.name) {
        return None;
    }
    let id = folder.child_in(TYPES, "FolderId")?.attribute("Id").and_then(FolderId::new)?;
    let class = folder.child_in(TYPES, "FolderClass").map_or("", |c| c.text.trim());
    // (A folder without a class is a plain folder of mail; `IPF.Note.*` are its kinds.)
    if !(class.is_empty() || class == "IPF.Note" || class.starts_with("IPF.Note.")) {
        return None;
    }
    let parent = folder.child_in(TYPES, "ParentFolderId").and_then(|p| p.attribute("Id")).and_then(FolderId::new);
    let name: String = folder.child_in(TYPES, "DisplayName")?.text.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if name.is_empty() {
        return None;
    }
    let count = |name: &str| folder.child_in(TYPES, name).and_then(|n| n.text.trim().parse::<u32>().ok());
    let (total, unread) = (count("TotalCount"), count("UnreadCount"));
    Some(GraphFolder {
        id: id.as_str().to_owned(),
        parent: parent.map(|p| p.as_str().to_owned()).unwrap_or_default(),
        name,
        kind: FolderKind::Custom,
        total: total.unwrap_or(0),
        unread: unread.unwrap_or(0),
        counted: total.is_some() && unread.is_some(),
    })
}
