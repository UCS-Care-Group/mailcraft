//! A stand-in for the mail of Microsoft Graph that behaves like the documented service, for the
//! tests. Nothing here leaves this machine, and nothing here is in a build that isn't a test's.
//!
//! It keeps a mailbox (folders, and messages as Internet messages) and holds a client to the
//! contract of Graph v1.0 as its reference has it:
//!
//! - **Addresses.** `/v1.0/me/mailFolders` (the folders at the top), `/me/mailFolders/{id}` (by id
//!   or well-known name), its `childFolders`, `move` and `permanentDelete`, its `messages` (only
//!   with a `$filter` on `internetMessageId`) and `messages/delta`; `/me/messages` (a new one from
//!   an Internet message in base64, which Graph puts among the drafts), `/me/messages/{id}` (read
//!   and changed), its `$value`, `move` and `permanentDelete`; `/me/sendMail`; `/$batch` (twenty
//!   at most, addresses below `/v1.0`, a body that isn't JSON in base64). An id is one piece of
//!   an address: ids here hold `/` and `=`, so one that isn't written as `%XX` names nothing.
//! - **Headers.** `Authorization: Bearer` with the token (401 otherwise); every request about
//!   mail says `Prefer: IdType="ImmutableId"` (400 otherwise; in a batch, the request itself);
//!   `odata.maxpagesize` is honoured. Ids don't change when a message moves.
//! - **Rounds.** A delta query without a token lists every message of the folder; with the token
//!   of its `@odata.deltaLink` it lists what came or changed since, and `@removed` for what went.
//!   A list comes in pages with `@odata.nextLink`. A token it no longer knows is 410
//!   `syncStateNotFound` ([`State::forget_rounds`]).
//! - **Not now.** [`State::throttle`] requests to come are answered 429 with `Retry-After: 1`;
//!   [`State::throttle_batch`] requests inside batches are, with the header among their own.
//! - **Hostile.** [`State::astray`] has a next page named on another server; [`State::junk`] has
//!   a round answered with something that isn't JSON.
//! - **Another mailbox.** Everything above as `/v1.0/users/{address}/…` too, for the mailboxes
//!   the token may reach ([`State::opens`], compared in lower case; a test for a shared mailbox
//!   sets the stand-in up as that mailbox's); any other address is 403. `/me/sendMail` with a
//!   message as JSON (the owner's own sending from a shared mailbox) is kept apart
//!   ([`State::sent_as`]), or refused with `ErrorSendAsDenied` ([`State::send_as_denied`]).
//!
//! It is reached by [`route`](crate::graph::standin::route), like the calendar's.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Value, json};

/// The token it takes unless it is told another.
pub const TOKEN: &str = "standin.mail-tok_en.0123456789";
/// What the top of the mailbox is called.
pub const ROOT: &str = "AAMkAGV4YW1wbGU=/root";
/// Largest request it reads, and the pages of a list when none are asked for.
const MAX_REQUEST: usize = 64 * 1024 * 1024;
const PAGE: usize = 10;
/// What a folder and a message may be asked for by name.
const FOLDER_PROPERTIES: &[&str] = &["id", "displayName", "parentFolderId", "childFolderCount", "totalItemCount", "unreadItemCount"];
const MESSAGE_PROPERTIES: &[&str] =
    &["id", "isRead", "flag", "categories", "internetMessageId", "receivedDateTime", "isDraft", "subject", "internetMessageHeaders"];

/// A folder of mail.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// The folder it lies in ([`ROOT`] at the top).
    pub parent: String,
    pub well_known: String,
}

/// A message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored {
    pub id: String,
    pub folder: String,
    pub mime: Vec<u8>,
    pub read: bool,
    pub flagged: bool,
    pub draft: bool,
    pub categories: Vec<String>,
    pub received: Timestamp,
    /// Without the angle brackets.
    pub message_id: String,
    /// When it last came or changed, by the stand-in's count of changes.
    changed: u64,
}

