// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail fetched on the phone (MB3.17): a message opened, the background, an attachment, a search
//! on the server, through a core over a fake platform and a runner that plays the Graph stand-in
//! (it asks no server). Nothing here leaves this machine; every message is invented.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::fetch::{Access, Renewal};
use mailcraft_host::secrets::secret_key;
use mailcraft_model::sync::{
    FolderListing, GraphFetchRequest, GraphFetchResult, GraphFetched, GraphFolder, GraphMailResult, GraphMessage, GraphSearchResult, MessagePage,
    ServerMessage, Tokens,
};
use mailcraft_model::{Address, FolderKind, Mailbox};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::fetch::{Answer, BACKGROUND_BATCH, BACKGROUND_TOTAL, FetchDone, FetchRunner, OPEN_MOST_BYTES, Work, bounded, room_for_more};
use crate::{AttachmentVerdict, Core, CoreError, Event, FetchFailure, NetworkKind, OpenState};

const EMAIL: &str = "owner@example.org";
const DAY: i64 = 86_400;

fn message(id: &str, subject: &str, received: i64, folder: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), ..Default::default() },
        subject: subject.into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        received,
        preview: format!("{subject} preview"),
        ..Default::default()
    }
}

/// A session at [`now`] with one Microsoft account whose Inbox has three messages of this week,
/// as headers and previews (what a delta brings).
fn session() -> Session {
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    s.prefs.sync_minutes = 0;
    let account = s.mailbox.accounts.iter().find(|a| a.email == EMAIL).unwrap().id;
    let folders = vec![
        GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() },
        GraphFolder { id: "arch".into(), name: "Archive".into(), kind: FolderKind::Archive, ..Default::default() },
    ];
    let result = GraphMailResult {
        account,
        email: EMAIL.into(),
        listing: Some(FolderListing { folders, complete: true }),
        pages: vec![MessagePage {
            folder: "in".into(),
            messages: vec![
                message("m1", "Budget review", now() - DAY, "in"),
                message("m2", "Team lunch", now() - 2 * DAY, "in"),
                message("m3", "Quarterly plan", now() - 3 * DAY, "in"),
            ],
            link: "link-1".into(),
            caught_up: true,
            ..Default::default()
        }],
        whole: true,
        ..Default::default()
    };
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    s
}

/// A message's MIME as the server would give it.
fn mime(subject: &str, body: &str) -> Vec<u8> {
    format!("From: Ada Lovelace <ada@example.org>\r\nTo: {EMAIL}\r\nSubject: {subject}\r\nContent-Type: text/plain\r\n\r\n{body}\r\n").into_bytes()
}

/// A message with an attachment named `name`.
fn with_attachment(name: &str) -> Vec<u8> {
    format!(
        "From: ada@example.org\r\nTo: {EMAIL}\r\nSubject: Files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n--b--\r\n"
    )
    .into_bytes()
}

/// What the fake runner was asked: whether with a token held or a renewal, and the work.
#[derive(Debug, Clone)]
enum Asked {
    Fetch { held: bool, refresh: String, request: GraphFetchRequest },
    Search { next: String },
}

/// How the fake server answers a fetch.
type Serve = dyn Fn(&GraphFetchRequest) -> GraphFetchResult + Send + Sync;
/// What it finds for a search (the query, the next page asked for).
type Find = dyn Fn(&str, &str) -> GraphSearchResult + Send + Sync;

struct Fake {
    asked: Arc<Mutex<Vec<Asked>>>,
    /// Held shut, every job waits until it is opened (ten seconds at most).
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Fake {
    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn fetches(&self) -> Vec<(bool, String, GraphFetchRequest)> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .filter_map(|a| match a {
                Asked::Fetch { held, refresh, request } => Some((*held, refresh.clone(), request.clone())),
                Asked::Search { .. } => None,
            })
            .collect()
    }
}

