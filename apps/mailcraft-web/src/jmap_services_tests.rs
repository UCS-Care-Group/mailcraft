//! The runner of a JMAP account's Send/Receive and downloads, over the in-process JMAP server: the job's
//! own credential or the host's, a credential the server refuses (at sign-in or during the run) and the
//! host renews, a host that has none yet, and a run that is stopped. Nothing here leaves this machine.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_jmap::standin::{SESSION, StandIn, TOKEN};
use mailcraft_jmap::{Auth, block_on};
use mailcraft_model::sync::{JmapFetchRequest, JmapMailRequest, JmapMailResult, JmapScope};
use mailcraft_ui_egui::jmap_job::{JmapFetchJob, JmapJob};

use super::{ASK_AGAIN_MS, AT_ONCE, Queue, fetch, run};
use crate::credentials::{Credentials, Renewal};

const ME: &str = "ada@example.org";

/// A host that holds a credential and answers each renewal from a list (`Reload` once it is empty), and, when it
/// gives a new token, makes the server take it, as the host and its server do together.
struct Host {
    held: RefCell<Option<Auth>>,
    answers: RefCell<VecDeque<Renewal>>,
    asked: Cell<u32>,
    server: StandIn,
}

impl Host {
    fn new(server: &StandIn, held: Option<&str>, answers: Vec<Renewal>) -> Host {
        Host { held: RefCell::new(held.map(Auth::bearer)), answers: RefCell::new(answers.into()), asked: Cell::new(0), server: server.clone() }
    }
}

impl Credentials for Host {
    fn auth(&self, _email: &str) -> Option<Auth> {
        self.held.borrow().clone()
    }

    async fn renew(&self, _email: &str) -> Renewal {
        self.asked.set(self.asked.get() + 1);
        let answer = self.answers.borrow_mut().pop_front().unwrap_or(Renewal::Reload);
        if let Renewal::Given(Auth::Bearer(token)) = &answer {
            self.server.with(|d| d.token = Some(token.clone()));
            *self.held.borrow_mut() = Some(Auth::bearer(token.clone()));
        }
        answer
    }
}

/// A server with an Inbox of two messages.
fn server() -> StandIn {
    let server = StandIn::new();
    server.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        for (n, received) in [(1, "2026-10-15T09:30:00Z"), (2, "2026-10-16T09:30:00Z")] {
            let raw =
                format!("From: Zoe <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: Hello {n}\r\nMessage-ID: <m{n}@example.org>\r\n\r\nHello.\r\n");
            d.deliver(&inbox, raw.as_bytes(), &[], received);
        }
    });
    server
}

/// A job for a run that lists the mailboxes and reads the Inbox, with the job's own secret or none.
fn job(secret: Option<&str>) -> JmapJob {
    let request = JmapMailRequest {
        account: 7,
        email: ME.into(),
        session: SESSION.into(),
        bearer: true,
        window: 50,
        list_folders: true,
        scope: JmapScope::Light { also: Vec::new() },
        ..Default::default()
    };
    JmapJob { request, secret: secret.map(str::to_string), cancel: Arc::new(AtomicBool::new(false)) }
}

/// Every piece of a run, and the waits it asked for, in milliseconds.
fn pieces(server: &StandIn, job: &JmapJob, host: Option<&Host>) -> (Vec<JmapMailResult>, Vec<i32>) {
    let waits = RefCell::new(Vec::new());
    let mut all = Vec::new();
    let sleep = |ms: i32| {
        waits.borrow_mut().push(ms);
        std::future::ready(())
    };
    block_on(run(server.clone(), job, host, sleep, &mut |piece| all.push(piece)));
    (all, waits.into_inner())
}

/// The last piece: the only one that isn't `partial`, and the last of all.
fn last(pieces: &[JmapMailResult]) -> &JmapMailResult {
    let (last, rest) = pieces.split_last().expect("a run ends with a piece");
    assert!(!last.partial && rest.iter().all(|piece| piece.partial), "{pieces:?}");
    assert!(pieces.iter().all(|piece| (piece.account, piece.email.as_str()) == (7, ME)), "{pieces:?}");
    last
}

