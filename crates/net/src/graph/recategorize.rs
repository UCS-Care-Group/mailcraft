// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A renamed category given to what only the server has (#851): the account's server is asked
//! for the items that carry the old name, and each is given the new one in its place.
//!
//! - **Asked of the server.** `GET {list}?$filter=categories/any(c:c eq '…')&$select=id,categories`
//!   (the syntax of "Use the $filter query parameter"; a `'` in the name is written `''`), a
//!   hundred to a page, followed to its end before anything is changed, so that the pages stand
//!   still while they are read. For mail the list is `/me/messages`, which is the whole mailbox,
//!   Deleted Items with it ("List messages"); for events the main calendar's
//!   `/me/calendar/events` and each of the account's other calendars the request names, which
//!   give single events and the series themselves ("List events"). Never a shared mailbox, and
//!   never a calendar the request doesn't name.
//! - **Changed.** Twenty at a time: first `GET {item}?$select=categories` of each, in one
//!   `POST /v1.0/$batch`, then `PATCH` of each one's `categories` alone, in another: the list the
//!   server has just given, with the new name where the old one stood (once, when the item had
//!   both). The list is read again right before it is written because the change writes the
//!   whole of it, and a run can take minutes: a category the item was given on the server since
//!   the lists were read stays (a change of a message's categories reads them just before it
//!   writes them too, `mail/up.rs`). No `If-Match` is sent: "Update message" and "Update event"
//!   name no such header. An item that no longer carries the old name is not written. One
//!   request at a time from a server that takes no batch, none of them after the owner stopped the run. At most [`MAX_RENAMED_ITEMS`] a run.
//! - **What comes back** ([`CategoryRenamed`]): `done` once the lists were read to their ends and
//!   every item was answered for good (or a run's fill was listed and none of it could be
//!   changed: `more`). An item the server puts off (408, 409, 412, 5xx: counted as
//!   `stuck`, as is one whose read or change got no answer, or whose read gave no list), a busy server, a lost connection: not done,
//!   and a later run asks again and finds what is left. An item the server won't change (another 4xx) is counted and not held against
//!   the rest; one that is gone (404) is nothing to change. A list the server won't say it for
//!   (403, another 4xx, an answer that is no list) is passed over and counted
//!   (`lists_refused`), and the others go on; a calendar that is gone (404) has nothing to rename,
//!   while the first list, the mailbox's own, is never gone: its 404 is a list not said.
//!   When no list says it, the sign-in doesn't cover it (`denied`, after a 403) or the server
//!   won't say (`refused`): neither is asked again, and the old name stays on the server's list. The same trouble on a later page of a list
//!   leaves the rest of that list for a later run, and the server failing at the question (5xx)
//!   is said too (`failed`): a rename that gets nowhere for such reasons is given up after a few
//!   runs ([`mailcraft_model::remote::MAX_RENAME_TRIES`]).
//!
//! What comes back is hostile JSON: ids are checked, and an item whose list of categories is
//! longer than is taken in, or holds what isn't a plain name, is left as it is (`passed`): the
//! change writes the whole list, so it is never written back cut. Neither name,
//! nor any id, goes into a log line or an error.

use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::remote::{CategoryRename, CategoryRenamed, MAX_RENAMED_ITEMS};
use mailcraft_model::sync::same_category;
use serde_json::{Value, json};

use super::favorites::header;
use super::pace::Kind;
use super::tasks::valid_id;
use super::{
    ASK_TIMEOUT, BATCH, BATCH_SIZE, Graph, IMMUTABLE_IDS, MAX_BATCH, MAX_PAGE, MAX_WRITTEN, NetError, WRITE_TIMEOUT, page_of, text, wait_of,
};
use crate::http;

/// Items asked for to a page, and most pages read of one list.
const PAGE: &str = "100";
const MAX_LIST_PAGES: usize = 40;
/// Most categories of an item whose list is written back, and the longest name in it.
const MAX_CATEGORIES: usize = 100;
/// How many names of an item's list are looked at for the old name.
const MAX_LISTED: usize = 1_000;
const MAX_CATEGORY: usize = 255;

