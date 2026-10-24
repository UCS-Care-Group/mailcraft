// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folders: choosing the one in view, making, renaming, moving, emptying, and what they hold.

use std::collections::{HashMap, HashSet};

use mailcraft_model::{Folder, FolderKind, Id, Message};
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
        CommandSpec::new("folder.delete", "Delete Folder", "Folder › Actions", delete).params(FOLDER).selection(can_delete),
        CommandSpec::new("folder.favorite", "Add to Favorites", "Folder › Favorites", favorite).params(
            r#"{"value"?: bool (default: toggle), "id"?: number} — refused for a folder of an account whose Favorites come from its server (Outlook's list, which is changed in Outlook; `view.favorites` says which accounts those are). A shared mailbox's folders are favourites kept in MailCraft, whatever the account it is opened through has: nothing is written to any server for them"#,
        ),
        CommandSpec::new("view.favorites", "Favorites from Outlook", "File › Options", favorites_command).pure().params(
            r#"{"own"?: bool} — "own": true shows the Favorites that are marked here even for an account whose server has a list of its own, false the server's list where there is one (default: as it is). Answers what the folder pane lists under Favorites, in its order: {"own": bool, "favorites": [{"id", "name", "account", "path"}], "accounts": [{"account": number, "from": "server" | "mailbox", "listed": number}]}"#,
        ),
        CommandSpec::new("folder.empty", "Empty Folder", "Folder › Clean Up", empty).params(FOLDER).selection(can_empty),
        CommandSpec::new("folder.properties", "Folder Properties", "Folder › Properties", properties).pure().params(
            r#"{"id"?: number (a folder; default: the folder in view)} | {"account": number (all of the account's folders)} — how many messages a folder and each folder under it hold, how many unread, and how much space they take, from what is on this computer; nothing is asked of a server. For an IMAP account that is what was downloaded, which can be only the newest of each folder. Answers {"account", "folder": number | null (null: the account), "name", "rows": [{"id", "name", "path" (the names of the folders from the top, joined by "/"; a name may hold a "/" itself, so "parent" is what to rely on), "parent": number | null, "depth" (0: the folder named, or the account's top folders), "kind", "messages", "unread", "size" (bytes), "unknownSize" (messages whose size isn't known here: a Microsoft account's message of which only the headers are here and whose size its server didn't say, or one too large to download from an IMAP server), "total": {"messages", "unread", "size", "unknownSize"} (with the folders under it), "server"?: {"messages", "unread"} (what a Microsoft account's server counted in it at the last Send/Receive that read it to the end)}] in the folder pane's order, at most 10000 and as deep as the folder pane lists them (32 folders under the top), "total": {"messages", "unread", "size", "unknownSize"} (everything it is about), "more": number (folders left out past 10000)}"#,
        ),
    ]
}

/// Most folders the pane lists under Favorites.
const MAX_FAVORITES: usize = 1_000;

/// The rank of the folders that lead an account's list, in the order they lead it.
fn lead_rank(kind: FolderKind) -> usize {
    const LEAD: [FolderKind; 4] = [FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted];
    LEAD.iter().position(|k| *k == kind).unwrap_or(LEAD.len())
}

/// Put folders that share a parent (`top`: none, they are an account's top level) in the order
/// the folder pane lists them: at the top level Inbox, Drafts, Sent Items and Deleted Items lead;
/// every other folder, Archive, Junk Email and Outbox among them, follows by name without regard
/// to case. Subfolders are by name alone.
pub fn sort_as_listed(list: &mut [&Folder], top: bool) {
    // The id last: two folders of one name keep their places from frame to frame.
    list.sort_by_cached_key(|f| (if top { lead_rank(f.kind) } else { 0 }, f.name.to_lowercase(), f.id));
}

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
    if s.mailbox.count(s.view.folder) == 0 {
        return Some("the folder is already empty");
    }
    // Not of what a JMAP account's server has (M6): the next run would bring it back.
    s.mailbox.emptying_stays_on_server(s.view.folder).then_some(mailcraft_model::NOT_MOVING_YET)
}

/// Not a folder holding what a JMAP account's server has (M6): the next run would bring it back.
fn can_delete(s: &Session) -> Option<&'static str> {
    s.mailbox.folder_stays_on_server(s.view.folder).then_some(mailcraft_model::NOT_MOVING_YET)
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

/// Most rows `folder.properties` answers, and how deep it follows subfolders (the folder pane's
/// depth): a mailbox may come from anywhere, and may hold 50,000 folders.
const MOST_ROWS: usize = 10_000;
const MOST_DEPTH: usize = 32;

/// What some messages come to: how many, how many unread, and their size where it is known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Tally {
    messages: u64,
    unread: u64,
    size: u64,
    /// Messages whose size isn't known here ([`known_size`]).
    unknown: u64,
}

impl Tally {
    fn count(&mut self, m: &Message) {
        self.messages = self.messages.saturating_add(1);
        self.unread = self.unread.saturating_add(u64::from(!m.read));
        match known_size(m) {
            Some(size) => self.size = self.size.saturating_add(size),
            None => self.unknown = self.unknown.saturating_add(1),
        }
    }

    fn add(&mut self, other: &Tally) {
        self.messages = self.messages.saturating_add(other.messages);
        self.unread = self.unread.saturating_add(other.unread);
        self.size = self.size.saturating_add(other.size);
        self.unknown = self.unknown.saturating_add(other.unknown);
    }

    fn json(&self) -> Value {
        json!({"messages": self.messages, "unread": self.unread, "size": self.size, "unknownSize": self.unknown})
    }
}

