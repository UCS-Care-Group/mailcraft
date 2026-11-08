// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for Gmail's API, for the tests: nothing here leaves this machine, and nothing here
//! is in a build that isn't a test's.
//!
//! It answers the one call MailCraft makes (`POST /gmail/v1/users/me/messages/{id}/modify`) as
//! Google's reference has it, and holds a client to it: `Authorization: Bearer` with the token it
//! was set up with (401 otherwise), a JSON body (400 otherwise) of `addLabelIds` and
//! `removeLabelIds` holding only label ids it knows, an id in hex that names a message it has
//! (404 otherwise). It keeps each message's labels, so a test sees what a move did, and every
//! request it was sent, so a test sees what went, and that nothing went. [`State::faults`] has it
//! answer the next requests with a status of the test's choosing (401, 403, 429, 500…), and
//! [`State::echo`] has a refusal repeat what it was sent, as a careless server might.
//!
//! [`route`] has the calls of this crate that go to Gmail's API go to a stand-in instead, on the
//! thread that asks for it and until the guard is dropped.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

/// The token a stand-in takes unless it is told another.
pub const TOKEN: &str = "standin.google-acc-ess_tok.en-9876543210";
/// Largest request it reads.
const MAX_REQUEST: usize = 64 * 1024;
/// Most requests kept for a test to look at.
const MAX_KEPT: usize = 10_000;
/// The labels of Gmail's categories.
const CATEGORIES: [&str; 5] = ["CATEGORY_PERSONAL", "CATEGORY_SOCIAL", "CATEGORY_PROMOTIONS", "CATEGORY_UPDATES", "CATEGORY_FORUMS"];

thread_local! {
    static ROUTE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Where this thread's calls to Gmail's API go instead, when a test has said so ([`route`]).
pub(crate) fn routed() -> Option<String> {
    ROUTE.with(|route| route.borrow().clone())
}

/// A stand-in is being spoken to in Gmail's API's place on this thread, for as long as this lives.
pub struct Routed(Option<String>);

/// Have this thread's calls to Gmail's API go to `base` (`http://127.0.0.1:{port}`) until the
/// guard is dropped.
pub fn route(base: &str) -> Routed {
    Routed(ROUTE.with(|route| route.borrow_mut().replace(base.to_string())))
}

impl Drop for Routed {
    fn drop(&mut self) {
        let before = self.0.take();
        ROUTE.with(|route| *route.borrow_mut() = before);
    }
}

/// One request the stand-in was sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub method: String,
    /// The path as it came.
    pub path: String,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// What it was answered.
    pub status: u16,
}

impl Asked {
    /// The value of header `name` (lower case); empty when it has none.
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(n, _)| n == name).map_or("", |(_, v)| v.as_str())
    }
}

/// What the stand-in keeps.
#[derive(Debug, Default)]
pub struct State {
    /// The token it takes.
    pub token: String,
    /// Each message it has, by Gmail's id of it, with its labels.
    pub messages: BTreeMap<u64, Vec<String>>,
    /// Every request it was sent, the latest [`MAX_KEPT`].
    pub requests: Vec<Asked>,
    /// The statuses the next requests are answered with instead, one each, in order.
    pub faults: VecDeque<u16>,
    /// A refusal repeats the request's `Authorization` header and body.
    pub echo: bool,
}

impl State {
    /// Message `id` with `labels`.
    pub fn add(&mut self, id: u64, labels: &[&str]) {
        self.messages.insert(id, labels.iter().map(|l| l.to_string()).collect());
    }

    /// The category labels message `id` has.
    pub fn categories(&self, id: u64) -> Vec<String> {
        self.messages.get(&id).map(|labels| labels.iter().filter(|l| CATEGORIES.contains(&l.as_str())).cloned().collect()).unwrap_or_default()
    }

