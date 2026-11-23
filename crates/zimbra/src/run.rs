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
//! 4. **The last piece** carries the token to go on from. A run that failed or was called off
//!    carries none: the mailbox stays at the token it had, and the next run reads again what this
//!    one didn't finish. A run that only reached one of its caps (more pages of changes, or of a
//!    folder, than one run reads) is not `whole` but does carry one: the token of the last page
//!    it read whole, with where each half-read folder goes on from (`resume`). The two belong
//!    together and are kept together, whether the run was whole or not.
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

/// Most pages of one folder read in a run (at [`items::PAGE`] each), most rounds of a delta, and
/// most times the folder listing is asked for again when a change names a folder it doesn't know.
#[cfg(not(test))]
const MAX_FOLDER_PAGES: usize = 800;
#[cfg(test)]
const MAX_FOLDER_PAGES: usize = 4;
#[cfg(not(test))]
const MAX_ROUNDS: usize = 400;
#[cfg(test)]
const MAX_ROUNDS: usize = 3;
#[cfg(not(test))]
const MAX_RELISTS: usize = 4;
#[cfg(test)]
const MAX_RELISTS: usize = 2;
/// Most messages fetched whole in one fetch.
const MAX_FETCH: usize = 100;
/// Most bytes of MIME one fetch hands back: at [`crate::http::RESPONSE_LIMIT`] a message, a
/// hundred of them would be more than three gigabytes. What doesn't fit is fetched by the fetch
/// that asks again for what this one left. (While the answer that turns out not to fit is being
/// read, it is held beside these: at most one message's [`crate::http::RESPONSE_LIMIT`] more.)
const MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

/// Why a run stopped before its end.
enum Stop {
    /// The owner called it off.
    Cancelled,
    /// Something went wrong that ends the run: said in the last piece.
    Failed(Error, Phase),
}

#[derive(Clone, Copy)]
enum Phase {
    SignIn,
    Folders,
    Changes,
    Messages,
    Probe,
    Body,
}

impl Phase {
    fn failed(self, error: Error) -> Stop {
        Stop::Failed(error, self)
    }

