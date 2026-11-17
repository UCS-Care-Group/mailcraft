// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the apps call to send and receive mail and to sign an account in.
//!
//! Microsoft (Graph) accounts only for now: another kind of account says so, in words
//! ([`SyncStatus::unsupported`], the answer of `sync_now`), and is never run, never silently
//! skipped and never a crash.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_host::sync::Adopted;
use mailcraft_i18n::Lang;
use mailcraft_model::AccountKind;

use crate::core::CancelOnDrop;
use crate::queue::Queue;
use crate::sync::{SyncAnswer, SyncHost, SyncStatus, SyncSummary, Wake};
use crate::{Core, CoreError, SignInOutcome, SignInsForgotten};

/// The shortest and the longest time a background sync is given (the system gives a worker some
/// minutes; a `0` or a day is a mistake).
const LEAST_BUDGET_MS: u64 = 500;
const MOST_BUDGET_MS: u64 = 10 * 60 * 1000;

#[uniffi::export]
impl Core {
    /// Send and receive now, the owner having asked: starts a round of the accounts that are kept
    /// in step through Microsoft Graph, and returns at once. Progress, new mail, a sign-in that has
    /// to be redone and the end of the round come as events. `started` is false, with the reason
    /// for the owner, while Work Offline is on, with no network, with no account, or with only
    /// accounts of a kind that isn't sent and received here yet.
    pub async fn sync_now(&self) -> Result<SyncAnswer, CoreError> {
        let sync = self.sync.clone();
        self.call(move |session| Ok(sync.ask(session, None))).await
    }

    /// Hold `account` back from Send/Receive, the calendar's, the to-dos' and the background's
    /// downloads that start by themselves (`held`), or let it go again: for a new account whose
    /// first download the owner said "not now" to, for want of room. A background run leaves it
    /// out; the owner's yes (`held` false) or a Send/Receive asked for (`sync_now`, a pull to
    /// refresh) lets it go. Kept while the core is (the app says it again when it starts).
    pub async fn hold_background_sync(&self, account: u64, held: bool) -> Result<(), CoreError> {
        let sync = self.sync.clone();
        self.call(move |session| {
            if session.mailbox.account(account).is_none() {
                return Err(CoreError::BadRequest { reason: "there is no such account".to_string() });
            }
            sync.hold(account, held);
            Ok(())
        })
        .await
    }

