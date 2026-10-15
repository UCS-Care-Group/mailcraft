//! Synchronising with the account's servers, both ways. The Outbox goes out through SMTP. Then,
//! over IMAP: a copy of what was sent is filed, the changes made here since the last run are
//! pushed (flags, moves, deletions: [`crate::push`]), and new mail, flag changes and removals
//! come in, read after the push so that they show it: the Inbox first, then the other standard
//! folders, then the rest.
//!
//! The answer is handed over in pieces as the run goes ([`sync_with`]): what was sent after each
//! message, what was pushed after each mailbox, what came in a batch of messages at a time.
//! Every piece can be taken in on its own, and a run that ends early (called off, cut off, or
//! with nobody left to take its pieces in) loses nothing: [`FolderSync::run`] says how a
//! mailbox's pieces keep count.
//!
//! What makes it quick: messages come many to a command (one round trip for fifty, not one
//! each); a mailbox the server says is as it was is only glanced at ([`FolderMark`]); changes the
//! owner makes while a run is under way go up on its own connection ([`sync_with_updates`])
//! instead of the run starting over; and mail a limit left out comes with later runs instead of
//! never.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_mime::WriteOptions;
use mailcraft_model::sync::{
    FolderDelta, FolderMark, FolderState, HeldMessage, MAX_SKIPPED, RemoteCategories, RemoteFlags, SyncRequest, SyncResult, SyncScope,
    keyword_category,
};
use mailcraft_model::{Account, Body, FlagStatus, FolderKind, Id, Message, Security};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{Fetched, Imap, Mailbox, Selected, uid_sets};
use mailcraft_time::{Date, DateTime, Timestamp};

use crate::push::{Arrived, push, uploads};
use crate::send::{Envelope, dated, deliver, open_smtp_with};
use crate::transport::{self, Patience, Service, Stream};
use crate::{NetError, kinds, not_signed_in, scrub};

/// How often a run that is busy with other mailboxes looks at the Inbox again (between two of
/// them), so that new mail doesn't wait for the run's end.
const INBOX_EVERY: Duration = Duration::from_secs(60);
/// The same through Microsoft Graph, where a look costs one question.
const INBOX_EVERY_GRAPH: Duration = Duration::from_secs(30);
/// Most mailboxes read in one run.
const MAX_FOLDERS: usize = 2_000;
/// Most message bytes downloaded in one run whose answer is kept whole ([`sync`]); the rest
/// comes with the next one.
const MAX_RUN_BYTES: usize = 256 * 1024 * 1024;
/// Most messages downloaded in such a run, however small.
const MAX_RUN_MESSAGES: usize = 100_000;
/// The largest message that is downloaded whole. Of a larger one only the header is, and a note
/// takes the place of its text ([`too_large`]).
const MAX_MESSAGE: u64 = 25 * 1024 * 1024;
/// Most new messages handed over in one piece, and most bytes of them...
const BATCH_MESSAGES: usize = 50;
const BATCH_BYTES: usize = 4 * 1024 * 1024;
/// ...and the longest that messages which have arrived wait for the piece to fill.
const BATCH_WAIT: Duration = Duration::from_secs(1);
/// Most messages asked for in one command, and most bytes they may come to by what the server
/// says of their sizes: the server sends them one after the other, and the round trip each would
/// cost on its own (most of the time a first download took) is paid once for the lot.
const FETCH_MESSAGES: usize = 50;
const FETCH_BYTES: u64 = 8 * 1024 * 1024;
/// A mailbox the server says is as it was ([`FolderMark`]) has the flags of all its messages read
/// every this many runs; in between, only those of its newest ([`RECENT_FLAGS`]).
pub(crate) const FULL_SCAN_EVERY: u32 = 5;
const RECENT_FLAGS: usize = 200;
/// The header a message kept without its text has, for a program to know it by: its size on
/// the server, in bytes.
pub(crate) const NOT_DOWNLOADED: &str = "X-MailCraft-Not-Downloaded";

/// How far one run goes, and how its answer is cut into pieces.
#[derive(Clone, Debug)]
pub(crate) struct Limits {
    /// Most message bytes downloaded; what doesn't fit stays for the next run.
    pub(crate) bytes: usize,
    /// Most messages downloaded, however small.
    pub(crate) messages: usize,
    /// The largest message that is downloaded whole.
    pub(crate) largest: u64,
    /// Most new messages in one piece, and most bytes of them...
    pub(crate) batch_messages: usize,
    pub(crate) batch_bytes: usize,
    /// ...and the longest that messages which have arrived wait for the piece to fill.
    pub(crate) batch_wait: Duration,
    /// Most messages asked for in one command, and most bytes they may come to.
    pub(crate) fetch_messages: usize,
    pub(crate) fetch_bytes: u64,
    /// How often a run busy with other mailboxes looks at the Inbox again: over IMAP between two
    /// mailboxes, and through Microsoft Graph between two batches of messages too.
    pub(crate) inbox_every: Duration,
    pub(crate) inbox_every_graph: Duration,
    /// How long the servers are waited for.
    pub(crate) patience: Patience,
}

impl Limits {
    /// For a run whose answer is taken in as it comes: nothing piles up here, so the run goes on
    /// for as long as there is something to bring.
    pub(crate) fn in_pieces() -> Limits {
        Limits {
            bytes: usize::MAX,
            messages: usize::MAX,
            largest: MAX_MESSAGE,
            batch_messages: BATCH_MESSAGES,
            batch_bytes: BATCH_BYTES,
            batch_wait: BATCH_WAIT,
            fetch_messages: FETCH_MESSAGES,
            fetch_bytes: FETCH_BYTES,
            inbox_every: INBOX_EVERY,
            inbox_every_graph: INBOX_EVERY_GRAPH,
            patience: Patience::default(),
        }
    }

    /// For a run whose answer is kept until it is whole: it downloads only so much, so that a
    /// large mailbox arrives over several runs instead of filling the memory.
    pub(crate) fn kept_whole() -> Limits {
        Limits { bytes: MAX_RUN_BYTES, messages: MAX_RUN_MESSAGES, ..Limits::in_pieces() }
    }
}

/// Send the outbox, push the changes made here ([`SyncRequest::ops`]), then download new mail
/// and flag changes. Never fails as a whole: each failed step becomes a line in
/// `SyncResult::errors`, and a change that didn't go through is simply not among the finished
/// ones, to be asked for again.
///
/// Nothing is removed from the server but the very message the owner deleted or moved here:
/// by UID MOVE, or marked `\Deleted` and expunged by its UID (UIDPLUS). A server without
/// UIDPLUS keeps it marked; the mailbox-wide EXPUNGE, which would take what other programs
/// marked too, is never sent.
///
/// The answer comes back whole, when everything is done; as all of it is held until then, one
/// run downloads only so much, and the rest comes with the next. A host that shows mail as it
/// arrives, says how far the run has got, and may call it off uses [`sync_with`].
pub fn sync(request: &SyncRequest) -> SyncResult {
    sync_within(request, Limits::kept_whole())
}

/// [`sync`], with the answer handed to `emit` in pieces as the run goes, in the order things are
/// done: mail that was sent, message by message; the changes made here that the server took on,
/// mailbox by mailbox; then what came in, the Inbox first, a batch of messages at a time. Every
/// piece but the last is marked [`partial`](SyncResult::partial) and has a line for a status bar
/// ([`progress`](SyncResult::progress)). The last piece always comes, whatever went wrong.
///
/// Each piece is safe to take in on its own, in the order they come. A host that takes in only
/// the first few of them (it was closed, say) has lost nothing: what the others would have
/// brought is asked for again by the next run, and what they would have said was done on the
/// server is found done.
///
/// Setting `cancel` calls the run off. It stops before the next message is sent and before the
/// next mailbox is brought up to date (neither is left half done); while it is only reading, it
/// stops in the middle of a command, with the next bytes that come or when none have come for
/// too long. A server that is still listening is said goodbye to, and the last piece says
/// [`stopped`](SyncResult::stopped). What the pieces before it brought stays brought.
///
/// Blocks until the run is over; `emit` is called on this thread, and a slow `emit` holds the
/// run up rather than letting pieces pile up.
pub fn sync_with(request: &SyncRequest, cancel: &AtomicBool, emit: &mut dyn FnMut(SyncResult)) {
    sync_as(request, Limits::in_pieces(), Host::of(&request.account), cancel, emit);
}

/// [`sync_with`], for a host where the owner goes on working while the run is under way: changes
/// made here since it began (a read mark, a move, a deletion) go up on the same connection, between
/// two mailboxes and between two batches of messages, instead of the run being called off for them
/// and started again.
///
/// `more` is asked at each of those places, and answers `None` when nothing new waits. What it
/// answers is a request like the one the run began with, for the same account, whose `ops` are
/// the changes that have not been handed over yet (each change only once: nothing here remembers
/// which were); its `folders` say which life of each mailbox the UIDs are from. Its outbox, its
/// secrets and everything else in it are not looked at. What became of the changes comes in the
/// pieces like that of the ones the run began with.
pub fn sync_with_updates(
    request: &SyncRequest,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(SyncResult),
    more: &mut dyn FnMut() -> Option<SyncRequest>,
) {
    sync_more_as(request, Limits::in_pieces(), Host::of(&request.account), cancel, emit, Some(more));
}

/// [`sync`], going only as far as `limits` allow.
pub(crate) fn sync_within(request: &SyncRequest, limits: Limits) -> SyncResult {
    let mut whole = SyncResult::default();
    sync_as(request, limits, Host::of(&request.account), &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    // The batches came newest first.
    for folder in &mut whole.folders {
        folder.new.sort_by_key(|m| m.uid);
    }
    whole
}

/// [`sync_with`], told how far to go and what is known of the server from its name (the tests'
/// servers are all on this machine).
pub(crate) fn sync_as(request: &SyncRequest, limits: Limits, host: Host, cancel: &AtomicBool, emit: &mut dyn FnMut(SyncResult)) {
    sync_more_as(request, limits, host, cancel, emit, None);
}

/// [`sync_as`], with changes made here while it runs coming from `more` ([`sync_with_updates`]).
pub(crate) fn sync_more_as(
    request: &SyncRequest,
    limits: Limits,
    host: Host,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(SyncResult),
    more: Option<&mut dyn FnMut() -> Option<SyncRequest>>,
) {
    // (Out of the `Option`, so its lifetime can shrink to the run's.)
    let more = more.map(|more| -> &mut dyn FnMut() -> Option<SyncRequest> { more });
    let mut run = Run {
        request,
        limits,
        cancel,
        emit,
        out: blank(request),
        stopped: false,
        handed_over: Instant::now(),
        more,
        arrived: Arrived::new(),
        brought: HashMap::new(),
        inbox_read: None,
        retry_after: None,
        busy: String::new(),
        touched: HashSet::new(),
    };
    // An account whose mail is read through Microsoft Graph: all of it goes that way.
    if request.account.mail_source == mailcraft_model::MailSource::Graph {
        crate::graph::mail::exchange(&mut run);
        run.finish();
        return;
    }
    // (A look at the Inbox alone sends nothing and changes nothing.)
    let sent = if request.inbox_only { Vec::new() } else { send_outbox(&mut run) };
    // Folders made, renamed, moved or deleted here first: the rest of the run reads the server
    // as they left it, and knows its mailboxes by their new names.
    let reshaped = if request.inbox_only { None } else { crate::push::folders(&mut run) };
    let mut run = match &reshaped {
        Some(request) => Run { request, ..run },
        None => run,
    };
    let imap = exchange(&mut run, &sent, host);
    run.finish();
    // Said once the answer is handed over: nobody waits for a goodbye.
    if let Some(mut imap) = imap {
        imap.get_mut().leave();
        let _ = imap.logout();
    }
}

/// A piece of the answer with nothing in it yet.
fn blank(request: &SyncRequest) -> SyncResult {
    SyncResult { account: request.account.id, email: request.account.email.clone(), ..Default::default() }
}

/// A run under way: what it was asked, how far it may go, and where its pieces go.
pub(crate) struct Run<'a> {
    pub(crate) request: &'a SyncRequest,
    pub(crate) limits: Limits,
    cancel: &'a AtomicBool,
    emit: &'a mut dyn FnMut(SyncResult),
    /// What happened since the last piece was handed over.
    pub(crate) out: SyncResult,
    /// The run was called off, and something was left undone for it.
    stopped: bool,
    /// When the last piece was handed over.
    handed_over: Instant,
    /// Where changes made here while the run is under way come from ([`sync_with_updates`]).
    more: Option<&'a mut dyn FnMut() -> Option<SyncRequest>>,
    /// Messages that moves of this run put into mailboxes, for the download to take for known.
    arrived: Arrived,
    /// What the pieces handed over so far brought of each mailbox, by its path: for reading one
    /// again in the same run from where the run left it ([`Run::state_now`]).
    brought: HashMap<String, Brought>,
    /// When the Inbox was last read in this run: a run busy with other folders for long looks at
    /// it again now and then ([`Run::inbox_due`]), so that new mail doesn't wait for the run's end.
    pub(crate) inbox_read: Option<Instant>,
    /// The server said "not now" and the run ended on it: how long it asked to be left alone for,
    /// and its words ([`SyncResult::retry_after`]).
    retry_after: Option<u32>,
    busy: String,
    /// Through Microsoft Graph: the folders (by path) that this run's pushes put messages into
    /// (a move, a draft), whose listed counts are from before that: they are read whatever the
    /// counts say.
    pub(crate) touched: HashSet<String>,
}

