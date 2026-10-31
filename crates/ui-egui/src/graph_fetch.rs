// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of a Microsoft account fetched whole (R3.2; R3 spec S12, S14, S15): in the
//! background, newest first, within the account's "Download messages in full"; at once, ahead of
//! that, for the message the owner opens (after the batch under way, when there is one: one at
//! a time); and before Reply, Reply All, Forward, Save Message As
//! and Save Attachment, which then go on. Only while a Graph token from this session is good:
//! each Send/Receive renews it. Cancel (Stop) stops it all until the next Send/Receive starts,
//! the owner's or one by itself; Send/Receive Progress says how far it has got, account by account
//! (the engine counts them as they come in, [`mailcraft_engine::cmd::sync_progress::FullMessages`]).
//! A forward of several messages (#360) waits for every one of them, an account's at a time.
//!
//! A JMAP account's messages come the same way (M6), at once and before a reply alike, through
//! [`crate::Services::jmap_fetch`] with the secret its Send/Receive went with
//! ([`crate::jmap_mail`]), and are taken in by the engine's JMAP side, which holds them to sizes
//! first.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::{compose, ews_mail, graph_mail, jmap_mail};
use mailcraft_model::sync::{GraphFetchRequest, GraphFetchResult};
use mailcraft_model::{AccountKind, Id};
use serde_json::{Value, json};

use crate::ews_mail::EwsFetchJob;
use crate::jmap_mail::JmapFetchJob;
use crate::{MailApp, now_ms};

/// Messages asked for at a time: in the background, and for a forward of several (#360).
const BATCH: usize = 10;
/// How long the background waits before looking again when there was nothing to fetch.
const IDLE_MS: f64 = 20_000.0;
/// The longest the background waits after batches that brought nothing, one after another: the
/// wait doubles from [`IDLE_MS`] up to this.
const MOST_IDLE_MS: f64 = 15.0 * 60_000.0;

/// What the host is asked to fetch ([`crate::Services::graph_fetch`]).
pub struct GraphFetchJob {
    pub request: GraphFetchRequest,
    /// A Graph token of this session that covers the mail.
    pub access: String,
    pub cancel: Arc<AtomicBool>,
}

/// The state of fetching, in the window.
#[derive(Default)]
pub struct Fetching {
    /// The background's batch under way: for which account, under which opening of the mailbox
    /// ([`mailcraft_engine::Session::openings`]), and the answer to come.
    background: Option<(Id, u64, Receiver<GraphFetchResult>)>,
    /// The owner's messages under way: which ones (one, or of a forward of several, #360), under
    /// which opening, what calls it off, and the answer to come.
    now: Option<(Vec<Id>, u64, Arc<AtomicBool>, Receiver<GraphFetchResult>)>,
    /// What waits for those messages: a command, its params, the mailbox opening it was asked
    /// under ([`mailcraft_engine::Session::openings`]), for a forward of several every message
    /// fetched for it so far, and the messages it waits for (of those under way, it may wait for
    /// some only: it took the place of a forward, or joined its batch).
    then: Option<(String, Value, u64, Option<Vec<Id>>, Vec<Id>)>,
    /// The message being written when the command that waits was asked to let it go
    /// (`discard`), as it was then: run again, it lets go of that one alone (#513).
    let_go: Option<mailcraft_engine::Compose>,
    /// A command whose messages came while a form with changes that aren't saved was open (a
    /// contact, a task, an appointment): it goes on once that form is closed, for its question
    /// would take the form's place (#513). As [`Fetching::then`], with a forward's tried.
    held: Option<(String, Value, u64, Option<Vec<Id>>)>,
    /// While a forward of several runs again once messages came: those fetched for it already,
    /// not asked for again ([`MailApp::fetch_first`]).
    tried: Option<Vec<Id>>,
    /// The message last asked for when it was opened (not asked again while it stays open).
    opened: Option<Id>,
    pub(crate) cancel: Arc<AtomicBool>,
    idle_until: f64,
    /// The last try couldn't reach Microsoft.
    pub(crate) offline: bool,
    /// Accounts whose background batches stopped with nothing fetched, one after another: how
    /// many times, and until when (milliseconds) the account is left alone. Each wait is twice
    /// the one before, and the other accounts go on meanwhile.
    stuck: std::collections::HashMap<Id, (u32, f64)>,
    /// For the status bar: how many are fetched of how many, while some aren't.
    pub(crate) progress: String,
}

