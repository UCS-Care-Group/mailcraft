// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive for a host with no window: ask the engine what is due, run it, take the answer
//! in. Two calls, which a binding wraps:
//!
//! * [`SendReceive::what_to_run`] looks at the round ([`mailcraft_engine::sync_driver::RoundDriver`]),
//!   starts the next account's run on a thread of the host's and says what it did ([`Step`]);
//! * [`SendReceive::take_result`] takes in what the run brought, as far as it has come
//!   ([`Taken`]), and ends the run when its answer is in.
//!
//! [`SendReceive::run_due`] is the two together, in the order a front end calls them every
//! second or so. Only accounts kept in step through Microsoft Graph are run here for now
//! ([`Step::Unsupported`] says so for the others, which a front end runs its own way).
//! Secrets come from the platform's credential store and from memory, are never logged and never
//! appear in an error's text. Nothing of a message is logged.
//!
//! What the window refuses, this refuses: nothing starts while Work Offline is on, nor while the
//! mailbox is unconfirmed (a file's accounts nobody has confirmed) or its data removed
//! ([`Context::unconfirmed`], [`Context::data_removed`]); then no secret is read or kept either.
//!
//! The credential-store contract for a binding: [`SecretStore::get`] must not block on a prompt
//! (hand in a store that answers from what it has), a refresh token is kept under
//! [`secret_key`]`("refresh", account)` and nowhere else, and every key this module writes is
//! reported once by [`SendReceive::take_written_keys`], for the binding to note in its own index
//! so that removing all data finds it. A first sign-in is the binding's (open
//! [`Platform::open_sign_in_page`], exchange the code) and is handed in with
//! [`SendReceive::adopt_tokens`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::{ClientIds, MailRefusal, MailStart, Next, Waiting, plan_graph_mail};
use mailcraft_model::sync::{GraphMailResult, SyncPhase, Tokens};
use mailcraft_model::{AccountKind, Id, OAuth};

use crate::graph::{GraphRequest, GraphResult};
use crate::secrets::{SecretStore, secret_key};
use crate::{Host, Platform};

/// What a call is told of its surroundings.
pub struct Context<'a> {
    /// The platform's credential store, when it has one.
    pub secrets: Option<&'a dyn SecretStore>,
    /// Whether sign-ins may be read from and kept in it (File › Options › remember).
    pub remember: bool,
    /// The application ids the build and the owner chose.
    pub ids: ClientIds<'a>,
    /// The clock, in milliseconds (any epoch the caller keeps to), for the round's timers.
    pub now_ms: f64,
    /// Unix seconds, for the tokens' expiry.
    pub now_s: i64,
    /// The owner's minutes between rounds (`syncMinutes`; 0 is never).
    pub minutes: f64,
    /// Writes the mailbox once the messages about to be sent are marked as sent (AU3-26): the
    /// run goes only when this says yes. `None`: nothing outlives the process, nothing to write.
    pub persist: Option<&'a dyn Fn(&Session, u64) -> Result<(), String>>,
    /// The mailbox came from a file whose accounts nobody has confirmed (C1): nothing reaches a
    /// server, no remembered secret is read, and a manual round is refused.
    pub unconfirmed: bool,
    /// The owner removed all MailCraft's data: no secret is kept again.
    pub data_removed: bool,
    /// Whether another job of the front end (the calendar's) is at work for an account: its run
    /// waits in line until that has ended. `None`: no other job.
    pub job_busy: Option<&'a dyn Fn(Id) -> bool>,
}

impl Context<'_> {
    /// Whether nothing may reach a server or a secret store: the mailbox is unconfirmed or its
    /// data removed.
    fn held(&self) -> bool {
        self.unconfirmed || self.data_removed
    }
}

/// What the host remembers between calls, in memory only: the sign-ins of this session. Never
/// saved, never in `Debug` output.
pub struct SendReceive {
    /// A Graph access token of the session, with the account's newest refresh token.
    tokens: HashMap<Id, Tokens>,
    /// Set to stop the run under way at its next safe place.
    cancel: Arc<AtomicBool>,
    running: Option<Running>,
    /// The opening of the mailbox the state belongs to: another mailbox, another state.
    opening: Option<u64>,
    runner: Runner,
    /// Names of the credential-store entries written, not yet reported.
    written: Vec<String>,
}

