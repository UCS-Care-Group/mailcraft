// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the apps call to send and receive mail and to sign an account in.
//!
//! Microsoft (Graph) accounts only for now: another kind of account says so, in words
//! ([`SyncStatus::unsupported`], the answer of `sync_now`), and is never run, never silently
//! skipped and never a crash.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_engine::sync_driver::{ClientIds, effective_oauth};
use mailcraft_host::sync::Adopted;
use mailcraft_i18n::Lang;
use mailcraft_model::{AccountKind, OAuth, OAuthProvider};

use crate::core::CancelOnDrop;
use crate::queue::Queue;
use crate::sign_in::{self, Ended};
use crate::sync::{SyncAnswer, SyncStatus, SyncSummary, Wake};
use crate::{Core, CoreError, RedirectKind, SignInsForgotten};

/// The shortest and the longest time a background sync is given (the system gives a worker some
/// minutes; a `0` or a day is a mistake).
const LEAST_BUDGET_MS: u64 = 500;
const MOST_BUDGET_MS: u64 = 10 * 60 * 1000;

/// Ends a sign-in's claim on the core when the call ends or is dropped.
struct Open(Arc<AtomicBool>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

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

    /// One round to its end, or until `budget_ms` runs out (a worker's time: it is held between
    /// half a second and ten minutes): the round is then called off, and the run under way stops
    /// at its next safe place. For the system's scheduler, which calls it with a network. What it
    /// found is applied to the mailbox on the session's thread before it returns; it raises the
    /// same events as `sync_now`.
    pub async fn background_sync(&self, budget_ms: u64) -> Result<SyncSummary, CoreError> {
        let budget = Duration::from_millis(budget_ms.clamp(LEAST_BUDGET_MS, MOST_BUDGET_MS));
        let replies = Queue::<Wake>::new();
        let (sync, waiter) = (self.sync.clone(), replies.clone());
        let answer = self.call(move |session| Ok(sync.ask(session, Some(waiter)))).await?;
        if !answer.started {
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
        match replies.next().await {
            Some(Wake::Done(summary)) => Ok(summary),
            Some(Wake::Budget) => {
                let sync = self.sync.clone();
                self.call(move |session| Ok(sync.stop(session))).await
            }
            None => Err(CoreError::Cancelled),
        }
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

    /// Sign `account` (a Microsoft account) in on the provider's page. Opens the page through
    /// [`crate::Platform::open_sign_in_page`] and waits for the app to hand over the address the
    /// system opened it with ([`Core::sign_in_redirect`]); dropping the call (or `shutdown`)
    /// calls the sign-in off. On success the sign-in is kept for the session and, when the owner
    /// lets it be, in the credential store; refused when it is somebody else's. Run `sync_now`
    /// after it.
    pub async fn sign_in(&self, account: u64) -> Result<(), CoreError> {
        let lang = self.lang();
        if self.signing_in.swap(true, Ordering::AcqRel) {
            return Err(fail(lang, words("A sign-in is already open.")));
        }
        let _open = Open(self.signing_in.clone());
        let scheme = match self.platform.redirect() {
            Ok(RedirectKind::CustomScheme { scheme }) => scheme,
            Ok(RedirectKind::Loopback) | Err(_) => return Err(fail(lang, words("Signing in isn't available on this phone."))),
        };
        let ids_account = account;
        let (oauth, address) = self
            .call(move |session| {
                let Some(a) = session.mailbox.account(ids_account) else {
                    return Err(fail(lang, words("That account is gone.")));
                };
                if a.kind != AccountKind::Microsoft || a.is_shared() {
                    return Err(fail(lang, words("Only a Microsoft account signs in on the phone for now.")));
                }
                let oauth = a.oauth.clone().unwrap_or(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
                let oauth = effective_oauth(oauth, &ClientIds { microsoft_built_in: crate::sync::MS_CLIENT_ID, ..Default::default() });
                Ok((oauth, a.email.clone()))
            })
            .await?;
        if oauth.client_id.is_empty() {
            return Err(fail(lang, words("This build has no application id to sign in with.")));
        }
        let tokens = self.page_sign_in(oauth, address, scheme).await?;
        self.adopt_tokens(account, tokens).await
    }

    /// The app hands over the address the system opened it with after the provider's page (a
    /// custom-scheme link). Taken only while a [`Core::sign_in`] waits, and only the first; it is
    /// read strictly there, and anything wrong with it ends the sign-in in an error that says
    /// nothing of it. Returns whether it was taken.
    pub fn sign_in_redirect(&self, address: String) -> bool {
        self.redirects.deliver(&address)
    }
}

impl Core {
    /// Hand the tokens of a sign-in to Send/Receive: refused when they are somebody else's, kept
    /// for the session otherwise, and remembered in the credential store when the owner lets it be.
    pub(crate) async fn adopt_tokens(&self, account: u64, tokens: mailcraft_model::sync::Tokens) -> Result<(), CoreError> {
        let lang = self.lang();
        let sync = self.sync.clone();
        let adopted = self.call(move |session| Ok(sync.adopt(session, account, tokens))).await?;
        match adopted {
            Adopted::Kept => Ok(()),
            Adopted::WrongPerson => Err(fail(lang, words("That sign-in is for somebody else's mailbox, not this account's. Nothing was kept."))),
            Adopted::Refused => Err(fail(lang, words("The account can't be signed in now."))),
        }
    }

    /// The page, on a thread of its own (it waits for the owner), its end as a reply.
    pub(crate) async fn page_sign_in(&self, oauth: OAuth, address: String, scheme: String) -> Result<mailcraft_model::sync::Tokens, CoreError> {
        self.page_sign_in_at(oauth, address, scheme, None).await
    }

    pub(crate) async fn page_sign_in_at(
        &self,
        oauth: OAuth,
        address: String,
        scheme: String,
        endpoints: Option<mailcraft_net::oauth::Endpoints>,
    ) -> Result<mailcraft_model::sync::Tokens, CoreError> {
        type Reply = Result<mailcraft_model::sync::Tokens, CoreError>;
        let lang = self.lang();
        let reply = Queue::<Reply>::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancel.clone());
        let (redirects, platform, out) = (self.redirects.clone(), self.platform.clone(), reply.clone());
        let attempt = redirects.arm();
        let thread_cancel = cancel.clone();
        let spawned = std::thread::Builder::new().name("mailcraft-sign-in".to_string()).spawn(move || {
            let page_failed = Arc::new(AtomicBool::new(false));
            let (opened_flag, opened_cancel) = (page_failed.clone(), thread_cancel.clone());
            let open = |url: &str| {
                if platform.open_sign_in_page(url.to_string()).is_err() {
                    // Nothing was opened, so nothing will come back: no waiting for it.
                    opened_flag.store(true, Ordering::Release);
                    opened_cancel.store(true, Ordering::Release);
                }
            };
            let done = sign_in::run(&oauth, &address, &scheme, &open, &redirects, &thread_cancel, endpoints.as_ref());
            redirects.disarm(attempt);
            out.push(match done {
                Ok(tokens) => Ok(tokens),
                Err(_) if page_failed.load(Ordering::Acquire) => Err(fail(lang, words("The sign-in page couldn't be opened."))),
                Err(Ended::Cancelled) => Err(CoreError::Cancelled),
                Err(Ended::Said(said)) => Err(fail(lang, said)),
            });
        });
        if spawned.is_err() {
            self.redirects.disarm(attempt);
            return Err(fail(lang, words("The sign-in couldn't be started.")));
        }
        reply.next().await.unwrap_or(Err(CoreError::Cancelled))
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
