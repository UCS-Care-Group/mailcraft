//! An account's mail through Microsoft Graph, for an account whose owner chose that way
//! ([`MailSource::Graph`](mailcraft_model::MailSource::Graph)): the same run as over IMAP and
//! SMTP, said to the engine in the same words ([`SyncResult`](mailcraft_model::sync::SyncResult)),
//! with Graph's REST interface in place of the mail protocols. The token is a Graph one
//! (`Mail.ReadWrite` and `Mail.Send`).
//!
//! A run goes like this: the Outbox is sent (`POST /me/sendMail`, which files a copy in Sent
//! Items); the folders of mail are listed (`/me/mailFolders` and their `childFolders`, the special
//! ones known by their well-known names); folders made, renamed, moved and deleted here are
//! changed there ([`push::folder_ops`]); drafts written here go up, and the changes made to
//! messages here are made there ([`push`]); then each folder is read, the Inbox first, for what
//! changed since it was last read through: Graph's delta query of the folder's messages, which
//! names the messages that came, changed and went since the address it gave the last time
//! ([`Folder::delta`](mailcraft_model::Folder::delta)). New messages come down whole as Internet
//! messages (`/me/messages/{id}/$value`), twenty to a `$batch`, newest first.
//!
//! Graph knows a message by an id (an immutable one: `Prefer: IdType="ImmutableId"` goes with
//! every request, so a move doesn't change it), the engine by a number in its folder. The number
//! is made here: one more than the highest the folder has ([`FolderState::uid_seen`] and the
//! numbers it knows), for each message that comes; the folder's `UIDVALIDITY` is always 1. The
//! two go together in [`Message::remote_id`](mailcraft_model::Message::remote_id) and
//! [`FolderState::remote_ids`]. A message moved by a run is said moved without a number in its
//! new folder: its copy there is matched to it by its Message-ID when that folder is read.
//!
//! The address of the next round is kept only once every new message of the folder came down:
//! a run that stops early, or that a limit cut short, leaves the old one, and the next run asks
//! again from there (the messages that came meanwhile are known, and only looked at).
//!
//! A shared mailbox the owner opens ([`Account::shared_of`](mailcraft_model::Account::shared_of))
//! has the same run, with the owner's token: every address of it says `/users/{address}/…` in
//! the place of `/me/…`, inside a `$batch` too ([`Graph::for_mailbox`]), which takes the
//! permissions `Mail.ReadWrite.Shared` and `Mail.Send.Shared` beside the owner's own and Full
//! Access to the mailbox. Its Outbox goes out as the mailbox itself (`/users/{address}/sendMail`,
//! which files the copy in its own Sent Items; [`super::send_from_mailbox`]), and, for a sign-in
//! that doesn't cover that yet, through the owner's own sending with the mailbox as the sender
//! ([`super::send_as`]), where the copy is the owner's.
//!
//! Only `https://graph.microsoft.com` is spoken to: a next page or a next round that names
//! another server is not followed. What comes back is hostile JSON, every list and text in it
//! capped, every message parsed as hostile mail. The log says how many messages came and went,
//! never an id, a name, a subject or a token.

mod folders;
mod push;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mailcraft_model::sync::{
    FolderDelta, FolderMark, FolderState, MAX_KEYWORD_CATEGORY, MAX_MESSAGE_KEYWORDS, RemoteCategories, RemoteFlags, SyncScope,
};
use mailcraft_model::{Body, FlagStatus, FolderKind, Message};
use mailcraft_proto::ProtoError;
use serde_json::{Value, json};

use super::{Doing, Graph, code_of, graph_base, instant, nonsense, wait_of};
use crate::NetError;
use crate::http::{self, MAX_LARGE_BODY, MAX_LONG_URL, Reply, Url};
use crate::sync::{NOT_DOWNLOADED, Run, label, megabytes, messages, shown};

use folders::{Node, Tree};

/// What goes with every request about mail: ids that a move doesn't change.
const PREFER: &str = "IdType=\"ImmutableId\"";
/// And with a list, how long its pages are asked to be.
const PREFER_PAGES: &str = "IdType=\"ImmutableId\", odata.maxpagesize=200";
/// What is asked of each message in a round: no more than is kept.
const DELTA_FIELDS: &str = "id,isRead,flag,categories,internetMessageId,receivedDateTime,isDraft";
/// Graph's own way of saying that a round's address is no good any more (it may also answer 410).
const STALE_CODES: [&str; 4] = ["syncStateNotFound", "SyncStateNotFound", "syncStateInvalid", "resyncRequired"];
/// Most messages of one folder looked at in one round, and most pages; and how long a round,
/// or the listing of the folders, may take.
const MAX_FOLDER_MESSAGES: usize = 200_000;
const MAX_ROUND_PAGES: usize = 5_000;
const ROUND_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Most bytes of one page, and of any answer in JSON that isn't a batch of messages.
const MAX_PAGE: usize = 4 * 1024 * 1024;
/// Shortest and longest text taken for an id.
const MAX_ID: usize = 512;
/// How often a request that is told "not now" is asked, and the longest it is waited for.
const TRIES: usize = 3;
const MAX_PAUSE: Duration = Duration::from_secs(10);
/// How long a request may take; one that brings a message, or many, takes longer.
const ASK: Duration = Duration::from_secs(30);
const FETCH: Duration = Duration::from_secs(120);
/// Most header fields read of a message too large to download, and the longest of them.
const MAX_HEADERS: usize = 200;
const MAX_HEADER: usize = 8 * 1024;

