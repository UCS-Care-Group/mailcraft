// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in on the provider's page, as a front end that is handed back through its own scheme
//! does it (a phone): opening the page through the platform, waiting for the address the system
//! opened the program with, trading it for tokens, keeping the refresh token in the platform's
//! credential store, and asking what a stored sign-in covers. The secrets rules and the consent
//! rules live here once, so that no front end carries a copy of them.
//!
//! Everything about the address that comes back is judged by the OAuth strategy of `mailcraft-net`
//! (`oauth/scheme.rs`): the scheme and path to the letter, the state, the fields, the sizes. The
//! tokens never leave this module but through the credential store: the refresh token is kept
//! there and nowhere else, the access token is dropped, and neither appears in an error or a log
//! line.
//!
//! The functions here block; the front end runs them on a thread of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_model::sync::Tokens;
use mailcraft_model::{OAuth, OAuthProvider};
use mailcraft_net::oauth::{
    CustomScheme, DEFAULT_REDIRECT_HOST, Handback, RedirectStrategy, refresh_for_at, refresh_graph_shared_at, refresh_graph_shared_send_at,
    refresh_graph_tasks_at, sign_in_ending_via, sign_in_for_permission_at,
};

// What a front end needs to name the page it asks for, without a dependency on the network crate of its own.
pub use mailcraft_net::NetError;
pub use mailcraft_net::oauth::{Ending, Endpoints, FavoritesRefusal, Permission, Resource, admin_consent_url, permission_endpoints};

use crate::keys::microsoft_secret_key;
use crate::{Platform, Redirect, SecretStore};

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

/// The registration this build signs in with, when it has one.
pub fn client_oauth() -> Option<OAuth> {
    (!CLIENT_ID.trim().is_empty()).then(|| OAuth {
        provider: OAuthProvider::Microsoft,
        client_id: CLIENT_ID.trim().to_string(),
        ..Default::default()
    })
}

/// An address fit to be an account's: one `@`, no white space or control characters, within the
/// length an address has.
pub fn sensible_email(text: &str) -> bool {
    let shaped = text.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    shaped && text.len() <= MAX_EMAIL && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The hint the page is given: what the owner typed if it can be an address, else nothing.
pub fn hint_of(typed: &str) -> String {
    let typed = typed.trim();
    if sensible_email(typed) { typed.to_string() } else { String::new() }
}

/// The domain of `email` as the organisation's directory to name on the page for an
/// administrator, when it can be one; empty (Microsoft's own "whichever the administrator who
/// signs in is of") otherwise.
pub fn tenant_of(email: &str) -> String {
    let domain = email.trim().rsplit_once('@').map_or("", |(_, domain)| domain);
    let fit = !domain.is_empty() && domain.len() <= 100 && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if fit { domain.to_ascii_lowercase() } else { String::new() }
}

/// What the sign-ins of a program hold: where the redirect lands, and the one that is under way.
/// Only an armed sign-in takes an address, and only the first: another program may name our
/// scheme, and nothing is believed of what it sends (the state decides that, in the OAuth
/// strategy).
#[derive(Default)]
pub struct SignIns {
    slot: Mutex<Option<Sender<String>>>,
    /// The cancel flag of the sign-in that is open; one at a time.
    open: Mutex<Option<Arc<AtomicBool>>>,
}

/// Holds the one sign-in's place, and gives it up (and the redirect's slot) when dropped.
pub struct Place<'a> {
    owner: &'a SignIns,
    cancel: Arc<AtomicBool>,
}

impl Place<'_> {
    /// The flag that calls this sign-in off.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        // (Set too when the future waiting for the sign-in is dropped: the thread then stops.)
        self.cancel.store(true, Ordering::Release);
        self.owner.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        self.owner.open.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

impl SignIns {
    /// Take the place of the one sign-in; none when one is open.
    pub fn begin(&self) -> Option<Place<'_>> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.is_some() {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *open = Some(cancel.clone());
        Some(Place { owner: self, cancel })
    }

    /// Call off the sign-in that is open, if one is.
    pub fn cancel(&self) {
        if let Some(flag) = self.open.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            flag.store(true, Ordering::Release);
        }
    }

    /// Arm the redirect's slot: the receiver gets the first address delivered.
    pub fn arm(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }

    /// Whether the address was taken by a sign-in that waits for it.
    pub fn deliver(&self, address: String) -> bool {
        if address.len() > MAX_ADDRESS {
            return false;
        }
        // (Taken: a second address, or one when nobody waits, finds nothing to go to.)
        let taken = self.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        taken.is_some_and(|tx| tx.send(address).is_ok())
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

/// What the page is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Adding an account: a sign-in with what mail takes.
    Account,
    /// A permission that is asked for by itself.
    Permission,
}

