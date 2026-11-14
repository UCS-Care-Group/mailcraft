// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: the watch on an Inbox with IDLE, against the in-process fake server: it hears of new
//! mail, renews, stops, gives up and is bounded.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use mailcraft_model::sync::{IdleEnd, IdleNote, SyncRequest};
use mailcraft_model::{Account, AccountKind, Security, Server};

use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use crate::idle::{Timing, watch_with};

const PASSWORD: &str = "hunter2";

fn world() -> ImapWorld {
    let message = |uid| FakeMessage {
        uid,
        flags: String::new(),
        internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
        body: b"From: Bob <bob@example.org>\r\nSubject: Hi\r\n\r\nHi.\r\n".to_vec(),
        claimed_size: None,
    };
    let inbox = FakeMailbox {
        raw: "INBOX".into(),
        flags: "\\HasNoChildren".into(),
        uid_validity: 7,
        messages: vec![message(1), message(2)],
        refuse_open: false,
        next_uid: 0,
    };
    ImapWorld { user: "ada".into(), password: PASSWORD.into(), mailboxes: vec![inbox], idle: true, ..Default::default() }
}

fn request(imap: &FakeImap, password: &str) -> SyncRequest {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    SyncRequest { account, password: password.into(), ..Default::default() }
}

fn quick() -> Timing {
    Timing {
        period: Duration::from_millis(400),
        poll: Duration::from_millis(30),
        note_every: Duration::from_millis(50),
        backoff: Duration::from_millis(20),
        backoff_most: Duration::from_millis(60),
        attempts: 5,
        events: 100,
        idles: 20,
        shortest: Duration::from_millis(100),
    }
}

/// A watch on its own thread: what it says, and the flag that stops it.
struct Running {
    notes: Receiver<IdleNote>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    fn start(request: SyncRequest, timing: Timing) -> Running {
        let (tx, notes) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            watch_with(&request, &flag, timing, &mut |note| {
                let _ = tx.send(note);
            })
        });
        Running { notes, stop, thread: Some(thread) }
    }

    fn next(&self, within: Duration) -> Option<IdleNote> {
        self.notes.recv_timeout(within).ok()
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("waited for {what}");
}

#[test]
fn new_mail_is_told_and_nothing_else_is() {
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, PASSWORD), quick());
    wait_until("the client to idle", || imap.world().idling == 1);
    // The mailbox is only looked at.
    assert!(imap.world().log.iter().any(|line| line.contains("EXAMINE")) && !imap.world().log.iter().any(|line| line.contains(" SELECT ")));
    // Flags, and a count that goes down, are no news of mail.
    imap.world().idle_events = vec!["* 1 FETCH (FLAGS (\\Seen))".into(), "* 1 EXPUNGE".into()];
    // Sent before the next lines take their place (a slow machine may not have got to them yet).
    wait_until("the server to tell it", || imap.world().idle_events.is_empty());
    assert_eq!(watch.next(Duration::from_millis(300)), None);
    // A count that goes up is: 2 - 1 = 1 now, so 2 is up.
    imap.world().idle_events = vec!["* 2 EXISTS".into(), "* 1 RECENT".into()];
    assert_eq!(watch.next(Duration::from_secs(5)), Some(IdleNote::Arrived));
    assert_eq!(watch.next(Duration::from_millis(300)), None);
    watch.stop();
    wait_until("the client to be gone", || imap.world().idling == 0);
}

#[test]
fn idle_is_ended_and_begun_again_before_the_server_may_drop_it() {
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, PASSWORD), quick());
    wait_until("three IDLEs", || imap.world().idles >= 3);
    let log = imap.world().log.clone();
    assert!(log.iter().filter(|line| line.trim() == "DONE").count() + 1 >= 3, "{log:?}");
    // One connection all the while.
    assert_eq!(imap.world().connections, 1);
    watch.stop();
}

#[test]
fn an_idle_the_server_ended_itself_is_begun_again() {
    let mut w = world();
    w.idle_ends_after = Some(3);
    let imap = Fake::imap(w);
    let watch = Running::start(request(&imap, PASSWORD), quick());
    wait_until("a second IDLE", || imap.world().idles >= 2);
    assert_eq!(imap.world().connections, 1);
    watch.stop();
}

