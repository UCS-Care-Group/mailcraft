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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_model::sync::Tokens;
use mailcraft_model::{AccountKind, Directory, OAuth, OAuthProvider};
use mailcraft_net::NetError;
use mailcraft_net::oauth::{
    CustomScheme, DEFAULT_REDIRECT_PATH, Ending, Endpoints, FavoritesRefusal, Handback, Permission as NetPermission, RedirectStrategy, Resource,
    admin_consent_url, permission_endpoints, refresh_for_at, refresh_graph_shared_at, refresh_graph_shared_send_at, refresh_graph_tasks_at,
    sign_in_ending_via, sign_in_for_permission_at,
};
use serde_json::{Value, json};

use crate::platform::RedirectKind;
use crate::queue::{Queue, Shed, Weight};
use crate::{Core, CoreError};

/// The application (client) id this build presents to Microsoft's page: the project's own
/// registration, an identifier and not a secret (`MAILCRAFT_MS_CLIENT_ID` in `.cargo/config.toml`).
const CLIENT_ID: &str = match option_env!("MAILCRAFT_MS_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// The longest address handed back that is looked at; the OAuth strategy refuses a longer one.
const MAX_ADDRESS: usize = 8 * 1024;
/// The longest e-mail address (RFC 5321) taken as a hint.
const MAX_EMAIL: usize = 254;
/// How often the wait for the address looks at the cancel flag.
const TICK: Duration = Duration::from_millis(50);

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

/// Where the address the system opened the app with is held until the sign-in that waits for it
/// takes it. Only an armed sign-in takes one, and only the first: another program may name our
/// scheme, and nothing is believed of what it sends (the state decides that, in the OAuth
/// strategy).
#[derive(Default)]
pub(crate) struct Landing {
    slot: Mutex<Option<Sender<String>>>,
}

impl Landing {
    fn arm(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }

    fn disarm(&self) {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    }

    /// Whether the address was taken by a sign-in that waits for it.
    pub(crate) fn deliver(&self, address: String) -> bool {
        if address.len() > MAX_ADDRESS {
            return false;
        }
        // (Taken: a second address, or one when nobody waits, finds nothing to go to.)
        let taken = self.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        taken.is_some_and(|tx| tx.send(address).is_ok())
    }
}

/// What the sign-ins of a core hold: where the redirect lands, and the one that is under way.
#[derive(Default)]
pub(crate) struct SignIns {
    landing: Landing,
    /// The cancel flag of the sign-in that is open; one at a time.
    open: Mutex<Option<Arc<AtomicBool>>>,
    /// A token server and a page of the tests' own, for a sign-in at them (never in a build that
    /// isn't a test's: nothing sets it).
    #[cfg(test)]
    pub(crate) server: Mutex<Option<(String, String)>>,
}

/// Holds the one sign-in's place, and gives it up (and the redirect's slot) when dropped.
pub(crate) struct Place<'a> {
    owner: &'a SignIns,
    cancel: Arc<AtomicBool>,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        // (Set too when the future waiting for the sign-in is dropped: the thread then stops.)
        self.cancel.store(true, Ordering::Release);
        self.owner.landing.disarm();
        self.owner.open.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

impl SignIns {
    pub(crate) fn begin(&self) -> Option<Place<'_>> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.is_some() {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *open = Some(cancel.clone());
        Some(Place { owner: self, cancel })
    }

    fn cancel(&self) {
        if let Some(flag) = self.open.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            flag.store(true, Ordering::Release);
        }
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

/// Waits for the address, no longer than `patience` and not past `cancel`.
fn wait_for(rx: &Receiver<String>, cancel: &AtomicBool, patience: Duration) -> Result<String, NetError> {
    let end = Instant::now() + patience;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(NetError::SignInCancelled);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(NetError::SignInTimedOut);
        }
        match rx.recv_timeout(left.min(TICK)) {
            Ok(address) => return Ok(address),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(NetError::SignInCancelled),
        }
    }
}