/// What a run brought of one mailbox ([`Run::brought`]).
#[derive(Default)]
struct Brought {
    uid_validity: u32,
    uid_seen: u32,
    new: Vec<(u32, String)>,
    removed: Vec<u32>,
    delta: String,
    mark: FolderMark,
}

impl<'a> Run<'a> {
    /// The flag that calls the run off.
    pub(crate) fn cancel_flag(&self) -> &'a AtomicBool {
        self.cancel
    }
}

impl Run<'_> {
    /// Whether the run was called off. Once it was, it stays so, whatever becomes of the flag.
    pub(crate) fn cancelled(&mut self) -> bool {
        self.stopped |= self.cancel.load(Ordering::Relaxed);
        self.stopped
    }

    /// Hand over what happened since the last piece, with a line on how far the run has got.
    pub(crate) fn hand_over(&mut self, progress: String) {
        let mut piece = std::mem::replace(&mut self.out, blank(self.request));
        piece.partial = true;
        piece.progress = progress;
        self.send(piece);
    }

    /// Hand over the last piece.
    fn finish(mut self) {
        let mut piece = std::mem::replace(&mut self.out, blank(self.request));
        piece.stopped = self.stopped;
        piece.retry_after = self.retry_after;
        piece.busy = std::mem::take(&mut self.busy);
        self.send(piece);
    }

    fn send(&mut self, mut piece: SyncResult) {
        // Whatever a server wrote back, the password or the token doesn't leave in an error line.
        for line in &mut piece.errors {
            *line = scrub(&scrub(line, &self.request.password), &self.request.access_token);
        }
        self.note_brought(&piece);
        (self.emit)(piece);
        self.handed_over = Instant::now();
    }

    /// Keep count of what `piece` brings of each mailbox ([`Run::brought`]).
    fn note_brought(&mut self, piece: &SyncResult) {
        for folder in piece.folders.iter().filter(|f| !f.remote.is_empty()) {
            // (Only so many mailboxes and messages are kept count of: past that, a mailbox is
            // not read twice in a run.)
            if self.brought.len() >= MAX_FOLDERS && !self.brought.contains_key(&folder.remote) {
                continue;
            }
            let b = self.brought.entry(folder.remote.clone()).or_default();
            if folder.uid_validity != 0 && folder.uid_validity != b.uid_validity {
                // Another life of the mailbox: what was counted of the last one is no more.
                *b = Brought { uid_validity: folder.uid_validity, ..Brought::default() };
            }
            b.uid_seen = b.uid_seen.max(folder.uid_seen);
            if b.new.len() < MAX_RUN_MESSAGES {
                b.new.extend(folder.new.iter().filter(|m| m.uid != 0).map(|m| (m.uid, m.remote_id.clone())));
            }
            b.removed.extend(folder.removed.iter().copied());
            if !folder.delta.is_empty() {
                b.delta.clone_from(&folder.delta);
            }
            if !folder.mark.is_empty() {
                b.mark = folder.mark.clone();
            }
        }
    }

    /// The mailbox `state` is of, as the run has left it so far: what came and went in the pieces
    /// handed over is in it, and so are how far it was read and where to ask next. For reading it
    /// again in the same run. Hand the waiting piece over first, so that all of it counts.
    pub(crate) fn state_now(&self, state: &FolderState) -> FolderState {
        let mut now = state.clone();
        let Some(b) = self.brought.get(&state.remote) else { return now };
        if b.uid_validity != 0 && b.uid_validity != state.uid_validity {
            // Read whole this run under another validity: only what it brought counts.
            now.uid_validity = b.uid_validity;
            now.known_uids.clear();
            now.remote_ids.clear();
            now.uid_seen = 0;
        }
        let gone: HashSet<u32> = b.removed.iter().copied().collect();
        now.known_uids.retain(|uid| !gone.contains(uid));
        now.remote_ids.retain(|r| !gone.contains(&r.uid));
        for (uid, id) in b.new.iter().filter(|(uid, _)| !gone.contains(uid)) {
            if !now.known_uids.contains(uid) {
                now.known_uids.push(*uid);
            }
            if !id.is_empty() && !now.remote_ids.iter().any(|r| r.uid == *uid) {
                now.remote_ids.push(mailcraft_model::sync::RemoteId { uid: *uid, id: id.clone() });
            }
        }
        now.uid_seen = now.uid_seen.max(b.uid_seen);
        if !b.delta.is_empty() {
            now.delta.clone_from(&b.delta);
        }
        if !b.mark.is_empty() {
            now.mark = b.mark.clone();
        }
        now
    }

    /// Whether the Inbox, read earlier in this run, is due another look: the run has been busy
    /// with other folders for `every`.
    pub(crate) fn inbox_due(&self, every: Duration) -> bool {
        self.inbox_read.is_some_and(|at| at.elapsed() >= every)
    }

    /// Whether the messages waiting to be handed over have waited long enough.
    pub(crate) fn batch_due(&self) -> bool {
        self.handed_over.elapsed() >= self.limits.batch_wait
    }

    /// Changes made here since the run began, for this very account, when any wait
    /// ([`sync_with_updates`]).
    pub(crate) fn more_ops(&mut self) -> Option<SyncRequest> {
        if self.cancelled() {
            return None;
        }
        let request = (self.more.as_mut()?)()?;
        let same =
            request.account.id == self.request.account.id && request.account.email.trim().eq_ignore_ascii_case(self.request.account.email.trim());
        (same && !request.ops.is_empty()).then_some(request)
    }

    /// Note that something went wrong: `what`, and why. A command cut short because the run was
    /// called off is nothing to tell. Returns whether the connection is still good.
    pub(crate) fn failed(&mut self, what: &str, error: &NetError) -> bool {
        if matches!(error, NetError::Proto(ProtoError::Io(why)) if why == transport::STOPPED) {
            self.stopped = true;
        } else if let NetError::Busy { retry_after: Some(seconds), .. } = error {
            // "Not now" with a time: said apart from the problems, with how long to wait, so
            // that the host comes back then and need not tell the owner of the first few.
            let seconds = (*seconds).max(1);
            self.retry_after = Some(self.retry_after.map_or(seconds, |had| had.max(seconds)));
            self.busy = scrub(&scrub(&format!("{what}: {error}"), &self.request.password), &self.request.access_token);
        } else {
            self.out.errors.push(format!("{what}: {error}"));
        }
        !error.is_fatal()
    }

    /// Whether a server mailbox at `remote` for `kind` is one this run reads ([`SyncScope`]).
    pub(crate) fn in_scope(&self, remote: &str, kind: FolderKind) -> bool {
        self.request.scope.covers(remote, kind)
    }

    /// Whether the run is of the folders alone ([`SyncScope::Rest`]): the Inbox is not read in
    /// its turn, only glanced at between two folders, from the state the request carries of it.
    pub(crate) fn rest_only(&self) -> bool {
        self.request.scope == SyncScope::Rest && !self.request.inbox_only
    }
}

/// A message's subject for an error line: one line, not too long.
pub(crate) fn subject_of(message: &Message) -> String {
    let words: Vec<&str> = message.subject.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let subject = words.join(" ");
    if subject.is_empty() {
        return "(no subject)".into();
    }
    if subject.chars().count() <= 60 { subject } else { subject.chars().take(60).chain(['…']).collect() }
}

/// A mailbox's name for an error line.
pub(crate) fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// A mailbox's name for a line of progress.
pub(crate) fn label(remote: &str) -> String {
    if remote == "INBOX" { "Inbox".into() } else { shown(remote).chars().take(40).collect() }
}

/// `1 message`, `3 messages`.
pub(crate) fn messages(count: usize) -> String {
    format!("{count} message{}", if count == 1 { "" } else { "s" })
}

/// Send what waits in the Outbox, in one SMTP session, and say so message by message: one the
/// mailbox still took for unsent would go out again. Returns the messages that went, as they went.
fn send_outbox(run: &mut Run) -> Vec<Message> {
    let request = run.request;
    let mut sent = Vec::new();
    if request.outbox.is_empty() {
        return sent;
    }
    if request.account.outgoing.is_none() {
        run.out.errors.push("Couldn't send mail: the account has no outgoing server".into());
        return sent;
    }
    if run.cancelled() {
        return sent;
    }
    let waiting = request.outbox.len();
    run.hand_over(format!("Sending {}", messages(waiting)));
    // A shared mailbox's mail goes out signed in as its owner ([`SyncRequest::send_login`]).
    let mut smtp = match open_smtp_with(&request.sending_account(), request.secret(), &run.limits.patience, Some(run.cancel)) {
        Ok(smtp) => smtp,
        Err(e) => {
            run.failed("Couldn't send mail", &e);
            return sent;
        }
    };
    // A message goes out whole or not at all: the run is only called off between two of them.
    smtp.get_mut().watch(None);
    // A copy of each is kept on the server: by the server itself, whatever becomes of the
    // connection to its incoming side, or put there once they have all gone. The mailbox is told
    // with the message, so that it knows the copy for what it is even if it hears no more. Not
    // for a shared mailbox: the server files what the owner sends as it in the owner's Sent
    // Items, and the copy in its own is put there afterwards, and said once it is ([`file_sent`]).
    let filed_there = request.account.incoming.is_some() && !sends_as_shared(request);
    for (at, message) in request.outbox.iter().enumerate() {
        if run.cancelled() {
            break;
        }
        // Dated once, so that the copy kept on the server says what the one that went says.
        let message = dated(message);
        let outcome = Envelope::of(&request.account, &message).and_then(|envelope| deliver(&mut smtp, &envelope, &message));
        match outcome {
            Ok(()) => {
                run.out.sent.push(message.id);
                if filed_there {
                    run.out.filed.push(message.id);
                }
                sent.push(message.into_owned());
                run.hand_over(format!("Sent {} of {waiting}", at + 1));
            }
            Err(e) => {
                let subject = subject_of(&message);
                let refused = crate::shared::sending_refused(request, &subject, &e);
                if refused.is_some() {
                    // The right is given by an administrator, not by trying again every few
                    // minutes: the message is held until the owner changes it or sends it again.
                    run.out.held.push(HeldMessage { message: message.id, why: send_as_hold(request) });
                }
                let line = refused.unwrap_or_else(|| format!("Couldn't send \"{subject}\": {e}"));
                run.out.errors.push(line);
                // The rest waits for the next run when the connection is gone.
                if e.is_fatal() {
                    return sent;
                }
            }
        }
    }
    smtp.get_mut().leave();
    let _ = smtp.quit();
    sent
}