impl Fetching {
    /// Whether anything is being fetched, or a command whose messages came waits to go on.
    pub fn busy(&self) -> bool {
        self.background.is_some() || self.now.is_some() || self.held.is_some()
    }

    /// Something new may be waiting: the background looks again at once.
    pub(crate) fn wake(&mut self) {
        self.idle_until = 0.0;
    }

    /// Whether `message` is being fetched for the owner now.
    pub fn fetching(&self, message: Id) -> bool {
        self.now.as_ref().is_some_and(|(m, ..)| m.contains(&message))
    }

    /// Another mailbox took the place of the one these were for: what is under way is called off,
    /// its answers are no longer listened to, and the next mailbox starts afresh. (A command that
    /// waited for one of them knows its own mailbox, and doesn't run in another.)
    pub(crate) fn forget(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some((_, _, cancel, _)) = self.now.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        (self.background, self.opened) = (None, None);
        self.stuck.clear();
        self.progress.clear();
        (self.idle_until, self.offline) = (0.0, false);
    }
}

/// The commands other than a forward that need a message whole, and the message they are about.
fn needs_whole(app: &MailApp, id: &str, params: &Value) -> Option<Id> {
    if !matches!(
        id,
        "mail.reply"
            | "mail.replyAll"
            | "mail.replyAndDelete"
            | "mail.editDraft"
            | "mail.resend"
            | "file.exportMessage"
            | "mail.saveAttachment"
            | "mail.saveAllAttachments"
            | "mail.openAttachment"
            | "mail.viewSource"
            | "compose.attachItem"
    ) {
        return None;
    }
    let selected = match app.session.sel.messages.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    // Read as the command reads it (#553): `7.0` is message 7, and a message named wrongly is
    // refused by the command, so nothing is fetched for it.
    use mailcraft_engine::p;
    let named = match id {
        "mail.reply" | "mail.replyAll" | "mail.replyAndDelete" | "mail.editDraft" | "mail.resend" => p::message_strict(params).ok()?,
        "mail.viewSource" | "mail.saveAllAttachments" | "compose.attachItem" => p::id_strict(params, "id").ok()?,
        _ => p::id(params, "id"),
    };
    to_fetch(app, id, named.or(selected)?)
}

/// `message`, when command `id` waits for it to be fetched whole first.
fn to_fetch(app: &MailApp, id: &str, message: Id) -> Option<Id> {
    // One whose server's copy went before it could be fetched never will be: Forward as Attachment
    // says so itself (the others have no word of their own for it, and wait as they did).
    let never = |m: &mailcraft_model::Message| m.incomplete && id == "mail.forwardAsAttachment";
    app.session.mailbox.message(message).filter(|m| m.unfetched && !never(m) && m.server.is_some()).map(|m| m.id)
}

/// The messages a forward takes, as the engine takes them, and the one in focus. `None` for
/// another command, and for what the engine refuses (it says why once the command runs).
fn forwarded(app: &mut MailApp, id: &str, params: &Value) -> Option<(Vec<Id>, Id)> {
    if !matches!(id, "mail.forward" | "mail.forwardAsAttachment") {
        return None;
    }
    compose::forwarded(&mut app.session, params).ok()
}

/// What a forward that waits for its messages does, as [`MailApp::fetch_first`] named it: the
/// messages it takes, the one in focus, and whether what is being written is let go. (With
/// several, Forward and Forward as Attachment do the same.) `None` for another command.
fn forward_of(command: &str, params: &Value) -> Option<(Vec<Id>, Option<Id>, bool)> {
    if !matches!(command, "mail.forward" | "mail.forwardAsAttachment") {
        return None;
    }
    let mut messages: Vec<Id> = params.get("ids")?.as_array()?.iter().filter_map(Value::as_u64).collect();
    let focus = messages.last().copied();
    messages.sort_unstable();
    Some((messages, focus, params.get("discard").and_then(Value::as_bool) == Some(true)))
}

