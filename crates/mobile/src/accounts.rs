// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accounts and sign-in for the phone: Add Account (a Microsoft account), the permissions that
//! are asked for by themselves, the message for an administrator, and Remove Account.
//!
//! The owner signs in on Microsoft's own page in the system browser; the page comes back to the
//! app's custom scheme and the native side gives the address it was opened with to
//! [`Core::hand_back_redirect`], whole. Everything about it is read here, in Rust, by the OAuth
//! strategy of `mailcraft-net` (`oauth/scheme.rs`): the scheme and the path to the letter, the
//! state, the fields, the sizes. The native side passes the address through and does nothing else
//! with it. The tokens never cross the binding: the refresh token goes to the platform's
//! credential store and nowhere else, the access token is dropped, and neither appears in an
//! error or a log line.
//!
//! Permissions (To Do, shared mailboxes, sending from one) are asked for by themselves after the
//! app has asked the owner, never on the page of a sign-in, so that a sign-in never waits on an
//! administrator.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, PoisonError};

use mailcraft_host::signin::{
    self, Checked, Ending, Endpoints, FavoritesRefusal, NetError, Page, Permission as NetPermission, Resource, Standing, Why, admin_consent_url,
    permission_endpoints,
};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{AccountKind, Directory};
use serde_json::{Value, json};

use crate::queue::{Queue, Shed, Weight};
use crate::{Core, CoreError};

/// A permission that is asked for by itself, after the app has asked the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Permission {
    /// Reading and changing the owner's Microsoft To Do lists and tasks.
    Tasks,
    /// Reading and changing the shared mailboxes the owner was given access to.
    SharedMail,
    /// Sending from a shared mailbox, as it or on its behalf.
    SharedSend,
}

impl From<Standing> for PermissionState {
    fn from(s: Standing) -> PermissionState {
        match s {
            Standing::Allowed => PermissionState::Allowed,
            Standing::NotAllowed => PermissionState::NotAllowed,
            Standing::SignInNeeded => PermissionState::SignInNeeded,
            Standing::Unknown => PermissionState::Unknown,
        }
    }
}

impl From<Permission> for NetPermission {
    fn from(p: Permission) -> NetPermission {
        match p {
            Permission::Tasks => NetPermission::Tasks,
            Permission::SharedMail => NetPermission::SharedMail,
            Permission::SharedSend => NetPermission::SharedSend,
        }
    }
}

/// How a sign-in for an account ended.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SignInOutcome {
    /// Signed in: the account is in the mailbox (it was there already when the owner signed in
    /// to an account they had).
    SignedIn { account: u64, email: String, name: String },
    /// An administrator of the organisation has to approve MailCraft first.
    NeedsApproval,
    /// The page came back with a no, or the owner left the page that said an administrator has
    /// to approve first (Microsoft sends the same for both).
    Declined,
    /// Nothing came back in the time that is waited.
    TimedOut,
    /// Called off from here.
    Cancelled,
    /// The organisation's rules, Conditional Access among them, refuse this sign-in: not a fault,
    /// and not something an approval changes.
    OrganisationBlocks,
    /// No browser could be started for the page.
    NoBrowser,
    /// Anything else; `reason` is in the program's words.
    Failed { reason: String },
}

/// How asking for a permission ended.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PermissionOutcome {
    /// Given: the account's sign-in covers it now.
    Granted,
    NeedsApproval,
    OrganisationBlocks,
    /// Microsoft has no such permission for this kind of account.
    NotForThisAccount,
    Declined,
    TimedOut,
    Cancelled,
    NoBrowser,
    Failed {
        reason: String,
    },
}

/// Whether an account's sign-in covers a permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PermissionState {
    Allowed,
    /// Not given yet: the app can ask the owner.
    NotAllowed,
    /// The account's sign-in is over altogether: the owner signs in again first.
    SignInNeeded,
    /// Microsoft couldn't be asked (no network, a server's "not now").
    Unknown,
}

/// What the sign-ins of a core hold: the host's (where the redirect lands, and the one that is
/// under way), and for the tests a token server and a page of their own.
#[derive(Default)]
pub(crate) struct SignIns {
    host: signin::SignIns,
    /// A token server and a page of the tests' own, for a sign-in at them (never in a build that
    /// isn't a test's: nothing sets it).
    #[cfg(test)]
    pub(crate) server: Mutex<Option<(String, String)>>,
}

