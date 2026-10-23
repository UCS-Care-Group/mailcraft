// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An in-process JMAP server for tests: no socket, no thread, nothing leaves the process. A
//! [`StandIn`] is an [`HttpExchange`], so a [`Connection`](crate::Connection) connects to it as it would to a
//! real server (`Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN)?)`) while the test
//! keeps a clone to fill it with mailboxes and mail and to look at what it was asked.
//!
//! Where the other stand-ins of the tests answer whatever they are told and take whatever they are
//! sent, this one holds a client to the standard (RFC 8620, 8621) as a real server does, for the part
//! it serves: the session, `Mailbox/get`, `Email/query`, `Email/get` and the download of a blob.
//!
//! - **Who.** `Authorization: Basic` with [`USER`] and [`PASSWORD`], or `Bearer` with the one token it
//!   holds ([`Data::token`], [`TOKEN`] at first; a token it no longer holds is refused like any other:
//!   401). Nothing else is let in, not even the session.
//! - **What.** A request of method calls is `application/json` and JSON with `using` and `methodCalls`
//!   (400 `notJSON`, `notRequest`: a content type that is not JSON is `notJSON`). Only the capabilities
//!   it knows may be named in `using` (400 `unknownCapability`), no more calls than `maxCallsInRequest`
//!   and no more bytes than `maxSizeRequest` may be in a request (400 `limit`), and a method needs its
//!   capability in `using` or it is unknown.
//! - **How.** The arguments are the ones a method has, of the right type (`invalidArguments`): all of
//!   `Mailbox/get`'s, `Email/get`'s without the bodies, and `Email/query`'s but `anchor`, `anchorOffset`
//!   and a `collapseThreads` that is true (it has no threads). The account is the one the session names
//!   (`accountNotFound`). Ids are the URL-safe alphabet, `A-Z a-z 0-9 - _`, 1 to 255 characters (RFC 8620
//!   §1.2), and no more than `maxObjectsInGet` are asked for at a time (`requestTooLarge`); an id that names
//!   nothing is in `notFound`. Properties are among
//!   the ones the stand-in has. `Email/query` lists one mailbox (`{"inMailbox": id}`, `unsupportedFilter`)
//!   sorted by `receivedAt` (`unsupportedSort`), at most 500 at a time (it says the `limit` it applied),
//!   from a `position` that may be negative, and says the total when asked. `#`-arguments resolve a
//!   back-reference to an earlier call of the same request, with RFC 6901 pointers and `*` over lists
//!   (`invalidResultReference`).
//! - **Blobs.** The session's `downloadUrl` (`/jmap/download/{accountId}/{blobId}/{name}?type={type}`) gives
//!   the octets of an email's blob (`blobId`), as they were delivered, to the credentials it lets in
//!   (RFC 8620 §6.2). An account or a blob it doesn't have, and a path that isn't that of a download, is a
//!   404. Like any answer, one over the most the request may be answered with is no answer
//!   ([`TransportError::BodyTooLarge`]).
//! - **What it answers with.** JSON as `application/json`, a request refused as a whole as problem details
//!   (`application/problem+json`, RFC 9457), a blob as `application/octet-stream`, and a `Retry-After` in
//!   seconds when it asks to be left alone.
//! - **Not there.** Anything that changes mail, uploads, sending, `*/changes`, threads, push. The session
//!   names no `eventSourceUrl`.
//! - **Kinds of server.** [`Data::profile`] makes it a store of the standard kind, which parses the header
//!   fields of an email for a client (`subject`, `from`…), or a server that names a message by where it
//!   is, which serves the raw fields only, keeps an email in one mailbox and refuses `Mailbox/set`
//!   ([`Profile`], `standin/by_place.rs`).
//! - **Trouble on demand.** A test makes the next requests to the API answer `429` or `503` with a
//!   `Retry-After` ([`Data::refuse_next`]), makes a token expire in the middle of a run
//!   ([`Data::drop_token_after`]), makes a mailbox, or the details of some emails, fail with a
//!   method error ([`Data::broken_mailboxes`], [`Data::broken_emails`]), and makes a mailbox that it
//!   can not open answer as if it held nothing, which is how a store that swallows the failure of its
//!   `SELECT` answers ([`Data::hollow`], [`Data::forgetful`], [`Data::forgetful_headers`]), or leave out what it holds
//!   ([`Data::uncounted`]).
//! - **What it was asked.** [`Seen::asked`] has, for every call it ran, the mailbox it listed, the number
//!   of ids and the properties it was asked for, and the limit: what a test needs to count the work.
//!
//! It never panics, whatever it is sent: there is no `unwrap` here, in a test build or in another
//! crate's, which turns the `stand-in` feature on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use mailcraft_model::{Address, Message};
use serde_json::{Map, Value, json};

use crate::{CAPABILITY_CORE as CORE, CAPABILITY_MAIL as MAIL, HttpExchange, HttpRequest, HttpResponse, Id, TransportError};

mod by_place;

pub use by_place::Profile;

/// The session's address, to connect to.
pub const SESSION: &str = "https://jmap.standin.example/jmap/session";
/// The one account, and the user and password of `Basic`.
pub const ACCOUNT: &str = "A1";
pub const USER: &str = "user@standin.example";
pub const PASSWORD: &str = "standin-password";
/// The bearer token a stand-in holds at first.
pub const TOKEN: &str = "standin-token";