/// Of `ids`, those still to fetch first, but `tried`: of the first one's account, at most
/// [`BATCH`]. A Microsoft account's server doesn't say how large its messages are before they come
/// (a delta gives no sizes), so a forward of more than 64 MB together is found out after at most
/// that many more have come.
fn still_to_fetch(app: &MailApp, ids: &[Id], tried: &[Id]) -> Vec<Id> {
    let waiting = |m: &&mailcraft_model::Message| m.unfetched && m.server.is_some() && !tried.contains(&m.id);
    let mut waiting = ids.iter().filter_map(|id| app.session.mailbox.message(*id)).filter(waiting);
    let Some(first) = waiting.next() else { return Vec::new() };
    let account = first.account;
    std::iter::once(first.id).chain(waiting.filter(|m| m.account == account).map(|m| m.id)).take(BATCH).collect()
}

/// A Graph token of this session for `account` that is still good.
fn access(app: &MailApp, account: Id) -> Option<String> {
    app.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone())
}

/// How messages of an account are fetched now, when they can be: by a host that fetches for its
/// kind, with a Graph token of this session that is still good, or with a JMAP account's secret.
enum Way {
    Graph(String),
    Jmap(String),
    Ews(String),
}

fn way(app: &MailApp, account: Id) -> Option<Way> {
    match app.session.mailbox.account(account)?.kind {
        AccountKind::Microsoft if app.services.graph_fetch.is_some() => access(app, account).map(Way::Graph),
        // (Not with a credential its server turned down: the next Send/Receive the owner asks for first.)
        AccountKind::Jmap if app.services.jmap_fetch.is_some() && !app.jmap_refused.contains(&account) => app.jmap_secret(account).map(Way::Jmap),
        AccountKind::Exchange if app.services.ews_fetch.is_some() && app.session.ews && !app.jmap_refused.contains(&account) => {
            app.ews_secret(account).map(Way::Ews)
        }
        _ => None,
    }
}

/// Whether the host fetches messages of accounts of this kind.
fn fetched(app: &MailApp, kind: AccountKind) -> bool {
    match kind {
        AccountKind::Microsoft => app.services.graph_fetch.is_some(),
        AccountKind::Jmap => app.services.jmap_fetch.is_some(),
        AccountKind::Exchange => app.services.ews_fetch.is_some() && app.session.ews,
        _ => false,
    }
}

/// Take in messages fetched whole, as their account's kind is taken in (a JMAP account's are held
/// to sizes first).
fn take_in(app: &mut MailApp, result: GraphFetchResult) -> Result<Value, mailcraft_engine::CmdError> {
    let kind = app.session.mailbox.account(result.account).map(|a| a.kind);
    if kind == Some(AccountKind::Jmap) {
        jmap_mail::apply_jmap_fetch(&mut app.session, result)
    } else if kind == Some(AccountKind::Exchange) {
        ews_mail::apply_ews_fetch(&mut app.session, result)
    } else {
        graph_mail::apply_graph_fetch(&mut app.session, result)
    }
}