impl SignIns {
    pub(crate) fn begin(&self) -> Option<signin::Place<'_>> {
        self.host.begin()
    }

    /// Call off the sign-in that is open, if one is.
    pub(crate) fn cancel(&self) {
        self.host.cancel();
    }

    /// `endpoints`, at the tests' own server when a test set one.
    fn at(&self, endpoints: Endpoints) -> Endpoints {
        #[cfg(test)]
        if let Some((authorize, token)) = self.server.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return Endpoints { authorize, token, ..endpoints };
        }
        endpoints
    }
}

/// A reply to one call: the queue holds one, never reaches its bound.
impl Shed for Page {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

impl Shed for Checked {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

fn failed(reason: &str) -> SignInOutcome {
    SignInOutcome::Failed { reason: reason.to_string() }
}

fn not_granted(reason: &str) -> PermissionOutcome {
    PermissionOutcome::Failed { reason: reason.to_string() }
}

impl Core {
    /// Run a sign-in on the page on a thread of its own, which waits as long as the owner takes
    /// (five minutes at most), and give what it came to. The page and the waiting are the host's
    /// (`mailcraft_host::signin`); this only lends it a thread and an answer to wait for.
    async fn page(&self, endpoints: Endpoints, hint: String, why: Why, cancel: Arc<AtomicBool>) -> Page {
        let armer = self.signins.host.armer();
        let landed = armer.arm();
        let reply = Queue::<Page>::new();
        let platform = self.native.clone();
        let job_reply = reply.clone();
        let job = move || signin::page(platform.as_ref(), landed, &armer, &endpoints, &hint, why, &cancel);
        // (A panic in the job is an answer, not a wait that never ends.)
        let spawned = std::thread::Builder::new().name("mailcraft-sign-in".to_string()).spawn(move || {
            let page = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(Page::Unable("the sign-in stopped unexpectedly"));
            job_reply.push(page);
        });
        if spawned.is_err() {
            return Page::Unable("a thread for the sign-in could not be started");
        }
        reply.next().await.unwrap_or(Page::Ended(Ending::Cancelled))
    }

    /// The address and kind of an account of the mailbox.
    async fn email_of(&self, account: u64) -> Result<String, CoreError> {
        self.call(move |s| {
            let found = s.mailbox.accounts.iter().find(|a| a.id == account && a.kind == AccountKind::Microsoft);
            found.map(|a| a.email.clone()).ok_or_else(|| CoreError::Failed { reason: "there is no such account".to_string() })
        })
        .await
    }
}

fn ending_outcome(end: Ending) -> SignInOutcome {
    match end {
        Ending::Approval => SignInOutcome::NeedsApproval,
        Ending::Declined => SignInOutcome::Declined,
        Ending::TimedOut => SignInOutcome::TimedOut,
        Ending::Cancelled => SignInOutcome::Cancelled,
        Ending::Blocked => SignInOutcome::OrganisationBlocks,
        Ending::Other(e) => failed(&e.to_string()),
    }
}

fn refusal_outcome(refusal: FavoritesRefusal) -> PermissionOutcome {
    match refusal {
        FavoritesRefusal::Approval => PermissionOutcome::NeedsApproval,
        FavoritesRefusal::Organisation => PermissionOutcome::OrganisationBlocks,
        FavoritesRefusal::Account => PermissionOutcome::NotForThisAccount,
        FavoritesRefusal::Other(NetError::SignInCancelled) => PermissionOutcome::Cancelled,
        FavoritesRefusal::Other(NetError::SignInTimedOut) => PermissionOutcome::TimedOut,
        // (The owner's own no, or the page left: Microsoft says the same for both.)
        FavoritesRefusal::Other(e) if e.is_refused_sign_in() => PermissionOutcome::Declined,
        FavoritesRefusal::Other(e) => not_granted(&e.to_string()),
    }
}

#[uniffi::export]
impl Core {
    /// Give the address the system opened the app with (the redirect's) to the sign-in that waits
    /// for it, whole and unread. True when a sign-in took it; false when none waits, one already
    /// had an address, or it is too long. Anything else about it is judged in Rust.
    pub fn hand_back_redirect(&self, address: String) -> bool {
        self.signins.host.deliver(address)
    }

    /// Call off the sign-in that is open, if one is: it ends as `Cancelled`.
    pub fn cancel_sign_in(&self) {
        self.signins.host.cancel();
    }