/// Where the items are: the lists to ask (paths of Graph v1.0) and what one item is called in a
/// request, without the version (`/me/messages/`, the id follows).
pub(super) struct Items {
    pub lists: Vec<String>,
    pub item: String,
}

/// What a run of [`rename`] came to, with what the caller's own answer says of the server.
#[derive(Default)]
pub(super) struct Outcome {
    pub renamed: CategoryRenamed,
    /// How long the server asked to be left alone, when it said it was busy.
    pub busy_for: Option<u32>,
    /// The server stopped taking the sign-in.
    pub token_refused: bool,
    /// The owner stopped the run before the rename was through: the rest waits for a later run.
    pub stopped: bool,
}

/// One item that carried the old name when the lists were read: its id. (Its categories are read
/// again right before they are written.)
struct Item {
    id: String,
}

/// What became of one item's change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Said {
    Changed,
    Gone,
    /// For good: counted, and not asked again.
    Skipped,
    /// It no longer carries the old name (renamed or taken off on the server since the lists
    /// were read): nothing to change, and a later run doesn't list it.
    Clear,
    /// Its list can't be written back whole now: left as it is, and counted (`passed`).
    Passed,
    /// The server's own "not now" for this item (changed there since it was listed, its own
    /// trouble), or no answer for it at all: a later run, and counted.
    Stuck,
    /// Not asked (the server was busy, or stopped taking the sign-in, before its turn): a later
    /// run, and not counted.
    Later,
    Busy(Option<u32>),
    SignIn,
}

fn said_of(status: u16, retry_after: Option<u32>) -> Said {
    match status {
        200..=299 => Said::Changed,
        404 | 410 => Said::Gone,
        401 => Said::SignIn,
        429 | 503 | 504 => Said::Busy(retry_after),
        // Changed there since it was listed (412), a timeout, a server's own trouble.
        408 | 409 | 412 | 500..=599 => Said::Stuck,
        _ => Said::Skipped,
    }
}

