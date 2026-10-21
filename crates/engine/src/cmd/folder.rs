// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folders: choosing the one in view, making, renaming, moving, emptying.

use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Module, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("folder.select", "Go to Folder", "Folder", select)
            .pure()
            .key("Mod+Y")
            .params(r#"{"id": number} | {"kind": "inbox"|"drafts"|"sent"|"deleted"|"junk"|"archive"|"outbox", "account"?: number} | {"path": "Inbox/Projects"}"#),
        CommandSpec::new("folder.list", "List Folders", "Folder", list).pure().params(
            r#"{} — every account's folder tree with each folder's messages and unread, and for each account its own "unread" (a shared mailbox's are on its own entry, with "sharedOf": the account it is opened through, and never added to that account's), "favoritesFrom", and "favorites": what the pane lists under Favorites, in its order"#,
        ),
        CommandSpec::new("folder.new", "New Folder", "Folder › New", new)
            .key("Mod+Shift+E")
            .params(r#"{"name": string, "parent"?: number | null (default: the folder in view; null: top level), "account"?: number}"#),
        CommandSpec::new("folder.rename", "Rename Folder", "Folder › Actions", rename).key("F2").params(r#"{"name": string, "id"?: number}"#),
        CommandSpec::new("folder.move", "Move Folder", "Folder › Actions", move_folder).params(r#"{"parent": number | null, "id"?: number}"#),
        CommandSpec::new("folder.delete", "Delete Folder", "Folder › Actions", delete).params(FOLDER),
        CommandSpec::new("folder.favorite", "Add to Favorites", "Folder › Favorites", favorite).params(
            r#"{"value"?: bool (default: toggle), "id"?: number} — refused for a folder of an account whose Favorites come from its server (Outlook's list, which is changed in Outlook; `view.favorites` says which accounts those are). A shared mailbox's folders are favourites kept in MailCraft, whatever the account it is opened through has: nothing is written to any server for them"#,
        ),
        CommandSpec::new("view.favorites", "Favorites from Outlook", "File › Options", favorites_command).pure().params(
            r#"{"own"?: bool} — "own": true shows the Favorites that are marked here even for an account whose server has a list of its own, false the server's list where there is one (default: as it is). Answers what the folder pane lists under Favorites, in its order: {"own": bool, "favorites": [{"id", "name", "account", "path"}], "accounts": [{"account": number, "from": "server" | "mailbox", "listed": number}]}"#,
        ),
        CommandSpec::new("folder.empty", "Empty Folder", "Folder › Clean Up", empty).params(FOLDER).selection(can_empty),
    ]
}

/// Most folders the pane lists under Favorites.
const MAX_FAVORITES: usize = 1_000;

/// Whether the Favorites of `account` are the ones its server lists (the folders Outlook shows
/// under Favorites, taken in with `sync.applyFavorites`) rather than the ones marked here: the
/// mailbox has a list from that account's server, and the owner hasn't asked for the mailbox's
/// own instead ([`crate::Prefs::own_favorites`]).
pub fn favorites_from_server(s: &Session, account: Id) -> bool {
    !s.prefs.own_favorites && s.mailbox.favorites_from_server.iter().any(|list| list.account == account)
}

/// The folders the pane lists under Favorites, in its order: for each account whose Favorites
/// come from its server, that list in the server's order; then the folders marked as favourites
/// here of every other account, as they always were. The marks of an account whose list comes
/// from its server are kept, and count again once it no longer does.
pub fn favorites(s: &Session) -> Vec<Id> {
    let mut out: Vec<Id> = Vec::new();
    let mut served: Vec<Id> = Vec::new();
    if !s.prefs.own_favorites {
        for account in &s.mailbox.accounts {
            if let Some(list) = s.mailbox.server_favorites(account.id) {
                served.push(account.id);
                out.extend(list);
            }
        }
    }
    out.extend(s.mailbox.folders.iter().filter(|f| f.favorite && !served.contains(&f.account)).map(|f| f.id));
    out.truncate(MAX_FAVORITES);
    out
}

fn favorites_json(s: &Session) -> Value {
    let listed = favorites(s);
    let folders: Vec<Value> = listed
        .iter()
        .filter_map(|id| s.mailbox.folder(*id))
        .map(|f| json!({"id": f.id, "name": f.name, "account": f.account, "path": s.mailbox.folder_path(f.id)}))
        .collect();
    let accounts: Vec<Value> = s
        .mailbox
        .accounts
        .iter()
        .map(|a| {
            let server = favorites_from_server(s, a.id);
            let count = listed.iter().filter(|id| s.mailbox.folder(**id).is_some_and(|f| f.account == a.id)).count();
            json!({"account": a.id, "from": if server { "server" } else { "mailbox" }, "listed": count})
        })
        .collect();
    json!({"own": s.prefs.own_favorites, "favorites": folders, "accounts": accounts})
}

fn favorites_command(s: &mut Session, v: &Value) -> CmdResult {
    match v.get("own") {
        None | Some(Value::Null) => {}
        Some(_) => s.prefs.own_favorites = p::bool(v, "own").ok_or_else(|| bad("`own` is true or false"))?,
    }
    Ok(favorites_json(s))
}

const FOLDER: &str = r#"{"id"?: number} (default: the folder in view)"#;

/// Why `folder.favorite` refuses for an account whose Favorites are its server's.
pub const FROM_OUTLOOK: &str =
    "This account's Favorites come from Outlook and are changed there (File › Options can show the ones kept in MailCraft instead)";

fn can_empty(s: &Session) -> Option<&'static str> {
    if s.mailbox.count(s.view.folder) == 0 { Some("the folder is already empty") } else { None }
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").unwrap_or(s.view.folder);
    s.mailbox.folder(id).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id))
}

