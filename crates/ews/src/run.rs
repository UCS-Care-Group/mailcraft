// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run for an Exchange server account (#252, E6): the folders, then each folder's messages as
//! headers and a preview from the sync state it went on from, and messages fetched whole. It
//! speaks through a [`Client`] over any [`HttpExchange`] and hands the run back in pieces, as
//! `mailcraft-jmap` does; it reads no clock and starts no thread.
//!
//! **Pieces**, in this order: the folders (when asked for, one piece), then one piece for each
//! page of each folder (the page with the sync state to go on from, taken in together), and a last
//! piece, the only one that is not `partial`, that says how the run ended. `cancel` is looked at
//! before every request.
//!
//! **Waiting.** A busy server (`ErrorServerBusy` with `BackOffMilliseconds`, a 429 or a 503) is
//! waited for, up to three times for one request, through `wait` (the host sleeps, in small
//! steps, for as many milliseconds as it is told, and says `false` when the run was stopped meanwhile).
//! A server still busy after that ends the run with the seconds it asked for.
//!
//! **A state the server no longer knows** (`ErrorInvalidSyncStateData`) reads the folder (or the
//! folder list) afresh: the first page says so, and what the fresh read doesn't have is taken away
//! when the run ends well, matching by item id.
//!
//! **What a server lacks is left out** (E7): a server that doesn't know the preview or the flag
//! (`ErrorInvalidPropertyRequest`) is asked again without them, for the rest of the run; folders
//! that are not mail folders are passed over. These are logged as counts, once per run.
//!
//! **The lines of trouble** name no folder, no address and nothing a server said.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{
    EwsFetchRequest, EwsFolderDeltas, EwsMailRequest, EwsMailResult, EwsMessagePage, EwsScope, FolderChange, GraphFetchResult, GraphFetched,
    GraphFolder, GraphProgress, MAX_THREAD_ASKS, ThreadOf,
};

use crate::client::Client;
use crate::error::{Error, ResponseCode};
use crate::folder::{DistinguishedFolder, FolderInfo, GetFolder};
use crate::hierarchy::{MAX_NAME, SyncFolderHierarchy};
use crate::http::{HttpExchange, TransportError};
use crate::items::{GetItemConversations, GetItemMime, MAX_CONVERSATIONS, Shape, SyncFolderItems};
use crate::reply::Reply;
use crate::soap::Element;

/// How many changes one page of a folder asks for: enough that a large folder is a few hundred
/// requests, few enough that a page is quick to take in.
pub const PAGE: u32 = 200;
/// How many times one request is made again for a busy server.
const BUSY_TRIES: u32 = 3;
/// The most folders one run reads, and the most pages of one folder.
const MAX_FOLDERS: usize = 2_000;
const MAX_PAGES: usize = 5_000;
/// The most pages of the folder list one read follows.
const MAX_HIERARCHY_PAGES: usize = 20;
/// The most messages one fetch asks for.
const MAX_FETCH: usize = 100;
/// After this many folders in a row that couldn't be read, the run ends.
const MAX_IN_A_ROW: usize = 3;

/// Why a request didn't give an answer: the run has to stop, or the server answered with a code.
pub(crate) enum Asked {
    Stop(Stop),
    Code(ResponseCode),
    /// The answer wasn't one that could be read.
    Bad,
    /// A request that makes or sends something went out and its answer was lost (or was none the
    /// server's service would give): it may have happened, and it was not sent again.
    Lost,
    /// A page of a folder holds a change that can't be taken in: the folder stays where it was.
    Unreadable,
}

/// What ends a run.
pub(crate) enum Stop {
    Cancelled,
    Auth,
    Busy(u32),
    Tls,
    /// The server could not be reached at all.
    Unreachable,
    Line(&'static str),
}

/// How a run ended so far.
#[derive(Default)]
struct Outcome {
    errors: Vec<String>,
    stopped: bool,
    auth_refused: bool,
    busy_for: Option<u32>,
    /// A folder or the folder list was left unread.
    incomplete: bool,
    /// The folders (their server ids) whose pages hold a change that can't be taken in.
    blocked: Vec<String>,
}

impl Outcome {
    fn line(&mut self, text: &str) {
        if self.errors.len() < 32 && !self.errors.iter().any(|have| have == text) {
            self.errors.push(text.to_owned());
        }
    }