/// A message's size as Folder Properties counts it: [`Message::list_size`], except for one of
/// which only the headers and a preview are here (older than its account's mail to keep
/// offline, or not fetched yet) and whose size its server didn't say. Graph's delta gives none,
/// and what is here of it is no measure of the message: its size isn't known.
///
/// A message marked to be fetched again that is still whole here (#185) keeps its size: it has
/// its HTML, its attachments or content kept on disk, which a preview never has. (One of plain
/// text alone can't be told from a preview, and counts as unknown until its new copy comes.)
pub fn known_size(m: &Message) -> Option<u64> {
    let preview_only = m.body.html.is_none() && m.attachments.is_empty() && m.kept.is_none();
    if (m.unfetched || m.incomplete) && preview_only && m.server.as_ref().is_none_or(|s| s.size == 0) {
        return None;
    }
    m.list_size()
}

fn properties(s: &mut Session, v: &Value) -> CmdResult {
    let named = |k: &str| v.get(k).is_some_and(|x| !x.is_null());
    if named("id") && named("account") {
        return Err(bad("name a folder by `id` or an account by `account`, not both"));
    }
    let mb = &s.mailbox;
    // What it is about: a folder, or an account's top level.
    let (account, folder, name) = if named("account") {
        let id = p::req_id(v, "account")?;
        let a = mb.account(id).ok_or_else(|| failed!("no account with id {id}", id))?;
        (a.id, None, a.name.clone())
    } else {
        // (An `id` that names no folder is refused, never taken for the folder in view.)
        let id = if named("id") { p::req_id(v, "id")? } else { s.view.folder };
        let f = mb.folder(id).ok_or_else(|| failed!("no folder with id {id}", id))?;
        (f.account, Some(f.id), f.name.clone())
    };
    // The account's folders by parent, each list in the pane's order.
    let mut under: HashMap<Option<Id>, Vec<&Folder>> = HashMap::new();
    for f in mb.folders.iter().filter(|f| f.account == account) {
        under.entry(f.parent).or_default().push(f);
    }
    for (parent, list) in under.iter_mut() {
        sort_as_listed(list, parent.is_none());
    }
    // Depth first, as the pane lists them: each folder with its depth and the row of its parent.
    let mut start: Vec<&Folder> = match folder {
        Some(id) => mb.folder(id).into_iter().collect(),
        None => under.get(&None).cloned().unwrap_or_default(),
    };
    start.reverse();
    // How deep the folder named lies: the walk stops where the pane does, 32 folders from the top.
    let mut above = 0usize;
    let mut cur = folder.and_then(|id| mb.folder(id)).and_then(|f| f.parent);
    while let Some(p) = cur.and_then(|p| mb.folder(p)) {
        above = above.saturating_add(1);
        if above >= MOST_DEPTH {
            break;
        }
        cur = p.parent;
    }
    let mut stack: Vec<(&Folder, usize, Option<usize>)> = start.into_iter().map(|f| (f, 0, None)).collect();
    let mut seen: HashSet<Id> = HashSet::new();
    let mut walked: Vec<(&Folder, usize, Option<usize>)> = Vec::new();
    while let Some((f, depth, parent)) = stack.pop() {
        // (A folder is walked once, however its parents are tangled in the file.)
        if !seen.insert(f.id) {
            continue;
        }
        let row = walked.len();
        walked.push((f, depth, parent));
        if above.saturating_add(depth) < MOST_DEPTH {
            stack.extend(under.get(&Some(f.id)).into_iter().flatten().rev().map(|c| (*c, depth + 1, Some(row))));
        }
    }
    // What each holds, in one pass over the mail.
    let mut own: HashMap<Id, Tally> = walked.iter().map(|(f, _, _)| (f.id, Tally::default())).collect();
    for m in mb.messages.values() {
        if let Some(t) = own.get_mut(&m.folder) {
            t.count(m);
        }
    }
    // With the folders under each: a parent's row is always before its folders'.
    let mut totals: Vec<Tally> = walked.iter().map(|(f, _, _)| own.get(&f.id).copied().unwrap_or_default()).collect();
    let mut total = Tally::default();
    for row in (0..walked.len()).rev() {
        let Some(t) = totals.get(row).copied() else { continue };
        match walked.get(row).and_then(|w| w.2) {
            Some(parent) => {
                if let Some(p) = totals.get_mut(parent) {
                    p.add(&t);
                }
            }
            None => total.add(&t),
        }
    }
    // Each path from its parent's, which comes first: no folder is looked for again.
    let mut paths: Vec<String> = Vec::with_capacity(walked.len().min(MOST_ROWS));
    for (f, _, parent) in walked.iter().take(MOST_ROWS) {
        let path = match parent.and_then(|p| paths.get(p)) {
            Some(above) => format!("{above}/{}", f.name),
            // (Only the folder named can have a parent above the rows.)
            None if f.parent.is_none() => f.name.clone(),
            None => mb.folder_path(f.id),
        };
        paths.push(path);
    }
    let rows: Vec<Value> = walked
        .iter()
        .zip(&totals)
        .zip(paths)
        .map(|(((f, depth, _), all), path)| {
            let here = own.get(&f.id).copied().unwrap_or_default();
            let mut row = json!({
                "id": f.id, "name": f.name, "path": path, "parent": f.parent, "depth": depth, "kind": f.kind,
                "messages": here.messages, "unread": here.unread, "size": here.size, "unknownSize": here.unknown,
                "total": all.json(),
            });
            if let (Some(counts), Some(o)) = (f.server.as_ref().and_then(|server| server.counts), row.as_object_mut()) {
                o.insert("server".into(), json!({"messages": counts.total, "unread": counts.unread}));
            }
            row
        })
        .collect();
    Ok(json!({
        "account": account, "folder": folder, "name": name, "rows": rows, "total": total.json(),
        "more": walked.len().saturating_sub(MOST_ROWS),
    }))
}
