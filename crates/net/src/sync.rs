// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_mime::WriteOptions;
use mailcraft_model::sync::{FolderDelta, FolderState, RemoteFlags, SyncPhase, SyncRequest, SyncResult, SyncStage, SyncStep};
use mailcraft_model::{Account, Body, FlagStatus, FolderKind, Id, Message, Security, Server};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{Fetched, Imap, Mailbox, uid_sets};
use mailcraft_time::{Date, DateTime, Timestamp};

use crate::push::push;
use crate::send::{Envelope, dated, deliver, open_smtp_with};
use crate::transport::{self, Patience, Service, Stream};
use crate::{NetError, kinds, not_signed_in, scrub};

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
/// The header a message kept without its text has, for a program to know it by: its size on
/// the server, in bytes.
const NOT_DOWNLOADED: &str = mailcraft_model::NOT_DOWNLOADED;
const NOT_DOWNLOADED_NOTE: &str = mailcraft_model::NOT_DOWNLOADED_NOTE;

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
    let mut run = Run {
        request,
        limits,
        cancel,
        emit,
        out: blank(request),
        stopped: false,
        handed_over: Instant::now(),
        phase: SyncPhase::Sending,
        asked: 0,
        counted: 0,
    };
    let sent = send_outbox(&mut run);
    let imap = exchange(&mut run, &sent, host);
    if let Some(imap) = &imap {
        run.note(imap);
    }
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
    limits: Limits,
    cancel: &'a AtomicBool,
    emit: &'a mut dyn FnMut(SyncResult),
    /// What happened since the last piece was handed over.
    pub(crate) out: SyncResult,
    /// The run was called off, and something was left undone for it.
    stopped: bool,
    /// When the last piece was handed over.
    handed_over: Instant,
    /// The phase the time since then is spent in: the one the last piece said (#276).
    phase: SyncPhase,
    /// Requests (commands, messages sent) since the last piece.
    asked: u32,
    /// How many of the connection's commands `asked` has taken in.
    counted: u32,
}

impl Run<'_> {
    /// Whether the run was called off. Once it was, it stays so, whatever becomes of the flag.
    pub(crate) fn cancelled(&mut self) -> bool {
        self.stopped |= self.cancel.load(Ordering::Relaxed);
        self.stopped
    }

    /// Hand over what happened since the last piece, with how far the run has got: as numbers,
    /// and as the line for a status bar that they make.
    pub(crate) fn hand_over(&mut self, step: SyncStep) {
        let mut piece = std::mem::replace(&mut self.out, blank(self.request));
        self.lap(&mut piece);
        self.phase = step.stage.phase();
        piece.partial = true;
        piece.progress = step.line();
        piece.step = Some(step);
        self.send(piece);
    }

    /// [`Run::hand_over`], having counted the commands sent on `imap` first.
    pub(crate) fn hand_over_in(&mut self, imap: &Imap<Stream<'_>>, step: SyncStep) {
        self.note(imap);
        self.hand_over(step);
    }

    /// Count the commands sent on `imap` since they were last counted.
    pub(crate) fn note(&mut self, imap: &Imap<Stream<'_>>) {
        let sent = imap.commands();
        self.asked = self.asked.saturating_add(sent.saturating_sub(self.counted));
        self.counted = sent;
    }

    /// Put the time and requests since the last piece into `piece`, under the phase they were in.
    fn lap(&mut self, piece: &mut SyncResult) {
        let asked = std::mem::take(&mut self.asked);
        piece.timings.add_time(self.phase, self.handed_over.elapsed(), asked);
    }

    /// Hand over the last piece.
    fn finish(mut self) {
        let mut piece = std::mem::replace(&mut self.out, blank(self.request));
        self.lap(&mut piece);
        piece.stopped = self.stopped;
        self.send(piece);
    }

    fn send(&mut self, mut piece: SyncResult) {
        // Whatever a server wrote back, the password or the token doesn't leave in an error line.
        for line in &mut piece.errors {
            *line = scrub(&scrub(line, &self.request.password), &self.request.access_token);
        }
        (self.emit)(piece);
        self.handed_over = Instant::now();
    }

    /// Note that something went wrong: `what`, and why. A command cut short because the run was
    /// called off is nothing to tell. Returns whether the connection is still good.
    fn failed(&mut self, what: &str, error: &NetError) -> bool {
        if matches!(error, NetError::Proto(ProtoError::Io(why)) if why == transport::STOPPED) {
            self.stopped = true;
        } else {
            self.out.errors.push(format!("{what}: {error}"));
        }
        !error.is_fatal()
    }
}

