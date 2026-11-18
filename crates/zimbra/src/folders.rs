// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The folders of mail (`GetFolderRequest`): the whole tree, which is small, as a complete
//! listing. Folders of appointments, contacts, tasks and documents are left out with all that lies
//! in them, and so are the chat history, saved searches and folders shared from other mailboxes.

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{GraphFolder, MAX_LISTED_FOLDERS};
use serde_json::{Value, json};

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::json;
use crate::soap;

/// The folder every other folder lies in.
const ROOT: &str = "1";
/// Where Zimbra keeps its chat history: a folder of messages, but none of mail.
const CHATS: &str = "14";
/// The first id of a folder made by the owner; every folder below it is the server's own.
const FIRST_USER_ID: u64 = 256;
/// How deep a tree is followed.
const MAX_DEPTH: usize = 64;
/// Longest folder name taken, in characters.
const MAX_NAME: usize = 255;

/// A folder of another mailbox mounted into this one's tree (a `link` in `GetFolderResponse`):
/// what the server shows it as and whose it is, as far as the answer names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mountpoint {
    /// Its id in this mailbox (`<zid>:<rid>`), the one to search under.
    pub id: String,
    /// The name it is mounted under.
    pub name: String,
    /// The owner's address, when the server names it (`owner`).
    pub owner: String,
    /// The owner's mailbox id (`zid`).
    pub zid: String,
    /// The folder's id in the owner's mailbox (`rid`).
    pub rid: String,
    /// Its parent in this mailbox's tree.
    pub parent: String,
    /// What the folder holds (`message`, `appointment`, `contact`, …); only `message` ones are
    /// mountable mail here.
    pub view: String,
}

/// Every folder of mail the server has, parents before what lies in them, and which of them are
/// the server's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The folders, as the model lists them.
    pub folders: Vec<GraphFolder>,
    /// The ids of the server's own folders (the Inbox, Sent, Drafts, Trash, Junk and the like).
    pub well_known: Vec<String>,
    /// The folders of other mailboxes mounted into this one's tree, kept apart: their ids and
    /// messages are another mailbox's, asked for under their qualified ids.
    pub mounted: Vec<Mountpoint>,
    /// The tree was read to its end: a folder that isn't in it is one the server doesn't have.
    pub complete: bool,
    /// Every own id named, for knowing one the server names twice.
    seen: std::collections::HashSet<String>,
}

/// The request for the folder tree.
pub fn request(base: &str, token: &Secret) -> Result<HttpRequest, Error> {
    soap::request(base, "GetFolderRequest", soap::MAIL, json!({}), Some(token))
}

/// What a special folder is, by the id Zimbra gives it in every mailbox.
pub fn kind_of(id: &str) -> FolderKind {
    match id {
        "2" => FolderKind::Inbox,
        "3" => FolderKind::Deleted,
        "4" => FolderKind::Junk,
        "5" => FolderKind::Sent,
        "6" => FolderKind::Drafts,
        _ => FolderKind::Custom,
    }
}

/// The folders of mail of a `GetFolderResponse`.
pub fn answer(response: &HttpResponse) -> Result<Listing, Error> {
    let answer = soap::answer("GetFolderRequest", response)?;
    let mut out = Listing { complete: true, ..Listing::default() };
    if json::bad_list(answer.get("folder")) {
        return Err(Error::Unreadable);
    }
    let tops = json::list(answer.get("folder"));
    if tops.is_empty() {
        return Err(Error::Unreadable);
    }
    for top in tops {
        walk(top, "", 0, &mut out);
    }
    // Mountpoints that hang off no folder of the tree are named at the answer's top.
    for link in json::list(answer.get("link")) {
        mount(link, "", &mut out);
    }
    Ok(out)
}

/// One `link` of a folder: a mountpoint of another mailbox's folder, with `zid` and `rid` naming
/// it in the owner's. One that can't be told whose it is or what it is isn't mounted at all —
/// another mailbox's mail is never lost from a listing of this one.
fn mount(link: &Value, parent: &str, out: &mut Listing) {
    if out.mounted.len() >= MAX_LISTED_FOLDERS {
        return;
    }
    let id = json::line(link.get("id"));
    let Some((zid, rid)) = id.split_once(':') else { return };
    let (zid, rid) = (zid.trim().to_string(), rid.trim().to_string());
    if zid.is_empty() || !rid.bytes().all(|b| b.is_ascii_digit()) {
        return;
    }
    let name: String = json::line(link.get("name")).chars().take(MAX_NAME).collect();
    if name.is_empty() {
        return;
    }
    if !out.seen.insert(id.clone()) {
        out.complete = false;
    }
    out.mounted.push(Mountpoint {
        id,
        name,
        owner: json::line(link.get("owner")).chars().take(MAX_NAME).collect(),
        zid,
        rid,
        parent: parent.to_string(),
        view: json::line(link.get("view")),
    });
}

fn walk(folder: &Value, parent: &str, depth: usize, out: &mut Listing) {
    if depth > MAX_DEPTH || out.folders.len() >= MAX_LISTED_FOLDERS {
        out.complete = false;
        return;
    }
    let Some(id) = json::own_id(folder.get("id")) else {
        // A folder shared from another mailbox is named by a foreign id (`<account>:<id>`): it
        // isn't one of this mailbox's, and leaving it out loses nothing of ours. An id of no
        // shape at all, or none, means a folder we can't account for: the listing isn't complete.
        if !json::foreign_id(folder.get("id")) {
            out.complete = false;
        }
        return;
    };
    if !out.seen.insert(id.clone()) {
        // An id the server names twice names two folders, or one folder twice: the listing isn't
        // one that answers for what it lists.
        out.complete = false;
    }
    let view = json::raw(folder.get("view")).unwrap_or_default();
    let view = view.trim();
    // A view that is no word can't be told mail from contacts: the listing isn't one that
    // answers for what it lists.
    if !view.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        out.complete = false;
        return;
    }
    if !matches!(view, "" | "message" | "conversation") || id == CHATS {
        return;
    }
    let here = if id == ROOT {
        String::new()
    } else {
        let name: String = json::line(folder.get("name")).chars().take(MAX_NAME).collect();
        if name.is_empty() {
            // A folder of mail that can't be named, and all that lies in it, is left out: the
            // listing is not a complete one, and no token may be kept from a read on it.
            out.complete = false;
            return;
        }
        let (total, unread) = (json::number(folder.get("n")), json::number(folder.get("u")));
        let clamp = |n: Option<u64>| n.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
        let own = id.parse::<u64>().is_ok_and(|n| n < FIRST_USER_ID);
        if own {
            out.well_known.push(id.clone());
        }
        out.folders.push(GraphFolder {
            id: id.clone(),
            parent: parent.to_string(),
            name,
            kind: kind_of(&id),
            total: clamp(total),
            unread: clamp(unread),
            counted: total.is_some(),
            well_known: own,
        });
        id
    };
    // Children that came in no shape a folder comes in mean part of the tree can't be read: the
    // listing is not a complete one — a folder unseen would have its messages taken for deleted.
    if json::bad_list(folder.get("folder")) {
        out.complete = false;
        return;
    }
    for inner in json::list(folder.get("folder")) {
        walk(inner, &here, depth + 1, out);
    }
    // Folders of other mailboxes mounted here are `link` entries: kept for what they are, not
    // among this mailbox's own folders.
    for link in json::list(folder.get("link")) {
        mount(link, &here, out);
    }
}
