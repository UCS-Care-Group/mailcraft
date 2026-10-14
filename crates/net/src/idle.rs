//! Waiting for new mail between two Send/Receives (IDLE, RFC 2177): one more connection to the
//! account's incoming server, with the Inbox open, on which the server says as soon as mail
//! comes or goes. The host starts a run of its own when it hears of it ([`watch_inbox`]); nothing
//! is read or changed on this connection.
//!
//! The IDLE is begun again every 25 minutes (RFC 2177 asks for no longer than 29, or the server
//! may take the connection for dead), a connection that is lost is made again after a wait that
//! grows each time it fails, and a server that doesn't offer IDLE is left alone: its new mail
//! comes with the timed runs, as before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::sync::SyncRequest;
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{IdleNews, Imap};

use crate::NetError;
use crate::sync::open_imap_with;
use crate::transport::{Patience, Stream};

/// How a watch goes about it ([`watch_with`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleOptions {
    /// How long one IDLE lasts before it is ended and begun again.
    pub renew: Duration,
    /// How often the one waiting looks up between two reads (has it been told to stop).
    pub tick: Duration,
    /// How long to wait before the connection is made again the first time it was lost...
    pub first_retry: Duration,
    /// ...and the longest, however often it was.
    pub last_retry: Duration,
}

impl Default for IdleOptions {
    fn default() -> Self {
        IdleOptions {
            renew: Duration::from_secs(25 * 60),
            tick: Duration::from_secs(1),
            first_retry: Duration::from_secs(15),
            last_retry: Duration::from_secs(15 * 60),
        }
    }
}

/// Why a watch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdleEnd {
    /// It was told to.
    Stopped,
    /// The server doesn't offer IDLE: its new mail comes with the timed runs.
    NotOffered,
    /// The server no longer takes the sign-in (a token that ran out, a password that was
    /// changed): a watch begins again once a Send/Receive has signed in anew.
    SignIn(String),
    /// The account can't be watched as it is set up (no incoming server, no user name).
    Unusable(String),
}

/// Watch the Inbox of `request`'s account until `stop` is set, calling `on_news` whenever the
/// server says a message came, went or changed there: the host starts a Send/Receive for the
/// account then. Blocks: a host calls it on a thread of its own. The connection is made again
/// when it is lost, after a wait that grows each time; what ends the watch is said in the
/// answer. Only the account, the secret and nothing else of `request` is used.
pub fn watch_inbox(request: &SyncRequest, stop: &AtomicBool, on_news: &mut dyn FnMut()) -> IdleEnd {
    watch_with(request, stop, IdleOptions::default(), on_news)
}

/// [`watch_inbox`], going about it as `options` say.
pub fn watch_with(request: &SyncRequest, stop: &AtomicBool, options: IdleOptions, on_news: &mut dyn FnMut()) -> IdleEnd {
    let mut retry = options.first_retry;
    loop {
        if stop.load(Ordering::Relaxed) {
            return IdleEnd::Stopped;
        }
        let began = Instant::now();
        match session(request, stop, options, on_news) {
            Ok(end) => return end,
            Err(e) if e.is_refused_sign_in() => return IdleEnd::SignIn(e.without(request.secret()).to_string()),
            Err(NetError::Config(why)) => return IdleEnd::Unusable(why),
            Err(e) => {
                log::debug!("waiting for new mail: {}", e.without(request.secret()));
                // A connection that held for a good while was no failure to wait longer for.
                if began.elapsed() > options.renew {
                    retry = options.first_retry;
                }
                if !pause(stop, retry, options.tick) {
                    return IdleEnd::Stopped;
                }
                retry = retry.saturating_mul(2).min(options.last_retry);
            }
        }
    }
}

/// Wait `how_long`, looking up every `tick`; `false` when told to stop meanwhile.
fn pause(stop: &AtomicBool, how_long: Duration, tick: Duration) -> bool {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(tick.min(until.saturating_duration_since(Instant::now())).max(Duration::from_millis(1)));
    }
    !stop.load(Ordering::Relaxed)
}

/// One connection: sign in, open the Inbox, and IDLE on it, again and again, until told to stop.
/// `Err` when the connection was lost or refused.
fn session(request: &SyncRequest, stop: &AtomicBool, options: IdleOptions, on_news: &mut dyn FnMut()) -> Result<IdleEnd, NetError> {
    let mut imap = open_imap_with(&request.account, request.secret(), &Patience::default(), None)?;
    // Asked now: what a server can do is often only told once it knows who is asking.
    if !imap.capabilities()?.iter().any(|c| c == "IDLE") {
        goodbye(imap);
        return Ok(IdleEnd::NotOffered);
    }
    // Looked at only: nothing in it is marked as seen, nothing is changed.
    imap.select("INBOX", true)?;
    loop {
        imap.idle()?;
        imap.get_mut().wait_for_news(options.tick);
        let began = Instant::now();
        let stopped = loop {
            if stop.load(Ordering::Relaxed) {
                break true;
            }
            if began.elapsed() >= options.renew {
                break false;
            }
            if imap.idle_wait()? == IdleNews::Changed {
                on_news();
            }
        };
        imap.get_mut().back_to_work();
        if stopped {
            // Ended politely; nobody waits long for the answer.
            imap.get_mut().leave();
            let _ = imap.idle_done();
            goodbye(imap);
            return Ok(IdleEnd::Stopped);
        }
        match imap.idle_done() {
            Ok(()) => {}
            // A server that won't end it cleanly: the connection is made again.
            Err(ProtoError::Rejected(why)) => return Err(NetError::Proto(ProtoError::Io(why))),
            Err(e) => return Err(e.into()),
        }
    }
}

fn goodbye(mut imap: Imap<Stream<'_>>) {
    imap.get_mut().leave();
    let _ = imap.logout();
}