impl Default for SendReceive {
    fn default() -> Self {
        SendReceive { tokens: HashMap::new(), cancel: Arc::default(), running: None, opening: None, runner: real_runner(), written: Vec::new() }
    }
}

impl std::fmt::Debug for SendReceive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendReceive").field("running", &self.running.as_ref().map(|r| r.account)).finish_non_exhaustive()
    }
}

/// A run under way.
struct Running {
    account: Id,
    quiet: bool,
    opening: u64,
    pieces: Receiver<GraphMailResult>,
    answer: Receiver<GraphResult>,
    /// What waited to go once its first piece was in: more than that was made here since (the
    /// run look calls the run off).
    baseline: Option<usize>,
    /// The run was called off so that what the owner did could go first.
    called_off: bool,
    /// The owner stopped the round: nothing calls the run off or puts it back after that.
    stopped: bool,
    /// Lines for the owner's log from the pieces taken so far.
    errors: Vec<String>,
}

/// What [`SendReceive::adopt_tokens`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adopted {
    /// Kept for the session, and in the credential store when the owner lets it be.
    Kept,
    /// They are somebody else's: not kept.
    WrongPerson,
    /// Not kept: the account is gone, or the mailbox is unconfirmed or its data removed.
    Refused,
}

/// Why an account was left out of the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Nobody asked for the run and there is no sign-in to run with.
    NoSignIn,
    /// The account is gone.
    Gone,
    /// The mailbox could not make the request.
    Refused,
    /// The messages about to go could not be marked on disk first: nothing was sent.
    NotSaved,
}

/// What [`SendReceive::what_to_run`] did.
#[derive(Debug)]
pub enum Step {
    /// Nothing is due.
    Idle,
    /// A run is under way, or the next account waits a moment: ask again.
    Waiting,
    /// `account`'s run was started on a thread of the host's.
    Started { account: Id },
    /// The owner has to sign in for `account` (a run somebody asked for has no sign-in): the
    /// binding opens the provider's page ([`Platform::open_sign_in_page`]).
    SignIn { account: Id, oauth: Box<OAuth>, address: String },
    /// The account was left out of the round.
    LeftOut { account: Id, why: Left },
    /// The account is kept in step another way, which the front end runs.
    Unsupported { account: Id, kind: AccountKind },
}

/// What [`SendReceive::take_result`] took in.
#[derive(Debug, PartialEq, Eq)]
pub enum Taken {
    /// No run is under way.
    Nothing,
    /// Part of the mail was taken in; the run goes on.
    Piece,
    /// The run is not over yet.
    Pending,
    /// The run is over. `errors` are lines for the owner's log (never a secret or a message).
    Done { account: Id, errors: Vec<String> },
    /// The run is over and the owner has to sign in again (`consent`: Microsoft wants the owner's
    /// say-so first).
    SignInAgain { account: Id, consent: bool },
    /// The tokens were somebody else's: the run is over and they were not kept.
    WrongPerson { account: Id },
}

impl SendReceive {
    /// A new, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// A state whose runs are done as `runner` says, which asks no server (the tests').
    pub fn with_runner(runner: impl Fn(GraphRequest) -> GraphResult + Send + Sync + 'static) -> Self {
        SendReceive { runner: Arc::new(runner), ..Self::default() }
    }

    /// Whether a run is under way.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Call the run under way off at its next safe place; the rest of the round with it.
    pub fn stop(&mut self, session: &mut Session) {
        self.cancel.store(true, Ordering::Relaxed);
        session.driver.round.clear_lines();
        // An explicit stop outranks a call-off made earlier: the run is not put back.
        if let Some(running) = self.running.as_mut() {
            running.stopped = true;
            running.called_off = false;
        }
    }

    /// Let go of the run under way: its flag is set, and the next run gets a flag of its own, so
    /// that nothing the dropped job does to its flag reaches the next one.
    fn drop_run(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.running = None;
    }

    /// Forget the session's sign-ins (a mailbox took the place of the open one, or the owner
    /// signed out). A run under way is called off and let go of.
    pub fn forget(&mut self) {
        self.tokens.clear();
        self.drop_run();
    }

