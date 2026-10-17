//! The stand-in's messages: a folder's message delta and a message's MIME, as Graph v1.0
//! documents them (`message: delta`, "Get incremental changes to messages in a folder", "Use
//! delta query to track changes"):
//!
//! - `GET /v1.0/me/mailFolders/{id}/messages/delta` answers the folder's messages in pages with
//!   an `@odata.nextLink` (a `$skiptoken`) until the round is done, and then an
//!   `@odata.deltaLink` (a `$deltatoken`), never both. Asked again by its delta link, it answers
//!   what changed since: messages new or changed in their state now, and `{"@removed":
//!   {"reason": "deleted"}, "id"}` for each one deleted or moved out of the folder. A page may be
//!   short, or empty, and still have a next link.
//! - The first request may say `$select` (message properties only), `$top`, and `$orderby`
//!   (`receivedDateTime desc` only; without it the order is the stand-in's own, oldest first, so
//!   a client that counts on newest first without asking finds out). A later one says its token
//!   and nothing else: the token carries the rest. `Prefer: odata.maxpagesize` sets the size of a
//!   page.
//! - An `$expand` is answered `400 BadRequest` ("Parsing OData Select and Expand failed"), as
//!   Exchange Online answers one of the size; [`Mail::refuse_delta`] refuses every first request.
//! - Ids are immutable ids only when `Prefer: IdType="ImmutableId"` is said; otherwise the same
//!   message answers by another id, as Graph's ordinary ids differ from immutable ones.
//! - A change may come again in a later page ([`Mail::replays`]); a token the server has
//!   forgotten is answered `400 syncStateNotFound` ([`Mail::forget_links`]); and a round may be
//!   told to start over (`410 Gone` with a `Location` that starts it afresh, [`Mail::resets`]).
//! - `GET /v1.0/me/messages/{id}/$value` answers the message's MIME.
//!
//! A round sees the mailbox as it was when it began: what changes while it runs comes in the
//! next round, so that a client that follows the links misses nothing and needs nothing twice.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde_json::{Value, json};

use super::{Answer, Asked, Refusal, State, bad, not_found, refuse, stamp};

/// What a message may be asked for by name (`$select`).
pub(super) const PROPERTIES: &[&str] = &[
    "id",
    "receivedDateTime",
    "sentDateTime",
    "subject",
    "from",
    "toRecipients",
    "ccRecipients",
    "bodyPreview",
    "isRead",
    "flag",
    "categories",
    "importance",
    "parentFolderId",
    "internetMessageId",
    "hasAttachments",
    "isDraft",
    "changeKey",
    "conversationId",
    "bccRecipients",
    "body",
];
/// Most messages a page holds, whatever is asked for.
const MAX_PAGE: usize = 1_000;
/// How an immutable id begins, and how the same message's ordinary id begins.
const IMMUTABLE: &str = "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A";
const ORDINARY: &str = "AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAA";

/// A message the stand-in has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// Its immutable id.
    pub id: String,
    /// The folder it is in, by the folder's id.
    pub folder: String,
    pub received: Timestamp,
    pub subject: String,
    pub from: String,
    pub to: Vec<String>,
    pub preview: String,
    pub read: bool,
    /// `notFlagged`, `flagged` or `complete`.
    pub flag: String,
    /// The flag's start, due and completed dates and times (instants), when it has them.
    pub flag_start: Option<Timestamp>,
    pub flag_due: Option<Timestamp>,
    pub flag_completed: Option<Timestamp>,
    pub categories: Vec<String>,
    /// `low`, `normal` or `high`.
    pub importance: String,
    /// Its Message-ID, with the angle brackets.
    pub internet_id: String,
    /// Its MIME when it is to be something else than what is made of the rest.
    pub mime: Option<String>,
    /// The size the server says it is (`PidTagMessageSize`); 0: the size of its MIME.
    pub size: u64,
    /// Its single-value extended properties: their ids as they were written, and their values.
    pub extended: Vec<(String, String)>,
    /// It is a draft (`isDraft`): its subject, body and recipients may be changed ([`super::drafts`]).
    pub draft: bool,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    /// Its body, and whether that is HTML (else text). Empty: the preview is its text.
    pub body: String,
    pub html: bool,
    /// The conversation it is part of (`conversationId`); empty: one of its own.
    pub conversation: String,
    /// The message it answers (`In-Reply-To`), with the angle brackets.
    pub in_reply_to: String,
    /// Its `internetMessageHeaders`, as they were written when it was made.
    pub headers: Vec<(String, String)>,
    /// Its attachments.
    pub attachments: Vec<super::drafts::Attached>,
    /// When it last changed, in the stand-in's count of changes.
    pub(super) changed: u64,
}

