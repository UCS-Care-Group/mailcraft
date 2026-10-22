// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Favorites of the folder pane as Outlook has them: the folders the owner put under
//! Favorites there, in that order, read from the server and never written.
//!
//! Graph has no list of them as such. What it has is the cached configuration of Outlook on the
//! web, a user configuration item of the mailbox:
//!
//! 1. `GET /beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration`
//!    (permission `MailboxConfigItem.Read`) answers `{"id": …, "xmlData": "<base64>"}`. The XML
//!    in it is a dictionary of which one entry, `OWA.OutlookFavorites`, is the list
//!    ([`xml`]): for each favourite its place, its kind, and for a folder the folder's id.
//!    **Nothing else of the document is kept, shown or logged.** The user configuration API is
//!    documented, and only in the beta version; what the item holds is not documented at all.
//! 2. Each of those ids is asked about with `GET /v1.0/me/mailFolders/{id}` (permission
//!    `Mail.ReadBasic`), twenty to a `POST /v1.0/$batch`. The list keeps entries for folders
//!    that are long gone, and for folders of other mailboxes: only what answers 200 is a
//!    favourite today. In the same batches the special folders are asked for by their
//!    well-known names (`inbox`, `sentitems`…), so that an Inbox that is called something else
//!    is still known for the Inbox, and `msgfolderroot`, which is where a folder's path ends.
//! 3. The folders above each favourite are asked about in the same way, level by level, for the
//!    names from the top of the mailbox down to it: that is how the mailbox here finds its own
//!    folder for it. No id leaves this module.
//!
//! A request inside a batch that is told to come back later (429, 503, 504, with a
//! `Retry-After` among its headers) is asked once more, after as long as it says but no longer
//! than ten seconds; when it still has no answer the whole read fails ([`NetError::Busy`])
//! rather than pass a list off as whole that isn't.
//!
//! What can go wrong, for the host to act on: [`NetError::Forbidden`] when the sign-in doesn't
//! cover one of the two permissions (the owner can be asked for them on the provider's page,
//! and nothing else is the worse for it); `ProtoError::Auth` when Graph no longer takes the
//! token; anything else when the list couldn't be read this time. A mailbox that has no such
//! item, or no list in it, is not an error: [`FavoritesOutcome::Missing`].
//!
//! The log says how many entries there were and what became of them, never a name or an id.

mod xml;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use mailcraft_model::FolderKind;
use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome, MAX_FAVORITE_DEPTH, MAX_FAVORITES};
use mailcraft_proto::ProtoError;
use serde_json::{Value, json};

use super::{ASK_TIMEOUT, BATCH, BATCH_SIZE, Doing, Graph, MAX_BATCH, code_of, nonsense, text, wait_of};
use crate::NetError;
use crate::http;

/// The cached configuration of Outlook on the web, in the root of the mailbox.
const CONFIGURATION: &str = "/beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration";
/// The largest answer that is read (the item seen was 300 KB), and the largest document in it.
const MAX_CONFIGURATION: usize = 8 * 1024 * 1024;
const MAX_XML: usize = 6 * 1024 * 1024;
/// What is asked of a folder: what the server calls it, its name, and the folder it lies in.
const FOLDER_FIELDS: &str = "id,displayName,parentFolderId";
/// The special folders by the names Graph knows them by wherever they are and whatever they are
/// called, and the folder a path ends at.
const WELL_KNOWN: [(&str, FolderKind); 7] = [
    ("inbox", FolderKind::Inbox),
    ("sentitems", FolderKind::Sent),
    ("drafts", FolderKind::Drafts),
    ("deleteditems", FolderKind::Deleted),
    ("junkemail", FolderKind::Junk),
    ("archive", FolderKind::Archive),
    ("outbox", FolderKind::Outbox),
];
const TOP: &str = "msgfolderroot";
/// The shortest and the longest text that is taken for a folder's id, and the longest name read.
const MIN_ID: usize = 20;
const MAX_ID: usize = 512;
const MAX_NAME: usize = 1_024;
/// Most folders above the favourites that are asked about.
const MAX_ABOVE: usize = 200;
/// How long the whole read may take, the longest a request that was told "later" is waited for
/// before it is asked once more, and how long when it wasn't told how long.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PAUSE: Duration = Duration::from_secs(10);
const PAUSE: Duration = Duration::from_secs(2);

