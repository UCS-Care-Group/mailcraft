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
//! (`ErrorInvalidPropertyRequest`, or a schema fault from a server answering in a version older
//! than Exchange 2013's, whose schema hasn't them) is asked again without them, for the rest of the run; folders that are not mail
//! folders are passed over. These are logged as counts, once per run.
//!
//! **An older server** (#1349): a server that answers in a version older than Exchange 2013's
//! (the client stepped down to it, [`Client::version`]) is asked for neither the preview nor the
//! flag nor the recipients in `SyncFolderItems`, as Exchange 2010 refuses all three there
//! (the recipients with `ErrorInvalidPropertyForOperation`, which also steps any server down to
//! that shape). The recipients of each page's messages are then asked with `GetItem`, a hundred to
//! a request; a page goes on without the recipients the server won't give.
//!
//! **Recipients left out** (#1151): Exchange 2013 to 2019 take the recipients in the shape of
//! `SyncFolderItems` and answer without them, saying nothing. Whatever the shape, the messages of a
//! page that came with neither a To nor a Cc are asked with `GetItem` as above, so that Sent
//! Items says who each message went to.
//!
//! **The lines of trouble** name no folder, no address and nothing a server said.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{
    EwsFetchRequest, EwsFolderDeltas, EwsMailRequest, EwsMailResult, EwsMessagePage, EwsScope, FolderChange, GraphFetchResult, GraphFetched,
    GraphFolder, GraphMessage, GraphProgress, MAX_THREAD_ASKS, ThreadOf,
};

use crate::client::Client;
use crate::error::{Error, ResponseCode};
use crate::folder::{DistinguishedFolder, FolderInfo, GetFolder};
use crate::hierarchy::{MAX_NAME, SyncFolderHierarchy};
use crate::http::{HttpExchange, TransportError};
use crate::items::{GetItemConversations, GetItemMime, GetItemRecipients, MAX_CONVERSATIONS, MAX_RECIPIENTS_ASKED, Shape, SyncFolderItems};
use crate::reply::Reply;
use crate::soap::Element;

/// How many changes one page of a folder asks for: enough that a large folder is a few hundred
/// requests, few enough that a page is quick to take in.
pub const PAGE: u32 = 200;

/// The smallest a folder's page shrinks to: the least `MaxChangesReturned` the schema takes.
const MIN_PAGE: u32 = 1;
/// How many times one request is made again for a busy server.
const BUSY_TRIES: u32 = 3;
/// The most folders one run reads, and the most pages of one folder.
const MAX_FOLDERS: usize = 2_000;
const MAX_PAGES: usize = 5_000;
/// The most pages of the folder list one read follows.
const MAX_HIERARCHY_PAGES: usize = 20;
/// The most messages one fetch asks for.
const MAX_FETCH: usize = 100;
/// The last ask of a folder that outweighed the reader, said in full: which shape, which list,
/// how many at a time, afresh or from a state, afresh spent or not — the measurements of a death
/// are worth more than the theory of one (#1784, the folder that dies at five to the hour).
fn run_out_of_asking(shape: &Shape, ids: bool, page: u32, state: &str, lost: bool) {
    log::warn!(
        "ews mail: nothing lighter to ask: shape {shape:?}, ids {ids}, {page} at a time, {}, {}",
        if state.is_empty() { "afresh" } else { "carrying a state" },
        if lost { "afresh spent" } else { "afresh unused" }
    );
}

/// After this many folders in a row that couldn't be read, the run ends.
const MAX_IN_A_ROW: usize = 3;
/// The most `GetItem`s for recipients one page asks: a page of the most changes the model takes in,
/// at a hundred to a request.
const MAX_RECIPIENT_ASKS: usize = 20;

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
    /// An answer heavier than any this program reads. Where a folder's pages are asked, they are
    /// asked again with fewer changes at a time ([`MIN_PAGE`]) rather than failing the run.
    TooLarge,
}