impl MailApp {
    /// A command that needs its message whole waits for it to be fetched, and then runs. A forward
    /// of several messages (#360) is judged first (as many as one message carries, none that
    /// stands in for a message for good) and waits for each of them, an account's at a time and a
    /// batch at a time, judged again as each batch comes; once none is left to fetch it runs.
    pub(crate) fn fetch_first(&mut self, id: &str, params: &Value) -> Option<Result<Value, String>> {
        let tried = self.fetching.tried.take();
        let forward = forwarded(self, id, params);
        let several = forward.as_ref().is_some_and(|(ids, _)| ids.len() > 1);
        let messages = match &forward {
            Some((ids, _)) if several => {
                if let Err(e) = compose::forward_check(&self.session, ids, true) {
                    return Some(Err(self.kept(e)));
                }
                still_to_fetch(self, ids, tried.as_deref().unwrap_or_default())
            }
            Some((_, focus)) => to_fetch(self, id, *focus).into_iter().collect(),
            None => needs_whole(self, id, params).into_iter().collect(),
        };
        let first = *messages.first()?;
        let account = self.session.mailbox.message(first)?.account;
        if !self.fetch_now(account, &messages) {
            if self.jmap_refused.contains(&account) {
                return Some(Err(tl!("The server turned down the sign-in for this account. Use Send/Receive to sign in again.").to_string()));
            }
            // Of several, the one that can't come is named, as the engine names it.
            if several && let Err(e) = compose::forward_check(&self.session, &messages, false) {
                return Some(Err(self.kept(e)));
            }
            return Some(Err(tl!("This message isn't on this computer yet. It will download when you're online.").to_string()));
        }
        // Named, for the selection may have moved on by the time they are here. Another command's
        // `id`, if the call gave one, stays as it was, for the command to judge; a forward's
        // messages are named by `ids` instead, as the engine took them, the one in focus last.
        let mut then = params.as_object().cloned().unwrap_or_default();
        match &forward {
            Some((ids, focus)) => {
                let named: Vec<Id> = ids.iter().copied().filter(|m| m != focus).chain([*focus]).collect();
                then.remove("id");
                then.insert("ids".into(), json!(named));
            }
            None if then.get("id").is_none_or(Value::is_null) => {
                then.insert("id".into(), json!(first));
            }
            None => {}
        }
        let tried = several.then(|| tried.unwrap_or_default());
        let then = Value::Object(then);
        // One command waits at a time: a forward of several still waiting in this mailbox is
        // called off, and said to be, for its download can take a while (#360). The same forward
        // asked for again goes on.
        let called_off = self.fetching.then.as_ref().is_some_and(|(command, params, opening, waiting, _)| {
            waiting.is_some() && *opening == self.session.openings() && forward_of(command, params).is_none_or(|f| forward_of(id, &then) != Some(f))
        });
        let waits = forward.map_or_else(|| vec![first], |(ids, _)| ids);
        let discard = then.get("discard").and_then(Value::as_bool) == Some(true);
        self.fetching.let_go = if discard { self.session.compose.clone() } else { None };
        // So is one whose messages came and that waits for a form to be closed, and said to be.
        let held_off = self.fetching.held.take().is_some_and(|(_, _, opening, _)| opening == self.session.openings());
        self.fetching.then = Some((id.to_string(), then, self.session.openings(), tried, waits));
        self.status(if called_off {
            tl!("The forward of several messages that was waiting was called off: another download took its place.")
        } else if held_off {
            tl!("What was waiting for the form to be closed was called off: another download took its place.")
        } else if several {
            tl!("Downloading the messages first…")
        } else {
            tl!("Downloading the message first…")
        });
        Some(Ok(json!({"pending": "download", "message": first})))
    }

    /// Run `command` again, whose messages came: at once, or, while a form with changes that
    /// aren't saved is open, once it is closed ([`MailApp::run_held`]): what it asks (Unsaved
    /// Message, a confirmation) would take the form's place, and what was typed in it with it
    /// (#513).
    fn run_waited(&mut self, command: String, params: Value, opening: u64, tried: Option<Vec<Id>>) {
        if matches!(&self.dialog, Some(crate::dialogs::Dialog::Form(form)) if form.changed()) {
            self.fetching.held = Some((command, params, opening, tried));
            return;
        }
        let params = self.kept_from(params);
        self.fetching.tried = tried;
        let _ = self.run(&command, params);
        self.fetching.tried = None;
    }

    /// The command that waited for a form to be closed goes on, in the mailbox it was asked of.
    fn run_held(&mut self) {
        let Some((command, params, opening, tried)) = self.fetching.held.take() else { return };
        if opening == self.session.openings() {
            self.run_waited(command, params, opening, tried);
        }
    }

    /// What a command that waited runs again with: it lets go of the message being written
    /// (`discard`) only when that is the one it was asked to let go of, as it was then. One
    /// opened or changed since is the owner's to answer for, asked as usual (#513).
    fn kept_from(&mut self, mut params: Value) -> Value {
        let asked = self.fetching.let_go.take();
        if params.get("discard").and_then(Value::as_bool) == Some(true)
            && self.session.compose != asked
            && let Some(p) = params.as_object_mut()
        {
            p.remove("discard");
        }
        params
    }

    /// Whether `message`, the one the reading pane shows, is fetched once the batch under way has
    /// come: one at a time, and a forward's can take a while.
    pub(crate) fn opened_after_batch(&self, message: Id) -> bool {
        self.fetching.now.is_some() && self.fetching.opened != Some(message) && self.session.sel.messages == [message]
    }

    /// The engine's refusal `e`, kept to be said in the interface language ([`MailApp::run`]).
    fn kept(&mut self, e: mailcraft_engine::CmdError) -> String {
        let said = e.to_string();
        self.last_error = Some(e);
        said
    }