/// An item's list of categories as it is written back: `None` for one that is longer than is
/// taken in, or holds what isn't a plain name. The change writes the whole list, so such an item
/// is left as it is and said, never given a cut copy.
fn whole_list(listed: &[Value]) -> Option<Vec<String>> {
    let plain = |c: &Value| c.as_str().is_some_and(|c| c.chars().count() <= MAX_CATEGORY && !c.chars().any(char::is_control));
    (listed.len() <= MAX_CATEGORIES && listed.iter().all(plain)).then(|| listed.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

/// Whether a list of categories as the server gave it carries `name`.
fn carries(listed: &[Value], name: &str) -> bool {
    listed.iter().take(MAX_LISTED).filter_map(Value::as_str).any(|c| same_category(c, name))
}

/// What an item's own answer, read right before its change, says to do: the list to write, or
/// what became of the item without a write.
fn fresh_of(status: u16, body: &Value, retry_after: Option<u32>, rename: &CategoryRename) -> Result<Vec<String>, Said> {
    match said_of(status, retry_after) {
        Said::Changed => {}
        said => return Err(said),
    }
    // (An answer that gives no list says nothing of the item: counted, and a later run.)
    let Some(listed) = body.get("categories").and_then(Value::as_array) else { return Err(Said::Stuck) };
    // (A list too long to be written back is too long to say it doesn't carry the name.)
    let Some(categories) = whole_list(listed) else {
        return Err(if listed.len() > MAX_CATEGORIES || carries(listed, &rename.from) { Said::Passed } else { Said::Clear });
    };
    if !carries(listed, &rename.from) {
        return Err(Said::Clear);
    }
    Ok(renamed_list(&categories, &rename.from, &rename.to))
}

/// The query of an item's read.
fn read_query() -> String {
    http::form(&[("$select", "categories")])
}

/// The filter that finds what carries `name`.
fn filter(name: &str) -> String {
    format!("categories/any(c:c eq '{}')", name.replace('\'', "''"))
}

/// `categories` with `to` where `from` stood: once, in the place of the first of them.
pub(super) fn renamed_list(categories: &[String], from: &str, to: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(categories.len());
    for name in categories {
        let name = if same_category(name, from) || same_category(name, to) { to } else { name.as_str() };
        if !out.iter().any(|have| same_category(have, name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// Give every item of `items` that carries `rename.from` the name `rename.to` in its place.
/// `progress` is told how many of the items found were answered for, of how many: once they are
/// found, and after each batch.
pub(super) fn rename(
    graph: &Graph<'_>,
    items: &Items,
    rename: &CategoryRename,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u32, u32),
) -> Outcome {
    let mut rename = rename.clone();
    rename.tidy();
    let mut out =
        Outcome { renamed: CategoryRenamed { from: rename.from.clone(), to: rename.to.clone(), ..Default::default() }, ..Default::default() };
    if rename.from.is_empty() || rename.to.is_empty() || same_category(&rename.from, &rename.to) {
        out.renamed.refused = true;
        return out;
    }
    let stopped = || cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed));
    // Which items carry the name: every list to its end, before anything is changed.
    let mut found: Vec<Item> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Items left as they are because their list can't be written back whole, each counted once.
    let mut passed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut whole = true;
    // The server's own trouble with the question (a failure, or a refusal on a later page of a
    // list): not done, and counted against the tries a rename is given.
    let mut trouble = false;
    // A list the server won't say it for doesn't end the others: its items keep the old name.
    let (mut answered, mut forbidden, mut unsaid) = (0u32, 0u32, 0u32);
    let query = http::form(&[("$filter", filter(&rename.from).as_str()), ("$select", "id,categories"), ("$top", PAGE)]);
    'lists: for (index, list) in items.lists.iter().enumerate() {
        let mut next = Some(graph.url(&format!("{list}?{query}")));
        let mut first = true;
        for _ in 0..MAX_LIST_PAGES {
            let Some(url) = next.take() else { break };
            if stopped() {
                out.stopped = true;
                return out;
            }
            let reply = match graph.call(Kind::Read, &url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE) {
                Ok(reply) => reply,
                Err(e) if e.is_refused_sign_in() => {
                    out.token_refused = true;
                    return out;
                }
                Err(NetError::Busy { retry_after, .. }) => {
                    out.busy_for = Some(retry_after.unwrap_or(0));
                    return out;
                }
                Err(_) => {
                    log::info!("categories: the items of a renamed category couldn't be asked for now; a later run goes on");
                    return out;
                }
            };
            match reply.status {
                200 => {}
                401 => {
                    out.token_refused = true;
                    return out;
                }
                // (On a later page: what was read of the list is changed, the rest waits.)
                403 | 404 | 410 if !first => {
                    trouble = true;
                    continue 'lists;
                }
                403 => {
                    forbidden += 1;
                    continue 'lists;
                }
                // The first list is the mailbox's own (its mail, its main calendar), which is never
                // "gone": its "not found" is no answer, so it counts as a list the server won't
                // say it for, and the old name stays.
                404 | 410 if index == 0 => {
                    unsaid += 1;
                    continue 'lists;
                }
                // A calendar that is gone has nothing to rename.
                404 | 410 => continue 'lists,
                429 | 503 | 504 => {
                    out.busy_for = Some(wait_of(reply.retry_after.as_deref(), mailcraft_time::now()).unwrap_or(0));
                    return out;
                }
                408 | 500..=599 => {
                    out.renamed.failed = true;
                    log::info!(
                        "categories: the items of a renamed category couldn't be asked for now (status {}); a later run goes on",
                        reply.status
                    );
                    return out;
                }
                _ if !first => {
                    trouble = true;
                    continue 'lists;
                }
                status => {
                    log::info!(
                        "categories: the server won't say which items of a list carry a renamed category (status {status}); they keep the name"
                    );
                    unsaid += 1;
                    continue 'lists;
                }
            }
            let Ok(page) = page_of(&reply.body) else {
                if first {
                    unsaid += 1;
                } else {
                    trouble = true;
                }
                continue 'lists;
            };
            if std::mem::take(&mut first) {
                answered += 1;
            }
            for entry in page.entries.iter().take(1_000) {
                let id = text(entry, "id");
                let listed = entry.get("categories").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
                // (Only what does carry the name, whatever the server's filter took it for.)
                if !valid_id(id) || !carries(listed, &rename.from) || seen.contains(id) {
                    continue;
                }
                // (One whose list can't be written back whole takes no place in the run.)
                if whole_list(listed).is_none() {
                    if passed.insert(id.to_string()) {
                        out.renamed.passed = out.renamed.passed.saturating_add(1);
                    }
                    continue;
                }
                if found.len() >= MAX_RENAMED_ITEMS {
                    whole = false;
                    break 'lists;
                }
                seen.insert(id.to_string());
                found.push(Item { id: id.to_string() });
            }
            next = match &page.next {
                Some(link) => match graph.next_page(link, &url) {
                    Ok(url) => Some(url),
                    // A next page that can't be followed: what was read of the list is all of it
                    // that is changed, and the rest waits for a later run.
                    Err(_) => {
                        trouble = true;
                        None
                    }
                },
                None => None,
            };
        }
        // More pages than are read of one list: the rest with a later run.
        if next.is_some() {
            whole = false;
        }
    }
    // No list said it: the sign-in doesn't cover it, or the server won't say. Neither is asked again.
    if answered == 0 {
        (out.renamed.denied, out.renamed.refused) = (forbidden > 0, forbidden == 0);
        return out;
    }
    // (A list that is gone has nothing to rename; one that is refused keeps the old name, said.)
    out.renamed.lists_refused = forbidden.saturating_add(unsaid);
    // Each is given the new name in the old one's place.
    let mut later = false;
    let mut single = false;
    let mut gone = 0u32;
    let total = u32::try_from(found.len()).unwrap_or(u32::MAX);
    let mut answered = 0u32;
    if total > 0 {
        progress(0, total);
    }
    'chunks: for chunk in found.chunks(BATCH_SIZE) {
        if stopped() {
            (later, out.stopped) = (true, true);
            break;
        }
        let answers = if single { Err(None) } else { batch(graph, items, chunk, &rename) };
        let answers = match answers {
            Ok(answers) => answers,
            // A server that takes no batch: one at a time from here on.
            Err(None) => {
                single = true;
                let (answers, cut) = one_by_one(graph, items, chunk, &rename, &stopped);
                out.stopped |= cut;
                answers
            }
            Err(Some(said)) => vec![said; chunk.len()],
        };
        for (item, said) in chunk.iter().zip(answers) {
            match said {
                Said::Changed => out.renamed.changed = out.renamed.changed.saturating_add(1),
                // (One that no longer carries the name is, like one that is gone, not listed again.)
                Said::Gone | Said::Clear => gone = gone.saturating_add(1),
                Said::Passed => {
                    if passed.insert(item.id.clone()) {
                        out.renamed.passed = out.renamed.passed.saturating_add(1);
                    }
                }
                Said::Skipped => out.renamed.skipped = out.renamed.skipped.saturating_add(1),
                Said::Stuck => {
                    out.renamed.stuck = out.renamed.stuck.saturating_add(1);
                    later = true;
                }
                Said::Later => later = true,
                Said::Busy(wait) => {
                    out.busy_for = Some(out.busy_for.unwrap_or(0).max(wait.unwrap_or(0)));
                    later = true;
                }
                Said::SignIn => {
                    out.token_refused = true;
                    later = true;
                }
            }
        }
        if out.busy_for.is_some() || out.token_refused || out.stopped {
            later = true;
            break 'chunks;
        }
        answered = answered.saturating_add(u32::try_from(chunk.len()).unwrap_or(u32::MAX)).min(total);
        progress(answered, total);
    }
    // Done when everything was listed and answered for good. A run that listed its fill and could
    // change none of it (nor found any of it gone) would list the same again: that is all there
    // is to do, too, and it is said that there may be more (`more`).
    // (A fill of which none was changed and some were gone, or no longer carry the name, is
    // listed anew by a later run; a server that goes on listing such is failing at the question,
    // and counted.)
    out.renamed.failed = trouble || (!later && !whole && out.renamed.changed == 0 && gone > 0);
    let dead_end = !whole && out.renamed.changed == 0 && gone == 0;
    out.renamed.done = !later && !trouble && (whole || dead_end);
    out.renamed.more = out.renamed.done && dead_end;
    log::info!(
        "categories: a renamed category given to {} items on the server, {} it wouldn't change, {} left as they are; done: {}",
        out.renamed.changed,
        out.renamed.skipped,
        out.renamed.passed,
        out.renamed.done
    );
    out
}

/// The headers of one request of a batch.
fn sub_headers(graph: &Graph<'_>, write: bool) -> Value {
    let mut headers = if write { json!({"Content-Type": "application/json"}) } else { json!({}) };
    if graph.immutable {
        headers["Prefer"] = json!(IMMUTABLE_IDS);
    }
    headers
}

/// What one request of a batch was answered with: the status, the body, and how long the server
/// asked to be left alone (its own `Retry-After`: "Microsoft Graph throttling guidance" has each
/// throttled request of a batch answered 429 with one).
type Answer = (u16, Value, Option<u32>);

/// One `POST /v1.0/$batch` of `requests`, each with its place in the chunk as its id: the answer
/// of each one that was answered. `Err(None)`: the server takes no batch;
/// `Err(Some(said))`: what became of all of them at once.
fn batch_of(graph: &Graph<'_>, kind: Kind, requests: Vec<Value>, slots: usize) -> Result<Vec<Option<Answer>>, Option<Said>> {
    let body = json!({"requests": requests}).to_string();
    let patience = if matches!(kind, Kind::Read) { ASK_TIMEOUT } else { WRITE_TIMEOUT };
    let reply = match graph.call(kind, &graph.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), patience, MAX_BATCH) {
        Ok(reply) => reply,
        Err(e) if e.is_refused_sign_in() => return Err(Some(Said::SignIn)),
        Err(NetError::Busy { retry_after, .. }) => return Err(Some(Said::Busy(retry_after))),
        // (The lists were just read: a request that then doesn't get through is counted, so
        // that a server which never takes one isn't asked for ever.)
        Err(_) => return Err(Some(Said::Stuck)),
    };
    match reply.status {
        200 => {}
        401 => return Err(Some(Said::SignIn)),
        429 | 503 | 504 => return Err(Some(Said::Busy(wait_of(reply.retry_after.as_deref(), mailcraft_time::now())))),
        // (The server's own trouble with all of them: counted, as for one of them.)
        408 | 500..=599 => return Err(Some(Said::Stuck)),
        _ => return Err(None),
    }
    let mut json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let mut came: Vec<Option<Answer>> = vec![None; slots];
    let now = mailcraft_time::now();
    let mut answered = false;
    if let Some(responses) = json.get_mut("responses").and_then(Value::as_array_mut) {
        for one in responses.iter_mut().take(BATCH_SIZE * 2) {
            let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
            let status = one.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok());
            if let (Some(slot), Some(status)) = (n.and_then(|n| came.get_mut(n)), status) {
                let wait = wait_of(header(one, "Retry-After"), now);
                *slot = Some((status, one.get_mut("body").map(Value::take).unwrap_or(Value::Null), wait));
                answered = true;
            }
        }
    }
    // An answer that says nothing of any of them is the server's own trouble: counted, so that
    // a server that always answers so isn't asked for ever.
    if !answered && slots > 0 {
        return Err(Some(Said::Stuck));
    }
    Ok(came)
}