    fn text(self) -> &'static str {
        match self {
            Self::SignIn => "signing in",
            Self::Folders => "listing folders",
            Self::Changes => "reading mailbox changes",
            Self::Messages => "reading message headers",
            Self::Probe => "checking a message",
            Self::Body => "downloading message bodies",
        }
    }
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
    async fn ask(&self, phase: Phase, request: Result<HttpRequest, Error>) -> Result<HttpResponse, Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let request = request.map_err(|e| phase.failed(e))?;
        match self.exchange.exchange(&request).await {
            Ok(response) => Ok(response),
            Err(TransportError::Aborted) => Err(Stop::Cancelled),
            Err(e) => Err(phase.failed(Error::Transport(e))),
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
    // What is to go up is not sent by this run yet. Asked to, it says so before the server is
    // asked anything: read down and called whole, it would move the token past changes it left
    // unsent, and the server's state would be read back over them.
    if !(request.changes.is_empty() && request.messages.is_empty() && request.drafts.is_empty() && request.sends.is_empty()) {
        last.errors.push(
            "Couldn't send and receive with the Zimbra server: what was changed or written here can't be sent to a Zimbra server yet".to_string(),
        );
        emit(last);
        return;
    }
    match run(exchange, request, password, cancel, emit).await {
        Ok((token, resume, whole)) => (last.token, last.resume, last.whole) = (token, Some(resume), whole),
        Err(Stop::Cancelled) => last.stopped = true,
        Err(Stop::Failed(e, phase)) => {
            match e {
                Error::AuthRefused | Error::TwoFactor => last.auth_refused = true,
                Error::Busy(seconds) => last.busy_for = Some(seconds),
                _ => {}
            }
            last.errors.push(format!("Couldn't send and receive with the Zimbra server while {}: {e}", phase.text()));
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
) -> Result<(String, Vec<ZimbraResume>, bool), Stop> {
    let base = request.url.trim().trim_end_matches('/').to_string();
    let mut run = Run { exchange, cancel, base, token: Secret::new("") };
    let user = login(&request.username, &request.email);
    let signed_in = run.ask(Phase::SignIn, auth::request(&run.base, &user, password)).await?;
    run.token = auth::answer(&signed_in).map_err(|e| Phase::SignIn.failed(e))?;
    let token_ok = !request.token.is_empty() && mailcraft_model::ZimbraServer::token_ok(&request.token);
    if token_ok {
        let listing = folder_listing(&run, request, emit).await?;
        match changes(&run, request, &listing, emit).await {
            Err(Stop::Failed(Error::MustResync, _)) => {}
            done => {
                let (since, listing, complete) = done?;
                let resume = resume_folders(&run, request, &listing, emit).await?;
                let whole = complete && resume.is_empty();
                return Ok((since, resume, whole));
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
    let listed = run.ask(Phase::Folders, folders::request(&run.base, &run.token)).await?;
    let listing = folders::answer(&listed).map_err(|e| Phase::Folders.failed(e))?;
    let mut piece = blank(request);
    piece.listing = Some(FolderListing { folders: listing.folders.clone(), complete: listing.complete });
    piece.well_known = listing.well_known.clone();
    piece.partial = true;
    emit(piece);
    if listing.complete { Ok(listing) } else { Err(Phase::Folders.failed(Error::Unreadable)) }
}

/// What changed since the request's token, page by page. Answers the token to go on from.
async fn changes<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    listing: &folders::Listing,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<(String, folders::Listing, bool), Stop> {
    let mut listing = listing.clone();
    let mut relisted = 0;
    // A folder a listing was asked about again and still doesn't name is no folder of mail — the
    // chat history, a calendar: messages in it have left the mail, and no page after this one asks
    // about it again.
    let mut not_mail: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut since = request.token.clone();
    for page_index in 0..MAX_ROUNDS {
        let answered = run.ask(Phase::Changes, delta::request(&run.base, &run.token, &since)).await?;
        let page = delta::answer(&answered, false).map_err(|e| Phase::Changes.failed(e))?;
        if page.more && page.token == since {
            return Err(Phase::Changes.failed(Error::Unreadable));
        }
        let mut messages: Vec<GraphMessage> = Vec::new();
        let mut named: Vec<String> = Vec::new();
        let mut probed_gone: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ids in page.changed.chunks(items::BY_ID) {
            named.extend(ids.iter().cloned());
            let (found, gone) = messages_by_id(run, ids).await?;
            messages.extend(found);
            probed_gone.extend(gone);
        }
        // A message that lies in a folder no listing knows may have gone to one made while the run
        // has been going on: the listing is asked for again, then the folder is known for
        // certain — a message in one it still doesn't name (the chat history, a calendar) has
        // left the mail. Asked for again as often as new folders turn up, up to a cap: past it the
        // run keeps the token of the last whole page, so folders a mailbox gains faster than this
        // follows are caught up with by the run after, instead of failing from the start for ever.
        let mut mail: std::collections::HashSet<&str> = listing.folders.iter().map(|f| f.id.as_str()).collect();
        while messages.iter().any(|m| !mail.contains(m.server.folder.as_str()) && !not_mail.contains(m.server.folder.as_str())) {
            // A folder that is no folder of mail is never in the listing: asking again for it for
            // ever would end the run, so a listing that brings no new folder is the last word —
            // what still isn't known has left the mail, and is asked about no more.
            let before: std::collections::HashSet<String> = mail.iter().map(|id| id.to_string()).collect();
            relisted += 1;
            if relisted > MAX_RELISTS {
                if page_index == 0 {
                    return Err(Phase::Folders.failed(Error::Unreadable));
                }
                return Ok((since, listing, false));
            }
            let listed = run.ask(Phase::Folders, folders::request(&run.base, &run.token)).await?;
            let again = folders::answer(&listed).map_err(|e| Phase::Folders.failed(e))?;
            if !again.complete {
                return Err(Phase::Folders.failed(Error::Unreadable));
            }
            let mut piece = blank(request);
            piece.listing = Some(FolderListing { folders: again.folders.clone(), complete: true });
            piece.well_known = again.well_known.clone();
            piece.partial = true;
            emit(piece);
            listing = again;
            mail = listing.folders.iter().map(|f| f.id.as_str()).collect();
            // The ids, not their count: one folder made while another was deleted between the two
            // listings leaves the count unchanged.
            if mail.iter().all(|id| before.contains(*id)) {
                not_mail.extend(messages.iter().map(|m| m.server.folder.clone()).filter(|f| !mail.contains(f.as_str())));
                break;
            }
        }
        // An id the delta named that no message came back for can't be taken for gone on the
        // search's word alone: the message may still be there while the search leaves it out (an
        // index behind, a scope the server reads differently). Stopping without the token loses
        // nothing — the next run asks for the same delta, and a message that really was deleted
        // turns up in its `deleted` list. Only an id the server itself answered for by name — an
        // a `GetMsgRequest` probe answering `mail.NO_SUCH_MSG` — counts as gone: anything else
        // would end the run on the same page for ever once one id is truly deleted.
        let returned: std::collections::HashSet<&str> = messages.iter().map(|m| m.server.id.as_str()).collect();
        if named.iter().any(|id| !returned.contains(id.as_str()) && !probed_gone.contains(id.as_str())) {
            return Err(Phase::Messages.failed(Error::Unreadable));
        }
        let (messages, gone): (Vec<GraphMessage>, Vec<GraphMessage>) = messages.into_iter().partition(|m| mail.contains(m.server.folder.as_str()));
        let mut removed = page.deleted.clone();
        removed.extend(gone.into_iter().map(|m| m.server.id));
        removed.extend(probed_gone);
        let mut piece = blank(request);
        piece.pages.push(ZimbraMessagePage { messages, removed, last: !page.more, ..ZimbraMessagePage::default() });
        piece.partial = true;
        emit(piece);
        since = page.token;
        if !page.more {
            return Ok((since, listing, true));
        }
    }
    // More changes wait than one run reads: the token of the last whole page is kept, and the next
    // run goes on from it, so a mailbox that changes faster than a run follows still moves on.
    Ok((since, listing, false))
}

/// The messages `ids` names: one request for the chunk, unless the server faults it as holding a
/// name it no longer has — then each id is asked for by itself, so that a message deleted between
/// the delta and the read is left out and not the whole run's end. With it come the ids the
/// server itself answered for by name — gone for certain, which an id a search left out alone
/// never is.
async fn messages_by_id<E: HttpExchange>(run: &Run<'_, E>, ids: &[String]) -> Result<(Vec<GraphMessage>, Vec<String>), Stop> {
    let found = run.ask(Phase::Messages, items::by_id(&run.base, &run.token, ids)).await?;
    let mut messages = match items::answer(&found) {
        // (Each message once: a copy of one would be a second message of the page, or a second
        // removal, and enough of them a page past what is taken in.)
        Ok(page)
            if page.messages.iter().all(|m| ids.contains(&m.server.id))
                && page.messages.iter().map(|m| m.server.id.as_str()).collect::<std::collections::HashSet<_>>().len() == page.messages.len() =>
        {
            page.messages
        }
        Ok(_) => return Err(Phase::Messages.failed(Error::Unreadable)),
        // The chunk is faulted for an id it no longer has — or for `is:anywhere` walking a folder
        // the server already dropped, which faults any id.
        Err(Error::NoSuchItem) => Vec::new(),
        Err(e) => return Err(Phase::Messages.failed(e)),
    };
    // Every id the batch didn't return is asked one at a time by `GetMsgRequest`, whether the
    // batch faulted for it or quietly left it out — an index that's behind can do either, and a
    // quiet drop without a probe would end the run here for ever. The request names the item
    // itself wherever it lies, Trash and Junk included. Its `mail.NO_SUCH_MSG` is told to the
    // caller as gone: the server's own word, which an empty search page never is (a scope can
    // leave a folder out). Any other fault ends the run as it does for a name the search leaves
    // out.
    let have: std::collections::HashSet<String> = messages.iter().map(|m| m.server.id.clone()).collect();
    let mut gone = Vec::new();
    for id in ids.iter().filter(|id| !have.contains(id.as_str())) {
        let one = run.ask(Phase::Probe, items::probe(&run.base, &run.token, id)).await?;
        match items::probe_answer(&one, id) {
            Ok(Some(message)) => messages.push(message),
            Ok(None) => gone.push(id.clone()),
            Err(e) => return Err(Phase::Probe.failed(e)),
        }
    }
    Ok((messages, gone))
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
    // Every cursor this read has gone on from, the one it began at included.
    let mut seen: std::collections::HashSet<(String, u64)> = cursor.iter().cloned().collect();
    for page_number in 0..MAX_FOLDER_PAGES {
        let answered = run
            .ask(Phase::Messages, items::folder_page(&run.base, &run.token, folder, cursor.as_ref().map(|(id, sort)| (id.as_str(), *sort))))
            .await?;
        let page = items::answer(&answered).map_err(|e| Phase::Messages.failed(e))?;
        if page.messages.iter().any(|m| m.server.folder != folder) {
            return Err(Phase::Messages.failed(Error::Unreadable));
        }
        // A page that says more and neither brings a message nor a cursor to follow is no
        // page to go on from: without one, whatever lies behind it is never read.
        if page.more && (page.messages.is_empty() || page.cursor.is_none()) {
            return Err(Phase::Messages.failed(Error::Unreadable));
        }
        // A page that says more but hands back the cursor it was asked from, or one the read
        // went on from before, is no page to go on from either: followed, the same items would be
        // read round and round, this run and every one after.
        if page.more && page.cursor.as_ref().is_some_and(|next| !seen.insert(next.clone())) {
            return Err(Phase::Messages.failed(Error::Unreadable));
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
) -> Result<(String, Vec<ZimbraResume>, bool), Stop> {
    let started = run.ask(Phase::Changes, delta::request(&run.base, &run.token, "")).await?;
    let token = delta::answer(&started, true).map_err(|e| Phase::Changes.failed(e))?.token;
    let listing = folder_listing(run, request, emit).await?;
    let mut order: Vec<&mailcraft_model::sync::GraphFolder> = listing.folders.iter().collect();
    order.sort_by_key(|f| turn(f.kind));
    // Every folder read from its beginning: a fresh read is taken only when the token `request`
    // named can't be used, and the saved cursors were made under it, so going on from one would
    // leave what changed before it unread for good. `resume_folders` goes on from them on the
    // runs whose token still stands.
    let mut left: Vec<ZimbraResume> = Vec::new();
    let mut capped = false;
    for folder in order {
        if capped {
            // A folder this run never reached starts over on the next one.
            left.push(ZimbraResume { folder: folder.id.clone(), ..ZimbraResume::default() });
            continue;
        }
        if let Some(again) = folder_tail(run, request, &folder.id, None, emit).await? {
            left.push(again);
            capped = true;
        }
    }
    let whole = left.is_empty();
    Ok((token, left, whole))
}

/// Fetch the messages `request` names whole, signing in with `password`.
pub async fn fetch_messages<E: HttpExchange>(exchange: &E, request: &ZimbraFetchRequest, password: &Secret, cancel: &AtomicBool) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..GraphFetchResult::default() };
    let base = request.url.trim().trim_end_matches('/').to_string();
    let mut run = Run { exchange, cancel, base, token: Secret::new("") };
    let stop = |out: &mut GraphFetchResult, stop: Stop| {
        if let Stop::Failed(e, phase) = stop {
            if let Error::Busy(seconds) = e {
                out.busy_for = Some(seconds);
            }
            out.offline = matches!(e, Error::Transport(TransportError::Unreachable | TransportError::TimedOut));
            out.errors.push(format!("Couldn't download messages from the Zimbra server while {}: {e}", phase.text()));
        }
    };
    let signed_in = match run.ask(Phase::SignIn, auth::request(&run.base, &login(&request.username, &request.email), password)).await {
        Ok(signed_in) => signed_in,
        Err(e) => {
            stop(&mut out, e);
            return out;
        }
    };
    match auth::answer(&signed_in) {
        Ok(token) => run.token = token,
        Err(e) => {
            stop(&mut out, Phase::SignIn.failed(e));
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
        let answer = match run.ask(Phase::Body, asked).await {
            Ok(response) => fetch::answer(&response),
            Err(Stop::Failed(Error::Transport(TransportError::BodyTooLarge), _)) => {
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
                let total = held.saturating_add(mime.len() as u64);
                if total > MAX_FETCH_BYTES {
                    break;
                }
                held = total;
                out.fetched.push(GraphFetched { id: id.clone(), mime });
            }
            Err(Error::NoSuchItem) => out.missing.push(id.clone()),
            Err(e @ (Error::AuthRefused | Error::Busy(_))) => {
                stop(&mut out, Phase::Body.failed(e));
                return out;
            }
            Err(_) => out.failed.push(id.clone()),
        }
    }
    out
}
