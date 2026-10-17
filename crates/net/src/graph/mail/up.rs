//! A Microsoft account's changes of messages up through Graph (R3.3.2; R3 spec S16–S22).
//!
//! After the folder changes and before the listing, the changes of messages the engine worked
//! out ([`MessageChange`]) are made, twenty to a `$batch` (S22), each request of a batch answered
//! and settled by itself:
//!
//! - **Properties** (`PATCH /me/messages/{id}`: `isRead`, `flag` with its dates, `categories`,
//!   `importance`) as one change of each message. Categories changed here are applied to the
//!   server's list as it is then, read first (`GET …?$select=categories`), so that a category
//!   added in Outlook meanwhile stays (S17).
//! - **Moves** (`POST /me/messages/{id}/move`). A move whose answer was lost is looked for by the
//!   message's immutable id, which a move keeps, before it is asked again (S21).
//! - **Copies**: the source is given MailCraft's invisible marker (a single-value extended
//!   property, of a value used once), the destination is searched for that value, and only when
//!   nothing there has it is the copy asked for (`POST /me/messages/{id}/copy`), which carries
//!   the marker: a copy whose answer was lost, in this run or an earlier one, is found and not
//!   made twice (S21).
//! - **Deletes for good** (`DELETE /me/messages/{id}`), which leave the message recoverable for
//!   the server's retention period, as Outlook does. Nothing here asks Graph to remove anything
//!   past that (S19). A delete the server refuses (403, 400) is followed by a read of the
//!   message (`GET /me/messages/{id}`), for the engine to put it back here as the server has it;
//!   one whose read isn't answered waits for the next run, delete and all.
//!
//! A request the server refuses (404, 403, 400) is answered so, for the engine to put back and
//! say (S20), and logged by status and Graph's code alone. One it is too busy for (429, 503,
//! 504) is asked again after the wait it named, when it is one that is the same however often
//! it is made (a property, a delete, the marker); a move or a copy waits for the next run. No
//! subject, address, id or name of a message goes into a log line or an error.

use std::collections::HashMap;

use mailcraft_model::sync::{FlagUp, MessageChange, MessageChangeDone, MessageOutcome, merge_categories};
use mailcraft_model::{FlagStatus, Importance};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Map, Value, json};

use super::super::pace::Kind;
use super::super::{BATCH, BATCH_SIZE, Doing, Graph, IMMUTABLE_IDS, MAX_BATCH, WRITE_TIMEOUT, logged_code, text};
use super::id_of;

use crate::NetError;
use crate::http;

/// MailCraft's marker: a named string property of a property set of MailCraft's own, which
/// nothing but MailCraft reads.
pub(crate) const MARKER: &str = "String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Name MailCraftMarker";
/// Most categories taken from the server's answer, and longest name.
const MAX_CATEGORIES: usize = 25;
const MAX_CATEGORY: usize = 255;
/// Longest marker sent (the engine's are far shorter).
const MAX_MARKER: usize = 100;

/// One request of a batch: which change it is for, and what it asks.
#[derive(Clone, Debug)]
struct Sub {
    at: usize,
    method: &'static str,
    url: String,
    body: Option<Value>,
}

/// What a request of a batch was answered: its status, body and `Retry-After`; or nothing (the
/// answer to the whole batch was lost, or didn't name this request).
#[derive(Clone, Debug)]
enum Said {
    Status(u16, Value, Option<u32>),
    Lost,
}

fn busy(status: u16) -> bool {
    matches!(status, 429 | 503 | 504)
}

/// Make the changes of messages of a run, in the order of S16, putting what became of each into
/// `out`. A change that isn't there waits for the next run. An error is what stops the run's
/// changes (the server busy, no sign-in, no answer).
pub(super) fn messages(
    graph: &Graph<'_>,
    made: &HashMap<u64, String>,
    changes: &[MessageChange],
    out: &mut Vec<MessageChangeDone>,
) -> Result<(), NetError> {
    let changes: Vec<&MessageChange> = changes.iter().take(mailcraft_model::sync::MAX_MESSAGE_CHANGES).collect();
    if changes.is_empty() {
        return Ok(());
    }
    let mut up = Up { graph, changes: &changes, out };
    up.updates()?;
    up.moves(made)?;
    up.copies(made)?;
    up.deletes()?;
    let refused = up.out.iter().filter(|done| !matches!(done.outcome, MessageOutcome::Done { .. })).count();
    log::info!("graph changes: {} of {} changes of messages answered, {refused} of them refused", up.out.len(), changes.len());
    Ok(())
}

