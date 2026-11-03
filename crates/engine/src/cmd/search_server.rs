// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server (R6.2, slice 2): a search of a Microsoft account's mailbox, or a
//! shared mailbox's, on its server, when the owner asks for it.
//!
//! As for a run, the engine never talks to a server: [`requests`] (`sync.graphSearchRequest`)
//! says which page of which mailbox to ask for, a host with sockets asks
//! (`mailcraft_net::graph::search_messages`), and [`apply`] (`sync.applyGraphSearch`) takes in
//! what came back.
//!
//! - **The scope is the one in view.** Current Folder asks for that folder (of a Microsoft
//!   account, and only when its server has it); All Mailboxes asks each Microsoft account's
//!   mailbox and each shared mailbox opened through one, once each. IMAP accounts are not asked.
//! - **What goes** is the part of the query Graph's search understands
//!   ([`mailcraft_model::search::Query::for_server`]); the rest is checked here against what
//!   comes back, and the search says so ([`ServerSearch::checked_here`]).
//! - **Server results are held in the session, never in the mailbox**: 50 to a page, newest
//!   first, with the next page asked for on the owner's say-so. One that is on this computer
//!   already (the same server id in the same mailbox) is not among them: it shows once, in the
//!   local results. A new search, or clearing it, forgets them.
//! - **Opening one** ([`open`], `search.openServerResult`) puts it into its folder as a message
//!   not fetched yet, as a run would have brought it, and selects it: the window then fetches it
//!   whole, and it stays as any synced message.
//! - **Refusals** are said in the words a run has for them, and the local results stand.
//!
//! Nothing of the query or of what was found goes into a log line.

use std::collections::HashSet;

use mailcraft_model::search::Query;
use mailcraft_model::sync::{GraphMessage, GraphSearchRequest, GraphSearchResult};
use mailcraft_model::{AccountKind, FolderKind, Id, Message};
use mailcraft_time::{Date, Weekday};
use serde_json::{Value, json};

use super::{bad, fail, graph_down};
use crate::{CmdError, CmdResult, CommandSpec, Scope, Session, p};

/// Most server results held for one search, whatever the servers send: Graph gives at most 1,000
/// a mailbox, and a few mailboxes are searched at once.
const MAX_RESULTS: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.graphSearchRequest", "More Results on the Server", "Search", request)
            .pure()
            .params("{} — for the search in view: answers which page of which Microsoft mailbox a host should search on its server (the first, or the next)")
            .when(offered_reason),
        CommandSpec::new("sync.applyGraphSearch", "Apply Results from the Server", "Search", apply_command)
            .pure()
            .params(r#"{"result": GraphSearchResult}"#),
        CommandSpec::new("search.openServerResult", "Open Result from the Server", "Search", open_command)
            .pure()
            .params(r#"{"index": number} — puts that result of the server into its folder (to be fetched whole) and selects it"#),
    ]
}

/// One mailbox a search on the server goes to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Searched {
    pub account: Id,
    /// The folder's server id, for Current Folder; empty for the whole mailbox.
    pub folder: String,
    /// Where its next page is, as its server named it; empty before the first.
    pub next: String,
    /// It was asked at least once.
    pub asked: bool,
    /// Its server has no more to give (or won't give any).
    pub done: bool,
    /// A page of it is being asked for now.
    pub waiting: bool,
}

/// A message the server found that is not on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerResult {
    pub account: Id,
    /// The folder of this mailbox it is in.
    pub folder: Id,
    /// What the server said of it: what its row shows.
    pub found: GraphMessage,
    /// Once opened, the message it became here.
    pub opened: Option<Id>,
}