    /// Fetch `messages` of `account` at once, ahead of the background. Whether they could be
    /// asked for.
    fn fetch_now(&mut self, account: Id, messages: &[Id]) -> bool {
        if messages.iter().all(|m| self.fetching.fetching(*m)) {
            return true;
        }
        let Some(way) = way(self, account) else {
            self.fetching.offline = true;
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(answer) = self.hand_over(account, way, messages, 1, cancel.clone()) else { return false };
        // What these take the place of is called off: nobody waits for it now. The message open
        // in the reading pane, if it was of it, is asked for again once these are here.
        if let Some((before, _, called_off, _)) = self.fetching.now.replace((messages.to_vec(), self.session.openings(), cancel, answer)) {
            called_off.store(true, Ordering::Relaxed);
            if self.fetching.opened.is_some_and(|m| before.contains(&m) && !messages.contains(&m)) {
                self.fetching.opened = None;
            }
        }
        true
    }

    /// Hand the host a fetch of `account`'s messages: `messages` alone, or, when there are none,
    /// the next `most` in the background. `None` when there is nothing to fetch.
    fn hand_over(&self, account: Id, way: Way, messages: &[Id], most: usize, cancel: Arc<AtomicBool>) -> Option<Receiver<GraphFetchResult>> {
        // Working offline, nothing is fetched: what is here is shown.
        if self.offline() {
            return None;
        }
        match way {
            Way::Graph(access) => {
                let fetch = self.services.graph_fetch.as_ref()?;
                let request = asked(messages, |m| graph_mail::graph_fetch_request(&self.session, account, m, most).ok(), |r| &mut r.ids)?;
                Some(fetch(GraphFetchJob { request, access, cancel }))
            }
            Way::Jmap(secret) => {
                let fetch = self.services.jmap_fetch.as_ref()?;
                let request = asked(messages, |m| jmap_mail::jmap_fetch_request(&self.session, account, m, most).ok(), |r| &mut r.ids)?;
                Some(fetch(JmapFetchJob { request, secret, cancel }))
            }
            Way::Ews(secret) => {
                let fetch = self.services.ews_fetch.as_ref()?;
                let request = asked(messages, |m| ews_mail::ews_fetch_request(&self.session, account, m, most).ok(), |r| &mut r.ids)?;
                Some(fetch(EwsFetchJob { request, secret, cancel }))
            }
        }
    }

    /// Each frame: take in what came, fetch what the owner opened, and keep the background going.
    /// (Public for a host's tests, which have no window.)
    pub fn drive_fetch(&mut self) {
        self.run_held();
        self.poll_fetch_now();
        self.poll_fetch_background();
        if !self.session.mailbox.accounts.iter().any(|a| fetched(self, a.kind)) {
            return;
        }
        // The message the reading pane shows, at once.
        let shown = match self.session.sel.messages.as_slice() {
            [one] => Some(*one),
            _ => None,
        };
        if shown != self.fetching.opened {
            let to_fetch =
                shown.and_then(|m| self.session.mailbox.message(m)).filter(|m| m.unfetched && m.server.is_some()).map(|m| (m.account, m.id));
            match to_fetch {
                Some((_, m)) if self.opened_after_batch(m) => {}
                Some((account, m)) => {
                    self.fetching.opened = shown;
                    self.fetch_now(account, &[m]);
                }
                None => self.fetching.opened = shown,
            }
        }
        // The background, a few at a time.
        if self.fetching.background.is_some() || self.session.sync_progress.full_paused || now_ms() < self.fetching.idle_until {
            return;
        }
        let accounts: Vec<Id> = self.session.mailbox.accounts.iter().filter(|a| fetched(self, a.kind)).map(|a| a.id).collect();
        let now = now_ms();
        for account in accounts {
            if self.fetching.stuck.get(&account).is_some_and(|(_, until)| now < *until) {
                continue;
            }
            let Some(way) = way(self, account) else { continue };
            let cancel = Arc::new(AtomicBool::new(false));
            let Some(answer) = self.hand_over(account, way, &[], BATCH, cancel.clone()) else { continue };
            self.fetching.cancel = cancel;
            self.fetching.background = Some((account, self.session.openings(), answer));
            return;
        }
        self.fetching.idle_until = now_ms() + IDLE_MS;
        self.fetching.progress.clear();
    }

    fn poll_fetch_now(&mut self) {
        let Some((_, opening, _, rx)) = &self.fetching.now else { return };
        let opening = *opening;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => GraphFetchResult { offline: true, ..Default::default() },
        };
        let messages = self.fetching.now.take().map(|(messages, ..)| messages).unwrap_or_default();
        // Asked for in a mailbox that another has taken the place of: it is nobody's here (nor is
        // a command that waits for it, below).
        if opening != self.session.openings() {
            return;
        }
        // What came is judged for the messages the command that waits waits for.
        let judged: Vec<Id> = match &self.fetching.then {
            Some((.., waits)) => messages.iter().copied().filter(|m| waits.contains(m)).collect(),
            None => messages.clone(),
        };
        let came = Came::of(self, &judged, &result);
        // Whether the batch brought any of its messages: one cut short before it came to those
        // the command waits for is asked for again, as for its own.
        let brought = !Came::of(self, &messages, &result).answered.is_empty();
        self.fetching.offline = result.offline;
        let account = result.account;
        let mut refused = if account != 0 { take_in(self, result).err() } else { None };
        if let Some(e) = &refused {
            self.status(e.to_string());
        }
        let whole = |app: &MailApp, m: &Id| app.session.mailbox.message(*m).is_some_and(|m| !m.unfetched);
        if let Some((command, params, opening, tried, waits)) = self.fetching.then.take() {
            // Asked of a mailbox that another has taken the place of: the message it names is not
            // this one's, whatever has that id here.
            if opening != self.session.openings() {
                return;
            }
            // A forward of several stops when what came wasn't taken in, saying why, and at the
            // first, in the order asked, that its server didn't send or that came but wasn't kept
            // here, and when a batch brought none of its messages; else it goes on with the rest, if
            // there are any, and runs once none is left to fetch.
            if let Some(mut tried) = tried {
                // Those of this batch it waits for may have come by other means meanwhile (the
                // background): then it goes on.
                let here = !judged.is_empty() && judged.iter().all(|m| whole(self, m));
                let not_kept = came.sent.iter().find(|m| self.session.mailbox.message(**m).is_some_and(|m| m.unfetched)).copied();
                let at = |m: Id| judged.iter().position(|x| *x == m);
                let stop = match (came.not_sent, not_kept) {
                    // Not taken in as a whole: that is why, whatever it said of each.
                    _ if refused.is_some() => refused.take(),
                    (Some((m, gone)), k) if k.is_none_or(|k| at(m) < at(k)) => Some(compose::not_sent(&self.session, m, gone)),
                    (_, Some(k)) => Some(compose::not_kept(&self.session, k)),
                    _ if came.answered.is_empty() && self.fetching.offline => compose::forward_check(&self.session, &judged, false).err(),
                    _ => None,
                };
                if let Some(e) = stop {
                    let said = self.kept(e);
                    self.say_refused(&command, &said);
                } else if !brought && !here {
                    match came.why {
                        Some(why) => self.status(why),
                        None => self.status(tl!("The server didn't send the messages. Try again in a few minutes.")),
                    }
                } else {
                    tried.extend(came.answered);
                    self.run_waited(command, params, opening, Some(tried));
                }
            } else {
                // Another command runs once its own message is here, whatever else of the batch
                // it came in didn't come.
                if waits.iter().all(|m| whole(self, m)) {
                    self.run_waited(command, params, opening, None);
                } else {
                    self.status(tl!("This message isn't on this computer yet. It will download when you're online."));
                }
            }
        }
    }