/// The Favorites of the mailbox as Outlook has them, for the owner of `token` (an access token
/// for Graph that covers `MailboxConfigItem.Read` and `Mail.ReadBasic`): the folders, in
/// Outlook's order, each with where it is ([`RemoteFavorite`]), and how many entries were left
/// out. [`FavoritesOutcome::Missing`] when the mailbox has no such list.
///
/// The download doesn't know whose it is: say so with [`FavoritesDownload::for_account`] before
/// the mailbox takes it in. Blocks for about a minute at most.
pub fn favorites(token: &str) -> Result<FavoritesDownload, NetError> {
    favorites_at(&super::graph_base(), token, &|pause| std::thread::sleep(pause))
}

/// What Graph calls the folder that a favourite's `FolderId` names: the same id as it is written
/// in an address (`/` as `-`, `+` as `_`, the `=` at its end kept). `None` for what can't be an
/// id: nothing of it is sent anywhere then.
fn graph_id(folder_id: &str) -> Option<String> {
    let id = folder_id.trim();
    let body = id.trim_end_matches('=');
    let shaped = (MIN_ID..=MAX_ID).contains(&id.len())
        && id.len() - body.len() <= 2
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
    shaped.then(|| id.replace('/', "-").replace('+', "_"))
}

/// The document in an item's `xmlData`: base64 (in either alphabet, padded or not) of XML in
/// UTF-8, or in UTF-16 when it says so with a byte order mark. `None` for anything else, and
/// for a document larger than is read.
fn document(xml_data: &str) -> Option<String> {
    if xml_data.len() > MAX_CONFIGURATION {
        return None;
    }
    let packed: String = xml_data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let packed = packed.trim_end_matches('=');
    let url_safe = packed.contains(['-', '_']);
    let bytes = if url_safe { URL_SAFE_NO_PAD.decode(packed) } else { STANDARD_NO_PAD.decode(packed) }.ok()?;
    if bytes.len() > MAX_XML {
        return None;
    }
    // (Half a unit at the end is no character: the text isn't one then.)
    let units = |rest: &[u8], big: bool| -> Vec<u16> {
        let unit = |pair: &[u8]| match pair {
            [a, b] if big => u16::from_be_bytes([*a, *b]),
            [a, b] => u16::from_le_bytes([*a, *b]),
            _ => 0xD800,
        };
        rest.chunks(2).map(unit).collect()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return String::from_utf16(&units(rest, false)).ok();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return String::from_utf16(&units(rest, true)).ok();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(rest).ok().map(str::to_string);
    }
    String::from_utf8(bytes).ok()
}

/// What the list says, put in order: the ids of the folders it names (each once, as Graph
/// writes them), how many entries are not folders, and how many name no folder that could be
/// asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Wanted {
    ids: Vec<String>,
    skipped: u32,
    unresolved: u32,
}

/// The entries of a list in the order Outlook shows them: by their index, and those with the
/// same index (or none, which come last) in the order the document has them.
fn wanted(listed: xml::Listed) -> Wanted {
    let mut out = Wanted { skipped: u32::try_from(listed.more).unwrap_or(u32::MAX), ..Default::default() };
    let mut folders: Vec<(bool, i64, usize, String)> = Vec::new();
    for (n, entry) in listed.entries.into_iter().enumerate() {
        if !entry.kind.eq_ignore_ascii_case("Folder") {
            out.skipped = out.skipped.saturating_add(1);
            continue;
        }
        match graph_id(&entry.folder_id) {
            Some(id) => folders.push((entry.index.is_none(), entry.index.unwrap_or(0), n, id)),
            None => out.unresolved = out.unresolved.saturating_add(1),
        }
    }
    folders.sort();
    for (_, _, _, id) in folders {
        // (The same folder named twice is one favourite.)
        if !out.ids.contains(&id) {
            out.ids.push(id);
        }
    }
    out
}