/// A runner that answers as `serve` and `found` say, after the gate is open.
fn runner(serve: Arc<Serve>, found: Arc<Find>, open: bool) -> (FetchRunner, Fake) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new((Mutex::new(open), Condvar::new()));
    let (log, held_gate) = (asked.clone(), gate.clone());
    let run: FetchRunner = Arc::new(move |access: Access, work: Work, _cancel: Arc<AtomicBool>| {
        let (held, refresh) = match &access {
            Access::Held(_) => (true, String::new()),
            Access::Renew { refresh, .. } => (false, refresh.clone()),
        };
        match &work {
            Work::Messages(request) => log.lock().unwrap().push(Asked::Fetch { held, refresh, request: request.clone() }),
            Work::Search(request) => log.lock().unwrap().push(Asked::Search { next: request.next.clone() }),
        }
        {
            let (lock, cv) = &*held_gate;
            let mut open = lock.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !*open && Instant::now() < deadline {
                open = cv.wait_timeout(open, Duration::from_millis(20)).unwrap().0;
            }
        }
        // A renewal brings a new access token and a rotated refresh token, as Microsoft does.
        let renewal = Renewal {
            tokens: (!held).then(|| Tokens {
                access: "fetch-access".into(),
                refresh: "rotated-refresh".into(),
                expires_at: now() + 3_600,
                ..Default::default()
            }),
            ..Default::default()
        };
        let answer = match work {
            Work::Messages(request) => Answer::Messages(serve(&request)),
            Work::Search(request) => {
                let mut result = found(&request.query, &request.next);
                (result.account, result.ticket) = (request.account, request.ticket);
                Answer::Search(result)
            }
        };
        FetchDone { answer: Some(answer), renewal }
    });
    (run, Fake { asked, gate })
}

/// Each message asked for, whole, with a body of its own.
fn whole(request: &GraphFetchRequest) -> GraphFetchResult {
    GraphFetchResult {
        account: request.account,
        email: request.email.clone(),
        fetched: request
            .ids
            .iter()
            .map(|id| GraphFetched { id: id.clone(), mime: mime(&format!("About {id}"), &format!("The whole of {id}.")) })
            .collect(),
        ..Default::default()
    }
}

fn nothing_found() -> Arc<Find> {
    Arc::new(|_, _| GraphSearchResult::default())
}

/// A core over [`session`], its account signed in by a refresh token kept in the store, its jobs
/// run by `run`. No background fetch unless `background`.
fn core_with(run: FetchRunner, background: bool) -> (Arc<Core>, Arc<FakePlatform>, u64) {
    let platform = FakePlatform::new();
    let core = Core::start(platform.clone(), session).unwrap();
    core.sync.set_fetch_runner(run);
    let (account, key) = block_on(core.call(|s| {
        let a = s.mailbox.accounts.iter().find(|a| a.email == EMAIL).unwrap();
        Ok((a.id, secret_key("refresh", a)))
    }))
    .unwrap();
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    if !background {
        // The owner stopped the background (Send/Receive Progress): only what is asked for comes.
        block_on(core.call(|s| {
            s.sync_progress.full_paused = true;
            Ok(())
        }))
        .unwrap();
    }
    (core, platform, account)
}

fn id_of(core: &Core, server: &str) -> u64 {
    let server = server.to_string();
    block_on(core.call(move |s| Ok(s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|sv| sv.id == server)).map(|m| m.id))))
        .unwrap()
        .unwrap()
}

fn is_whole(core: &Core, id: u64) -> bool {
    block_on(core.call(move |s| Ok(s.mailbox.message(id).is_some_and(|m| !m.unfetched)))).unwrap()
}

fn drain(core: &Core) -> Vec<Event> {
    let mut out = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        let mut next = std::pin::pin!(core.next_event());
        match next.as_mut().poll(&mut cx) {
            Poll::Ready(Some(event)) => out.push(event),
            Poll::Ready(None) | Poll::Pending => return out,
        }
    }
}

