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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::{ews_mail, graph_mail, jmap_mail};
use mailcraft_engine::sync_driver::fetch::apply_fetch as take_in_fetch;
use mailcraft_engine::sync_driver::fetch_flow::{FirstNote, NowNext, NowNote, Waiting};
use mailcraft_engine::sync_driver::{FETCH_BATCH, FetchProgress};
use mailcraft_model::sync::GraphFetchResult;
use mailcraft_model::{AccountKind, Id};
use serde_json::{Value, json};

use crate::ews_mail::EwsFetchJob;
use crate::jmap_mail::JmapFetchJob;
use crate::{MailApp, now_ms};

/// Messages asked for at a time: in the background, and for a forward of several (#360).
const BATCH: usize = FETCH_BATCH;
// How long the background waits before looking again when there was nothing to fetch, and how
// long at most after batches that brought nothing, one after another, are the engine's
// ([`mailcraft_engine::sync_driver::fetch`]), as are the decisions about which messages a command
// needs and which are still to fetch.
#[cfg(test)]
use mailcraft_engine::sync_driver::FETCH_IDLE_MS as IDLE_MS;

/// What the host is asked to fetch ([`crate::Services::graph_fetch`]).
pub use mailcraft_host::fetch::GraphFetchJob;

/// The state of fetching, in the window.
#[derive(Default)]
pub struct Fetching {
    /// The background's batch under way: for which account, under which opening of the mailbox
    /// ([`mailcraft_engine::Session::openings`]), and the answer to come.
    background: Option<(Id, u64, Receiver<GraphFetchResult>)>,
    /// The owner's messages under way: which ones (one, or of a forward of several, #360), under
    /// which opening, what calls it off, and the answer to come.
    now: Option<(Vec<Id>, u64, Arc<AtomicBool>, Receiver<GraphFetchResult>)>,
    pub(crate) cancel: Arc<AtomicBool>,
    /// For the status bar: how many are fetched of how many, while some aren't.
    pub(crate) progress: String,
}

impl Fetching {
    /// Whether anything is being fetched, or a command whose messages came waits to go on.
    pub fn busy(&self) -> bool {
        self.background.is_some() || self.now.is_some()
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
        self.background = None;
        self.progress.clear();
    }
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
        AccountKind::Exchange if app.services.ews_fetch.is_some() && !app.jmap_refused.contains(&account) => app.ews_secret(account).map(Way::Ews),
        _ => None,
    }
}

/// Whether the host fetches messages of accounts of this kind.
fn fetched(app: &MailApp, kind: AccountKind) -> bool {
    match kind {
        AccountKind::Microsoft => app.services.graph_fetch.is_some(),
        AccountKind::Jmap => app.services.jmap_fetch.is_some(),
        AccountKind::Exchange => app.services.ews_fetch.is_some(),
        _ => false,
    }
}

impl MailApp {
    /// A command that needs its message whole waits for it to be fetched, and then runs. What is
    /// fetched, and for what the command waits, is the engine's to say
    /// ([`mailcraft_engine::Session::fetch_first_plan`]); this hands the host the fetch.
    pub(crate) fn fetch_first(&mut self, id: &str, params: &Value) -> Option<Result<Value, String>> {
        let plan = match self.session.fetch_first_plan(id, params) {
            Ok(plan) => plan?,
            Err(e) => return Some(Err(self.kept(e))),
        };
        if !self.fetch_now(plan.account, &plan.messages) {
            if self.jmap_refused.contains(&plan.account) {
                return Some(Err(tl!("The server turned down the sign-in for this account. Use Send/Receive to sign in again.").to_string()));
            }
            // Of several, the one that can't come is named, as the engine names it.
            if let Some(e) = self.session.fetch_unavailable(&plan) {
                return Some(Err(self.kept(e)));
            }
            return Some(Err(tl!("This message isn't on this computer yet. It will download when you're online.").to_string()));
        }
        let first = plan.first;
        let note = self.session.fetch_waiting(plan);
        self.status(match note {
            FirstNote::ForwardCalledOff => {
                tl!("The forward of several messages that was waiting was called off: another download took its place.")
            }
            FirstNote::HeldCalledOff => tl!("What was waiting for the form to be closed was called off: another download took its place."),
            FirstNote::Several => tl!("Downloading the messages first…"),
            FirstNote::One => tl!("Downloading the message first…"),
        });
        Some(Ok(json!({"pending": "download", "message": first})))
    }

    /// Run the command that waited, whose messages came: at once, or, while a form with changes
    /// that aren't saved is open, once it is closed ([`MailApp::run_held`]): what it asks (Unsaved
    /// Message, a confirmation) would take the form's place, and what was typed in it with it
    /// (#513).
    fn run_waited(&mut self, waiting: Waiting) {
        if matches!(&self.dialog, Some(crate::dialogs::Dialog::Form(form)) if form.changed()) {
            self.session.fetch_hold(waiting);
            return;
        }
        let Waiting { command, params, tried, .. } = waiting;
        self.session.fetch_running(tried);
        let _ = self.run(&command, params);
        self.session.fetch_ran();
    }

    /// The command that waited for a form to be closed goes on, in the mailbox it was asked of.
    fn run_held(&mut self) {
        if let Some(waiting) = self.session.fetch_take_held() {
            self.run_waited(waiting);
        }
    }