    /// Another mailbox, another state: what was kept for the one before is let go of.
    fn follow_opening(&mut self, session: &Session) {
        let opening = session.openings();
        if self.opening != Some(opening) {
            self.forget();
            self.opening = Some(opening);
        }
    }

    /// Fill the list of written entries (the tests': the cap is reached by no real mailbox).
    #[cfg(test)]
    pub(crate) fn fill_written(&mut self, n: usize) {
        self.written = (0..n).map(|i| format!("other-{i}")).collect();
    }

    /// The names of the credential-store entries written since the last call, for the binding to
    /// note (see the module's contract). At most 1,000 are held.
    pub fn take_written_keys(&mut self) -> Vec<String> {
        std::mem::take(&mut self.written)
    }

    /// Take in the tokens of a sign-in the binding made: refused when they are somebody else's
    /// (the account is not signed in as another person), kept for the session otherwise, with the
    /// refresh token in the credential store when the owner lets it be.
    pub fn adopt_tokens(&mut self, session: &Session, ctx: &Context, account: Id, tokens: Tokens) -> Adopted {
        self.follow_opening(session);
        let Some(a) = session.mailbox.account(account) else { return Adopted::Refused };
        if ctx.held() {
            return Adopted::Refused;
        }
        if mailcraft_engine::sync_driver::signed_in_as_other(a, &tokens) {
            return Adopted::WrongPerson;
        }
        // The newest refresh token is the account's, for mail and Graph alike.
        if !tokens.refresh.is_empty() {
            self.remember(session, ctx, account, &tokens.refresh);
        }
        self.tokens.insert(account, mailcraft_engine::sync_driver::kept_for_session(tokens, ctx.now_s));
        Adopted::Kept
    }

    /// Start a round for `accounts` now, the owner having asked for it. Refused (false) while
    /// Work Offline is on or the mailbox is unconfirmed or its data removed.
    pub fn start_round(&mut self, session: &mut Session, ctx: &Context, accounts: &[Id]) -> bool {
        if session.prefs.offline || ctx.held() {
            return false;
        }
        session.driver.round.begin_round(accounts, false);
        session.sync_progress.begin(accounts);
        true
    }

    /// What is due, and start it: a new round when the owner's minutes have gone by, changes made
    /// here that wait to go, and then the next account in line, one at a time.
    pub fn what_to_run(&mut self, host: &Host, session: &mut Session, ctx: &Context) -> Step {
        self.follow_opening(session);
        if self.running.is_some() {
            return Step::Waiting;
        }
        // Working offline, or a mailbox nobody has confirmed: nothing starts by itself either.
        if session.prefs.offline || ctx.held() {
            return Step::Idle;
        }
        let auto = session.driver.auto_sync();
        if session.driver.round.queue.is_empty() {
            // Something made here waits to go: it doesn't wait for the next round.
            if session.driver.round.push_look_due(auto, ctx.now_ms) {
                let waiting: Vec<Waiting> = mailcraft_engine::sync_driver::server_accounts(session)
                    .into_iter()
                    .map(|account| Waiting { account, count: mailcraft_engine::sync_driver::waiting_to_go(session, account) })
                    .collect();
                let due: Vec<Id> =
                    session.driver.round.pushes_due(&waiting, ctx.now_ms).into_iter().filter(|a| self.can_start_quietly(session, ctx, *a)).collect();
                if let Some(round) = session.driver.round.queue_pushes(&due) {
                    session.sync_progress.begin(&round);
                    return Step::Waiting;
                }
            }
            if !session.driver.round.round_due(auto, ctx.minutes, ctx.now_ms) {
                return Step::Idle;
            }
            let round = mailcraft_engine::sync_driver::server_accounts(session);
            session.driver.round.begin_auto_round(&round, ctx.now_ms);
            if round.is_empty() {
                return Step::Idle;
            }
            session.sync_progress.begin(&round);
        }
        let front_busy = match (session.driver.round.queue.front(), ctx.job_busy) {
            (Some((account, _)), Some(busy)) => busy(*account),
            _ => false,
        };
        match session.driver.round.next(|_| front_busy) {
            Next::Idle => Step::Idle,
            Next::Wait => Step::Waiting,
            Next::Start { account, quiet } => self.start(host, session, ctx, account, quiet),
        }
    }