/// Why a message from a shared mailbox is held when the server refused to send it as the mailbox
/// ([`HeldMessage::why`], shown with the message).
fn send_as_hold(request: &SyncRequest) -> String {
    let address: String = request.account.email.trim().chars().filter(|c| !c.is_control()).take(254).collect();
    format!(
        "The server didn't let your account send as {address}: sending from it takes the Send As right on it, which an administrator of your organisation gives"
    )
}

/// Whether the run sends from a shared mailbox signed in as its owner ([`SyncRequest::send_login`]):
/// then a server that files what it sends files the copy in the owner's Sent Items, not the
/// shared mailbox's, and the copy in the shared mailbox's is put there from here (Full Access
/// allows it).
fn sends_as_shared(request: &SyncRequest) -> bool {
    request.account.is_shared() || !request.send_login.trim().is_empty()
}

/// What is known of an account's incoming server from its name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Host {
    /// It keeps a copy of what is sent through it by itself (Microsoft's and Google's do): a
    /// second copy from here would show twice.
    pub(crate) files_sent: bool,
    /// It is Microsoft's (Microsoft 365, Outlook.com): the mailbox is on Exchange.
    pub(crate) microsoft: bool,
}

impl Host {
    pub(crate) fn of(account: &Account) -> Host {
        let Some(server) = &account.incoming else { return Host::default() };
        let host = server.host.trim().trim_end_matches('.').to_ascii_lowercase();
        let one_of =
            |domains: &[&str]| domains.iter().any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')));
        let microsoft = one_of(&["office365.com", "outlook.com"]);
        Host { files_sent: microsoft || one_of(&["gmail.com", "googlemail.com"]), microsoft }
    }
}

/// Put a copy of each message that was sent into the server's mailbox for sent mail, marked as
/// read. The mailbox here was told to expect the copies when the messages went; a copy that
/// didn't get there is said (`unfiled`). With `claim`, it wasn't told (a shared mailbox's copy,
/// [`sends_as_shared`]), and is told of each copy once it is there (`filed`). A server without a
/// mailbox for sent mail gets none: the copy here is the one there is. Returns whether the
/// connection is still good.
fn file_sent(imap: &mut Imap<Stream<'_>>, plan: &[(&Mailbox, FolderKind)], sent: &[Message], claim: bool, result: &mut SyncResult) -> bool {
    let Some((mailbox, _)) = plan.iter().find(|(_, kind)| *kind == FolderKind::Sent) else {
        if !claim {
            result.unfiled.extend(sent.iter().map(|m| m.id));
        }
        return true;
    };
    for (at, message) in sent.iter().enumerate() {
        // The sender's own copy says who got a blind copy.
        let options = WriteOptions { include_bcc: true, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
        match imap.append(&mailbox.raw, &["\\Seen"], &mailcraft_mime::write(message, &options)) {
            Ok(_) if claim => result.filed.push(message.id),
            Ok(_) => {}
            Err(e) => {
                let e = NetError::from(e);
                result.errors.push(format!("Couldn't keep a copy of \"{}\" on the server: {e}", subject_of(message)));
                if e.is_fatal() {
                    // The connection went while the copy was on its way: it may have arrived all
                    // the same. Should it show, it is this message's, not another one. The ones
                    // after it never set out.
                    if !claim {
                        result.unfiled.extend(sent.iter().skip(at + 1).map(|m| m.id));
                    }
                    return false;
                }
                if !claim {
                    result.unfiled.push(message.id);
                }
            }
        }
    }
    true
}

/// Connect to the account's incoming server, encrypt as it asks, and sign in: with `secret` as
/// the password, or, for an account that signs in with OAuth, as the access token (XOAUTH2).
pub(crate) fn open_imap<'a>(account: &Account, secret: &str) -> Result<Imap<Stream<'a>>, NetError> {
    open_imap_with(account, secret, &Patience::default(), None)
}

/// [`open_imap`], waiting as `patience` says, and giving up at once when `cancel` is set while
/// it is under way.
pub(crate) fn open_imap_with<'a>(
    account: &Account,
    secret: &str,
    patience: &Patience,
    cancel: Option<&'a AtomicBool>,
) -> Result<Imap<Stream<'a>>, NetError> {
    let server = account.incoming.as_ref().ok_or_else(|| NetError::Config("the account has no incoming server".into()))?;
    let user = account.login();
    if user.is_empty() {
        return Err(NetError::Config("the account has no user name or e-mail address to sign in with".into()));
    }
    // Without a token there is nothing to show the server: no connection is made.
    if account.oauth.is_some() && secret.is_empty() {
        return Err(not_signed_in(account));
    }
    let (mut stream, host) = transport::open(server, Service::Imap, patience)?;
    stream.watch(cancel);
    let mut imap = Imap::connect(stream)?;
    if server.security == Security::StartTls {
        // Never carry on in the clear when encryption was asked for.
        if !imap.capabilities()?.iter().any(|c| c == "STARTTLS") {
            return Err(NetError::Tls { host, reason: "the server doesn't offer STARTTLS".into() });
        }
        let plain = imap.starttls()?;
        imap = Imap::resume(transport::upgrade(plain, &host)?);
    }
    if imap.is_authenticated() {
        return Ok(imap);
    }
    if account.oauth.is_some() {
        imap.authenticate_xoauth2(user, secret)?;
    } else {
        imap.login(user, secret)?;
    }
    Ok(imap)
}

/// What Exchange calls, in English, the folders of a mailbox that are no folders of mail to
/// keep: where it has appointments, people, tasks, notes and the journal, mail on its way out,
/// and its reports on synchronising with Outlook.
pub(crate) const NOT_MAIL: [&str; 7] = ["calendar", "contacts", "tasks", "notes", "journal", "outbox", "sync issues"];
/// How many of them a listing has at its top when it is an Exchange mailbox's.
const EXCHANGE_SIGNS: usize = 3;
/// What Exchange calls, in English, folders of mail that other servers call otherwise.
const EXCHANGE_MAIL: [&str; 3] = ["sent items", "deleted items", "junk email"];

/// Whether the listing is that of a mailbox on Exchange whose folders have their English names
/// (Microsoft 365, Outlook.com, a company's own server).
///
/// Exchange lists the folders of [`NOT_MAIL`] over IMAP like folders of mail, with nothing to
/// tell them by but their names, and which of them a mailbox shows differs (one never opened
/// in Outlook has no `Sync Issues`). So the names themselves are the sign: three of the seven at
/// the top level are more than somebody is likely to have made by hand on another kind of
/// server. A server known to be Microsoft's by its name needs less: two of the names it gives
/// its folders of mail show that the mailbox is named in English, and then a `Notes` or an
/// `Outbox` there is Exchange's own.
///
/// A mailbox named in another language is not known by this, and shows those folders as it did
/// before: better than leaving out a folder of mail that happens to be called `Notes`.
fn is_exchange(mailboxes: &[Mailbox], microsoft: bool) -> bool {
    let listed = |names: &[&str]| names.iter().filter(|name| mailboxes.iter().any(|m| m.name.trim().eq_ignore_ascii_case(name))).count();
    listed(&NOT_MAIL) >= EXCHANGE_SIGNS || (microsoft && listed(&EXCHANGE_MAIL) >= 2)
}

/// Whether a folder of an Exchange mailbox is one of those that are not mail to keep, or lies
/// inside one (`Sync Issues/Conflicts`, `Contacts/Recipient Cache`).
fn not_mail(mailbox: &Mailbox) -> bool {
    let name = mailbox.name.trim();
    let top = mailbox.delimiter.and_then(|d| name.split_once(d)).map_or(name, |(top, _)| top.trim());
    NOT_MAIL.iter().any(|other| top.eq_ignore_ascii_case(other))
}

/// When a folder's turn comes: the Inbox, then sent mail and drafts, then where mail is kept,
/// then where it is thrown away, then every other folder.
pub(crate) fn turn(kind: FolderKind) -> u8 {
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

/// The mailboxes to read, each with what it is for, in the order they are read ([`turn`]; the
/// server's order within that). An Exchange mailbox's folders for other things than mail are
/// left out, unless a folder here follows one already (`tied`): what the owner has been shown
/// doesn't stop being looked after.
fn planned<'a>(mailboxes: &'a [Mailbox], tied: &[(&str, FolderKind)], microsoft: bool) -> Vec<(&'a Mailbox, FolderKind)> {
    let mut plan = kinds::plan(mailboxes, tied);
    if is_exchange(mailboxes, microsoft) {
        plan.retain(|(mailbox, _)| {
            let keep = !not_mail(mailbox) || tied.iter().any(|(remote, _)| *remote == kinds::remote_name(mailbox));
            if !keep {
                log::debug!("{}: not a folder of mail, left out", shown(&mailbox.name));
            }
            keep
        });
    }
    plan.sort_by_key(|(_, kind)| turn(*kind));
    plan
}

/// The folders of mail a run will read, as a tree: each with the path of the folder it lies in
/// (where the server names its parts apart and that folder is one of them).
fn listed(plan: &[(&Mailbox, FolderKind)]) -> Vec<mailcraft_model::sync::ListedFolder> {
    let mut out = Vec::new();
    for (mailbox, kind) in plan.iter().take(MAX_FOLDERS) {
        let remote = kinds::remote_name(mailbox);
        if remote.is_empty() {
            continue;
        }
        let name = mailbox.name.trim();
        let (above, leaf) = mailbox.delimiter.and_then(|d| name.rsplit_once(d)).map_or(("", name), |(above, leaf)| (above.trim(), leaf.trim()));
        // The folder it lies in, when that is one of those read too (the Inbox under any spelling).
        let parent = plan
            .iter()
            .map(|(other, _)| other)
            .find(|other| {
                !above.is_empty()
                    && (other.name.trim() == above || (above.eq_ignore_ascii_case("inbox") && other.name.trim().eq_ignore_ascii_case("inbox")))
            })
            .map(|other| kinds::remote_name(other))
            .unwrap_or_default();
        let name = if leaf.is_empty() || (parent.is_empty() && !above.is_empty()) { name } else { leaf };
        out.push(mailcraft_model::sync::ListedFolder { remote, kind: *kind, parent, name: name.to_string() });
    }
    out
}