    fn poll_fetch_background(&mut self) {
        let Some((account, opening, rx)) = &self.fetching.background else { return };
        let (account, opening) = (*account, *opening);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => GraphFetchResult { account, offline: true, ..Default::default() },
        };
        self.fetching.background = None;
        // Asked for in a mailbox that another has taken the place of: it is nobody's here.
        if opening != self.session.openings() {
            return;
        }
        self.fetching.offline = result.offline;
        // (Stopped by the owner, it is no server's doing: nothing to wait out.)
        let stuck = !self.session.sync_progress.full_paused
            && (result.offline || result.busy_for.is_some() || (result.fetched.is_empty() && !result.errors.is_empty()));
        if stuck {
            // Not now: this account is tried again in a while, longer each time it stops again
            // (a Send/Receive renews the token too, and starts the waits afresh); the others go on.
            let times = self.fetching.stuck.get(&account).map_or(0, |(times, _)| *times);
            let wait = backoff(times).max(f64::from(result.busy_for.unwrap_or(0)) * 1_000.0);
            self.fetching.stuck.insert(account, (times.saturating_add(1), now_ms() + wait));
            // Said once, when it first stops, not every time it tries again.
            if times == 0 {
                let why = result.errors.first().map_or("no answer", String::as_str);
                log::warn!(
                    "graph fetch, account {account}: the background download stopped ({why}); it tries again in {} s",
                    (wait / 1_000.0) as u64
                );
            }
        } else if !result.fetched.is_empty() || !result.missing.is_empty() || !result.failed.is_empty() {
            self.fetching.stuck.remove(&account);
        }
        match take_in(self, result) {
            Ok(said) => {
                let (waiting, large) = (said["waiting"].as_u64().unwrap_or(0), said["large"].as_u64().unwrap_or(0));
                let all = said["all"].as_u64().unwrap_or(0);
                self.fetching.progress = match (waiting.saturating_sub(large), large) {
                    (0, 0) => String::new(),
                    (0, large) => crate::i18n::fmt(tl!("{large} large messages download when opened"), &[("large", &large.to_string())]),
                    (_, 0) => crate::i18n::fmt(
                        tl!("Downloading {done} of {all}"),
                        &[("done", &all.saturating_sub(waiting).to_string()), ("all", &all.to_string())],
                    ),
                    (_, large) => crate::i18n::fmt(
                        tl!("Downloading {done} of {all} · {large} large messages download when opened"),
                        &[("done", &all.saturating_sub(waiting).to_string()), ("all", &all.to_string()), ("large", &large.to_string())],
                    ),
                };
            }
            Err(e) => self.status(e.to_string()),
        }
    }

    /// Cancel: the background stops after the message in hand (at once where the host calls the
    /// exchange under way off, as the browser's does), until the next Send/Receive.
    pub(crate) fn pause_fetch(&mut self) {
        self.session.sync_progress.full_paused = true;
        self.fetching.cancel.store(true, Ordering::Relaxed);
        // (Send/Receive Progress says how far they had got.)
        self.session.sync_progress.count_all_full(&self.session.mailbox, self.session.now());
    }

    /// A Send/Receive started: the background may go on.
    pub(crate) fn resume_fetch(&mut self) {
        self.session.sync_progress.full_paused = false;
        self.fetching.idle_until = 0.0;
        self.fetching.stuck.clear();
    }
}