/// One request it was sent, with the status it answered. In a batch, each request is one too.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub method: String,
    pub target: String,
    pub path: Vec<String>,
    pub query: Vec<(String, String)>,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub status: u16,
    /// It came inside a `$batch`.
    pub batched: bool,
}

impl Asked {
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(have, _)| have == name).map_or("", |(_, value)| value.as_str())
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// What it keeps, for a test to set up and look at.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub token: String,
    pub folders: Vec<Folder>,
    pub messages: Vec<Stored>,
    pub requests: Vec<Asked>,
    /// Internet messages sent through `sendMail`.
    pub sent: Vec<Vec<u8>>,
    /// How many requests to come are told to come back later (429).
    pub throttle: usize,
    /// And how many requests inside batches.
    pub throttle_batch: usize,
    /// The next page of a round is named on another server.
    pub astray: bool,
    /// A round is answered with something that isn't JSON.
    pub junk: bool,
    /// The most a page holds, whatever is asked.
    pub page_size: usize,
    /// Tokens of rounds before this count are no longer known (410).
    pub forgotten_before: u64,
    /// What it answers to the next request whose method is the first and whose address holds the
    /// second, instead of doing it: the status and Graph's name for it.
    pub fail_next: Vec<(String, String, u16, String)>,
    /// The mailboxes the token may reach as `/users/{address}/…` (in lower case). Any other is 403.
    pub opens: Vec<String>,
    /// Messages sent through `/me/sendMail` as JSON (from a shared mailbox through the owner's own
    /// sending): the `message` of each.
    pub sent_as: Vec<Value>,
    /// Such a message is refused: the owner has no Send As right (403 `ErrorSendAsDenied`).
    pub send_as_denied: bool,
    seq: u64,
    /// Messages that left a folder: the folder, the message, when.
    departures: Vec<(String, String, u64)>,
    made: u64,
    base: String,
}

/// An answer: the status and what goes with it.
enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

type Answer = (u16, Body);

fn error(status: u16, code: &str, message: &str) -> Answer {
    (status, Body::Json(json!({"error": {"code": code, "message": message}})))
}

fn bad(message: &str) -> Answer {
    error(400, "ErrorInvalidRequest", message)
}

fn not_found() -> Answer {
    error(404, "ErrorItemNotFound", "The specified object was not found in the store.")
}

