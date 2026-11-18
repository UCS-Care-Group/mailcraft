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

use crate::auth::{Answer, Credentials, Trust};
use crate::error::Error;
use crate::http::{HttpExchange, HttpRequest, HttpResponse, Secret, TransportError};
use crate::{auth, delta, fetch, folders, items, tags, up};
use mailcraft_model::sync::{SendDone, SendOutcome};

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

/// Sign in with `credentials`: the password (and a trusted-device token), and, when the server
/// asks for a second step, the code typed for this sign-in. Answers the token and what trust the
/// server gave.
async fn sign_in<E: HttpExchange>(run: &Run<'_, E>, user: &str, credentials: &Credentials) -> Result<(Secret, Trust), Stop> {
    // A kept sign-in token that is still good makes the sign-in needless: tried on a cheap
    // request first, so that a dead one isn't taken for a refused password by the run.
    if let Some(kept) = credentials.session.as_ref().filter(|t| !t.is_empty()) {
        match run.ask(auth::verify(&run.base, kept)).await {
            Err(Stop::Cancelled) => return Err(Stop::Cancelled),
            Ok(answered) if auth::verified(&answered) => return Ok((kept.clone(), Trust::default())),
            _ => {}
        }
    }
    let first = run.ask(auth::request(&run.base, user, credentials)).await?;
    match auth::answer(&first).map_err(Stop::Failed)? {
        Answer::Token(token, trust) => Ok((token, trust)),
        Answer::Code(pending) => {
            let Some(code) = credentials.code.as_ref() else { return Err(Stop::Failed(Error::TwoFactor)) };
            let second = run.ask(auth::second(&run.base, user, &pending, code)).await?;
            match auth::answer(&second).map_err(Stop::Failed)? {
                Answer::Token(token, trust) => Ok((token, trust)),
                Answer::Code(_) => Err(Stop::Failed(Error::TwoFactor)),
            }
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

/// Read mail down from the account `request` is for, signing in with `credentials`, and hand the
/// run to `emit` in pieces (`partial` on all but the last, which always comes). Setting `cancel`
/// calls the run off before its next request. The trust the sign-in brought goes to `keep` as soon as
/// the sign-in is done, before any piece (a trusted-device token is a secret, and goes in no
/// piece), and is answered too.
pub async fn sync_mail<E: HttpExchange>(
    exchange: &E,
    request: &ZimbraMailRequest,
    credentials: &Credentials,
    cancel: &AtomicBool,
    keep: &mut dyn FnMut(&Secret, &Trust),
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Trust {
    let mut last = blank(request);
    let mut trust = Trust::default();
    match run(exchange, request, credentials, cancel, emit, &mut trust, keep).await {
        Ok((token, resume)) => (last.token, last.resume, last.whole) = (token, Some(resume), true),
        Err(Stop::Cancelled) => last.stopped = true,
        Err(Stop::Failed(e)) => {
            match e {
                Error::AuthRefused => last.auth_refused = true,
                Error::TwoFactor => last.code_needed = true,
                Error::Busy(seconds) => last.busy_for = Some(seconds),
                _ => {}
            }
            last.errors.push(format!("Couldn't send and receive with the Zimbra server: {e}"));
        }
    }
    emit(last);
    trust
}

/// The run itself, up to the token to go on from and where each folder's unfinished fresh read
/// goes on from (empty when none was left unfinished).
async fn run<E: HttpExchange>(
    exchange: &E,
    request: &ZimbraMailRequest,
    credentials: &Credentials,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(ZimbraMailResult),
    trust: &mut Trust,
    keep: &mut dyn FnMut(&Secret, &Trust),
) -> Result<(String, Vec<ZimbraResume>), Stop> {
    let base = request.url.trim().trim_end_matches('/').to_string();
    let mut run = Run { exchange, cancel, base, token: Secret::new("") };
    let user = login(&request.username, &request.email);
    (run.token, *trust) = sign_in(&run, &user, credentials).await?;
    keep(&run.token, trust);
    // The server's tags as categories, before anything goes up: a tag a message change names and
    // the list doesn't have is made in the colour it has here, never twice in a run.
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    match run.ask(tags::request(&run.base, &run.token)).await {
        Ok(answered) => match tags::answer(&answered) {
            Ok(tags) => {
                known.extend(tags.iter().map(|tag| tag.name.to_lowercase()));
                let mut piece = blank(request);
                piece.categories = tags;
                piece.partial = true;
                emit(piece);
            }
            Err(e @ (Error::AuthRefused | Error::Busy(_))) => return Err(Stop::Failed(e)),
            // Tags that couldn't be read are no reason to leave the mail unread.
            Err(_) => {}
        },
        Err(e) => return Err(e),
    }
    upward(&run, request, &mut known, emit).await?;
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

/// What was done here, taken up before anything is read: the folders' changes, the messages', and
/// what waits in the Outbox. Each kind is a piece; each message sent is a piece of its own, as soon
/// as it is known.
async fn upward<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    known: &mut std::collections::HashSet<String>,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<(), Stop> {
    if !request.changes.is_empty() {
        let mut piece = blank(request);
        piece.partial = true;
        for change in request.changes.iter().take(mailcraft_model::sync::MAX_FOLDER_CHANGES) {
            let outcome = match up::folder_change(&run.base, &run.token, change) {
                Err(outcome) => outcome,
                Ok(asked) => {
                    let answer = run.ask(Ok(asked)).await?;
                    if let Some(e) = up::stops("FolderActionRequest", &answer).or_else(|| up::stops("CreateFolderRequest", &answer)) {
                        emit(piece);
                        return Err(Stop::Failed(e));
                    }
                    up::folder_done(change, &answer)
                }
            };
            piece.changes.push(up::folder_change_done(change, outcome));
        }
        emit(piece);
    }
    if !request.messages.is_empty() {
        let mut piece = blank(request);
        piece.partial = true;
        for change in request.messages.iter().take(mailcraft_model::sync::MAX_MESSAGE_CHANGES) {
            // A tag the server's list doesn't have is made first, in the colour it has here, when
            // the account's list has one for the name; a `tn` update would make it without one.
            if let mailcraft_model::sync::MessageChange::Update { categories: Some(categories), .. } = change {
                for name in up::tag_names(&categories.here) {
                    if !known.insert(name.to_lowercase()) {
                        continue;
                    }
                    let Some(rgb) = request.categories.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).map(|c| c.color) else {
                        continue;
                    };
                    let answer = run.ask(up::create_tag(&run.base, &run.token, &name, &rgb)).await?;
                    if let Some(e) = up::stops("CreateTagRequest", &answer) {
                        return Err(Stop::Failed(e));
                    }
                }
            }
            let mut outcome = up::message_done(change);
            match up::message_requests(&run.base, &run.token, change) {
                Err(e) => outcome = up::message_refused(e),
                Ok(asked) => {
                    for one in asked {
                        let answer = run.ask(Ok(one)).await?;
                        if let Some(e) = up::stops("MsgActionRequest", &answer) {
                            emit(piece);
                            return Err(Stop::Failed(e));
                        }
                        if let Err(e) = up::acted("MsgActionRequest", &answer) {
                            outcome = up::message_refused(e);
                            break;
                        }
                    }
                }
            }
            piece.messages.push(up::message_change_done(change, outcome));
        }
        emit(piece);
    }
    for send in request.sends.iter().take(up::MAX_SENDS) {
        let mut piece = blank(request);
        piece.partial = true;
        let held = |why: String, lost: bool| SendDone { message: send.message, outcome: SendOutcome::Held { lost, why }, ..SendDone::default() };
        if let Some(done) = up::held_before(send) {
            piece.sent.push(done);
            emit(piece);
            continue;
        }
        let Some(message) = send.mime.as_deref() else { continue };
        let text = up::written(message);
        if text.len() > up::MAX_UPLOAD {
            piece.sent.push(held(crate::Error::Transport(TransportError::BodyTooLarge).to_string(), false));
            emit(piece);
            continue;
        }
        // The upload sends nothing: whatever happens to it, the message waits or is held with why.
        let answered = run.ask(up::upload(&run.base, &run.token, text)).await?;
        let upload = match up::uploaded(&answered) {
            Ok(upload) => upload,
            Err(e @ (Error::AuthRefused | Error::Busy(_))) => return Err(Stop::Failed(e)),
            Err(e) => {
                piece.sent.push(held(format!("the Zimbra server didn't take the message: {e}"), false));
                emit(piece);
                continue;
            }
        };
        // The send is asked once; an answer that never came holds it as one that may have gone.
        let suid = up::send_id(request.account, send, message);
        let outcome = match run.ask(up::send(&run.base, &run.token, &upload, &suid)).await {
            Ok(answer) => match crate::soap::answer("SendMsgRequest", &answer) {
                Ok(_) => SendOutcome::Sent,
                Err(e @ (Error::AuthRefused | Error::Busy(_))) => return Err(Stop::Failed(e)),
                Err(e) => SendOutcome::Held { lost: false, why: e.to_string() },
            },
            // (Called off, or the server not reached: nothing went, and it waits.)
            Err(Stop::Cancelled) => return Err(Stop::Cancelled),
            Err(Stop::Failed(e @ Error::Transport(TransportError::Unreachable | TransportError::Tls | TransportError::RedirectRefused))) => {
                emit(piece);
                return Err(Stop::Failed(e));
            }
            Err(Stop::Failed(e)) => {
                piece.sent.push(held(String::new(), true));
                emit(piece);
                return Err(Stop::Failed(e));
            }
        };
        piece.sent.push(SendDone { message: send.message, outcome, ..SendDone::default() });
        emit(piece);
    }
    Ok(())
}

/// What changed since the request's token, page by page. Answers the token to go on from.
async fn changes<E: HttpExchange>(
    run: &Run<'_, E>,
    request: &ZimbraMailRequest,
    listing: &folders::Listing,
    emit: &mut dyn FnMut(ZimbraMailResult),
) -> Result<String, Stop> {
    let mut listing = listing.clone();
    let mut relisted = 0;
    // A folder a listing was asked about again and still doesn't name is no folder of mail — the
    // chat history, a calendar: messages in it have left the mail, and no page after this one asks
    // about it again.
    let mut not_mail: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut since = request.token.clone();
    for _ in 0..MAX_ROUNDS {
        let answered = run.ask(delta::request(&run.base, &run.token, &since)).await?;
        let page = delta::answer(&answered, false).map_err(Stop::Failed)?;
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
                return Ok(since);
            }
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
            return Err(Stop::Failed(Error::Unreadable));
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
            return Ok(since);
        }
    }
    // More changes wait than one run reads: the token of the last whole page is kept, and the next
    // run goes on from it, so a mailbox that changes faster than a run follows still moves on.
    Ok(since)
}

/// The messages `ids` names: one request for the chunk, unless the server faults it as holding a
/// name it no longer has — then each id is asked for by itself, so that a message deleted between
/// the delta and the read is left out and not the whole run's end. With it come the ids the
/// server itself answered for by name — gone for certain, which an id a search left out alone
/// never is.
async fn messages_by_id<E: HttpExchange>(run: &Run<'_, E>, ids: &[String]) -> Result<(Vec<GraphMessage>, Vec<String>), Stop> {
    let found = run.ask(items::by_id(&run.base, &run.token, ids)).await?;
    let mut messages = match items::answer(&found) {
        Ok(page) => page.messages,
        // The chunk is faulted for an id it no longer has — or for `is:anywhere` walking a folder
        // the server already dropped, which faults any id.
        Err(Error::NoSuchItem) => Vec::new(),
        Err(e) => return Err(Stop::Failed(e)),
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
        let one = run.ask(items::probe(&run.base, &run.token, id)).await?;
        match items::probe_answer(&one) {
            Ok(Some(message)) => messages.push(message),
            Ok(None) => gone.push(id.clone()),
            Err(e) => return Err(Stop::Failed(e)),
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
    for page_number in 0..MAX_FOLDER_PAGES {
        let answered = run.ask(items::folder_page(&run.base, &run.token, folder, cursor.as_ref().map(|(id, sort)| (id.as_str(), *sort)))).await?;
        let page = items::answer(&answered).map_err(Stop::Failed)?;
        // A page that says more and neither brings a message nor a cursor to follow is no
        // page to go on from: without one, whatever lies behind it is never read.
        if page.more && (page.messages.is_empty() || page.cursor.is_none()) {
            return Err(Stop::Failed(Error::Unreadable));
        }
        // A page that says more but hands back the cursor it was asked from is no page to go on
        // from either: asking it again would read the same items every run, for ever.
        if page.more && page.cursor == cursor {
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
    Ok((token, left))
}

/// Fetch the messages `request` names whole, signing in with `credentials` (a code is never asked
/// for here: a fetch that would need one says so in its errors).
pub async fn fetch_messages<E: HttpExchange>(
    exchange: &E,
    request: &ZimbraFetchRequest,
    credentials: &Credentials,
    cancel: &AtomicBool,
    keep: &mut dyn FnMut(&Secret, &Trust),
) -> GraphFetchResult {
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
    match sign_in(&run, &login(&request.username, &request.email), credentials).await {
        Ok((token, trust)) => {
            run.token = token;
            keep(&run.token, &trust);
        }
        Err(e) => {
            out.code_needed = matches!(e, Stop::Failed(Error::TwoFactor));
            stop(&mut out, e);
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
