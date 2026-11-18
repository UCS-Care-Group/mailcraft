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

/// Every folder of mail the server has, parents before what lies in them, and which of them are
/// the server's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The folders, as the model lists them.
    pub folders: Vec<GraphFolder>,
    /// The ids of the server's own folders (the Inbox, Sent, Drafts, Trash, Junk and the like).
    pub well_known: Vec<String>,
    /// The tree was read to its end: a folder that isn't in it is one the server doesn't have.
    pub complete: bool,
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
    let tops = json::list(answer.get("folder"));
    if tops.is_empty() {
        return Err(Error::Unreadable);
    }
    for top in tops {
        walk(top, "", 0, &mut out);
    }
    Ok(out)
}

fn walk(folder: &Value, parent: &str, depth: usize, out: &mut Listing) {
    if depth > MAX_DEPTH || out.folders.len() >= MAX_LISTED_FOLDERS {
        out.complete = false;
        return;
    }
    let Some(id) = json::own_id(folder.get("id")) else { return };
    let view = json::line(folder.get("view"));
    if !matches!(view.as_str(), "" | "message" | "conversation") || id == CHATS {
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
    for inner in json::list(folder.get("folder")) {
        walk(inner, &here, depth + 1, out);
    }
}