/// Graph for the mail of one run: where it is, the token, and the flag that calls the run off.
pub(crate) struct Client<'a> {
    graph: Graph<'a>,
    cancel: &'a AtomicBool,
}

/// Whether `error` is trouble of the connection that a second try may not have: one that couldn't
/// be made or a handshake that didn't finish (nothing went to Graph); and, for a request that
/// only reads (`reads`), an answer that stalled or a connection that broke on the way. A change
/// that may have reached Graph is never sent twice. Not a refused sign-in, an answer that makes
/// no sense, or one too long.
fn passing(error: &NetError, reads: bool) -> bool {
    match error {
        NetError::Connect { .. } | NetError::Tls { .. } => true,
        NetError::Proto(ProtoError::Io(_)) => reads && !http::is_too_long(error),
        _ => false,
    }
}

/// Why a list stopped before its end.
pub(crate) enum Stop {
    /// The round's address is no good any more: begin again without it.
    Stale,
    Net(NetError),
}

impl From<NetError> for Stop {
    fn from(e: NetError) -> Stop {
        Stop::Net(e)
    }
}

impl Stop {
    fn into_error(self) -> NetError {
        match self {
            Stop::Stale => nonsense("Microsoft Graph no longer knows where the last look at the folder ended"),
            Stop::Net(e) => e,
        }
    }
}

/// How a list that was read ended.
pub(crate) struct Listed {
    /// Read to its end (nothing was left out for a cap).
    pub(crate) whole: bool,
    /// The address of the next round, when the list was a round and said one.
    pub(crate) delta: Option<String>,
}

/// The short name Graph gives a refusal, without anything around it.
fn error_code(body: &[u8]) -> String {
    code_of(body, &[]).trim().trim_start_matches('(').trim_end_matches(')').to_string()
}

/// Whether an error is one after which nothing more is tried in the run: the sign-in is no good,
/// Graph can't be reached, or it says "not now" even after waiting.
pub(crate) fn ends_run(e: &NetError) -> bool {
    e.is_refused_sign_in() || e.is_unreachable() || matches!(e, NetError::Busy { .. } | NetError::Connect { .. } | NetError::Tls { .. })
}

/// Text that is an id Graph gave: visible ASCII, not too long.
pub(crate) fn id_of(value: Option<&Value>) -> Option<String> {
    let id = value?.as_str()?;
    (!id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_graphic())).then(|| id.to_string())
}