/// The search on the server under way for the search in view. Never saved.
#[derive(Clone, Debug, Default)]
pub struct ServerSearch {
    /// Counts the searches: a request carries it, and an answer for another is not taken in.
    pub ticket: u64,
    /// The search these are for: the query, the scope, and the folder in view.
    key: Option<(String, Scope, Id, Option<(Date, Weekday)>, Option<String>)>,
    pub mailboxes: Vec<Searched>,
    /// Newest first.
    pub results: Vec<ServerResult>,
    /// What went wrong, each said once.
    pub errors: Vec<String>,
    /// Some of the search was checked on this computer: terms Graph's search doesn't understand.
    pub checked_here: bool,
    /// The last try couldn't reach the server.
    pub offline: bool,
}

impl ServerSearch {
    /// Whether the owner has asked for anything yet.
    pub fn asked(&self) -> bool {
        self.mailboxes.iter().any(|m| m.asked || m.waiting)
    }

    /// Whether a page is being asked for now.
    pub fn waiting(&self) -> bool {
        self.mailboxes.iter().any(|m| m.waiting)
    }

    /// Whether there may be more to ask for.
    pub fn more(&self) -> bool {
        self.mailboxes.iter().any(|m| !m.done)
    }

    /// Everything of the search forgotten (a new search, or none).
    pub fn forget(&mut self) {
        *self = ServerSearch { ticket: self.ticket.wrapping_add(1), ..Default::default() };
    }
}

/// What the search in view is, for [`ServerSearch`]'s key: with the day it is made on and the
/// first day of the week when it needs them, as `received:today` is another search tomorrow, and
/// the zone when it has a date, as a day is other messages in another zone (#329); another search
/// keeps what the server found past midnight.
fn key_of(s: &Session) -> (String, Scope, Id, Option<(Date, Weekday)>, Option<String>) {
    // The folder in view, or for a mailbox's search its account: another folder of the same
    // account keeps what the server found.
    let place = match s.view.scope {
        Scope::Everywhere => 0,
        Scope::Mailbox => s.mailbox.folder(s.view.folder).map_or(0, |f| f.account),
        Scope::Folder | Scope::Subfolders => s.view.folder,
    };
    let query = Query::parse(&s.view.search);
    let day = query.needs_day().then(|| (s.today(), s.prefs.week_start));
    let zone = query.has_dates().then(|| s.zone_key());
    (s.view.search.clone(), s.view.scope, place, day, zone)
}

/// The mailboxes the search in view would go to: each with the server id of the folder to search
/// in (empty for all of it).
pub fn mailboxes_for(s: &Session) -> Vec<(Id, String)> {
    let mb = &s.mailbox;
    let usable = |account: Id| {
        mb.account(account).is_some_and(|a| {
            a.kind == AccountKind::Microsoft
                && (!a.is_shared() || (mb.owner_of(account).is_some() && mailcraft_model::mailbox_address(&a.email).is_some()))
        })
    };
    match s.view.scope {
        Scope::Folder => mb
            .folder(s.view.folder)
            .filter(|f| usable(f.account))
            .and_then(|f| f.server.as_ref().map(|server| (f.account, server.id.clone())))
            .into_iter()
            .collect(),
        // The account in view, all of it: what is under the folder is kept when it comes back.
        Scope::Subfolders | Scope::Mailbox => {
            mb.folder(s.view.folder).filter(|f| usable(f.account)).map(|f| (f.account, String::new())).into_iter().collect()
        }
        Scope::Everywhere => mb.accounts.iter().filter(|a| usable(a.id)).map(|a| (a.id, String::new())).collect(),
    }
}

/// Why More results on the server isn't offered for the search in view, when it isn't.
pub fn offered_reason(s: &Session) -> Option<&'static str> {
    let query = Query::parse(&s.view.search);
    if query.is_empty() {
        return Some("there is no search");
    }
    if query.for_server().terms.is_empty() {
        return Some("nothing of this search can be asked of the server");
    }
    if mailboxes_for(s).is_empty() {
        return Some("the search involves no mailbox of a Microsoft account");
    }
    None
}

/// Whether what [`Session::server_search`] holds is for the search in view.
pub fn is_current(s: &Session) -> bool {
    s.server_search.key.as_ref() == Some(&key_of(s))
}

