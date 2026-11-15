// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Subscribed calendars in the window (#209, Outlook's Internet Calendars): Calendar › Open
//! Calendar › From Internet… and File › Info › Subscribed calendars.
//!
//! The engine makes the calendar and takes in what a look at the web brought; this is the part in
//! between that has the address. The address of a subscribed calendar may hold a secret (the
//! private addresses calendar services hand out have a token in the path), so it is kept as a
//! password is: in the system's credential store when the owner lets MailCraft remember sign-ins
//! (`MailApp::secret_cache` has what the background read of the store found), and in memory for
//! the session otherwise. With neither, the calendar stays listed, with what it held, and File ›
//! Info asks for its address again. It is never in the mailbox file, `ui.json`, a log line or a
//! command's answer.
//!
//! Nothing is fetched until the owner has subscribed (the dialog's Subscribe). Then every
//! subscribed calendar is looked at when it is due: every hour by default, on Send/Receive
//! (`ui.sync`) and from Update Now, one at a time, on a thread of the host's
//! ([`Services::subscription`]); the engine says which are due (`sync.subscriptionRequest`) and
//! takes the answer in (`sync.applySubscription`).

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailcraft_model::Id;
use mailcraft_model::subscription::{Failure, SubscriptionAsk, SubscriptionOutcome, try_https};
use serde_json::{Value, json};

use crate::{MailApp, now_ms};

/// How often the engine is asked which calendars are due (milliseconds); a look itself is as
/// often as the owner chose for the calendar, an hour unless said.
const ASK_EVERY: f64 = 20_000.0;

/// One calendar for the host to look at. The address is the owner's and may hold a secret: it is
/// not in the `Debug` output.
pub struct SubscriptionJob {
    pub calendar: Id,
    pub address: String,
    pub etag: String,
    pub last_modified: String,
}

impl std::fmt::Debug for SubscriptionJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SubscriptionJob({})", self.calendar)
    }
}

/// What the host brought for a job.
#[derive(Debug)]
pub struct SubscriptionDone {
    pub calendar: Id,
    pub outcome: SubscriptionOutcome,
}

/// What the window keeps of subscribed calendars.
#[derive(Default)]
pub(crate) struct Subscribed {
    /// The addresses of this session's subscriptions: what was typed, or typed again. (Those the
    /// credential store holds are in `MailApp::secret_cache`.)
    addresses: HashMap<Id, String>,
    /// The look in flight: the calendar, the mailbox it was for ([`mailcraft_engine::Session::openings`])
    /// and the answer to come.
    job: Option<(Id, u64, Receiver<SubscriptionDone>)>,
    /// When the engine is next asked what is due (milliseconds); 0 for now.
    next_ask: f64,
    /// The mailbox the addresses are of.
    opening: u64,
    /// The address being typed again for a calendar whose address wasn't kept (File › Info): the
    /// calendar, the text and what is wrong with it.
    pub(crate) again: Option<(Id, String, String)>,
}

impl Subscribed {
    /// The engine is asked again at the next frame, not in twenty seconds (tests).
    #[cfg(test)]
    pub(crate) fn reset_clock(&mut self) {
        self.next_ask = 0.0;
    }

    /// No address is held in memory (tests).
    #[cfg(test)]
    pub(crate) fn addresses_are_empty(&self) -> bool {
        self.addresses.is_empty()
    }
}

impl MailApp {
    /// The address of the subscribed calendar `id`, when this session has it: typed in, or
    /// remembered by the operating system and read from it.
    fn subscribed_address(&self, id: Id) -> Option<String> {
        if let Some(address) = self.subscribed.addresses.get(&id) {
            return Some(address.clone());
        }
        if !self.ui.remember_sign_in || self.unconfirmed {
            return None;
        }
        let name = self.session.mailbox.calendar(id)?.subscription.as_ref()?.secret_name(id);
        self.secret_cache.get(&name).cloned()
    }

    /// Move the addresses the store gave this session into memory (before the cache is cleared).
    pub(crate) fn keep_read_subscription_addresses(&mut self) {
        for c in &self.session.mailbox.calendars {
            let Some(name) = c.subscription.as_ref().map(|sub| sub.secret_name(c.id)) else { continue };
            if let Some(address) = self.secret_cache.get(&name) {
                self.subscribed.addresses.entry(c.id).or_insert_with(|| address.clone());
            }
        }
    }