    fn clean(&self) -> bool {
        self.errors.is_empty() && !self.stopped && !self.auth_refused && self.busy_for.is_none() && !self.incomplete
    }

    fn stop(&mut self, why: Stop) {
        self.incomplete = true;
        match why {
            Stop::Cancelled => self.stopped = true,
            Stop::Auth => self.auth_refused = true,
            Stop::Busy(seconds) => self.busy_for = Some(seconds),
            Stop::Tls => self.line("Couldn't set up a secure connection with the server"),
            Stop::Unreachable => self.line("Couldn't reach the server"),
            Stop::Line(text) => self.line(text),
        }
    }
}

pub(crate) struct Run<'a, E: HttpExchange> {
    pub(crate) client: &'a Client,
    pub(crate) exchange: &'a E,
    pub(crate) cancel: &'a AtomicBool,
    pub(crate) wait: &'a mut dyn FnMut(u32) -> bool,
    /// The properties asked for besides the default ones, until a server says it lacks some.
    pub(crate) shape: Shape,
    /// The server has no follow-up flag: flags stay here (E7), said once.
    pub(crate) flag_lacking: bool,
    /// A read-state change that said no state was passed over: said once a run, with no id.
    pub(crate) read_unsaid: bool,
    /// The ids of the server's other own folders ([`OWN`]) it has, as far as it said (#779): a
    /// change of one is never sent.
    pub(crate) own: HashSet<String>,
    /// A folder deleted here is still on the server (it refused, or it is one of its own): the
    /// folder list is read afresh, as a delta wouldn't bring it back.
    pub(crate) reread_folders: bool,
}

impl<E: HttpExchange> Run<'_, E> {
    /// Sends `operation` and reads the answer with `read`, again after a busy server's wait.
    pub(crate) async fn ask<T>(&mut self, operation: &Element, read: &dyn Fn(&Reply) -> Result<T, Error>) -> Result<T, Asked> {
        self.ask_with(operation, read, false).await
    }

    /// [`Run::ask`] for an operation that makes or sends something: never sent twice, and an answer
    /// that was lost, or that is none the service gives, is [`Asked::Lost`].
    pub(crate) async fn ask_once<T>(&mut self, operation: &Element, read: &dyn Fn(&Reply) -> Result<T, Error>) -> Result<T, Asked> {
        self.ask_with(operation, read, true).await
    }

    async fn ask_with<T>(&mut self, operation: &Element, read: &dyn Fn(&Reply) -> Result<T, Error>, once: bool) -> Result<T, Asked> {
        let mut tries = 0u32;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Asked::Stop(Stop::Cancelled));
            }
            let called = if once { self.client.call_once(self.exchange, operation).await } else { self.client.call(self.exchange, operation).await };
            let outcome = match called {
                Ok(reply) => read(&reply),
                Err(error) => Err(error),
            };
            let error = match outcome {
                Ok(done) => return Ok(done),
                Err(error) => error,
            };
            match error {
                Error::Busy { back_off_ms } => {
                    tries = tries.saturating_add(1);
                    if tries > BUSY_TRIES {
                        return Err(Asked::Stop(Stop::Busy(back_off_ms.div_ceil(1000).max(1))));
                    }
                    if !(self.wait)(back_off_ms) {
                        return Err(Asked::Stop(Stop::Cancelled));
                    }
                }
                Error::Unauthorized => return Err(Asked::Stop(Stop::Auth)),
                Error::Forbidden | Error::Blocked(_) => {
                    return Err(Asked::Stop(Stop::Line("The server doesn't allow Exchange Web Services for this account")));
                }
                Error::Endpoint(_) => return Err(Asked::Stop(Stop::Line("The server's address can't be used"))),
                Error::Transport(TransportError::Aborted) => return Err(Asked::Stop(Stop::Cancelled)),
                Error::Transport(TransportError::Tls) => return Err(Asked::Stop(Stop::Tls)),
                Error::Transport(TransportError::Unreachable) => return Err(Asked::Stop(Stop::Unreachable)),
                Error::Transport(_) if once => return Err(Asked::Lost),
                Error::Transport(_) => return Err(Asked::Stop(Stop::Unreachable)),
                Error::Server(code) => return Err(Asked::Code(code)),
                // (An answer that is none the service gives, to a request that may have been done.)
                Error::Status(_) | Error::Malformed | Error::TooLarge | Error::Unexpected if once => return Err(Asked::Lost),
                Error::Unreadable => return Err(Asked::Unreadable),
                _ => return Err(Asked::Bad),
            }
        }
    }
}

