// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An in-process Zimbra server for tests: strict about what it is sent, small in what it does.
//!
//! It answers `AuthRequest`, `GetFolderRequest`, `SyncRequest`, `SearchRequest` (`inid:"<folder>"`
//! and `is:anywhere item:{…}` — `in:` takes a folder's path, and `anywhere` is none), `GetMsgRequest`,
//! and the REST address of a whole message, in JSON as Zimbra writes it. A request
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
    /// Items a delta still names that no search returns — deleted between the two.
    pub vanished: std::collections::BTreeSet<u32>,
    /// Items a search silently leaves out but still has — an index a change is behind on. Asked
    /// for by name, one comes back whole.
    pub behind: std::collections::BTreeSet<u32>,
    /// Every request that came, as its path and SOAP operation (no secret is kept).
    pub log: Vec<String>,
    next: u32,
}

/// The stand-in server.
pub struct StandIn {
    pub state: RefCell<State>,
    /// Things that happen while the next delta is being answered, as another program would do
    /// them: for a folder made while a run is already under way.
    pub during: RefCell<Vec<Box<dyn FnOnce(&mut State)>>>,
    /// A broken server that answers every folder page with `more` and ignores the cursor it was
    /// given: the same page comes back each time, for ever.
    pub stuck: std::cell::Cell<bool>,
}

impl Default for StandIn {
    /// A default mailbox is a standard one — [`StandIn::new`], not the empty map a derived
    /// `Default` would give (a folder id `1` there would be no Inbox).
    fn default() -> Self {
        Self::new()
    }
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
        StandIn { state: RefCell::new(state), during: RefCell::new(Vec::new()), stuck: std::cell::Cell::new(false) }
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

    /// The item is simply gone — as a deletion between the delta that named it and the read, too
    /// late for the delta's own `deleted` list: the delta names it still, no search finds it.
    pub fn vanish(&self, id: u32) {
        self.state.borrow_mut().vanished.insert(id);
    }

    /// A delta still names the message, but a search for it comes back empty: it went between the
    /// two answers.
    pub fn hide(&self, id: u32) {
        self.state.borrow_mut().hidden.insert(id);
    }