impl Message {
    /// The message as a list of messages writes it, with its id as asked for and only what
    /// `select` names (and its id) when it names anything; with its size when that was asked for
    /// (`$expand` of the extended property `Integer 0x0E08`, `PidTagMessageSize`).
    pub(super) fn json(&self, immutable: bool, select: Option<&[String]>, size: bool) -> Value {
        let address = |address: &str| json!({"emailAddress": {"name": address.split('@').next().unwrap_or(""), "address": address}});
        let mut all = json!({
            "id": shown(&self.id, immutable),
            "receivedDateTime": format!("{}Z", stamp(self.received).trim_end_matches(".0000000")),
            "sentDateTime": format!("{}Z", stamp(self.received.saturating_sub(30)).trim_end_matches(".0000000")),
            "subject": self.subject,
            "from": address(&self.from),
            "toRecipients": self.to.iter().map(|to| address(to)).collect::<Vec<_>>(),
            "ccRecipients": [],
            "bodyPreview": self.preview,
            "isRead": self.read,
            "flag": self.flag_json(),
            "categories": self.categories,
            "importance": self.importance,
            "parentFolderId": self.folder,
            "internetMessageId": self.internet_id,
            "hasAttachments": self.attachments.iter().any(|a| !a.inline),
            "isDraft": self.draft,
            "changeKey": self.change_key(),
            "conversationId": self.conversation_id(),
            "bccRecipients": self.bcc.iter().map(|to| address(to)).collect::<Vec<_>>(),
            "body": {"contentType": if self.html { "html" } else { "text" }, "content": self.text()},
        });
        all["ccRecipients"] = json!(self.cc.iter().map(|to| address(to)).collect::<Vec<_>>());
        if let (Some(select), Some(map)) = (select, all.as_object_mut()) {
            map.retain(|name, _| name == "id" || select.iter().any(|s| s == name));
        }
        if size {
            let bytes = if self.size > 0 { self.size } else { self.mime().len() as u64 };
            all["singleValueExtendedProperties"] = json!([{"id": "Integer 0xe08", "value": bytes.to_string()}]);
        }
        all
    }

    /// Its flag as Graph writes a `followupFlag`: the dates in UTC, and only those it has.
    pub(super) fn flag_json(&self) -> Value {
        let mut flag = json!({"flagStatus": self.flag});
        let dates = [("startDateTime", self.flag_start), ("dueDateTime", self.flag_due), ("completedDateTime", self.flag_completed)];
        for (name, at) in dates {
            if let Some(at) = at {
                flag[name] = json!({"dateTime": stamp(at), "timeZone": "UTC"});
            }
        }
        flag
    }

    /// Its change key: a new one with every change of it.
    pub fn change_key(&self) -> String {
        format!("CQAAABYAAAD{:010}", self.changed)
    }

    /// The conversation it is part of.
    pub fn conversation_id(&self) -> String {
        if self.conversation.is_empty() {
            format!("AAQkAD-c-{}", self.id.get(self.id.len().saturating_sub(16)..).unwrap_or(""))
        } else {
            self.conversation.clone()
        }
    }

    /// Its body's text: what it was written with, else its preview.
    pub fn text(&self) -> String {
        if self.body.is_empty() { self.preview.clone() } else { self.body.clone() }
    }