    /// Whether the subscribed calendar `id` has to be given its address again (File › Info).
    pub(crate) fn subscription_needs_address(&self, id: Id) -> bool {
        self.subscribed_address(id).is_none()
    }

    /// Keep the address of calendar `id`: in memory for the session, and in the credential store
    /// when the owner lets sign-ins be remembered. Under a name that holds the host (the
    /// subscription's own), so that it is never given to another host.
    fn keep_subscription_address(&mut self, id: Id, address: &str) {
        let Some(name) = self.session.mailbox.calendar(id).and_then(|c| c.subscription.as_ref()).map(|sub| sub.secret_name(id)) else { return };
        self.subscribed.addresses.insert(id, address.to_string());
        if !self.ui.remember_sign_in || self.data_removed || self.unconfirmed {
            return;
        }
        let Some(store) = &self.services.secrets else { return };
        match store.set(&name, address) {
            Ok(()) => {
                self.secret_cache.insert(name.clone(), address.to_string());
                self.index_secret_key(name);
            }
            Err(_) => {
                // (A store's error can echo what it was given: nothing of it is shown or logged.)
                log::warn!("couldn't keep a calendar's address in the operating system's credential store");
                store.delete(&name);
                self.secret_cache.remove(&name);
            }
        }
    }

    /// Forget the address of a calendar that goes: memory, the store's entry, and its name in the
    /// settings.
    fn forget_subscription_address(&mut self, id: Id, name: &str) {
        self.subscribed.addresses.remove(&id);
        if self.subscribed.again.as_ref().is_some_and(|(again, ..)| *again == id) {
            self.subscribed.again = None;
        }
        if name.is_empty() {
            return;
        }
        self.secret_cache.remove(name);
        let Some(store) = &self.services.secrets else { return };
        store.delete(name);
        let gone = store.get(name).is_none();
        // (As `forget_account_secrets` does: a name whose delete failed stays listed, and the owner is told.)
        match store.take_trouble() {
            Some(trouble) => self.note_secret_trouble(trouble),
            None if gone => self.ui.remembered_keys.retain(|key| key != name),
            None => {}
        }
    }

    /// What the window does after a command of the subscribed calendars ran (`execute` calls this
    /// with the params it was given, which is where the address is).
    pub(crate) fn subscribed_after(&mut self, command: &str, params: &Value, result: &Result<Value, String>) {
        let Ok(done) = result else { return };
        match command {
            "calendar.subscribe" => {
                if let (Some(id), Some(address)) = (done.get("id").and_then(Value::as_u64), params.get("address").and_then(Value::as_str)) {
                    self.keep_subscription_address(id, address.trim());
                    // The first look is now, not in a while.
                    self.subscribed.next_ask = 0.0;
                }
            }
            "calendar.subscriptionSettings" => {
                if let (Some(id), Some(address)) = (done.get("id").and_then(Value::as_u64), params.get("address").and_then(Value::as_str)) {
                    self.keep_subscription_address(id, address.trim());
                    self.subscribed.again = None;
                    self.subscribed.next_ask = 0.0;
                }
            }
            "calendar.unsubscribe" => {
                if let (Some(id), Some(name)) = (done.get("unsubscribed").and_then(Value::as_u64), done.get("secret").and_then(Value::as_str)) {
                    self.forget_subscription_address(id, name);
                }
            }
            "calendar.refreshSubscribed" => self.subscribed.next_ask = 0.0,
            _ => {}
        }
    }

    /// Subscribe to the calendar at `address` (the dialog's Subscribe). Nothing is read until the
    /// calendar is there; the first look follows at once. The words are for the dialog.
    pub(crate) fn begin_subscribe(&mut self, address: &str, name: &str) -> Result<Id, String> {
        let mut params = json!({"address": address.trim()});
        if !name.trim().is_empty() {
            params["name"] = json!(name.trim());
        }
        let done = self.execute("calendar.subscribe", params)?;
        let id = done.get("id").and_then(Value::as_u64).ok_or_else(|| tl!("The calendar couldn't be added").to_string())?;
        self.status(crate::i18n::fmt(
            tl!("Subscribed to {name}. It is listed under Other Calendars, read-only, and brought up to date from the web every hour."),
            &[("name", done.get("name").and_then(Value::as_str).unwrap_or_default())],
        ));
        Ok(id)
    }