fn piece(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16)).and_then(|d| u8::try_from(d).ok());
    let (mut out, mut i) = (Vec::with_capacity(bytes.len()), 0);
    while let Some(&b) = bytes.get(i) {
        match (b, hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_target(method: &str, target: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Asked {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Asked {
        method: method.to_string(),
        target: target.to_string(),
        path: path.split('/').filter(|part| !part.is_empty()).map(piece).collect(),
        query: crate::http::pairs(query),
        headers,
        body,
        status: 0,
        batched: false,
    }
}

impl State {
    /// A folder called `name` inside `parent` (`None`: at the top). Its id.
    pub fn add_folder(&mut self, name: &str, parent: Option<&str>) -> String {
        self.made += 1;
        let id = format!("AAMkAGV4YW1wbGU=/folder+{:04}", self.made);
        self.folders.push(Folder { id: id.clone(), name: name.into(), parent: parent.unwrap_or(ROOT).into(), well_known: String::new() });
        id
    }

    /// The id of the folder of that well-known name, or of that display name.
    pub fn folder(&self, name: &str) -> String {
        self.folders.iter().find(|f| f.well_known == name || f.name == name).map(|f| f.id.clone()).unwrap_or_default()
    }

    /// A message in `folder` (an id), received at `received`, with that subject. Its id.
    pub fn add_message(&mut self, folder: &str, subject: &str, received: Timestamp) -> String {
        self.made += 1;
        let n = self.made;
        let message_id = format!("m{n}@example.org");
        let mime = format!(
            "From: Ada <ada@example.org>\r\nTo: Owner <owner@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{message_id}>\r\nDate: {}\r\n\r\nText of {subject}.\r\n",
            mailcraft_time::format_rfc5322(received, 0)
        );
        self.put(folder, mime.into_bytes(), received, &message_id)
    }

    /// A message with these bytes. Its id.
    pub fn put(&mut self, folder: &str, mime: Vec<u8>, received: Timestamp, message_id: &str) -> String {
        self.made += 1;
        self.seq += 1;
        let id = format!("AAMkAGV4YW1wbGU=/msg+{:06}=", self.made);
        self.messages.push(Stored {
            id: id.clone(),
            folder: folder.into(),
            mime,
            received,
            message_id: message_id.into(),
            changed: self.seq,
            ..Default::default()
        });
        id
    }

    /// Change a message as another program would: `edit` has it.
    pub fn edit(&mut self, id: &str, edit: impl FnOnce(&mut Stored)) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
            edit(m);
            m.changed = seq;
        }
    }

    /// Take a message away as another program would.
    pub fn remove(&mut self, id: &str) {
        self.seq += 1;
        if let Some(at) = self.messages.iter().position(|m| m.id == id) {
            let m = self.messages.remove(at);
            self.departures.push((m.folder, m.id, self.seq));
        }
    }

    /// Have every round's token from before now forgotten.
    pub fn forget_rounds(&mut self) {
        self.forgotten_before = self.seq + 1;
    }

    pub fn message(&self, id: &str) -> Option<&Stored> {
        self.messages.iter().find(|m| m.id == id)
    }

    /// The messages of a folder (an id), by subject.
    pub fn subjects(&self, folder: &str) -> Vec<String> {
        let mut out: Vec<String> = self.messages.iter().filter(|m| m.folder == folder).map(|m| mailcraft_mime::parse(&m.mime).subject).collect();
        out.sort();
        out
    }

    fn folder_by(&self, name: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == name || (!f.well_known.is_empty() && f.well_known == name))
    }

    fn folder_json(&self, f: &Folder) -> Value {
        let children = self.folders.iter().filter(|c| c.parent == f.id).count();
        let total = self.messages.iter().filter(|m| m.folder == f.id).count();
        let unread = self.messages.iter().filter(|m| m.folder == f.id && !m.read).count();
        json!({"id": f.id, "displayName": f.name, "parentFolderId": f.parent, "childFolderCount": children, "totalItemCount": total, "unreadItemCount": unread})
    }

    fn message_json(&self, m: &Stored, select: Option<&str>) -> Value {
        let mut all = json!({
            "id": m.id,
            "isRead": m.read,
            "flag": {"flagStatus": if m.flagged { "flagged" } else { "notFlagged" }},
            "categories": m.categories,
            "internetMessageId": format!("<{}>", m.message_id),
            "receivedDateTime": format!("{}Z", DateTime::from_timestamp(m.received, 0).iso()),
            "isDraft": m.draft,
            "subject": mailcraft_mime::parse(&m.mime).subject,
        });
        if select.is_some_and(|s| s.contains("internetMessageHeaders")) {
            let parsed = mailcraft_mime::parse(&m.mime);
            all["internetMessageHeaders"] = json!([
                {"name": "From", "value": parsed.from.email},
                {"name": "Subject", "value": parsed.subject},
                {"name": "Message-ID", "value": format!("<{}>", m.message_id)},
            ]);
        }
        match select {
            Some(select) => {
                let mut out = serde_json::Map::new();
                for name in select.split(',') {
                    if let Some(v) = all.get(name) {
                        out.insert(name.into(), v.clone());
                    }
                }
                out.insert("id".into(), json!(m.id));
                Value::Object(out)
            }
            None => all,
        }
    }

    fn check_select(asked: &Asked, allowed: &[&str]) -> Result<(), Answer> {
        match asked.param("$select") {
            Some(select) if !select.split(',').all(|name| allowed.contains(&name)) => Err(bad("Could not find a property named in $select.")),
            _ => Ok(()),
        }
    }

    fn page_size(&self, asked: &Asked) -> usize {
        let prefer = asked.header("prefer");
        let asked_for = prefer
            .split(',')
            .filter_map(|p| p.trim().strip_prefix("odata.maxpagesize="))
            .find_map(|n| n.trim().parse::<usize>().ok())
            .or_else(|| asked.param("$top").and_then(|n| n.parse().ok()))
            .unwrap_or(PAGE);
        let cap = if self.page_size == 0 { 1_000 } else { self.page_size };
        asked_for.clamp(1, cap)
    }

    /// A page of `entries` from `skip`, with the address of the next page (`next`, given the
    /// offset) or the round's end (`end`).
    fn paged(&self, entries: Vec<Value>, skip: usize, size: usize, next: impl Fn(usize) -> String, end: Option<String>) -> Answer {
        let page: Vec<Value> = entries.iter().skip(skip).take(size).cloned().collect();
        let mut body = json!({ "value": page });
        if skip + size < entries.len() {
            let host = if self.astray { "http://elsewhere.example:8080".to_string() } else { self.base.clone() };
            body["@odata.nextLink"] = json!(format!("{host}{}", next(skip + size)));
        } else if let Some(end) = end {
            body["@odata.deltaLink"] = json!(format!("{}{end}", self.base));
        }
        (200, Body::Json(body))
    }

    fn answer(&mut self, asked: &Asked) -> Answer {
        if asked.header("authorization") != format!("Bearer {}", self.token) {
            return error(401, "InvalidAuthenticationToken", "Access token is empty or invalid.");
        }
        if self.throttle > 0 && !asked.batched {
            self.throttle -= 1;
            return error(429, "ApplicationThrottled", "Too many requests.");
        }
        if let Some(at) = self.fail_next.iter().position(|(method, part, _, _)| *method == asked.method && asked.target.contains(part.as_str())) {
            let (_, _, status, code) = self.fail_next.remove(at);
            return error(status, &code, "As the test asked.");
        }
        let path: Vec<&str> = asked.path.iter().map(String::as_str).collect();
        if path.first() != Some(&"v1.0") {
            return bad("Invalid version or resource.");
        }
        // Whose mailbox: the owner's own, or another's the token may reach.
        let (mine, rest) = match path.get(1..) {
            Some(["me", rest @ ..]) => ("/v1.0/me".to_string(), rest),
            Some(["$batch"]) => return self.batch(asked),
            Some(["users", address, rest @ ..]) => {
                if !self.opens.iter().any(|open| open.eq_ignore_ascii_case(address)) {
                    return error(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again.");
                }
                (format!("/v1.0/users/{}", crate::http::encode(address)), rest)
            }
            _ => return bad("Invalid version or resource."),
        };
        if rest.first() == Some(&"sendMail") {
            return self.send_mail(asked);
        }
        if !asked.header("prefer").contains("IdType=\"ImmutableId\"") {
            return bad("This test stand-in wants Prefer: IdType=\"ImmutableId\" on every request about mail.");
        }
        match (asked.method.as_str(), rest) {
            ("GET", ["mailFolders"]) => self.list_folders(asked, ROOT),
            ("POST", ["mailFolders"]) => self.make_folder(asked, ROOT),
            ("GET", ["mailFolders", "msgfolderroot"]) => (200, Body::Json(json!({"id": ROOT, "displayName": "Top of Information Store"}))),
            ("GET", ["mailFolders", id]) => match self.folder_by(id).cloned() {
                Some(f) => match Self::check_select(asked, FOLDER_PROPERTIES) {
                    Ok(()) => (200, Body::Json(self.folder_json(&f))),
                    Err(answer) => answer,
                },
                None => not_found(),
            },
            ("PATCH", ["mailFolders", id]) => {
                let Some(name) = asked.json().get("displayName").and_then(Value::as_str).map(str::to_string) else { return bad("displayName") };
                let id = self.folder_by(id).map(|f| f.id.clone());
                let Some(id) = id else { return not_found() };
                if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
                    f.name = name;
                }
                let f = self.folders.iter().find(|f| f.id == id).cloned().unwrap_or_default();
                (200, Body::Json(self.folder_json(&f)))
            }
            ("GET", ["mailFolders", id, "childFolders"]) => {
                let Some(id) = self.folder_by(id).map(|f| f.id.clone()).or_else(|| (*id == "msgfolderroot").then(|| ROOT.to_string())) else {
                    return not_found();
                };
                self.list_folders(asked, &id)
            }
            ("POST", ["mailFolders", id, "childFolders"]) => {
                let Some(id) = self.folder_by(id).map(|f| f.id.clone()) else { return not_found() };
                self.make_folder(asked, &id)
            }
            ("POST", ["mailFolders", id, "move"]) => {
                let Some(id) = self.folder_by(id).map(|f| f.id.clone()) else { return not_found() };
                let to = asked.json().get("destinationId").and_then(Value::as_str).unwrap_or("").to_string();
                let to = if to == "msgfolderroot" { Some(ROOT.to_string()) } else { self.folder_by(&to).map(|f| f.id.clone()) };
                let Some(to) = to else { return bad("destinationId") };
                if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
                    f.parent = to;
                }
                let f = self.folders.iter().find(|f| f.id == id).cloned().unwrap_or_default();
                (201, Body::Json(self.folder_json(&f)))
            }
            ("POST", ["mailFolders", id, "permanentDelete"]) => {
                let Some(id) = self.folder_by(id).map(|f| f.id.clone()) else { return not_found() };
                let mut gone = vec![id];
                let mut at = 0;
                while let Some(f) = gone.get(at).cloned() {
                    gone.extend(self.folders.iter().filter(|c| c.parent == f).map(|c| c.id.clone()));
                    at += 1;
                }
                self.folders.retain(|f| !gone.contains(&f.id));
                self.messages.retain(|m| !gone.contains(&m.folder));
                (204, Body::None)
            }
            ("GET", ["mailFolders", id, "messages"]) => {
                let Some(id) = self.folder_by(id).map(|f| f.id.clone()) else { return not_found() };
                let Some(filter) = asked.param("$filter") else { return bad("This stand-in lists a folder's messages only with a $filter") };
                let Some(wanted) = filter.strip_prefix("internetMessageId eq '").and_then(|rest| rest.strip_suffix('\'')) else {
                    return bad("$filter");
                };
                let wanted = wanted.replace("''", "'");
                let found: Vec<Value> =
                    self.messages.iter().filter(|m| m.folder == id && format!("<{}>", m.message_id) == wanted).map(|m| json!({"id": m.id})).collect();
                (200, Body::Json(json!({ "value": found })))
            }
            ("GET", ["mailFolders", id, "messages", "delta"]) => self.delta(asked, id, &mine),
            ("POST", ["messages"]) => self.new_message(asked),
            ("GET", ["messages", id]) => {
                if let Err(answer) = Self::check_select(asked, MESSAGE_PROPERTIES) {
                    return answer;
                }
                match self.message(id) {
                    Some(m) => (200, Body::Json(self.message_json(&m.clone(), asked.param("$select")))),
                    None => not_found(),
                }
            }
            ("GET", ["messages", id, "$value"]) => match self.message(id) {
                Some(m) => (200, Body::Bytes(m.mime.clone())),
                None => not_found(),
            },
            ("PATCH", ["messages", id]) => {
                let body = asked.json();
                let Some(fields) = body.as_object() else { return bad("not JSON") };
                if fields.keys().any(|k| !["isRead", "flag", "categories"].contains(&k.as_str())) {
                    return bad("A property that can't be changed this way.");
                }
                let Some(id) = self.message(id).map(|m| m.id.clone()) else { return not_found() };
                let read = fields.get("isRead").and_then(Value::as_bool);
                let flagged = fields.get("flag").and_then(|f| f.get("flagStatus")).and_then(Value::as_str).map(|s| s == "flagged");
                let categories: Option<Vec<String>> = fields
                    .get("categories")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect());
                self.edit(&id, |m| {
                    if let Some(read) = read {
                        m.read = read;
                    }
                    if let Some(flagged) = flagged {
                        m.flagged = flagged;
                    }
                    if let Some(categories) = categories {
                        m.categories = categories;
                    }
                });
                let m = self.message(&id).cloned().unwrap_or_default();
                (200, Body::Json(self.message_json(&m, None)))
            }
            ("POST", ["messages", id, "move"]) => {
                let to = asked.json().get("destinationId").and_then(Value::as_str).unwrap_or("").to_string();
                let Some(to) = self.folder_by(&to).map(|f| f.id.clone()) else { return bad("destinationId") };
                let Some(id) = self.message(id).map(|m| m.id.clone()) else { return not_found() };
                self.seq += 1;
                let seq = self.seq;
                if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
                    let from = std::mem::replace(&mut m.folder, to);
                    m.changed = seq;
                    self.departures.push((from, id.clone(), seq));
                }
                let m = self.message(&id).cloned().unwrap_or_default();
                (201, Body::Json(self.message_json(&m, None)))
            }
            ("POST", ["messages", id, "permanentDelete"]) => {
                let Some(id) = self.message(id).map(|m| m.id.clone()) else { return not_found() };
                self.remove(&id);
                (204, Body::None)
            }
            _ => bad("Resource not found for the segment."),
        }
    }

    fn list_folders(&self, asked: &Asked, parent: &str) -> Answer {
        if let Err(answer) = Self::check_select(asked, FOLDER_PROPERTIES) {
            return answer;
        }
        let entries: Vec<Value> = self.folders.iter().filter(|f| f.parent == parent).map(|f| self.folder_json(f)).collect();
        let size = asked
            .param("$top")
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(PAGE)
            .clamp(1, if self.page_size == 0 { 1_000 } else { self.page_size });
        let skip = asked.param("$skip").and_then(|n| n.parse().ok()).unwrap_or(0);
        let path = format!("/{}", asked.target.split_once('?').map_or(asked.target.as_str(), |(p, _)| p).trim_start_matches('/'));
        let select = asked.param("$select").unwrap_or("").to_string();
        self.paged(entries, skip, size, |at| format!("{path}?$select={select}&$top={size}&$skip={at}"), None)
    }

    fn make_folder(&mut self, asked: &Asked, parent: &str) -> Answer {
        let Some(name) = asked.json().get("displayName").and_then(Value::as_str).map(str::to_string) else { return bad("displayName") };
        if self.folders.iter().any(|f| f.parent == parent && f.name.eq_ignore_ascii_case(&name)) {
            return error(409, "ErrorFolderExists", "A folder with the specified name already exists.");
        }
        let id = self.add_folder(&name, Some(parent));
        let f = self.folders.iter().find(|f| f.id == id).cloned().unwrap_or_default();
        (201, Body::Json(self.folder_json(&f)))
    }

    fn delta(&mut self, asked: &Asked, folder: &str, mine: &str) -> Answer {
        let Some(folder) = self.folder_by(folder).map(|f| f.id.clone()) else { return not_found() };
        if let Err(answer) = Self::check_select(asked, MESSAGE_PROPERTIES) {
            return answer;
        }
        if self.junk {
            return (200, Body::Bytes(b"<html>not what you think</html>".to_vec()));
        }
        // Where the round is: since which change, up to which, from which entry; and what was asked.
        let (since, snapshot, skip, select) = match (asked.param("$deltatoken"), asked.param("$skiptoken")) {
            (Some(token), _) => match token.parse::<u64>() {
                Ok(since) if since >= self.forgotten_before => (since, self.seq, 0, String::new()),
                _ => return error(410, "syncStateNotFound", "The sync state is no longer available."),
            },
            (None, Some(skip)) => {
                let parts: Vec<&str> = skip.split('.').collect();
                let number = |at: usize| parts.get(at).and_then(|p| p.parse::<u64>().ok());
                match (number(0), number(1), number(2)) {
                    (Some(since), Some(snapshot), Some(skip)) => {
                        (since, snapshot, usize::try_from(skip).unwrap_or(0), parts.get(3).unwrap_or(&"").to_string())
                    }
                    _ => return bad("$skiptoken"),
                }
            }
            (None, None) => (0, self.seq, 0, asked.param("$select").unwrap_or("").to_string()),
        };
        let select = if select.is_empty() { None } else { Some(select.as_str()) };
        let mut here: Vec<&Stored> = self.messages.iter().filter(|m| m.folder == folder && m.changed > since && m.changed <= snapshot).collect();
        here.sort_by_key(|m| (m.changed, m.id.clone()));
        let mut entries: Vec<Value> = here.iter().map(|m| self.message_json(m, select)).collect();
        if since > 0 {
            for (_, id, _) in self.departures.iter().filter(|(f, id, seq)| {
                *f == folder && *seq > since && *seq <= snapshot && !self.messages.iter().any(|m| m.id == *id && m.folder == folder)
            }) {
                entries.push(json!({"id": id, "@removed": {"reason": "deleted"}}));
            }
        }
        let size = self.page_size(asked);
        let path = format!("{mine}/mailFolders/{}/messages/delta", crate::http::encode(&folder));
        let select_text = select.unwrap_or("").to_string();
        let next = |at: usize| format!("{path}?$skiptoken={since}.{snapshot}.{at}.{select_text}");
        self.paged(entries, skip, size, next, Some(format!("{path}?$deltatoken={snapshot}")))
    }

    fn new_message(&mut self, asked: &Asked) -> Answer {
        if !asked.header("content-type").starts_with("text/plain") {
            return error(415, "UnsupportedMediaType", "text/plain with an Internet message in base64");
        }
        let text: Vec<u8> = asked.body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
        let Ok(mime) = STANDARD.decode(&text) else { return bad("Invalid base64 string for MIME content.") };
        let parsed = mailcraft_mime::parse(&mime);
        let drafts = self.folder("drafts");
        let id = self.put(&drafts, mime, mailcraft_time::now(), &parsed.message_id);
        self.edit(&id, |m| m.draft = true);
        let m = self.message(&id).cloned().unwrap_or_default();
        (201, Body::Json(self.message_json(&m, None)))
    }

    fn send_mail(&mut self, asked: &Asked) -> Answer {
        // The owner's own sending from a shared mailbox: Graph's JSON, with the mailbox as `from`.
        if asked.header("content-type").starts_with("application/json") {
            if self.send_as_denied {
                return error(
                    403,
                    "ErrorSendAsDenied",
                    "The user account which was used to submit this request does not have the right to send mail on behalf of the specified sending account.",
                );
            }
            let Some(message) = asked.json().get("message").cloned().filter(Value::is_object) else { return bad("message") };
            if message.get("from").and_then(|f| f.get("emailAddress")).and_then(|a| a.get("address")).and_then(Value::as_str).is_none() {
                return bad("from");
            }
            self.sent_as.push(message);
            return (202, Body::None);
        }
        let text: Vec<u8> = asked.body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
        let Ok(mime) = STANDARD.decode(&text) else { return bad("Invalid base64 string for MIME content.") };
        let parsed = mailcraft_mime::parse(&mime);
        let sent = self.folder("sentitems");
        let id = self.put(&sent, mime.clone(), mailcraft_time::now(), &parsed.message_id);
        self.edit(&id, |m| m.read = true);
        self.sent.push(mime);
        (202, Body::None)
    }

    fn batch(&mut self, asked: &Asked) -> Answer {
        let body = asked.json();
        let Some(requests) = body.get("requests").and_then(Value::as_array).cloned() else { return bad("requests") };
        if requests.len() > 20 {
            return bad("The number of requests in a batch can't be more than 20.");
        }
        let mut responses = Vec::new();
        for request in requests {
            let id = request.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let method = request.get("method").and_then(Value::as_str).unwrap_or("GET").to_string();
            let url = request.get("url").and_then(Value::as_str).unwrap_or("").to_string();
            if !url.starts_with('/') || url.starts_with("/v1.0") {
                return bad("A batch request's url is relative to the version.");
            }
            let mut headers: Vec<(String, String)> = asked.headers.iter().filter(|(name, _)| name == "authorization").cloned().collect();
            if let Some(Value::Object(theirs)) = request.get("headers") {
                for (name, value) in theirs {
                    headers.push((name.to_ascii_lowercase(), value.as_str().unwrap_or("").to_string()));
                }
            }
            let body = request.get("body").map(|b| b.to_string().into_bytes()).unwrap_or_default();
            let mut inner = parse_target(&method, &format!("/v1.0{url}"), headers, body);
            inner.batched = true;
            let (status, answer) = if self.throttle_batch > 0 {
                self.throttle_batch -= 1;
                error(429, "ApplicationThrottled", "Too many requests.")
            } else {
                self.answer(&inner)
            };
            inner.status = status;
            self.requests.push(inner);
            let mut response = json!({"id": id, "status": status});
            if status == 429 {
                response["headers"] = json!({"Retry-After": "1"});
            }
            match answer {
                Body::None => {}
                Body::Json(value) => response["body"] = value,
                Body::Bytes(bytes) => response["body"] = json!(STANDARD.encode(bytes)),
            }
            responses.push(response);
        }
        (200, Body::Json(json!({ "responses": responses })))
    }
}