const ORIGIN: &str = "https://jmap.standin.example";
const SESSION_PATH: &str = "/jmap/session";
const API_PATH: &str = "/jmap/api";
const DOWNLOAD_PATH: &str = "/jmap/download/";
/// Largest request it reads (`maxSizeRequest`), and most ids one `Email/query` answers however many are asked.
const MAX_REQUEST: usize = 10_000_000;
const MAX_QUERY: usize = 500;
/// Most requests kept for a test to look at.
const MAX_SEEN: usize = 10_000;
/// Most header fields, and longest preview, of an email.
const MAX_HEADERS: usize = 500;
const MAX_PREVIEW: usize = 256;
/// Most segments of the path of a back-reference.
const MAX_POINTER: usize = 16;

const MAILBOX_PROPERTIES: &[&str] = &["id", "name", "parentId", "role", "sortOrder", "totalEmails", "unreadEmails"];
const EMAIL_PROPERTIES: &[&str] = &["id", "blobId", "mailboxIds", "keywords", "size", "receivedAt", "hasAttachment", "preview", "headers"];
/// What a store of the standard kind parses out of the header for a client (RFC 8621 §4.1).
const EMAIL_PARSED: &[&str] = &["threadId", "subject", "from", "to", "cc", "messageId", "sentAt"];
/// What `Email/get` answers when no properties are named: all but the raw headers.
const EMAIL_DEFAULT: &[&str] = &["id", "blobId", "mailboxIds", "keywords", "size", "receivedAt", "hasAttachment", "preview"];

/// A mailbox the stand-in has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub role: Option<String>,
}

/// An email the stand-in has.
#[derive(Clone, PartialEq, Eq)]
pub struct Email {
    pub id: String,
    pub blob: String,
    pub mailboxes: BTreeSet<String>,
    pub keywords: BTreeSet<String>,
    /// A UTC date as `2026-10-15T06:00:00Z`: emails are put in order by it, as text.
    pub received: String,
    /// The message, as it was delivered.
    pub raw: Vec<u8>,
}

/// What may be shown of an email: not the message (mail is private, even a test's).
impl std::fmt::Debug for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Email")
            .field("id", &self.id)
            .field("mailboxes", &self.mailboxes)
            .field("keywords", &self.keywords)
            .field("received", &self.received)
            .field("raw_bytes", &self.raw.len())
            .finish()
    }
}

/// One HTTP request the stand-in was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    /// The path, without the query.
    pub path: String,
    /// The status it was answered with.
    pub status: u16,
    /// The methods it called, in order (none for a request that was refused before they were read).
    pub calls: Vec<String>,
    /// What each of those calls was asked, once its references were resolved (a call whose arguments could
    /// not be read is not here).
    pub asked: Vec<Asked>,
}

/// What one method call asked for: the part of it a test counts the work by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub method: String,
    /// The mailbox an `Email/query` listed.
    pub mailbox: Option<String>,
    /// How many ids a `get` was given.
    pub ids: Option<usize>,
    /// The properties asked for (none named: empty).
    pub properties: Vec<String>,
    /// The `limit` of a query.
    pub limit: Option<usize>,
}

impl Asked {
    fn of(method: &str, args: &Map<String, Value>) -> Asked {
        let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
        Asked {
            method: method.to_string(),
            mailbox: text(args.get("filter").and_then(|filter| filter.get("inMailbox"))),
            ids: args.get("ids").and_then(Value::as_array).map(Vec::len),
            properties: args
                .get("properties")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            limit: args.get("limit").and_then(Value::as_u64).map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        }
    }
}

/// What the stand-in holds, for a test to set up and to look at.
pub struct Data {
    pub mailboxes: Vec<Mailbox>,
    pub emails: Vec<Email>,
    /// The requests it was sent, oldest first (the first 10,000).
    pub seen: Vec<Seen>,
    /// What the session says it takes: method calls in a request, ids in a `get`.
    pub max_calls: usize,
    pub max_get: usize,
    /// The one bearer token it lets in; `None`: no bearer token.
    pub token: Option<String>,
    /// What kind of server it is.
    pub profile: Profile,
    /// What the next requests to the API are answered with instead of being run, oldest first: `Some`
    /// refuses one with an HTTP status (429 and 503 are the ones a client waits out) and a `Retry-After`
    /// of that many seconds; `None` lets that one through.
    pub refuse_next: VecDeque<Option<(u16, Option<u32>)>>,
    /// Lets this many more requests to the API through, then forgets the bearer token (as a server does
    /// that ends a session): the next request is refused with 401.
    pub drop_token_after: Option<usize>,
    /// Mailboxes (ids) that an `Email/query` fails for, with the method error of that `type`
    /// (`serverFail`, `serverUnavailable`…).
    pub broken_mailboxes: BTreeMap<String, &'static str>,
    /// Emails (ids) that an `Email/get` fails for as a whole, with the method error `serverFail`, when it asks
    /// for their `headers` (what a server has to fetch the message for; the keywords come without).
    pub broken_emails: BTreeSet<String>,
    /// Mailboxes (ids) the server can not open and says nothing of: an `Email/query` for one lists no email
    /// and a total of 0, and an `Email/get` of an email that lies in such mailboxes alone says it is not found.
    /// `Mailbox/get` still counts what the mailbox holds, as a store does that reads the counts another way.
    pub hollow: BTreeSet<String>,
    /// Mailboxes (ids) whose emails an `Email/get` can not find, though an `Email/query` lists them: a `SELECT`
    /// that works for one call and fails for the next. As [`Data::hollow`], for an email that lies in such
    /// mailboxes alone.
    pub forgetful: BTreeSet<String>,
    /// As [`Data::forgetful`], but only for an `Email/get` that asks for the `headers` (what a server has to
    /// fetch the message for): the keywords come, and then the headers are not found.
    pub forgetful_headers: BTreeSet<String>,
    /// Mailboxes (ids) whose counts `Mailbox/get` leaves out (`totalEmails`, `unreadEmails`): a store that could
    /// not read them.
    pub uncounted: BTreeSet<String>,
    next: u32,
    version: u32,
    /// For the answer being made: the `Retry-After` it carries, and what its calls were asked.
    retry_after: Option<u32>,
    asked: Vec<Asked>,
}