/// What a sign-in on the page came to, before the account is dealt with.
enum Page {
    Tokens(Tokens),
    Ended(Ending),
    Refused(FavoritesRefusal),
    NoBrowser,
    /// Could not start: in the program's words.
    Unable(&'static str),
}

/// A reply to one call: the queue holds one, never reaches its bound.
impl Shed for Page {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

impl Shed for PermissionState {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

/// What the page is for.
#[derive(Clone, Copy)]
enum Why {
    Account,
    Permission,
}

fn oauth() -> Option<OAuth> {
    (!CLIENT_ID.trim().is_empty()).then(|| OAuth {
        provider: OAuthProvider::Microsoft,
        client_id: CLIENT_ID.trim().to_string(),
        ..Default::default()
    })
}

fn secret_key(kind: &str, email: &str) -> String {
    format!("{kind}:{}|graph", email.trim().to_ascii_lowercase())
}

/// An address fit to be an account's: one `@`, no white space or control characters, within the
/// length an address has.
fn sensible_email(text: &str) -> bool {
    let shaped = text.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    shaped && text.len() <= MAX_EMAIL && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The hint the page is given: what the owner typed if it can be an address, else nothing.
fn hint_of(typed: &str) -> String {
    let typed = typed.trim();
    if sensible_email(typed) { typed.to_string() } else { String::new() }
}

fn failed(reason: &str) -> SignInOutcome {
    SignInOutcome::Failed { reason: reason.to_string() }
}

fn not_granted(reason: &str) -> PermissionOutcome {
    PermissionOutcome::Failed { reason: reason.to_string() }
}

impl Core {
    /// The scheme the platform says it is handed back through, as a registered redirect.
    fn redirect_scheme(&self) -> Option<String> {
        match self.platform.redirect() {
            Ok(RedirectKind::CustomScheme { scheme }) => Some(scheme),
            Ok(RedirectKind::Loopback) | Err(_) => None,
        }
    }

    /// Run a sign-in on the page on a thread of its own, which waits as long as the owner takes
    /// (five minutes at most), and give what it came to.
    async fn page(&self, scheme: String, endpoints: Endpoints, hint: String, why: Why, cancel: Arc<AtomicBool>) -> Page {
        let Some(oauth) = oauth() else { return Page::Unable("this build of MailCraft has no Microsoft sign-in set up") };
        let rx = self.signins.landing.arm();
        let reply = Queue::<Page>::new();
        let platform = self.platform.clone();
        let (job_reply, job_cancel) = (reply.clone(), cancel.clone());
        let job = move || {
            let no_browser = AtomicBool::new(false);
            let open = |url: &str| {
                if platform.open_sign_in_page(url.to_string()).is_err() {
                    no_browser.store(true, Ordering::Release);
                    job_cancel.store(true, Ordering::Release);
                }
            };
            let handback = move |flag: &AtomicBool, patience: Duration| wait_for(&rx, flag, patience);
            let handback: &Handback = &handback;
            let via = match CustomScheme::new(&scheme, DEFAULT_REDIRECT_PATH, handback) {
                Ok(custom) => RedirectStrategy::CustomScheme(custom),
                Err(_) => return Page::Unable("this app's sign-in address is not one Microsoft's page can come back to"),
            };
            match &why {
                Why::Account => match sign_in_ending_via(&endpoints, &oauth, &hint, &open, &job_cancel, &via) {
                    Ok(tokens) => Page::Tokens(tokens),
                    Err(_) if no_browser.load(Ordering::Acquire) => Page::NoBrowser,
                    Err(end) => Page::Ended(end),
                },
                Why::Permission => match sign_in_for_permission_at(&endpoints, &oauth, &hint, &open, &job_cancel, &via) {
                    Ok(tokens) => Page::Tokens(tokens),
                    Err(_) if no_browser.load(Ordering::Acquire) => Page::NoBrowser,
                    Err(refusal) => Page::Refused(refusal),
                },
            }
        };
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

    /// Keep `refresh` as the account's sign-in in the platform's credential store.
    fn keep_refresh(&self, email: &str, refresh: &str) -> Result<(), &'static str> {
        if refresh.is_empty() {
            return Err("Microsoft didn't let MailCraft stay signed in to this account");
        }
        let store = match self.platform.secrets() {
            Ok(Some(store)) => store,
            _ => return Err("this phone has no secure store to keep your sign-in in"),
        };
        store.set(secret_key("refresh", email), refresh.to_string()).map_err(|_| "MailCraft couldn't keep your sign-in in this phone's secure store")
    }

    fn forget(&self, email: &str) {
        if let Ok(Some(store)) = self.platform.secrets() {
            for kind in ["refresh", "password"] {
                let _ = store.delete(secret_key(kind, email));
            }
        }
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

/// The domain of `email` as the organisation's directory to name on the page for an
/// administrator, when it can be one; empty (Microsoft's own "whichever the administrator who
/// signs in is of") otherwise.
fn tenant_of(email: &str) -> String {
    let domain = email.trim().rsplit_once('@').map_or("", |(_, domain)| domain);
    let fit = !domain.is_empty() && domain.len() <= 100 && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if fit { domain.to_ascii_lowercase() } else { String::new() }
}

#[uniffi::export]
impl Core {
    /// Give the address the system opened the app with (the redirect's) to the sign-in that waits
    /// for it, whole and unread. True when a sign-in took it; false when none waits, one already
    /// had an address, or it is too long. Anything else about it is judged in Rust.
    pub fn hand_back_redirect(&self, address: String) -> bool {
        self.signins.landing.deliver(address)
    }

    /// Call off the sign-in that is open, if one is: it ends as `Cancelled`.
    pub fn cancel_sign_in(&self) {
        self.signins.cancel();
    }

    /// Add a Microsoft account: the provider's page opens in the system browser through the
    /// platform, and what comes back is checked here. `email` is only a hint for the page and
    /// may be empty. On success the account is in the mailbox and its sign-in is in the
    /// platform's credential store; nothing is kept if the store refuses.
    pub async fn sign_in_microsoft(&self, email: String) -> Result<SignInOutcome, CoreError> {
        let Some(place) = self.signins.begin() else { return Ok(failed("a sign-in is already open")) };
        let Some(scheme) = self.redirect_scheme() else { return Ok(failed("this phone can't take a sign-in back through the app's own address")) };
        let endpoints = match oauth() {
            Some(o) => self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail)),
            None => return Ok(failed("this build of MailCraft has no Microsoft sign-in set up")),
        };
        let hint = hint_of(&email);
        let page = self.page(scheme, endpoints, hint.clone(), Why::Account, place.cancel.clone()).await;
        let tokens = match page {
            Page::Tokens(tokens) => tokens,
            Page::Ended(end) => return Ok(ending_outcome(end)),
            Page::NoBrowser => return Ok(SignInOutcome::NoBrowser),
            Page::Unable(why) => return Ok(failed(why)),
            Page::Refused(_) => return Ok(failed("the sign-in stopped unexpectedly")),
        };
        // Who signed in is who Microsoft says (the owner picks the account on its page).
        let address = if tokens.email.is_empty() { hint } else { tokens.email.clone() };
        if !sensible_email(&address) {
            return Ok(failed("Microsoft didn't say which account signed in"));
        }
        if let Err(why) = self.keep_refresh(&address, &tokens.refresh) {
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
                // (Nothing is kept of a sign-in that made no account, unless the account was there.)
                if !matches!(error, CoreError::Cancelled) {
                    self.forget_unless_account(&address).await;
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
        let Some(scheme) = self.redirect_scheme() else {
            return Ok(not_granted("this phone can't take a sign-in back through the app's own address"));
        };
        let endpoints = match oauth() {
            Some(o) => self.signins.at(permission_endpoints(&o, permission.into())),
            None => return Ok(not_granted("this build of MailCraft has no Microsoft sign-in set up")),
        };
        let page = self.page(scheme, endpoints, email.clone(), Why::Permission, place.cancel.clone()).await;
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
        match self.keep_refresh(&email, &tokens.refresh) {
            Ok(()) => Ok(PermissionOutcome::Granted),
            Err(why) => Ok(not_granted(why)),
        }
    }

    /// Whether `account`'s sign-in covers `permission`, asked of Microsoft with the stored refresh
    /// token (nothing is opened and the owner is not asked). Nothing is stored of the answer.
    pub async fn permission_state(&self, account: u64, permission: Permission) -> Result<PermissionState, CoreError> {
        let email = self.email_of(account).await?;
        let Some(o) = oauth() else { return Ok(PermissionState::Unknown) };
        let refresh = match self.platform.secrets() {
            Ok(Some(store)) => store.get(secret_key("refresh", &email)).ok().flatten(),
            _ => None,
        };
        let Some(refresh) = refresh else { return Ok(PermissionState::SignInNeeded) };
        let mail = self.signins.at(Endpoints::for_resource(&o, Resource::GraphMail));
        let graph = self.signins.at(Endpoints::for_resource(&o, Resource::Graph));
        let reply = Queue::<PermissionState>::new();
        let job_reply = reply.clone();
        let job = move || {
            let asked = match permission {
                Permission::Tasks => refresh_graph_tasks_at(&graph, (&mail, Resource::GraphMail), &o, &refresh).map(|_| ()),
                Permission::SharedMail => refresh_graph_shared_at(&mail, &o, &refresh).map(|_| ()),
                Permission::SharedSend => refresh_graph_shared_send_at(&mail, &o, &refresh).map(|_| ()),
            };
            match asked {
                Ok(()) => PermissionState::Allowed,
                Err(e) if e.is_consent_needed() => PermissionState::NotAllowed,
                // "No longer good": for this permission only, or for the whole sign-in?
                Err(e) if e.is_refused_sign_in() => match refresh_for_at(&mail, &o, &refresh, Resource::GraphMail) {
                    Ok(_) => PermissionState::NotAllowed,
                    Err(e) if e.is_refused_sign_in() => PermissionState::SignInNeeded,
                    Err(_) => PermissionState::Unknown,
                },
                Err(_) => PermissionState::Unknown,
            }
        };
        let spawned = std::thread::Builder::new().name("mailcraft-permission".to_string()).spawn(move || {
            let state = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(PermissionState::Unknown);
            job_reply.push(state);
        });
        if spawned.is_err() {
            return Ok(PermissionState::Unknown);
        }
        Ok(reply.next().await.unwrap_or(PermissionState::Unknown))
    }

    /// The address of Microsoft's page where an administrator approves MailCraft for an
    /// organisation, once for everybody in it, for the owner to pass on. Nothing is opened or
    /// sent. `email` names the organisation's directory by its domain when it can; it may be empty.
    /// The address names the application, the directory and the permissions, and nothing of the owner.
    pub fn admin_approval_link(&self, email: String) -> Result<String, CoreError> {
        let o = oauth().ok_or_else(|| CoreError::Failed { reason: "this build of MailCraft has no Microsoft sign-in set up".to_string() })?;
        admin_consent_url(&o, &tenant_of(&email))
            .map_err(|_| CoreError::Failed { reason: "the address of the approval page could not be made".to_string() })
    }

    /// Remove an account: its mail and what is kept for it on this phone (`account.remove`), and
    /// its sign-in from the platform's credential store. Nothing on the server is touched. The
    /// sign-in is removed only once the account is.
    pub async fn remove_account(&self, account: u64) -> Result<(), CoreError> {
        let email = self.email_of(account).await?;
        self.call(move |s| s.run("account.remove", &json!({"id": account})).map(|_| ()).map_err(CoreError::from)).await?;
        // (Nothing else of the account's is held under its address: a mailbox with another account
        // of the same address keeps it.)
        let still = self
            .call({
                let email = email.clone();
                move |s| Ok(s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&email)))
            })
            .await?;
        if !still {
            self.forget(&email);
        }
        Ok(())
    }
}

impl Core {
    /// Take back what [`Core::sign_in_microsoft`] kept when no account came of it, unless an
    /// account of that address exists (its sign-in was just renewed, and stays).
    async fn forget_unless_account(&self, email: &str) {
        let email_for_call = email.to_string();
        let exists = self
            .call(move |s| {
                Ok(s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft && a.email.trim().eq_ignore_ascii_case(&email_for_call)))
            })
            .await
            .unwrap_or(false);
        if !exists {
            self.forget(email);
        }
    }
}