/// The distinguished folders a run asks the ids of, with the kind each is. The server's Outbox is
/// no kind of MailCraft's (AU2-V2: mail goes from MailCraft's own Outbox): it is asked for to be
/// left out of the folder list while it is empty ([`server_outbox`]).
const KNOWN: [(DistinguishedFolder, Option<FolderKind>); 7] = [
    (DistinguishedFolder::MessageRoot, None),
    (DistinguishedFolder::Inbox, Some(FolderKind::Inbox)),
    (DistinguishedFolder::Drafts, Some(FolderKind::Drafts)),
    (DistinguishedFolder::SentItems, Some(FolderKind::Sent)),
    (DistinguishedFolder::DeletedItems, Some(FolderKind::Deleted)),
    (DistinguishedFolder::JunkEmail, Some(FolderKind::Junk)),
    (DistinguishedFolder::Outbox, None),
];

/// The server's own folders that are no kind of MailCraft's (#779), and its Outbox, which isn't
/// either (AU2-V2): asked for in a request of their own, so that a server that doesn't know one of
/// the names loses nothing else. They are never renamed, moved or deleted (the server refuses it,
/// or should). These are the mail folders among the schema's `DistinguishedFolderIdNameType` names
/// that lie under the top of the mail; Graph's `clutter` and `scheduled` have no such name, and
/// what lies under the top of the mailbox (`searchfolders`, `recoverableitems…`) is never listed.
const OWN: [DistinguishedFolder; 6] = [
    DistinguishedFolder::Outbox,
    DistinguishedFolder::ConversationHistory,
    DistinguishedFolder::SyncIssues,
    DistinguishedFolder::Conflicts,
    DistinguishedFolder::LocalFailures,
    DistinguishedFolder::ServerFailures,
];

/// How [`OWN`] is asked for when a server won't take the names together: the Outbox, which every
/// server knows, Conversation History, and Sync Issues with the three folders in it (a group that
/// fails too is asked name by name).
const OWN_GROUPS: [std::ops::Range<usize>; 3] = [0..1, 1..2, 2..6];

/// What the server calls its special folders.
#[derive(Default)]
struct Specials {
    root: Option<String>,
    inbox: Option<String>,
    kinds: HashMap<String, FolderKind>,
    /// The server's Outbox, as it is now.
    outbox: Option<FolderInfo>,
}

/// The server's Outbox is left out of the folder list while it holds nothing and has no folder in
/// it, as an Exchange mailbox's is over IMAP and a Microsoft account's (#743), so that the account
/// shows one Outbox, MailCraft's own, not two (#747). A list that had it loses it (its id is among
/// the deleted). While it holds mail it is listed like any other folder, so nothing is hidden: a
/// delta that doesn't bring it has it as the server says it is now, at the top of the mail, where
/// every server keeps it. Never sent from, either way. The id it left out, if it did.
fn server_outbox(deltas: &mut EwsFolderDeltas, specials: &Specials) -> Option<String> {
    let outbox = specials.outbox.as_ref()?;
    let id = outbox.id.as_str();
    if outbox.total_count == Some(0) && outbox.child_folder_count == Some(0) {
        deltas.changed.retain(|f| f.id != id);
        if !deltas.deleted.iter().any(|have| have == id) {
            deltas.deleted.push(id.to_owned());
        }
        return Some(id.to_owned());
    }
    let name: String = outbox.display_name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if !name.is_empty() && !deltas.changed.iter().any(|f| f.id == id) {
        deltas.changed.push(GraphFolder {
            id: id.to_owned(),
            name,
            kind: FolderKind::Custom,
            total: outbox.total_count.unwrap_or(0),
            unread: outbox.unread_count.unwrap_or(0),
            counted: outbox.total_count.is_some() && outbox.unread_count.is_some(),
            ..GraphFolder::default()
        });
    }
    None
}