    /// Add a Microsoft account: the provider's page opens in the system browser through the
    /// platform, and what comes back is checked here. `email` is only a hint for the page and
    /// may be empty. On success the account is in the mailbox, its sign-in is in the platform's
    /// credential store (when the owner lets sign-ins be remembered) and Send/Receive has it for
    /// the session; nothing is kept if the store refuses, nor if the call is dropped before the
    /// account is in. `settings` is what the owner chose in Add Account of what the phone keeps
    /// of a new account's mail (its window, and the phone's storage cap); an account the owner
    /// had already keeps its own.
    pub async fn sign_in_microsoft(&self, email: String, settings: crate::limits::DownloadSettings) -> Result<SignInOutcome, CoreError> {
        let (address, tokens) = match self.account_sign_in(signin::hint_of(&email)).await? {
            Ok(signed) => signed,
            Err(outcome) => return Ok(outcome),
        };
        let name = tokens.name.clone();
        let directory = match Directory::of_tenant(&tokens.tenant) {
            Directory::Organisation => Some("organisation"),
            Directory::Personal => Some("personal"),
            Directory::Unknown => None,
        };
        let new_account = json!({
            "email": address, "name": name, "displayName": name, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "directory": directory,
        });
        // One job on the session's thread: the sign-in is kept, the account is added and
        // Send/Receive takes the sign-in, or none of it. A call dropped (or a core shut down) before
        // the job runs keeps nothing, and a job that runs needs nobody waiting for it to finish.
        let (platform, sync, for_call, lang) = (self.native.clone(), self.sync.clone(), address.clone(), self.lang());
        let added = self
            .call(move |s| {
                // (An account the owner has signs in again: its settings stay as they are. A shared
                // mailbox is read with another account's sign-in and never signs in itself.)
                let known = s
                    .mailbox
                    .accounts
                    .iter()
                    .find(|a| a.kind == AccountKind::Microsoft && !a.is_shared() && a.email.trim().eq_ignore_ascii_case(&for_call));
                let known = known.map(|a| a.id);
                // Kept in the credential store only when the owner lets sign-ins be remembered; else
                // Send/Receive holds it for the session alone. (What was kept before for an account
                // the owner has goes back if Send/Receive refuses the new sign-in.)
                let remember = sync.remembers();
                let before = if remember && known.is_some() { signin::kept_refresh(platform.as_ref(), &for_call) } else { None };
                if remember && let Err(why) = signin::keep_refresh(platform.as_ref(), &for_call, &tokens.refresh) {
                    return Ok(Err(why.to_string()));
                }
                let undo_kept = |platform: &dyn mailcraft_host::Platform| {
                    if remember {
                        signin::restore_refresh(platform, &for_call, before.as_deref());
                    }
                };
                let account = match known {
                    Some(id) => id,
                    None => {
                        let made = s.run("account.add", &new_account).map_err(CoreError::from);
                        // The phone keeps what the owner chose of a new account's mail (MB3.16).
                        let made = made.and_then(|m| m.get("id").and_then(Value::as_u64).ok_or(CoreError::Internal)).and_then(|id| {
                            match crate::limits::apply_to(s, id, settings) {
                                Ok(()) => Ok(id),
                                Err(e) => {
                                    let _ = s.run("account.remove", &json!({"id": id}));
                                    Err(CoreError::from(e))
                                }
                            }
                        });
                        match made {
                            Ok(id) => id,
                            Err(error) => {
                                // (Nothing is kept of a sign-in that made no account.)
                                undo_kept(platform.as_ref());
                                return Err(error);
                            }
                        }
                    }
                };
                // Send/Receive has the sign-in for the session at once, and an account that had to
                // be signed in again no longer does. When it refuses the sign-in, nothing of it is
                // left: an account made for it goes again, and the credential store has what it had.
                let adopted = sync.adopt(s, account, tokens);
                if let Some(why) = crate::sync_api::adopt_refusal(lang, adopted) {
                    if known.is_none() && s.run("account.remove", &json!({"id": account})).is_err() {
                        log::warn!("accounts: an account added for a refused sign-in couldn't be removed again");
                    }
                    undo_kept(platform.as_ref());
                    return Ok(Err(why));
                }
                Ok(Ok(account))
            })
            .await;
        match added {
            Ok(Ok(account)) => Ok(SignInOutcome::SignedIn { account, email: address, name }),
            Ok(Err(reason)) => Ok(SignInOutcome::Failed { reason }),
            Err(CoreError::Cancelled) => Err(CoreError::Cancelled),
            Err(CoreError::Failed { reason } | CoreError::BadRequest { reason }) => Ok(SignInOutcome::Failed { reason }),
            Err(_) => Ok(failed("the account could not be added")),
        }
    }