    /// Whether `account`'s run can go ahead with nobody asked anything: its sign-in is at hand.
    fn can_start_quietly(&self, session: &Session, ctx: &Context, account: Id) -> bool {
        let Some(a) = session.mailbox.account(account) else { return false };
        a.kind == AccountKind::Microsoft && !a.is_shared() && self.refresh_for(session, ctx, account).is_some()
    }

    /// The account's refresh token: this session's, else the credential store's.
    pub(crate) fn refresh_for(&self, session: &Session, ctx: &Context, account: Id) -> Option<String> {
        if ctx.held() {
            return None;
        }
        let kept = self.tokens.get(&account).map(|t| t.refresh.as_str());
        let stored = || {
            let (store, a) = (ctx.secrets?, session.mailbox.account(account)?);
            if ctx.remember { store.get(&secret_key("refresh", a)) } else { None }
        };
        let found = mailcraft_engine::sync_driver::refresh_token(kept, None);
        if found.is_empty() { stored().filter(|r| !r.is_empty()) } else { Some(found) }
    }

    fn start(&mut self, host: &Host, session: &mut Session, ctx: &Context, account: Id, quiet: bool) -> Step {
        let Some(a) = session.mailbox.account(account) else {
            session.sync_progress.leave_out(account);
            return Step::LeftOut { account, why: Left::Gone };
        };
        // (A shared mailbox is read with its owner's sign-in, and is the window's for now.)
        // The front end runs it (and says so in Send/Receive Progress, or leaves it out there).
        if a.kind != AccountKind::Microsoft || a.is_shared() {
            return Step::Unsupported { account, kind: a.kind };
        }
        // A message an earlier run handed over, whose answer never came back, may have gone: it
        // is held for the owner, never sent again by itself (AU3-26).
        let _ = session.send_hold_unanswered(account);
        let refresh = self.refresh_for(session, ctx, account).unwrap_or_default();
        let plan = match plan_graph_mail(session, account, quiet, refresh) {
            Ok(MailStart::Run(plan)) => *plan,
            Ok(MailStart::Quiet { .. }) => {
                session.sync_progress.leave_out(account);
                return Step::LeftOut { account, why: Left::NoSignIn };
            }
            Ok(MailStart::SignIn { oauth, address }) => {
                session.sync_progress.leave_out(account);
                return Step::SignIn { account, oauth: Box::new(mailcraft_engine::sync_driver::effective_oauth(oauth, &ctx.ids)), address };
            }
            Err(MailRefusal::AccountGone) => {
                session.sync_progress.leave_out(account);
                return Step::LeftOut { account, why: Left::Gone };
            }
            Err(MailRefusal::NoSignIn | MailRefusal::NotGraph(_)) => {
                session.sync_progress.leave_out(account);
                return Step::LeftOut { account, why: Left::Refused };
            }
        };
        // What it sends is marked so, on disk, before it leaves (AU3-26).
        let going: Vec<Id> = plan.mail.sends.iter().map(|up| up.message).collect();
        if let Some(rev) = session.send_marking(account, &going)
            && let Some(persist) = ctx.persist
            && let Err(e) = persist(session, rev)
        {
            log::warn!("send/receive: the mailbox couldn't be saved before sending; nothing is sent: {e}");
            session.sending_answered(account);
            session.sync_progress.leave_out(account);
            return Step::LeftOut { account, why: Left::NotSaved };
        }
        let (pieces_tx, pieces) = sync_channel(8);
        // A flag of this run's own: an earlier job that is still winding down can't call it off.
        self.cancel = Arc::new(AtomicBool::new(false));
        session.driver.fetch.resume();
        let request = GraphRequest {
            account: plan.owner,
            oauth: mailcraft_engine::sync_driver::effective_oauth(plan.oauth, &ctx.ids),
            refresh: plan.refresh,
            // (Renewed for the job: a token kept from another job may not cover the mail.)
            access: String::new(),
            outbox: Vec::new(),
            directory: false,
            calendar: None,
            changes: None,
            asks: Vec::new(),
            settings: false,
            favorites: false,
            favorites_scopes: false,
            mail: Some(plan.mail),
            mail_pieces: Some(pieces_tx),
            cancel: self.cancel.clone(),
        };
        session.sync_progress.phase(account, SyncPhase::Sending);
        let runner = self.runner.clone();
        let answer = host.run_job("send-receive", move |_platform: &dyn Platform| runner(request), || {});
        self.running = Some(Running {
            account,
            quiet,
            opening: session.openings(),
            pieces,
            answer,
            baseline: None,
            called_off: false,
            stopped: false,
            errors: Vec::new(),
        });
        Step::Started { account }
    }