/// The server search, made to be the one for the search in view: forgotten and begun afresh when
/// the search changed.
pub fn current(s: &mut Session) -> &mut ServerSearch {
    let key = key_of(s);
    if s.server_search.key.as_ref() != Some(&key) {
        s.server_search.forget();
        let checked_here = !Query::parse(&key.0).for_server().local.is_empty();
        let mailboxes = mailboxes_for(s).into_iter().map(|(account, folder)| Searched { account, folder, ..Default::default() }).collect();
        s.server_search.key = Some(key);
        s.server_search.mailboxes = mailboxes;
        s.server_search.checked_here = checked_here;
    }
    &mut s.server_search
}

/// The pages a host should ask for now: the next of each mailbox the search goes to that may have
/// more and isn't being asked already. Each is then waited for.
pub fn requests(s: &mut Session) -> Result<Vec<GraphSearchRequest>, CmdError> {
    if let Some(why) = offered_reason(s) {
        return Err(failed!("More results on the server aren't offered: {why}", why = mailcraft_i18n::Arg::words(why)));
    }
    current(s);
    let query = s.view.search.clone();
    let mut out = Vec::new();
    let ticket = s.server_search.ticket;
    let mut errors_cleared = false;
    for at in 0..s.server_search.mailboxes.len() {
        let Some(searched) = s.server_search.mailboxes.get(at).cloned() else { continue };
        if searched.done || searched.waiting {
            continue;
        }
        let Some(a) = s.mailbox.account(searched.account) else { continue };
        let request = GraphSearchRequest {
            account: a.id,
            email: a.email.clone(),
            shared: a.is_shared(),
            ticket,
            folder: searched.folder.clone(),
            query: query.clone(),
            next: searched.next.clone(),
        };
        out.push(request);
        if !errors_cleared {
            // Asked again: what went wrong last time is for this try to say again.
            s.server_search.errors.clear();
            s.server_search.offline = false;
            errors_cleared = true;
        }
        if let Some(m) = s.server_search.mailboxes.get_mut(at) {
            m.waiting = true;
        }
    }
    Ok(out)
}

/// Take in one page that came back. Not an undo step, and nothing of it goes into the mailbox.
pub fn apply(s: &mut Session, result: GraphSearchResult) -> CmdResult {
    current(s);
    if result.ticket != s.server_search.ticket {
        return Ok(json!({"taken": 0, "stale": true}));
    }
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("these results were for another mailbox, not for {name}", name = a.name));
    }
    let Some(at) = s.server_search.mailboxes.iter().position(|m| m.account == account) else {
        return Ok(json!({"taken": 0, "stale": true}));
    };
    let mb = &s.mailbox;
    // Deleted and junk mail stays out of a search of a whole mailbox or of everything, as it does
    // here; a search of a folder's subfolders keeps only what is under it (#493).
    let hidden: HashSet<Id> = match s.view.scope {
        Scope::Mailbox | Scope::Everywhere => mb
            .folders
            .iter()
            .filter(|f| f.account == account && matches!(f.kind, FolderKind::Deleted | FolderKind::Junk))
            .flat_map(|f| mb.subtree(f.id))
            .collect(),
        Scope::Folder => HashSet::new(),
        Scope::Subfolders => {
            let under: HashSet<Id> = mb.subtree(s.view.folder).into_iter().collect();
            mb.folders.iter().map(|f| f.id).filter(|id| !under.contains(id)).collect()
        }
    };
    // What this mailbox has here already: shown once, among the local results.
    let here: HashSet<&str> =
        mb.messages.values().filter(|m| m.account == account).filter_map(|m| m.server.as_ref().map(|s| s.id.as_str())).collect();
    let held: HashSet<&str> = s.server_search.results.iter().filter(|r| r.account == account).map(|r| r.found.server.id.as_str()).collect();
    let local = s.search_query().for_server().local;
    let mut taken: Vec<ServerResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let room = MAX_RESULTS.saturating_sub(s.server_search.results.len());
    for found in result.messages.iter().take(1_000) {
        let id = found.server.id.as_str();
        if id.is_empty() || here.contains(id) || held.contains(id) || !seen.insert(id.to_string()) || taken.len() >= room {
            continue;
        }
        // In a folder this mailbox has (a hidden folder's, or one not listed yet, is passed over).
        let Some(folder) = mb.folder_by_server(account, &found.server.folder) else { continue };
        if hidden.contains(&folder) {
            continue;
        }
        let shown = as_message(account, folder, found);
        if !local.is_empty() && !local.matches_in(&shown, |at| s.offset_at(at)) {
            continue;
        }
        taken.push(ServerResult { account, folder, found: found.clone(), opened: None });
    }
    let n = taken.len();
    let search = &mut s.server_search;
    search.results.extend(taken);
    search.results.sort_by_key(|r| std::cmp::Reverse(r.found.received));
    for e in &result.errors {
        if !search.errors.contains(e) {
            search.errors.push(e.clone());
        }
    }
    search.offline |= result.offline;
    if let Some(m) = search.mailboxes.get_mut(at) {
        m.waiting = false;
        m.asked = true;
        m.next = result.next.clone();
        // No more: the last page came, or the mailbox won't be searched for the owner at all.
        // (Busy, unreachable or another trouble: the owner may ask again.)
        m.done = (result.next.is_empty() && result.errors.is_empty()) || result.refused.is_some();
    }
    log::info!("server search: {n} results taken in from a page; more to ask for: {}", search.more());
    Ok(json!({"taken": n, "results": search.results.len(), "more": search.more(), "errors": result.errors, "offline": result.offline}))
}