    /// Ask the owner, on Microsoft's page, for `permission` for `account`, by itself and never
    /// with a sign-in. Ask the owner first in the app: this opens the page. The new sign-in
    /// replaces the old in the credential store only if it is the account's own.
    pub async fn grant_permission(&self, account: u64, permission: Permission) -> Result<PermissionOutcome, CoreError> {
        let email = self.email_of(account).await?;
        let Some(place) = self.signins.begin() else { return Ok(not_granted("a sign-in is already open")) };
        // (Looked at with the place held: `shutdown` calls off the one that is open, and none opens after it.)
        if self.closing.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled);
        }
        let endpoints = match signin::client_oauth() {
            Some(o) => self.signins.at(permission_endpoints(&o, permission.into())),
            None => return Ok(not_granted("this build of MailCraft has no Microsoft sign-in set up")),
        };
        let page = self.page(endpoints, email.clone(), Why::Permission, place.cancel_flag()).await;
        let tokens = match page {
            Page::Tokens(tokens) => tokens,
            Page::Refused(refusal) => return Ok(refusal_outcome(refusal)),
            Page::NoBrowser => return Ok(PermissionOutcome::NoBrowser),
            Page::Unable(why) => return Ok(not_granted(why)),
            Page::Ended(_) => return Ok(not_granted("the sign-in stopped unexpectedly")),
        };
        // The owner picks the account on Microsoft's page: somebody else's sign-in is not this account's.
        if tokens.someone_else(&email, "") {
            return Ok(not_granted("that sign-in was for another account"));
        }
        // Kept on the session's thread, after a look that the account is still there: removing it
        // is on that thread too, so a sign-in finishing after a removal never restores its credential.
        // (With sign-ins not remembered nothing is written: the permission is Microsoft's to keep.)
        let (platform, tokens_refresh, for_email, sync) = (self.native.clone(), tokens.refresh.clone(), email.clone(), self.sync.clone());
        let kept = self
            .call(move |s| {
                let there = s.mailbox.accounts.iter().any(|a| a.id == account && a.kind == AccountKind::Microsoft);
                Ok(match (there, sync.remembers()) {
                    (false, _) => None,
                    (true, true) => Some(signin::keep_refresh(platform.as_ref(), &for_email, &tokens_refresh)),
                    (true, false) => Some(Ok(())),
                })
            })
            .await?;
        match kept {
            Some(Ok(())) => Ok(PermissionOutcome::Granted),
            Some(Err(why)) => Ok(not_granted(why)),
            None => Ok(not_granted("there is no such account")),
        }
    }