    /// Take in what the run under way has brought so far: a few pieces of mail, and its answer
    /// when it is in. An answer for a mailbox that is no longer open is let go of.
    pub fn take_result(&mut self, session: &mut Session, ctx: &Context) -> Taken {
        let Some(running) = self.running.as_mut() else { return Taken::Nothing };
        let (account, quiet) = (running.account, running.quiet);
        if running.opening != session.openings() {
            log::info!("send/receive: the answer is for a mailbox that is no longer open; let go of");
            self.drop_run();
            return Taken::Nothing;
        }
        let mut piece_taken = false;
        let mut over = false;
        // A few a call: each one changes the lists, and the caller should stay quick.
        for _ in 0..4 {
            match running.pieces.try_recv() {
                Ok(piece) => {
                    piece_taken = true;
                    // New mail may be waiting to be fetched whole: the background looks again.
                    if !piece.pages.is_empty() {
                        session.driver.fetch.wake();
                    }
                    match mailcraft_engine::cmd::graph_mail::apply_graph_mail(session, piece) {
                        Ok(said) => running.errors.extend(
                            said["errors"]
                                .as_array()
                                .map(|all| all.iter().filter_map(|v| v.as_str()).map(str::to_string).collect::<Vec<_>>())
                                .unwrap_or_default(),
                        ),
                        Err(e) => {
                            running.errors.push(e.to_string());
                            session.driver.sending.doubted();
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    over = true;
                    break;
                }
            }
        }
        // Its first piece made the changes made here and sent the Outbox; what waits beyond what
        // waits now was done here since.
        if piece_taken && running.baseline.is_none() {
            running.baseline = Some(mailcraft_engine::sync_driver::waiting_to_go(session, account));
        }
        // Something the owner did waits while a long run is still bringing mail down: the run is
        // called off, which loses nothing, and the next one tells the server first.
        if !over
            && !running.called_off
            && !running.stopped
            && let Some(baseline) = running.baseline
            && session.driver.auto_sync()
            && session.driver.round.run_look_due(true, ctx.now_ms)
            && mailcraft_engine::sync_driver::waiting_to_go(session, account) > baseline
        {
            self.cancel.store(true, Ordering::Relaxed);
            running.called_off = true;
        }
        if !over {
            return if piece_taken { Taken::Piece } else { Taken::Pending };
        }
        let (mut result, answered) = match running.answer.try_recv() {
            Ok(r) => (r, true),
            Err(TryRecvError::Empty) => return Taken::Pending,
            Err(TryRecvError::Disconnected) => {
                log::warn!("send/receive: the job ended without an answer (its thread ended early)");
                (GraphResult { account, ..Default::default() }, false)
            }
        };
        let (errors, called_off) = (std::mem::take(&mut running.errors), running.called_off);
        self.running = None;
        // Called off so that what the owner did could go out: it goes on at once, as it was.
        if called_off {
            session.driver.round.put_back(account, quiet);
        }
        self.finish(session, ctx, account, answered, &mut result, errors)
    }

    /// The run's answer is in: tokens, what was sent, what was held, and the end of the run.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        account: Id,
        answered: bool,
        result: &mut GraphResult,
        mut errors: Vec<String>,
    ) -> Taken {
        // What was handed over and whose answer was lost may have gone: held for the owner, never
        // sent again by itself (AU3-26).
        let mut said = Vec::new();
        if mailcraft_engine::cmd::graph_send::hold_lost(
            &mut session.mailbox,
            account,
            &result.unsure,
            "Microsoft",
            session.interface_language,
            &mut said,
        ) {
            session.mailbox_changed();
        }
        // What the run sent is filed by now, and the rest didn't go: no message of the account is
        // marked as being sent any more.
        if answered && result.mail.is_none() {
            session.send_run_answered(account);
        }
        errors.append(&mut result.errors);
        if result.token_refused {
            self.tokens.remove(&account);
        }
        let mut wrong_person = false;
        if let Some(tokens) = result.tokens.take() {
            wrong_person = self.adopt_tokens(session, ctx, account, tokens) == Adopted::WrongPerson;
        }
        // Whether the calendar and contacts need the owner's permission is known from the answer.
        if result.consent_needed {
            session.driver.calendar.consent_needed.insert(account);
        } else {
            session.driver.calendar.consent_needed.remove(&account);
        }
        if !result.sent.is_empty() {
            let sent = mailcraft_model::sync::SyncResult { account, sent: result.sent.clone(), filed: result.sent.clone(), ..Default::default() };
            if let Err(e) = mailcraft_engine::cmd::account::apply_sync(session, sent) {
                errors.push(e.to_string());
            }
        }
        session.sync_progress.end(account, false);
        // What still waits after this run doesn't start another every few seconds.
        let waiting = mailcraft_engine::sync_driver::waiting_to_go(session, account);
        session.driver.round.note_pushed(account, waiting, ctx.now_ms);
        if wrong_person {
            return Taken::WrongPerson { account };
        }
        // No token for the mail: Microsoft wants the owner's say-so, or the sign-in has run out.
        if result.mail.is_none() && (result.consent_needed || result.sign_in_refused) {
            return Taken::SignInAgain { account, consent: result.consent_needed };
        }
        if let Some(mail) = result.mail.take() {
            match mailcraft_engine::cmd::graph_mail::apply_graph_mail(session, mail) {
                Ok(taken) => errors.extend(
                    taken["errors"]
                        .as_array()
                        .map(|all| all.iter().filter_map(|v| v.as_str()).map(str::to_string).collect::<Vec<_>>())
                        .unwrap_or_default(),
                ),
                Err(e) => errors.push(e.to_string()),
            }
        }
        Taken::Done { account, errors }
    }

    /// Keep the account's newest refresh token in the credential store, when the owner lets it be.
    fn remember(&mut self, session: &Session, ctx: &Context, account: Id, refresh: &str) {
        let (Some(store), Some(a)) = (ctx.secrets, session.mailbox.account(account)) else { return };
        if !ctx.remember || ctx.held() || refresh.is_empty() {
            return;
        }
        let key = secret_key("refresh", a);
        // Every entry written must be reportable for the data-removal contract: with the list full
        // and this key not in it, nothing is written (the token stays in memory for the session).
        if self.written.len() >= 1000 && !self.written.contains(&key) {
            log::warn!("couldn't keep a sign-in in the operating system's credential store: too many entries are waiting to be reported");
            return;
        }
        if store.set(&key, refresh).is_err() {
            // A store error can echo its input; never put it in diagnostics. A partial write must
            // not leave a credential that appears successfully saved.
            log::warn!("couldn't keep a sign-in in the operating system's credential store");
            store.delete(&key);
        } else if !self.written.contains(&key) {
            self.written.push(key);
        }
    }

    /// [`what_to_run`](Self::what_to_run) and [`take_result`](Self::take_result) in the order a
    /// front end calls them at each tick: what the run under way brought is taken in first, and
    /// when nothing is under way, what is due starts.
    pub fn run_due(&mut self, host: &Host, session: &mut Session, ctx: &Context) -> (Taken, Step) {
        let taken = self.take_result(session, ctx);
        if self.running.is_some() {
            return (taken, Step::Waiting);
        }
        (taken, self.what_to_run(host, session, ctx))
    }
}

/// How a run is done: over the real network, or as a test says.
type Runner = Arc<dyn Fn(GraphRequest) -> GraphResult + Send + Sync>;

fn real_runner() -> Runner {
    #[cfg(not(target_arch = "wasm32"))]
    return Arc::new(crate::graph::graph);
    #[cfg(target_arch = "wasm32")]
    Arc::new(|request| GraphResult {
        account: request.account.id,
        errors: vec!["this build can't connect to Microsoft".into()],
        ..Default::default()
    })
}