/// The messages the windows of a run brought.
fn messages(pieces: &[JmapMailResult]) -> usize {
    pieces.iter().flat_map(|piece| &piece.pages).map(|page| page.messages.len()).sum()
}

#[test]
fn a_run_with_the_host_s_credential_reads_the_inbox() {
    let server = server();
    let host = Host::new(&server, Some(TOKEN), Vec::new());
    let (all, waits) = pieces(&server, &job(None), Some(&host));
    let end = last(&all);
    assert!(end.whole && !end.auth_refused && !end.stopped, "{end:?}");
    assert!(all.first().is_some_and(|piece| piece.listing.is_some()), "the mailboxes first");
    assert_eq!(messages(&all), 2);
    assert_eq!((host.asked.get(), waits.len()), (0, 0), "nothing to renew");
}

#[test]
fn a_token_refused_at_sign_in_is_renewed_by_the_host_and_the_run_goes_on() {
    let server = server();
    // The host's token has expired: the server holds another one now.
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Given(Auth::bearer("fresh"))]);
    let (all, _) = pieces(&server, &job(None), Some(&host));
    let end = last(&all);
    assert!(end.whole && !end.auth_refused, "{end:?}");
    assert_eq!(messages(&all), 2);
    assert_eq!(host.asked.get(), 1);
}

#[test]
fn a_token_that_expires_during_the_run_is_renewed_and_the_run_is_read_again() {
    let server = server();
    // The session and the list of mailboxes go through; the next request finds the token gone.
    server.with(|d| d.drop_token_after = Some(1));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Given(Auth::bearer("fresh"))]);
    let (all, _) = pieces(&server, &job(None), Some(&host));
    let end = last(&all);
    assert!(end.whole && !end.auth_refused, "{end:?}");
    assert_eq!(all.iter().filter(|piece| !piece.partial).count(), 1, "one last piece, the refusal held back");
    assert_eq!(messages(&all), 2);
    assert_eq!(host.asked.get(), 1);
}

#[test]
fn a_host_with_nothing_yet_is_asked_again_a_little_later() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Later, Renewal::Later, Renewal::Given(Auth::bearer("fresh"))]);
    let (all, waits) = pieces(&server, &job(None), Some(&host));
    assert!(last(&all).whole);
    // Waited 5 s, then 10 s, in slices that let a stop through.
    assert_eq!(waits.iter().sum::<i32>(), ASK_AGAIN_MS[..2].iter().sum::<i32>());
    assert!(waits.iter().all(|ms| *ms <= 500), "{waits:?}");
    assert_eq!(host.asked.get(), 3);
}

#[test]
fn a_host_that_never_has_one_leaves_the_refusal_standing() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    // Asked once, then once after each wait: then the run ends.
    let host = Host::new(&server, Some(TOKEN), (0..10).map(|_| Renewal::Later).collect());
    let (all, waits) = pieces(&server, &job(None), Some(&host));
    let end = last(&all);
    assert!(end.auth_refused && !end.whole, "{end:?}");
    assert_eq!(waits.iter().sum::<i32>(), ASK_AGAIN_MS.iter().sum::<i32>());
    assert_eq!(host.asked.get(), 4);
    // A host that says it has nothing more to give is asked once.
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Reload]);
    let (all, waits) = pieces(&server, &job(None), Some(&host));
    assert!(last(&all).auth_refused);
    assert_eq!((host.asked.get(), waits.len()), (1, 0));
}

/// A host whose new tokens the server never takes: the server moves on to another one each time.
struct Outrun<'a>(&'a Host);

impl Credentials for Outrun<'_> {
    fn auth(&self, email: &str) -> Option<Auth> {
        self.0.auth(email)
    }

    async fn renew(&self, email: &str) -> Renewal {
        let answer = self.0.renew(email).await;
        self.0.server.with(|d| d.token = Some("rotated-again".into()));
        answer
    }
}

#[test]
fn a_renewed_token_that_is_refused_too_is_not_renewed_again() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Given(Auth::bearer("fresh")), Renewal::Given(Auth::bearer("fresher"))]);
    let mut all = Vec::new();
    block_on(run(server.clone(), &job(None), Some(&Outrun(&host)), |_| std::future::ready(()), &mut |piece| all.push(piece)));
    assert!(last(&all).auth_refused);
    assert_eq!(host.asked.get(), 1, "one renewal a run");
}