    /// Whether `account`'s sign-in covers `permission`, asked of Microsoft with the account's
    /// refresh token as Send/Receive has it: the session's first (a sign-in the owner doesn't let
    /// be remembered is in memory alone), then the credential store's. Nothing is opened and the
    /// owner is not asked. Nothing is stored of the answer.
    pub async fn permission_state(&self, account: u64, permission: Permission) -> Result<PermissionState, CoreError> {
        let email = self.email_of(account).await?;
        let Some(o) = signin::client_oauth() else { return Ok(PermissionState::Unknown) };
        let mail = self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail));
        let graph = self.signins.at(Endpoints::for_resource(&o, Resource::Graph));
        let sync = self.sync.clone();
        let refresh = self.call(move |s| Ok(sync.refresh_for(s, account))).await?;
        let reply = Queue::<Checked>::new();
        let job_reply = reply.clone();
        let for_job = email.clone();
        let job = move || signin::permission_standing(refresh.as_deref(), &for_job, permission.into(), &mail, &graph);
        let spawned = std::thread::Builder::new().name("mailcraft-permission".to_string()).spawn(move || {
            let checked =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(Checked { standing: Standing::Unknown, renewed: None });
            job_reply.push(checked);
        });
        if spawned.is_err() {
            return Ok(PermissionState::Unknown);
        }
        let Some(Checked { standing, renewed }) = reply.next().await else { return Ok(PermissionState::Unknown) };
        // The check writes nothing itself. A sign-in Microsoft renewed in its answer is kept here, on
        // the session's thread, and only while its account is still there: removing it is on that
        // thread too, so an answer that comes after a removal never puts its sign-in back, and a
        // call dropped meanwhile (or a core shutting down) keeps nothing either.
        if let Some(renewed) = renewed {
            let (platform, sync) = (self.native.clone(), self.sync.clone());
            let _ = self
                .call(move |s| {
                    // The very account that was checked (one removed and added again is another, with
                    // its own sign-in), and only in place of the sign-in the check began with (the
                    // session's, and the stored one when the owner lets sign-ins be remembered): one
                    // forgotten, or replaced by a newer sign-in, meanwhile is never written over.
                    let there = s
                        .mailbox
                        .accounts
                        .iter()
                        .any(|a| a.id == account && a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&email));
                    if there {
                        // The session's sign-in, whether or not it is remembered, and the stored
                        // one when the owner lets it be.
                        sync.take_renewed(s, account, &renewed);
                        if sync.remembers() {
                            renewed.keep(platform.as_ref(), &email);
                        }
                    }
                    Ok(())
                })
                .await;
        }
        Ok(PermissionState::from(standing))
    }

    /// The address of Microsoft's page where an administrator approves MailCraft for an
    /// organisation, once for everybody in it, for the owner to pass on. Nothing is opened or
    /// sent. `email` names the organisation's directory by its domain when it can; it may be empty.
    /// The address names the application, the directory and the permissions, and nothing of the owner.
    pub fn admin_approval_link(&self, email: String) -> Result<String, CoreError> {
        let o = signin::client_oauth()
            .ok_or_else(|| CoreError::Failed { reason: "this build of MailCraft has no Microsoft sign-in set up".to_string() })?;
        admin_consent_url(&o, &signin::tenant_of(&email))
            .map_err(|_| CoreError::Failed { reason: "the address of the approval page could not be made".to_string() })
    }

    /// Remove an account: its mail and what is kept for it on this phone (`account.remove`), and
    /// its sign-in from the platform's credential store. Nothing on the server is touched. The
    /// sign-in is removed only once the account is.
    pub async fn remove_account(&self, account: u64) -> Result<(), CoreError> {
        let email = self.email_of(account).await?;
        // One job: the account goes and, when no other account has its address, so does its sign-in,
        // whatever becomes of the caller (a shutdown between two calls would leave the credential).
        let platform = self.native.clone();
        self.call(move |s| {
            s.run("account.remove", &json!({"id": account})).map_err(CoreError::from)?;
            // (Nothing else of the account's is held under its address: a mailbox with another account
            // of the same address keeps it.)
            let still = s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&email));
            // (The account stays removed when the store refuses: the owner is told its sign-in is still kept.)
            if !still && signin::forget(platform.as_ref(), &email).is_err() {
                return Err(CoreError::Failed {
                    reason: "the account was removed, but its sign-in couldn't be removed from this phone's secure store".to_string(),
                });
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl Core {
    /// The one sign-in on the page of a phone, for Add Account and for an account signing in
    /// again ([`Core::sign_in`]): one at a time, its redirect handed over by
    /// [`Core::hand_back_redirect`] and called off by [`Core::cancel_sign_in`]. Gives who signed in
    /// (as Microsoft names them, never the hint) and the tokens, or how it ended.
    pub(crate) async fn account_sign_in(&self, hint: String) -> Result<Result<(String, Tokens), SignInOutcome>, CoreError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled);
        }
        let Some(place) = self.signins.begin() else { return Ok(Err(failed("a sign-in is already open"))) };
        // (Looked at again with the place held: `shutdown` calls off the one that is open.)
        if self.closing.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled);
        }
        let endpoints = match signin::client_oauth() {
            Some(o) => self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail)),
            None => return Ok(Err(failed("this build of MailCraft has no Microsoft sign-in set up"))),
        };
        let page = self.page(endpoints, hint, Why::Account, place.cancel_flag()).await;
        // (The place is given up here, before the account is dealt with: the page is over.)
        drop(place);
        let tokens = match page {
            Page::Tokens(tokens) => tokens,
            Page::Ended(end) => return Ok(Err(ending_outcome(end))),
            Page::NoBrowser => return Ok(Err(SignInOutcome::NoBrowser)),
            Page::Unable(why) => return Ok(Err(failed(why))),
            Page::Refused(_) => return Ok(Err(failed("the sign-in stopped unexpectedly"))),
        };
        // Who signed in is who Microsoft says (the owner picks the account on its page), never
        // what was typed: the typed address is only a hint for the page.
        match signin::signed_in_address(&tokens) {
            Some(address) => Ok(Ok((address, tokens))),
            None => Ok(Err(failed("Microsoft didn't say which account signed in"))),
        }
    }
}