/// Events until one matches (ten seconds at most).
fn until(core: &Core, what: impl Fn(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        all.extend(drain(core));
        if all.iter().any(&what) {
            return all;
        }
        assert!(Instant::now() < deadline, "the event never came: {all:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits a bounded time for `f` to hold.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn opening_a_message_with_only_its_headers_fetches_it_whole_and_says_so() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, platform, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert!(!is_whole(&core, m1));
    assert!(!block_on(core.message(m1, 400.0, false)).unwrap().whole, "only the preview is on the phone");

    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    let events = until(&core, |e| matches!(e, Event::MessageFetched { message } if *message == m1));
    assert!(!events.iter().any(|e| matches!(e, Event::FetchFailed { .. })), "{events:?}");
    assert!(is_whole(&core, m1));
    assert!(block_on(core.message(m1, 400.0, false)).unwrap().whole);
    // One message, at once, within what the phone takes, with a token renewed from the sign-in kept.
    let asked = fake.fetches();
    assert_eq!(asked.len(), 1);
    let (held, refresh, request) = &asked[0];
    assert!(!held && refresh == "stored-refresh");
    assert_eq!(request.ids, ["m1"]);
    assert!(!request.background && request.most_bytes == OPEN_MOST_BYTES && request.most_total == OPEN_MOST_BYTES);
    // The renewal's rotated refresh token is kept as Send/Receive keeps it; no secret is in an event.
    eventually("the rotated sign-in is kept", || platform.secrets.kept.lock().unwrap().iter().any(|(_, v)| v == "rotated-refresh"));
    let said = format!("{events:?}");
    assert!(!said.contains("refresh") && !said.contains("fetch-access"), "{said}");
    // Shown, and fetched this session: kept on the phone whatever its limits say.
    assert!(block_on(core.call(move |s| Ok(s.sel.messages == [m1] && s.kept_for_now(m1)))).unwrap());

    // The next one goes with the session's token, renewing nothing.
    let m2 = id_of(&core, "m2");
    assert_eq!(block_on(core.open_message(m2)).unwrap(), OpenState::Fetching);
    until(&core, |e| matches!(e, Event::MessageFetched { message } if *message == m2));
    assert!(fake.fetches()[1].0, "the token renewed for the first was held");
    // A message that is whole is here at once, and nothing is asked.
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Here);
    assert_eq!(fake.fetches().len(), 2);
}

#[test]
fn nothing_is_fetched_offline_and_an_account_with_no_sign_in_is_asked_to_sign_in() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let platform = FakePlatform::new();
    let core = Core::start(platform.clone(), session).unwrap();
    core.sync.set_fetch_runner(run);
    let m1 = id_of(&core, "m1");
    *platform.net.lock().unwrap() = NetworkKind::Offline;
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Offline);
    *platform.net.lock().unwrap() = NetworkKind::Online { metered: false, data_saving: false };
    // Work Offline.
    block_on(core.call(|s| {
        s.prefs.offline = true;
        Ok(())
    }))
    .unwrap();
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Offline);
    block_on(core.call(|s| {
        s.prefs.offline = false;
        Ok(())
    }))
    .unwrap();
    // No sign-in kept: nothing is asked of the server, and the app is told to sign in.
    let state = block_on(core.open_message(m1)).unwrap();
    assert!(matches!(state, OpenState::Unavailable { .. }), "{state:?}");
    let events = until(&core, |e| matches!(e, Event::FetchFailed { reason: FetchFailure::SignInNeeded, .. }));
    assert!(events.iter().any(|e| matches!(e, Event::SignInNeeded { .. })), "{events:?}");
    assert!(fake.fetches().is_empty());
}