#[test]
fn the_owner_s_own_credential_refused_is_never_renewed() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some("rotated"), vec![Renewal::Given(Auth::bearer("fresh"))]);
    // The job carries the owner's own token: the host is not asked, even when it could answer.
    let (all, _) = pieces(&server, &job(Some(TOKEN)), Some(&host));
    assert!(last(&all).auth_refused);
    assert_eq!(host.asked.get(), 0);
    // And with the right one it goes through.
    let (all, _) = pieces(&server, &job(Some("rotated")), Some(&host));
    assert!(last(&all).whole);
}

#[test]
fn without_any_credential_the_run_ends_refused() {
    let server = server();
    let host = Host::new(&server, None, Vec::new());
    let (all, _) = pieces(&server, &job(None), Some(&host));
    assert!(last(&all).auth_refused);
    let (all, _) = pieces(&server, &job(None), None);
    assert!(last(&all).auth_refused);
    assert!(server.seen().is_empty(), "nothing was asked of the server, not even its session");
}

#[test]
fn a_run_stopped_before_it_starts_or_while_it_waits_says_so() {
    let server = server();
    let host = Host::new(&server, Some(TOKEN), Vec::new());
    let stopped = job(None);
    stopped.cancel.store(true, Ordering::Relaxed);
    let (all, _) = pieces(&server, &stopped, Some(&host));
    let end = last(&all);
    assert!(end.stopped && !end.auth_refused && !end.whole, "{end:?}");
    assert!(server.seen().is_empty(), "not even the session");
    // Stopped while it waits for the host: stopped, not refused.
    server.with(|d| d.token = Some("rotated".into()));
    let waiting = job(None);
    let cancel = waiting.cancel.clone();
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Later, Renewal::Later]);
    let mut all = Vec::new();
    let sleep = |_ms: i32| {
        cancel.store(true, Ordering::Relaxed);
        std::future::ready(())
    };
    block_on(run(server.clone(), &waiting, Some(&host), sleep, &mut |piece| all.push(piece)));
    let end = last(&all);
    assert!(end.stopped && !end.auth_refused, "{end:?}");
    assert_eq!(host.asked.get(), 1);
}

#[test]
fn a_password_goes_with_the_account_s_address() {
    assert_eq!(super::own_auth(false, ME, "pw"), Auth::basic(ME, "pw"));
    assert_eq!(super::own_auth(true, ME, "tok"), Auth::bearer("tok"));
}

/// A job that downloads the server's emails whole, with the job's own secret or none.
fn fetch_job(server: &StandIn, secret: Option<&str>) -> JmapFetchJob {
    let ids = server.with(|d| d.emails.iter().map(|email| email.id.clone()).collect());
    let request = JmapFetchRequest { account: 7, email: ME.into(), session: SESSION.into(), bearer: true, ids, background: false };
    JmapFetchJob { request, secret: secret.map(str::to_string), cancel: Arc::new(AtomicBool::new(false)) }
}

#[test]
fn a_download_with_the_host_s_credential_brings_the_messages_whole() {
    let server = server();
    let host = Host::new(&server, Some(TOKEN), Vec::new());
    let answer = block_on(fetch(server.clone(), &fetch_job(&server, None), Some(&host), |_| std::future::ready(())));
    assert_eq!(answer.fetched.len(), 2, "{answer:?}");
    assert!(answer.fetched.iter().all(|message| String::from_utf8_lossy(&message.mime).contains("Subject: Hello")));
    assert!(answer.errors.is_empty() && answer.missing.is_empty() && (answer.account, answer.email.as_str()) == (7, ME));
    assert_eq!(host.asked.get(), 0);
}

