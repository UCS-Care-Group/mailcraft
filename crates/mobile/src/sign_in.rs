// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing an account in on the provider's page, from a phone: the core opens the page through
//! the platform ([`crate::Platform::open_sign_in_page`]), the system brings the app back through
//! the program's own scheme, and the app hands the address it was opened with to
//! [`crate::Core::sign_in_redirect`]. Everything else (PKCE, the state, the trade of the code) is
//! `mailcraft_net::oauth`'s, which reads the address strictly: the address, the code and the state
//! never appear in an error or a log line here either.
//!
//! The tokens that come back go to the sync host ([`crate::sync`]): refused when they are somebody
//! else's, kept for the session, and remembered in the credential store when the owner lets it be.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_net::NetError;
use mailcraft_net::oauth::{CustomScheme, DEFAULT_REDIRECT_PATH, Endpoints, RedirectStrategy, Resource, SignInEnd};

/// The longest address that is taken (the reader refuses a longer one; none is kept).
const MOST_ADDRESS: usize = 8 * 1024;
/// How long a wait for the address goes before the flag is looked at again.
const SLICE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Slot {
    armed: bool,
    /// The number of the attempt that armed the slot, and the last one handed out.
    attempt: u64,
    address: Option<String>,
    /// The core was shut down: every wait ends, and none begins again.
    closed: bool,
}

/// Where the address the system opened the app with lands, while a sign-in waits for it.
#[derive(Default)]
pub(crate) struct Redirects {
    slot: Mutex<Slot>,
    cv: Condvar,
}

impl Redirects {
    /// A sign-in is about to open its page: forget what is left and listen.
    /// Gives the number of this attempt, which alone may [`disarm`](Self::disarm) it.
    pub(crate) fn arm(&self) -> u64 {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let attempt = slot.attempt.wrapping_add(1);
        *slot = Slot { armed: true, attempt, address: None, closed: slot.closed };
        attempt
    }

    /// The core was shut down: a sign-in that waits is called off.
    pub(crate) fn close(&self) {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        self.cv.notify_all();
    }

    /// Stop listening, when `attempt` is the one that armed the slot (a later sign-in's stays).
    pub(crate) fn disarm(&self, attempt: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.attempt == attempt {
            *slot = Slot { closed: slot.closed, attempt, ..Slot::default() };
        }
    }

    /// The app hands over the address it was opened with. Taken only while a sign-in waits, and
    /// only the first; another program may name our scheme, so anything else is dropped.
    pub(crate) fn deliver(&self, address: &str) -> bool {
        if address.len() > MOST_ADDRESS {
            return false;
        }
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if !slot.armed || slot.address.is_some() {
            return false;
        }
        slot.address = Some(address.to_string());
        self.cv.notify_all();
        true
    }

    /// Wait for the address, no longer than `patience` and not past `cancel` being set.
    pub(crate) fn wait(&self, cancel: &AtomicBool, patience: Duration) -> Result<String, NetError> {
        let deadline = Instant::now().checked_add(patience);
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(address) = slot.address.take() {
                return Ok(address);
            }
            if cancel.load(Ordering::Acquire) || slot.closed {
                return Err(NetError::SignInCancelled);
            }
            let left = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => SLICE,
            };
            if left.is_zero() {
                return Err(NetError::SignInTimedOut);
            }
            let (next, _) = self.cv.wait_timeout(slot, left.min(SLICE)).unwrap_or_else(PoisonError::into_inner);
            slot = next;
        }
    }
}

/// How a sign-in ended without tokens, for the owner. Never what the provider said.
pub(crate) enum Ended {
    Cancelled,
    Said(&'static str),
}

/// Run the sign-in on the page, on this thread: `open` shows the page, `redirects` waits for the
/// address, `cancel` calls it off.
pub(crate) fn run(
    oauth: &mailcraft_model::OAuth,
    address: &str,
    scheme: &str,
    open: &dyn Fn(&str),
    redirects: &Arc<Redirects>,
    cancel: &AtomicBool,
    endpoints: Option<&Endpoints>,
) -> Result<mailcraft_model::sync::Tokens, Ended> {
    let waiting = redirects.clone();
    let handback = move |cancel: &AtomicBool, patience: Duration| waiting.wait(cancel, patience);
    let Ok(custom) = CustomScheme::new(scheme, DEFAULT_REDIRECT_PATH, &handback) else {
        return Err(Ended::Said(words("Sign-in isn't set up on this phone.")));
    };
    let via = RedirectStrategy::CustomScheme(custom);
    let own = Endpoints::for_resource(oauth, Resource::GraphMail);
    let endpoints = endpoints.unwrap_or(&own);
    mailcraft_net::oauth::sign_in_explained_via(endpoints, oauth, address, open, cancel, &via).map_err(|end| match end {
        SignInEnd::Approval => Ended::Said(words("Your organisation has to approve MailCraft before you can sign in. Ask an administrator.")),
        SignInEnd::Declined => Ended::Said(words("The sign-in was declined.")),
        SignInEnd::TimedOut => Ended::Said(words("Nothing came back from the sign-in page in time.")),
        SignInEnd::Cancelled => Ended::Cancelled,
        // (Never what the provider said: it may echo what was sent.)
        SignInEnd::Other(_) => Ended::Said(words("The sign-in didn't work. Try again.")),
    })
}

/// Marks a sentence for the catalogue (`cargo xtask i18n` gathers what is passed to it).
fn words(text: &'static str) -> &'static str {
    text
}