#[test]
fn a_fetch_called_off_stores_nothing_of_what_it_brings() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    eventually("the job is under way", || !fake.fetches().is_empty());
    let rev = block_on(core.call(|s| Ok(s.rev()))).unwrap();
    // The sign-ins forgotten ("remove all my data"): the job under way is called off.
    block_on(core.forget_sign_ins()).unwrap();
    let events = until(&core, |e| matches!(e, Event::FetchFailed { reason: FetchFailure::Cancelled, .. }));
    assert!(!events.iter().any(|e| matches!(e, Event::MessageFetched { .. })));
    fake.release();
    // Its answer comes, and is let go of.
    std::thread::sleep(Duration::from_millis(600));
    assert!(!is_whole(&core, m1));
    assert_eq!(block_on(core.call(|s| Ok(s.rev()))).unwrap(), rev, "nothing taken in");
}

#[test]
fn a_fetch_under_way_is_called_off_when_its_account_goes_or_the_core_shuts_down() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let (core, _, account) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    eventually("the job is under way", || !fake.fetches().is_empty());
    block_on(core.remove_account(account)).unwrap();
    until(&core, |e| matches!(e, Event::FetchFailed { message, reason: FetchFailure::Cancelled } if *message == m1));
    fake.release();

    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let (core, _, _) = core_with(run, false);
    let m2 = id_of(&core, "m2");
    assert_eq!(block_on(core.open_message(m2)).unwrap(), OpenState::Fetching);
    eventually("the job is under way", || !fake.fetches().is_empty());
    core.shutdown();
    fake.release();
    // A call after it ends rather than waiting for ever.
    assert!(block_on(core.fetch_attachment(m2, 0)).is_err());
}

#[test]
fn what_a_fetch_brings_is_held_to_its_sizes_whoever_sent_it() {
    // A message larger than asked for, one past the batch's room, and lists without end.
    let result = GraphFetchResult {
        account: 1,
        fetched: vec![
            GraphFetched { id: "big".into(), mime: vec![b'x'; 2_000] },
            GraphFetched { id: "a".into(), mime: vec![b'x'; 600] },
            GraphFetched { id: "b".into(), mime: vec![b'x'; 600] },
        ],
        missing: (0..10_000).map(|i| i.to_string()).collect(),
        errors: vec!["e".repeat(100_000); 50],
        ..Default::default()
    };
    let held = bounded(result, 1_000, 1_000);
    assert_eq!(held.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["a"], "within each message's and the batch's room");
    assert_eq!(held.too_large, ["big"]);
    assert!(held.failed.contains(&"big".to_string()));
    assert!(held.missing.len() <= 100 && held.errors.len() <= 5 && held.errors.iter().all(|e| e.len() <= 200));
}

#[test]
fn a_message_larger_than_the_phone_takes_says_so_and_stays_as_headers() {
    let serve: Arc<Serve> = Arc::new(|request: &GraphFetchRequest| GraphFetchResult {
        account: request.account,
        email: request.email.clone(),
        // A server that sends more than it was asked for: held to the request on the phone too.
        fetched: request.ids.iter().map(|id| GraphFetched { id: id.clone(), mime: vec![b'x'; request.most_bytes as usize + 1] }).collect(),
        ..Default::default()
    });
    let (run, _) = runner(serve, nothing_found(), true);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    until(&core, |e| matches!(e, Event::FetchFailed { message, reason: FetchFailure::TooLarge } if *message == m1));
    assert!(!is_whole(&core, m1));

    // Gone from the server meanwhile.
    let serve: Arc<Serve> = Arc::new(|request: &GraphFetchRequest| GraphFetchResult {
        account: request.account,
        email: request.email.clone(),
        missing: request.ids.clone(),
        ..Default::default()
    });
    let (run, _) = runner(serve, nothing_found(), true);
    let (core, _, _) = core_with(run, false);
    let m2 = id_of(&core, "m2");
    assert_eq!(block_on(core.open_message(m2)).unwrap(), OpenState::Fetching);
    until(&core, |e| matches!(e, Event::FetchFailed { message, reason: FetchFailure::Gone } if *message == m2));
}