    /// The same address over `https://`, for the offer that goes with a refused `http://` one.
    pub(crate) fn https_offer(address: &str) -> Option<String> {
        try_https(address)
    }

    /// Once a frame: take in a look that came back, and start the next one that is due.
    pub(crate) fn subscribed_tick(&mut self) {
        // Another mailbox took the place of the open one: what was kept is the old one's.
        let opening = self.session.openings();
        if self.subscribed.opening != opening {
            self.subscribed = Subscribed { opening, ..Default::default() };
        }
        if let Some((calendar, for_opening, receive)) = &self.subscribed.job {
            let (calendar, for_opening) = (*calendar, *for_opening);
            let done = match receive.try_recv() {
                Ok(done) => Some(done),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                // The thread ended without an answer.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(SubscriptionDone { calendar, outcome: SubscriptionOutcome::Failed { failure: Failure::Network.code() } })
                }
            };
            self.subscribed.job = None;
            if let Some(done) = done.filter(|_| for_opening == opening) {
                self.take_in_subscription(done);
            }
            // The next one without waiting for the clock.
            self.subscribed.next_ask = 0.0;
            return;
        }
        let now = now_ms();
        if now < self.subscribed.next_ask || self.data_removed || self.unconfirmed || self.opening.is_some() {
            return;
        }
        self.subscribed.next_ask = now + ASK_EVERY;
        if self.offline() || self.services.subscription.is_none() {
            return;
        }
        let Ok(asked) = self.session.run("sync.subscriptionRequest", &json!({})) else { return };
        let requests: Vec<SubscriptionAsk> = asked.get("requests").and_then(|r| serde_json::from_value(r.clone()).ok()).unwrap_or_default();
        // The first that has its address; the rest are asked about again (those that haven't it
        // wait for the owner, in File › Info).
        let Some((ask, address)) = requests.into_iter().find_map(|ask| self.subscribed_address(ask.calendar).map(|address| (ask, address))) else {
            return;
        };
        let Some(start) = &self.services.subscription else { return };
        let receive = start(SubscriptionJob { calendar: ask.calendar, address, etag: ask.etag, last_modified: ask.last_modified });
        self.subscribed.job = Some((ask.calendar, opening, receive));
        // (Another look right after, if more are due.)
        self.subscribed.next_ask = now + 1_000.0;
    }

    /// Take in what a look brought, and tell the owner when a calendar stopped updating.
    fn take_in_subscription(&mut self, done: SubscriptionDone) {
        let was_failing =
            self.session.mailbox.calendar(done.calendar).and_then(|c| c.subscription.as_ref()).is_some_and(|sub| !sub.failure.is_empty());
        let params = json!({"calendar": done.calendar, "outcome": done.outcome});
        let applied = match self.session.run("sync.applySubscription", &params) {
            Ok(applied) => applied,
            Err(e) => {
                log::warn!("a subscribed calendar's look couldn't be taken in: {e}");
                return;
            }
        };
        // Said once, when it begins: not at every look.
        if let Some(why) = applied.get("failure").and_then(Value::as_str)
            && !was_failing
        {
            let name = self.session.mailbox.calendar(done.calendar).map(|c| c.name.clone()).unwrap_or_default();
            self.status(crate::i18n::fmt(tl!("Couldn't update {name}: {why}. What it held stays."), &[("name", &name), ("why", why)]));
        }
    }

    /// Send/Receive, or Update Now: every subscribed calendar is looked at now, ahead of its time.
    pub(crate) fn refresh_subscribed(&mut self) {
        if self.services.subscription.is_some() && !self.offline() {
            let _ = self.session.run("calendar.refreshSubscribed", &json!({}));
            self.subscribed.next_ask = 0.0;
        }
    }
}

/// How long a subscribed calendar waits between two looks, as File › Info offers it: minutes.
pub(crate) const INTERVALS: [u32; 8] = [15, 30, 60, 120, 240, 480, 720, 1440];

/// An interval in words.
pub(crate) fn interval_words(minutes: u32) -> String {
    match minutes {
        m if m < 60 => tln!(m, "Every {n} minute", "Every {n} minutes"),
        m if m % 1440 == 0 => tln!(m / 1440, "Every {n} day", "Every {n} days"),
        m => tln!(m / 60, "Every {n} hour", "Every {n} hours"),
    }
}