/// What a sign-in on the page came to, before the account is dealt with.
pub enum Page {
    Tokens(Tokens),
    Ended(Ending),
    Refused(FavoritesRefusal),
    NoBrowser,
    /// Could not start: in the program's words.
    Unable(&'static str),
}

/// The scheme the platform says it is handed back through, as a registered redirect.
pub fn redirect_scheme(platform: &dyn Platform) -> Option<String> {
    match platform.redirect() {
        Redirect::CustomScheme(scheme) => Some(scheme),
        Redirect::Loopback => None,
    }
}

/// Run a sign-in on the page, waiting for the address on `landed` (taken: the wait holds it) as long as the owner takes
/// (five minutes at most) and not past `cancel`. Blocks.
pub fn page(platform: &dyn Platform, landed: Receiver<String>, endpoints: &Endpoints, hint: &str, why: Why, cancel: &AtomicBool) -> Page {
    let Some(oauth) = client_oauth() else { return Page::Unable("this build of MailCraft has no Microsoft sign-in set up") };
    let Some(scheme) = redirect_scheme(platform) else { return Page::Unable("this phone can't take a sign-in back through the app's own address") };
    let no_browser = AtomicBool::new(false);
    // (No page opened means nothing will come back: the wait is over.)
    let open = |url: &str| {
        if platform.open_sign_in_page(url).is_err() {
            no_browser.store(true, Ordering::Release);
            cancel.store(true, Ordering::Release);
        }
    };
    let handback = move |flag: &AtomicBool, patience: Duration| wait_for(&landed, flag, patience);
    let handback: &Handback = &handback;
    let via = match CustomScheme::with_host_only(&scheme, DEFAULT_REDIRECT_HOST, handback) {
        Ok(custom) => RedirectStrategy::CustomScheme(custom),
        Err(_) => return Page::Unable("this app's sign-in address is not one Microsoft's page can come back to"),
    };
    match why {
        Why::Account => match sign_in_ending_via(endpoints, &oauth, hint, &open, cancel, &via) {
            Ok(tokens) => Page::Tokens(tokens),
            Err(_) if no_browser.load(Ordering::Acquire) => Page::NoBrowser,
            Err(end) => Page::Ended(end),
        },
        Why::Permission => match sign_in_for_permission_at(endpoints, &oauth, hint, &open, cancel, &via) {
            Ok(tokens) => Page::Tokens(tokens),
            Err(_) if no_browser.load(Ordering::Acquire) => Page::NoBrowser,
            Err(refusal) => Page::Refused(refusal),
        },
    }
}

/// Keep `refresh` as the account's sign-in in the platform's credential store.
pub fn keep_refresh(platform: &dyn Platform, email: &str, refresh: &str) -> Result<(), &'static str> {
    if refresh.is_empty() {
        return Err("Microsoft didn't let MailCraft stay signed in to this account");
    }
    let Some(store) = platform.secrets() else { return Err("this phone has no secure store to keep your sign-in in") };
    store.set(&microsoft_secret_key("refresh", email), refresh).map_err(|_| "MailCraft couldn't keep your sign-in in this phone's secure store")
}

/// Take back every sign-in kept for the account at `email`.
pub fn forget(platform: &dyn Platform, email: &str) {
    if let Some(store) = platform.secrets() {
        for kind in ["refresh", "password"] {
            store.delete(&microsoft_secret_key(kind, email));
        }
    }
}

/// Whether an account's sign-in covers a permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Allowed,
    /// Not given yet: the program can ask the owner.
    NotAllowed,
    /// The account's sign-in is over altogether: the owner signs in again first.
    SignInNeeded,
    /// Microsoft couldn't be asked (no network, a server's "not now").
    Unknown,
}

/// Keep the refresh token Microsoft sent with an answer, when it is the account's own: the old
/// one is replaced by the one Microsoft rotated it to, so a stored sign-in doesn't age out.
fn renewed(store: &dyn SecretStore, email: &str, tokens: &Tokens) {
    if !tokens.refresh.is_empty() && !tokens.someone_else(email, "") {
        // (A store that refuses leaves the old token, which is good until Microsoft says otherwise.)
        let _ = store.set(&microsoft_secret_key("refresh", email), &tokens.refresh);
    }
}

/// Whether the sign-in kept for `email` covers `permission`, asked of Microsoft with the stored
/// refresh token (nothing is opened and the owner is not asked). `mail` and `graph` are the
/// token endpoints for mail and for Graph. A new refresh token in an answer is kept. Blocks.
pub fn permission_standing(platform: &dyn Platform, email: &str, permission: Permission, mail: &Endpoints, graph: &Endpoints) -> Standing {
    let Some(oauth) = client_oauth() else { return Standing::Unknown };
    let Some(store) = platform.secrets() else { return Standing::SignInNeeded };
    let Some(refresh) = store.get(&microsoft_secret_key("refresh", email)) else { return Standing::SignInNeeded };
    let asked = match permission {
        Permission::Tasks => refresh_graph_tasks_at(graph, (mail, Resource::GraphMail), &oauth, &refresh).map(|g| g.tokens),
        Permission::SharedMail => refresh_graph_shared_at(mail, &oauth, &refresh).map(|g| g.tokens),
        Permission::SharedSend => refresh_graph_shared_send_at(mail, &oauth, &refresh).map(|g| g.tokens),
    };
    match asked {
        Ok(tokens) => {
            renewed(store.as_ref(), email, &tokens);
            Standing::Allowed
        }
        Err(e) if e.is_consent_needed() => Standing::NotAllowed,
        // "No longer good": for this permission only, or for the whole sign-in?
        Err(e) if e.is_refused_sign_in() => match refresh_for_at(mail, &oauth, &refresh, Resource::GraphMail) {
            Ok(tokens) => {
                renewed(store.as_ref(), email, &tokens);
                Standing::NotAllowed
            }
            Err(e) if e.is_refused_sign_in() => Standing::SignInNeeded,
            Err(_) => Standing::Unknown,
        },
        Err(_) => Standing::Unknown,
    }
}

/// The own address of the account a sign-in is for, when Microsoft names one that can be an
/// account's. What the owner typed is only a hint for the page and is never taken for who signed in.
pub fn signed_in_address(tokens: &Tokens) -> Option<String> {
    let address = tokens.email.trim();
    sensible_email(address).then(|| address.to_string())
}