struct Up<'g, 'c, 'o> {
    graph: &'g Graph<'g>,
    changes: &'c [&'c MessageChange],
    out: &'o mut Vec<MessageChangeDone>,
}

/// An id as the server writes one, as a piece of an address.
fn piece(id: &str) -> Option<String> {
    id_of(id).map(|id| http::encode(&id))
}

/// `{"dateTime": "…", "timeZone": "UTC"}`.
fn date_time(at: Timestamp) -> Value {
    json!({"dateTime": DateTime::from_timestamp(at, 0).iso(), "timeZone": "UTC"})
}

fn flag_body(flag: &FlagUp) -> Value {
    let mut body = Map::new();
    let status = match flag.status {
        FlagStatus::None => "notFlagged",
        FlagStatus::Flagged => "flagged",
        FlagStatus::Complete => "complete",
    };
    body.insert("flagStatus".into(), json!(status));
    if flag.status == FlagStatus::Flagged {
        // A due date goes only with a start: "to set the due date, you must also specify the
        // startDateTime; otherwise, you get a 400 Bad Request" (Graph's followupFlag). And a start
        // goes only with a due date: Microsoft 365 answers a start with a `dueDateTime` of `null`
        // (or none) 400, and takes the same with the due date at the start (Outlook's own flags
        // carry both). Without a start, `null` takes a due date away.
        match flag.start {
            Some(start) => {
                body.insert("startDateTime".into(), date_time(start));
                body.insert("dueDateTime".into(), date_time(flag.due.unwrap_or(start)));
            }
            None => {
                body.insert("dueDateTime".into(), Value::Null);
            }
        }
    }
    if let (FlagStatus::Complete, Some(done)) = (flag.status, flag.completed) {
        body.insert("completedDateTime".into(), date_time(done));
    }
    Value::Object(body)
}

fn importance_name(importance: Importance) -> &'static str {
    match importance {
        Importance::Low => "low",
        Importance::Normal => "normal",
        Importance::High => "high",
    }
}

/// The categories in an answer, capped.
fn categories_of(body: &Value) -> Option<Vec<String>> {
    let all = body.get("categories")?.as_array()?;
    Some(
        all.iter()
            .filter_map(Value::as_str)
            .take(MAX_CATEGORIES)
            .map(|c| c.chars().filter(|c| !c.is_control()).take(MAX_CATEGORY).collect())
            .collect(),
    )
}

/// The outcome of a refusal for good, when `status` is one.
fn refusal(status: u16) -> Option<MessageOutcome> {
    match status {
        404 | 410 => Some(MessageOutcome::Gone),
        400 | 403 => Some(MessageOutcome::Refused { status }),
        _ => None,
    }
}

fn done() -> MessageOutcome {
    placed(String::new(), String::new())
}

/// Done: the message (or the copy `id`) is in `folder` now.
fn placed(folder: String, id: String) -> MessageOutcome {
    MessageOutcome::Done { categories: None, flag: None, folder, id }
}