/// The changes of one chunk, in two `$batch`es: each item's categories as the server has them
/// now, then the change of each that still carries the old name. `Err(None)`: the server takes
/// no batch; `Err(Some(said))`: what became of all of them at once.
fn batch(graph: &Graph<'_>, items: &Items, chunk: &[Item], rename: &CategoryRename) -> Result<Vec<Said>, Option<Said>> {
    let query = read_query();
    let reads: Vec<Value> = chunk
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let url = format!("{}{}?{query}", items.item, http::encode(&item.id));
            json!({"id": n.to_string(), "method": "GET", "url": url, "headers": sub_headers(graph, false)})
        })
        .collect();
    // (One that isn't answered wasn't done, as far as is known: a later run finds it, and it is
    // counted, so that a server which never answers for it isn't asked for ever.)
    let mut said = vec![Said::Stuck; chunk.len()];
    let mut writes: Vec<Value> = Vec::new();
    let mut written: Vec<usize> = Vec::new();
    for (n, (item, came)) in chunk.iter().zip(batch_of(graph, Kind::Read, reads, chunk.len())?).enumerate() {
        let Some((status, body, wait)) = came else { continue };
        match fresh_of(status, &body, wait, rename) {
            Ok(categories) => {
                let url = format!("{}{}", items.item, http::encode(&item.id));
                writes.push(json!({
                    "id": n.to_string(), "method": "PATCH", "url": url, "headers": sub_headers(graph, true), "body": {"categories": categories}
                }));
                written.push(n);
            }
            Err(what) => {
                if let Some(slot) = said.get_mut(n) {
                    *slot = what;
                }
            }
        }
    }
    if writes.is_empty() {
        return Ok(said);
    }
    match batch_of(graph, Kind::Same, writes, chunk.len()) {
        Ok(came) => {
            // (Only for what was asked: an answer for one that wasn't written says nothing of it.)
            for n in written {
                if let (Some(slot), Some(Some((status, _, wait)))) = (said.get_mut(n), came.get(n)) {
                    *slot = said_of(*status, *wait);
                }
            }
        }
        // What the reads said of the others stands; the ones to write get the batch's answer.
        // (A server that took the batch of reads takes batches: another answer to this one is
        // its own trouble, counted.)
        Err(all) => {
            let all = all.unwrap_or(Said::Stuck);
            for n in written {
                if let Some(slot) = said.get_mut(n) {
                    *slot = all;
                }
            }
        }
    }
    Ok(said)
}