impl Data {
    fn new() -> Data {
        Data {
            mailboxes: Vec::new(),
            emails: Vec::new(),
            seen: Vec::new(),
            max_calls: 16,
            max_get: 500,
            token: Some(TOKEN.into()),
            profile: Profile::standard(),
            refuse_next: VecDeque::new(),
            drop_token_after: None,
            broken_mailboxes: BTreeMap::new(),
            broken_emails: BTreeSet::new(),
            hollow: BTreeSet::new(),
            forgetful: BTreeSet::new(),
            forgetful_headers: BTreeSet::new(),
            uncounted: BTreeSet::new(),
            next: 0,
            version: 0,
            retry_after: None,
            asked: Vec::new(),
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next = self.next.saturating_add(1);
        format!("{prefix}{}", self.next)
    }

    fn changed(&mut self) {
        self.version = self.version.saturating_add(1);
    }

    fn state(&self) -> String {
        format!("s{}", self.version)
    }

    /// A mailbox (`role` as `inbox`, `sent`, `trash`…): its id.
    pub fn mailbox(&mut self, name: &str, parent: Option<&str>, role: Option<&str>) -> String {
        let id = self.id("M");
        self.mailboxes.push(Mailbox { id: id.clone(), name: name.into(), parent: parent.map(str::to_string), role: role.map(str::to_string) });
        self.changed();
        id
    }

    /// An email in `mailbox` (an id this stand-in gave), as a message `raw` with `keywords` (`$seen`…)
    /// received at `received` (`2026-10-15T06:00:00Z`): its id.
    pub fn deliver(&mut self, mailbox: &str, raw: &[u8], keywords: &[&str], received: &str) -> String {
        let blob = self.id("B");
        let id = self.id("E");
        self.emails.push(Email {
            id: id.clone(),
            blob,
            mailboxes: BTreeSet::from([mailbox.to_string()]),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            received: received.into(),
            raw: raw.to_vec(),
        });
        self.changed();
        id
    }

    pub fn email(&self, id: &str) -> Option<&Email> {
        self.emails.iter().find(|e| e.id == id)
    }

    /// Put an email in one more mailbox, as a server that lets an email lie in several does: whether it
    /// was done (not when either is unknown, or when the profile keeps an email in one mailbox).
    pub fn add_to_mailbox(&mut self, email: &str, mailbox: &str) -> bool {
        if self.profile.one_mailbox || !self.mailboxes.iter().any(|m| m.id == mailbox) {
            return false;
        }
        let Some(email) = self.emails.iter_mut().find(|e| e.id == email) else {
            return false;
        };
        email.mailboxes.insert(mailbox.to_string());
        self.changed();
        true
    }

    /// Move an email to another mailbox, as another client does (it lies in that one alone afterwards): its id
    /// afterwards, which is the same one unless this server names mail by where it lies, when a move gives the
    /// email (and its blob) a new one; `None` when either is unknown. Moving it to the one it lies in alone is
    /// no move.
    pub fn move_to(&mut self, email: &str, mailbox: &str) -> Option<String> {
        if !self.mailboxes.iter().any(|m| m.id == mailbox) || !self.emails.iter().any(|e| e.id == email) {
            return None;
        }
        let to = BTreeSet::from([mailbox.to_string()]);
        let moves = self.emails.iter().any(|e| e.id == email && e.mailboxes != to);
        let renamed = (moves && self.profile.one_mailbox).then(|| (self.id("B"), self.id("E")));
        let moved = self.emails.iter_mut().find(|e| e.id == email)?;
        moved.mailboxes = to;
        if let Some((blob, id)) = renamed {
            (moved.blob, moved.id) = (blob, id);
        }
        let now = moved.id.clone();
        self.changed();
        Some(now)
    }

    /// Remove an email, as the server does when another client deletes it (or moves it, for a server that
    /// names an email by where it is): whether there was one. Its blob goes with it.
    pub fn remove(&mut self, email: &str) -> bool {
        let before = self.emails.len();
        self.emails.retain(|e| e.id != email);
        let removed = self.emails.len() < before;
        if removed {
            self.changed();
        }
        removed
    }

    /// The id of the mailbox called `name`.
    pub fn mailbox_named(&self, name: &str) -> Option<&str> {
        self.mailboxes.iter().find(|m| m.name == name).map(|m| m.id.as_str())
    }

    /// Set or clear a keyword, as another client would: whether there was such an email.
    pub fn set_keyword(&mut self, email: &str, keyword: &str, on: bool) -> bool {
        let Some(email) = self.emails.iter_mut().find(|e| e.id == email) else {
            return false;
        };
        if on {
            email.keywords.insert(keyword.to_string());
        } else {
            email.keywords.remove(keyword);
        }
        self.changed();
        true
    }

    fn authorized(&self, fields: &[(&str, &str)]) -> bool {
        let Some((scheme, credentials)) = header(fields, "authorization").and_then(|v| v.split_once(' ')) else {
            return false;
        };
        if scheme.eq_ignore_ascii_case("bearer") {
            return self.token.as_deref() == Some(credentials);
        }
        if scheme.eq_ignore_ascii_case("basic") {
            let decoded = mailcraft_charset::base64::STANDARD.decode(credentials).ok().and_then(|bytes| String::from_utf8(bytes).ok());
            return decoded.and_then(|text| text.split_once(':').map(|(user, password)| user == USER && password == PASSWORD)).unwrap_or(false);
        }
        false
    }

    /// One HTTP request, by its method, its address, its header fields and its body.
    fn handle(&mut self, method: &str, url: &str, fields: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, TransportError> {
        // No address, or another place: no route to that host.
        let address = crate::Address::parse(url).map_err(|_| TransportError::Unreachable)?;
        if address.origin().to_string() != ORIGIN {
            return Err(TransportError::Unreachable);
        }
        let path = address.path().to_string();
        // A blob is the one answer that isn't JSON.
        let (status, body, calls) = if method == "GET" && path.starts_with(DOWNLOAD_PATH) {
            let (status, body) = self.download(fields, &path);
            (status, body, Vec::new())
        } else {
            let (status, body, calls) = self.route(method, &path, fields, body);
            (status, serde_json::to_vec(&body).unwrap_or_default(), calls)
        };
        let (asked, retry_after) = (std::mem::take(&mut self.asked), self.retry_after.take());
        if self.seen.len() < MAX_SEEN {
            self.seen.push(Seen { method: method.to_string(), path: path.clone(), status, calls, asked });
        }
        let content_type = match status {
            200 if path.starts_with(DOWNLOAD_PATH) => "application/octet-stream",
            200 => "application/json",
            _ => "application/problem+json",
        };
        let mut answer = HttpResponse::new(status, body).with_field("Content-Type", content_type);
        if let Some(seconds) = retry_after {
            answer.set_field("Retry-After", &seconds.to_string());
        }
        Ok(answer)
    }

    /// `GET /jmap/download/{accountId}/{blobId}/{name}` (RFC 8620 §6.2): the octets of a blob, as they
    /// were delivered. The credentials are asked for as for any other address.
    fn download(&self, fields: &[(&str, &str)], path: &str) -> (u16, Vec<u8>) {
        let refused = |status: u16, detail: &str| (status, serde_json::to_vec(&problem(status, "about:blank", detail)).unwrap_or_default());
        if !self.authorized(fields) {
            return refused(401, "the credentials were refused");
        }
        let mut parts = path.strip_prefix(DOWNLOAD_PATH).unwrap_or_default().split('/');
        let (Some(account), Some(blob), Some(name), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return refused(404, "no such address");
        };
        if account != ACCOUNT || name.is_empty() {
            return refused(404, "no such blob");
        }
        match self.emails.iter().find(|e| e.blob == blob) {
            Some(email) => (200, email.raw.clone()),
            None => refused(404, "no such blob"),
        }
    }

    fn route(&mut self, method: &str, path: &str, fields: &[(&str, &str)], body: &[u8]) -> (u16, Value, Vec<String>) {
        if self.drop_token_after == Some(0) {
            (self.token, self.drop_token_after) = (None, None);
        }
        if !self.authorized(fields) {
            return (401, problem(401, "about:blank", "the credentials were refused"), Vec::new());
        }
        match (method, path) {
            ("GET", SESSION_PATH) => (200, self.session(), Vec::new()),
            ("POST", API_PATH) => {
                // A server that can't take this one just now (the queue holds `None` for one it can).
                if let Some(Some((status, retry_after))) = self.refuse_next.pop_front() {
                    self.retry_after = retry_after;
                    return (status, problem(status, "about:blank", "the server can't take this request just now"), Vec::new());
                }
                if let Some(left) = self.drop_token_after.as_mut() {
                    *left = left.saturating_sub(1);
                }
                let json = header(fields, "content-type")
                    .is_some_and(|v| v.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")));
                if json { self.api(body) } else { rejected("notJSON", "the request isn't application/json") }
            }
            (_, SESSION_PATH | API_PATH) => (405, problem(405, "about:blank", "not a method of that address"), Vec::new()),
            _ => (404, problem(404, "about:blank", "no such address"), Vec::new()),
        }
    }

    fn session(&self) -> Value {
        json!({
            "capabilities": {
                CORE: {
                    "maxSizeUpload": 25_000_000,
                    "maxConcurrentUpload": 2,
                    "maxSizeRequest": MAX_REQUEST,
                    "maxConcurrentRequests": 2,
                    "maxCallsInRequest": self.max_calls,
                    "maxObjectsInGet": self.max_get,
                    "maxObjectsInSet": self.max_get,
                    "collationAlgorithms": ["i;ascii-casemap"],
                },
                MAIL: {},
            },
            "accounts": {
                ACCOUNT: {
                    "name": USER,
                    "isPersonal": true,
                    "isReadOnly": false,
                    "accountCapabilities": {MAIL: {
                        "maxMailboxesPerEmail": if self.profile.one_mailbox { json!(1) } else { Value::Null },
                        "emailQuerySortOptions": ["receivedAt"],
                    }},
                },
            },
            "primaryAccounts": {MAIL: ACCOUNT},
            "username": USER,
            "apiUrl": API_PATH,
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "state": self.state(),
        })
    }

    /// A request of method calls: the status, the body, and the names of the methods it called.
    fn api(&mut self, body: &[u8]) -> (u16, Value, Vec<String>) {
        if body.len() > MAX_REQUEST {
            return limit("maxSizeRequest", "the request is too large");
        }
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return rejected("notJSON", "the body isn't JSON");
        };
        let using = request.get("using").and_then(Value::as_array).and_then(|u| u.iter().map(Value::as_str).collect::<Option<Vec<&str>>>());
        let (Some(using), Some(calls)) = (using, request.get("methodCalls").and_then(Value::as_array)) else {
            return rejected("notRequest", "a request has `using` (a list of names) and `methodCalls`");
        };
        if using.iter().any(|capability| !matches!(*capability, CORE | MAIL)) {
            return rejected("unknownCapability", "a capability this server doesn't have is in `using`");
        }
        if calls.len() > self.max_calls {
            return limit("maxCallsInRequest", "too many method calls");
        }
        let mut parsed: Vec<(&str, &Value, &str)> = Vec::new();
        for call in calls {
            let parts = call.as_array().filter(|p| p.len() == 3);
            let name = parts.and_then(|p| p.first()).and_then(Value::as_str);
            let id = parts.and_then(|p| p.get(2)).and_then(Value::as_str);
            let (Some(name), Some(args), Some(id)) = (name, parts.and_then(|p| p.get(1)), id) else {
                return rejected("notRequest", "a method call is [name, arguments, id]");
            };
            parsed.push((name, args, id));
        }
        let (mut done, mut responses, mut names) = (Vec::new(), Vec::new(), Vec::new());
        for (name, args, id) in parsed {
            names.push(name.to_string());
            if let Ok(resolved) = arguments(args, &done) {
                self.asked.push(Asked::of(name, &resolved));
            }
            let outcome = self.method(&using, name, args, &done);
            let (response, result) = match outcome {
                Ok(result) => (json!([name, result, id]), Some(result)),
                Err(refusal) => (json!(["error", {"type": refusal.kind, "description": refusal.description}, id]), None),
            };
            responses.push(response);
            done.push(Done { id: id.to_string(), name: name.to_string(), result });
        }
        (200, json!({"methodResponses": responses, "sessionState": self.state()}), names)
    }

    fn method(&self, using: &[&str], name: &str, args: &Value, done: &[Done]) -> Outcome<Value> {
        // A method whose capability the request doesn't name is unknown to the server (RFC 8620 §3.6.2).
        let served = match name {
            "Mailbox/get" | "Email/query" | "Email/get" => true,
            "Mailbox/set" => self.profile.fixed_mailboxes,
            _ => false,
        };
        if !served || !using.contains(&MAIL) {
            return Err(refuse("unknownMethod", format!("no method {}", name.chars().take(40).collect::<String>())));
        }
        let args = arguments(args, done)?;
        match name {
            "Mailbox/get" => self.mailbox_get(&args),
            "Mailbox/set" => by_place::mailbox_set(&args, &self.state()),
            "Email/query" => self.email_query(&args),
            _ => self.email_get(&args),
        }
    }

    fn mailbox_get(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ids", "properties"])?;
        account(args)?;
        let wanted = ids(args, self.max_get)?;
        let properties = properties(args, MAILBOX_PROPERTIES, MAILBOX_PROPERTIES)?;
        let (mut list, mut not_found) = (Vec::new(), Vec::new());
        match wanted {
            None if self.mailboxes.len() > self.max_get => return Err(refuse("requestTooLarge", "more mailboxes than a `get` may answer")),
            None => list.extend(self.mailboxes.iter().map(|m| self.mailbox_json(m, &properties))),
            Some(wanted) => {
                for id in wanted {
                    match self.mailboxes.iter().find(|m| m.id == id) {
                        Some(m) => list.push(self.mailbox_json(m, &properties)),
                        None => not_found.push(id),
                    }
                }
            }
        }
        Ok(json!({"accountId": ACCOUNT, "state": self.state(), "list": list, "notFound": not_found}))
    }

    fn mailbox_json(&self, mailbox: &Mailbox, properties: &[String]) -> Value {
        let inside = || self.emails.iter().filter(|e| e.mailboxes.contains(&mailbox.id));
        let mut out = Map::new();
        out.insert("id".into(), json!(mailbox.id));
        for property in properties {
            let value = match property.as_str() {
                "name" => json!(mailbox.name),
                "parentId" => json!(mailbox.parent),
                "role" => json!(mailbox.role),
                "sortOrder" => json!(0),
                "totalEmails" | "unreadEmails" if self.uncounted.contains(&mailbox.id) => continue,
                "totalEmails" => json!(inside().count()),
                // Neither $seen nor $draft (RFC 8621 §2).
                "unreadEmails" => json!(inside().filter(|e| !e.keywords.contains("$seen") && !e.keywords.contains("$draft")).count()),
                _ => continue,
            };
            out.insert(property.clone(), value);
        }
        Value::Object(out)
    }

    fn email_query(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "filter", "sort", "position", "limit", "calculateTotal", "collapseThreads"])?;
        account(args)?;
        let unsupported = || refuse("unsupportedFilter", "only one mailbox can be listed: {\"inMailbox\": id}");
        let mailbox = match args.get("filter") {
            Some(Value::Object(filter)) if filter.len() == 1 => filter.get("inMailbox").and_then(Value::as_str).ok_or_else(unsupported)?,
            _ => return Err(unsupported()),
        };
        if Id::new(mailbox).is_err() {
            return Err(invalid("not a mailbox id"));
        }
        if !self.mailboxes.iter().any(|m| m.id == mailbox) {
            return Err(invalid("no such mailbox"));
        }
        if let Some(kind) = self.broken_mailboxes.get(mailbox) {
            return Err(refuse(kind, "this mailbox can't be read just now"));
        }
        let ascending = sort_direction(args)?;
        let asked = number(args, "limit")?;
        let limit = asked.map_or(MAX_QUERY, |limit| limit.min(MAX_QUERY));
        let total = match args.get("calculateTotal") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(wanted)) => *wanted,
            Some(_) => return Err(invalid("calculateTotal is a boolean")),
        };
        match args.get("collapseThreads") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => return Err(invalid("threads can't be collapsed: there are none")),
            Some(_) => return Err(invalid("collapseThreads is a boolean")),
        }
        // Oldest first, and those received together in the order they were delivered.
        let mut found: Vec<(usize, &Email)> = self.emails.iter().enumerate().filter(|(_, e)| e.mailboxes.contains(mailbox)).collect();
        found.sort_by(|a, b| (a.1.received.as_str(), a.0).cmp(&(b.1.received.as_str(), b.0)));
        if !ascending {
            found.reverse();
        }
        if self.hollow.contains(mailbox) {
            found.clear();
        }
        let count = found.len();
        // A negative position counts from the end (RFC 8620 §5.5), and not beyond the start.
        let position = match integer(args, "position")?.unwrap_or(0) {
            from_end if from_end < 0 => count.saturating_sub(usize::try_from(from_end.unsigned_abs()).unwrap_or(usize::MAX)),
            from_start => usize::try_from(from_start).unwrap_or(usize::MAX),
        };
        let ids: Vec<&str> = found.iter().skip(position).take(limit).map(|(_, e)| e.id.as_str()).collect();
        let mut out = json!({"accountId": ACCOUNT, "queryState": self.state(), "canCalculateChanges": false, "position": position, "ids": ids});
        if let Some(map) = out.as_object_mut() {
            if total {
                map.insert("total".into(), json!(count));
            }
            // A limit it chose, rather than the one asked for, is said (RFC 8620 §5.5).
            if asked.is_none_or(|asked| asked > MAX_QUERY) {
                map.insert("limit".into(), json!(MAX_QUERY));
            }
        }
        Ok(out)
    }

    fn email_get(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ids", "properties"])?;
        account(args)?;
        // `ids: null` asks for every email, which is more than a `get` answers.
        let Some(wanted) = ids(args, self.max_get)? else {
            return Err(refuse("requestTooLarge", "name the emails to read"));
        };
        let allowed: Vec<&str> = if self.profile.parsed_fields { [EMAIL_PROPERTIES, EMAIL_PARSED].concat() } else { EMAIL_PROPERTIES.to_vec() };
        let properties = properties(args, &allowed, EMAIL_DEFAULT)?;
        if properties.iter().any(|p| p == "headers") && wanted.iter().any(|id| self.broken_emails.contains(id)) {
            return Err(refuse("serverFail", "an email can't be read just now"));
        }
        let (mut list, mut not_found) = (Vec::new(), Vec::new());
        let with_headers = properties.iter().any(|p| p == "headers");
        let shut = |m: &String| self.hollow.contains(m) || self.forgetful.contains(m) || (with_headers && self.forgetful_headers.contains(m));
        for id in wanted {
            match self.email(&id) {
                // (One that lies nowhere is none that a server has.)
                Some(email) if email.mailboxes.is_empty() || !email.mailboxes.iter().all(shut) => list.push(email_json(email, &properties)),
                _ => not_found.push(id),
            }
        }
        Ok(json!({"accountId": ACCOUNT, "state": self.state(), "list": list, "notFound": not_found}))
    }
}