/// What ends a run.
#[derive(Debug)]
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
    /// A server with no flag family in its schema took the flag over the MAPI tag instead:
    /// said once (#1784, the flag).
    pub(crate) flag_by_tag: bool,
    /// A read-state change that said no state was passed over: said once a run, with no id.
    pub(crate) read_unsaid: bool,
    /// The ids of the server's other own folders ([`OWN`]) it has, as far as it said (#779): a
    /// change of one is never sent.
    pub(crate) own: HashSet<String>,
    /// A folder deleted here is still on the server (it refused, or it is one of its own): the
    /// folder list is read afresh, as a delta wouldn't bring it back.
    pub(crate) reread_folders: bool,
    /// The server changes an item only when it is named with its change key (Exchange 2010,
    /// #1349): every change of a message asks for the key first, for the rest of the run.
    pub(crate) needs_change_key: bool,
    /// The recipients of some messages couldn't be had with `GetItem`: said once a run.
    pub(crate) recipients_unsaid: bool,
    /// How many changes one page of the folder being read asks for: [`PAGE`] when each folder
    /// starts, halved each time a page is refused for its weight, back to [`PAGE`] when the
    /// folder takes up the light shape — measured to be honoured by servers that pick their own
    /// page size, Kerio Connect among them (#964).
    pub(crate) page: u32,
    /// The pages were shrunk this run: said once, with no folder named.
    pub(crate) page_said: bool,
    /// The light base shape was taken up somewhere this run: said once, with no folder named.
    pub(crate) ids_said: bool,
    /// The folder being read asks with the light base shape: its own lesson, forgotten when the
    /// next folder starts, since the weight a page carries is this folder's (`shape` — what the
    /// server's schema takes — is the run's, and learned for good).
    pub(crate) ids: bool,
    /// The light shape was refused as a shape: said once, with no folder named.
    pub(crate) ids_refused_said: bool,
    /// The server refuses the light asking outright: no folder tries it again this run, and the
    /// oversized pages fall back to halving in the schema shape, which still fits eventually
    /// once the items are big rather than the page (#1784, the re-review's 1 MB-item world).
    pub(crate) ids_refused: bool,
    /// The light asking still names the flag. It goes false for the run the first time an ids
    /// page that asked the flag is refused: a server may take `IdOnly` and only balk at the
    /// flag on top of it, and the flag is worth losing before the whole light asking is (#1784).
    pub(crate) ids_flag: bool,
    /// The read state is named as an extended property, `PR_IS_READ`, the way Outlook for Mac
    /// names it — the way Kerio answers it. It goes false for the run the first time a page
    /// answers nothing about the read state even so, or the asking itself is refused (#1784).
    pub(crate) extended: bool,
    /// The extended asking was given up for the run: said once.
    pub(crate) extended_said: bool,
    /// The run gave the naming up: refused, or the page carrying it died on the way back. It
    /// travels home on the result, and the runner remembers it for the account (#1784).
    naming_refused: bool,
}

impl<'a, E: HttpExchange> Run<'a, E> {
    pub(crate) fn new(client: &'a Client, exchange: &'a E, cancel: &'a AtomicBool, wait: &'a mut dyn FnMut(u32) -> bool) -> Self {
        Run {
            client,
            exchange,
            cancel,
            wait,
            shape: Shape::Full,
            flag_lacking: false,
            flag_by_tag: false,
            read_unsaid: false,
            own: HashSet::new(),
            reread_folders: false,
            needs_change_key: false,
            recipients_unsaid: false,
            page: PAGE,
            page_said: false,
            ids_said: false,
            ids: false,
            ids_refused_said: false,
            ids_refused: false,
            ids_flag: true,
            extended: true,
            naming_refused: false,
            extended_said: false,
        }
    }
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
                // (A transport that read no further because the answer outweighed the limit says
                // what the answer itself would have said (#1783): the page is asked smaller.)
                Error::Transport(TransportError::BodyTooLarge) => return Err(Asked::TooLarge),
                Error::Transport(_) => return Err(Asked::Stop(Stop::Unreachable)),
                Error::Server(code) => return Err(Asked::Code(code)),
                // (An answer that is none the service gives, to a request that may have been done.)
                Error::Status(_) | Error::Malformed | Error::TooLarge | Error::Unexpected if once => return Err(Asked::Lost),
                Error::Unreadable => return Err(Asked::Unreadable),
                Error::TooLarge => return Err(Asked::TooLarge),
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