/// A folder by `Inbox/Projects/Apollo` (case-insensitive), in `account` or any account.
fn by_path(s: &Session, account: Option<Id>, path: &str) -> Option<Id> {
    let want = path.trim().trim_matches('/').to_lowercase();
    s.mailbox
        .folders
        .iter()
        .filter(|f| account.is_none_or(|a| f.account == a))
        .find(|f| s.mailbox.folder_path(f.id).to_lowercase() == want)
        .map(|f| f.id)
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account");
    let id = if let Some(id) = p::id(v, "id") {
        s.mailbox.folder(id).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id))?
    } else if let Some(kind) = p::str(v, "kind") {
        let kind = FolderKind::parse(kind).ok_or_else(|| bad("`kind` is inbox, drafts, sent, deleted, junk, archive or outbox"))?;
        let account = account.or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account)).or_else(|| s.mailbox.default_account().map(|a| a.id));
        account
            .and_then(|a| s.mailbox.folder_of_kind(a, kind))
            .ok_or_else(|| failed!("no {folder} folder", folder = mailcraft_i18n::Arg::folder(kind.default_name())))?
    } else if let Some(path) = p::str(v, "path") {
        by_path(s, account, path).ok_or_else(|| failed!("no folder \"{path}\"", path))?
    } else {
        return Err(bad("`id`, `kind` or `path` is required"));
    };
    if s.view.folder != id || s.view.module != Module::Mail {
        s.view.folder = id;
        s.view.module = Module::Mail;
        s.sel.messages.clear();
    }
    // A search belongs to the folder it was typed in.
    s.view.search.clear();
    Ok(json!({"folder": id, "path": s.mailbox.folder_path(id), "messages": s.mailbox.count(id), "unread": s.mailbox.unread_count(id)}))
}