impl Up<'_, '_, '_> {
    fn settle(&mut self, at: usize, outcome: MessageOutcome) {
        if let Some(change) = self.changes.get(at) {
            self.out.push(MessageChangeDone { change: (*change).clone(), outcome, there: None });
        }
    }

    /// Answered for good with `status`: settled as a refusal, logged by status and code alone.
    /// Whether it was one.
    fn refused(&mut self, at: usize, status: u16, body: &Value, what: &str) -> bool {
        let Some(outcome) = refusal(status) else { return false };
        let code = logged_code(body.to_string().as_bytes(), &[self.graph.token]);
        log::info!("graph changes: {what} was refused (status {status}{code})");
        self.settle(at, outcome);
        true
    }

    /// One `$batch` of at most twenty requests, each answered by itself. A batch whose answer
    /// was lost is answered `Lost` throughout when it is of `Kind::Once` (whoever asked looks
    /// before asking again); any other trouble with the batch as a whole is the error.
    fn batch(&self, kind: Kind, subs: &[Sub]) -> Result<Vec<Said>, NetError> {
        let requests: Vec<Value> = subs
            .iter()
            .enumerate()
            .map(|(n, sub)| {
                let mut request = json!({"id": n.to_string(), "method": sub.method, "url": sub.url, "headers": {"Prefer": IMMUTABLE_IDS}});
                if let Some(body) = &sub.body {
                    request["body"] = body.clone();
                    request["headers"]["Content-Type"] = json!("application/json");
                }
                request
            })
            .collect();
        let body = json!({"requests": requests}).to_string();
        let graph = self.graph;
        let reply = match graph.call(kind, &graph.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_BATCH)
        {
            Ok(reply) => reply,
            Err(e) if kind == Kind::Once && e.is_unreachable() => {
                log::info!("graph changes: the answer to a batch of {} was lost; each is looked for before it is asked again", subs.len());
                return Ok(vec![Said::Lost; subs.len()]);
            }
            Err(e) => return Err(e),
        };
        if reply.status != 200 {
            return Err(graph.refusal(Doing::Mail, &reply, &body));
        }
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        let mut said = vec![Said::Lost; subs.len()];
        for one in json.get("responses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(BATCH_SIZE * 2) {
            let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
            let (Some(slot), Some(status)) = (n.and_then(|n| said.get_mut(n)), one.get("status").and_then(Value::as_u64)) else { continue };
            let retry_after = one
                .get("headers")
                .and_then(Value::as_object)
                .and_then(|headers| headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("Retry-After")))
                .and_then(|(_, value)| value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|n| n.to_string())))
                .and_then(|value| value.trim().parse::<u32>().ok());
            *slot = Said::Status(u16::try_from(status).unwrap_or(500), one.get("body").cloned().unwrap_or(Value::Null), retry_after);
        }
        Ok(said)
    }

    /// Requests that are the same however often they are made (reads, properties, deletes): in
    /// batches, each one that was told to come back later asked again after the wait it named
    /// (S21), as often as the retry policy allows. What each was answered, by the change it is
    /// for; one that never got an answer isn't there.
    fn repeatable(&self, kind: Kind, mut subs: Vec<Sub>) -> Result<Vec<(Sub, u16, Value)>, NetError> {
        let mut answered = Vec::new();
        let (tries, retry) = match &self.graph.pace {
            Some(pace) => (pace.retry.tries.max(1), Some(pace.retry)),
            None => (1, None),
        };
        for tried in 1..=tries {
            let mut again: Vec<Sub> = Vec::new();
            let mut wait: Option<u32> = None;
            for chunk in subs.chunks(BATCH_SIZE) {
                for (sub, said) in chunk.iter().zip(self.batch(kind, chunk)?) {
                    match said {
                        Said::Status(status, _, retry_after) if busy(status) => {
                            wait = wait.max(retry_after);
                            again.push(sub.clone());
                        }
                        Said::Status(status, body, _) => answered.push((sub.clone(), status, body)),
                        Said::Lost => again.push(sub.clone()),
                    }
                }
            }
            let (Some(retry), Some(pace)) = (retry, self.graph.pace.as_ref()) else { break };
            if again.is_empty() || tried >= tries {
                if !again.is_empty() {
                    log::info!("graph changes: {} changes were not taken yet; they wait for the next run", again.len());
                }
                break;
            }
            let wait = retry.wait_before(tried.saturating_add(1), wait);
            if wait > retry.longest_wait {
                log::info!("graph changes: the server asked for a wait of {} s; {} changes wait for the next run", wait.as_secs(), again.len());
                pace.gate.pause(wait);
                break;
            }
            log::debug!("graph changes: {} changes told to come back later; asked again in {} s", again.len(), wait.as_secs());
            // Everybody asking this mailbox waits, and these are asked again after.
            pace.gate.pause(wait);
            subs = again;
        }
        Ok(answered)
    }

    /// Requests that would happen twice (moves, copies), in batches, once each. What each was
    /// answered; `Lost` for one whose answer didn't come. One told to come back later isn't
    /// there: it waits for the next run. What stopped the batches, when something did: what
    /// was answered before is answered all the same (it was done, and is not to be done again).
    fn once(&self, subs: &[Sub]) -> (Vec<(Sub, Said)>, Option<NetError>) {
        let mut answered = Vec::new();
        for chunk in subs.chunks(BATCH_SIZE) {
            let said = match self.batch(Kind::Once, chunk) {
                Ok(said) => said,
                Err(e) => return (answered, Some(e)),
            };
            for (sub, said) in chunk.iter().zip(said) {
                if !matches!(said, Said::Status(status, _, _) if busy(status)) {
                    answered.push((sub.clone(), said));
                }
            }
        }
        (answered, None)
    }

    /// The properties: the server's categories first where they were changed here, then one
    /// `PATCH` per message.
    fn updates(&mut self) -> Result<(), NetError> {
        // The server's list of categories now, where categories were changed here.
        let mut reads = Vec::new();
        for (at, change) in self.changes.iter().enumerate() {
            if let MessageChange::Update { id, categories: Some(_), .. } = change {
                match piece(id) {
                    Some(id) => reads.push(Sub {
                        at,
                        method: "GET",
                        url: format!("/me/messages/{id}?{}", http::form(&[("$select", "categories")])),
                        body: None,
                    }),
                    None => self.settle(at, MessageOutcome::Refused { status: 400 }),
                }
            }
        }
        let mut server: HashMap<usize, Vec<String>> = HashMap::new();
        let mut settled: Vec<usize> = Vec::new();
        for (sub, status, body) in self.repeatable(Kind::Read, reads)? {
            match status {
                200 => {
                    server.insert(sub.at, categories_of(&body).unwrap_or_default());
                }
                _ => {
                    // Gone, or not to be read, is said so; anything else waits. Neither is sent.
                    self.refused(sub.at, status, &body, "reading a message's categories");
                    settled.push(sub.at);
                }
            }
        }
        let mut patches = Vec::new();
        let mut sent: HashMap<usize, (Option<Vec<String>>, Option<FlagUp>)> = HashMap::new();
        for (at, change) in self.changes.iter().enumerate() {
            let MessageChange::Update { id, read, flag, categories, importance, .. } = change else { continue };
            if settled.contains(&at) {
                continue;
            }
            let Some(id) = piece(id) else {
                if categories.is_none() {
                    self.settle(at, MessageOutcome::Refused { status: 400 });
                }
                continue;
            };
            let mut body = Map::new();
            if let Some(read) = read {
                body.insert("isRead".into(), json!(read));
            }
            if let Some(importance) = importance {
                body.insert("importance".into(), json!(importance_name(*importance)));
            }
            let merged = match categories {
                Some(up) => {
                    // Not read (it waits): not sent either.
                    let Some(there) = server.get(&at) else { continue };
                    let merged = merge_categories(&up.synced, &up.here, there);
                    body.insert("categories".into(), json!(merged));
                    Some(merged)
                }
                None => None,
            };
            if let Some(flag) = flag {
                body.insert("flag".into(), flag_body(flag));
            }
            // (A due date filled in at the start comes back with the folder's next delta.)
            sent.insert(at, (merged, flag.clone()));
            patches.push(Sub { at, method: "PATCH", url: format!("/me/messages/{id}"), body: Some(Value::Object(body)) });
        }
        for (sub, status, body) in self.repeatable(Kind::Same, patches)? {
            if (200..300).contains(&status) {
                let (categories, flag) = sent.remove(&sub.at).unwrap_or_default();
                self.settle(sub.at, MessageOutcome::Done { categories, flag, folder: String::new(), id: String::new() });
            } else {
                self.refused(sub.at, status, &body, "a change of a message");
            }
        }
        Ok(())
    }

    /// Where a move or a copy goes: the folder's server id (one made in this run by its id here).
    fn to(made: &HashMap<u64, String>, to: &str, to_folder: Option<u64>) -> Option<String> {
        let to = if to.is_empty() { made.get(&to_folder?)?.clone() } else { to.to_string() };
        id_of(&to)
    }

    /// Moves, and the moves whose answer was lost, looked for first.
    fn moves(&mut self, made: &HashMap<u64, String>) -> Result<(), NetError> {
        let mut subs = Vec::new();
        let mut to_of: HashMap<usize, String> = HashMap::new();
        for (at, change) in self.changes.iter().enumerate() {
            let MessageChange::Move { id, to, to_folder, .. } = change else { continue };
            // The folder isn't on the server yet: next time.
            let Some(to) = Up::to(made, to, *to_folder) else { continue };
            let Some(id) = piece(id) else {
                self.settle(at, MessageOutcome::Refused { status: 400 });
                continue;
            };
            subs.push(Sub { at, method: "POST", url: format!("/me/messages/{id}/move"), body: Some(json!({"destinationId": to})) });
            to_of.insert(at, to);
        }
        for round in 0..2 {
            let mut lost = Vec::new();
            let (answers, stopped) = self.once(&subs);
            for (sub, said) in answers {
                match said {
                    Said::Status(status, body, _) if (200..300).contains(&status) => {
                        // Where it was asked to go, as the mailbox names that folder.
                        let folder = to_of.get(&sub.at).cloned().or_else(|| id_of(text(&body, "parentFolderId"))).unwrap_or_default();
                        self.settle(sub.at, placed(folder, String::new()));
                    }
                    Said::Status(status, body, _) => {
                        self.refused(sub.at, status, &body, "a move of a message");
                    }
                    Said::Lost => lost.push(sub),
                }
            }
            if let Some(e) = stopped {
                return Err(e);
            }
            if lost.is_empty() || round == 1 {
                break;
            }
            // Where is it now? A move keeps the message's id.
            let looks: Vec<Sub> = lost
                .iter()
                .map(|sub| Sub {
                    url: format!("{}?{}", sub.url.trim_end_matches("/move"), http::form(&[("$select", "parentFolderId")])),
                    method: "GET",
                    body: None,
                    ..sub.clone()
                })
                .collect();
            let mut again = Vec::new();
            for (look, status, body) in self.repeatable(Kind::Read, looks)? {
                let to = to_of.get(&look.at).cloned().unwrap_or_default();
                match status {
                    200 if text(&body, "parentFolderId") == to => self.settle(look.at, placed(to, String::new())),
                    200 => again.extend(lost.iter().filter(|sub| sub.at == look.at).cloned()),
                    _ => {
                        self.refused(look.at, status, &body, "looking for a moved message");
                    }
                }
            }
            subs = again;
        }
        Ok(())
    }

    /// Copies: the marker on the source, a look for it in the destination, and the copy only
    /// when nothing there has it.
    fn copies(&mut self, made: &HashMap<u64, String>) -> Result<(), NetError> {
        let mut marks = Vec::new();
        let mut plan: HashMap<usize, (String, String, String)> = HashMap::new();
        for (at, change) in self.changes.iter().enumerate() {
            let MessageChange::Copy { id, to, to_folder, marker, .. } = change else { continue };
            let Some(to) = Up::to(made, to, *to_folder) else { continue };
            let marker_ok = !marker.is_empty() && marker.len() <= MAX_MARKER && marker.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            let (Some(id), true) = (piece(id), marker_ok) else {
                self.settle(at, MessageOutcome::Refused { status: 400 });
                continue;
            };
            let body = json!({"singleValueExtendedProperties": [{"id": MARKER, "value": marker}]});
            marks.push(Sub { at, method: "PATCH", url: format!("/me/messages/{id}"), body: Some(body) });
            plan.insert(at, (id, to, marker.clone()));
        }
        let mut marked = Vec::new();
        for (sub, status, body) in self.repeatable(Kind::Same, marks)? {
            if (200..300).contains(&status) {
                marked.push(sub.at);
            } else {
                self.refused(sub.at, status, &body, "marking a message to be copied");
            }
        }
        for round in 0..2 {
            // A copy the marker is on already (an answer lost, now or in an earlier run).
            let looks: Vec<Sub> = marked
                .iter()
                .filter_map(|at| {
                    let (_, to, marker) = plan.get(at)?;
                    let filter = format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{marker}')");
                    let query = http::form(&[("$filter", &filter), ("$select", "id,parentFolderId"), ("$top", "1")]);
                    Some(Sub { at: *at, method: "GET", url: format!("/me/mailFolders/{}/messages?{query}", http::encode(to)), body: None })
                })
                .collect();
            let mut to_copy = Vec::new();
            for (look, status, body) in self.repeatable(Kind::Read, looks)? {
                let first = body.get("value").and_then(Value::as_array).and_then(|all| all.first()).and_then(|m| id_of(text(m, "id")));
                match (status, first) {
                    (200, Some(made)) => {
                        let folder = plan.get(&look.at).map(|(_, to, _)| to.clone()).unwrap_or_default();
                        self.settle(look.at, placed(folder, made));
                    }
                    (200, None) => {
                        if let Some((id, to, _)) = plan.get(&look.at) {
                            to_copy.push(Sub {
                                at: look.at,
                                method: "POST",
                                url: format!("/me/messages/{id}/copy"),
                                body: Some(json!({"destinationId": to})),
                            });
                        }
                    }
                    _ => {
                        self.refused(look.at, status, &body, "looking for a copied message");
                    }
                }
            }
            let mut lost = Vec::new();
            let (answers, stopped) = self.once(&to_copy);
            for (sub, said) in answers {
                match said {
                    Said::Status(status, body, _) if (200..300).contains(&status) => {
                        let folder = plan.get(&sub.at).map(|(_, to, _)| to.clone()).unwrap_or_default();
                        match id_of(text(&body, "id")) {
                            Some(made) => self.settle(sub.at, placed(folder, made)),
                            // Made, but which one it is isn't said: found by its marker next time.
                            None => lost.push(sub.at),
                        }
                    }
                    Said::Status(status, body, _) => {
                        self.refused(sub.at, status, &body, "a copy of a message");
                    }
                    Said::Lost => lost.push(sub.at),
                }
            }
            if let Some(e) = stopped {
                return Err(e);
            }
            if lost.is_empty() || round == 1 {
                break;
            }
            marked = lost;
        }
        Ok(())
    }

    /// Deletes for good, and the server's copies of messages moved out of the account.
    fn deletes(&mut self) -> Result<(), NetError> {
        let mut subs = Vec::new();
        for (at, change) in self.changes.iter().enumerate() {
            let (MessageChange::Delete { id } | MessageChange::MovedOut { id, .. }) = change else { continue };
            match piece(id) {
                Some(id) => subs.push(Sub { at, method: "DELETE", url: format!("/me/messages/{id}"), body: None }),
                None => self.settle(at, MessageOutcome::Refused { status: 400 }),
            }
        }
        // A delete for good refused is put back here as the server has it: read first. One whose
        // look isn't answered waits for the next run with it (a delete is the same however often).
        let mut looks = Vec::new();
        for (sub, status, body) in self.repeatable(Kind::Same, subs)? {
            let refused_for_good = matches!(refusal(status), Some(MessageOutcome::Refused { .. }));
            if (200..300).contains(&status) {
                self.settle(sub.at, done());
            } else if refused_for_good && matches!(self.changes.get(sub.at), Some(MessageChange::Delete { .. })) {
                let code = logged_code(body.to_string().as_bytes(), &[self.graph.token]);
                log::info!("graph changes: a delete of a message was refused (status {status}{code}); it is read to be put back");
                let query = http::form(&[("$select", super::down::FIELDS), ("$expand", super::down::SIZE)]);
                looks.push((Sub { method: "GET", url: format!("{}?{query}", sub.url), body: None, ..sub }, status));
            } else {
                self.refused(sub.at, status, &body, "a delete of a message");
            }
        }
        let refused: HashMap<usize, u16> = looks.iter().map(|(sub, status)| (sub.at, *status)).collect();
        for (look, status, body) in self.repeatable(Kind::Read, looks.into_iter().map(|(sub, _)| sub).collect())? {
            let was = refused.get(&look.at).copied().unwrap_or(403);
            match status {
                200 => {
                    let there = super::down::message_of(&body).map(Box::new);
                    if let Some(change) = self.changes.get(look.at) {
                        self.out.push(MessageChangeDone { change: (*change).clone(), outcome: MessageOutcome::Refused { status: was }, there });
                    }
                }
                // Gone after all.
                404 | 410 => self.settle(look.at, MessageOutcome::Gone),
                // Not to be read either: put back from what is kept here.
                400 | 403 => self.settle(look.at, MessageOutcome::Refused { status: was }),
                _ => log::info!("graph changes: a message whose delete was refused couldn't be read (status {status}); it waits for the next run"),
            }
        }
        Ok(())
    }
}