/// A server result as a message of the mailbox would be: what its row shows, and what the part
/// of the search checked here looks at.
pub fn as_message(account: Id, folder: Id, found: &GraphMessage) -> Message {
    Message { account, ..graph_down::new_message(folder, &found.server.folder, found) }
}

/// Open the server result at `index`: into its folder, as a message not fetched yet, and
/// selected (the window fetches it whole). The message it became, which a result opened already
/// is.
pub fn open(s: &mut Session, index: usize) -> Result<Id, CmdError> {
    current(s);
    let result = s.server_search.results.get(index).cloned().ok_or_else(|| bad(format!("there is no result {index} from the server")))?;
    let account = result.account;
    let id = match result.opened.filter(|id| s.mailbox.message(*id).is_some()) {
        Some(id) => id,
        None => {
            let server_id = result.found.server.id.clone();
            // Brought here meanwhile (a run, or opened before): that one.
            let there =
                s.mailbox.messages.values().find(|m| m.account == account && m.server.as_ref().is_some_and(|sv| sv.id == server_id)).map(|m| m.id);
            match there {
                Some(id) => id,
                None => {
                    let folder = s
                        .mailbox
                        .folder_by_server(account, &result.found.server.folder)
                        .ok_or_else(|| fail("the folder this message is in isn't on this computer any more"))?;
                    if s.file_is_current() {
                        s.note_saved();
                    }
                    let id = s.mailbox.add_message(as_message(account, folder, &result.found)).map_err(|e| fail(e.to_string()))?;
                    s.mailbox_changed();
                    id
                }
            }
        }
    };
    if let Some(r) = s.server_search.results.get_mut(index) {
        r.opened = Some(id);
    }
    s.sel.messages = vec![id];
    Ok(id)
}

fn request(s: &mut Session, _: &Value) -> CmdResult {
    let requests = requests(s)?;
    Ok(json!({"requests": requests}))
}

fn apply_command(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphSearchResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply(s, result)
}

fn open_command(s: &mut Session, v: &Value) -> CmdResult {
    let index = p::u64(v, "index").and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("`index` (a number) is required"))?;
    let id = open(s, index)?;
    Ok(json!({"id": id}))
}

#[cfg(test)]
#[path = "search_server_tests.rs"]
mod tests;