/// An earlier call of the same request and what it answered (`None`: it failed).
struct Done {
    id: String,
    name: String,
    result: Option<Value>,
}

/// A method call that failed: JMAP's `type` and `description`.
struct Refusal {
    kind: &'static str,
    description: String,
}

type Outcome<T> = std::result::Result<T, Refusal>;

fn refuse(kind: &'static str, description: impl Into<String>) -> Refusal {
    Refusal { kind, description: description.into() }
}

fn invalid(description: impl Into<String>) -> Refusal {
    refuse("invalidArguments", description)
}

/// A request refused as a whole (RFC 8620 §3.6.1): problem details with HTTP status 400.
fn rejected(kind: &str, detail: &str) -> (u16, Value, Vec<String>) {
    (400, problem(400, &format!("urn:ietf:params:jmap:error:{kind}"), detail), Vec::new())
}

/// A request over a limit of the session (RFC 8620 §3.6.1): the problem names the limit.
fn limit(name: &str, detail: &str) -> (u16, Value, Vec<String>) {
    let body = json!({"type": "urn:ietf:params:jmap:error:limit", "status": 400, "limit": name, "detail": detail});
    (400, body, Vec::new())
}

fn problem(status: u16, kind: &str, detail: &str) -> Value {
    json!({"type": kind, "status": status, "detail": detail})
}