#[test]
fn a_download_whose_token_is_refused_signs_in_again_once_with_a_new_one() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Given(Auth::bearer("fresh"))]);
    let answer = block_on(fetch(server.clone(), &fetch_job(&server, None), Some(&host), |_| std::future::ready(())));
    assert_eq!((answer.fetched.len(), host.asked.get()), (2, 1), "{answer:?}");
    // A host with nothing to give: nothing fetched, and why
    server.with(|d| d.token = Some("rotated-again".into()));
    let host = Host::new(&server, Some("fresh"), vec![Renewal::Reload]);
    let answer = block_on(fetch(server.clone(), &fetch_job(&server, None), Some(&host), |_| std::future::ready(())));
    assert!(answer.fetched.is_empty() && !answer.errors.is_empty(), "{answer:?}");
    // The owner's own token refused: the host is never asked
    let host = Host::new(&server, Some("rotated-again"), vec![Renewal::Given(Auth::bearer("fresh"))]);
    let answer = block_on(fetch(server.clone(), &fetch_job(&server, Some(TOKEN)), Some(&host), |_| std::future::ready(())));
    assert!(answer.fetched.is_empty() && !answer.errors.is_empty(), "{answer:?}");
    assert_eq!(host.asked.get(), 0);
}

#[test]
fn a_run_refused_mid_way_and_stopped_while_it_waits_ends_stopped() {
    let server = server();
    // The session and the list of mailboxes go through; then the token is gone, and the host has none yet.
    server.with(|d| d.drop_token_after = Some(1));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Later, Renewal::Later]);
    let waiting = job(None);
    let cancel = waiting.cancel.clone();
    let mut all = Vec::new();
    // The owner stops the run while it waits for the host.
    let sleep = |_ms: i32| {
        cancel.store(true, Ordering::Relaxed);
        std::future::ready(())
    };
    block_on(run(server.clone(), &waiting, Some(&host), sleep, &mut |piece| all.push(piece)));
    let end = last(&all);
    assert!(end.stopped && !end.auth_refused && !end.whole, "{end:?}");
    assert_eq!(all.iter().filter(|piece| !piece.partial).count(), 1, "one last piece");
}

#[test]
fn a_download_whose_renewed_token_is_refused_too_asks_once() {
    let server = server();
    server.with(|d| d.token = Some("rotated".into()));
    let host = Host::new(&server, Some(TOKEN), vec![Renewal::Given(Auth::bearer("fresh")), Renewal::Given(Auth::bearer("fresher"))]);
    let answer = block_on(fetch(server.clone(), &fetch_job(&server, None), Some(&Outrun(&host)), |_| std::future::ready(())));
    assert!(answer.fetched.is_empty() && !answer.errors.is_empty(), "{answer:?}");
    assert_eq!(host.asked.get(), 1, "one renewal a download");
    // Stopped while it waits for the host: nothing fetched, and nothing said against the server.
    server.with(|d| d.token = Some("rotated-again".into()));
    let host = Host::new(&server, Some("fresh"), vec![Renewal::Later]);
    let stopped = fetch_job(&server, None);
    let cancel = stopped.cancel.clone();
    let sleep = |_ms: i32| {
        cancel.store(true, Ordering::Relaxed);
        std::future::ready(())
    };
    let answer = block_on(fetch(server.clone(), &stopped, Some(&host), sleep));
    assert!(answer.fetched.is_empty() && answer.errors.is_empty() && !answer.offline, "{answer:?}");
}

#[test]
fn the_gate_lets_requests_go_in_the_order_they_came() {
    let mut queue = Queue::default();
    let (first, second, third) = (queue.take(), queue.take(), queue.take());
    // Not out of turn, and never more than its room.
    assert!(!queue.go(second), "the second waits for the first");
    assert!(queue.go(first) && queue.go(second));
    assert_eq!(AT_ONCE, 2);
    assert!(!queue.go(third), "no room for a third");
    // A request that was on its way takes a new ticket for its next one: behind the third.
    queue.leave(first, true);
    let again = queue.take();
    assert!(!queue.go(again), "the one that waited goes first");
    assert!(queue.go(third));
    // A request that gives up waiting leaves the queue, and the one after it moves up.
    let gives_up = queue.take();
    let after = queue.take();
    queue.leave(gives_up, false);
    queue.leave(second, true);
    assert!(!queue.go(after), "the one before it first");
    assert!(queue.go(again));
    queue.leave(third, true);
    assert!(queue.go(after));
}