#[test]
fn the_background_fetches_recent_mail_in_small_batches_within_the_download_limit() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, platform, _) = core_with(run, true);
    let (m1, m2, m3) = (id_of(&core, "m1"), id_of(&core, "m2"), id_of(&core, "m3"));
    // Not on a metered connection (the fake's own): nothing comes by itself.
    for _ in 0..5 {
        core.enqueue_tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fake.fetches().is_empty(), "nothing in the background on a metered connection");
    // Between rounds, by itself, with nobody opening anything, once the connection isn't metered.
    *platform.net.lock().unwrap() = NetworkKind::Online { metered: false, data_saving: false };
    core.enqueue_tick();
    eventually("the recent mail is whole", || {
        core.enqueue_tick();
        is_whole(&core, m1) && is_whole(&core, m2) && is_whole(&core, m3)
    });
    let asked = fake.fetches();
    assert!(!asked.is_empty());
    for (_, _, request) in &asked {
        assert!(request.background && request.ids.len() <= BACKGROUND_BATCH);
        // The phone's Download messages in full (1 MB), and the batch's own room.
        assert_eq!(request.most_bytes, 1 << 20);
        assert_eq!(request.most_total, BACKGROUND_TOTAL);
    }
    // Newest first.
    assert_eq!(asked[0].2.ids.first().map(String::as_str), Some("m1"));
}

#[test]
fn the_background_rests_while_the_phone_holds_all_it_may() {
    let mut s = session();
    s.run("account.deviceLimits", &json!({"messages": 2_000})).unwrap();
    assert!(room_for_more(&s));
    let folder = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Inbox && f.server.is_some()).unwrap().id;
    let account = s.mailbox.folder(folder).unwrap().account;
    for i in 0..2_001 {
        let m = mailcraft_model::Message { account, folder, subject: format!("Filler {i}"), ..Default::default() };
        s.mailbox.add_message(m).unwrap();
    }
    assert!(!room_for_more(&s), "past the cap of messages");
}

#[test]
fn an_attachment_comes_with_its_message_and_is_judged_as_the_desktop_judges_it() {
    let serve: Arc<Serve> = Arc::new(|request: &GraphFetchRequest| GraphFetchResult {
        account: request.account,
        email: request.email.clone(),
        fetched: request
            .ids
            .iter()
            .map(|id| GraphFetched { id: id.clone(), mime: with_attachment(if id == "m1" { "notes.txt" } else { "../setup.exe" }) })
            .collect(),
        ..Default::default()
    });
    let (run, fake) = runner(serve, nothing_found(), true);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    // Only its headers are here: it is fetched first, and the call waits for it.
    let file = block_on(core.fetch_attachment(m1, 0)).unwrap();
    assert_eq!((file.name.as_str(), file.verdict, file.bytes.as_slice()), ("notes.txt", AttachmentVerdict::Open, b"Hello".as_slice()));
    assert_eq!(fake.fetches().len(), 1);
    // Whole now: read at once.
    assert_eq!(block_on(core.fetch_attachment(m1, 0)).unwrap().bytes, b"Hello");
    assert_eq!(fake.fetches().len(), 1);
    assert!(matches!(block_on(core.fetch_attachment(m1, 7)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.fetch_attachment(m1, u32::MAX)), Err(CoreError::BadRequest { .. })));
    // A program is never to be opened, and its name is made safe.
    let m2 = id_of(&core, "m2");
    let program = block_on(core.fetch_attachment(m2, 0)).unwrap();
    assert_eq!(program.verdict, AttachmentVerdict::SaveOnly);
    assert!(!program.name.contains('/') && !program.name.starts_with('.'), "{}", program.name);
}