impl<'a> Client<'a> {
    /// Graph for the mail of `mailbox` (an address; empty: the signed-in owner's own), with `token`.
    pub(crate) fn new(base: &str, token: &'a str, mailbox: &str, cancel: &'a AtomicBool) -> Result<Client<'a>, NetError> {
        Ok(Client { graph: Graph::at(base, token)?.for_mailbox(mailbox)?.cancelled_by(cancel), cancel })
    }

    /// The address of the mailbox spoken of, when it isn't the owner's own (a shared mailbox).
    pub(crate) fn mailbox(&self) -> Option<&str> {
        self.graph.user.as_deref()
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        self.graph.url(path)
    }

    /// Send one message as the mailbox spoken of: the owner's own, or a shared one as itself
    /// ([`super::send_from_mailbox`]).
    pub(crate) fn send(&self, message: &Message) -> Result<(), NetError> {
        super::send_with(&self.graph, message, &super::SendOptions::default())
    }

    /// An address Graph named (a next page, a next round): one on Graph itself, or `None`.
    pub(crate) fn named(&self, link: &str) -> Option<Url> {
        let url = Url::parse_within(link, MAX_LONG_URL).ok()?;
        self.graph.base.same_server(&url).then_some(url)
    }

    /// Wait `pause`, or less when the run is called off meanwhile.
    fn wait(&self, pause: Duration) {
        let until = Instant::now() + pause;
        while Instant::now() < until && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50).min(until.saturating_duration_since(Instant::now())));
        }
    }

    /// One request, asked again after a while when Graph says "not now" (a few times, and never
    /// for longer than [`MAX_PAUSE`] each). The answer comes back whatever its status.
    pub(crate) fn call(
        &self,
        method: &str,
        url: &Url,
        body: Option<(&str, &str)>,
        prefer: &str,
        patience: Duration,
        max_body: usize,
    ) -> Result<Reply, NetError> {
        let mut headers: Vec<(&str, &str)> = vec![("Prefer", prefer)];
        if let Some((kind, _)) = body {
            headers.push(("Content-Type", kind));
        }
        let text = body.map(|(_, text)| text);
        let mut tries = 0;
        loop {
            tries += 1;
            // A connection that couldn't be made, or that stalled or broke on the way (a
            // handshake that didn't finish in its time, say), is tried again after a moment:
            // one bad connection among the many of a long run doesn't end a folder's read.
            let reply = match self.graph.ask(url, method, &headers, text, patience, max_body) {
                Ok(reply) => reply,
                Err(e) if passing(&e, method == "GET" || url.path.ends_with("/$batch")) && tries < TRIES && !self.cancel.load(Ordering::Relaxed) => {
                    log::debug!("Microsoft Graph couldn't be reached this time ({e}); asking again");
                    self.wait(Duration::from_secs(2));
                    continue;
                }
                Err(e) => return Err(e),
            };
            if !matches!(reply.status, 429 | 503 | 504) || tries >= TRIES || self.cancel.load(Ordering::Relaxed) {
                return Ok(reply);
            }
            let seconds = wait_of(reply.retry_after.as_deref(), mailcraft_time::now()).unwrap_or(2);
            self.wait(Duration::from_secs(u64::from(seconds)).min(MAX_PAUSE));
        }
    }

    /// The error for an answer that isn't the one hoped for.
    pub(crate) fn refusal(&self, reply: &Reply, sent: &str) -> NetError {
        self.graph.refusal(Doing::Mail, reply, sent)
    }

    /// GET `path` and read the JSON object it answers with (status 200).
    pub(crate) fn get_json(&self, path: &str) -> Result<Value, NetError> {
        let reply = self.call("GET", &self.url(path), None, PREFER, ASK, MAX_PAGE)?;
        if reply.status != 200 {
            return Err(self.refusal(&reply, ""));
        }
        object(&reply.body)
    }

    /// Send `body` (JSON) with `method` to `path`; the answer, whatever its status.
    pub(crate) fn send_json(&self, method: &str, path: &str, body: &Value) -> Result<Reply, NetError> {
        let text = body.to_string();
        self.call(method, &self.url(path), Some(("application/json", &text)), PREFER, ASK, MAX_PAGE)
    }

    /// Read a list page by page from `first`, handing every entry to `take`, `most` at most.
    /// A round ends with the address of the next one, which comes back.
    pub(crate) fn pages(&self, first: Url, most: usize, take: &mut dyn FnMut(&Value)) -> Result<Listed, Stop> {
        let started = Instant::now();
        let mut taken = 0usize;
        let mut url = first;
        for _ in 0..MAX_ROUND_PAGES {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Stop::Net(NetError::Proto(ProtoError::Io(crate::transport::STOPPED.into()))));
            }
            if started.elapsed() >= ROUND_TIMEOUT {
                return Err(Stop::Net(NetError::Proto(ProtoError::Io("Microsoft Graph took too long to answer".into()))));
            }
            let reply = self.call("GET", &url, None, PREFER_PAGES, ASK, MAX_PAGE)?;
            if reply.status == 410 || (matches!(reply.status, 400 | 404) && STALE_CODES.contains(&error_code(&reply.body).as_str())) {
                return Err(Stop::Stale);
            }
            if reply.status != 200 {
                return Err(Stop::Net(self.refusal(&reply, "")));
            }
            let mut page = object(&reply.body)?;
            let Some(Value::Array(entries)) = page.get_mut("value").map(Value::take) else {
                return Err(Stop::Net(nonsense("Microsoft Graph's answer has no list in it")));
            };
            for entry in &entries {
                if taken >= most {
                    return Ok(Listed { whole: false, delta: None });
                }
                taken += 1;
                take(entry);
            }
            if let Some(next) = page.get("@odata.nextLink").filter(|link| !link.is_null()) {
                let astray = || Stop::Net(nonsense("Microsoft Graph named a next page that can't be followed"));
                let next = next.as_str().and_then(|link| self.named(link)).ok_or_else(astray)?;
                if next == url {
                    return Err(astray());
                }
                url = next;
                continue;
            }
            let delta = match page.get("@odata.deltaLink") {
                None | Some(Value::Null) => None,
                Some(link) => {
                    let link = link.as_str().filter(|link| self.named(link).is_some());
                    Some(link.ok_or_else(|| Stop::Net(nonsense("Microsoft Graph named a next round that can't be followed")))?.to_string())
                }
            };
            return Ok(Listed { whole: true, delta });
        }
        Ok(Listed { whole: false, delta: None })
    }

    /// Requests in one `$batch` (twenty at most): each its method, its address below `/v1.0`
    /// and its body. The answers, in the order the requests went; `None` where Graph gave none.
    /// One that is told "not now" is asked once more, after a while.
    pub(crate) fn batch(&self, requests: &[(&str, String, Option<Value>)], max_body: usize) -> Result<Vec<Option<(u16, Value)>>, NetError> {
        let mut answers: Vec<Option<(u16, Value)>> = vec![None; requests.len()];
        let mut todo: Vec<usize> = (0..requests.len().min(super::BATCH_SIZE)).collect();
        for round in 0..2 {
            if todo.is_empty() {
                break;
            }
            let asked: Vec<Value> = todo
                .iter()
                .filter_map(|&at| {
                    let (method, url, body) = requests.get(at)?;
                    // (For the mailbox spoken of: `/users/{address}/…` in the place of `/me/…`.)
                    let mut one = json!({"id": at.to_string(), "method": method, "url": self.graph.mine(url), "headers": {"Prefer": PREFER}});
                    if let Some(body) = body {
                        one["body"] = body.clone();
                        one["headers"]["Content-Type"] = json!("application/json");
                    }
                    Some(one)
                })
                .collect();
            let text = json!({ "requests": asked }).to_string();
            let reply = self.call("POST", &self.url(super::BATCH), Some(("application/json", &text)), PREFER, FETCH, max_body)?;
            if reply.status != 200 {
                return Err(self.refusal(&reply, &text));
            }
            let body = object(&reply.body)?;
            let Some(Value::Array(responses)) = body.get("responses") else {
                return Err(nonsense("Microsoft Graph's batch answer has no answers in it"));
            };
            let mut again = Vec::new();
            let mut pause = 0u32;
            for response in responses.iter().take(super::BATCH_SIZE) {
                let Some(at) = response.get("id").and_then(Value::as_str).and_then(|id| id.parse::<usize>().ok()).filter(|at| todo.contains(at))
                else {
                    continue;
                };
                let status = response.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()).unwrap_or(0);
                if matches!(status, 429 | 503 | 504) && round == 0 {
                    let said = response.get("headers").and_then(|h| h.get("Retry-After").or_else(|| h.get("retry-after"))).and_then(Value::as_str);
                    pause = pause.max(wait_of(said, mailcraft_time::now()).unwrap_or(2));
                    again.push(at);
                    continue;
                }
                if let Some(slot) = answers.get_mut(at) {
                    *slot = Some((status, response.get("body").cloned().unwrap_or(Value::Null)));
                }
            }
            if again.is_empty() {
                break;
            }
            self.wait(Duration::from_secs(u64::from(pause)).min(MAX_PAUSE));
            todo = again;
        }
        Ok(answers)
    }
}