/// What a fetch of messages for the owner brought of each of them: those it answered for (whole,
/// gone or refused), those it sent whole, the first of them, in the order asked, that it didn't
/// send (`true`: gone from the server), and the server's own word on why it stopped, if it said
/// one. One it didn't answer
/// for (the server busy, or a host that stops at a batch's size) is asked for again; one no longer
/// here (deleted meanwhile) is nothing to wait for, and the engine says so when the command runs.
struct Came {
    answered: Vec<Id>,
    sent: Vec<Id>,
    not_sent: Option<(Id, bool)>,
    why: Option<String>,
}

impl Came {
    fn of(app: &MailApp, messages: &[Id], result: &GraphFetchResult) -> Came {
        let fetched: HashSet<&str> = result.fetched.iter().map(|f| f.id.as_str()).collect();
        let missing: HashSet<&str> = result.missing.iter().map(String::as_str).collect();
        let failed: HashSet<&str> = result.failed.iter().map(String::as_str).collect();
        let mut came = Came { answered: Vec::new(), sent: Vec::new(), not_sent: None, why: result.errors.first().cloned() };
        for m in messages {
            let Some(on_server) = app.session.mailbox.message(*m).and_then(|m| m.server.as_ref()).map(|s| s.id.as_str()) else {
                came.answered.push(*m);
                continue;
            };
            if missing.contains(on_server) || failed.contains(on_server) {
                came.not_sent = came.not_sent.or(Some((*m, missing.contains(on_server))));
            } else if fetched.contains(on_server) {
                came.sent.push(*m);
            } else {
                continue;
            }
            came.answered.push(*m);
        }
        came
    }
}

/// The engine's request for `messages` alone (the first one's, with the others' ids in it too), or
/// with none, for the background's next. `None` when there is nothing in it to fetch.
fn asked<R>(messages: &[Id], request: impl Fn(Option<Id>) -> Option<R>, ids: fn(&mut R) -> &mut Vec<String>) -> Option<R> {
    let mut all = request(messages.first().copied())?;
    for m in messages.iter().skip(1) {
        let mut more = request(Some(*m))?;
        ids(&mut all).append(ids(&mut more));
    }
    if ids(&mut all).is_empty() { None } else { Some(all) }
}