#[test]
fn a_search_on_the_server_brings_older_mail_in_as_headers_kept_for_the_session() {
    let found: Arc<Find> = Arc::new(|query: &str, next: &str| {
        assert_eq!(query, "budget");
        if next.is_empty() {
            GraphSearchResult {
                email: EMAIL.into(),
                // One on the phone already, one a year old in the Archive, one in a folder the phone hasn't.
                messages: vec![
                    message("m1", "Budget review", now() - DAY, "in"),
                    message("old", "Budget 2025", now() - 365 * DAY, "arch"),
                    message("lost", "Budget elsewhere", now() - 400 * DAY, "nowhere"),
                ],
                next: "page-2".into(),
                ..Default::default()
            }
        } else {
            assert_eq!(next, "page-2");
            GraphSearchResult {
                email: EMAIL.into(),
                messages: vec![message("older", "Budget 2024", now() - 700 * DAY, "arch")],
                ..Default::default()
            }
        }
    });
    let (run, fake) = runner(Arc::new(whole), found, true);
    let (core, _, account) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    let answer = block_on(core.search_server(account, "budget".into())).unwrap();
    let old = id_of(&core, "old");
    assert_eq!(answer.messages, [m1, old], "newest first, the one here already among them");
    assert!(answer.more && answer.left_out == 1 && !answer.offline, "{answer:?}");
    // As headers, and kept on the phone this session although older than its window.
    assert!(!is_whole(&core, old));
    block_on(core.execute("account.trimDevice".into(), String::new())).unwrap();
    assert!(block_on(core.call(move |s| Ok(s.mailbox.message(old).is_some() && s.kept_for_now(old)))).unwrap(), "trimming kept it");

    let more = block_on(core.search_server_more(account)).unwrap();
    assert_eq!(more.messages, [id_of(&core, "older")]);
    assert!(!more.more);
    assert!(matches!(block_on(core.search_server_more(account)), Err(CoreError::BadRequest { .. })), "nothing more to ask for");
    assert!(matches!(&fake.asked.lock().unwrap()[1], Asked::Search { next } if next == "page-2"));

    // Opening one fetches it whole, as any other.
    assert_eq!(block_on(core.open_message(old)).unwrap(), OpenState::Fetching);
    until(&core, |e| matches!(e, Event::MessageFetched { message } if *message == old));
}

#[test]
fn a_search_that_cant_be_asked_is_refused_and_never_a_crash() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, platform, account) = core_with(run, false);
    assert!(matches!(block_on(core.search_server(account, "   ".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.search_server(account, "x".repeat(100_000))), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.search_server(account, "is:unread".into())), Err(CoreError::Failed { .. })), "nothing for the server");
    assert!(matches!(block_on(core.search_server(account + 99, "budget".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.search_server_more(account)), Err(CoreError::BadRequest { .. })));
    *platform.net.lock().unwrap() = NetworkKind::Offline;
    assert!(matches!(block_on(core.search_server(account, "budget".into())), Err(CoreError::Failed { .. })));
    assert!(fake.asked.lock().unwrap().is_empty());
}

#[test]
fn opens_wait_their_turn_one_job_at_a_time_and_none_is_asked_twice() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    eventually("the first job is under way", || !fake.fetches().is_empty());
    // The same message again is not asked twice; the others wait their turn.
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    let (m2, m3) = (id_of(&core, "m2"), id_of(&core, "m3"));
    assert_eq!(block_on(core.open_message(m2)).unwrap(), OpenState::Fetching);
    assert_eq!(block_on(core.open_message(m3)).unwrap(), OpenState::Fetching);
    assert_eq!(fake.fetches().len(), 1, "one job at a time for the account");
    fake.release();
    eventually("each came in its turn", || is_whole(&core, m1) && is_whole(&core, m2) && is_whole(&core, m3));
    let asked: Vec<Vec<String>> = fake.fetches().into_iter().map(|(_, _, r)| r.ids).collect();
    assert_eq!(asked.iter().filter(|ids| ids.contains(&"m1".to_string())).count(), 1, "{asked:?}");
}

#[test]
fn hostile_ids_and_indexes_are_refused_without_a_crash() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, _, _) = core_with(run, false);
    for id in [0, u64::MAX, 1 << 53] {
        assert!(matches!(block_on(core.open_message(id)), Err(CoreError::BadRequest { .. })));
        assert!(block_on(core.fetch_attachment(id, 0)).is_err());
    }
    assert!(fake.fetches().is_empty());
}