    /// One round to its end, or until `budget_ms` runs out (a worker's time: it is held between
    /// half a second and ten minutes): the round is then called off, and the run under way stops
    /// at its next safe place. For the system's scheduler, which calls it with a network. What it
    /// found is applied to the mailbox on the session's thread before it returns; it raises the
    /// same events as `sync_now`.
    pub async fn background_sync(&self, budget_ms: u64) -> Result<SyncSummary, CoreError> {
        let budget = Duration::from_millis(budget_ms.clamp(LEAST_BUDGET_MS, MOST_BUDGET_MS));
        let replies = Queue::<Wake>::new();
        // Dropping this call (the worker stopped, its coroutine cancelled) calls the round off too,
        // so that the budget bounds it whoever stops waiting first; from the moment it is started,
        // its answer still on its way back included.
        let mut stop_on_drop = StopOnDrop { core: self, shared: Arc::new(Started::default()), armed: true };
        let (sync, waiter, shared) = (self.sync.clone(), replies.clone(), stop_on_drop.shared.clone());
        let answer = self.call(move |session| Ok(start_attached(&sync, session, waiter, &shared))).await?;
        if !answer.started {
            stop_on_drop.armed = false;
            return Ok(SyncSummary { ran: false, finished: true, errors: vec![answer.message], ..Default::default() });
        }
        // The timer: one short-lived thread, ended when this call is.
        let over = Arc::new(AtomicBool::new(false));
        let _end = CancelOnDrop(over.clone());
        let (timer_over, timer_replies, closing) = (over.clone(), replies.clone(), self.closing.clone());
        std::thread::Builder::new()
            .name("mailcraft-sync-budget".to_string())
            .spawn(move || {
                let end = std::time::Instant::now().checked_add(budget);
                while !timer_over.load(Ordering::Acquire) {
                    if closing.load(Ordering::Acquire) || end.is_some_and(|end| std::time::Instant::now() >= end) {
                        timer_replies.push(Wake::Budget);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
            .map_err(|_| CoreError::Failed { reason: "a timer for the sync could not be started".to_string() })?;
        let out = match replies.next().await {
            Some(Wake::Done(summary)) => Ok(summary),
            Some(Wake::Budget) => {
                let sync = self.sync.clone();
                self.call(move |session| Ok(sync.stop(session))).await
            }
            None => Err(CoreError::Cancelled),
        };
        stop_on_drop.armed = false;
        out
    }

    /// Where Send/Receive stands: whether a round is under way, what the last said went wrong,
    /// the accounts that have to be signed in again, and those that aren't sent and received here.
    pub async fn sync_status(&self) -> Result<SyncStatus, CoreError> {
        let sync = self.sync.clone();
        self.call(move |session| Ok(sync.status(session))).await
    }

    /// Whether the refresh token of a sign-in is kept in the platform's credential store so that
    /// the next start signs in by itself (File › Options › remember; on by default).
    pub fn set_remember_sign_ins(&self, on: bool) {
        self.sync.set_remember(on);
    }

    /// Whether new mail raises a `Notification` event when a round brings some (on by default).
    pub fn set_notify_new_mail(&self, on: bool) {
        self.sync.set_notify_mail(on);
    }

    /// Whether a notification for new mail says "New mail" and nothing of the message: no sender,
    /// no subject, no count, whatever the lock screen shows (the owner's privacy switch in
    /// Settings, off by default). The app tells the core at start and when the switch changes.
    pub fn set_notify_new_mail_only(&self, on: bool) {
        self.sync.set_notify_new_mail_only(on);
    }

    /// Whether new mail that Gmail put in a category other than Primary raises no notification
    /// (the owner's "Primary mail only" switch in Settings, off by default). Microsoft's
    /// Focused and Other are not kept by the mailbox, so they are not told apart yet.
    pub fn set_notify_primary_only(&self, on: bool) {
        self.sync.set_notify_primary_only(on);
    }

    /// Remove every sign-in the core kept (from the credential store and from memory), for
    /// "remove all my data". Says how many entries were looked for, and when the list of the
    /// sign-ins kept was damaged (and set aside) that older ones may still be in the store (the
    /// app shows those words, then calls [`Core::sign_ins_notice_shown`]); an error says that
    /// some are still in the store (calling it again tries again).
    pub async fn forget_sign_ins(&self) -> Result<SignInsForgotten, CoreError> {
        let sync = self.sync.clone();
        self.call(move |session| match sync.forget_sign_ins(session) {
            Ok(forgotten) => Ok(SignInsForgotten {
                looked_for: u64::try_from(forgotten.count).unwrap_or(u64::MAX),
                may_remain: forgotten.some_may_remain.then(|| crate::core::may_remain(session.interface_language)),
            }),
            Err(reason) => Err(CoreError::Failed { reason }),
        })
        .await
    }

    /// The app has shown the owner the words that older sign-ins may remain
    /// (`DataRemoved::SignInsMayRemain`, `SignInsForgotten::may_remain`): later removals don't
    /// say it again. Until it is called they all do, after a restart too, so that a notice whose
    /// answer was lost on its way to the window is never lost with it.
    pub async fn sign_ins_notice_shown(&self) -> Result<(), CoreError> {
        let sync = self.sync.clone();
        self.call(move |_| {
            sync.notice_given();
            Ok(())
        })
        .await
    }

    /// Sign `account` (a Microsoft account) in again on the provider's page, its address as the
    /// page's hint. The same sign-in as Add Account's ([`Core::sign_in_microsoft`]): one at a time,
    /// the page opened through [`crate::Platform::open_sign_in_page`], the address the system
    /// opened the app with handed over by [`Core::hand_back_redirect`], called off by
    /// [`Core::cancel_sign_in`], by dropping the call or by `shutdown`. On success the sign-in is
    /// kept for the session and, when the owner lets it be, in the credential store; refused when
    /// it is somebody else's. Run `sync_now` after it.
    pub async fn sign_in(&self, account: u64) -> Result<(), CoreError> {
        let lang = self.lang();
        let address = self
            .call(move |session| {
                let Some(a) = session.mailbox.account(account) else {
                    return Err(fail(lang, words("That account is gone.")));
                };
                if a.kind != AccountKind::Microsoft || a.is_shared() {
                    return Err(fail(lang, words("Only a Microsoft account signs in on the phone for now.")));
                }
                Ok(a.email.clone())
            })
            .await?;
        let tokens = match self.account_sign_in(mailcraft_host::signin::hint_of(&address)).await? {
            Ok((_, tokens)) => tokens,
            Err(SignInOutcome::Cancelled) => return Err(CoreError::Cancelled),
            Err(SignInOutcome::NoBrowser) => return Err(fail(lang, words("The sign-in page couldn't be opened."))),
            Err(SignInOutcome::NeedsApproval) => {
                return Err(fail(lang, words("Your organisation has to approve MailCraft before you can sign in. Ask an administrator.")));
            }
            Err(SignInOutcome::Declined | SignInOutcome::OrganisationBlocks) => return Err(fail(lang, words("The sign-in was declined."))),
            Err(SignInOutcome::TimedOut) => return Err(fail(lang, words("Nothing came back from the sign-in page in time."))),
            Err(SignInOutcome::Failed { .. } | SignInOutcome::SignedIn { .. }) => {
                return Err(fail(lang, words("The sign-in didn't work. Try again.")));
            }
        };
        self.adopt_tokens(account, tokens).await
    }
}

impl Core {
    /// Hand the tokens of a sign-in to Send/Receive: refused when they are somebody else's, kept
    /// for the session otherwise, and remembered in the credential store when the owner lets it be.
    pub(crate) async fn adopt_tokens(&self, account: u64, tokens: mailcraft_model::sync::Tokens) -> Result<(), CoreError> {
        let lang = self.lang();
        let sync = self.sync.clone();
        let adopted = self.call(move |session| Ok(sync.adopt(session, account, tokens))).await?;
        match adopt_refusal(lang, adopted) {
            None => Ok(()),
            Some(reason) => Err(CoreError::Failed { reason }),
        }
    }
}

/// Why Send/Receive didn't take a sign-in, in the interface language; nothing when it took it.
pub(crate) fn adopt_refusal(lang: Lang, adopted: Adopted) -> Option<String> {
    let text = match adopted {
        Adopted::Kept => return None,
        Adopted::WrongPerson => words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept."),
        Adopted::Refused => words("The account can't be signed in now."),
    };
    Some(lang.t(text).to_string())
}

/// Calls a `background_sync`'s round off when the call is dropped before it has its answer. It
/// only asks: the session's thread does it at its next wake, nothing is waited for in `drop`.
struct StopOnDrop<'a> {
    core: &'a Core,
    shared: Arc<Started>,
    armed: bool,
}

/// Start (or join) a round for a `background_sync`, on the session's thread, and note in `shared`
/// the round the call is attached to. The round is read before `ask`, which may end it before it
/// returns (nothing to run): read after, it would name the next round, which a later drop would
/// then call off in its place.
pub(crate) fn start_attached(sync: &SyncHost, session: &mut Session, waiter: Arc<Queue<Wake>>, shared: &Started) -> SyncAnswer {
    let round = sync.round();
    let answer = sync.ask(session, Some(waiter));
    if answer.started {
        shared.round.store(round.saturating_add(1), Ordering::SeqCst);
        // Dropped while this ran: the drop may not have seen the round, so it is called off here.
        if shared.dropped.load(Ordering::SeqCst) {
            sync.stop(session);
        }
    }
    answer
}

/// What the call's job and its drop tell each other: whichever comes second calls the round off.
#[derive(Default)]
pub(crate) struct Started {
    /// The round the call started, plus one; 0 until it has started one.
    pub(crate) round: AtomicU64,
    pub(crate) dropped: AtomicBool,
}

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.shared.dropped.store(true, Ordering::SeqCst);
        if let Some(round) = self.shared.round.load(Ordering::SeqCst).checked_sub(1) {
            self.core.sync.stop_later(round);
            self.core.enqueue_tick();
        }
    }
}

/// Marks a sentence for the catalogue (`cargo xtask i18n` gathers what is passed to it); the
/// words are put through the interface language where they are said.
fn words(text: &'static str) -> &'static str {
    text
}

fn fail(lang: Lang, text: &str) -> CoreError {
    CoreError::Failed { reason: lang.t(text).to_string() }
}