/// How long the background waits after `stuck` batches one after another brought nothing:
/// [`IDLE_MS`], doubling each time, at most [`MOST_IDLE_MS`].
fn backoff(stuck: u32) -> f64 {
    (IDLE_MS * 2f64.powi(i32::try_from(stuck.min(10)).unwrap_or(10))).min(MOST_IDLE_MS)
}

#[cfg(test)]
mod forward_several_tests;

#[cfg(test)]
mod let_go_tests;

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use mailcraft_engine::Session;
    use mailcraft_model::sync::{GraphFetchResult, ServerMessage, Tokens};
    use mailcraft_model::{FolderKind, Message};

    use super::*;
    use crate::Services;

    type Jobs = Arc<Mutex<Vec<(GraphFetchJob, mpsc::Sender<GraphFetchResult>)>>>;

    /// A window with two Microsoft accounts, each with `n` messages to fetch, and a host that
    /// keeps every job for the test to answer.
    fn window(n: i64) -> (MailApp, [Id; 2], Jobs) {
        let mut app = MailApp::new(Session::empty(), Services::default());
        app.autosave = false;
        let mut accounts = [0; 2];
        for (k, address) in ["one@example.org", "two@example.org"].iter().enumerate() {
            let account = app.execute("account.add", json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
            let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
            for m in 0..n {
                let server = ServerMessage { id: format!("AA{k}M{m}"), folder: "inbox".into(), ..Default::default() };
                let message =
                    Message { account, folder: inbox, received: 1_800_000_000 - m, unfetched: true, server: Some(server), ..Default::default() };
                app.session.mailbox.add_message(message).unwrap();
            }
            let tokens = Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
            app.seed_graph_token(account, tokens);
            accounts[k] = account;
        }
        let jobs: Jobs = Default::default();
        let kept = jobs.clone();
        app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
            let (tx, rx) = mpsc::channel();
            kept.lock().unwrap().push((job, tx));
            rx
        }));
        (app, accounts, jobs)
    }

    /// The first three messages of every job refused, as a server that won't convert them says.
    fn refuse_three(jobs: &Jobs) -> Vec<Id> {
        let pending: Vec<_> = std::mem::take(&mut *jobs.lock().unwrap());
        let mut whose = Vec::new();
        for (job, tx) in pending {
            whose.push(job.request.account);
            let failed = job.request.ids.iter().take(3).cloned().collect();
            let errors = vec!["Couldn't download the messages from Microsoft: Microsoft Graph answered with status 400 (ErrorInvalidRequest)".into()];
            tx.send(GraphFetchResult { account: job.request.account, email: job.request.email.clone(), failed, errors, ..Default::default() })
                .unwrap();
        }
        whose
    }

    #[test]
    fn an_account_whose_messages_are_refused_waits_longer_each_time_and_the_others_go_on() {
        let (mut app, [one, two], jobs) = window(20);
        app.drive_fetch();
        assert_eq!(refuse_three(&jobs), [one]);
        // Taken in: the next frame asks for the other account, not the same one again.
        app.drive_fetch();
        app.drive_fetch();
        assert_eq!(refuse_three(&jobs), [two]);
        // Both wait now.
        for _ in 0..3 {
            app.drive_fetch();
        }
        assert!(jobs.lock().unwrap().is_empty());
        let (times, until) = app.fetching.stuck[&one];
        assert_eq!(times, 1);
        assert!(until >= now_ms() + IDLE_MS * 0.9, "the first wait is the usual one");
        // Its wait over, the account is asked again: without the messages refused, and when it
        // stops again it waits twice as long.
        app.fetching.stuck.get_mut(&one).unwrap().1 = 0.0;
        app.fetching.idle_until = 0.0;
        app.drive_fetch();
        let asked = jobs.lock().unwrap().first().map(|(job, _)| (job.request.account, job.request.ids.clone())).unwrap();
        assert_eq!(asked.0, one);
        assert!(asked.1.iter().all(|id| !["AA0M0", "AA0M1", "AA0M2"].contains(&id.as_str())), "{:?}", asked.1);
        refuse_three(&jobs);
        app.drive_fetch();
        let (times, until) = app.fetching.stuck[&one];
        assert_eq!(times, 2);
        assert!(until >= now_ms() + IDLE_MS * 1.9);
        // A Send/Receive starts the waits afresh.
        app.resume_fetch();
        assert!(app.fetching.stuck.is_empty());
    }
}
