// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An in-process Zimbra server for tests: strict about what it is sent, small in what it does.
//!
//! It answers `AuthRequest`, `GetFolderRequest`, `SyncRequest`, `SearchRequest` (`inid:"<folder>"`
//! and `in:anywhere item:{…}`) and the REST address of a whole message, in JSON as Zimbra writes it. A request
//! that isn't as the protocol has it (not a POST of a SOAP envelope in JSON, the wrong namespace, a
//! token in the address or missing from the header) is refused with a fault, so that a client that
//! strays fails its tests. Every change made through it is numbered, which is what its tokens are.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::{Future, ready};

use serde_json::{Value, json};

use crate::http::{HttpExchange, HttpRequest, HttpResponse, Method, TransportError};

/// The password the stand-in's account takes.
pub const PASSWORD: &str = "stand-in-password";
/// The name it signs in with.
pub const USER: &str = "ada@example.org";
/// The token it hands out on a sign-in.
pub const TOKEN: &str = "0_standin-token_abc";

/// A message of the stand-in's mailbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub folder: u32,
    pub flags: String,
    pub tags: Vec<String>,
    pub subject: String,
    pub from: String,
    /// When it arrived, in milliseconds.
    pub date: u64,
    pub mime: String,
}

/// What the stand-in holds.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Folders by id: (parent, name, view).
    pub folders: BTreeMap<u32, (u32, String, String)>,
    pub items: BTreeMap<u32, Item>,
    /// The number of the last change; a token is one of these.
    pub change: u64,
    /// For each item, the number of the change that last touched it.
    pub touched: BTreeMap<u32, u64>,
    /// Items deleted, with the number of the change that deleted them.
    pub deleted: Vec<(u32, u64)>,
    /// Tokens older than this are refused with `mail.MUST_RESYNC`.
    pub oldest: u64,
    /// Items a delta names but a search doesn't find (`hide`): gone between the two answers.
    pub hidden: std::collections::BTreeSet<u32>,
    /// Every request that came, as its path and SOAP operation (no secret is kept).
    pub log: Vec<String>,
    next: u32,
}

/// The stand-in server.
#[derive(Default)]
pub struct StandIn {
    pub state: RefCell<State>,
    /// Things that happen while the next delta is being answered, as another program would do
    /// them: for a folder made while a run is already under way.
    pub during: RefCell<Vec<Box<dyn FnOnce(&mut State)>>>,
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StandIn").field("state", &self.state).finish_non_exhaustive()
    }
}

impl StandIn {
    /// A mailbox with Zimbra's own folders (Inbox, Trash, Junk, Sent, Drafts, Chats, Calendar,
    /// Contacts) and nothing in them.
    pub fn new() -> StandIn {
        let mut state = State { next: 300, ..State::default() };
        for (id, name, view) in [
            (2, "Inbox", "message"),
            (3, "Trash", ""),
            (4, "Junk", "message"),
            (5, "Sent", "message"),
            (6, "Drafts", "message"),
            (7, "Contacts", "contact"),
            (10, "Calendar", "appointment"),
            (14, "Chats", "message"),
        ] {
            state.folders.insert(id, (1, name.into(), view.into()));
        }
        StandIn { state: RefCell::new(state), during: RefCell::new(Vec::new()) }
    }

    /// Make a folder; answers its id.
    pub fn add_folder(&self, parent: u32, name: &str) -> u32 {
        let mut s = self.state.borrow_mut();
        s.next += 1;
        let id = s.next;
        s.folders.insert(id, (parent, name.into(), "message".into()));
        id
    }

    /// Put a message in `folder`; answers its id.
    pub fn deliver(&self, folder: u32, subject: &str, flags: &str) -> u32 {
        let mut s = self.state.borrow_mut();
        s.next += 1;
        let id = s.next;
        let mime =
            format!("From: Bob <bob@example.org>\r\nTo: {USER}\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.org>\r\n\r\nHello from {id}.\r\n");
        let date = 1_700_000_000_000 + u64::from(id) * 1000;
        s.items
            .insert(id, Item { folder, flags: flags.into(), subject: subject.into(), from: "bob@example.org".into(), date, mime, ..Item::default() });
        s.change += 1;
        let change = s.change;
        s.touched.insert(id, change);
        id
    }

    /// Change a message as another program would.
    pub fn change(&self, id: u32, change: impl FnOnce(&mut Item)) {
        let mut s = self.state.borrow_mut();
        if let Some(item) = s.items.get_mut(&id) {
            change(item);
        }
        s.change += 1;
        let at = s.change;
        s.touched.insert(id, at);
    }

    /// Delete a message as another program would.
    pub fn delete(&self, id: u32) {
        let mut s = self.state.borrow_mut();
        s.items.remove(&id);
        s.change += 1;
        let at = s.change;
        s.deleted.push((id, at));
    }

    /// A delta still names the message, but a search for it comes back empty: it went between the
    /// two answers.
    pub fn hide(&self, id: u32) {
        self.state.borrow_mut().hidden.insert(id);
    }