    /// Its MIME: what it was given, or a plain message made of its headers and preview.
    pub(super) fn mime(&self) -> String {
        if let Some(mime) = &self.mime {
            return mime.clone();
        }
        if self.draft || !self.attachments.is_empty() {
            return super::drafts::mime_of(self);
        }
        let date = mailcraft_time::DateTime::from_timestamp(self.received, 0);
        format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n",
            self.from,
            self.to.join(", "),
            self.subject,
            date.iso(),
            self.internet_id,
            self.preview
        )
    }
}

/// The immutable id of the message an id names (an ordinary id names the same message as the
/// immutable id it was made from).
pub(super) fn immutable_of(id: &str) -> String {
    match id.strip_prefix(ORDINARY) {
        Some(rest) => format!("{IMMUTABLE}{rest}"),
        None => id.to_string(),
    }
}

/// `id` as it is shown: itself, or the same message's ordinary id.
fn shown(id: &str, immutable: bool) -> String {
    match id.strip_prefix(IMMUTABLE) {
        Some(rest) if !immutable => format!("{ORDINARY}{rest}"),
        _ => id.to_string(),
    }
}

/// What a round of a folder's delta goes through, in order.
#[derive(Clone, Debug)]
enum Key {
    Message(String),
    Gone(String),
}

/// A round of a folder's delta, part way: what a `$skiptoken` stands for.
#[derive(Clone, Debug)]
struct Round {
    folder: String,
    /// The changes it covers go up to this one; what changed later comes in the next round.
    upto: u64,
    keys: Arc<Vec<Key>>,
    at: usize,
    page: usize,
    select: Option<Vec<String>>,
    newest_first: bool,
    size: bool,
}

/// What a `$deltatoken` stands for: the folder, and the changes it has seen.
#[derive(Clone, Debug)]
struct Seen {
    folder: String,
    upto: u64,
    select: Option<Vec<String>>,
    newest_first: bool,
    size: bool,
}

/// The stand-in's messages, and the rounds of delta under way.
#[derive(Clone, Debug, Default)]
pub struct Mail {
    pub messages: HashMap<String, Message>,
    /// What left a folder (deleted, or moved out): the folder and the message, and when.
    gone: HashMap<(String, String), u64>,
    changes: u64,
    made: u64,
    rounds: HashMap<String, Round>,
    /// What changes of a message are answered with instead of being made, by its immutable id
    /// ([`super::changes`]).
    pub faults: HashMap<String, super::changes::Fault>,
    seen: HashMap<String, Seen>,
    tokens: u64,
    /// How many pages to come end where the page before them ended, so that what was in that
    /// page comes again (Graph warns that changes may be replayed).
    pub replays: usize,
    /// How many requests by a token to come are answered `410 Gone`, with a `Location` that
    /// starts the folder afresh.
    pub resets: usize,
    /// Most messages a page of delta holds, whatever is asked for; 0 for as asked.
    pub page_cap: usize,
    /// The top of every folder's delta is refused (`400 ErrorInvalidRequest`), whatever it asks.
    pub refuse_delta: bool,
    /// Uploads of large attachments under way, by the token of their address ([`super::drafts`]).
    pub(super) uploads: HashMap<String, super::drafts::Upload>,
}

impl Mail {
    pub(super) fn change(&mut self) -> u64 {
        self.changes += 1;
        self.changes
    }

    fn token(&mut self) -> String {
        self.tokens += 1;
        // Opaque, and long, as Graph's are; `-` and `_` among them.
        format!("aSt-{:016x}_{:08x}", self.tokens.wrapping_mul(0x9E37_79B9_7F4A_7C15), self.tokens)
    }

    /// A new immutable id, and the number it was made with.
    pub(super) fn new_id(&mut self) -> (String, u64) {
        self.made += 1;
        (format!("{IMMUTABLE}{:010}AAA=", self.made), self.made)
    }