/// What is done on the incoming server: copies of what was `sent` are filed, the changes made
/// here are pushed, and what is new in every mailbox is downloaded. Returns the connection when
/// it is still good, to be said goodbye on.
fn exchange<'a>(run: &mut Run<'a>, sent: &[Message], host: Host) -> Option<Imap<Stream<'a>>> {
    let request = run.request;
    request.account.incoming.as_ref()?;
    // A message that went is seen through, also by a run that was called off since: its copy is
    // kept on the server, and then nothing else is begun. (What a shared mailbox's server filed
    // by itself is the owner's copy: the shared mailbox's is put there from here.)
    let shared = sends_as_shared(request);
    let owed: Vec<Id> = if host.files_sent && !shared { Vec::new() } else { sent.iter().map(|m| m.id).collect() };
    if run.cancelled() && owed.is_empty() {
        return None;
    }
    run.hand_over("Connecting to the mail server".into());
    let watched = owed.is_empty().then_some(run.cancel);
    let mut imap = match open_imap_with(&request.account, request.secret(), &run.limits.patience, watched) {
        Ok(imap) => imap,
        Err(e) => {
            match crate::shared::opening_refused(request, &e) {
                Some(line) => run.out.errors.push(line),
                None => {
                    run.failed("Couldn't check for new mail", &e);
                }
            }
            // The copies that were to be kept there are not.
            if !shared {
                run.out.unfiled.extend(owed);
            }
            return None;
        }
    };
    // (What the server can do decides how its mailboxes are looked at: asked once when it
    // hasn't said.)
    if !imap.offers("IMAP4REV1") {
        let _ = imap.capabilities();
    }
    let mailboxes = match imap.list() {
        Ok(list) => list,
        Err(e) => {
            let alive = run.failed("Couldn't read the list of folders", &e.into());
            if !shared {
                run.out.unfiled.extend(owed);
            }
            return alive.then_some(imap);
        }
    };
    // Folders that already follow a server mailbox keep their job.
    let tied: Vec<(&str, FolderKind)> = request.folders.iter().filter(|f| !f.remote.is_empty()).map(|f| (f.remote.as_str(), f.kind)).collect();
    // Every folder of mail, for the list and for filing what was sent; then the ones this run
    // reads ([`SyncScope`]).
    let whole_plan = planned(&mailboxes, &tied, host.microsoft);
    let mut plan = whole_plan.clone();
    if request.inbox_only && request.scope == SyncScope::All {
        // The Inbox and nothing else: no list of folders is said, so nothing is taken for gone.
        plan.retain(|(mailbox, kind)| *kind == FolderKind::Inbox || kinds::is_inbox(mailbox));
        plan.truncate(1);
    } else {
        plan.retain(|(mailbox, kind)| run.in_scope(&kinds::remote_name(mailbox), *kind));
    }
    if whole_plan.len() > MAX_FOLDERS {
        run.out.errors.push(format!("The server has {} folders; only the first {MAX_FOLDERS} were read", whole_plan.len()));
    }
    // The whole tree first: the folders are there to see before their mail is.
    if !request.inbox_only {
        run.out.listed = listed(&whole_plan);
        // The whole list, when nothing was left out of it and the Inbox is there: only then is a
        // folder that isn't in it gone from the server.
        run.out.listed_whole = whole_plan.len() <= MAX_FOLDERS && whole_plan.iter().any(|(mailbox, _)| kinds::is_inbox(mailbox));
    }
    if !run.out.listed.is_empty() {
        run.hand_over(format!(
            "Found {}",
            if run.out.listed.len() == 1 { "1 folder".to_string() } else { format!("{} folders", whole_plan.len().min(MAX_FOLDERS)) }
        ));
    }
    // First what the server doesn't know yet, so that what is read afterwards shows it. None of
    // it is left half done because the run was called off, and when the connection is lost on
    // the way, everything else waits for the next run.
    imap.get_mut().watch(None);
    if !owed.is_empty() {
        run.hand_over("Keeping a copy of what was sent".into());
        if !file_sent(&mut imap, &whole_plan, sent, shared, &mut run.out) {
            return None;
        }
    }
    if run.cancelled() {
        return Some(imap);
    }
    // What is here and not there yet goes up first: a draft saved again is put there before
    // the copy it replaces is taken away.
    run.arrived = uploads(run, request, &mut imap, &mailboxes)?;
    let pushed = push(run, request, &mut imap, &mailboxes)?;
    for (remote, uids) in pushed {
        run.arrived.entry(remote).or_default().extend(uids);
    }
    // Asked only to tell, not to read: done.
    if request.push_only {
        return Some(imap);
    }
    // From here on the server is only read: the run can be called off in the middle of a command.
    imap.get_mut().watch(Some(run.cancel));
    // A local folder takes the news of one server mailbox only.
    let mut claimed: HashSet<Id> = HashSet::new();
    let count = plan.len().min(MAX_FOLDERS);
    let max_new = request.max_per_folder as usize;
    // The Inbox as the run began, for looking at it again while the run is busy with the others.
    // A run of the folders alone doesn't read it in its turn, but glances at it all the same,
    // from the state the request carries of it.
    let mut inbox: Option<(&Mailbox, String, FolderState)> = None;
    if run.rest_only()
        && let Some((mailbox, kind)) = whole_plan.iter().find(|(_, kind)| *kind == FolderKind::Inbox)
    {
        let remote = kinds::remote_name(mailbox);
        let state = state_for(request, &remote, *kind);
        run.inbox_read = Some(Instant::now());
        inbox = Some((mailbox, remote.clone(), FolderState { remote, kind: *kind, ..state.cloned().unwrap_or_default() }));
    }
    for (at, (mailbox, kind)) in plan.into_iter().take(MAX_FOLDERS).enumerate() {
        if run.cancelled() {
            break;
        }
        let remote = kinds::remote_name(mailbox);
        // A folder without a name couldn't be told from one that only exists here.
        if remote.is_empty() {
            continue;
        }
        // What was changed here meanwhile goes up first.
        if top_up(run, &mut imap, &mailboxes).is_err() {
            return None;
        }
        // A run busy with other folders for long looks at the Inbox again: new mail doesn't wait.
        if let Some((inbox_mailbox, inbox_remote, base)) = &inbox
            && run.inbox_due(run.limits.inbox_every)
            && !run.cancelled()
        {
            run.hand_over("Checking Inbox for new mail".into());
            let now = run.state_now(base);
            run.inbox_read = Some(Instant::now());
            let glance = FolderSync {
                mailbox: inbox_mailbox,
                remote: inbox_remote.clone(),
                kind: FolderKind::Inbox,
                state: Some(&now),
                max_new,
                mailboxes: &mailboxes,
                place: (1, count),
            };
            if !glance.run(&mut imap, run) {
                return None;
            }
        }
        let state = state_for(request, &remote, kind).filter(|state| claimed.insert(state.id));
        let first_inbox = kind == FolderKind::Inbox && inbox.is_none();
        // (Known by the mailbox's path, which a folder here that doesn't follow it yet hasn't got.)
        let base = first_inbox.then(|| FolderState { remote: remote.clone(), kind, ..state.cloned().unwrap_or_default() });
        let folder = FolderSync { mailbox, remote: remote.clone(), kind, state, max_new, mailboxes: &mailboxes, place: (at + 1, count) };
        if !folder.run(&mut imap, run) {
            // The connection is gone: the other mailboxes wait for the next run.
            return None;
        }
        if let Some(base) = base {
            run.inbox_read = Some(Instant::now());
            inbox = Some((mailbox, remote, base));
        }
    }
    top_up(run, &mut imap, &mailboxes).ok()?;
    Some(imap)
}

/// Push what was changed here since the run began ([`sync_with_updates`]), when there is any.
/// Answers whether something was pushed (another mailbox may have been opened for it), or `Err`
/// when the connection is gone.
fn top_up<'a>(run: &mut Run<'a>, imap: &mut Imap<Stream<'a>>, mailboxes: &[Mailbox]) -> Result<bool, ()> {
    if run.cancelled() {
        return Ok(false);
    }
    let Some(more) = run.more.as_mut() else { return Ok(false) };
    let Some(request) = more() else { return Ok(false) };
    // Only changes for this very account are taken: the UIDs are this server's.
    let same = request.account.id == run.request.account.id && request.account.email.trim().eq_ignore_ascii_case(run.request.account.email.trim());
    if !same || request.ops.is_empty() {
        return Ok(false);
    }
    // Nothing of a change is left half done because the run was called off.
    imap.get_mut().watch(None);
    let arrived = push(run, &request, imap, mailboxes);
    imap.get_mut().watch(Some(run.cancel));
    let arrived = arrived.ok_or(())?;
    for (remote, uids) in arrived {
        run.arrived.entry(remote).or_default().extend(uids);
    }
    Ok(true)
}

/// A size for a person to read: `100 MB`.
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1024 * 1024))
}

/// The request's folder for a server mailbox: the one already tied to that name, else, for a
/// standard kind, the local folder of that kind that isn't tied to a mailbox yet.
pub(crate) fn state_for<'a>(request: &'a SyncRequest, remote: &str, kind: FolderKind) -> Option<&'a FolderState> {
    let by_name = request.folders.iter().find(|f| !f.remote.is_empty() && f.remote == remote);
    by_name.or_else(|| request.folders.iter().find(|f| kind != FolderKind::Custom && f.remote.is_empty() && f.kind == kind))
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
}

/// The categories a server keeps as keywords on a message ([`keyword_category`]): each once,
/// in the server's order, and only so many.
pub(crate) fn categories_of(flags: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in flags.iter().filter_map(|flag| keyword_category(flag)) {
        if names.len() >= mailcraft_model::sync::MAX_MESSAGE_KEYWORDS {
            break;
        }
        if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names
}

/// What [`FolderDelta::categories`] says of a message the mailbox has: its categories on the
/// server, when it has any.
fn remote_categories(message: &Fetched) -> Option<RemoteCategories> {
    let names = categories_of(&message.flags);
    (!names.is_empty()).then_some(RemoteCategories { uid: message.uid, names })
}

fn remote_flags(message: &Fetched) -> RemoteFlags {
    let flags = &message.flags;
    RemoteFlags {
        uid: message.uid,
        read: has(flags, "\\Seen"),
        flagged: has(flags, "\\Flagged"),
        answered: has(flags, "\\Answered"),
        draft: has(flags, "\\Draft"),
    }
}

/// The message a server handed over, with what the server knows about it.
fn message_from(fetched: Fetched) -> Message {
    let flags = remote_flags(&fetched);
    let mut message = mailcraft_mime::parse(fetched.body.as_deref().unwrap_or_default());
    message.uid = fetched.uid;
    message.read = flags.read;
    message.flag = if flags.flagged { FlagStatus::Flagged } else { FlagStatus::None };
    message.answered = flags.answered;
    message.draft = flags.draft;
    message.forwarded |= has(&fetched.flags, "$Forwarded");
    // Categories given elsewhere come as keywords; what its header says of them stays too.
    let categories = categories_of(&fetched.flags);
    for name in &categories {
        if !message.has_category(name) {
            message.categories.push(name.clone());
        }
    }
    message.synced_categories = categories;
    message.received = fetched.internal_date.as_deref().and_then(internal_date).unwrap_or(message.date);
    message
}

/// What is kept of a message that is too large to download (`size` bytes, where `largest` is
/// what is taken): its header as the server handed it over, and in place of its text a note
/// that says so. It has its UID like any other message, so it is not asked for again; flags,
/// moves and deletions reach the whole message on the server.
fn too_large(header: Fetched, size: u64, largest: u64) -> Message {
    let mut message = message_from(header);
    message.attachments.clear();
    message.body = Body {
        text: format!(
            "This message is too large to download: it is {} on the server, and MailCraft takes messages of up to {}.\n\n\
             What is here comes from its header alone: who sent it, when, and what it is about. The message itself, \
             with anything attached to it, is still on the server. Read it there with another mail program or on the web.\n",
            megabytes(size),
            megabytes(largest)
        ),
        html: None,
    };
    message.headers.retain(|(name, _)| !name.eq_ignore_ascii_case(NOT_DOWNLOADED));
    message.headers.push((NOT_DOWNLOADED.into(), size.to_string()));
    message
}

/// What is kept of a message the server refused to hand over (`why`) when its header could be
/// had: the header, and in place of its text a note that says so ([`NOT_DOWNLOADED`] marks it as
/// only partly here, as a message too large to download is).
fn refused_stub(header: Fetched, why: &NetError) -> Message {
    let mut message = message_from(header);
    message.attachments.clear();
    message.body = Body {
        text: format!(
            "This message couldn't be downloaded: the server wouldn't hand it over ({why}).\n\n\
             What is here comes from its header alone: who sent it, when, and what it is about. The message itself, \
             with anything attached to it, is still on the server. Read it there with another mail program or on the web.\n"
        ),
        html: None,
    };
    message.headers.retain(|(name, _)| !name.eq_ignore_ascii_case(NOT_DOWNLOADED));
    message.headers.push((NOT_DOWNLOADED.into(), "0".into()));
    message
}

/// One server mailbox being synchronised.
struct FolderSync<'a> {
    mailbox: &'a Mailbox,
    /// The name the mailbox is known by here.
    remote: String,
    kind: FolderKind,
    /// What the local mailbox has of it, if anything.
    state: Option<&'a FolderState>,
    /// Most messages to download in this run; 0 for all of them.
    max_new: usize,
    /// The server's mailboxes, as listed at the start of the run (for changes pushed meanwhile).
    mailboxes: &'a [Mailbox],
    /// Which of the run's mailboxes this is, and of how many.
    place: (usize, usize),
}

