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
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::{Mutex, PoisonError};

use mailcraft_host::signin::{
    self, Ending, Endpoints, FavoritesRefusal, NetError, Page, Permission as NetPermission, Resource, Standing, Why, admin_consent_url,
    permission_endpoints,
};
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

impl Shed for Standing {
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
    /// may be empty. On success the account is in the mailbox and its sign-in is in the
    /// platform's credential store; nothing is kept if the store refuses.
    pub async fn sign_in_microsoft(&self, email: String) -> Result<SignInOutcome, CoreError> {
        let Some(place) = self.signins.begin() else { return Ok(failed("a sign-in is already open")) };
        let endpoints = match signin::client_oauth() {
            Some(o) => self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail)),
            None => return Ok(failed("this build of MailCraft has no Microsoft sign-in set up")),
        };
        let hint = signin::hint_of(&email);
        let page = self.page(endpoints, hint, Why::Account, place.cancel_flag()).await;
        let tokens = match page {
            Page::Tokens(tokens) => tokens,
            Page::Ended(end) => return Ok(ending_outcome(end)),
            Page::NoBrowser => return Ok(SignInOutcome::NoBrowser),
            Page::Unable(why) => return Ok(failed(why)),
            Page::Refused(_) => return Ok(failed("the sign-in stopped unexpectedly")),
        };
        // Who signed in is who Microsoft says (the owner picks the account on its page), never
        // what was typed: the typed address is only a hint for the page.
        let Some(address) = signin::signed_in_address(&tokens) else { return Ok(failed("Microsoft didn't say which account signed in")) };
        // (Asked before anything is kept: a core that is shutting down stores nothing, and whether the
        // account was there decides whether a failure takes its sign-in back.)
        let existed = self.has_microsoft_account(&address).await?;
        if let Err(why) = signin::keep_refresh(self.native.as_ref(), &address, &tokens.refresh) {
            return Ok(failed(why));
        }
        let name = tokens.name.clone();
        let directory = match Directory::of_tenant(&tokens.tenant) {
            Directory::Organisation => Some("organisation"),
            Directory::Personal => Some("personal"),
            Directory::Unknown => None,
        };
        let params = json!({
            "email": address, "name": name, "displayName": name, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "directory": directory,
        });
        let (for_call, new_account) = (address.clone(), params);
        let added = self
            .call(move |s| {
                let known = s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&for_call));
                if let Some(known) = known {
                    // An account the owner has signs in again: its settings stay as they are.
                    return Ok(known.id);
                }
                let made = s.run("account.add", &new_account)?;
                made.get("id").and_then(Value::as_u64).ok_or(CoreError::Internal)
            })
            .await;
        match added {
            Ok(account) => Ok(SignInOutcome::SignedIn { account, email: address, name }),
            Err(error) => {
                // (Nothing is kept of a sign-in that made no account, unless the account was there;
                // not through the session, which may be gone.)
                if !existed {
                    signin::forget(self.native.as_ref(), &address);
                }
                match error {
                    CoreError::Cancelled => Err(CoreError::Cancelled),
                    CoreError::Failed { reason } | CoreError::BadRequest { reason } => Ok(SignInOutcome::Failed { reason }),
                    _ => Ok(failed("the account could not be added")),
                }
            }
        }
    }

    /// Ask the owner, on Microsoft's page, for `permission` for `account`, by itself and never
    /// with a sign-in. Ask the owner first in the app: this opens the page. The new sign-in
    /// replaces the old in the credential store only if it is the account's own.
    pub async fn grant_permission(&self, account: u64, permission: Permission) -> Result<PermissionOutcome, CoreError> {
        let email = self.email_of(account).await?;
        let Some(place) = self.signins.begin() else { return Ok(not_granted("a sign-in is already open")) };
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
        let (platform, tokens_refresh, for_email) = (self.native.clone(), tokens.refresh.clone(), email.clone());
        let kept = self
            .call(move |s| {
                let there = s.mailbox.accounts.iter().any(|a| a.id == account && a.kind == AccountKind::Microsoft);
                Ok(if there { Some(signin::keep_refresh(platform.as_ref(), &for_email, &tokens_refresh)) } else { None })
            })
            .await?;
        match kept {
            Some(Ok(())) => Ok(PermissionOutcome::Granted),
            Some(Err(why)) => Ok(not_granted(why)),
            None => Ok(not_granted("there is no such account")),
        }
    }

    /// Whether `account`'s sign-in covers `permission`, asked of Microsoft with the stored refresh
    /// token (nothing is opened and the owner is not asked). Nothing is stored of the answer.
    pub async fn permission_state(&self, account: u64, permission: Permission) -> Result<PermissionState, CoreError> {
        let email = self.email_of(account).await?;
        let Some(o) = signin::client_oauth() else { return Ok(PermissionState::Unknown) };
        let mail = self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail));
        let graph = self.signins.at(Endpoints::for_resource(&o, Resource::Graph));
        let reply = Queue::<Standing>::new();
        let job_reply = reply.clone();
        let platform = self.native.clone();
        let renewed_for = email.clone();
        let job = move || signin::permission_standing(platform.as_ref(), &email, permission.into(), &mail, &graph);
        let spawned = std::thread::Builder::new().name("mailcraft-permission".to_string()).spawn(move || {
            let state = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(Standing::Unknown);
            job_reply.push(state);
        });
        if spawned.is_err() {
            return Ok(PermissionState::Unknown);
        }
        let standing = reply.next().await.unwrap_or(Standing::Unknown);
        // The answer may have renewed the stored sign-in of an account removed meanwhile: it goes
        // again, unless another account has the same address (a second one, or the same one added
        // again under a new id), whose sign-in it is. One job, on the session's thread, so nothing
        // is added or removed between the look and the deletion.
        let platform = self.native.clone();
        let _ = self
            .call(move |s| {
                let gone = !s.mailbox.accounts.iter().any(|a| a.id == account);
                let shared = s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&renewed_for));
                if gone && !shared {
                    signin::forget(platform.as_ref(), &renewed_for);
                }
                Ok(())
            })
            .await;
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
            if !still {
                signin::forget(platform.as_ref(), &email);
            }
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl Core {
    /// Whether the mailbox has a Microsoft account at `email`.
    async fn has_microsoft_account(&self, email: &str) -> Result<bool, CoreError> {
        let email = email.to_string();
        self.call(move |s| Ok(s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&email))))
            .await
    }
}