#[test]
fn a_server_that_ends_every_idle_at_once_costs_attempts_and_is_given_up() {
    let mut w = world();
    w.idle_ends_after = Some(0);
    let imap = Fake::imap(w);
    // (The fake ends one IDLE each time it is set; a server that always does is the same.)
    let watch = Running::start(request(&imap, PASSWORD), Timing { attempts: 3, shortest: Duration::from_secs(1), ..quick() });
    let stop = watch.stop.clone();
    let keep = std::thread::spawn({
        let world = imap.world_handle();
        move || {
            while !stop.load(Ordering::Relaxed) {
                world.lock().unwrap().idle_ends_after = Some(0);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    assert_eq!(watch.next(Duration::from_secs(10)), Some(IdleNote::Ended(IdleEnd::TooOften)));
    let connections = imap.world().connections;
    let idles = imap.world().idles;
    assert!(connections <= 3 && idles <= 3, "{connections} {idles}");
    watch.stop.store(true, Ordering::Relaxed);
    keep.join().unwrap();
    watch.stop();
}

#[test]
fn the_line_allowance_is_for_the_connection_not_for_each_idle() {
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, PASSWORD), Timing { attempts: 1, events: 50, period: Duration::from_millis(150), ..quick() });
    wait_until("the client to idle", || imap.world().idling == 1);
    // Forty lines in each of several IDLEs: no IDLE is over the limit, the connection is.
    let mut left = false;
    for _ in 0..10 {
        let before = imap.world().idles;
        imap.world().idle_events = (0..40).map(|n| format!("* OK still here {n}")).collect();
        wait_until("the lines to be taken", || {
            left |= watch.notes.try_recv().is_ok();
            left || imap.world().idle_events.is_empty()
        });
        // The next IDLE: the lines said in it are the same connection's.
        wait_until("IDLE to be begun again", || {
            left |= watch.notes.try_recv().is_ok();
            left || imap.world().idles > before
        });
        if left {
            break;
        }
    }
    assert!(left, "the connection was never left");
    assert!(imap.world().connections == 1);
    watch.stop();
}

#[test]
fn stopping_ends_it_at_once_and_says_goodbye() {
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, PASSWORD), Timing { poll: Duration::from_millis(500), ..quick() });
    wait_until("the client to idle", || imap.world().idling == 1);
    let started = Instant::now();
    let notes = watch.notes.try_iter().count();
    assert_eq!(notes, 0);
    let stop = watch.stop.clone();
    let rx = watch.notes;
    stop.store(true, Ordering::Relaxed);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).ok(), Some(IdleNote::Ended(IdleEnd::Stopped)));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    wait_until("the client to be gone", || imap.world().idling == 0);
    assert!(imap.world().log.iter().any(|line| line.contains("LOGOUT")));
}

#[test]
fn a_server_without_idle_or_a_refused_sign_in_ends_the_watch() {
    let mut w = world();
    w.idle = false;
    let imap = Fake::imap(w);
    let watch = Running::start(request(&imap, PASSWORD), quick());
    assert_eq!(watch.next(Duration::from_secs(5)), Some(IdleNote::Ended(IdleEnd::NoIdle)));
    assert_eq!(imap.world().idles, 0);
    watch.stop();

    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, "wrong"), quick());
    assert_eq!(watch.next(Duration::from_secs(5)), Some(IdleNote::Ended(IdleEnd::Refused)));
    // The wrong password is nowhere in what was logged or said.
    assert!(imap.world().connections == 1);
    watch.stop();

    // Nothing to sign in with: not even a connection is made.
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, ""), quick());
    assert_eq!(watch.next(Duration::from_secs(5)), Some(IdleNote::Ended(IdleEnd::Refused)));
    assert_eq!(imap.world().connections, 0);
    watch.stop();
}

#[test]
fn a_connection_that_keeps_dropping_is_made_so_many_times_and_then_given_up() {
    let mut w = world();
    w.hang_up_on = Some(" IDLE".into());
    let imap = Fake::imap(w);
    let watch = Running::start(request(&imap, PASSWORD), quick());
    assert_eq!(watch.next(Duration::from_secs(10)), Some(IdleNote::Ended(IdleEnd::TooOften)));
    assert_eq!(imap.world().connections, 5);
    watch.stop();
}

#[test]
fn a_server_that_floods_idle_is_left_and_the_flood_is_bounded() {
    let imap = Fake::imap(world());
    let watch = Running::start(request(&imap, PASSWORD), Timing { attempts: 1, ..quick() });
    wait_until("the client to idle", || imap.world().idling == 1);
    // A thousand lines that say nothing of mail; the limit is a hundred.
    imap.world().idle_events = (0..1_000).map(|n| format!("* OK still here {n}")).collect();
    // The connection is left, and with one attempt an hour that is the end of it.
    assert_eq!(watch.next(Duration::from_secs(10)), Some(IdleNote::Ended(IdleEnd::TooOften)));
    assert_eq!(imap.world().connections, 1);
    watch.stop();
}

#[test]
fn mail_that_came_while_the_connection_was_down_is_told_after_it_is_made_again() {
    let mut w = world();
    // The first connection is lost after a while of idling; the second finds the Inbox as it was.
    w.idle_hangs_up_after = Some(5);
    let imap = Fake::imap(w);
    let watch = Running::start(request(&imap, PASSWORD), quick());
    assert_eq!(watch.next(Duration::from_secs(10)), Some(IdleNote::Arrived));
    assert!(imap.world().connections >= 2);
    watch.stop();
}