    /// A folder that appears only when the next delta is answered, as if made while a run was
    /// already under way; answers the id it will have.
    pub fn add_folder_later(&self, parent: u32, name: &str) -> u32 {
        let id = {
            let mut s = self.state.borrow_mut();
            s.next += 1;
            s.next
        };
        let name = name.to_string();
        self.during.borrow_mut().push(Box::new(move |s: &mut State| {
            s.folders.insert(id, (parent, name, "message".into()));
        }));
        id
    }

    fn answer(&self, request: &HttpRequest) -> HttpResponse {
        let url = request.url().to_string();
        let Some(path) = url.split_once("://").and_then(|(_, rest)| rest.find('/').map(|at| rest.get(at..).unwrap_or("").to_string())) else {
            return HttpResponse::new(400, Vec::new());
        };
        if path.contains("zauthtoken") || path.contains(TOKEN) {
            return fault("service.INVALID_REQUEST");
        }
        if let Some(query) = path.strip_prefix("/service/home/~/?") {
            self.state.borrow_mut().log.push("REST".into());
            return self.rest(request, query);
        }
        let Some(name) = path.strip_prefix("/service/soap/") else { return HttpResponse::new(404, Vec::new()) };
        self.state.borrow_mut().log.push(name.to_string());
        if request.method() != Method::Post || !request.field("Content-Type").is_some_and(|t| t.starts_with("application/json")) {
            return fault("service.INVALID_REQUEST");
        }
        let Ok(envelope) = serde_json::from_slice::<Value>(request.body()) else { return fault("service.PARSE_ERROR") };
        let Some(body) = envelope.pointer(&format!("/Body/{name}")) else { return fault("service.INVALID_REQUEST") };
        let context = envelope.pointer("/Header/context");
        if context.and_then(|c| c.get("_jsns")).and_then(Value::as_str) != Some("urn:zimbra") {
            return fault("service.INVALID_REQUEST");
        }
        let ns = body.get("_jsns").and_then(Value::as_str).unwrap_or("");
        let signed_in = context.and_then(|c| c.pointer("/authToken/_content")).and_then(Value::as_str) == Some(TOKEN);
        match (name, ns) {
            ("AuthRequest", "urn:zimbraAccount") => {
                let user = body.pointer("/account/_content").and_then(Value::as_str);
                let password = body.pointer("/password/_content").and_then(Value::as_str);
                if user == Some(USER) && password == Some(PASSWORD) {
                    soap("AuthResponse", json!({"authToken": [{"_content": TOKEN}], "lifetime": 172_800_000}))
                } else {
                    fault("account.AUTH_FAILED")
                }
            }
            (_, "urn:zimbraMail") if !signed_in => fault("service.AUTH_REQUIRED"),
            ("GetFolderRequest", "urn:zimbraMail") => soap("GetFolderResponse", json!({"folder": [self.tree(1)]})),
            ("SyncRequest", "urn:zimbraMail") => self.sync(body),
            ("SearchRequest", "urn:zimbraMail") => self.search(body),
            _ => fault("service.UNKNOWN_DOCUMENT"),
        }
    }

    fn tree(&self, id: u32) -> Value {
        let s = self.state.borrow();
        let (name, view) = s.folders.get(&id).map_or(("USER_ROOT".to_string(), String::new()), |(_, name, view)| (name.clone(), view.clone()));
        let count = s.items.values().filter(|i| i.folder == id).count();
        let unread = s.items.values().filter(|i| i.folder == id && i.flags.contains('u')).count();
        let children: Vec<u32> = s.folders.iter().filter(|(_, (parent, _, _))| *parent == id).map(|(id, _)| *id).collect();
        drop(s);
        let inner: Vec<Value> = children.into_iter().map(|child| self.tree(child)).collect();
        let mut folder = json!({"id": id.to_string(), "name": name, "n": count, "u": unread, "folder": inner});
        if !view.is_empty()
            && let Value::Object(map) = &mut folder
        {
            map.insert("view".into(), json!(view));
        }
        folder
    }

    fn sync(&self, body: &Value) -> HttpResponse {
        let s = self.state.borrow_mut();
        let Some(since) = body.get("token").and_then(Value::as_str) else {
            return soap("SyncResponse", json!({"token": s.change.to_string(), "md": 1, "folder": [{"id": "1"}]}));
        };
        let Ok(since) = since.parse::<u64>() else { return fault("service.INVALID_REQUEST") };
        // Things that happen while this delta is being put together.
        let mut s = s;
        for act in self.during.borrow_mut().drain(..) {
            act(&mut s);
        }
        if since < s.oldest {
            return fault("mail.MUST_RESYNC");
        }
        let limit = body.get("changeLimit").and_then(Value::as_u64).unwrap_or(500) as usize;
        let mut changed: Vec<(u32, u64)> =
            s.touched.iter().filter(|(id, at)| **at > since && s.items.contains_key(id)).map(|(id, at)| (*id, *at)).collect();
        changed.sort_by_key(|(_, at)| *at);
        let more = changed.len() > limit;
        changed.truncate(limit);
        let reached = changed.last().map_or(s.change, |(_, at)| if more { *at } else { s.change });
        let deleted: Vec<String> = s.deleted.iter().filter(|(_, at)| *at > since && *at <= reached).map(|(id, _)| id.to_string()).collect();
        let m: Vec<Value> = changed
            .iter()
            .filter_map(|(id, _)| s.items.get(id).map(|i| json!({"id": id.to_string(), "l": i.folder.to_string(), "f": i.flags})))
            .collect();
        let mut answer = json!({"token": reached.to_string(), "md": 1, "m": m});
        if let Value::Object(map) = &mut answer {
            if more {
                map.insert("more".into(), json!(true));
            }
            if !deleted.is_empty() {
                map.insert("deleted".into(), json!([{"ids": deleted.join(","), "m": [{"ids": deleted.join(",")}]}]));
            }
        }
        soap("SyncResponse", answer)
    }