fn header<'a>(fields: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    fields.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

/// The arguments of a method call, with every `#name` replaced by the result it refers to.
fn arguments(args: &Value, done: &[Done]) -> Outcome<Map<String, Value>> {
    let Some(map) = args.as_object() else {
        return Err(invalid("the arguments are an object"));
    };
    let mut out = Map::new();
    for (key, value) in map {
        match key.strip_prefix('#') {
            Some(name) if map.contains_key(name) => return Err(invalid("an argument is given by value and by reference")),
            Some(name) => out.insert(name.to_string(), reference(value, done)?),
            None => out.insert(key.clone(), value.clone()),
        };
    }
    Ok(out)
}

/// A result reference (RFC 8620 §3.7): `{resultOf, name, path}`.
fn reference(value: &Value, done: &[Done]) -> Outcome<Value> {
    let bad = || refuse("invalidResultReference", "the reference doesn't name a result of an earlier call");
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let (Some(of), Some(name), Some(path)) = (text("resultOf"), text("name"), text("path")) else {
        return Err(invalid("a reference has resultOf, name and path"));
    };
    let earlier = done.iter().find(|d| d.id == of && d.name == name).and_then(|d| d.result.as_ref()).ok_or_else(bad)?;
    let segments: Vec<&str> = path.split('/').collect();
    match segments.split_first() {
        Some((&"", rest)) if rest.len() <= MAX_POINTER => pointer(earlier, rest).ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// The value at an RFC 6901 pointer, in which `*` stands for every item of a list (RFC 8620 §3.7:
/// the lists found are joined into one).
fn pointer(value: &Value, segments: &[&str]) -> Option<Value> {
    let Some((first, rest)) = segments.split_first() else {
        return Some(value.clone());
    };
    let key = first.replace("~1", "/").replace("~0", "~");
    match value {
        Value::Array(items) if key == "*" => {
            let mut joined = Vec::new();
            for item in items {
                match pointer(item, rest)? {
                    Value::Array(inner) => joined.extend(inner),
                    other => joined.push(other),
                }
            }
            Some(Value::Array(joined))
        }
        Value::Array(items) => pointer(items.get(index_of(&key)?)?, rest),
        Value::Object(map) => pointer(map.get(&key)?, rest),
        _ => None,
    }
}

/// An array index in a pointer (RFC 6901): digits, with no sign and no leading zero.
fn index_of(token: &str) -> Option<usize> {
    let canonical = token == "0" || (!token.starts_with('0') && !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()));
    if canonical { token.parse().ok() } else { None }
}

fn allow_only(args: &Map<String, Value>, names: &[&str]) -> Outcome<()> {
    match args.keys().find(|key| !names.contains(&key.as_str())) {
        Some(key) => Err(invalid(format!("no argument called {}", key.chars().take(40).collect::<String>()))),
        None => Ok(()),
    }
}

fn account(args: &Map<String, Value>) -> Outcome<()> {
    match args.get("accountId").and_then(Value::as_str) {
        Some(ACCOUNT) => Ok(()),
        Some(_) => Err(refuse("accountNotFound", "no such account")),
        None => Err(invalid("accountId is needed")),
    }
}

/// The `ids` of a `get`: `None` for `null`.
fn ids(args: &Map<String, Value>, most: usize) -> Outcome<Option<Vec<String>>> {
    match args.get("ids") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) if items.len() > most => {
            Err(refuse("requestTooLarge", format!("{} ids, and {most} are taken at a time", items.len())))
        }
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().filter(|id| Id::new(id).is_ok()).map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .map(Some)
            .ok_or_else(|| invalid("an id is 1 to 255 characters of A-Z a-z 0-9 - _")),
        Some(_) => Err(invalid("ids is a list, or null")),
    }
}

