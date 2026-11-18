// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run: mail down from a Zimbra account, over any [`HttpExchange`], handed back in pieces.
//!
//! 1. **Sign in** with the password the host hands over beside the request (`AuthRequest`).
//! 2. **The folders**, as a complete listing (`GetFolderRequest`): one piece.
//! 3. **The messages.** With a change token, what changed since (`SyncRequest`, a page at a time):
//!    the messages new or changed are read as headers and a preview by id (`SearchRequest`), and
//!    each page is a piece of changes. Without one, or when the server can no longer go on from it
//!    (`mail.MUST_RESYNC`), a token is asked for first and then every folder is read from its start,
//!    newest first, the Inbox first: each page of a folder is a piece, the first of them marked as
//!    the start of a fresh read and the last as its end.
//! 4. **The last piece** carries the token to go on from, only when everything the run meant to
//!    read was read: a run cut short leaves the mailbox at the token it had, and the next run reads
//!    again what this one didn't finish.
//!
//! Every piece says the token the run went on from, so that the engine never takes a piece in for
//! a mailbox that has moved on meanwhile. Nothing a server wrote reaches a piece's lines.

use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{
    FolderListing, GraphFetchResult, GraphFetched, GraphMessage, MAX_LISTED_FOLDERS, ZimbraFetchRequest, ZimbraMailRequest, ZimbraMailResult,
    ZimbraMessagePage, ZimbraResume,
};

use crate::error::Error;
use crate::http::{HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};
use crate::{auth, delta, fetch, folders, items};

/// Most pages of one folder read in a run (at [`items::PAGE`] each), and most rounds of a delta.
const MAX_FOLDER_PAGES: usize = 800;
const MAX_ROUNDS: usize = 400;
/// Most messages fetched whole in one fetch.
const MAX_FETCH: usize = 100;
/// Most bytes of MIME one fetch holds in memory at once: at [`crate::http::RESPONSE_LIMIT`] a
/// message, a hundred of them would be more than three gigabytes. What doesn't fit is fetched by
/// the fetch that asks again for what this one left.
const MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

/// Why a run stopped before its end.
enum Stop {
    /// The owner called it off.
    Cancelled,
    /// Something went wrong that ends the run: said in the last piece.
    Failed(Error),
}

/// A run under way.
struct Run<'a, E> {
    exchange: &'a E,
    cancel: &'a AtomicBool,
    base: String,
    token: Secret,
}

impl<E: HttpExchange> Run<'_, E> {
    /// One exchange, unless the run was called off.
    async fn ask(&self, request: Result<HttpRequest, Error>) -> Result<HttpResponse, Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let request = request.map_err(Stop::Failed)?;
        match self.exchange.exchange(&request).await {
            Ok(response) => Ok(response),
            Err(TransportError::Aborted) => Err(Stop::Cancelled),
            Err(e) => Err(Stop::Failed(Error::Transport(e))),
        }
    }
}

/// The piece every piece of the run starts from. `from` is the token the run goes on from, or
/// empty when it reads afresh: an unusable token is never echoed (a piece's check would refuse it
/// whole, and the account would stay at the cursor that fails forever).
fn blank(request: &ZimbraMailRequest) -> ZimbraMailResult {
    ZimbraMailResult {
        account: request.account,
        email: request.email.clone(),
        url: request.url.clone(),
        from: if mailcraft_model::ZimbraServer::token_ok(&request.token) { request.token.clone() } else { String::new() },
        ..ZimbraMailResult::default()
    }
}

/// The name a run signs in with: the settings', else the account's address.
fn login(username: &str, email: &str) -> String {
    if username.trim().is_empty() { email.trim().to_string() } else { username.trim().to_string() }
}

/// When a folder's turn comes: the Inbox, then sent mail and drafts, then the rest.
fn turn(kind: FolderKind) -> u8 {
    match kind {
        FolderKind::Inbox => 0,
        FolderKind::Sent => 1,
        FolderKind::Drafts => 2,
        FolderKind::Archive => 3,
        FolderKind::Deleted => 4,
        FolderKind::Junk => 5,
        FolderKind::Outbox | FolderKind::Custom => 6,
    }
}