    /// The folder list from `state`: all of it (a read afresh when the state is lost). A read afresh
    /// that passed over a mail folder it couldn't read says where that folder lies, so that the
    /// folder (and the mail kept of it) isn't taken away for not being in it (#1108).
    async fn folders(&mut self, state: &str, specials: &Specials) -> Result<(EwsFolderDeltas, String), Asked> {
        let (mut state, mut deltas, mut tried_afresh) =
            (state.to_owned(), EwsFolderDeltas { afresh: state.is_empty(), ..EwsFolderDeltas::default() }, false);
        let mut skipped = 0usize;
        // (What is in `unreadable_in` already, looked up in one step: a server may name thousands.)
        let mut seen = std::collections::HashSet::new();
        for _ in 0..MAX_HIERARCHY_PAGES {
            let op = SyncFolderHierarchy::new(&state);
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(page) => {
                    skipped = skipped.saturating_add(page.skipped);
                    for parent in page.unreadable {
                        // (A delta keeps none of it, and once anywhere is said nothing more is needed.)
                        if !deltas.afresh || deltas.unreadable_anywhere {
                            break;
                        }
                        // (None at all: that couldn't be read either. The root's id is the top here.)
                        if parent.is_empty() {
                            deltas.unreadable_anywhere = true;
                            continue;
                        }
                        let parent = if specials.root.as_deref() == Some(parent.as_str()) { String::new() } else { parent };
                        if seen.contains(&parent) {
                            continue;
                        }
                        if deltas.unreadable_in.len() < MAX_FOLDERS {
                            seen.insert(parent.clone());
                            deltas.unreadable_in.push(parent);
                        } else {
                            deltas.unreadable_anywhere = true;
                        }
                    }
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
                            log::info!("ews folders: {skipped} folders that are not mail folders or can't be read are left out");
                        }
                        if !deltas.afresh {
                            // (A delta removes nothing for not being in it: where they lie means nothing.)
                            (deltas.unreadable_in, deltas.unreadable_anywhere) = (Vec::new(), false);
                        } else if deltas.unreadable_anywhere {
                            log::info!("ews folders: a mail folder couldn't be read, nor where it lies; nothing is removed for not being listed");
                        } else if !deltas.unreadable_in.is_empty() {
                            log::info!(
                                "ews folders: mail folders in {} folders couldn't be read; what lies there is kept",
                                deltas.unreadable_in.len()
                            );
                        }
                        return Ok((deltas, state));
                    }
                }
                Err(Asked::Code(ResponseCode::InvalidSyncState)) if !tried_afresh => {
                    (state, tried_afresh, skipped) = (String::new(), true, 0);
                    seen.clear();
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
    let mut run = Run::new(client, exchange, cancel, wait);
    // (The naming is a probe, and a probe is only worth its cost once per server per session:
    // the runner carries the lesson from earlier runs in the request (#1784).)
    run.extended = !request.naming_refused;
    run.naming_refused = request.naming_refused;
    let mut out = Outcome::default();
    if let Err(stop) = run.all(request, &mut out, emit).await {
        match stop {
            Asked::Stop(why) => out.stop(why),
            Asked::Code(_) | Asked::Bad | Asked::TooLarge | Asked::Lost | Asked::Unreadable => {
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
        naming_refused: run.naming_refused,
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
        // (The model's cap is a thousand a run; this server reads a thousand conversations as a
        // thousand fights and drops folder reads mid-answer while it's at it — the log's
        // "couldn't be read" folders are always the ones asked after the storm started. Two
        // hundred a run: the tail works down across runs instead of burning each one (#1784,
        // measured 20:55 and 21:00).)
        let ids = ids.get(..ids.len().min(MAX_THREAD_ASKS.min(200))).unwrap_or_default();
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
            // (A remembered folder whose name is nothing but a placeholder — the `<id>` a
            // Display once leaked into a state — is no folder: the server never served it,
            // and asking for it is how a run dies at folder one of seven with six healthy
            // folders behind it. It joins nothing; a finished run lets the folder list take
            // its state away like any folder the server doesn't have (#1784, measured).)
            let placeholder = id.starts_with('<') && id.ends_with('>');
            if placeholder {
                // (Measured 17:14: the ban shipped and folder one was still the ghost — so
                // what the state actually remembers isn't what the Display prints. One log
                // with the real bytes will say what it is (#1784).)
                log::info!("ews mail: letting go of a remembered folder named {id:?}");
            }
            if !placeholder && !id.is_empty() && left_out.as_deref() != Some(id) && !read.iter().any(|have| have == id) && read.len() < MAX_FOLDERS {
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
                Err(Asked::Stop(stop)) => {
                    // (Which folder did the server die on? The line the user sees doesn't say,
                    // and a stop on folder seven of eight is a different story from a stop on
                    // folder one. One INFO, every stop, #1784.)
                    log::info!("ews mail: the server stopped answering on folder {number} of {folders} ({folder}): {stop:?}");
                    return Err(Asked::Stop(stop));
                }
                // A folder that is blocked is not a server that is failing: the other folders go on,
                // and it doesn't count towards the folders in a row (the engine says which it was).
                Err(Asked::Unreadable) => {
                    out.incomplete = true;
                    if out.blocked.len() < MAX_FOLDERS {
                        out.blocked.push(folder.clone());
                    }
                }
                // A code no version of the service is known for: that folder's own problem, not
                // a server failing — the same route as a blocked folder: the engine says which
                // by name and the other folders go on (#1784: Kerio's template-leaked `<id>`).
                Err(Asked::Code(ResponseCode::Other(code))) => {
                    out.incomplete = true;
                    log::warn!("ews mail: folder {number} of {folders} refused: {code}");
                    if out.blocked.len() < MAX_FOLDERS {
                        out.blocked.push(folder.clone());
                    }
                }
                Err(other) => {
                    out.incomplete = true;
                    log::warn!(
                        "ews mail: the changes of folder {number} of {folders} couldn't be read: {} ({})",
                        folder,
                        match &other {
                            Asked::Code(code) => code.to_string(),
                            Asked::Bad => "an answer this program can't read".to_string(),
                            Asked::Lost => "the connection died mid-answer".to_string(),
                            Asked::TooLarge => "the answer outweighed the limit".to_string(),
                            Asked::Unreadable => "the answer couldn't be taken in".to_string(),
                            _ => "the run was asked to stop".to_string(),
                        }
                    );
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
        // (Each folder asks with the usual page again: what is heavy is this folder's, and a
        // shrunk page or a light shape carried into the light folders only costs them what they
        // never earned (#1784, the review and the re-review).)
        self.page = PAGE;
        self.ids = false;
        // How many pages this folder has had refused for their weight, one after the other.
        let mut heavy = 0u8;
        // The page size this folder had when the light asking was taken up: if that asking is
        // refused, halving resumes below it — the sizes above it are what taught the lesson.
        let mut below_page = PAGE;
        // (A server older than Exchange 2013 is asked for what its schema has from the start, #1349.)
        if matches!(self.shape, Shape::Full | Shape::Lean) && !self.client.version().has_preview() {
            self.shape = Shape::Bare;
            log::info!("ews mail: the server is older than Exchange 2013; the recipients are asked message by message");
        }
        for _ in 0..MAX_PAGES {
            let mut op = SyncFolderItems::new(
                folder,
                &state,
                self.page,
                if self.ids { Shape::Ids { flag: self.shape == Shape::Full && self.ids_flag } } else { self.shape },
            );
            op.extended = self.extended;
            let page = match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(page) => {
                    if page.read_unsaid > 0 && !self.read_unsaid {
                        self.read_unsaid = true;
                        log::info!("ews mail: the server gave a read-state change that says no state; it is passed over");
                    }
                    // (Asked the read state the Outlook way too, and the answer is silent on it
                    // just the same: the asking is given up for the run, one page having proved
                    // it — a server that answers answers every page (#1784).)
                    if self.extended && !page.messages.is_empty() && !page.messages.iter().any(|m| m.server.read_said) {
                        self.extended = false;
                        if !self.extended_said {
                            self.extended_said = true;
                            log::info!("ews mail: the server says nothing of the read state even asked as an extended property; not asking again");
                        }
                    }
                    page
                }
                Err(Asked::Code(ResponseCode::FolderNotFound)) => return Ok(false),
                Err(Asked::Code(ResponseCode::InvalidSyncState)) if !lost => {
                    (state, afresh, lost) = (String::new(), true, true);
                    log::info!("ews mail: the server no longer knows a folder's sync state; reading the folder afresh");
                    continue;
                }
                // (A schema fault on a page that was asked as ids is a refusal of the ids asking
                // itself: it says nothing about what the schema shape would answer, and taking
                // it as a schema lesson would punish every folder after this one (#1784, the
                // re-reviews). The folder goes back to the schema shape and resumes halving
                // below the page that taught the size lesson — which fits where the items are
                // big but not huge (at ~1 MB an item, pages of a dozen read fine). The refusal
                // is the run's: no later folder tries the refused asking again.)
                Err(Asked::Code(
                    ResponseCode::InvalidPropertyRequest
                    | ResponseCode::InvalidPropertyForOperation
                    | ResponseCode::SchemaValidation
                    // (Measured on Kerio, twice: it refuses the light asking with a bare SOAP
                    // `Client` fault where Exchange would name a schema code — and the refusal
                    // of the light asking is a refusal of the light asking, whatever words the
                    // server chose to refuse it in (#1784).)
                    | ResponseCode::Other(_),
                )) if self.ids => {
                    // (It may be the flag the page asked on top of `IdOnly` that was refused,
                    // not the light asking itself: ask it once more without the flag, and
                    // learn for the run that this server wants the list without it (#1784).)
                    if self.shape == Shape::Full && self.ids_flag {
                        self.ids_flag = false;
                        continue;
                    }
                    if !self.ids_refused_said {
                        self.ids_refused_said = true;
                        log::info!("ews mail: the server won't take the list with the properties alone; asking with fewer changes instead");
                    }
                    self.ids = false;
                    self.ids_refused = true;
                    // (Half of the size that taught the size lesson — the sizes above it were
                    // already refused, and re-asking them only re-pays them.)
                    self.page = (below_page / 2).max(MIN_PAGE);
                    continue;
                }
                // (A refusal while the read state is also named as an extended property may be
                // a refusal of that naming — servers older than the idea know no extended
                // properties — and one page without it tells which it was before any lesson is
                // drawn from the fault (#1784).)
                // (Only a page asked from nothing can blame the naming alone: on a read that
                // rides a state, the state itself is a suspect, and the fault belongs to the
                // afresh answer below with the naming still on — the read afresh is exactly
                // where the naming earns its keep (#1784, the review). Measured on Kerio the
                // naming is refused as a schema code, with the detail block and proper
                // response code: the naming is dropped before any lesson about the SHAPE is
                // drawn from its refusal, and the shape learns its own lesson on the re-ask
                // that doesn't carry it — one refused page per run at the outside, and it
                // saves a Kerio from a preview lesson it was never meant to teach.)
                Err(Asked::Code(ResponseCode::InvalidPropertyRequest | ResponseCode::InvalidPropertyForOperation | ResponseCode::SchemaValidation))
                    if self.extended && !self.ids =>
                {
                    self.extended = false;
                    self.naming_refused = true;
                    if !self.extended_said {
                        self.extended_said = true;
                        log::info!("ews mail: the server won't take the read state asked as an extended property; not asking again");
                    }
                    continue;
                }
                // (Measured on Kerio at 16:15: the page carrying the naming doesn't come back
                // refused — it comes back as NOTHING, the connection dropped mid-answer. A
                // transport that dies on the naming's first ride is read as the naming's
                // doing: it is dropped and the page asked again without it, once, before the
                // network is believed. State and shape are no suspects of a dropped socket,
                // so this doesn't wait for the page to be asked from nothing — and if the
                // network really is dead, the re-ask without the naming says so the same way
                // it always did (#1784, measured).)
                Err(Asked::Stop(Stop::Unreachable) | Asked::Lost | Asked::Bad) if self.extended => {
                    self.extended = false;
                    self.naming_refused = true;
                    if !self.extended_said {
                        self.extended_said = true;
                        log::info!("ews mail: a page asked with the read state named the MAPI way died on the way back; not naming it again");
                    }
                    // (Measured at 16:41: the re-asked page died too, 50ms later — a servlet
                    // killed mid-answer needs a moment to come back, and a connection that
                    // dies again right away was never about the naming. Three seconds, once
                    // per run, before the network is believed.)
                    if !(self.wait)(3000) {
                        return Err(Asked::Stop(Stop::Cancelled));
                    }
                    continue;
                }
                // (A refusal in SOAP's own words, from a page asked from nothing, is the
                // naming's too — the guard on the state is what separates it from the lost
                // state the afresh answer below is for (#1784, the review).)
                Err(Asked::Code(ResponseCode::Other(_))) if self.extended && !self.ids && state.is_empty() => {
                    self.extended = false;
                    self.naming_refused = true;
                    if !self.extended_said {
                        self.extended_said = true;
                        log::info!("ews mail: the server won't take the read state asked as an extended property; not asking again");
                    }
                    continue;
                }
                // (A fault in SOAP's own blaming-the-client words, on a read that goes on from a
                // state, may well be the server losing that state in words no version of Exchange
                // would use: the folder is read afresh once before it costs anything, which is
                // exactly what a named lost state does (the re-review: a bare `Server` fault is
                // the server blaming itself and likely temporary — that one doesn't earn having
                // its state thrown away; it falls through to the folder's problem below).
                // Kerio answers refused asks as a bare SOAP `Client` fault (#1784, measured).)
                Err(Asked::Code(ResponseCode::Other(ref words)))
                    if !lost && !state.is_empty() && (words.as_str() == "Client" || words.as_str() == "Sender") =>
                {
                    (state, afresh, lost) = (String::new(), true, true);
                    log::info!("ews mail: the server refused a folder's own asking; it is read afresh");
                    continue;
                }
                // (A schema fault is read as a missing property only from a server older than Exchange
                // 2013: a newer one's has another cause, and isn't hidden by asking for less.)
                Err(Asked::Code(code @ (ResponseCode::InvalidPropertyRequest | ResponseCode::SchemaValidation)))
                    if self.shape == Shape::Full && (code != ResponseCode::SchemaValidation || !self.client.version().has_preview()) =>
                {
                    self.shape = Shape::Lean;
                    heavy = 0;
                    log::info!("ews mail: the server lacks the preview or the flag; asking for the rest");
                    continue;
                }
                Err(Asked::Code(code @ (ResponseCode::InvalidPropertyForOperation | ResponseCode::SchemaValidation)))
                    if matches!(self.shape, Shape::Full | Shape::Lean)
                        && (code != ResponseCode::SchemaValidation || !self.client.version().has_preview()) =>
                {
                    self.shape = Shape::Bare;
                    heavy = 0;
                    log::info!("ews mail: the server won't list the recipients with the changes; they are asked message by message");
                    continue;
                }
                // (A server that answers the page heavier than any answer this program reads —
                // Kerio Connect's servlet among them, which hangs whole messages on the changes
                // when only the list was asked for (#964)): the first time is asked with fewer
                // changes at a time; a second refusal says the weight is in the items and not in
                // their number, and this folder then asks its list over the light base shape,
                // from the usual page again, headers being light (#1784). A folder still
                // outweighed there costs its one problem, as any answer that can't be read.)
                // (A delta that outweighs the reader even at the smallest page is a server that
                // can't say what happened since the state in anything this program can read:
                // measured on Kerio at 21:29, where the heavy folders' deltas — ten thousand
                // read-flag changes deep — outweigh the reader however small the page is asked,
                // while their afresh listings page fine. A state that can't be told is a state
                // lost, whatever the words were; the folder is read afresh, once, before it
                // costs its one problem (#1784).)
                Err(Asked::TooLarge) if !lost && !state.is_empty() && self.page == MIN_PAGE => {
                    (state, afresh, lost) = (String::new(), true, true);
                    self.ids = false;
                    self.page = PAGE;
                    heavy = 0;
                    log::info!("ews mail: the server can't tell what changed; the folder is read afresh");
                    continue;
                }
                Err(Asked::TooLarge) => {
                    if self.ids {
                        // (Half as many again, from the fresh page: headers being few and light,
                        // a folder still outweighed at the last is the folder's one problem.)
                        if self.page > MIN_PAGE {
                            self.page = (self.page / 2).max(MIN_PAGE);
                        } else {
                            run_out_of_asking(&self.shape, self.ids, self.page, &state, lost);
                            return Err(Asked::TooLarge);
                        }
                    } else {
                        heavy = heavy.saturating_add(1);
                        if heavy == 1 && self.page > MIN_PAGE {
                            self.page = (self.page / 2).max(MIN_PAGE);
                            if !self.page_said {
                                self.page_said = true;
                                log::info!("ews mail: the server's answers are too big to read; asking for fewer changes at a time");
                            }
                        } else if !self.ids_refused && self.shape != Shape::Bare {
                            // (Never from `Bare`: over the sparse base it already asks every
                            // byte the ids asking would, and an identical request can teach
                            // nothing.)
                            self.ids = true;
                            below_page = self.page;
                            self.page = PAGE;
                            if !self.ids_said {
                                self.ids_said = true;
                                log::info!("ews mail: whole messages come with the changes; the list is asked with the properties alone");
                            }
                        } else if self.page > MIN_PAGE {
                            // (The light asking is refused by this server: halving goes on past
                            // where the ids lesson would have taken over.)
                            self.page = (self.page / 2).max(MIN_PAGE);
                        } else {
                            run_out_of_asking(&self.shape, self.ids, self.page, &state, lost);
                            return Err(Asked::TooLarge);
                        }
                    }
                    continue;
                }
                Err(other) => return Err(other),
            };
            heavy = 0;
            read = read.saturating_add(u32::try_from(page.messages.len()).unwrap_or(u32::MAX));
            let mut messages = page.messages;
            self.recipients(&mut messages).await?;
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

impl<E: HttpExchange> Run<'_, E> {
    /// The recipients of the `messages` that came without any, asked with `GetItem`
    /// ([`GetItemRecipients`]): every message of a server whose `SyncFolderItems` won't give them
    /// ([`Shape::Bare`]), and those a server left them out of (#1151). What stops the run stops it
    /// here too (the page is then not taken in, and is read again); anything else the server says
    /// leaves those messages without recipients, said once a run.
    async fn recipients(&mut self, messages: &mut [GraphMessage]) -> Result<(), Asked> {
        let without = |m: &&GraphMessage| m.to.is_empty() && m.cc.is_empty();
        let ids: Vec<String> = messages.iter().filter(without).map(|m| m.server.id.clone()).collect();
        if ids.is_empty() {
            return Ok(());
        }
        let at: HashMap<String, usize> = messages.iter().enumerate().map(|(n, m)| (m.server.id.clone(), n)).collect();
        let mut short = false;
        for some in ids.chunks(MAX_RECIPIENTS_ASKED).take(MAX_RECIPIENT_ASKS) {
            let op = GetItemRecipients::new(some);
            if op.is_empty() {
                continue;
            }
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(found) => {
                    short |= found.len() < some.len();
                    for one in found {
                        if let Some(message) = at.get(&one.id).and_then(|n| messages.get_mut(*n)) {
                            (message.to, message.cc) = (one.to, one.cc);
                        }
                    }
                }
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => short = true,
            }
        }
        short |= ids.len() > MAX_RECIPIENTS_ASKED.saturating_mul(MAX_RECIPIENT_ASKS);
        if short && !self.recipients_unsaid {
            self.recipients_unsaid = true;
            log::info!("ews mail: the server didn't give the recipients of some messages; they are listed without them");
        }
        Ok(())
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
    let mut run = Run::new(client, exchange, cancel, wait);
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..GraphFetchResult::default() };
    let mut outcome = Outcome::default();
    for id in request.ids.iter().take(MAX_FETCH) {
        let op = GetItemMime::new(id);
        match run.ask(&op.element(), &|reply| op.answer(reply)).await {
            Ok(mime) => out.fetched.push(GraphFetched { id: id.clone(), mime }),
            Err(Asked::Code(ResponseCode::ItemNotFound)) => out.missing.push(id.clone()),
            Err(Asked::Code(_) | Asked::Bad | Asked::TooLarge | Asked::Lost | Asked::Unreadable) => out.failed.push(id.clone()),
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