fn properties(args: &Map<String, Value>, allowed: &[&str], default: &[&str]) -> Outcome<Vec<String>> {
    match args.get("properties") {
        None | Some(Value::Null) => Ok(default.iter().map(|p| p.to_string()).collect()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().filter(|p| allowed.contains(p)).map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| invalid("a property this server doesn't have")),
        Some(_) => Err(invalid("properties is a list, or null")),
    }
}

fn integer(args: &Map<String, Value>, key: &str) -> Outcome<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| invalid(format!("{key} is an integer"))),
    }
}

fn number(args: &Map<String, Value>, key: &str) -> Outcome<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            value.as_u64().map(|n| Some(usize::try_from(n).unwrap_or(usize::MAX))).ok_or_else(|| invalid(format!("{key} is an unsigned integer")))
        }
    }
}

/// Whether a query is sorted oldest first: by `receivedAt`, and by nothing else.
fn sort_direction(args: &Map<String, Value>) -> Outcome<bool> {
    let unsupported = || refuse("unsupportedSort", "only receivedAt can be sorted by");
    match args.get("sort") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Array(comparators)) => {
            let mut ascending = None;
            for comparator in comparators {
                let Some(comparator) = comparator.as_object() else {
                    return Err(unsupported());
                };
                if comparator.get("property").and_then(Value::as_str) != Some("receivedAt")
                    || comparator.keys().any(|k| k != "property" && k != "isAscending")
                {
                    return Err(unsupported());
                }
                let up = match comparator.get("isAscending") {
                    None => true,
                    Some(Value::Bool(up)) => *up,
                    Some(_) => return Err(invalid("isAscending is a boolean")),
                };
                ascending.get_or_insert(up);
            }
            Ok(ascending.unwrap_or(true))
        }
        Some(_) => Err(unsupported()),
    }
}