fn read_request(stream: &TcpStream) -> Option<Asked> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok().filter(|n| *n > 0)?;
    let mut words = line.split_whitespace();
    let (method, target) = (words.next()?.to_string(), words.next()?.to_string());
    let (mut headers, mut length) = (Vec::new(), 0usize);
    for _ in 0..200 {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_string());
        if name == "content-length" {
            length = value.parse().ok().filter(|n| *n <= MAX_REQUEST)?;
        }
        headers.push((name, value));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(parse_target(&method, &target, headers, body))
}

/// The stand-in, on a loopback port.
pub struct MailStandIn {
    pub port: u16,
    state: Arc<Mutex<State>>,
}

impl MailStandIn {
    /// Start one: the top of the mailbox and its special folders, nothing in them, taking [`TOKEN`].
    pub fn start() -> std::io::Result<MailStandIn> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let mut state = State { token: TOKEN.into(), base: format!("http://127.0.0.1:{port}"), seq: 1, ..Default::default() };
        for (name, well_known) in [
            ("Inbox", "inbox"),
            ("Drafts", "drafts"),
            ("Sent Items", "sentitems"),
            ("Deleted Items", "deleteditems"),
            ("Junk Email", "junkemail"),
            ("Archive", "archive"),
            ("Outbox", "outbox"),
        ] {
            let id = state.add_folder(name, None);
            if let Some(f) = state.folders.iter_mut().find(|f| f.id == id) {
                f.well_known = well_known.into();
            }
        }
        let state = Arc::new(Mutex::new(state));
        let shared = state.clone();
        std::thread::Builder::new().name("graph mail stand-in".into()).spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Some(mut asked) = read_request(&stream) else { continue };
                let mut state = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let (status, body) = state.answer(&asked);
                asked.status = status;
                state.requests.push(asked);
                drop(state);
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    202 => "Accepted",
                    204 => "No Content",
                    400 => "Bad Request",
                    401 => "Unauthorized",
                    403 => "Forbidden",
                    404 => "Not Found",
                    410 => "Gone",
                    429 => "Too Many Requests",
                    _ => "Other",
                };
                let retry = if status == 429 { "Retry-After: 1\r\n" } else { "" };
                let (kind, bytes) = match body {
                    Body::None => ("", Vec::new()),
                    Body::Json(value) => ("Content-Type: application/json\r\n", value.to_string().into_bytes()),
                    Body::Bytes(bytes) => ("Content-Type: text/plain\r\n", bytes),
                };
                let head = format!("HTTP/1.1 {status} {reason}\r\n{kind}{retry}Content-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&bytes);
            }
        })?;
        Ok(MailStandIn { port, state })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Have this thread's calls to Graph come here until the guard is dropped.
    pub fn route(&self) -> crate::graph::standin::Routed {
        crate::graph::standin::route(&self.url())
    }
}