    fn search(&self, body: &Value) -> HttpResponse {
        let s = self.state.borrow();
        if body.get("types").and_then(Value::as_str) != Some("message") {
            return fault("service.INVALID_REQUEST");
        }
        let query = body.get("query").and_then(Value::as_str).unwrap_or("");
        let limit = body.get("limit").and_then(Value::as_u64).unwrap_or(10).min(1000) as usize;
        let offset = body.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let mut found: Vec<(&u32, &Item)> = if let Some(folder) = query.strip_prefix("inid:\"").and_then(|q| q.strip_suffix('"')) {
            let Ok(folder) = folder.parse::<u32>() else { return fault("service.INVALID_REQUEST") };
            s.items.iter().filter(|(_, i)| i.folder == folder).collect()
        } else if let Some(ids) = query.strip_prefix("in:anywhere item:{").and_then(|q| q.strip_suffix('}')) {
            let ids: Vec<u32> = ids.split(',').filter_map(|id| id.parse().ok()).collect();
            s.items.iter().filter(|(id, _)| ids.contains(id) && !s.hidden.contains(id)).collect()
        } else {
            return fault("service.INVALID_REQUEST");
        };
        found.sort_by_key(|(_, item)| std::cmp::Reverse(item.date));
        // A cursor continues after the item it names.
        let offset = match body.get("cursor") {
            Some(cursor) => {
                let at = cursor.get("id").and_then(Value::as_str).and_then(|id| id.parse::<u32>().ok());
                match at.and_then(|at| found.iter().position(|(id, _)| **id == at)) {
                    Some(at) => at.saturating_add(1),
                    None => return fault("service.INVALID_REQUEST"),
                }
            }
            None => offset,
        };
        let more = found.len() > offset.saturating_add(limit);
        let m: Vec<Value> = found
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(id, i)| {
                json!({"id": id.to_string(), "l": i.folder.to_string(), "f": i.flags, "tn": i.tags.join(","), "d": i.date, "s": i.mime.len(),
                    "su": i.subject, "fr": format!("Hello from {id}."), "mid": format!("<{id}@example.org>"),
                    "e": [{"t": "f", "a": i.from, "p": "Bob"}, {"t": "t", "a": USER}]})
            })
            .collect();
        soap("SearchResponse", json!({"more": more, "offset": offset, "m": m}))
    }

    fn rest(&self, request: &HttpRequest, query: &str) -> HttpResponse {
        let cookie = format!("ZM_AUTH_TOKEN={TOKEN}");
        if request.method() != Method::Get || request.field("Cookie") != Some(cookie.as_str()) {
            return HttpResponse::new(401, Vec::new());
        }
        let id = query.split('&').find_map(|pair| pair.strip_prefix("id=")).and_then(|id| id.parse::<u32>().ok());
        match id.and_then(|id| self.state.borrow().items.get(&id).map(|i| i.mime.clone())) {
            Some(mime) => HttpResponse::new(200, mime.into_bytes()),
            None => HttpResponse::new(404, Vec::new()),
        }
    }
}

fn soap(name: &str, response: Value) -> HttpResponse {
    let mut body = serde_json::Map::new();
    body.insert(name.into(), response);
    HttpResponse::new(
        200,
        json!({"Header": {"context": {"_jsns": "urn:zimbra"}}, "Body": Value::Object(body), "_jsns": "urn:zimbraSoap"}).to_string().into_bytes(),
    )
}

fn fault(code: &str) -> HttpResponse {
    let body = json!({"Body": {"Fault": {"Code": {"Value": "soap:Sender"}, "Reason": {"Text": format!("stand-in: {code} (password {PASSWORD})")}, "Detail": {"Error": {"Code": code}}}}});
    HttpResponse::new(500, body.to_string().into_bytes())
}

impl HttpExchange for StandIn {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        ready(Ok(self.answer(request)))
    }
}

/// Runs a future to completion by polling it with a waker that does nothing. The crate's futures
/// only wait on the exchange, and a stand-in is always ready, so a few polls suffice. It gives up
/// (with `None`) rather than wait for ever.
pub fn block_on<F: Future>(future: F) -> Option<F::Output> {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return Some(output);
        }
    }
    None
}