/// An answer that is a JSON object.
pub(crate) fn object(body: &[u8]) -> Result<Value, NetError> {
    match serde_json::from_slice::<Value>(body) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(nonsense("Microsoft Graph's answer isn't the JSON it should be")),
    }
}

/// A message as a round names it.
#[derive(Clone, Debug, Default)]
struct Entry {
    id: String,
    read: bool,
    flagged: bool,
    draft: bool,
    categories: Vec<String>,
    received: i64,
}

/// What a round says of one message: there it is, as it is now, or it went.
enum Seen {
    Here(Entry),
    Gone(String),
}

fn seen(value: &Value) -> Option<Seen> {
    let id = id_of(value.get("id"))?;
    if value.get("@removed").is_some() {
        return Some(Seen::Gone(id));
    }
    let flagged = value.get("flag").and_then(|f| f.get("flagStatus")).and_then(Value::as_str) == Some("flagged");
    Some(Seen::Here(Entry {
        id,
        read: value.get("isRead").and_then(Value::as_bool).unwrap_or(false),
        flagged,
        draft: value.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        categories: categories_in(value.get("categories")),
        received: value.get("receivedDateTime").and_then(Value::as_str).and_then(instant).unwrap_or(0),
    }))
}

/// Category names from a list Graph gave: one line each, not too long, each once, only so many.
pub(crate) fn categories_in(value: Option<&Value>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let Some(Value::Array(list)) = value else { return names };
    for name in list.iter().take(200).filter_map(Value::as_str) {
        if names.len() >= MAX_MESSAGE_KEYWORDS {
            break;
        }
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_KEYWORD_CATEGORY || names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

impl Entry {
    fn flags(&self, uid: u32) -> RemoteFlags {
        RemoteFlags { uid, read: self.read, flagged: self.flagged, answered: false, draft: self.draft }
    }

    /// The message that came down, with what Graph says of it.
    fn message(&self, bytes: &[u8], uid: u32) -> Message {
        let mut message = mailcraft_mime::parse(bytes);
        self.dress(&mut message, uid);
        message
    }

    fn dress(&self, message: &mut Message, uid: u32) {
        message.uid = uid;
        message.remote_id = self.id.clone();
        message.read = self.read;
        message.flag = if self.flagged { FlagStatus::Flagged } else { FlagStatus::None };
        message.draft = self.draft;
        for name in &self.categories {
            if !message.has_category(name) {
                message.categories.push(name.clone());
            }
        }
        message.synced_categories = self.categories.clone();
        if self.received != 0 {
            message.received = self.received;
        } else {
            message.received = message.date;
        }
    }
}

/// Everything the run does, over Graph: see the module's documentation.
pub(crate) fn exchange(run: &mut Run<'_>) {
    let request = run.request;
    let base = graph_base();
    // A shared mailbox is spoken of by its address, with its owner's token.
    let mailbox = if request.account.is_shared() { request.account.email.as_str() } else { "" };
    let client = match Client::new(&base, &request.access_token, mailbox, run.cancel_flag()) {
        Ok(client) => client,
        Err(e) => {
            run.failed("Couldn't check for new mail", &e);
            return;
        }
    };
    // (A look at the Inbox alone sends nothing and changes nothing.)
    if !request.inbox_only && !push::outbox(&client, run) {
        return;
    }
    if run.cancelled() {
        return;
    }
    run.hand_over("Connecting to Microsoft Graph".into());
    let tied: Vec<(String, FolderKind)> = request.folders.iter().filter(|f| !f.remote.is_empty()).map(|f| (f.remote.clone(), f.kind)).collect();
    let mut tree = match folders::list(&client, &tied) {
        Ok(tree) => tree,
        Err(e) => {
            run.failed("Couldn't read the list of folders", &e);
            return;
        }
    };
    // Folders made, renamed, moved or deleted here first: the rest of the run knows the folders
    // by their new names.
    let reshaped = if request.inbox_only { None } else { push::folder_ops(&client, run, &mut tree) };
    let request = match &reshaped {
        Some(Ok(request)) => request,
        Some(Err(())) => return,
        None => request,
    };
    run.out.errors.append(&mut tree.errors);
    if !request.inbox_only {
        run.out.listed = tree.listed();
        run.out.listed_whole = tree.whole;
    }
    if !run.out.listed.is_empty() {
        let count = run.out.listed.len();
        run.hand_over(format!("Found {}", if count == 1 { "1 folder".to_string() } else { format!("{count} folders") }));
    }
    if run.cancelled() {
        return;
    }
    if !request.inbox_only && (!push::uploads(&client, run, request, &mut tree) || !push::ops(&client, run, request, &mut tree)) {
        return;
    }
    if request.push_only {
        return;
    }
    let mut plan = tree.plan(request.scope);
    if request.inbox_only && request.scope == SyncScope::All {
        plan.retain(|node| node.kind == FolderKind::Inbox);
        plan.truncate(1);
    }
    let count = plan.len();
    let mut claimed: HashSet<u64> = HashSet::new();
    let max_new = request.max_per_folder as usize;
    let mut inbox: Option<Glance> = None;
    // A run of the folders alone doesn't read the Inbox in its turn, but glances at it between
    // two folders all the same, from the state the request carries of it.
    if run.rest_only()
        && let Some(node) = tree.of_kind(FolderKind::Inbox).cloned()
    {
        let state = crate::sync::state_for(request, &node.path, node.kind);
        let base = FolderState { remote: node.path.clone(), kind: node.kind, ..state.cloned().unwrap_or_default() };
        run.inbox_read = Some(Instant::now());
        inbox = Some(Glance { node, state: base, max_new });
    }
    for (at, node) in plan.iter().enumerate() {
        if run.cancelled() {
            break;
        }
        if !push::top_up(&client, run, &mut tree) {
            return;
        }
        // A run busy with other folders for long looks at the Inbox again: new mail doesn't wait.
        if let Some(glance) = inbox.as_ref()
            && !glance.look(&client, run, &mut tree)
        {
            return;
        }
        let state = crate::sync::state_for(request, &node.path, node.kind).filter(|state| claimed.insert(state.id));
        if node.kind == FolderKind::Inbox && inbox.is_none() {
            // (Known by the folder's path, which a folder here that doesn't follow it yet hasn't got.)
            let base = FolderState { remote: node.path.clone(), kind: node.kind, ..state.cloned().unwrap_or_default() };
            if !read_folder(&client, run, node, state, max_new, (at + 1, count), &mut tree, None) {
                return;
            }
            run.inbox_read = Some(Instant::now());
            inbox = Some(Glance { node: node.clone(), state: base, max_new });
            continue;
        }
        if !read_folder(&client, run, node, state, max_new, (at + 1, count), &mut tree, inbox.as_ref()) {
            return;
        }
    }
    push::top_up(&client, run, &mut tree);
}

/// The Inbox, as it was when the run began, for looking at it again later in the run.
struct Glance {
    node: Node,
    state: FolderState,
    max_new: usize,
}

impl Glance {
    /// Read the Inbox again, from where the run left it, when it is due (`Limits::inbox_every_graph`): what
    /// came since its last look comes down now, and not only when the run is over. Returns
    /// whether the run goes on.
    fn look(&self, client: &Client<'_>, run: &mut Run<'_>, tree: &mut Tree) -> bool {
        if !run.inbox_due(run.limits.inbox_every_graph) || run.cancelled() {
            return true;
        }
        // What waits goes first, so that the Inbox's state counts all of it.
        run.hand_over("Checking Inbox for new mail".into());
        let now = run.state_now(&self.state);
        run.inbox_read = Some(Instant::now());
        read_folder(client, run, &self.node, Some(&now), self.max_new, (1, 1), tree, None)
    }
}

/// The messages of a folder as they come down, handed over a batch at a time.
struct Arrivals {
    delta: FolderDelta,
    /// The folder's count while messages are still to come.
    base: u32,
    waiting: usize,
    done: usize,
    total: usize,
    /// How many more the run's limit left for a later run.
    more: usize,
    label: String,
}

impl Arrivals {
    fn take(&mut self, run: &mut Run<'_>, message: Message, bytes: usize) {
        run.limits.messages = run.limits.messages.saturating_sub(1);
        run.limits.bytes = run.limits.bytes.saturating_sub(bytes);
        self.waiting = self.waiting.saturating_add(bytes);
        self.done = self.done.saturating_add(1);
        self.delta.new.push(message);
        let full = self.delta.new.len() >= run.limits.batch_messages || self.waiting >= run.limits.batch_bytes;
        if self.done < self.total && (full || run.batch_due()) {
            let rest = FolderDelta {
                id: self.delta.id,
                remote: self.delta.remote.clone(),
                kind: self.delta.kind,
                uid_validity: 1,
                uid_seen: self.base,
                ..Default::default()
            };
            let mut batch = std::mem::replace(&mut self.delta, rest);
            batch.new.sort_by_key(|m| m.uid);
            run.out.folders.push(batch);
            run.hand_over(crate::sync::progress(&self.label, self.done, self.total, self.more));
            self.waiting = 0;
        }
    }
}

/// Read one folder: what came, changed and went since it was last read through, and the new
/// messages, newest first, as far as the run's limits go. Returns whether the run goes on.
#[allow(clippy::too_many_arguments)]
fn read_folder(
    client: &Client<'_>,
    run: &mut Run<'_>,
    node: &Node,
    state: Option<&FolderState>,
    max_new: usize,
    place: (usize, usize),
    tree: &mut Tree,
    inbox: Option<&Glance>,
) -> bool {
    let name = shown(&node.path);
    let label = label(&node.path);
    run.hand_over(format!("Checking {label} ({} of {})", place.0, place.1));
    // A folder read through before whose counts (every message, the unread ones) are as the
    // listing said then has nothing new, nothing read elsewhere and nothing gone: its round is
    // not asked for. Every few runs it is all the same, in case as much came in as went out;
    // and a folder this run's pushes put something into is read whatever the counts say.
    if let Some(state) = state
        && let Some(mark) = unchanged_by_counts(node, state, run)
    {
        log::debug!("{name}: as it was by its counts ({} messages, {} unread), not read", node.total, node.unread);
        run.out.folders.push(FolderDelta {
            id: Some(state.id).filter(|id| *id != 0),
            remote: node.path.clone(),
            kind: node.kind,
            uid_validity: 1,
            uid_seen: state.uid_seen,
            mark,
            ..Default::default()
        });
        return true;
    }
    let known: HashMap<&str, u32> =
        state.map_or_else(HashMap::new, |s| s.remote_ids.iter().filter(|r| r.uid != 0).map(|r| (r.id.as_str(), r.uid)).collect());
    let known_uids: BTreeSet<u32> = state.map_or_else(BTreeSet::new, |s| s.known_uids.iter().copied().filter(|uid| *uid != 0).collect());
    let stored = state.map(|s| s.delta.as_str()).filter(|link| link.contains("/messages/delta")).and_then(|link| client.named(link));
    let fresh = client.url(&format!("/v1.0/me/mailFolders/{}/messages/delta?$select={DELTA_FIELDS}", http::encode(&node.id)));
    // A round from where the last one ended, or from the start; and from the start when Graph
    // no longer knows where that was.
    let mut rounds = match stored {
        Some(url) => vec![(url, false), (fresh.clone(), true)],
        None => vec![(fresh, true)],
    };
    rounds.truncate(2);
    let mut outcome = None;
    for (url, whole_round) in rounds {
        let mut here: Vec<Entry> = Vec::new();
        let mut gone: Vec<String> = Vec::new();
        let read = client.pages(url, MAX_FOLDER_MESSAGES, &mut |value| match seen(value) {
            Some(Seen::Here(entry)) => here.push(entry),
            Some(Seen::Gone(id)) => gone.push(id),
            None => {}
        });
        match read {
            Ok(listed) => {
                outcome = Some((listed, here, gone, whole_round));
                break;
            }
            Err(Stop::Stale) if !whole_round => {
                log::debug!("{name}: Graph no longer knows the last round; read from the start");
                continue;
            }
            Err(stop) => {
                let e = stop.into_error();
                if matches!(&e, NetError::Proto(ProtoError::Io(why)) if why == crate::transport::STOPPED) {
                    return true;
                }
                let goes_on = !ends_run(&e);
                run.failed(&format!("Couldn't read \"{name}\""), &e);
                return goes_on;
            }
        }
    }
    let Some((listed, here, gone, whole_round)) = outcome else { return true };
    if !listed.whole {
        run.out.errors.push(format!("\"{name}\" has more messages than are read at once; only the first {MAX_FOLDER_MESSAGES} were"));
    }
    // A whole folder that says it holds messages and lists none is not believed: believing it
    // would take every message here of it away.
    if whole_round && listed.whole && here.is_empty() && node.total > 0 && !known_uids.is_empty() {
        run.out.errors.push(format!(
            "Couldn't read \"{name}\": Microsoft Graph listed none of its {}",
            messages(usize::try_from(node.total).unwrap_or(usize::MAX))
        ));
        return true;
    }
    let mut flags = Vec::new();
    let mut categories = Vec::new();
    let mut seen_known: HashSet<u32> = HashSet::new();
    let mut wanted: Vec<Entry> = Vec::new();
    let mut wanted_ids: HashSet<String> = HashSet::new();
    for entry in here {
        match known.get(entry.id.as_str()) {
            Some(&uid) => {
                seen_known.insert(uid);
                flags.push(entry.flags(uid));
                if !entry.categories.is_empty() {
                    categories.push(RemoteCategories { uid, names: entry.categories.clone() });
                }
            }
            None => {
                if wanted_ids.insert(entry.id.clone()) {
                    wanted.push(entry);
                }
            }
        }
    }
    let mut removed: Vec<u32> = gone.iter().filter_map(|id| known.get(id.as_str()).copied()).filter(|uid| !seen_known.contains(uid)).collect();
    if whole_round && listed.whole {
        removed.extend(known_uids.iter().copied().filter(|uid| !seen_known.contains(uid)));
    }
    removed.sort_unstable();
    removed.dedup();
    // What went is not wanted, whatever came before it in the round.
    let gone_ids: HashSet<&str> = gone.iter().map(String::as_str).collect();
    wanted.retain(|entry| !gone_ids.contains(entry.id.as_str()));
    flags.retain(|f| !removed.contains(&f.uid));
    categories.retain(|c| !removed.contains(&c.uid));

    // Newest first; with a limit the newest win, and the rest is for a later run.
    wanted.sort_by_key(|entry| std::cmp::Reverse(entry.received));
    let mut complete = listed.whole;
    let mut more = 0;
    if max_new > 0 && wanted.len() > max_new {
        more = wanted.len() - max_new;
        wanted.truncate(max_new);
        complete = false;
    }
    let highest_known = known_uids.iter().copied().chain(known.values().copied()).max().unwrap_or(0);
    let base = state.map_or(0, |s| s.uid_seen).max(highest_known);
    // Oldest of them first in the count.
    let total = wanted.len();
    let numbered: Vec<(u32, Entry)> =
        wanted.into_iter().enumerate().map(|(at, entry)| (base.saturating_add(u32::try_from(total - at).unwrap_or(u32::MAX)), entry)).collect();
    let delta = FolderDelta {
        // (A folder here that doesn't follow the mailbox yet is found by the mailbox's path.)
        id: state.map(|s| s.id).filter(|id| *id != 0),
        remote: node.path.clone(),
        kind: node.kind,
        uid_validity: 1,
        uid_seen: base,
        new: Vec::new(),
        flags,
        removed,
        categories,
        mark: FolderMark::default(),
        delta: String::new(),
    };
    let mut arrivals = Arrivals { delta, base, waiting: 0, done: 0, total, more, label };
    let mut missing: BTreeSet<u32> = numbered.iter().map(|(uid, _)| *uid).collect();
    let mut highest = base;
    let mut alive = true;
    let mut rest: &[(u32, Entry)] = &numbered;
    let mut first = true;
    while alive && !rest.is_empty() && !run.cancelled() {
        if !std::mem::take(&mut first) && !push::top_up(client, run, tree) {
            alive = false;
            break;
        }
        // A large folder that takes long doesn't hold the Inbox up either.
        if let Some(glance) = inbox
            && !glance.look(client, run, tree)
        {
            alive = false;
            break;
        }
        let room = run.limits.messages.min(run.limits.fetch_messages.max(1)).min(super::BATCH_SIZE);
        if room == 0 || run.limits.bytes == 0 {
            break;
        }
        let (now, later) = rest.split_at(room.min(rest.len()));
        rest = later;
        match download(client, run, now) {
            Ok((got, vanished)) => {
                for (uid, message, bytes) in got {
                    missing.remove(&uid);
                    highest = highest.max(uid);
                    arrivals.take(run, message, bytes);
                }
                // Gone between the round and now: not missing.
                for uid in vanished {
                    missing.remove(&uid);
                }
            }
            Err(e) => {
                alive = !ends_run(&e);
                run.failed(&format!("Couldn't download a message from \"{name}\""), &e);
            }
        }
    }
    let mut delta = arrivals.delta;
    delta.uid_seen = highest.max(base);
    if complete && missing.is_empty() && alive && !run.cancelled() {
        delta.delta = listed.delta.unwrap_or_default();
        // Read through: the counts the listing gave are what the next run compares with.
        delta.mark = counts_mark(node);
    }
    delta.new.sort_by_key(|m| m.uid);
    log::debug!("{name}: {} new, {} gone, {} left for later", arrivals.done, delta.removed.len(), missing.len());
    run.out.folders.push(delta);
    alive
}

/// The mark of a folder just read through: how many messages the listing said it holds and how
/// many of them are unread, for the next run to compare with ([`unchanged_by_counts`]).
fn counts_mark(node: &Node) -> FolderMark {
    FolderMark { exists: u32::try_from(node.total).unwrap_or(u32::MAX), unread: u32::try_from(node.unread).unwrap_or(u32::MAX), ..Default::default() }
}

/// Whether the folder is as it was when it was last read through, by the listing's counts
/// alone: the mark to write when it is (its `quick_runs` one more), `None` when its round is
/// to be asked for. Only a folder that was read through (it has a round to go on from), whose
/// two counts are the mark's, with nothing left for later, nothing this run put into it, and
/// not every [`crate::sync::FULL_SCAN_EVERY`]th run.
fn unchanged_by_counts(node: &Node, state: &FolderState, run: &Run<'_>) -> Option<FolderMark> {
    let mark = &state.mark;
    let read_through = !state.delta.is_empty();
    let same = u64::from(mark.exists) == node.total && u64::from(mark.unread) == node.unread;
    let due_anyway = mark.quick_runs.saturating_add(1) >= crate::sync::FULL_SCAN_EVERY
        || (!mark.skipped.is_empty() && mark.skip_runs.saturating_add(1) >= crate::sync::FULL_SCAN_EVERY);
    if !read_through || !same || mark.backfill != 0 || due_anyway || run.touched.contains(&node.path) {
        return None;
    }
    Some(FolderMark { quick_runs: mark.quick_runs.saturating_add(1), ..mark.clone() })
}

/// Bring the messages of `now` down: twenty to a batch; each on its own when the batch's
/// answer is too long; and of one too large to take, only its header. Each comes with its
/// number and how many bytes it was; and the numbers of those Graph no longer has come apart.
/// One it wouldn't hand over is in neither, and is asked for by the next run.
fn download(client: &Client<'_>, run: &mut Run<'_>, now: &[(u32, Entry)]) -> Result<Downloaded, NetError> {
    let largest = usize::try_from(run.limits.largest).unwrap_or(usize::MAX).min(MAX_LARGE_BODY);
    let requests: Vec<(&str, String, Option<Value>)> =
        now.iter().map(|(_, entry)| ("GET", format!("/me/messages/{}/$value", http::encode(&entry.id)), None)).collect();
    let (mut out, mut vanished) = (Vec::new(), Vec::new());
    let answers = match client.batch(&requests, MAX_LARGE_BODY) {
        Ok(answers) => answers,
        // Too much for one answer: one at a time.
        Err(e) if http::is_too_long(&e) => vec![None; now.len()],
        Err(e) => return Err(e),
    };
    for ((uid, entry), answer) in now.iter().zip(answers) {
        match answer {
            Some((200, Value::String(text))) => match STANDARD.decode(text.as_bytes()) {
                Ok(bytes) if bytes.len() <= largest => {
                    let size = bytes.len();
                    out.push((*uid, entry.message(&bytes, *uid), size));
                }
                Ok(_) => out.push((*uid, too_large(client, entry, *uid, largest), 0)),
                Err(_) => run.out.errors.push("Microsoft Graph handed over a message that couldn't be read".into()),
            },
            // Gone since the round: nothing to bring.
            Some((404, _)) => vanished.push(*uid),
            Some((401, _)) => return Err(NetError::Proto(ProtoError::Auth("Microsoft Graph didn't take the sign-in".into()))),
            Some((status, _)) if status != 200 && status != 0 => {
                log::debug!("a message wasn't handed over: status {status}");
                run.out.errors.push(format!("Microsoft Graph didn't hand over a message (status {status})"));
            }
            // No answer in the batch, or one that is no message: asked for on its own.
            _ => match one(client, entry, largest)? {
                Fetched::Message(bytes) => {
                    let size = bytes.len();
                    out.push((*uid, entry.message(&bytes, *uid), size));
                }
                Fetched::TooLarge => out.push((*uid, too_large(client, entry, *uid, largest), 0)),
                Fetched::Gone => vanished.push(*uid),
            },
        }
    }
    Ok((out, vanished))
}

/// What came down: each message with its number and its size; and the numbers of those that
/// were gone.
type Downloaded = (Vec<(u32, Message, usize)>, Vec<u32>);

/// What asking for one message came to.
enum Fetched {
    Message(Vec<u8>),
    TooLarge,
    Gone,
}

fn one(client: &Client<'_>, entry: &Entry, largest: usize) -> Result<Fetched, NetError> {
    let url = client.url(&format!("/v1.0/me/messages/{}/$value", http::encode(&entry.id)));
    match client.call("GET", &url, None, PREFER, FETCH, largest) {
        Ok(reply) if reply.status == 200 => Ok(Fetched::Message(reply.body)),
        Ok(reply) if reply.status == 404 => Ok(Fetched::Gone),
        Ok(reply) => Err(client.refusal(&reply, "")),
        Err(e) if http::is_too_long(&e) => Ok(Fetched::TooLarge),
        Err(e) => Err(e),
    }
}

/// What is kept of a message too large to download: its header fields as Graph lists them, and
/// in place of its text a note that says so.
fn too_large(client: &Client<'_>, entry: &Entry, uid: u32, largest: usize) -> Message {
    let mut header = Vec::new();
    if let Ok(value) = client.get_json(&format!("/v1.0/me/messages/{}?$select=internetMessageHeaders", http::encode(&entry.id)))
        && let Some(Value::Array(fields)) = value.get("internetMessageHeaders")
    {
        for field in fields.iter().take(MAX_HEADERS) {
            let (Some(name), Some(text)) = (field.get("name").and_then(Value::as_str), field.get("value").and_then(Value::as_str)) else { continue };
            let plain = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>();
            let (name, text) = (plain(name), plain(text));
            if name.is_empty() || name.len() > 100 || !name.bytes().all(|b| b.is_ascii_graphic() && b != b':') || text.len() > MAX_HEADER {
                continue;
            }
            header.extend_from_slice(format!("{name}: {text}\r\n").as_bytes());
        }
    }
    header.extend_from_slice(b"\r\n");
    let mut message = mailcraft_mime::parse(&header);
    entry.dress(&mut message, uid);
    message.attachments.clear();
    let largest = u64::try_from(largest).unwrap_or(u64::MAX);
    message.body = Body {
        text: format!(
            "This message is too large to download: MailCraft takes messages of up to {}.\n\n\
             What is here comes from its header alone: who sent it, when, and what it is about. The message itself, \
             with anything attached to it, is still on the server. Read it there with another mail program or on the web.\n",
            megabytes(largest)
        ),
        html: None,
    };
    message.headers.retain(|(name, _)| !name.eq_ignore_ascii_case(NOT_DOWNLOADED));
    message.headers.push((NOT_DOWNLOADED.into(), largest.saturating_add(1).to_string()));
    message
}