/// What Graph said of a folder that was asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Found {
    /// A folder of this mailbox: what the server calls it, its name, and the folder it lies in.
    Folder { id: String, name: String, parent: String },
    /// The mailbox has no such folder.
    Gone,
}

/// A question of a batch, as far as it has got.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    Answered(Found),
    /// To be asked again: after this many seconds, when it said, having answered this status.
    Later(Option<u32>, u16),
}

/// The folder in the body of an answer; `None` when it isn't one.
fn folder_of(body: &Value) -> Option<Found> {
    let id = mailcraft_model::remote::clean_key(text(body, "id"));
    let name: String = text(body, "displayName").chars().take(MAX_NAME).collect();
    let parent = mailcraft_model::remote::clean_key(text(body, "parentFolderId"));
    (!id.is_empty() && id.len() <= MAX_ID && parent.len() <= MAX_ID).then_some(Found::Folder { id, name, parent })
}

/// What a header of an answer inside a batch says, whatever the case of its name.
fn header<'v>(answer: &'v Value, name: &str) -> Option<&'v str> {
    let headers = answer.get("headers")?.as_object()?;
    headers.iter().take(64).find(|(have, _)| have.eq_ignore_ascii_case(name)).and_then(|(_, value)| value.as_str())
}

impl Graph<'_> {
    /// One batch of questions about folders (ids as Graph writes them, or well-known names), a
    /// slot each, in order.
    fn folder_batch(&self, names: &[&String], patience: Duration) -> Result<Vec<Slot>, NetError> {
        let select = http::form(&[("$select", FOLDER_FIELDS)]);
        let requests: Vec<Value> = names
            .iter()
            .enumerate()
            .map(|(n, name)| json!({"id": n.to_string(), "method": "GET", "url": format!("/me/mailFolders/{}?{select}", http::encode(name))}))
            .collect();
        let body = json!({"requests": requests}).to_string();
        let reply = self.ask(&self.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), patience.min(ASK_TIMEOUT), MAX_BATCH)?;
        let now = mailcraft_time::now();
        match reply.status {
            200 => {}
            // The batch as a whole is told to come back later.
            429 | 503 | 504 => return Ok(names.iter().map(|_| Slot::Later(wait_of(reply.retry_after.as_deref(), now), reply.status)).collect()),
            _ => return Err(self.refusal(Doing::Favorites, &reply, &body)),
        }
        let json = serde_json::from_slice::<Value>(&reply.body).ok();
        let Some(said) = json.as_ref().and_then(|json| json.get("responses")).and_then(Value::as_array) else {
            return Err(nonsense("Microsoft Graph's answer isn't the JSON it should be"));
        };
        let mut slots: Vec<Slot> = names.iter().map(|_| Slot::Later(None, 0)).collect();
        for one in said.iter().take(BATCH_SIZE * 2) {
            // Each answer says which question it is to; one that doesn't is no answer.
            let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
            let Some(slot) = n.and_then(|n| slots.get_mut(n)) else { continue };
            let status = one.get("status").and_then(Value::as_u64).and_then(|status| u16::try_from(status).ok()).unwrap_or(0);
            let said = one.get("body").unwrap_or(&Value::Null);
            let code = said.get("error").map_or("", |error| text(error, "code"));
            *slot = match status {
                200 => Slot::Answered(folder_of(said).ok_or_else(|| nonsense("Microsoft Graph answered with a folder that can't be read"))?),
                404 | 410 => Slot::Answered(Found::Gone),
                // An id that is none (what is left of a folder of long ago may be anything).
                400 if code == "ErrorInvalidIdMalformed" => Slot::Answered(Found::Gone),
                401 => return Err(NetError::Proto(ProtoError::Auth("Microsoft Graph didn't take the sign-in".into()))),
                403 => {
                    let code = code_of(said.to_string().as_bytes(), &[&self.token(), &body]);
                    return Err(NetError::Forbidden(format!("this account may not read its folders through Microsoft Graph{code}")));
                }
                400 => return Err(NetError::Proto(ProtoError::Rejected("Microsoft Graph answered with status 400".into()))),
                status => Slot::Later(wait_of(header(one, "Retry-After"), now), status),
            };
        }
        Ok(slots)
    }

    /// Ask about the folders `names`, [`BATCH_SIZE`] to a request: an answer each, in order.
    /// What is told to come back later is asked once more after `pause`d for as long as it
    /// says (never longer than [`MAX_PAUSE`]); what has no answer then fails the lot.
    fn folders(&self, names: &[String], started: Instant, pause: &dyn Fn(Duration)) -> Result<Vec<Found>, NetError> {
        let mut out: Vec<Found> = Vec::with_capacity(names.len());
        for some in names.chunks(BATCH_SIZE) {
            let left = || READ_TIMEOUT.checked_sub(started.elapsed()).filter(|left| !left.is_zero()).ok_or_else(too_slow);
            let asked: Vec<&String> = some.iter().collect();
            let mut slots = self.folder_batch(&asked, left()?)?;
            let later: Vec<usize> = slots.iter().enumerate().filter(|(_, slot)| matches!(slot, Slot::Later(..))).map(|(n, _)| n).collect();
            if !later.is_empty() {
                let said = slots.iter().filter_map(|slot| if let Slot::Later(wait, _) = slot { *wait } else { None }).max();
                let wait = said.map_or(PAUSE, |seconds| Duration::from_secs(u64::from(seconds))).clamp(Duration::from_secs(1), MAX_PAUSE);
                log::debug!("favorites: {} of {} folders are asked about once more", later.len(), some.len());
                pause(wait.min(left()?));
                let again: Vec<&String> = later.iter().filter_map(|n| some.get(*n)).collect();
                let answers = self.folder_batch(&again, left()?)?;
                for (n, answer) in later.iter().zip(answers) {
                    if let Some(slot) = slots.get_mut(*n) {
                        *slot = answer;
                    }
                }
            }
            for slot in slots {
                match slot {
                    Slot::Answered(found) => out.push(found),
                    Slot::Later(wait, 429 | 503 | 504) => {
                        let text = "Microsoft Graph is busy; try again later".to_string();
                        return Err(NetError::Busy { text, retry_after: wait });
                    }
                    Slot::Later(_, 0) => return Err(nonsense("Microsoft Graph left a question about a folder unanswered")),
                    Slot::Later(_, status) => {
                        return Err(NetError::Proto(ProtoError::Rejected(format!("Microsoft Graph answered with status {status}"))));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn too_slow() -> NetError {
    NetError::Proto(ProtoError::Io("Microsoft Graph took too long to answer".into()))
}

/// [`favorites`] from Graph at `base`, waiting by `pause` when a request is told "later".
fn favorites_at(base: &str, token: &str, pause: &dyn Fn(Duration)) -> Result<FavoritesDownload, NetError> {
    let graph = Graph::at(base, token)?;
    let started = Instant::now();
    let missing = || Ok(FavoritesDownload { outcome: FavoritesOutcome::Missing, ..Default::default() });
    let reply = graph.ask(&graph.url(CONFIGURATION), "GET", &[], None, ASK_TIMEOUT, MAX_CONFIGURATION)?;
    match reply.status {
        200 => {}
        // The mailbox has no such item (nobody has opened Outlook on the web for it), or the
        // service has no such question for this kind of mailbox: there is no list to show.
        400 | 404 | 405 | 410 | 501 => {
            log::info!("favorites: the mailbox has no list on its server (status {})", reply.status);
            return missing();
        }
        403 => {
            let code = code_of(&reply.body, &[token]);
            return Err(NetError::Forbidden(format!("this account may not read its Favorites through Microsoft Graph{code}")));
        }
        _ => return Err(graph.refusal(Doing::Favorites, &reply, "")),
    }
    // Of the whole answer only the list is kept: the rest is dropped as soon as it is read.
    let listed = {
        let unreadable = || nonsense("the list of Favorites on the server can't be read");
        let json = serde_json::from_slice::<Value>(&reply.body).ok().filter(Value::is_object);
        drop(reply);
        let Some(json) = json else { return Err(nonsense("Microsoft Graph's answer isn't the JSON it should be")) };
        match json.get("xmlData") {
            // The item is there and holds no document: no list.
            None | Some(Value::Null) => return missing(),
            Some(Value::String(packed)) if packed.trim().is_empty() => return missing(),
            Some(Value::String(packed)) => xml::favorites_of(&document(packed).ok_or_else(unreadable)?).map_err(|_| unreadable())?,
            Some(_) => return Err(unreadable()),
        }
    };
    let Some(listed) = listed else {
        log::info!("favorites: the server's configuration has no list of them");
        return missing();
    };
    let entries = listed.entries.len().saturating_add(listed.more);
    let wanted = wanted(listed);
    let mut download =
        FavoritesDownload { outcome: FavoritesOutcome::Read, skipped: wanted.skipped, unresolved: wanted.unresolved, ..Default::default() };
    // The favourites, then the special folders and the top of the mailbox, in one go.
    let mut names = wanted.ids.clone();
    names.extend(WELL_KNOWN.iter().map(|(name, _)| (*name).to_string()));
    names.push(TOP.to_string());
    let answers = graph.folders(&names, started, pause)?;
    let (of_favorites, of_special) = answers.split_at(wanted.ids.len().min(answers.len()));
    let mut special: HashMap<&str, FolderKind> = HashMap::new();
    let mut top: Option<&str> = None;
    // What is known of a folder, by what the server calls it: its name and the folder it lies in.
    let mut known: HashMap<String, (String, String)> = HashMap::new();
    for (n, answer) in of_special.iter().enumerate() {
        let Found::Folder { id, name, parent } = answer else { continue };
        match WELL_KNOWN.get(n) {
            Some((_, kind)) => {
                special.insert(id.as_str(), *kind);
                known.insert(id.clone(), (name.clone(), parent.clone()));
            }
            None => top = Some(id.as_str()),
        }
    }
    // Those that are folders of this mailbox today, each once by what the server calls it.
    let mut kept: Vec<(&str, &str, &str)> = Vec::new();
    for answer in of_favorites {
        match answer {
            Found::Folder { id, name, parent } => {
                if kept.len() < MAX_FAVORITES && !kept.iter().any(|(have, _, _)| have == id) {
                    kept.push((id.as_str(), name.as_str(), parent.as_str()));
                    known.insert(id.clone(), (name.clone(), parent.clone()));
                }
            }
            Found::Gone => download.unresolved = download.unresolved.saturating_add(1),
        }
    }
    // The folders above them, level by level, as far up as the top of the mailbox.
    let mut gone: HashSet<String> = HashSet::new();
    let mut above = 0usize;
    for _ in 0..MAX_FAVORITE_DEPTH {
        let mut ask: Vec<String> = Vec::new();
        for (_, parent) in known.values() {
            let unknown = !parent.is_empty() && top != Some(parent.as_str()) && !known.contains_key(parent) && !gone.contains(parent);
            if unknown && !ask.contains(parent) && above.saturating_add(ask.len()) < MAX_ABOVE {
                ask.push(parent.clone());
            }
        }
        if ask.is_empty() {
            break;
        }
        above = above.saturating_add(ask.len());
        let answers = graph.folders(&ask, started, pause)?;
        for (asked, answer) in ask.into_iter().zip(answers) {
            match answer {
                // (Under the name it was asked by, which is the one its children know it by.)
                Found::Folder { name, parent, .. } => {
                    known.insert(asked, (name, parent));
                }
                Found::Gone => {
                    gone.insert(asked);
                }
            }
        }
    }
    let kind_of = |id: &str| special.get(id).copied().unwrap_or(FolderKind::Custom);
    for (id, name, parent) in &kept {
        // (A ring of folders, or a folder whose parent can't be read, has no path.)
        match crate::favorite_paths::favorite_of(id, name, parent, &known, top, &kind_of) {
            Some(favorite) => download.folders.push(favorite),
            None => download.unresolved = download.unresolved.saturating_add(1),
        }
    }
    log::info!(
        "favorites: {entries} entries on the server, {} not folders, {} asked about, {} are folders of this mailbox, {} are not ({above} folders above them asked about)",
        download.skipped,
        wanted.ids.len(),
        download.folders.len(),
        download.unresolved
    );
    Ok(download)
}

#[cfg(test)]
mod tests;