    /// The delta names the message and a search for it says nothing — no fault, just no hit, as
    /// an index behind leaves it out. Asked for by name it answers whole.
    pub fn behind(&self, id: u32) {
        self.state.borrow_mut().behind.insert(id);
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
            ("GetMsgRequest", "urn:zimbraMail") => self.get_msg(body),
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
        // Things that happen while this delta is being put together — one a delta, so that two
        // of them land on different pages of a long one.
        let mut s = s;
        let next = {
            let mut during = self.during.borrow_mut();
            if during.is_empty() { None } else { Some(during.remove(0)) }
        };
        if let Some(act) = next {
            act(&mut s);
        }
        if since < s.oldest {
            return fault("mail.MUST_RESYNC");
        }
        let limit = body.get("changeLimit").and_then(Value::as_u64).unwrap_or(500) as usize;
        let mut changed: Vec<(u32, u64)> =
            s.touched.iter().filter(|(id, at)| **at > since && s.items.contains_key(id)).map(|(id, at)| (*id, *at)).collect();
        changed.sort_by_key(|(_, at)| *at);
        let delete_limit = body.get("deleteLimit").and_then(Value::as_u64).unwrap_or(1000) as usize;
        let mut deleted: Vec<_> = s.deleted.iter().filter(|(_, at)| *at > since).copied().collect();
        deleted.sort_by_key(|(_, at)| *at);
        let boundary = changed.get(limit).map(|(_, at)| *at).into_iter().chain(deleted.get(delete_limit).map(|(_, at)| *at)).min();
        let reached = boundary.map_or(s.change, |at| at.saturating_sub(1));
        let more = boundary.is_some();
        changed.retain(|(_, at)| *at <= reached);
        let deleted: Vec<String> = deleted.iter().filter(|(_, at)| *at <= reached).map(|(id, _)| id.to_string()).collect();
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
            s.items.iter().filter(|(id, i)| i.folder == folder && !s.vanished.contains(id)).collect()
        } else if query.contains("item:{") {
            // As the real server: `in:` takes a folder's path and `anywhere` is no folder —
            // `is:anywhere` is the scope that names every folder. A search with no scope leaves
            // Trash and Junk out, and `item:{id}` alone is no word that the item is gone.
            if query.starts_with("in:anywhere") {
                return fault("mail.NO_SUCH_FOLDER");
            }
            let (anywhere, ids) = match query.strip_prefix("is:anywhere item:{").or_else(|| query.strip_prefix("item:{")) {
                Some(rest) => (query.starts_with("is:anywhere"), rest.strip_suffix('}')),
                None => (false, None),
            };
            let Some(ids) = ids else { return fault("service.INVALID_REQUEST") };
            let ids: Vec<u32> = ids.split(',').filter_map(|id| id.parse().ok()).collect();
            if anywhere {
                // Scoped over every folder, an id it no longer has faults the whole request,
                // while one it can't see (a hidden item stands for an index behind) faults too.
                if ids.iter().any(|id| !s.items.contains_key(id) || s.hidden.contains(id) || s.vanished.contains(id)) {
                    return fault("mail.NO_SUCH_MSG");
                }
                // One the index is behind on is no fault and no hit — the answer just leaves it
                // out, the way a real server's quiet drop comes back.
                s.items.iter().filter(|(id, _)| ids.contains(id) && !s.behind.contains(id)).collect()
            } else {
                // No scope: Trash and Junk are out, and an id that isn't found is simply no
                // answer — the empty page is never the word that the item is gone.
                s.items
                    .iter()
                    .filter(|(id, i)| ids.contains(id) && i.folder != 3 && i.folder != 4 && !s.hidden.contains(id) && !s.vanished.contains(id))
                    .collect()
            }
        } else {
            return fault("service.INVALID_REQUEST");
        };
        found.sort_by_key(|(_, item)| std::cmp::Reverse(item.date));
        // A cursor continues after the item it names; when that item is gone Zimbra goes on from
        // its sort value instead (the list is dateDesc). A stuck server gives the first page again.
        let offset = match body.get("cursor") {
            Some(_) if self.stuck.get() => offset,
            Some(cursor) => {
                let at = cursor.get("id").and_then(Value::as_str).and_then(|id| id.parse::<u32>().ok());
                match at.and_then(|at| found.iter().position(|(id, _)| **id == at)) {
                    Some(at) => at.saturating_add(1),
                    None => match cursor.get("sortVal").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok()) {
                        Some(sort) => found.iter().position(|(_, i)| i.date < sort).unwrap_or(found.len()),
                        None => return fault("service.INVALID_REQUEST"),
                    },
                }
            }
            None => offset,
        };
        let more = self.stuck.get() || found.len() > offset.saturating_add(limit);
        let m: Vec<Value> = found
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(id, i)| {
                json!({"id": id.to_string(), "l": i.folder.to_string(), "f": i.flags, "tn": i.tags.join(","), "d": i.date, "sf": i.date.to_string(), "s": i.mime.len(),
                    "su": i.subject, "fr": format!("Hello from {id}."), "mid": format!("<{id}@example.org>"),
                    "e": [{"t": "f", "a": i.from, "p": "Bob"}, {"t": "t", "a": USER}]})
            })
            .collect();
        soap("SearchResponse", json!({"more": more, "offset": offset, "m": m}))
    }

    /// One item asked for by name, wherever it lies: gone is `mail.NO_SUCH_MSG`, the server's own
    /// word for it, while one it can't see faults another way (the index behind is no deletion).
    fn get_msg(&self, body: &Value) -> HttpResponse {
        let s = self.state.borrow();
        let Some(id) = body.pointer("/m/id").and_then(Value::as_str).and_then(|id| id.parse::<u32>().ok()) else {
            return fault("service.INVALID_REQUEST");
        };
        if s.hidden.contains(&id) {
            return fault("service.FAILURE");
        }
        let Some(i) = s.items.get(&id).filter(|_| !s.vanished.contains(&id)) else {
            return fault("mail.NO_SUCH_MSG");
        };
        soap(
            "GetMsgResponse",
            json!({"m": [{"id": id.to_string(), "l": i.folder.to_string(), "f": i.flags, "tn": i.tags.join(","), "d": i.date, "sf": i.date.to_string(), "s": i.mime.len(),
                "su": i.subject, "fr": format!("Hello from {id}."), "mid": format!("<{id}@example.org>"),
                "e": [{"t": "f", "a": i.from, "p": "Bob"}, {"t": "t", "a": USER}]}]}),
        )
    }

    fn rest(&self, request: &HttpRequest, query: &str) -> HttpResponse {
        let cookie = format!("ZM_AUTH_TOKEN={TOKEN}");
        if request.method() != Method::Get || request.field("Cookie") != Some(cookie.as_str()) {
            return HttpResponse::new(401, Vec::new());
        }
        let id = query.split('&').find_map(|pair| pair.strip_prefix("id=")).and_then(|id| id.parse::<u32>().ok());
        let s = self.state.borrow();
        match id.and_then(|id| s.items.get(&id).filter(|_| !s.vanished.contains(&id)).map(|i| i.mime.clone())) {
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
        let response = self.answer(request);
        // What a real transport does with an answer longer than the request allows: the crate
        // counts on it, so a test stand-in can't look more generous.
        ready(if response.body().len() > request.max_response_bytes() { Err(TransportError::BodyTooLarge) } else { Ok(response) })
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
