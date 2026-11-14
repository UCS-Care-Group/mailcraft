// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Waiting for new mail (#276): a connection to an IMAP account's own server kept open in IDLE
//! (RFC 2177) on its Inbox, so that the window hears of a new message within moments instead of
//! at the next timed Send/Receive. [`watch`] runs on a thread of the host's, until it is told to
//! stop, and does nothing with the mail: when the server says the Inbox has more messages, it
//! says so ([`IdleNote::Arrived`]), and the window runs the account's Send/Receive, as ever.
//!
//! - **Encrypted or not at all**, like every connection ([`open_imap_with`]), signed in with the
//!   password or token the Send/Receive of the account is ([`SyncRequest::secret`]), which are
//!   never logged. The mailbox is opened read-only (EXAMINE): nothing is changed by looking.
//! - **Renewed**: IDLE is ended and begun again every 25 minutes (a server may drop one after
//!   30), and again when the server ends it itself.
//! - **Bounded**: a server that says more than [`Timing::events`] lines in one IDLE is treated as
//!   hostile and left; a line over the protocol's limit or a literal ends the session; reads
//!   wait a second at a time and never spin; the news is passed on at most once in
//!   [`Timing::note_every`]; a connection that drops is made again after a wait that doubles from
//!   five seconds to five minutes, and no more than [`Timing::attempts`] times an hour, after
//!   which the watch gives up ([`IdleEnd::TooOften`]) and the window decides when to try again.
//! - **Stopped** at once (within a second) by the flag: the server is said DONE and goodbye to.
//! - A server that can't do IDLE ends the watch ([`IdleEnd::NoIdle`]); one that refuses the
//!   sign-in does too ([`IdleEnd::Refused`]): the account keeps to its timer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::sync::{IdleEnd, IdleNote, SyncRequest};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{IdleEvent, Imap};

use crate::NetError;
use crate::sync::open_imap_with;
use crate::transport::{Patience, Stream};

/// The times and limits of a watch.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// IDLE is ended and begun again this often (RFC 2177: a server may drop one after 30 minutes).
    pub period: Duration,
    /// How long one read waits for the server to speak.
    pub poll: Duration,
    /// The news is passed on at most this often.
    pub note_every: Duration,
    /// The wait before the first new connection, doubling to `backoff_most`.
    pub backoff: Duration,
    pub backoff_most: Duration,
    /// Most connections made in an hour.
    pub attempts: usize,
    /// Most lines the server may say in one connection.
    pub events: usize,
    /// Most IDLEs begun on one connection.
    pub idles: usize,
    /// An IDLE the server ends sooner than this is a failure.
    pub shortest: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            period: Duration::from_secs(25 * 60),
            poll: Duration::from_secs(1),
            note_every: Duration::from_secs(2),
            backoff: Duration::from_secs(5),
            backoff_most: Duration::from_secs(5 * 60),
            attempts: 12,
            events: 5_000,
            idles: 200,
            shortest: Duration::from_secs(5),
        }
    }
}

/// What ended one connection's watch.
enum Over {
    Stopped,
    NoIdle,
    Refused,
    /// The connection is gone or the server misbehaved; `idled` says it had been idling for a
    /// while (so a wait of its own is not a sign of trouble).
    Dropped {
        idled: bool,
    },
}

/// Watch the Inbox of `request`'s account until `stop` is set or the watch gives up, telling
/// `note` of what happens; the last thing it tells is [`IdleNote::Ended`]. Blocks.
pub fn watch(request: &SyncRequest, stop: &AtomicBool, note: &mut dyn FnMut(IdleNote)) {
    watch_with(request, stop, Timing::default(), note);
}

/// [`watch`], with `timing`.
pub fn watch_with(request: &SyncRequest, stop: &AtomicBool, timing: Timing, note: &mut dyn FnMut(IdleNote)) {
    let mut tried: VecDeque<Instant> = VecDeque::new();
    let mut wait = timing.backoff;
    let mut connected_before = false;
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break IdleEnd::Stopped;
        }
        let now = Instant::now();
        while tried.front().is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(3600)) {
            tried.pop_front();
        }
        if tried.len() >= timing.attempts {
            break IdleEnd::TooOften;
        }
        tried.push_back(now);
        match session(request, stop, timing, connected_before, note) {
            Over::Stopped => break IdleEnd::Stopped,
            Over::NoIdle => break IdleEnd::NoIdle,
            Over::Refused => break IdleEnd::Refused,
            Over::Dropped { idled } => {
                connected_before |= idled;
                if idled {
                    wait = timing.backoff;
                }
                log::debug!("idle: the connection is gone; trying again after a wait");
                if !sleep(wait, stop) {
                    break IdleEnd::Stopped;
                }
                wait = wait.saturating_mul(2).min(timing.backoff_most);
            }
        }
    };
    note(IdleNote::Ended(end));
}

/// Wait `how_long` unless told to stop (looked at every tenth of a second): whether it was waited out.
fn sleep(how_long: Duration, stop: &AtomicBool) -> bool {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())));
    }
    !stop.load(Ordering::Relaxed)
}