fn email_json(email: &Email, properties: &[String]) -> Value {
    let text = String::from_utf8_lossy(&email.raw);
    let (fields, body) = split_message(&text);
    // What a store of the standard kind parses out of the header, read by the program's own reader.
    let parsed = properties.iter().any(|p| EMAIL_PARSED.contains(&p.as_str())).then(|| mailcraft_mime::parse(&email.raw));
    let truths = |set: &BTreeSet<String>| Value::Object(set.iter().map(|k| (k.clone(), Value::Bool(true))).collect());
    let mut out = Map::new();
    out.insert("id".into(), json!(email.id));
    for property in properties {
        let value = match property.as_str() {
            "blobId" => json!(email.blob),
            "mailboxIds" => truths(&email.mailboxes),
            "keywords" => truths(&email.keywords),
            "size" => json!(email.raw.len()),
            "receivedAt" => json!(email.received),
            "hasAttachment" => json!(text.to_ascii_lowercase().contains("content-disposition: attachment")),
            "preview" => json!(body.split_whitespace().collect::<Vec<&str>>().join(" ").chars().take(MAX_PREVIEW).collect::<String>()),
            "headers" => Value::Array(fields.iter().map(|(name, value)| json!({"name": name, "value": value})).collect()),
            "threadId" => json!(email.id),
            parsed_property if EMAIL_PARSED.contains(&parsed_property) => parsed.as_ref().map_or(Value::Null, |m| parsed_field(parsed_property, m)),
            _ => continue,
        };
        out.insert(property.clone(), value);
    }
    Value::Object(out)
}