/// The messages of a mailbox as they arrive, and how they are handed over a batch at a time.
struct Arrivals {
    delta: FolderDelta,
    /// How far the mailbox is read while messages are still to come, and what is left for later.
    below: u32,
    mark: FolderMark,
    /// The bytes of the messages waiting to be handed over.
    waiting: usize,
    /// How many messages arrived, of how many wanted in this run; and how many more the run's
    /// limit left for a later one.
    done: usize,
    total: usize,
    more: usize,
    label: String,
}

impl Arrivals {
    /// Take a message in, and hand over the batch when it is full or has waited long enough. The
    /// last of them go with what ends the mailbox.
    fn take(&mut self, run: &mut Run, message: Message, bytes: usize) {
        run.limits.messages = run.limits.messages.saturating_sub(1);
        run.limits.bytes = run.limits.bytes.saturating_sub(bytes);
        self.waiting = self.waiting.saturating_add(bytes);
        self.done = self.done.saturating_add(1);
        self.delta.new.push(message);
        let full = self.delta.new.len() >= run.limits.batch_messages || self.waiting >= run.limits.batch_bytes;
        let due = run.handed_over.elapsed() >= run.limits.batch_wait;
        if self.done < self.total && (full || due) {
            let rest = FolderDelta {
                id: self.delta.id,
                remote: self.delta.remote.clone(),
                kind: self.delta.kind,
                uid_validity: self.delta.uid_validity,
                uid_seen: self.below,
                mark: self.mark.clone(),
                ..Default::default()
            };
            let mut batch = std::mem::replace(&mut self.delta, rest);
            batch.new.sort_by_key(|m| m.uid);
            run.out.folders.push(batch);
            run.hand_over(progress(&self.label, self.done, self.total, self.more));
            self.waiting = 0;
        }
    }
}

/// `Inbox: 12 of 500`, and when the run's limit left messages for a later run,
/// `Inbox: 12 of 500 (4,500 more later)`: the count is this run's, not the folder's.
pub(crate) fn progress(label: &str, done: usize, total: usize, more: usize) -> String {
    if more == 0 { format!("{label}: {done} of {total}") } else { format!("{label}: {done} of {total} ({} more later)", grouped(more)) }
}

/// A count with thousands separators: `4,500`.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