fn tree(s: &Session, account: Id, parent: Option<Id>, depth: usize) -> Vec<Value> {
    if depth > 40 {
        return Vec::new();
    }
    s.mailbox
        .children(account, parent)
        .into_iter()
        .map(|f| {
            json!({
                "id": f.id, "name": f.name, "kind": f.kind, "favorite": f.favorite,
                "messages": s.mailbox.count(f.id), "unread": s.mailbox.unread_count(f.id),
                "folders": tree(s, account, Some(f.id), depth + 1),
            })
        })
        .collect()
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    // Each account's unread, in one pass over the mail: its own messages alone.
    let mut unread: std::collections::HashMap<Id, usize> = std::collections::HashMap::new();
    for m in s.mailbox.messages.values().filter(|m| !m.read) {
        let n = unread.entry(m.account).or_default();
        *n = n.saturating_add(1);
    }
    let accounts: Vec<Value> = s
        .mailbox
        .accounts
        .iter()
        .map(|a| {
            let unread = unread.get(&a.id).copied().unwrap_or(0);
            let mut entry = json!({
                "id": a.id, "name": a.name, "email": a.email, "kind": a.kind, "unread": unread,
                "favoritesFrom": if favorites_from_server(s, a.id) { "server" } else { "mailbox" },
                "folders": tree(s, a.id, None, 0),
            });
            // A shared mailbox (#70): its unread are its own, and the account it is opened
            // through is named.
            if let (Some(owner), Some(o)) = (a.shared_of, entry.as_object_mut()) {
                o.insert("sharedOf".into(), json!(owner));
            }
            entry
        })
        .collect();
    Ok(json!({"accounts": accounts, "current": s.view.folder, "favorites": favorites(s)}))
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?;
    // `"parent": null` asks for the top level; no `parent` means under the folder in view.
    let parent = match v.get("parent") {
        Some(Value::Null) => None,
        Some(_) => Some(p::req_id(v, "parent")?),
        None => s.mailbox.folder(s.view.folder).map(|f| f.id),
    };
    let account = match parent.and_then(|p| s.mailbox.folder(p)) {
        Some(f) => f.account,
        None => p::id(v, "account")
            .or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account))
            .or_else(|| s.mailbox.default_account().map(|a| a.id))
            .ok_or_else(|| fail("add an account first"))?,
    };
    let id = s.mailbox.add_folder(account, parent, name)?;
    Ok(json!({"id": id, "path": s.mailbox.folder_path(id)}))
}

fn rename(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.rename_folder(id, p::req_str(v, "name")?)?;
    Ok(json!({"id": id, "path": s.mailbox.folder_path(id)}))
}

fn move_folder(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let parent = match v.get("parent") {
        Some(Value::Null) => None,
        Some(_) => Some(p::req_id(v, "parent")?),
        None => return Err(bad("`parent` (a folder id, or null for the top level) is required")),
    };
    s.mailbox.move_folder(id, parent)?;
    Ok(json!({"id": id, "path": s.mailbox.folder_path(id)}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let parent = s.mailbox.folder(id).and_then(|f| f.parent);
    let account = s.mailbox.folder(id).map(|f| f.account);
    let permanently = s.mailbox.delete_folder(id)?;
    // The view goes to where the folder was.
    if s.view.folder == id || s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = parent
            .filter(|p| s.mailbox.folder(*p).is_some())
            .or_else(|| account.and_then(|a| s.mailbox.folder_of_kind(a, FolderKind::Inbox)))
            .unwrap_or(0);
        s.sel.messages.clear();
    }
    Ok(json!({"permanently": permanently}))
}

fn favorite(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    // Outlook's list is only ever read: a mark made here would show nowhere, and nothing is
    // written to the server for it.
    if s.mailbox.folder(id).is_some_and(|f| favorites_from_server(s, f.account)) {
        return Err(fail(FROM_OUTLOOK));
    }
    let f = s.mailbox.folder_mut(id)?;
    f.favorite = p::bool(v, "value").unwrap_or(!f.favorite);
    Ok(json!({"id": id, "favorite": f.favorite}))
}

fn empty(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let kind = s.mailbox.folder(id).map(|f| f.kind);
    let ids: Vec<Id> = s.mailbox.messages_in(id).map(|m| m.id).collect();
    for m in &ids {
        // Emptying Deleted Items or Junk removes for good; any other folder goes to Deleted Items.
        if matches!(kind, Some(FolderKind::Deleted | FolderKind::Junk)) {
            s.mailbox.remove_message(*m)?;
        } else {
            s.mailbox.delete_message(*m)?;
        }
    }
    // Folders deleted into Deleted Items go with it.
    if kind == Some(FolderKind::Deleted) {
        let children: Vec<Id> = s.mailbox.folders.iter().filter(|f| f.parent == Some(id)).map(|f| f.id).collect();
        for c in children {
            s.mailbox.delete_folder(c)?;
        }
    }
    s.sel.messages.clear();
    Ok(json!({"removed": ids.len()}))
}