/// The same, one request each: the read, then the change. Nothing more is asked once the owner
/// stopped the run (`stopped`, looked at before each request): the rest waits for a later run,
/// and the second of the pair says so.
fn one_by_one(graph: &Graph<'_>, items: &Items, chunk: &[Item], rename: &CategoryRename, stopped: &dyn Fn() -> bool) -> (Vec<Said>, bool) {
    let query = read_query();
    let now = mailcraft_time::now;
    let mut said = Vec::with_capacity(chunk.len());
    for item in chunk {
        if stopped() {
            said.resize(chunk.len(), Said::Later);
            return (said, true);
        }
        let path = format!("/v1.0{}{}", items.item, http::encode(&item.id));
        let read = match graph.call(Kind::Read, &graph.url(&format!("{path}?{query}")), "GET", &[], None, ASK_TIMEOUT, MAX_WRITTEN) {
            Ok(reply) => {
                let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                fresh_of(reply.status, &body, wait_of(reply.retry_after.as_deref(), now()), rename)
            }
            Err(e) if e.is_refused_sign_in() => Err(Said::SignIn),
            Err(NetError::Busy { retry_after, .. }) => Err(Said::Busy(retry_after)),
            Err(_) => Err(Said::Stuck),
        };
        let one = match read {
            // (Stopped while the item was read: it isn't written, and waits.)
            Ok(_) if stopped() => {
                said.resize(chunk.len(), Said::Later);
                return (said, true);
            }
            Ok(categories) => {
                let body = json!({"categories": categories}).to_string();
                let headers = [("Content-Type", "application/json")];
                match graph.call(Kind::Same, &graph.url(&path), "PATCH", &headers, Some(&body), WRITE_TIMEOUT, MAX_WRITTEN) {
                    Ok(reply) => said_of(reply.status, wait_of(reply.retry_after.as_deref(), now())),
                    Err(e) if e.is_refused_sign_in() => Said::SignIn,
                    Err(NetError::Busy { retry_after, .. }) => Said::Busy(retry_after),
                    Err(_) => Said::Stuck,
                }
            }
            Err(what) => what,
        };
        said.push(one);
        // Nothing more is asked of a server that is busy or no longer takes the sign-in.
        if matches!(one, Said::Busy(_) | Said::SignIn) {
            said.resize(chunk.len(), Said::Later);
            break;
        }
    }
    (said, false)
}