/// A field of the header as RFC 8621 §4.1.2 shows it: `null` when the message has none.
fn parsed_field(property: &str, message: &Message) -> Value {
    let people = |list: &[Address]| -> Value {
        if list.is_empty() {
            return Value::Null;
        }
        Value::Array(list.iter().map(|a| json!({"name": if a.name.is_empty() { Value::Null } else { json!(a.name) }, "email": a.email})).collect())
    };
    match property {
        "subject" => json!(message.subject),
        "from" => people(if message.from == Address::default() { &[] } else { std::slice::from_ref(&message.from) }),
        "to" => people(&message.to),
        "cc" => people(&message.cc),
        "messageId" if message.message_id.is_empty() => Value::Null,
        "messageId" => json!([message.message_id]),
        "sentAt" if message.date == 0 => Value::Null,
        "sentAt" => json!(mailcraft_time::format_iso_utc(message.date)),
        _ => Value::Null,
    }
}

/// A message as its header fields (each with its name and the raw value: all that follows the
/// colon, the folds kept, RFC 8621 §4.1.2) and its body.
fn split_message(text: &str) -> (Vec<(String, String)>, &str) {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut offset: usize = 0;
    let mut body_at = text.len();
    for line in text.split_inclusive('\n') {
        offset = offset.saturating_add(line.len());
        let content = line.trim_end_matches(['\r', '\n']);
        if content.is_empty() {
            body_at = offset;
            break;
        }
        if content.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                value.push_str("\r\n");
                value.push_str(content);
            }
        } else if let Some((name, value)) = content.split_once(':')
            && !name.is_empty()
            && !name.ends_with(char::is_whitespace)
            && fields.len() < MAX_HEADERS
        {
            fields.push((name.to_string(), value.to_string()));
        }
    }
    (fields, text.get(body_at..).unwrap_or_default())
}

/// An in-process JMAP server. Clones share it.
#[derive(Clone)]
pub struct StandIn {
    data: Arc<Mutex<Data>>,
}

impl Default for StandIn {
    fn default() -> StandIn {
        StandIn::new()
    }
}

impl StandIn {
    /// A server with no mailbox, that takes 16 calls and 500 ids, and lets in [`USER`] with [`PASSWORD`]
    /// and the bearer token [`TOKEN`].
    pub fn new() -> StandIn {
        StandIn { data: Arc::new(Mutex::new(Data::new())) }
    }

    /// Look at what it holds, or change it, for as long as `f` runs (hold nothing across a request).
    pub fn with<R>(&self, f: impl FnOnce(&mut Data) -> R) -> R {
        let mut data = self.data.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut data)
    }

    /// The requests it was sent so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.with(|d| d.seen.clone())
    }

    /// The methods it was asked to call so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.with(|d| d.seen.iter().flat_map(|s| s.calls.iter().cloned()).collect())
    }

    /// What those calls asked for, in order (see [`Asked`]).
    pub fn asked(&self) -> Vec<Asked> {
        self.with(|d| d.seen.iter().flat_map(|s| s.asked.iter().cloned()).collect())
    }

    /// How many requests to the API it was sent so far (the session is no request to the API).
    pub fn api_requests(&self) -> usize {
        self.with(|d| d.seen.iter().filter(|s| s.path == API_PATH).count())
    }

    /// Answer an HTTP request given by its parts (its method, its address, its header fields and its body), as
    /// any client may send one: for the tests of the stand-in itself. What a [`Connection`](crate::Connection)
    /// sends comes through [`HttpExchange`].
    pub fn answer(&self, method: &str, url: &str, fields: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, TransportError> {
        self.with(|d| d.handle(method, url, fields, body))
    }
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with(|d| f.debug_struct("StandIn").field("mailboxes", &d.mailboxes.len()).field("emails", &d.emails.len()).finish_non_exhaustive())
    }
}

impl HttpExchange for StandIn {
    async fn exchange(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let fields: Vec<(&str, &str)> = req.fields().collect();
        let answer = self.answer(req.method().as_str(), req.url(), &fields, req.body())?;
        // As an exchange does: an answer over the most the request may be answered with is no answer.
        if answer.body().len() > req.max_response_bytes() {
            return Err(TransportError::BodyTooLarge);
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests;
