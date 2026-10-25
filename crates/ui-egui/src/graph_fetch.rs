// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of a Microsoft account fetched whole (R3.2; R3 spec S12, S14, S15): in the
//! background, newest first, within the account's "Download messages in full"; at once, ahead of
//! that, for the message the owner opens; and before Reply, Reply All, Forward, Save Message As
//! and Save Attachment, which then go on. Only while a Graph token from this session is good:
//! each Send/Receive renews it. Cancel (Stop) stops it all until the next Send/Receive starts,
//! the owner's or one by itself; Send/Receive Progress says how far it has got, account by account
//! (the engine counts them as they come in, [`mailcraft_engine::cmd::sync_progress::FullMessages`]).
//!
//! A JMAP account's messages come the same way (M6), at once and before a reply alike, through
//! [`crate::Services::jmap_fetch`] with the secret its Send/Receive went with
//! ([`crate::jmap_mail`]), and are taken in by the engine's JMAP side, which holds them to sizes
//! first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::{graph_mail, jmap_mail};
use mailcraft_model::sync::{GraphFetchRequest, GraphFetchResult};
use mailcraft_model::{AccountKind, Id};
use serde_json::{Value, json};

use crate::jmap_mail::JmapFetchJob;
use crate::{MailApp, now_ms};

/// Messages asked for at a time in the background.
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
    /// The owner's message under way: which one, under which opening, what calls it off, and the
    /// answer to come.
    now: Option<(Id, u64, Arc<AtomicBool>, Receiver<GraphFetchResult>)>,
    /// What waits for that message: a command, its params, and the mailbox opening it was asked
    /// under ([`mailcraft_engine::Session::openings`]).
    then: Option<(String, Value, u64)>,
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
    /// Whether anything is being fetched.
    pub fn busy(&self) -> bool {
        self.background.is_some() || self.now.is_some()
    }

    /// Something new may be waiting: the background looks again at once.
    pub(crate) fn wake(&mut self) {
        self.idle_until = 0.0;
    }

    /// Whether `message` is being fetched for the owner now.
    pub fn fetching(&self, message: Id) -> bool {
        self.now.as_ref().is_some_and(|(m, ..)| *m == message)
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

/// The commands that need a message whole, and the message they are about.
fn needs_whole(app: &MailApp, id: &str, params: &Value) -> Option<Id> {
    if !matches!(
        id,
        "mail.reply"
            | "mail.replyAll"
            | "mail.forward"
            | "mail.forwardAsAttachment"
            | "mail.editDraft"
            | "file.exportMessage"
            | "mail.saveAttachment"
            | "mail.openAttachment"
    ) {
        return None;
    }
    let selected = match app.session.sel.messages.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    let message = params.get("id").and_then(Value::as_u64).or(selected)?;
    // One whose server's copy went before it could be fetched never will be: Forward as Attachment
    // says so itself (the others have no word of their own for it, and wait as they did).
    let never = |m: &mailcraft_model::Message| m.incomplete && id == "mail.forwardAsAttachment";
    app.session.mailbox.message(message).filter(|m| m.unfetched && !never(m) && m.server.is_some()).map(|m| m.id)
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
}

fn way(app: &MailApp, account: Id) -> Option<Way> {
    match app.session.mailbox.account(account)?.kind {
        AccountKind::Microsoft if app.services.graph_fetch.is_some() => access(app, account).map(Way::Graph),
        // (Not with a credential its server turned down: the next Send/Receive the owner asks for first.)
        AccountKind::Jmap if app.services.jmap_fetch.is_some() && !app.jmap_refused.contains(&account) => app.jmap_secret(account).map(Way::Jmap),
        _ => None,
    }
}

/// Whether the host fetches messages of accounts of this kind.
fn fetched(app: &MailApp, kind: AccountKind) -> bool {
    match kind {
        AccountKind::Microsoft => app.services.graph_fetch.is_some(),
        AccountKind::Jmap => app.services.jmap_fetch.is_some(),
        _ => false,
    }
}

/// Take in messages fetched whole, as their account's kind is taken in (a JMAP account's are held
/// to sizes first).
fn take_in(app: &mut MailApp, result: GraphFetchResult) -> Result<Value, mailcraft_engine::CmdError> {
    if app.session.mailbox.account(result.account).is_some_and(|a| a.kind == AccountKind::Jmap) {
        jmap_mail::apply_jmap_fetch(&mut app.session, result)
    } else {
        graph_mail::apply_graph_fetch(&mut app.session, result)
    }
}

impl MailApp {
    /// A command that needs its message whole waits for it to be fetched, and then runs.
    pub(crate) fn fetch_first(&mut self, id: &str, params: &Value) -> Option<Result<Value, String>> {
        let message = needs_whole(self, id, params)?;
        let account = self.session.mailbox.message(message)?.account;
        if !self.fetch_now(account, message) {
            if self.jmap_refused.contains(&account) {
                return Some(Err(tl!("The server turned down the sign-in for this account. Use Send/Receive to sign in again.").to_string()));
            }
            return Some(Err(tl!("This message isn't on this computer yet. It will download when you're online.").to_string()));
        }
        // Named, for the selection may have moved on by the time it is here. An `id` the call
        // gave stays as it was, for the command to judge.
        let mut then = params.as_object().cloned().unwrap_or_default();
        if then.get("id").is_none_or(Value::is_null) {
            then.insert("id".into(), json!(message));
        }
        self.fetching.then = Some((id.to_string(), Value::Object(then), self.session.openings()));
        self.status(tl!("Downloading the message first…"));
        Some(Ok(json!({"pending": "download", "message": message})))
    }

    /// Fetch `message` at once, ahead of the background. Whether it could be asked for.
    fn fetch_now(&mut self, account: Id, message: Id) -> bool {
        if self.fetching.fetching(message) {
            return true;
        }
        let Some(way) = way(self, account) else {
            self.fetching.offline = true;
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(answer) = self.hand_over(account, way, Some(message), 1, cancel.clone()) else { return false };
        self.fetching.now = Some((message, self.session.openings(), cancel, answer));
        true
    }

    /// Hand the host a fetch of `account`'s messages: `message` alone, or the next `most` in the
    /// background. `None` when there is nothing to fetch.
    fn hand_over(&self, account: Id, way: Way, message: Option<Id>, most: usize, cancel: Arc<AtomicBool>) -> Option<Receiver<GraphFetchResult>> {
        match way {
            Way::Graph(access) => {
                let fetch = self.services.graph_fetch.as_ref()?;
                let request = graph_mail::graph_fetch_request(&self.session, account, message, most).ok().filter(|r| !r.ids.is_empty())?;
                Some(fetch(GraphFetchJob { request, access, cancel }))
            }
            Way::Jmap(secret) => {
                let fetch = self.services.jmap_fetch.as_ref()?;
                let request = jmap_mail::jmap_fetch_request(&self.session, account, message, most).ok().filter(|r| !r.ids.is_empty())?;
                Some(fetch(JmapFetchJob { request, secret, cancel }))
            }
        }
    }

    /// Each frame: take in what came, fetch what the owner opened, and keep the background going.
    /// (Public for a host's tests, which have no window.)
    pub fn drive_fetch(&mut self) {
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
            self.fetching.opened = shown;
            if let Some(m) =
                shown.and_then(|m| self.session.mailbox.message(m)).filter(|m| m.unfetched && m.server.is_some()).map(|m| (m.account, m.id))
                && self.fetching.now.is_none()
            {
                self.fetch_now(m.0, m.1);
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
            let Some(answer) = self.hand_over(account, way, None, BATCH, cancel.clone()) else { continue };
            self.fetching.cancel = cancel;
            self.fetching.background = Some((account, self.session.openings(), answer));
            return;
        }
        self.fetching.idle_until = now_ms() + IDLE_MS;
        self.fetching.progress.clear();
    }

    fn poll_fetch_now(&mut self) {
        let Some((message, opening, _, rx)) = &self.fetching.now else { return };
        let (message, opening) = (*message, *opening);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => GraphFetchResult { offline: true, ..Default::default() },
        };
        self.fetching.now = None;
        // Asked for in a mailbox that another has taken the place of: it is nobody's here (nor is
        // a command that waits for it, below).
        if opening != self.session.openings() {
            return;
        }
        self.fetching.offline = result.offline;
        let account = result.account;
        if account != 0
            && let Err(e) = take_in(self, result)
        {
            self.status(e.to_string());
        }
        let whole = self.session.mailbox.message(message).is_some_and(|m| !m.unfetched);
        if let Some((command, params, opening)) = self.fetching.then.take() {
            // Asked of a mailbox that another has taken the place of: the message it names is not
            // this one's, whatever has that id here.
            if opening != self.session.openings() {
                return;
            }
            if whole {
                let _ = self.run(&command, params);
            } else {
                self.status(tl!("This message isn't on this computer yet. It will download when you're online."));
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

/// How long the background waits after `stuck` batches one after another brought nothing:
/// [`IDLE_MS`], doubling each time, at most [`MOST_IDLE_MS`].
fn backoff(stuck: u32) -> f64 {
    (IDLE_MS * 2f64.powi(i32::try_from(stuck.min(10)).unwrap_or(10))).min(MOST_IDLE_MS)
}

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