impl Specials {
    /// The ids of the server's own folders: these, and `own` (not the top of the mail, which is
    /// no folder here). Sorted, so that a run says them the same way each time.
    fn own(&self, own: &HashSet<String>) -> Vec<String> {
        let mut ids: Vec<String> = own.iter().chain(self.kinds.keys()).cloned().chain(self.outbox.iter().map(|o| o.id.as_str().to_owned())).collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Whether a request that failed with `asked` ends the run even when it was only for the server's
/// own folders (#779): it was called off, the sign-in was refused, or the server asked to be left
/// alone (a 429, a 503 or `ErrorServerBusy`, which the next request would meet as well).
/// Anything else of theirs is taken as "not there", so that Send/Receive never fails over them
/// (and a mark once set stays).
fn ends_the_run(asked: &Asked) -> bool {
    matches!(asked, Asked::Stop(Stop::Cancelled | Stop::Auth | Stop::Busy(_)))
}

impl<E: HttpExchange> Run<'_, E> {
    /// The ids of the folders of [`OWN`] the server has: one request for them all, and, when the
    /// server won't take it (a schema that lacks one of the names), one for each of
    /// [`OWN_GROUPS`], and name by name for a group it won't take either. Whatever it says of one,
    /// but for what [`ends_the_run`], is "not there".
    async fn own_folders(&mut self) -> Result<HashSet<String>, Asked> {
        let mut found = HashSet::new();
        let mut asks: Vec<&[DistinguishedFolder]> = vec![&OWN];
        let mut at = 0usize;
        // (At most the whole list, its groups and each name: 1 + 3 + 6 requests.)
        while let Some(names) = asks.get(at).copied() {
            at = at.saturating_add(1);
            let get = GetFolder::new(names);
            match self.ask(&get.element(), &|reply| get.answer(reply)).await {
                Ok(answers) => found.extend(answers.into_iter().flatten().map(|info| info.id.as_str().to_owned())),
                Err(stop) if ends_the_run(&stop) => return Err(stop),
                // (Unreachable, refused, no secure connection: nothing more is asked of them. A
                // change of one is then refused by the engine's mark, once a run has set it.)
                Err(Asked::Stop(_)) => {
                    log::info!("ews folders: the server's own folders couldn't be asked for; those not answered are taken as not there");
                    break;
                }
                Err(_) if names.len() == OWN.len() => {
                    log::info!("ews folders: the server didn't take the names of its own folders together; asking them in groups");
                    asks.extend(OWN_GROUPS.iter().filter_map(|group| OWN.get(group.clone())));
                }
                Err(_) if names.len() > 1 => asks.extend(names.chunks(1)),
                Err(_) => {}
            }
        }
        Ok(found)
    }

    async fn specials(&mut self) -> Result<Specials, Asked> {
        let get = GetFolder::new(&KNOWN.map(|(folder, _)| folder));
        let found = self.ask(&get.element(), &|reply| get.answer(reply)).await?;
        let mut out = Specials::default();
        for ((folder, kind), info) in KNOWN.iter().zip(found) {
            let Ok(info) = info else { continue };
            let id = info.id.as_str().to_owned();
            match (folder, kind) {
                (DistinguishedFolder::MessageRoot, _) => out.root = Some(id),
                (DistinguishedFolder::Outbox, _) => out.outbox = Some(info),
                (DistinguishedFolder::Inbox, Some(kind)) => {
                    out.inbox = Some(id.clone());
                    out.kinds.insert(id, *kind);
                }
                (_, Some(kind)) => {
                    out.kinds.insert(id, *kind);
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// The folder list from `state`: all of it (a read afresh when the state is lost).
    async fn folders(&mut self, state: &str, specials: &Specials) -> Result<(EwsFolderDeltas, String), Asked> {
        let (mut state, mut deltas, mut tried_afresh) =
            (state.to_owned(), EwsFolderDeltas { afresh: state.is_empty(), ..EwsFolderDeltas::default() }, false);
        let mut skipped = 0usize;
        for _ in 0..MAX_HIERARCHY_PAGES {
            let op = SyncFolderHierarchy::new(&state);
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(page) => {
                    skipped = skipped.saturating_add(page.skipped);
                    for mut folder in page.changed {
                        if specials.root.as_deref() == Some(folder.parent.as_str()) {
                            folder.parent.clear();
                        }
                        folder.kind = specials.kinds.get(&folder.id).copied().unwrap_or(FolderKind::Custom);
                        deltas.changed.push(folder);
                    }
                    deltas.deleted.extend(page.deleted);
                    state = page.state;
                    if page.last {
                        if skipped > 0 {
                            log::info!("ews folders: {skipped} folders that are not mail folders are left out");
                        }
                        return Ok((deltas, state));
                    }
                }
                Err(Asked::Code(ResponseCode::InvalidSyncState)) if !tried_afresh => {
                    (state, tried_afresh) = (String::new(), true);
                    deltas = EwsFolderDeltas { afresh: true, ..EwsFolderDeltas::default() };
                    log::info!("ews folders: the server no longer knows the folder list's state; reading it afresh");
                }
                Err(other) => return Err(other),
            }
        }
        // (A list that goes on for ever is no list.)
        Err(Asked::Bad)
    }
}

pub(crate) fn base(request: &EwsMailRequest) -> EwsMailResult {
    EwsMailResult { account: request.account, email: request.email.clone(), url: request.url.clone(), ..EwsMailResult::default() }
}

/// Runs `request` against the server `client` speaks to, handing the run back in pieces through
/// `emit`. See the module documentation for the pieces and for waiting.
pub async fn sync_mail<E: HttpExchange>(
    client: &Client,
    exchange: &E,
    request: &EwsMailRequest,
    cancel: &AtomicBool,
    wait: &mut dyn FnMut(u32) -> bool,
    emit: &mut dyn FnMut(EwsMailResult),
) {
    let mut run = Run {
        client,
        exchange,
        cancel,
        wait,
        shape: Shape::Full,
        flag_lacking: false,
        read_unsaid: false,
        own: HashSet::new(),
        reread_folders: false,
    };
    let mut out = Outcome::default();
    if let Err(stop) = run.all(request, &mut out, emit).await {
        match stop {
            Asked::Stop(why) => out.stop(why),
            Asked::Code(_) | Asked::Bad | Asked::Lost | Asked::Unreadable => {
                out.incomplete = true;
                out.line("The server's answer couldn't be used");
            }
        }
    }
    // Then the conversations of mail listed before they were asked for (#795), unless the run was
    // stopped or the server asked to be left alone. Never worth failing the run for.
    let threads = if request.threads.is_empty() || out.stopped || out.busy_for.is_some() || out.auth_refused {
        Vec::new()
    } else {
        run.conversations(&request.threads, &mut out).await
    };
    if !out.blocked.is_empty() {
        // (Once a run, and no id: the engine names the folders by what the owner calls them.)
        log::warn!("ews mail: {} folders hold a change that can't be taken in; they are left where they were", out.blocked.len());
    }
    log::debug!("ews mail: the run is over (clean: {}, stopped: {}, busy: {})", out.clean(), out.stopped, out.busy_for.is_some());
    emit(EwsMailResult {
        errors: out.errors.clone(),
        busy_for: out.busy_for,
        // (Only a run of every folder is the end of a read that may take away what the server lacks.)
        whole: out.clean() && !request.inbox_pass && request.scope == EwsScope::All,
        stopped: out.stopped,
        auth_refused: out.auth_refused,
        blocked: out.blocked,
        threads,
        ..base(request)
    });
}

impl<E: HttpExchange> Run<'_, E> {
    /// The conversation of each of the items `ids` ([`GetItemConversations`]), a hundred to a
    /// request, at most [`MAX_THREAD_ASKS`]: what the server answered for, until it doesn't. A busy
    /// server is said in `outcome`; a request it refused is each of its items refused.
    async fn conversations(&mut self, ids: &[String], outcome: &mut Outcome) -> Vec<ThreadOf> {
        let mut out = Vec::new();
        let ids = ids.get(..ids.len().min(MAX_THREAD_ASKS)).unwrap_or_default();
        for some in ids.chunks(MAX_CONVERSATIONS) {
            let op = GetItemConversations::new(some);
            let said = match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(said) => said,
                Err(Asked::Stop(Stop::Busy(seconds))) => {
                    outcome.busy_for = Some(seconds.max(1));
                    break;
                }
                Err(Asked::Code(_)) => {
                    log::info!("ews mail: the server refused to say the conversations; asked again in a later run, a few times");
                    out.extend(op.ids().iter().map(|id| ThreadOf::refused(id)));
                    break;
                }
                Err(_) => {
                    log::info!("ews mail: conversations not asked for now; a later run goes on");
                    break;
                }
            };
            // (Nothing answered, or every one refused: the server isn't saying these now.)
            let answered = said.iter().any(|t| !t.refused);
            out.extend(said);
            if !answered {
                break;
            }
        }
        log::info!("ews mail: the conversations of {} of {} messages asked for", out.len(), ids.len());
        out
    }
}

impl<E: HttpExchange> Run<'_, E> {
    async fn all(&mut self, request: &EwsMailRequest, out: &mut Outcome, emit: &mut dyn FnMut(EwsMailResult)) -> Result<(), Asked> {
        // The server's own folders (#779), before anything goes up: a change of one is never sent.
        let changes_existing = request.changes.iter().any(|c| !matches!(c, FolderChange::Create { .. }));
        if !request.inbox_pass && (request.list_folders || changes_existing) {
            self.own = self.own_folders().await?;
        }
        // What was changed, written and sent here goes first (not in the Inbox pass).
        if !request.inbox_pass {
            self.push(request, emit).await?;
        }
        // What the server calls its special folders: the Inbox first, and what each folder is.
        let specials = self.specials().await?;
        if specials.inbox.is_none() {
            out.incomplete = true;
            out.line("Couldn't find the Inbox on the server");
        }
        let mut order: Vec<String> = Vec::new();
        // (The server's Outbox, when the list leaves it out: there is nothing in it to read.)
        let mut left_out: Option<String> = None;
        // (A run that lists no folders still reads them afresh when a folder deleted here is kept there.)
        if (request.list_folders || self.reread_folders) && !request.inbox_pass {
            // (A folder deleted here that the server keeps comes back only in a read afresh.)
            let from = if self.reread_folders { "" } else { request.hierarchy.as_str() };
            let (mut deltas, state) = match self.folders(from, &specials).await {
                Ok(found) => found,
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => {
                    out.incomplete = true;
                    out.line("Couldn't read the folders from the server");
                    return Ok(());
                }
            };
            left_out = server_outbox(&mut deltas, &specials);
            deltas.well_known = specials.own(&self.own);
            order.extend(deltas.changed.iter().map(|f| f.id.clone()));
            emit(EwsMailResult { folders: Some(deltas), hierarchy: state, partial: true, ..base(request) });
        }
        // Which folders are read, in which order.
        let mut read: Vec<String> = Vec::new();
        let put = |id: &str, read: &mut Vec<String>| {
            if !id.is_empty() && left_out.as_deref() != Some(id) && !read.iter().any(|have| have == id) && read.len() < MAX_FOLDERS {
                read.push(id.to_owned());
            }
        };
        if let Some(inbox) = &specials.inbox {
            put(inbox, &mut read);
        }
        if !request.inbox_pass {
            match &request.scope {
                EwsScope::Light { also } => also.iter().for_each(|id| put(id, &mut read)),
                EwsScope::All => {
                    request.states.iter().map(|s| s.folder.as_str()).chain(order.iter().map(String::as_str)).for_each(|id| put(id, &mut read))
                }
            }
        }
        let states: HashMap<&str, &str> = request.states.iter().map(|s| (s.folder.as_str(), s.state.as_str())).collect();
        let folders = u32::try_from(read.len()).unwrap_or(u32::MAX);
        let mut failed_in_a_row = 0usize;
        for (at, folder) in read.iter().enumerate() {
            let number = u32::try_from(at.saturating_add(1)).unwrap_or(u32::MAX);
            let state = states.get(folder.as_str()).copied().unwrap_or_default();
            match self.folder(request, folder, state, (number, folders), emit).await {
                Ok(true) => failed_in_a_row = 0,
                // (A folder the server doesn't have: the folder list takes it away.)
                Ok(false) => {}
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                // A folder that is blocked is not a server that is failing: the other folders go on,
                // and it doesn't count towards the folders in a row (the engine says which it was).
                Err(Asked::Unreadable) => {
                    out.incomplete = true;
                    if out.blocked.len() < MAX_FOLDERS {
                        out.blocked.push(folder.clone());
                    }
                }
                Err(_) => {
                    out.incomplete = true;
                    out.line("Some folders couldn't be read from the server");
                    failed_in_a_row = failed_in_a_row.saturating_add(1);
                    if failed_in_a_row >= MAX_IN_A_ROW {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// One folder, from `state` to its end, a page to a piece. `Ok(false)` when the server doesn't
    /// have the folder.
    async fn folder(
        &mut self,
        request: &EwsMailRequest,
        folder: &str,
        state: &str,
        (number, folders): (u32, u32),
        emit: &mut dyn FnMut(EwsMailResult),
    ) -> Result<bool, Asked> {
        let (mut state, mut afresh, mut lost) = (state.to_owned(), state.is_empty(), false);
        let mut read = 0u32;
        for _ in 0..MAX_PAGES {
            let op = SyncFolderItems::new(folder, &state, PAGE, self.shape);
            let page = match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(page) => {
                    if page.read_unsaid > 0 && !self.read_unsaid {
                        self.read_unsaid = true;
                        log::info!("ews mail: the server gave a read-state change that says no state; it is passed over");
                    }
                    page
                }
                Err(Asked::Code(ResponseCode::FolderNotFound)) => return Ok(false),
                Err(Asked::Code(ResponseCode::InvalidSyncState)) if !lost => {
                    (state, afresh, lost) = (String::new(), true, true);
                    log::info!("ews mail: the server no longer knows a folder's sync state; reading the folder afresh");
                    continue;
                }
                Err(Asked::Code(ResponseCode::InvalidPropertyRequest)) if self.shape == Shape::Full => {
                    self.shape = Shape::Lean;
                    log::info!("ews mail: the server lacks the preview or the flag; asking for the rest");
                    continue;
                }
                Err(other) => return Err(other),
            };
            read = read.saturating_add(u32::try_from(page.messages.len()).unwrap_or(u32::MAX));
            let mut messages = page.messages;
            messages.iter_mut().for_each(|m| m.server.folder = folder.to_owned());
            let last = page.last;
            // (The page carries the state it was read from and the one that goes on from it, so that
            // the engine takes a state in only after the page it came from, in order.)
            emit(EwsMailResult {
                pages: vec![EwsMessagePage {
                    folder: folder.to_owned(),
                    messages,
                    removed: page.removed,
                    read: page.read,
                    last,
                    afresh,
                    from: state.clone(),
                    state: page.state.clone(),
                }],
                progress: Some(GraphProgress { folder: folder.to_owned(), read, total: 0, number, folders }),
                partial: true,
                ..base(request)
            });
            afresh = false;
            state = page.state;
            if last {
                return Ok(true);
            }
        }
        // (A folder that goes on for ever: the next run goes on from here.)
        Err(Asked::Bad)
    }
}

/// The end of a run that could not begin: the client could not be made (the address or the
/// credentials cannot be used).
pub fn failed_run(request: &EwsMailRequest, why: &str) -> EwsMailResult {
    EwsMailResult { errors: vec![why.to_owned()], ..base(request) }
}

/// Fetches the messages `request` names whole (their MIME content), one request each, and says
/// which the server no longer has (`missing`) and which it wouldn't give (`failed`). `cancel` is
/// read before every request; `wait` is as for [`sync_mail`].
pub async fn fetch_messages<E: HttpExchange>(
    client: &Client,
    exchange: &E,
    request: &EwsFetchRequest,
    cancel: &AtomicBool,
    wait: &mut dyn FnMut(u32) -> bool,
) -> GraphFetchResult {
    let mut run = Run {
        client,
        exchange,
        cancel,
        wait,
        shape: Shape::Full,
        flag_lacking: false,
        read_unsaid: false,
        own: HashSet::new(),
        reread_folders: false,
    };
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..GraphFetchResult::default() };
    let mut outcome = Outcome::default();
    for id in request.ids.iter().take(MAX_FETCH) {
        let op = GetItemMime::new(id);
        match run.ask(&op.element(), &|reply| op.answer(reply)).await {
            Ok(mime) => out.fetched.push(GraphFetched { id: id.clone(), mime }),
            Err(Asked::Code(ResponseCode::ItemNotFound)) => out.missing.push(id.clone()),
            Err(Asked::Code(_) | Asked::Bad | Asked::Lost | Asked::Unreadable) => out.failed.push(id.clone()),
            Err(Asked::Stop(stop)) => {
                if let Stop::Busy(seconds) = stop {
                    out.busy_for = Some(seconds.max(1));
                }
                out.offline = matches!(stop, Stop::Unreachable);
                let line = match stop {
                    Stop::Cancelled => None,
                    Stop::Unreachable => Some("Couldn't reach the server"),
                    Stop::Auth => Some("The server turned down the sign-in"),
                    Stop::Busy(_) => Some("The server is busy"),
                    Stop::Tls => Some("Couldn't set up a secure connection with the server"),
                    Stop::Line(text) => Some(text),
                };
                if let Some(line) = line {
                    outcome.line(&format!("Couldn't download the messages from the server: {line}"));
                }
                break;
            }
        }
    }
    out.errors = outcome.errors;
    out
}