    /// Whether `message`, the one the reading pane shows, is fetched once the batch under way has
    /// come: one at a time, and a forward's can take a while.
    pub(crate) fn opened_after_batch(&self, message: Id) -> bool {
        self.session.fetch_after_batch(message, self.fetching.now.is_some())
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
            self.session.driver.fetch.offline = true;
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(answer) = self.hand_over(account, way, messages, 1, cancel.clone()) else { return false };
        // What these take the place of is called off: nobody waits for it now. The message open
        // in the reading pane, if it was of it, is asked for again once these are here.
        if let Some((before, _, called_off, _)) = self.fetching.now.replace((messages.to_vec(), self.session.openings(), cancel, answer)) {
            called_off.store(true, Ordering::Relaxed);
            self.session.fetch_replaced(&before, messages);
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
        let host_fetches = |kind| self.fetched_kind(kind);
        if self.session.fetch_accounts(host_fetches).is_empty() {
            return;
        }
        // The message the reading pane shows, at once.
        if let Some((account, m)) = self.session.fetch_shown(self.fetching.now.is_some()) {
            self.fetch_now(account, &[m]);
        }
        // The background, a few at a time.
        let now = now_ms();
        if self.fetching.background.is_some() || !self.session.fetch_background_due(now) {
            return;
        }
        for account in self.session.fetch_background_accounts(|kind| self.fetched_kind(kind), now) {
            let Some(way) = way(self, account) else { continue };
            let cancel = Arc::new(AtomicBool::new(false));
            let Some(answer) = self.hand_over(account, way, &[], BATCH, cancel.clone()) else { continue };
            self.fetching.cancel = cancel;
            self.fetching.background = Some((account, self.session.openings(), answer));
            return;
        }
        self.session.driver.fetch.found_nothing(now_ms());
        self.fetching.progress.clear();
    }

    /// Whether the host fetches messages of accounts of this kind.
    fn fetched_kind(&self, kind: AccountKind) -> bool {
        fetched(self, kind)
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
        // What came is taken in, and what the command that waits does next is the engine's to say.
        let answered = self.session.fetch_now_answered(&messages, opening, result);
        if let Some(said) = answered.refused {
            self.status(said);
        }
        match answered.next {
            NowNext::Nothing => {}
            NowNext::Run(waiting) => self.run_waited(waiting),
            NowNext::Stopped { command, error } => {
                let said = self.kept(error);
                self.say_refused(&command, &said);
            }
            NowNext::Note(NowNote::Why(why)) => self.status(why),
            NowNext::Note(NowNote::NotSent) => self.status(tl!("The server didn't send the messages. Try again in a few minutes.")),
            NowNext::Note(NowNote::NotHere) => self.status(tl!("This message isn't on this computer yet. It will download when you're online.")),
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
        // (Stopped by the owner, it is no server's doing: nothing to wait out.)
        let paused = self.session.sync_progress.full_paused;
        if let Some(stopped) = self.session.driver.fetch.background_came(account, paused, &result, now_ms()) {
            // Said once, when it first stops, not every time it tries again.
            if stopped.first {
                let why = result.errors.first().map_or("no answer", String::as_str);
                log::warn!(
                    "graph fetch, account {account}: the background download stopped ({why}); it tries again in {} s",
                    (stopped.wait_ms / 1_000.0) as u64
                );
            }
        }
        match take_in_fetch(&mut self.session, result) {
            Ok(said) => {
                let progress = FetchProgress::of(&said);
                let (done, all, large) = (progress.done, progress.all, progress.large);
                self.fetching.progress = match (progress.background(), large) {
                    (0, 0) => String::new(),
                    (0, large) => crate::i18n::fmt(tl!("{large} large messages download when opened"), &[("large", &large.to_string())]),
                    (_, 0) => crate::i18n::fmt(tl!("Downloading {done} of {all}"), &[("done", &done.to_string()), ("all", &all.to_string())]),
                    (_, large) => crate::i18n::fmt(
                        tl!("Downloading {done} of {all} · {large} large messages download when opened"),
                        &[("done", &done.to_string()), ("all", &all.to_string()), ("large", &large.to_string())],
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
        self.session.driver.fetch.resume();
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
        let (times, until) = app.session.driver.fetch.stuck[&one];
        assert_eq!(times, 1);
        assert!(until >= now_ms() + IDLE_MS * 0.9, "the first wait is the usual one");
        // Its wait over, the account is asked again: without the messages refused, and when it
        // stops again it waits twice as long.
        app.session.driver.fetch.stuck.get_mut(&one).unwrap().1 = 0.0;
        app.session.driver.fetch.idle_until = 0.0;
        app.drive_fetch();
        let asked = jobs.lock().unwrap().first().map(|(job, _)| (job.request.account, job.request.ids.clone())).unwrap();
        assert_eq!(asked.0, one);
        assert!(asked.1.iter().all(|id| !["AA0M0", "AA0M1", "AA0M2"].contains(&id.as_str())), "{:?}", asked.1);
        refuse_three(&jobs);
        app.drive_fetch();
        let (times, until) = app.session.driver.fetch.stuck[&one];
        assert_eq!(times, 2);
        assert!(until >= now_ms() + IDLE_MS * 1.9);
        // A Send/Receive starts the waits afresh.
        app.resume_fetch();
        assert!(app.session.driver.fetch.stuck.is_empty());
    }
}