    fn answer(&mut self, asked: &Asked) -> (u16, Value) {
        let refuse = |status: u16, why: &str| (status, json!({"error": {"code": status, "message": why, "status": "FAILED"}}));
        if let Some(status) = self.faults.pop_front() {
            return refuse(status, "As the test asked.");
        }
        if asked.header("authorization") != format!("Bearer {}", self.token) {
            return refuse(401, "Request had invalid authentication credentials.");
        }
        let Some(id) = asked.path.strip_prefix("/gmail/v1/users/me/messages/").and_then(|rest| rest.strip_suffix("/modify")) else {
            return refuse(404, "Not found.");
        };
        if asked.method != "POST" {
            return refuse(405, "Method not allowed.");
        }
        if !asked.header("content-type").starts_with("application/json") {
            return refuse(400, "Invalid JSON payload received.");
        }
        let Some(id) = u64::from_str_radix(id, 16).ok().filter(|_| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit())) else {
            return refuse(400, "Invalid id value");
        };
        let Ok(body) = serde_json::from_str::<Value>(&asked.body) else { return refuse(400, "Invalid JSON payload received.") };
        let list = |key: &str| -> Option<Vec<String>> {
            match body.get(key) {
                None => Some(Vec::new()),
                Some(Value::Array(items)) => items.iter().map(|v| v.as_str().map(str::to_string)).collect(),
                Some(_) => None,
            }
        };
        let (Some(add), Some(remove)) = (list("addLabelIds"), list("removeLabelIds")) else {
            return refuse(400, "Invalid JSON payload received.");
        };
        if body.as_object().is_none_or(|o| o.keys().any(|k| k != "addLabelIds" && k != "removeLabelIds")) {
            return refuse(400, "Invalid JSON payload received. Unknown name.");
        }
        if add.iter().chain(&remove).any(|l| !CATEGORIES.contains(&l.as_str()) && l != "INBOX" && l != "UNREAD") {
            return refuse(400, "Invalid label");
        }
        let Some(labels) = self.messages.get_mut(&id) else { return refuse(404, "Requested entity was not found.") };
        labels.retain(|l| !remove.contains(l));
        for l in add {
            if !labels.contains(&l) {
                labels.push(l);
            }
        }
        (200, json!({"id": format!("{id:x}"), "threadId": format!("{id:x}"), "labelIds": labels}))
    }
}

/// Read one request from `stream`: the request line, the headers, and as much body as it says.
fn read_request(stream: &TcpStream) -> Option<Asked> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok().filter(|n| *n > 0)?;
    let mut words = line.split_whitespace();
    let (method, path) = (words.next()?.to_string(), words.next()?.to_string());
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
    Some(Asked { method, path, headers, body: String::from_utf8_lossy(&body).into_owned(), status: 0 })
}

/// A stand-in for Gmail's API on a loopback port.
pub struct StandIn {
    pub port: u16,
    state: Arc<Mutex<State>>,
}

impl StandIn {
    /// Start one that takes [`TOKEN`] and has no messages.
    pub fn start() -> std::io::Result<StandIn> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(Mutex::new(State { token: TOKEN.into(), ..Default::default() }));
        let shared = state.clone();
        std::thread::Builder::new().name("gmail stand-in".into()).spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Some(mut asked) = read_request(&stream) else { continue };
                let mut state = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let (status, mut body) = state.answer(&asked);
                if state.echo && status >= 400 {
                    body = json!({"error": {"code": status, "message": format!("{} {}", asked.header("authorization"), asked.body)}});
                }
                asked.status = status;
                if state.requests.len() >= MAX_KEPT {
                    state.requests.drain(..MAX_KEPT / 2);
                }
                state.requests.push(asked);
                drop(state);
                let reason = match status {
                    200 => "OK",
                    400 => "Bad Request",
                    401 => "Unauthorized",
                    403 => "Forbidden",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    _ => "Other",
                };
                let body = body.to_string();
                let answer = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(answer.as_bytes());
            }
        })?;
        Ok(StandIn { port, state })
    }

    /// `http://127.0.0.1:{port}`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// What it keeps, to set up and to look at.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Every request it was sent so far.
    pub fn requests(&self) -> Vec<Asked> {
        self.state().requests.clone()
    }

    /// Have this thread's calls to Gmail's API come here until the guard is dropped ([`route`]).
    pub fn route(&self) -> Routed {
        route(&self.url())
    }
}