impl FolderSync<'_> {
    /// Read the mailbox and note what changed in the run's answer. Returns whether the
    /// connection is still good for the next mailbox.
    ///
    /// New messages come newest first, so that the latest mail is what shows first: many to a
    /// command ([`Limits::fetch_messages`]), handed over a batch at a time. The mailbox's count of
    /// how far it was read (`uid_seen`) stays, piece after piece, just below the lowest new
    /// message still to come; only the piece with the last of them moves it past them all. So
    /// wherever the pieces stop being taken in, every message that came is known here by its UID
    /// and is not asked for again, and every message that didn't is above the count and unknown,
    /// which is what makes the next run ask for it.
    ///
    /// Messages left out because a run takes only so many (`max_new`, the newest win), and older
    /// ones still to come since an earlier run left them out, are at or below the mark's
    /// `backfill` from the first piece on: the next run asks for them, newest first, as far as
    /// its own limit allows. A mailbox the server says is as it was is only glanced at
    /// ([`FolderSync::glance`]).
    fn run<'r>(&self, imap: &mut Imap<Stream<'r>>, run: &mut Run<'r>) -> bool {
        let name = shown(&self.remote);
        let label = label(&self.remote);
        run.hand_over(format!("Checking {label} ({} of {})", self.place.0, self.place.1));
        // A mailbox that was read through before is asked how it stands without being opened
        // (STATUS, which costs the server next to nothing): as it was, there is nothing to
        // bring and nothing gone, and it is passed over. The flags of its messages are read
        // every few runs all the same, and the Inbox is always opened: a flag changed elsewhere
        // shows there without that wait.
        match self.unchanged_by_status(imap, run) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(alive) => return alive,
        }
        let selected = match imap.select(&self.mailbox.raw, true) {
            Ok(selected) => selected,
            Err(e) => return run.failed(&format!("Couldn't open \"{name}\""), &e.into()),
        };
        // New UIDVALIDITY: the UIDs we hold mean nothing any more, so everything starts over.
        let reset = self.state.is_some_and(|s| s.uid_validity != 0 && s.uid_validity != selected.uid_validity);
        let mut known: BTreeSet<u32> = match self.state {
            Some(state) if !reset => state.known_uids.iter().copied().filter(|uid| *uid != 0).collect(),
            _ => BTreeSet::new(),
        };
        let arrived = run.arrived.get(&self.remote).cloned().unwrap_or_default();
        if self.state.is_some() && !reset {
            // What a move just put here is here already: only its UID is new, and the result says it.
            known.extend(arrived.iter().filter(|(validity, uid)| *validity == selected.uid_validity && *uid != 0).map(|(_, uid)| *uid));
        }
        let uid_seen = self.state.filter(|_| !reset).map_or(0, |s| s.uid_seen);
        let mark = self.state.filter(|_| !reset).map_or_else(FolderMark::default, |s| s.mark.clone());
        // Messages the server refused to hand over before ([`FolderMark::skipped`]) are asked
        // for again every few runs, in case it has come round.
        let retry_due = !mark.skipped.is_empty() && mark.skip_runs.saturating_add(1) >= FULL_SCAN_EVERY;
        // Nothing came and nothing went since the mailbox was last read through.
        let unchanged = self.state.is_some_and(|s| s.uid_validity == selected.uid_validity)
            && selected.uid_next != 0
            && mark.uid_next == selected.uid_next
            && mark.exists == selected.exists
            && mark.backfill == 0
            && arrived.is_empty();
        if unchanged && !retry_due && mark.quick_runs.saturating_add(1) < FULL_SCAN_EVERY {
            return self.glance(imap, run, &selected, &known, uid_seen, mark);
        }

        let listed = match imap.uid_fetch_flags("1:*") {
            Ok(listed) => listed,
            // Some servers refuse `1:*` in an empty mailbox rather than answer with nothing.
            Err(ProtoError::Rejected(_)) if selected.exists == 0 => Vec::new(),
            Err(e) => return run.failed(&format!("Couldn't read \"{name}\""), &e.into()),
        };
        if listed.is_empty() && selected.exists > 0 {
            // Believing this would delete every message we have of the mailbox.
            run.out.errors.push(format!("Couldn't read \"{name}\": the server listed none of its {} messages", selected.exists));
            return true;
        }
        let highest = listed.iter().map(|m| m.uid).max();
        // Messages marked as deleted are on their way out; like other mail programs, we don't show them.
        let present: BTreeMap<u32, &Fetched> = listed.iter().filter(|m| !has(&m.flags, "\\Deleted")).map(|m| (m.uid, m)).collect();

        // What the server refused before and still has (what it no longer has is forgotten), and
        // which of them are asked for again this run.
        let mut skipped: Vec<u32> = mark.skipped.iter().copied().filter(|uid| present.contains_key(uid)).take(MAX_SKIPPED).collect();
        let retried: Vec<u32> = if retry_due { skipped.clone() } else { Vec::new() };
        // What to download, in UID order: what is new, and what an earlier run left for later;
        // with a limit, the newest win and the rest is left for the next run.
        let older = mark.backfill.min(uid_seen);
        let mut wanted: Vec<u32> =
            present.keys().copied().filter(|uid| ((*uid > uid_seen || *uid <= older) && !known.contains(uid)) || retried.contains(uid)).collect();
        let mut left_out = 0;
        let mut more = 0;
        if self.max_new > 0 && wanted.len() > self.max_new {
            let cut = wanted.len() - self.max_new;
            left_out = wanted.get(cut.saturating_sub(1)).copied().unwrap_or(0);
            wanted.drain(..cut);
            more = cut;
        }
        // How far the mailbox is read while messages are still to come, and the older ones that
        // are still to come.
        let below = wanted.first().map_or(uid_seen, |lowest| lowest.saturating_sub(1).max(uid_seen));
        // (One asked for again is no message left for later: it is kept in mind by itself.)
        let older_wanted = wanted.iter().copied().filter(|uid| *uid <= below && !retried.contains(uid)).max().unwrap_or(0);
        let pending =
            FolderMark { backfill: left_out.max(older_wanted), skipped: mark.skipped.clone(), skip_runs: mark.skip_runs, ..Default::default() };
        // What is said once, with the first piece: the flags of the messages already here, and
        // which of them are gone.
        let delta = FolderDelta {
            id: self.state.map(|s| s.id).filter(|id| *id != 0),
            remote: self.remote.clone(),
            kind: self.kind,
            uid_validity: selected.uid_validity,
            uid_seen: below,
            new: Vec::new(),
            flags: known.iter().filter_map(|uid| present.get(uid).map(|m| remote_flags(m))).collect(),
            categories: known.iter().filter_map(|uid| present.get(uid).and_then(|m| remote_categories(m))).collect(),
            removed: match self.state {
                Some(state) if reset => state.known_uids.iter().copied().filter(|uid| *uid != 0).collect(),
                _ => known.iter().copied().filter(|uid| !present.contains_key(uid)).collect(),
            },
            mark: pending.clone(),
            delta: String::new(),
        };
        let mut alive = true;
        // What is wanted and hasn't arrived (yet): the next run asks for it again.
        let mut missing: BTreeSet<u32> = wanted.iter().copied().collect();
        // Messages that were asked for and not handed over, without a word why; and those the
        // server refused when asked for on their own.
        let mut silent: Vec<u32> = Vec::new();
        let mut refused: Vec<u32> = Vec::new();
        let sizes = self.sizes(imap, &wanted, run, &mut alive);
        let mut arrivals = Arrivals { delta, below, mark: pending, waiting: 0, done: 0, total: wanted.len(), more, label: label.clone() };
        // Newest first, so that when a run is cut short the latest mail is what is already here.
        let mut rest: &[u32] = &wanted;
        let mut first = true;
        while alive && !rest.is_empty() && !run.cancelled() {
            if !std::mem::take(&mut first) {
                // What was changed here meanwhile goes up between two commands; the mailbox is
                // opened again after, unless it was made anew meanwhile.
                match top_up(run, imap, self.mailboxes) {
                    Ok(false) => {}
                    Ok(true) => match imap.select(&self.mailbox.raw, true) {
                        Ok(again) if again.uid_validity == selected.uid_validity => {}
                        Ok(_) => break,
                        Err(e) => {
                            alive = run.failed(&format!("Couldn't open \"{name}\""), &e.into());
                            break;
                        }
                    },
                    Err(()) => {
                        alive = false;
                        break;
                    }
                }
            }
            let (asked, large) = next_command(&mut rest, &sizes, &run.limits);
            if asked.is_empty() {
                // Nothing more fits in this run; not a problem, the rest comes with the next one.
                continue;
            }
            let mut got: Vec<Fetched> = Vec::new();
            let outcome = fetch_set(imap, &asked, large, &mut got);
            // The server answers oldest first; they are taken in newest first like the commands.
            got.sort_by_key(|m| std::cmp::Reverse(m.uid));
            for fetched in got {
                self.arrived(fetched, large, &sizes, &mut missing, &mut arrivals, run);
            }
            let unanswered: Vec<u32> = asked.iter().copied().filter(|uid| missing.contains(uid)).collect();
            match outcome {
                // Gone between the listing and now, as a rule (looked into below).
                Ok(()) => silent.extend(unanswered),
                // Refused, as a rule for one message of them: each of the others is asked for
                // on its own. One the server refuses on its own too is one it won't hand over
                // (a message Exchange can't render, say): the run passes it over and says so
                // once with the others it didn't get, rather than stopping at it run after run.
                Err(e) if !e.is_fatal() => {
                    if unanswered.len() > 1 {
                        log::debug!("{name}: {} messages refused together, asked for one by one: {e}", unanswered.len());
                    } else {
                        log::debug!("{name}: a message was refused: {e}");
                    }
                    for uid in unanswered.into_iter().rev() {
                        if run.cancelled() {
                            break;
                        }
                        let mut got: Vec<Fetched> = Vec::new();
                        match fetch_set(imap, &[uid], large, &mut got) {
                            Ok(()) if got.is_empty() => silent.push(uid),
                            Ok(()) => got.into_iter().for_each(|fetched| self.arrived(fetched, large, &sizes, &mut missing, &mut arrivals, run)),
                            Err(e) if !e.is_fatal() => {
                                refused.push(uid);
                                if !self.refused_alone(imap, uid, large, &e, &mut missing, &mut arrivals, run) {
                                    alive = false;
                                    break;
                                }
                            }
                            Err(e) => {
                                alive = run.failed(&format!("Couldn't download a message from \"{name}\""), &e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => alive = run.failed(&format!("Couldn't download a message from \"{name}\""), &e),
            }
        }
        // Not handed over: what the server refused (said once: not again for one it refused
        // before and was asked for again), and what it was silent about and still has.
        let mut not_handed_over = refused.iter().filter(|uid| !mark.skipped.contains(uid)).count();
        if alive && !silent.is_empty() && !run.cancelled() {
            // A message the mailbox still has is not passed over for the server's silence.
            let kept_back = self.kept_back(imap, &silent, run, &mut alive);
            not_handed_over = not_handed_over.saturating_add(kept_back.len());
            for uid in silent.iter().filter(|uid| !kept_back.contains(uid)) {
                missing.remove(uid);
            }
        }
        // A refused message is passed over by this run: the mailbox is read on past it, so that
        // it doesn't hold the count below itself and bring an error line with every run. It is
        // kept in mind, with those refused before, and asked for again every few runs; one that
        // was asked for again and came at last is kept in mind no longer.
        for uid in &refused {
            missing.remove(uid);
            if !skipped.contains(uid) && skipped.len() < MAX_SKIPPED {
                skipped.push(*uid);
            }
        }
        skipped.retain(|uid| !retried.contains(uid) || refused.contains(uid) || missing.contains(uid));
        let skip_runs = if skipped.is_empty() || retry_due || not_handed_over > 0 { 0 } else { mark.skip_runs.saturating_add(1) };
        if not_handed_over > 0 {
            run.out.errors.push(format!("The server didn't hand over {} of \"{name}\"", messages(not_handed_over)));
        }
        let mut delta = arrivals.delta;
        // Read up to just below the lowest new message still to come; older ones still to come
        // are left for later with those the limit left out.
        let lowest_new = missing.iter().copied().find(|uid| *uid > below);
        let older_missing = missing.iter().copied().filter(|uid| *uid <= below && !retried.contains(uid)).max().unwrap_or(0);
        delta.uid_seen = match lowest_new {
            Some(lowest) => lowest.saturating_sub(1).max(below),
            None => highest.unwrap_or(uid_seen),
        };
        let backfill = left_out.max(older_missing);
        // Read through with nothing left to bring: the next run can tell whether anything changed.
        // (One asked for again that didn't come is left to the next asking, not to a backfill.)
        let still_missing = missing.iter().any(|uid| !retried.contains(uid));
        delta.mark = if !still_missing && backfill == 0 {
            FolderMark { uid_next: selected.uid_next, exists: selected.exists, quick_runs: 0, backfill: 0, skipped, skip_runs, unread: 0 }
        } else {
            FolderMark { backfill, skipped, skip_runs, ..Default::default() }
        };
        delta.new.sort_by_key(|m| m.uid);
        log::debug!(
            "{name}: {} wanted, {} gone, read up to {}, {} left for later",
            arrivals.total,
            delta.removed.len(),
            delta.uid_seen,
            missing.len()
        );
        // It goes with the next piece: the next mailbox's first line, or the end of the run.
        run.out.folders.push(delta);
        alive
    }

    /// Whether the mailbox is as it was when it was last read through, by STATUS alone (`Ok(true)`:
    /// a piece that says so has gone out, and the mailbox is not opened), or has to be opened
    /// (`Ok(false)`). Only on a server with UIDPLUS (where a move leaves no `\Deleted` ghost that
    /// STATUS would count), and only a mailbox other than the Inbox that the mailbox here follows
    /// under the same `UIDVALIDITY`, with nothing left for later, nothing just moved into it, no
    /// refused message due to be asked for again and whose flags are not due to be read whole.
    /// `Err` says whether the connection is still good after a STATUS the server wouldn't answer.
    fn unchanged_by_status(&self, imap: &mut Imap<Stream<'_>>, run: &mut Run) -> Result<bool, bool> {
        let Some(state) = self.state else { return Ok(false) };
        let mark = &state.mark;
        let retry_due = !mark.skipped.is_empty() && mark.skip_runs.saturating_add(1) >= FULL_SCAN_EVERY;
        // (A server without UIDPLUS keeps what was moved out marked `\Deleted` in the mailbox,
        // which STATUS counts as still there: such a mailbox is opened, as before.)
        let worth_asking = self.kind != FolderKind::Inbox
            && imap.offers("UIDPLUS")
            && state.uid_validity != 0
            && mark.uid_next != 0
            && mark.backfill == 0
            && !retry_due
            && mark.quick_runs.saturating_add(1) < FULL_SCAN_EVERY
            && run.arrived.get(&self.remote).is_none_or(Vec::is_empty);
        if !worth_asking {
            return Ok(false);
        }
        let status = match imap.status(&self.mailbox.raw) {
            Ok(status) => status,
            // A server that won't say: the mailbox is opened as before.
            Err(e) => {
                let e = NetError::from(e);
                if e.is_fatal() {
                    return Err(run.failed(&format!("Couldn't read \"{}\"", shown(&self.remote)), &e));
                }
                return Ok(false);
            }
        };
        let same =
            status.uid_validity == state.uid_validity && status.uid_next != 0 && status.uid_next == mark.uid_next && status.exists == mark.exists;
        if !same {
            return Ok(false);
        }
        log::debug!("{}: as it was by STATUS, not opened", shown(&self.remote));
        run.out.folders.push(FolderDelta {
            id: Some(state.id).filter(|id| *id != 0),
            remote: self.remote.clone(),
            kind: self.kind,
            uid_validity: state.uid_validity,
            uid_seen: state.uid_seen,
            mark: FolderMark {
                quick_runs: mark.quick_runs.saturating_add(1),
                skip_runs: if mark.skipped.is_empty() { 0 } else { mark.skip_runs.saturating_add(1) },
                ..mark.clone()
            },
            ..Default::default()
        });
        Ok(true)
    }

    /// A message the server refused to hand over when asked for on its own (`why`): kept with
    /// its header alone, when the server will hand that over, as a message that is only partly
    /// here (like one too large to download, [`too_large`]), so that it is seen to exist; else
    /// passed over. Either way the run reads on past it. Returns whether the connection is still
    /// good.
    #[allow(clippy::too_many_arguments)]
    fn refused_alone(
        &self,
        imap: &mut Imap<Stream<'_>>,
        uid: u32,
        large: bool,
        why: &NetError,
        missing: &mut BTreeSet<u32>,
        arrivals: &mut Arrivals,
        run: &mut Run,
    ) -> bool {
        let name = shown(&self.remote);
        if large {
            // It was its header that was asked for, and refused.
            log::debug!("{name}: a message's header was refused on its own; the message is passed over: {why}");
            return true;
        }
        let mut header: Vec<Fetched> = Vec::new();
        match fetch_set(imap, &[uid], true, &mut header) {
            Ok(()) => {
                let mut kept = false;
                for fetched in header.into_iter().filter(|f| f.uid == uid) {
                    if missing.remove(&uid) {
                        arrivals.take(run, refused_stub(fetched, why), 0);
                        kept = true;
                    }
                }
                log::debug!("{name}: a message was refused on its own; {}: {why}", if kept { "its header is kept" } else { "it is passed over" });
                true
            }
            Err(e) if !e.is_fatal() => {
                log::debug!("{name}: a message and its header were refused on their own; the message is passed over: {e}");
                true
            }
            Err(e) => run.failed(&format!("Couldn't download a message from \"{name}\""), &e),
        }
    }

    /// Take in a message that arrived, if it is one that was wanted and still missing.
    fn arrived(
        &self,
        fetched: Fetched,
        large: bool,
        sizes: &BTreeMap<u32, u64>,
        missing: &mut BTreeSet<u32>,
        arrivals: &mut Arrivals,
        run: &mut Run,
    ) {
        if !missing.remove(&fetched.uid) {
            return;
        }
        let bytes = fetched.body.as_ref().map_or(0, Vec::len);
        let message = if large {
            let size = sizes.get(&fetched.uid).copied().unwrap_or(0);
            too_large(fetched, size, run.limits.largest)
        } else {
            message_from(fetched)
        };
        arrivals.take(run, message, bytes);
    }

    /// A mailbox the server says is as it was since it was last read through ([`FolderMark`]):
    /// nothing to download and nothing gone, so only the flags of its newest messages are read,
    /// which is where they change (read elsewhere, flagged on a phone). Those of all the others
    /// are read every few runs, by [`FolderSync::run`]. Returns whether the connection is still
    /// good.
    fn glance(
        &self,
        imap: &mut Imap<Stream<'_>>,
        run: &mut Run,
        selected: &Selected,
        known: &BTreeSet<u32>,
        uid_seen: u32,
        mark: FolderMark,
    ) -> bool {
        let (mut flags, mut removed, mut categories) = (Vec::new(), Vec::new(), Vec::new());
        if let (Some(lowest), Some(highest)) =
            (known.iter().rev().nth(RECENT_FLAGS.saturating_sub(1)).or(known.first()).copied(), known.last().copied())
        {
            let set = if lowest == highest { lowest.to_string() } else { format!("{lowest}:{highest}") };
            match imap.uid_fetch_flags(&set) {
                Ok(listed) => {
                    for m in listed.iter().filter(|m| m.uid >= lowest && known.contains(&m.uid)) {
                        if has(&m.flags, "\\Deleted") {
                            removed.push(m.uid);
                        } else {
                            flags.push(remote_flags(m));
                            categories.extend(remote_categories(m));
                        }
                    }
                    // A message held here under a UID the server doesn't list in that range isn't
                    // there (an undo can put one back under a UID it never had), as a full scan
                    // would find.
                    let there: BTreeSet<u32> = listed.iter().map(|m| m.uid).collect();
                    removed.extend(known.range(lowest..=highest).filter(|uid| !there.contains(uid)));
                }
                Err(e) => return run.failed(&format!("Couldn't read \"{}\"", shown(&self.remote)), &e.into()),
            }
        }
        log::debug!("{}: unchanged, {} flags read", shown(&self.remote), flags.len());
        run.out.folders.push(FolderDelta {
            id: self.state.map(|s| s.id).filter(|id| *id != 0),
            remote: self.remote.clone(),
            kind: self.kind,
            uid_validity: selected.uid_validity,
            uid_seen,
            new: Vec::new(),
            flags,
            removed,
            categories,
            mark: FolderMark {
                quick_runs: mark.quick_runs.saturating_add(1),
                skip_runs: if mark.skipped.is_empty() { 0 } else { mark.skip_runs.saturating_add(1) },
                ..mark
            },
            delta: String::new(),
        });
        true
    }

    /// Which of the messages that were asked for and not handed over (`silent`) the mailbox
    /// still has. One that went between the listing and the asking is gone, and that is that;
    /// one that is still there is asked for again by the next run. When the server won't say,
    /// all of them are.
    fn kept_back(&self, imap: &mut Imap<Stream<'_>>, silent: &[u32], run: &mut Run, alive: &mut bool) -> Vec<u32> {
        let mut there = Vec::new();
        for (set, uids) in uid_sets(silent) {
            match imap.uid_fetch_flags(&set) {
                Ok(listed) => there.extend(listed.iter().filter(|m| !has(&m.flags, "\\Deleted") && uids.contains(&m.uid)).map(|m| m.uid)),
                Err(e) => {
                    let e = NetError::from(e);
                    if e.is_fatal() {
                        *alive = run.failed(&format!("Couldn't read \"{}\"", shown(&self.remote)), &e);
                    }
                    return silent.to_vec();
                }
            }
        }
        there
    }

    /// The sizes of the messages about to be downloaded, so that one too large to take is
    /// known before it is asked for, and so many are asked for at once as make a good batch. A
    /// server that won't say gets the benefit of the doubt.
    fn sizes(&self, imap: &mut Imap<Stream<'_>>, wanted: &[u32], run: &mut Run, alive: &mut bool) -> BTreeMap<u32, u64> {
        let (Some(first), Some(last)) = (wanted.first(), wanted.last()) else { return BTreeMap::new() };
        match imap.uid_fetch_sizes(&format!("{first}:{last}")) {
            Ok(sizes) => sizes.into_iter().collect(),
            Err(e) => {
                let e = NetError::from(e);
                if e.is_fatal() {
                    *alive = run.failed(&format!("Couldn't read \"{}\"", shown(&self.remote)), &e);
                }
                BTreeMap::new()
            }
        }
    }
}

/// The next messages to ask for, from the top of `rest` (newest first), and whether only their
/// headers are (one message too large to take whole, alone). Messages that don't fit in what is
/// left of the run ([`Limits::bytes`], [`Limits::messages`]) are passed over: they stay wanted,
/// for the next run. Empty when nothing more fits.
fn next_command(rest: &mut &[u32], sizes: &BTreeMap<u32, u64>, limits: &Limits) -> (Vec<u32>, bool) {
    let (mut asked, mut bytes) = (Vec::new(), 0u64);
    let (mut room_messages, mut room_bytes) = (limits.messages, limits.bytes);
    while let Some((&uid, lower)) = rest.split_last() {
        let size = sizes.get(&uid).copied().unwrap_or(0);
        // Of a message too large to take, the header is.
        let large = size > limits.largest;
        let fits = large || usize::try_from(size).unwrap_or(usize::MAX) <= room_bytes;
        if room_messages == 0 || room_bytes == 0 || !fits {
            // Enough for one run; not a problem, the rest comes with the next one.
            *rest = lower;
            continue;
        }
        if large {
            if asked.is_empty() {
                asked.push(uid);
                *rest = lower;
                return (asked, true);
            }
            break;
        }
        if !asked.is_empty() && (asked.len() >= limits.fetch_messages.max(1) || bytes.saturating_add(size) > limits.fetch_bytes) {
            break;
        }
        asked.push(uid);
        bytes = bytes.saturating_add(size);
        room_messages = room_messages.saturating_sub(1);
        room_bytes = room_bytes.saturating_sub(usize::try_from(size).unwrap_or(usize::MAX));
        *rest = lower;
    }
    (asked, false)
}

/// Ask for `uids` (whole, or `large`: their headers) in as few commands as their set takes, the
/// messages that arrive going into `got`, also when a command fails part way.
fn fetch_set(imap: &mut Imap<Stream<'_>>, uids: &[u32], large: bool, got: &mut Vec<Fetched>) -> Result<(), NetError> {
    for (set, _) in uid_sets(uids) {
        imap.uid_fetch_messages(&set, large, &mut |fetched| got.push(fetched))?;
    }
    Ok(())
}

/// The instant of an IMAP `INTERNALDATE`: `17-Jul-1996 02:44:25 -0700` (the day may be padded with a space).
pub(crate) fn internal_date(text: &str) -> Option<Timestamp> {
    let mut parts = text.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let mut date = date.split('-');
    let (day, month, year) = (date.next()?.parse::<u8>().ok()?, mailcraft_time::parse_month(date.next()?)?, date.next()?.parse::<i32>().ok()?);
    let mut time = time.split(':');
    let (hour, minute, second) = (time.next()?.parse::<u8>().ok()?, time.next()?.parse::<u8>().ok()?, time.next()?.parse::<u8>().ok()?);
    if date.next().is_some() || time.next().is_some() {
        return None;
    }
    let (sign, digits) = match zone.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (zone_hours, zone_minutes) = (digits.get(..2)?.parse::<i32>().ok()?, digits.get(2..)?.parse::<i32>().ok()?);
    if zone_hours > 23 || zone_minutes > 59 {
        return None;
    }
    // A leap second is read as the last second of its minute.
    let clock = DateTime::new(Date::new(year, month, day)?, hour, minute, second.min(59))?;
    Some(clock.to_timestamp(sign * (zone_hours * 60 + zone_minutes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_dates() {
        // The example of RFC 3501: 09:44:25 UTC.
        assert_eq!(internal_date("17-Jul-1996 02:44:25 -0700"), Some(837_596_665));
        assert_eq!(internal_date(" 1-Jan-1970 00:00:00 +0000"), Some(0));
        assert_eq!(internal_date("01-jan-1970 05:30:00 +0530"), Some(0));
        assert_eq!(internal_date("31-Dec-2016 23:59:60 +0000"), internal_date("31-Dec-2016 23:59:59 +0000"));
        assert_eq!(internal_date("11-Oct-2026 09:30:00 +0200"), mailcraft_time::parse_rfc5322("Sun, 11 Oct 2026 09:30:00 +0200").map(|(ts, _)| ts));
    }

    #[test]
    fn junk_is_not_a_date() {
        for junk in [
            "",
            "yesterday",
            "17-Jul-1996",
            "17-Jul-1996 02:44:25",
            "17-Jul-1996 02:44:25 0700",
            "17-Jul-1996 02:44:25 -07:00",
            "17-Jul-1996 02:44:25 -0700 extra",
            "32-Jul-1996 02:44:25 -0700",
            "17-Foo-1996 02:44:25 -0700",
            "17-Jul-0 02:44:25 -0700",
            "17-Jul-1996-3 02:44:25 -0700",
            "17-Jul-1996 24:44:25 -0700",
            "17-Jul-1996 02:44 -0700",
            "17-Jul-1996 02:44:25:01 -0700",
            "17-Jul-1996 02:44:25 -2500",
            "17-Jul-1996 02:44:25 +0199",
            "17-Jul-1996 02:44:25 é0700",
            "17-Jul-99999999999 02:44:25 -0700",
        ] {
            assert_eq!(internal_date(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn what_a_servers_name_says_of_it() {
        let named = |host: &str| {
            let incoming = Some(mailcraft_model::Server { host: host.into(), port: 993, security: Security::Tls });
            Host::of(&Account { incoming, ..Default::default() })
        };
        // Microsoft's and Google's keep what is sent through them; Microsoft's are Exchange.
        for host in ["outlook.office365.com", "imap-mail.outlook.com", " OUTLOOK.Office365.com. ", "outlook.com"] {
            assert_eq!(named(host), Host { files_sent: true, microsoft: true }, "{host}");
        }
        for host in ["imap.gmail.com", "imap.googlemail.com", " IMAP.GMAIL.COM. "] {
            assert_eq!(named(host), Host { files_sent: true, microsoft: false }, "{host}");
        }
        // Not a name that only looks like one of theirs, nor a server of one's own.
        for host in [
            "mail.example.org",
            "notgmail.com",
            "imap.gmail.com.example.org",
            "office365.com.evil.example",
            "notoutlook.com",
            "gmail.co",
            "127.0.0.1",
            "",
        ] {
            assert_eq!(named(host), Host::default(), "{host}");
        }
        assert_eq!(Host::of(&Account::default()), Host::default());
    }

    #[test]
    fn subjects_for_error_lines() {
        let subject = |s: &str| subject_of(&Message { subject: s.into(), ..Default::default() });
        assert_eq!(subject("  Lunch\r\n on\tFriday "), "Lunch on Friday");
        assert_eq!(subject(" \n "), "(no subject)");
        let long = subject(&"é".repeat(200));
        assert_eq!(long.chars().count(), 61);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn local_folders_are_found_by_name_then_by_kind() {
        let folder = |id, remote: &str, kind| FolderState { id, remote: remote.into(), kind, ..Default::default() };
        let request = SyncRequest {
            folders: vec![
                folder(1, "", FolderKind::Inbox),
                folder(2, "", FolderKind::Sent),
                folder(3, "Sent Items", FolderKind::Sent),
                folder(4, "Projects", FolderKind::Custom),
                folder(5, "", FolderKind::Custom),
            ],
            ..Default::default()
        };
        let id = |remote: &str, kind| state_for(&request, remote, kind).map(|f| f.id);
        assert_eq!(id("INBOX", FolderKind::Inbox), Some(1));
        // A folder already tied to the name wins over one of the right kind.
        assert_eq!(id("Sent Items", FolderKind::Sent), Some(3));
        assert_eq!(id("Gesendet", FolderKind::Sent), Some(2));
        assert_eq!(id("Projects", FolderKind::Custom), Some(4));
        // A folder the mailbox doesn't have yet; local-only custom folders are never matched.
        assert_eq!(id("Receipts", FolderKind::Custom), None);
        assert_eq!(id("", FolderKind::Custom), None);
        assert_eq!(id("Trash", FolderKind::Deleted), None);
    }

    fn listed(names: &[&str]) -> Vec<Mailbox> {
        let flags = vec!["\\HasNoChildren".to_string()];
        names.iter().map(|name| Mailbox { name: name.to_string(), raw: name.to_string(), delimiter: Some('/'), flags: flags.clone() }).collect()
    }

    #[test]
    fn an_exchange_mailbox_is_known_by_the_folders_it_has() {
        // Three of its folders that are no mail at the top level, in whatever letter case: all
        // of them, as a company's mailbox lists them, or the few of a mailbox at Outlook.com.
        assert!(is_exchange(&listed(&["INBOX", "Calendar", "Contacts", "Journal", "Notes", "Outbox", "Sync Issues", "Tasks"]), false));
        assert!(is_exchange(&listed(&["Inbox", "Notes", "Outbox", "Sync Issues", "Sync Issues/Conflicts"]), false));
        assert!(is_exchange(&listed(&["inbox", "NOTES", "journal", " Contacts "]), false));
        // Fewer are somebody's own folders, and so are folders that only sound like them.
        assert!(!is_exchange(&listed(&["INBOX", "Notes", "Tasks", "Sent", "Sent Items", "Deleted Items"]), false));
        assert!(!is_exchange(&listed(&["INBOX", "Old/Calendar", "Old/Contacts", "Old/Tasks", "Calendars", "My Notes", "Journal"]), false));
        assert!(!is_exchange(&[], false) && !is_exchange(&[], true));
        // On a server of Microsoft's, two of Exchange's English names for folders of mail show
        // that its other folders have theirs.
        let never_in_outlook = listed(&["Archive", "Deleted Items", "Drafts", "INBOX", "Junk Email", "Notes", "Outbox", "Sent Items"]);
        assert!(is_exchange(&never_in_outlook, true) && !is_exchange(&never_in_outlook, false));
        // A mailbox named in another language is not known by this: its "Notes" may be mail.
        let german = listed(&["Posteingang", "Gesendete Elemente", "Entwürfe", "Journal", "Notes", "Kalender", "Kontakte", "Aufgaben", "Notizen"]);
        assert!(!is_exchange(&german, true) && !is_exchange(&german, false));
        for (name, other_than_mail) in [
            ("Calendar", true),
            ("calendar/Birthdays", true),
            ("Contacts/Recipient Cache", true),
            ("Tasks", true),
            ("Notes", true),
            ("Journal", true),
            ("OUTBOX", true),
            ("Sync Issues", true),
            ("Sync Issues/Server Failures", true),
            ("Conversation History", false),
            ("Calendar 2025", false),
            ("Projects/Calendar", false),
            ("INBOX", false),
            ("INBOX/Notes", false),
            ("Sent Items", false),
            ("Deleted Items", false),
            ("", false),
        ] {
            assert_eq!(not_mail(&listed(&[name])[0]), other_than_mail, "{name}");
        }
        // Whatever parts the levels on the server parts them here.
        let dotted = Mailbox { name: "Sync Issues.Conflicts".into(), raw: "Sync Issues.Conflicts".into(), delimiter: Some('.'), flags: Vec::new() };
        assert!(not_mail(&dotted) && !not_mail(&Mailbox { delimiter: None, ..dotted }));
    }

    #[test]
    fn mailboxes_are_read_in_the_order_they_matter_to_someone_waiting() {
        let names = |plan: Vec<(&Mailbox, FolderKind)>| {
            plan.into_iter().map(|(m, kind)| format!("{} ({})", m.name, kind.code())).collect::<Vec<_>>().join(", ")
        };
        // As Exchange lists them: by name, and nothing said about what they are for.
        let exchange = listed(&[
            "Archive",
            "Calendar",
            "Contacts",
            "Deleted Items",
            "Drafts",
            "INBOX",
            "Journal",
            "Junk Email",
            "Notes",
            "Outbox",
            "Receipts",
            "Sent Items",
            "Sync Issues",
            "Sync Issues/Conflicts",
            "Tasks",
        ]);
        let mail = "INBOX (inbox), Sent Items (sent), Drafts (drafts), Archive (archive), Deleted Items (deleted), Junk Email (junk)";
        assert_eq!(names(planned(&exchange, &[], false)), format!("{mail}, Receipts (custom)"));
        assert_eq!(names(planned(&exchange, &[], true)), format!("{mail}, Receipts (custom)"));
        // A folder here that follows one of the others already keeps being read.
        let tied = [("Notes", FolderKind::Custom), ("INBOX", FolderKind::Inbox), ("Gone", FolderKind::Custom)];
        assert_eq!(names(planned(&exchange, &tied, false)), format!("{mail}, Notes (custom), Receipts (custom)"));
        // On another kind of server every folder is one of mail, whatever it is called. Nor is
        // the same listing an English Exchange mailbox's when it comes from a server of Microsoft's.
        let other = listed(&["Notes", "Junk", "INBOX", "Trash", "Sent", "Tasks", "Sent Items"]);
        let all = "INBOX (inbox), Sent (sent), Trash (deleted), Junk (junk), Notes (custom), Tasks (custom), Sent Items (custom)";
        assert_eq!((names(planned(&other, &[], false)), names(planned(&other, &[], true))), (all.to_string(), all.to_string()));
        // A mailbox at Outlook.com that was never opened in Outlook: its "Notes" and "Outbox" are
        // Exchange's own, which only the server's name tells.
        let plain = listed(&["Archive", "Deleted Items", "Drafts", "INBOX", "Junk Email", "Notes", "Outbox", "Sent Items"]);
        assert_eq!(names(planned(&plain, &[], true)), mail);
        assert_eq!(names(planned(&plain, &[], false)), format!("{mail}, Notes (custom), Outbox (custom)"));
        assert!(planned(&[], &[], true).is_empty());
    }

    #[test]
    fn of_a_message_too_large_the_header_is_kept_and_a_note_says_so() {
        let header = b"From: Bob <bob@example.org>\r\nTo: ada@example.org\r\nSubject: =?utf-8?q?Gro=C3=9Fe_Datei?=\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\
            Message-ID: <big@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\
            X-MailCraft-Not-Downloaded: 1\r\nX-Other: kept\r\n\r\n";
        let fetched = Fetched {
            uid: 7,
            flags: vec!["\\Seen".into(), "\\Flagged".into()],
            internal_date: Some(" 1-Jul-2003 09:52:37 +0000".into()),
            body: Some(header.to_vec()),
        };
        let m = too_large(fetched, 60 * 1024 * 1024 + 1, MAX_MESSAGE);
        assert_eq!(
            (m.uid, m.subject.as_str(), m.from.email.as_str(), m.message_id.as_str()),
            (7, "Große Datei", "bob@example.org", "big@example.org")
        );
        assert_eq!((m.read, m.flag, m.date, m.received), (true, FlagStatus::Flagged, 1_057_049_557, 1_057_053_157));
        assert!(
            m.body.text.starts_with("This message is too large to download: it is 61 MB on the server, and MailCraft takes messages of up to 25 MB.")
        );
        assert!(m.body.html.is_none() && m.attachments.is_empty());
        // Its mark says how large it is, once: a sender can't plant another.
        let marks: Vec<&str> =
            m.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case(NOT_DOWNLOADED)).map(|(_, size)| size.as_str()).collect();
        assert_eq!(marks, ["62914561"]);
        assert!(m.headers.iter().any(|(name, value)| name == "X-Other" && value == "kept"));
        // A header that is none still makes a message that says what it is.
        let m = too_large(Fetched { uid: 8, body: Some(vec![0xFF, 0x00, b'x']), ..Default::default() }, u64::MAX, 0);
        assert!(m.uid == 8 && m.body.text.contains("too large to download") && m.body.text.contains("up to 0 MB"));
        assert_eq!((megabytes(0), megabytes(1), megabytes(25 * 1024 * 1024)), ("0 MB".to_string(), "1 MB".to_string(), "25 MB".to_string()));
    }

    #[test]
    fn names_for_a_line_of_progress() {
        assert_eq!(
            (label("INBOX"), label("Sent Items"), label("Projects/Apollo")),
            ("Inbox".to_string(), "Sent Items".to_string(), "Projects/Apollo".to_string())
        );
        assert_eq!((label("a\r\nb\u{7}"), label(&"é".repeat(100)).chars().count(), label("")), ("ab".to_string(), 40, String::new()));
        assert_eq!((messages(0), messages(1), messages(2)), ("0 messages".to_string(), "1 message".to_string(), "2 messages".to_string()));
        // The count is this run's; what a limit left for later is said with it.
        assert_eq!(progress("Inbox", 12, 500, 0), "Inbox: 12 of 500");
        assert_eq!(progress("Inbox", 12, 500, 4_500), "Inbox: 12 of 500 (4,500 more later)");
        assert_eq!((grouped(0), grouped(999), grouped(1_000), grouped(1_234_567)), ("0".into(), "999".into(), "1,000".into(), "1,234,567".into()));
    }

    proptest::proptest! {
        #[test]
        fn any_listing_is_planned_in_turn_and_without_what_is_no_mail(
            names in proptest::collection::vec("(INBOX|Calendar|Contacts|Tasks|Notes|Journal|Outbox|Sync Issues|Sent Items|Deleted Items|Junk Email|Drafts|x)(/[a-z]{0,3})?", 0..14),
            tied in proptest::collection::vec("(Notes|Calendar|x|)", 0..3),
            microsoft in proptest::bool::ANY,
        ) {
            let mailboxes = listed(&names.iter().map(String::as_str).collect::<Vec<_>>());
            let tied: Vec<(&str, FolderKind)> = tied.iter().map(|name| (name.as_str(), FolderKind::Custom)).collect();
            let plan = planned(&mailboxes, &tied, microsoft);
            proptest::prop_assert!(plan.len() <= mailboxes.len());
            // In turn, and nothing that is no mail unless a folder here follows it.
            proptest::prop_assert!(plan.windows(2).all(|pair| turn(pair[0].1) <= turn(pair[1].1)));
            if is_exchange(&mailboxes, microsoft) {
                proptest::prop_assert!(plan.iter().all(|(m, _)| !not_mail(m) || tied.iter().any(|(name, _)| *name == m.name)));
            } else {
                proptest::prop_assert_eq!(plan.len(), mailboxes.len());
            }
        }

        #[test]
        fn any_text_is_a_date_or_not(text in "[0-9A-Za-z:+ -]{0,40}") {
            let _ = internal_date(&text);
        }

        #[test]
        fn dates_written_the_imap_way_are_read_back(ts in 0i64..4_000_000_000, offset in -1439i32..=1439) {
            let t = DateTime::from_timestamp(ts, offset);
            let text = format!(
                "{:02}-{}-{:04} {:02}:{:02}:{:02} {}{:02}{:02}",
                t.date.day,
                mailcraft_time::month_short(t.date.month),
                t.date.year,
                t.hour,
                t.minute,
                t.second,
                if offset < 0 { '-' } else { '+' },
                offset.abs() / 60,
                offset.abs() % 60
            );
            proptest::prop_assert_eq!(internal_date(&text), Some(ts));
        }
    }
}