    /// A message of the stand-in's making in `folder`: its immutable id.
    pub fn add(&mut self, folder: &str, subject: &str, received: Timestamp) -> String {
        let (id, n) = self.new_id();
        let changed = self.change();
        self.gone.remove(&(folder.to_string(), id.clone()));
        let message = Message {
            id: id.clone(),
            folder: folder.to_string(),
            received,
            subject: subject.to_string(),
            from: format!("sender{}@example.org", n % 97),
            to: vec!["owner@example.org".into()],
            preview: format!("The words of message {n}."),
            flag: "notFlagged".into(),
            importance: "normal".into(),
            internet_id: format!("<m{n}@mail.example.org>"),
            changed,
            ..Default::default()
        };
        self.messages.insert(id.clone(), message);
        id
    }

    /// `m` as a new message, under a new immutable id: that id.
    pub(super) fn put(&mut self, mut m: Message) -> String {
        let (id, _) = self.new_id();
        m.id = id.clone();
        m.changed = self.change();
        self.gone.remove(&(m.folder.clone(), id.clone()));
        self.messages.insert(id.clone(), m);
        id
    }

    /// `count` messages in `folder`, one a minute back from `newest`, every third one read: their ids.
    pub fn generate(&mut self, folder: &str, count: usize, newest: Timestamp) -> Vec<String> {
        (0..count).map(|i| self.add(folder, &format!("Message {i}"), newest.saturating_sub(60 * i as i64))).collect()
    }

    /// Change a message as `how` does; `false` when there is no such message.
    pub fn update(&mut self, id: &str, how: impl FnOnce(&mut Message)) -> bool {
        let changed = self.change();
        let Some(message) = self.messages.get_mut(id) else { return false };
        how(message);
        message.changed = changed;
        true
    }

    /// A copy of a message in `to`, under an id of its own, with everything it has (its
    /// extended properties too): the copy's id; `None` when there is no such message.
    pub fn copy_to(&mut self, id: &str, to: &str) -> Option<String> {
        let mut copy = self.messages.get(id)?.clone();
        let (new, _) = self.new_id();
        copy.id = new.clone();
        copy.folder = to.to_string();
        copy.changed = self.change();
        self.gone.remove(&(to.to_string(), new.clone()));
        self.messages.insert(new.clone(), copy);
        Some(new)
    }

    /// Move a message to another folder: it is gone from the one it was in, and new in `to`.
    pub fn move_to(&mut self, id: &str, to: &str) -> bool {
        let Some(from) = self.messages.get(id).map(|m| m.folder.clone()) else { return false };
        let changed = self.change();
        self.gone.insert((from, id.to_string()), changed);
        self.gone.remove(&(to.to_string(), id.to_string()));
        self.update(id, |m| m.folder = to.to_string())
    }

    /// Delete a message for good.
    pub fn delete(&mut self, id: &str) -> bool {
        let Some(message) = self.messages.remove(id) else { return false };
        let changed = self.change();
        self.gone.insert((message.folder, id.to_string()), changed);
        true
    }

    /// Forget every token, as the server does when its cache of them is full.
    pub fn forget_links(&mut self) {
        self.rounds.clear();
        self.seen.clear();
    }

    /// How many messages `folder` has, and how many of them are unread.
    pub fn counts(&self, folder: &str) -> (usize, usize) {
        self.messages.values().filter(|m| m.folder == folder).fold((0, 0), |(all, unread), m| (all + 1, unread + usize::from(!m.read)))
    }

    /// The messages of `folder` changed after `since` up to `upto`, and what left it then,
    /// in the order of their changes; or, for a first round, all of its messages.
    fn keys(&self, folder: &str, since: Option<u64>, upto: u64, newest_first: bool) -> Vec<Key> {
        let mut messages: Vec<&Message> =
            self.messages.values().filter(|m| m.folder == folder && m.changed <= upto && since.is_none_or(|s| m.changed > s)).collect();
        match since {
            None if newest_first => messages.sort_by(|a, b| b.received.cmp(&a.received).then_with(|| a.id.cmp(&b.id))),
            None => messages.sort_by(|a, b| a.received.cmp(&b.received).then_with(|| a.id.cmp(&b.id))),
            Some(since) => {
                let mut all: Vec<(u64, Key)> = messages.iter().map(|m| (m.changed, Key::Message(m.id.clone()))).collect();
                all.extend(
                    self.gone
                        .iter()
                        .filter(|((f, _), at)| f == folder && **at > since && **at <= upto)
                        .map(|((_, id), at)| (*at, Key::Gone(id.clone()))),
                );
                all.sort_by_key(|(at, _)| *at);
                return all.into_iter().map(|(_, key)| key).collect();
            }
        }
        messages.into_iter().map(|m| Key::Message(m.id.clone())).collect()
    }