/// One connection's watch.
fn session(request: &SyncRequest, stop: &AtomicBool, timing: Timing, reconnected: bool, note: &mut dyn FnMut(IdleNote)) -> Over {
    let mut imap = match open_imap_with(&request.account, request.secret(), &Patience::default(), Some(stop)) {
        Ok(imap) => imap,
        // Nothing to show the server, or the server said no to it: the same again says no again.
        Err(NetError::Proto(ProtoError::Auth(_)) | NetError::Config(_)) => return Over::Refused,
        Err(_) => return Over::Dropped { idled: false },
    };
    let over = idle_on(&mut imap, stop, timing, reconnected, note);
    // Said once the watch is over: nobody waits for a goodbye.
    imap.get_mut().leave();
    let _ = imap.logout();
    over
}

/// The server is waiting, and the watch was told to stop: it is told DONE (the connection no
/// longer minds the flag for that), so that it is not left waiting for a client that is gone.
fn stop_idling(imap: &mut Imap<Stream<'_>>) {
    imap.get_mut().watch(None);
    let _ = imap.idle_end();
}

fn idle_on(imap: &mut Imap<Stream<'_>>, stop: &AtomicBool, timing: Timing, reconnected: bool, note: &mut dyn FnMut(IdleNote)) -> Over {
    // Asked again now that the server knows who we are: some say more after the sign-in.
    match imap.capabilities() {
        Ok(caps) if caps.iter().any(|c| c.eq_ignore_ascii_case("IDLE")) => {}
        Ok(_) => return Over::NoIdle,
        Err(_) => return Over::Dropped { idled: false },
    }
    let mut exists = match imap.select("INBOX", true) {
        Ok(selected) => selected.exists,
        Err(_) => return Over::Dropped { idled: false },
    };
    // Mail may have come while the connection was down.
    let mut due: Option<Instant> = reconnected.then(Instant::now);
    let mut last_note: Option<Instant> = None;
    let mut idled = false;
    // Lines heard and IDLEs begun over the whole connection, not each IDLE: ending IDLE
    // early must not buy a server a new allowance.
    let mut heard = 0usize;
    let mut begun_count = 0usize;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Over::Stopped;
        }
        begun_count += 1;
        if begun_count > timing.idles {
            log::warn!("idle: too many IDLEs on one connection; leaving it");
            return Over::Dropped { idled };
        }
        // The server's go-ahead gets the time an answer does; only the wait that follows is a second a read.
        if imap.get_mut().answer_normally().is_err() {
            return Over::Dropped { idled };
        }
        match imap.idle_begin() {
            Ok(()) => {}
            // A server that offers IDLE and won't do it here.
            Err(ProtoError::Rejected(_)) => return Over::NoIdle,
            Err(_) => return Over::Dropped { idled },
        }
        idled = true;
        if imap.get_mut().wait_for_news(timing.poll, timing.period.saturating_add(Duration::from_secs(600))).is_err() {
            return Over::Dropped { idled };
        }
        let begun = Instant::now();
        let mut ended_itself = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                stop_idling(imap);
                return Over::Stopped;
            }
            let now = Instant::now();
            if due.is_some_and(|at| now >= at) && last_note.is_none_or(|at| now.duration_since(at) >= timing.note_every) {
                due = None;
                last_note = Some(now);
                note(IdleNote::Arrived);
            }
            if now.duration_since(begun) >= timing.period {
                break;
            }
            match imap.idle_poll() {
                Ok(Some(IdleEvent::Ended)) => {
                    // A server that ends IDLE at once is not waiting for anything: that is a
                    // failure (a wait and an attempt of the hour), not a reason to ask again.
                    if now.duration_since(begun) < timing.shortest {
                        return Over::Dropped { idled: false };
                    }
                    ended_itself = true;
                    break;
                }
                Ok(Some(event)) => {
                    heard += 1;
                    if heard > timing.events {
                        log::warn!("idle: the server said too much while idle; leaving it");
                        return Over::Dropped { idled };
                    }
                    match event {
                        IdleEvent::Exists(n) => {
                            if n > exists && due.is_none() {
                                // Passed on at once, or when the last was long enough ago.
                                due = Some(Instant::now());
                            }
                            exists = n;
                        }
                        IdleEvent::Expunge(_) => exists = exists.saturating_sub(1),
                        IdleEvent::Recent(_) | IdleEvent::Other | IdleEvent::Ended => {}
                    }
                }
                Ok(None) => {
                    // A read that came back at once with nothing is never a reason to spin.
                    let took = Instant::now().duration_since(now);
                    if took < Duration::from_millis(100) {
                        std::thread::sleep(Duration::from_millis(100) - took);
                    }
                }
                // (The flag also ends the stream's reads: that is a stop, not a drop.)
                Err(_) if stop.load(Ordering::Relaxed) => {
                    stop_idling(imap);
                    return Over::Stopped;
                }
                Err(_) => return Over::Dropped { idled },
            }
        }
        if !ended_itself {
            match imap.idle_end() {
                Ok(events) => {
                    // What it said on the way counts against the connection too.
                    heard = heard.saturating_add(events.len());
                    if heard > timing.events {
                        log::warn!("idle: the server said too much while idle; leaving it");
                        return Over::Dropped { idled };
                    }
                    for event in events {
                        if let IdleEvent::Exists(n) = event {
                            if n > exists && due.is_none() {
                                due = Some(Instant::now());
                            }
                            exists = n;
                        }
                    }
                }
                Err(_) => return Over::Dropped { idled },
            }
        }
    }
}