/// Read mail down from the account `request` is for, signing in with `password`, and hand the run
/// to `emit` in pieces (`partial` on all but the last, which always comes). Setting `cancel` calls
/// the run off before its next request.
pub async fn sync_mail<E: HttpExchange>(
    exchange: &E,
    request: &ZimbraMailRequest,
    password: &Secret,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(ZimbraMailResult),
) {
    let mut last = blank(request);
    match run(exchange, request, password, cancel, emit).await {
        Ok((token, resume)) => (last.token, last.resume, last.whole) = (token, Some(resume), true),
        Err(Stop::Cancelled) => last.stopped = true,
        Err(Stop::Failed(e)) => {
            match e {
                Error::AuthRefused | Error::TwoFactor => last.auth_refused = true,
                Error::Busy(seconds) => last.busy_for = Some(seconds),
                _ => {}
            }
            last.errors.push(format!("Couldn't send and receive with the Zimbra server: {e}"));
        }
    }
    emit(last);
}

/// The run itself, up to the token to go on from and where each folder's unfinished fresh read
/// goes on from (empty when none was left unfinished).
async fn run<E: HttpExchange>(
    exchange: &E,
    request: &ZimbraMailRequest,
    password: &Secret,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<(String, Vec<ZimbraResume>), Stop> {
    let base = request.url.trim().trim_end_matches('/').to_string();
    let mut run = Run { exchange, cancel, base, token: Secret::new("") };
    let user = login(&request.username, &request.email);
    let signed_in = run.ask(auth::request(&run.base, &user, password)).await?;
    run.token = auth::answer(&signed_in).map_err(Stop::Failed)?;
    let token_ok = !request.token.is_empty() && mailcraft_model::ZimbraServer::token_ok(&request.token);
    if token_ok {
        let listing = folder_listing(&run, request, emit).await?;
        match changes(&run, request, &listing, emit).await {
            Err(Stop::Failed(Error::MustResync)) => {}
            done => {
                let since = done?;
                let resume = resume_folders(&run, request, &listing, emit).await?;
                return Ok((since, resume));
            }
        }
    }
    afresh(&run, request, emit).await
}

/// The server's folders of mail, as a piece, and for the run itself: an incomplete listing
/// answers for nothing (a folder left out of it would have its messages taken for deleted, so a
/// run on it never keeps a token).
async fn folder_listing<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<folders::Listing, Stop> {
    let listed = run.ask(folders::request(&run.base, &run.token)).await?;
    let listing = folders::answer(&listed).map_err(Stop::Failed)?;
    let mut piece = blank(request);
    piece.listing = Some(FolderListing { folders: listing.folders.clone(), complete: listing.complete });
    piece.well_known = listing.well_known.clone();
    piece.partial = true;
    emit(piece);
    if listing.complete { Ok(listing) } else { Err(Stop::Failed(Error::Unreadable)) }
}

/// What changed since the request's token, page by page. Answers the token to go on from.
async fn changes<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    listing: &folders::Listing,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<String, Stop> {
    let mut listing = listing.clone();
    let mut relisted = false;
    let mut since = request.token.clone();
    for _ in 0..MAX_ROUNDS {
        let answered = run.ask(delta::request(&run.base, &run.token, &since)).await?;
        let page = delta::answer(&answered, false).map_err(Stop::Failed)?;
        let mut messages: Vec<GraphMessage> = Vec::new();
        let mut named: Vec<String> = Vec::new();
        for ids in page.changed.chunks(items::BY_ID) {
            named.extend(ids.iter().cloned());
            let found = run.ask(items::by_id(&run.base, &run.token, ids)).await?;
            messages.extend(items::answer(&found).map_err(Stop::Failed)?.messages);
        }
        // A message that lies in a folder no listing knows may have gone to one made while the run
        // has been going on: the listing is asked for once more, then the folder is known for
        // certain — a message in one it still doesn't name (the chat history, a calendar) has
        // left the mail.
        let mut mail: std::collections::HashSet<&str> = listing.folders.iter().map(|f| f.id.as_str()).collect();
        if !relisted && messages.iter().any(|m| !mail.contains(m.server.folder.as_str())) {
            relisted = true;
            let listed = run.ask(folders::request(&run.base, &run.token)).await?;
            let again = folders::answer(&listed).map_err(Stop::Failed)?;
            if !again.complete {
                return Err(Stop::Failed(Error::Unreadable));
            }
            let mut piece = blank(request);
            piece.listing = Some(FolderListing { folders: again.folders.clone(), complete: true });
            piece.well_known = again.well_known.clone();
            piece.partial = true;
            emit(piece);
            listing = again;
            mail = listing.folders.iter().map(|f| f.id.as_str()).collect();
        }
        // An id the delta named that no message came back for was either deleted between the two
        // answers, or is no message at all: either way nothing of it stays, and it is dropped by
        // name rather than by silence.
        let returned: std::collections::HashSet<&str> = messages.iter().map(|m| m.server.id.as_str()).collect();
        let missing: Vec<String> = named.into_iter().filter(|id| !returned.contains(id.as_str())).collect();
        let (messages, gone): (Vec<GraphMessage>, Vec<GraphMessage>) = messages.into_iter().partition(|m| mail.contains(m.server.folder.as_str()));
        let mut removed = page.deleted.clone();
        removed.extend(gone.into_iter().map(|m| m.server.id));
        removed.extend(missing);
        let mut piece = blank(request);
        piece.pages.push(ZimbraMessagePage { messages, removed, last: !page.more, ..ZimbraMessagePage::default() });
        piece.partial = true;
        emit(piece);
        since = page.token;
        if !page.more {
            return Ok(since);
        }
    }
    // More changes wait than one run reads: the token of the last whole page is kept, and the next
    // run goes on from it, so a mailbox that changes faster than a run follows still moves on.
    Ok(since)
}

/// One folder's read to its end, or as far as the page cap lets it go. `start` is where a run
/// before this one left off (`None`: from its start, the first page marked `afresh`); the answer
/// is where to go on from when the cap was reached, or `None` when the folder is done.
async fn folder_tail<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    folder: &str,
    start: Option<(String, u64)>,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<Option<ZimbraResume>, Stop> {
    let fresh = start.is_none();
    let mut cursor = start;
    for page_number in 0..MAX_FOLDER_PAGES {
        let answered = run.ask(items::folder_page(&run.base, &run.token, folder, cursor.as_ref().map(|(id, sort)| (id.as_str(), *sort)))).await?;
        let page = items::answer(&answered).map_err(Stop::Failed)?;
        // A page that says more and neither brings a message nor a cursor to follow is no
        // page to go on from: without one, whatever lies behind it is never read.
        if page.more && (page.messages.is_empty() || page.cursor.is_none()) {
            return Err(Stop::Failed(Error::Unreadable));
        }
        let last = !page.more;
        cursor = page.cursor;
        let mut piece = blank(request);
        piece.pages.push(ZimbraMessagePage {
            folder: folder.to_string(),
            messages: page.messages,
            afresh: page_number == 0 && fresh,
            last,
            ..ZimbraMessagePage::default()
        });
        piece.partial = true;
        emit(piece);
        if last {
            return Ok(None);
        }
        if page_number + 1 == MAX_FOLDER_PAGES {
            // A folder too large for one run: where it stopped is kept with the run's token, and
            // the next run goes on from there rather than failing this one again from its start.
            let (id, sort) = cursor.unwrap_or_default();
            return Ok(Some(ZimbraResume { folder: folder.to_string(), cursor: id, sort }));
        }
    }
    Ok(None)
}

/// The folders whose fresh read a run before this one had to stop in the middle of (`request`'s
/// `resume`), read to their end — or to where they stop again, which is handed back.
async fn resume_folders<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    listing: &folders::Listing,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<Vec<ZimbraResume>, Stop> {
    let mut left = Vec::new();
    for entry in request.resume.iter().take(MAX_LISTED_FOLDERS) {
        // A folder no longer in the listing is gone, and its read with it.
        if !listing.folders.iter().any(|f| f.id == entry.folder) {
            continue;
        }
        let start = if entry.cursor.is_empty() { None } else { Some((entry.cursor.clone(), entry.sort)) };
        if let Some(again) = folder_tail(run, request, &entry.folder, start, emit).await? {
            left.push(again);
        }
    }
    Ok(left)
}

/// Every folder read from its start. The token is asked for **before** the folders are listed,
/// so that what another program does while the folders are read is part of the delta that goes on
/// from the run, not of a read that has already passed it by.
async fn afresh<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<(String, Vec<ZimbraResume>), Stop> {
    let started = run.ask(delta::request(&run.base, &run.token, "")).await?;
    let token = delta::answer(&started, true).map_err(Stop::Failed)?.token;
    let listing = folder_listing(run, request, emit).await?;
    let mut order: Vec<&mailcraft_model::sync::GraphFolder> = listing.folders.iter().collect();
    order.sort_by_key(|f| turn(f.kind));
    // Where a run before this one stopped inside a folder: go on from there. An entry whose
    // folder is no longer listed answers for nothing.
    let mut pending: std::collections::HashMap<String, ZimbraResume> = request
        .resume
        .iter()
        .take(MAX_LISTED_FOLDERS)
        .filter(|e| listing.folders.iter().any(|f| f.id == e.folder))
        .map(|e| (e.folder.clone(), e.clone()))
        .collect();
    let mut left: Vec<ZimbraResume> = Vec::new();
    let mut capped = false;
    for folder in order {
        if capped {
            // A folder this run never reached goes on from where the run before left it, or from
            // its beginning when no run ever started it.
            left.push(pending.remove(&folder.id).unwrap_or_else(|| ZimbraResume { folder: folder.id.clone(), ..ZimbraResume::default() }));
            continue;
        }
        let start = pending.remove(&folder.id).and_then(|e| if e.cursor.is_empty() { None } else { Some((e.cursor, e.sort)) });
        if let Some(again) = folder_tail(run, request, &folder.id, start, emit).await? {
            left.push(again);
            capped = true;
        }
    }
    Ok((token, left))
}

/// Fetch the messages `request` names whole, signing in with `password`.
pub async fn fetch_messages<E: HttpExchange>(exchange: &E, request: &ZimbraFetchRequest, password: &Secret, cancel: &AtomicBool) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..GraphFetchResult::default() };
    let base = request.url.trim().trim_end_matches('/').to_string();
    let mut run = Run { exchange, cancel, base, token: Secret::new("") };
    let stop = |out: &mut GraphFetchResult, stop: Stop| {
        if let Stop::Failed(e) = stop {
            if let Error::Busy(seconds) = e {
                out.busy_for = Some(seconds);
            }
            out.offline = matches!(e, Error::Transport(TransportError::Unreachable | TransportError::TimedOut));
            out.errors.push(format!("Couldn't download messages from the Zimbra server: {e}"));
        }
    };
    let signed_in = match run.ask(auth::request(&run.base, &login(&request.username, &request.email), password)).await {
        Ok(signed_in) => signed_in,
        Err(e) => {
            stop(&mut out, e);
            return out;
        }
    };
    match auth::answer(&signed_in) {
        Ok(token) => run.token = token,
        Err(e) => {
            stop(&mut out, Stop::Failed(e));
            return out;
        }
    }
    let mut held: u64 = 0;
    for id in request.ids.iter().take(MAX_FETCH) {
        if held >= MAX_FETCH_BYTES {
            break;
        }
        // An id that is no item of the mailbox's own is one the server doesn't have.
        let asked = fetch::request(&run.base, &run.token, id);
        if matches!(asked, Err(Error::NoSuchItem)) {
            out.missing.push(id.clone());
            continue;
        }
        let answer = match run.ask(asked).await {
            Ok(response) => fetch::answer(&response),
            Err(Stop::Failed(Error::Transport(TransportError::BodyTooLarge))) => {
                out.too_large.push(id.clone());
                out.failed.push(id.clone());
                continue;
            }
            Err(e) => {
                stop(&mut out, e);
                return out;
            }
        };
        match answer {
            Ok(mime) => {
                held = held.saturating_add(mime.len() as u64);
                out.fetched.push(GraphFetched { id: id.clone(), mime });
            }
            Err(Error::NoSuchItem) => out.missing.push(id.clone()),
            Err(e @ (Error::AuthRefused | Error::Busy(_))) => {
                stop(&mut out, Stop::Failed(e));
                return out;
            }
            Err(_) => out.failed.push(id.clone()),
        }
    }
    out
}