/// A message's subject for an error line: one line, not too long.
fn subject_of(message: &Message) -> String {
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
fn messages(count: usize) -> String {
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
    run.hand_over(SyncStep::new(SyncStage::Sending, "", 0, waiting));
    let mut smtp = match open_smtp_with(&request.account, request.secret(), &run.limits.patience, Some(run.cancel)) {
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
    // with the message, so that it knows the copy for what it is even if it hears no more.
    let filed_there = request.account.incoming.is_some();
    for (at, message) in request.outbox.iter().enumerate() {
        if run.cancelled() {
            break;
        }
        // Dated once, so that the copy kept on the server says what the one that went says.
        let message = dated(message);
        run.asked = run.asked.saturating_add(1);
        let outcome = Envelope::of(&request.account, &message).and_then(|envelope| deliver(&mut smtp, &envelope, &message));
        match outcome {
            Ok(()) => {
                run.out.sent.push(message.id);
                if filed_there {
                    run.out.filed.push(message.id);
                }
                sent.push(message.into_owned());
                run.hand_over(SyncStep::new(SyncStage::Sending, "", at + 1, waiting));
            }
            Err(e) => {
                run.out.errors.push(format!("Couldn't send \"{}\": {e}", subject_of(&message)));
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
        let microsoft = is_microsoft_host(&host);
        Host { files_sent: microsoft || host_in(&host, &["gmail.com", "googlemail.com"]), microsoft }
    }
}

/// Whether `host` (lower case, no trailing dot) is `domain` or a name under it.
fn host_in(host: &str, domains: &[&str]) -> bool {
    domains.iter().any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')))
}

/// Whether a mail server's name is Microsoft's (Microsoft 365, Outlook.com).
pub(crate) fn is_microsoft_host(host: &str) -> bool {
    host_in(&host.trim().trim_end_matches('.').to_ascii_lowercase(), &["office365.com", "outlook.com"])
}

/// A sign-in with a Microsoft access token (XOAUTH2) goes only to Microsoft's own mail servers
/// (or to this computer, for tests): an account that names another server, from a mailbox file
/// say, is refused before a connection is made, so that the token never leaves for a stranger.
pub(crate) fn token_may_go_to(server: &Server) -> Result<(), NetError> {
    if is_microsoft_host(&server.host) || crate::transport::is_loopback(&server.host) {
        return Ok(());
    }
    Err(NetError::Config("a Microsoft sign-in is only used with Microsoft's own mail servers: this account names another server".into()))
}

/// Put a copy of each message that was sent into the server's mailbox for sent mail, marked as
/// read. The mailbox here was told to expect the copies when the messages went; a copy that
/// didn't get there is said (`unfiled`). A server without a mailbox for sent mail gets none: the
/// copy here is the one there is. Returns whether the connection is still good.
fn file_sent(imap: &mut Imap<Stream<'_>>, plan: &[(&Mailbox, FolderKind)], sent: &[Message], result: &mut SyncResult) -> bool {
    let Some((mailbox, _)) = plan.iter().find(|(_, kind)| *kind == FolderKind::Sent) else {
        result.unfiled.extend(sent.iter().map(|m| m.id));
        return true;
    };
    for (at, message) in sent.iter().enumerate() {
        // The sender's own copy says who got a blind copy.
        let options = WriteOptions { include_bcc: true, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
        if let Err(e) = imap.append(&mailbox.raw, &["\\Seen"], &mailcraft_mime::write(message, &options)) {
            let e = NetError::from(e);
            result.errors.push(format!("Couldn't keep a copy of \"{}\" on the server: {e}", subject_of(message)));
            if e.is_fatal() {
                // The connection went while the copy was on its way: it may have arrived all
                // the same. Should it show, it is this message's, not another one. The ones
                // after it never set out.
                result.unfiled.extend(sent.iter().skip(at + 1).map(|m| m.id));
                return false;
            }
            result.unfiled.push(message.id);
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
    if account.oauth.is_none() && secret.is_empty() {
        return Err(NetError::Config("enter a password to sign in".into()));
    }
    if account.oauth.is_some() {
        token_may_go_to(server)?;
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
        imap.authenticate_password(user, secret)?;
    }
    Ok(imap)
}

/// What Exchange calls, in English, the folders of a mailbox that are no folders of mail to
/// keep: where it has appointments, people, tasks, notes and the journal, mail on its way out,
/// and its reports on synchronising with Outlook.
const NOT_MAIL: [&str; 7] = ["calendar", "contacts", "tasks", "notes", "journal", "outbox", "sync issues"];
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
                log::debug!("a folder that is not a folder of mail is left out");
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
    // kept on the server, and then nothing else is begun.
    let owed: Vec<Id> = if host.files_sent { Vec::new() } else { sent.iter().map(|m| m.id).collect() };
    if run.cancelled() && owed.is_empty() {
        return None;
    }
    run.hand_over(SyncStep::new(SyncStage::Connecting, "", 0, 0));
    let watched = owed.is_empty().then_some(run.cancel);
    let mut imap = match open_imap_with(&request.account, request.secret(), &run.limits.patience, watched) {
        Ok(imap) => imap,
        Err(e) => {
            run.failed("Couldn't check for new mail", &e);
            // The copies that were to be kept there are not.
            run.out.unfiled.extend(owed);
            return None;
        }
    };
    let mailboxes = match imap.list() {
        Ok(list) => list,
        Err(e) => {
            let alive = run.failed("Couldn't read the list of folders", &e.into());
            run.out.unfiled.extend(owed);
            return alive.then_some(imap);
        }
    };
    // Folders that already follow a server mailbox keep their job.
    let tied: Vec<(&str, FolderKind)> = request.folders.iter().filter(|f| !f.remote.is_empty()).map(|f| (f.remote.as_str(), f.kind)).collect();
    let plan = planned(&mailboxes, &tied, host.microsoft);
    if plan.len() > MAX_FOLDERS {
        run.out.errors.push(format!("The server has {} folders; only the first {MAX_FOLDERS} were read", plan.len()));
    }
    // The whole tree first: the folders are there to see before their mail is.
    run.out.listed = listed(&plan);
    if !run.out.listed.is_empty() {
        let found = if run.out.listed.len() == 1 { 1 } else { plan.len().min(MAX_FOLDERS) };
        run.hand_over_in(&imap, SyncStep::new(SyncStage::Listed, "", 0, found));
    }
    // First what the server doesn't know yet, so that what is read afterwards shows it. None of
    // it is left half done because the run was called off, and when the connection is lost on
    // the way, everything else waits for the next run.
    imap.get_mut().watch(None);
    if !owed.is_empty() {
        run.hand_over_in(&imap, SyncStep::new(SyncStage::Filing, "", 0, 0));
        if !file_sent(&mut imap, &plan, sent, &mut run.out) {
            return None;
        }
    }
    if run.cancelled() {
        return Some(imap);
    }
    let arrived = push(run, &mut imap, &mailboxes)?;
    // Asked only to tell, not to read: done.
    if request.push_only {
        return Some(imap);
    }
    // From here on the server is only read: the run can be called off in the middle of a command.
    imap.get_mut().watch(Some(run.cancel));
    // A local folder takes the news of one server mailbox only.
    let mut claimed: HashSet<Id> = HashSet::new();
    let count = plan.len().min(MAX_FOLDERS);
    for (at, (mailbox, kind)) in plan.into_iter().take(MAX_FOLDERS).enumerate() {
        if run.cancelled() {
            break;
        }
        let remote = kinds::remote_name(mailbox);
        // A folder without a name couldn't be told from one that only exists here.
        if remote.is_empty() {
            continue;
        }
        let state = state_for(request, &remote, kind).filter(|state| claimed.insert(state.id));
        let arrived = arrived.get(&remote).map_or(&[][..], Vec::as_slice);
        let folder = FolderSync { mailbox, remote, kind, state, max_new: request.max_per_folder as usize, arrived, place: (at + 1, count) };
        if !folder.run(&mut imap, run) {
            // The connection is gone: the other mailboxes wait for the next run.
            return None;
        }
    }
    Some(imap)
}

/// A size for a person to read: `100 MB`.
fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1024 * 1024))
}

/// The request's folder for a server mailbox: the one already tied to that name, else, for a
/// standard kind, the local folder of that kind that isn't tied to a mailbox yet.
fn state_for<'a>(request: &'a SyncRequest, remote: &str, kind: FolderKind) -> Option<&'a FolderState> {
    let by_name = request.folders.iter().find(|f| !f.remote.is_empty() && f.remote == remote);
    by_name.or_else(|| request.folders.iter().find(|f| kind != FolderKind::Custom && f.remote.is_empty() && f.kind == kind))
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
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
    message.received = fetched.internal_date.as_deref().and_then(internal_date).unwrap_or(message.date);
    message
}

/// What is kept of a message that is too large to download (`size` bytes, where `largest` is
/// what is taken): its header as the server handed it over, and in place of its text a note
/// that says so, and its size on the server (#300: [`Message::not_downloaded`], which the list
/// shows, and the header [`NOT_DOWNLOADED`] for other programs). It has its UID like any other
/// message, so it is not asked for again; flags, moves and deletions reach the whole message on
/// the server.
fn too_large(header: Fetched, size: u64, largest: u64) -> Message {
    let mut message = message_from(header);
    message.attachments.clear();
    message.body = Body {
        text: format!(
            "{NOT_DOWNLOADED_NOTE} it is {} on the server, and MailCraft takes messages of up to {}.\n\n\
             What is here comes from its header alone: who sent it, when, and what it is about. The message itself, \
             with anything attached to it, is still on the server. Read it there with another mail program or on the web.\n",
            megabytes(size),
            megabytes(largest)
        ),
        html: None,
    };
    message.headers.retain(|(name, _)| !name.eq_ignore_ascii_case(NOT_DOWNLOADED));
    message.headers.push((NOT_DOWNLOADED.into(), size.to_string()));
    message.not_downloaded = size;
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
    /// How many of the mailbox's newest messages to have here; 0 for all of them.
    max_new: usize,
    /// Messages a move of this run put here: the `UIDVALIDITY` they were put under, and their UIDs.
    arrived: &'a [(u32, u32)],
    /// Which of the run's mailboxes this is, and of how many.
    place: (usize, usize),
}

impl FolderSync<'_> {
    /// Read the mailbox and note what changed in the run's answer. Returns whether the
    /// connection is still good for the next mailbox.
    ///
    /// New messages come newest first, so that the latest mail is what shows first, and are
    /// handed over a batch at a time. The mailbox's count of how far it was read (`uid_seen`)
    /// stays, piece after piece, just below the lowest message still to come; only the piece
    /// with the last of them moves it past them all. So wherever the pieces stop being taken in,
    /// every message that came is known here by its UID and is not asked for again, and every
    /// message that didn't is above the count and unknown, which is what makes the next run ask
    /// for it. Messages left out because only so many are taken (`max_new`) are below the count
    /// from the first piece on, as they are when the run goes through, and below how far down
    /// the mailbox is read (`uid_from`), which only the last piece moves: when the limit grows
    /// to take them in, they are asked for.
    fn run(&self, imap: &mut Imap<Stream<'_>>, run: &mut Run) -> bool {
        let name = shown(&self.remote);
        let label = label(&self.remote);
        run.hand_over_in(imap, SyncStep::new(SyncStage::Checking, &label, self.place.0, self.place.1));
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
        if self.state.is_some() && !reset {
            // What a move just put here is here already: only its UID is new, and the result says it.
            known.extend(self.arrived.iter().filter(|(validity, uid)| *validity == selected.uid_validity && *uid != 0).map(|(_, uid)| *uid));
        }
        let uid_seen = self.state.filter(|_| !reset).map_or(0, |s| s.uid_seen);
        let uid_from = self.state.filter(|_| !reset).map_or(0, |s| s.uid_from);

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

        // What to download, in UID order: what isn't here of the newest the limit allows (of
        // all, without one), above how far the mailbox was read or below how far down. So what
        // a smaller limit left out comes down once it is raised, and what left the folder here
        // without a word to the server (moved to another account) doesn't come back.
        let older = if self.max_new > 0 { present.len().saturating_sub(self.max_new) } else { 0 };
        let oldest = present.keys().nth(older).copied();
        let wanted: Vec<u32> =
            present.keys().skip(older).copied().filter(|uid| (*uid > uid_seen || *uid < uid_from) && !known.contains(uid)).collect();
        // How far the mailbox is read while messages are still to come.
        let below = wanted.first().map_or(uid_seen, |lowest| lowest.saturating_sub(1).max(uid_seen));
        // What is said once, with the first piece: the flags of the messages already here, and
        // which of them are gone.
        let mut delta = FolderDelta {
            id: self.state.map(|s| s.id),
            remote: self.remote.clone(),
            kind: self.kind,
            uid_validity: selected.uid_validity,
            uid_seen: below,
            uid_from: 0,
            new: Vec::new(),
            flags: known.iter().filter_map(|uid| present.get(uid).map(|m| remote_flags(m))).collect(),
            removed: match self.state {
                Some(state) if reset => state.known_uids.iter().copied().filter(|uid| *uid != 0).collect(),
                _ => known.iter().copied().filter(|uid| !present.contains_key(uid)).collect(),
            },
            complete: false,
        };
        let mut alive = true;
        // The lowest UID that is wanted but didn't arrive: the next run starts again from there.
        let mut missed: Option<u32> = None;
        // Messages that were asked for and not handed over, without a word why.
        let mut silent: Vec<u32> = Vec::new();
        let sizes = self.sizes(imap, &wanted, run, &mut alive);
        let total = wanted.len();
        // The bytes of the messages waiting to be handed over.
        let mut waiting = 0usize;
        // Newest first, so that when a run is cut short the latest mail is what is already here.
        for (at, uid) in wanted.iter().rev().copied().enumerate() {
            if !alive || run.cancelled() {
                // This one and all below it.
                missed = wanted.first().copied();
                break;
            }
            let size = sizes.get(&uid).copied().unwrap_or(0);
            // Of a message too large to take, the header is.
            let large = size > run.limits.largest;
            let fits = large || usize::try_from(size).unwrap_or(usize::MAX) <= run.limits.bytes;
            if run.limits.messages == 0 || run.limits.bytes == 0 || !fits {
                // Enough for one run; not a problem, the rest comes with the next one.
                missed = Some(uid);
                continue;
            }
            let fetched = if large { imap.uid_fetch_header(uid) } else { imap.uid_fetch_message(uid) };
            match fetched {
                Ok(Some(fetched)) => {
                    let bytes = fetched.body.as_ref().map_or(0, Vec::len);
                    run.limits.messages = run.limits.messages.saturating_sub(1);
                    run.limits.bytes = run.limits.bytes.saturating_sub(bytes);
                    waiting = waiting.saturating_add(bytes);
                    delta.new.push(if large { too_large(fetched, size, run.limits.largest) } else { message_from(fetched) });
                }
                // Gone between the listing and now, as a rule (looked into below).
                Ok(None) => silent.push(uid),
                Err(e) => {
                    alive = run.failed(&format!("Couldn't download a message from \"{name}\""), &e.into());
                    missed = Some(uid);
                    continue;
                }
            }
            let full = delta.new.len() >= run.limits.batch_messages || waiting >= run.limits.batch_bytes;
            let due = !delta.new.is_empty() && run.handed_over.elapsed() >= run.limits.batch_wait;
            // The last of them go with what ends the mailbox, below.
            if at + 1 < total && (full || due) {
                let rest = FolderDelta {
                    id: delta.id,
                    remote: delta.remote.clone(),
                    kind: delta.kind,
                    uid_validity: delta.uid_validity,
                    uid_seen: below,
                    ..Default::default()
                };
                let mut batch = std::mem::replace(&mut delta, rest);
                batch.new.sort_by_key(|m| m.uid);
                run.out.folders.push(batch);
                run.hand_over_in(imap, SyncStep::new(SyncStage::Reading, &label, at + 1, total));
                waiting = 0;
            }
        }
        if alive && !silent.is_empty() && !run.cancelled() {
            // A message the mailbox still has is not passed over for the server's silence.
            let kept_back = self.kept_back(imap, &silent, run, &mut alive);
            if let Some(lowest) = kept_back.iter().copied().min() {
                run.out.errors.push(format!("The server didn't hand over {} of \"{name}\"", messages(kept_back.len())));
                missed = Some(missed.map_or(lowest, |missed| missed.min(lowest)));
            }
        }
        delta.uid_seen = match missed {
            Some(missed) => missed.saturating_sub(1).max(below),
            None => highest.unwrap_or(uid_seen),
        };
        // Read down to the oldest the limit allows, unless older mail asked for is still to come.
        delta.uid_from = match (missed, oldest) {
            (Some(missed), _) if missed < uid_from => uid_from,
            (_, Some(oldest)) if uid_from == 0 || oldest < uid_from => oldest,
            _ => uid_from,
        };
        // Everything wanted came: the folder is read whole (an empty one at its first reading).
        delta.complete = alive && missed.is_none();
        delta.new.sort_by_key(|m| m.uid);
        log::debug!("a folder: {total} wanted, {} gone, read up to {}", delta.removed.len(), delta.uid_seen);
        // It goes with the next piece: the next mailbox's first line, or the end of the run.
        run.out.folders.push(delta);
        alive
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
    /// known before it is asked for. A server that won't say gets the benefit of the doubt.
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
#[path = "sync_size_tests.rs"]
mod size_tests;

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