    /// The next page of `round`, and whether the round is done.
    fn page(&self, round: &mut Round, immutable: bool) -> (Vec<Value>, bool) {
        let mut out = Vec::new();
        while out.len() < round.page {
            let Some(key) = round.keys.get(round.at) else { break };
            round.at += 1;
            match key {
                // What changed again since the round began comes in the next round.
                Key::Message(id) => {
                    if let Some(m) = self.messages.get(id).filter(|m| m.folder == round.folder && m.changed <= round.upto) {
                        out.push(m.json(immutable, round.select.as_deref(), round.size));
                    }
                }
                Key::Gone(id) => {
                    if self.gone.get(&(round.folder.clone(), id.clone())).is_some_and(|at| *at <= round.upto) {
                        out.push(json!({"@removed": {"reason": "deleted"}, "id": shown(id, immutable)}));
                    }
                }
            }
        }
        (out, round.at >= round.keys.len())
    }
}

impl State {
    /// `GET /me/mailFolders/{id}/messages/delta`.
    pub(super) fn message_delta(&mut self, asked: &Asked, folder: &str) -> Result<Answer, Refusal> {
        let folder = self.mail_folder(folder)?.id.clone();
        let prefer = asked.header("prefer").to_ascii_lowercase();
        let immutable = prefer.contains("idtype=\"immutableid\"");
        let size = prefer.split([',', ';']).find_map(|p| p.trim().strip_prefix("odata.maxpagesize=").and_then(|n| n.trim().parse::<usize>().ok()));
        let skip = asked.param("$skiptoken");
        let delta = asked.param("$deltatoken");
        if (skip.is_some() || delta.is_some()) && asked.query.len() > 1 {
            return Err(bad("A request by a state token says nothing else: the token carries the rest."));
        }
        let mut round = match (skip, delta) {
            (Some(token), _) => match self.mailbox.rounds.get(token) {
                Some(round) if round.folder == folder => round.clone(),
                _ => return Err(self.expired()),
            },
            // An empty delta token starts afresh (it is what a reset's `Location` says).
            (None, Some(token)) if !token.is_empty() => {
                if self.mailbox.resets > 0 {
                    self.mailbox.resets -= 1;
                    return Err(self.reset(&folder));
                }
                let Some(seen) = self.mailbox.seen.get(token).filter(|s| s.folder == folder).cloned() else { return Err(self.expired()) };
                let upto = self.mailbox.changes;
                let keys = self.mailbox.keys(&folder, Some(seen.upto), upto, seen.newest_first);
                Round {
                    folder: folder.clone(),
                    upto,
                    keys: Arc::new(keys),
                    at: 0,
                    page: 0,
                    select: seen.select,
                    newest_first: seen.newest_first,
                    size: seen.size,
                }
            }
            _ => {
                if let Some((other, _)) =
                    asked.query.iter().find(|(name, _)| !matches!(name.as_str(), "$select" | "$top" | "$orderby" | "$deltatoken" | "$expand"))
                {
                    return Err(bad(format!("The query parameter '{other}' is not one this stand-in answers for a message delta.")));
                }
                if self.mailbox.refuse_delta {
                    return Err(refuse(400, "ErrorInvalidRequest", "The request is not valid for this folder."));
                }
                let newest_first = match asked.param("$orderby") {
                    None => false,
                    Some(order) if order.trim().eq_ignore_ascii_case("receivedDateTime desc") => true,
                    Some(_) => return Err(bad("The only order a message delta supports is 'receivedDateTime desc'.")),
                };
                let select: Option<Vec<String>> = asked.param("$select").map(|names| names.split(',').map(|n| n.trim().to_string()).collect());
                if let Some(unknown) = select.iter().flatten().find(|name| !PROPERTIES.contains(&name.as_str())) {
                    return Err(refuse(400, "BadRequest", format!("Could not find a property named '{unknown}' on type 'microsoft.graph.message'.")));
                }
                let page = match asked.param("$top") {
                    None => 0,
                    Some(n) => n
                        .parse::<usize>()
                        .ok()
                        .filter(|n| (1..=MAX_PAGE).contains(n))
                        .ok_or_else(|| bad("The value of '$top' must be between 1 and 1000."))?,
                };
                // As Exchange Online answers a delta that expands the size (or anything else).
                if asked.param("$expand").is_some() {
                    return Err(refuse(
                        400,
                        "BadRequest",
                        "Parsing OData Select and Expand failed: Value cannot be null. (Parameter 'initialState')",
                    ));
                }
                let upto = self.mailbox.changes;
                let keys = self.mailbox.keys(&folder, None, upto, newest_first);
                Round { folder: folder.clone(), upto, keys: Arc::new(keys), at: 0, page, select, newest_first, size: false }
            }
        };
        // The page size: what `Prefer` says, else what the first request said, within limits.
        let mut page = size.unwrap_or(if round.page == 0 { super::PAGE } else { round.page }).clamp(1, MAX_PAGE);
        if self.mailbox.page_cap > 0 {
            page = page.min(self.mailbox.page_cap);
        }
        round.page = page;
        let start = round.at;
        let (value, done) = self.mailbox.page(&mut round, immutable);
        let base = format!("{}/v1.0/{}/mailFolders/{folder}/messages/delta", self.base, self.whose());
        let mut list = json!({"@odata.context": format!("{}/v1.0/$metadata#Collection(message)", self.base), "value": value});
        if done && self.mailbox.replays == 0 {
            let token = self.mailbox.token();
            let seen =
                Seen { folder: folder.clone(), upto: round.upto, select: round.select.clone(), newest_first: round.newest_first, size: round.size };
            self.mailbox.seen.insert(token.clone(), seen);
            list["@odata.deltaLink"] = json!(format!("{base}?$deltatoken={token}"));
        } else {
            if self.mailbox.replays > 0 {
                // What this page held comes again in the next one.
                self.mailbox.replays -= 1;
                round.at = start;
            }
            let token = self.mailbox.token();
            self.mailbox.rounds.insert(token.clone(), round);
            list["@odata.nextLink"] = json!(format!("{base}?$skiptoken={token}"));
        }
        Ok((200, Some(list)))
    }

    /// A token the server no longer has.
    fn expired(&self) -> Refusal {
        refuse(400, "syncStateNotFound", "The sync state for this request was not found. Start over with a new request.")
    }

    /// `410 Gone`: start the folder afresh, at the address the `Location` header gives.
    fn reset(&mut self, folder: &str) -> Refusal {
        self.location = Some(format!("{}/v1.0/{}/mailFolders/{folder}/messages/delta?$deltatoken=", self.base, self.whose()));
        refuse(410, "resyncRequired", "The delta state was reset. Synchronize again from the start.")
    }

    /// `GET /me/messages/{id}/$value`: the message's MIME.
    pub(super) fn message_value(&mut self, id: &str) -> Result<Answer, Refusal> {
        if id.len() < 20 || id.chars().any(|c| !(c.is_ascii_alphanumeric() || "-_=".contains(c))) {
            return Err(refuse(400, "ErrorInvalidIdMalformed", "Id is malformed."));
        }
        let immutable = immutable_of(id);
        let mime = self.mailbox.messages.get(&immutable).map(Message::mime).ok_or_else(not_found)?;
        self.raw = Some(("text/plain".into(), mime));
        Ok((200, None))
    }
}

#[cfg(test)]
mod tests;
