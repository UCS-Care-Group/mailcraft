// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A background job's answer belongs to the mailbox opening that started it (AU3-08, AU3-09,
//! AU3-10, AU3-21, AU3-V8, H6), and a message handed to a server is never sent again by itself
//! when the answer is lost (AU3-26).
//!
//! One row for every [`JobKind`]: the job starts under mailbox A, mailbox B takes its place with
//! the same accounts (ids and addresses alike), and A's answer comes. B stays byte for byte as it
//! was and nothing is remembered from the answer; and the frame's own look at the replacement
//! calls the job off. Then, for each protocol that sends, a send in flight whose answer is lost
//! (another mailbox opened meanwhile, or the program killed) leaves the message held at the next
//! run, not sent again. Hosts are fakes that keep every job for the test to answer; every address
//! is invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
use mailcraft_engine::cmd::graph_send::MAY_HAVE_GONE;
use mailcraft_model::remote::EventsDownload;
use mailcraft_model::remote_rules::RulesDownload;
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use mailcraft_model::sync::{
    EwsMailResult, FolderListing, GraphFolder, GraphMailResult, GraphMessage, JmapMailResult, MessagePage, SendDone, SendOutcome, ServerMessage,
    SyncRequest, SyncResult, Tokens,
};
use mailcraft_model::{Address, FolderKind, Id, Mailbox, Message};
use serde_json::json;

use crate::auto_replies::{AutoRepliesJob, AutoRepliesResult};
use crate::ews_mail::EwsJob;
use crate::jmap_mail::JmapJob;
use crate::jobs::JobKind;
use crate::rule_sync::{RulesJob, RulesResult};
use crate::task_sync::{TasksJob, TasksResult};
use crate::{
    AdminConsentAnswer, AdminConsentRequest, CalendarCheckRequest, CalendarCheckResult, FreeBusyRequest, FreeBusyResult, GraphRequest, GraphResult,
    MailApp, SecretStore, Services, SignInRequest,
};

const NOW: i64 = 1_791_993_600;
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const JMAP: &str = "https://jmap.example.org/jmap/session";
/// The access token every stale answer brings: never kept.
const STALE: &str = "stale-access";

type Kept<R, A> = Arc<Mutex<Vec<(R, mpsc::Sender<A>)>>>;

/// Every job the hosts were handed, with the end each answers through.
#[derive(Clone, Default)]
struct Hosts {
    sync: Kept<SyncRequest, SyncResult>,
    graph: Kept<GraphRequest, GraphResult>,
    ews: Kept<EwsJob, EwsMailResult>,
    jmap: Kept<JmapJob, JmapMailResult>,
    tasks: Kept<TasksJob, TasksResult>,
    rules: Kept<RulesJob, RulesResult>,
    replies: Kept<AutoRepliesJob, AutoRepliesResult>,
    free_busy: Kept<FreeBusyRequest, FreeBusyResult>,
    check: Kept<CalendarCheckRequest, CalendarCheckResult>,
    sign_in: Kept<SignInRequest, Result<Tokens, String>>,
    admin_consent: Kept<AdminConsentRequest, AdminConsentAnswer>,
}

fn take<R, A>(kept: &Kept<R, A>) -> (R, mpsc::Sender<A>) {
    kept.lock().unwrap().pop().expect("the host was handed a job")
}

macro_rules! keep {
    ($slot:expr, $kept:expr) => {{
        let kept = $kept.clone();
        $slot = Some(Box::new(move |request| {
            let (tx, rx) = mpsc::channel();
            kept.lock().unwrap().push((request, tx));
            rx
        }));
    }};
}

/// The system's credential store, counting what it was asked to keep.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<usize>>);

impl SecretStore for Store {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
    fn delete(&self, _: &str) {}
}

impl Store {
    fn writes(&self) -> usize {
        *self.0.lock().unwrap()
    }
}

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// What every stale answer says of its sign-in: a refresh token and nobody's address (H6).
fn stale_tokens() -> Tokens {
    tokens(STALE, "stale-refresh")
}

/// The accounts of the mailboxes.
#[derive(Clone, Copy)]
struct Accounts {
    imap: Id,
    microsoft: Id,
    shared: Id,
    ews: Id,
    jmap: Id,
    /// An IMAP account that signs in with Microsoft.
    oauth: Id,
}

/// A window with every kind of account, every host keeping its jobs, and a credential store that
/// counts what it keeps.
fn window() -> (MailApp, Accounts, Hosts, Store) {
    let (mut app, hosts, store) = hosted(Mailbox::with_local_account("", ""));
    let add = |app: &mut MailApp, params: serde_json::Value| app.execute("account.add", params).unwrap()["id"].as_u64().unwrap();
    let imap = add(
        &mut app,
        json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org", "port": 993}, "outgoing": {"host": "smtp.example.org", "port": 465}}),
    );
    let microsoft = add(&mut app, json!({"email": "noor@example.org", "kind": "microsoft"}));
    let shared =
        app.execute("account.addSharedMailbox", json!({"account": microsoft, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    let ews = add(&mut app, json!({"email": "ewa@example.org", "kind": "exchange", "ews": {"url": EWS, "username": "CORP\\ewa"}}));
    let jmap = add(&mut app, json!({"email": "jo@example.org", "kind": "jmap", "jmap": {"session": JMAP}}));
    let oauth =
        add(&mut app, json!({"email": "avery@example.org", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.example.org"}}));
    for (account, email) in [(microsoft, "noor@example.org"), (shared, "support@example.org")] {
        seed_inbox(&mut app, account, email);
    }
    let ids = Accounts { imap, microsoft, shared, ews, jmap, oauth };
    sign_in(&mut app, ids);
    (app, ids, hosts, store)
}

/// The passwords typed and the sign-ins made this session.
fn sign_in(app: &mut MailApp, ids: Accounts) {
    for account in [ids.imap, ids.ews, ids.jmap] {
        app.passwords.insert(account, "invented-password".into());
    }
    for account in [ids.microsoft, ids.oauth] {
        app.tokens.insert(account, tokens("own-access", "ref-1"));
        app.graph_tokens.insert(account, tokens("own-access", "ref-1"));
    }
}

/// A window on `mailbox` whose every host keeps its jobs, with a credential store that counts
/// what it keeps.
fn hosted(mailbox: Mailbox) -> (MailApp, Hosts, Store) {
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    let store = Store::default();
    app.services.secrets = Some(Arc::new(store.clone()));
    let hosts = Hosts::default();
    keep!(app.services.sync, hosts.sync);
    keep!(app.services.graph, hosts.graph);
    keep!(app.services.ews, hosts.ews);
    keep!(app.services.jmap, hosts.jmap);
    keep!(app.services.graph_tasks, hosts.tasks);
    keep!(app.services.graph_rules, hosts.rules);
    keep!(app.services.graph_auto_replies, hosts.replies);
    keep!(app.services.free_busy, hosts.free_busy);
    keep!(app.services.check_calendar, hosts.check);
    keep!(app.services.sign_in, hosts.sign_in);
    keep!(app.services.admin_consent, hosts.admin_consent);
    (app, hosts, store)
}

/// Give `account` an Inbox known to the server, with a delta link.
fn seed_inbox(app: &mut MailApp, account: Id, email: &str) {
    let folders = vec![GraphFolder { id: format!("in-{account}"), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let link = format!("https://graph.microsoft.com/v1.0/me/mailFolders/in-{account}/messages/delta?$deltatoken=one");
    let pages = vec![MessagePage { folder: format!("in-{account}"), link, caught_up: true, ..Default::default() }];
    let seed =
        GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), pages, ..Default::default() };
    apply_graph_mail(&mut app.session, seed).unwrap();
}

/// A message waiting in `account`'s Outbox.
fn queued(app: &mut MailApp, account: Id, subject: &str) -> Id {
    let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).expect("an Outbox");
    let m = Message {
        account,
        folder: outbox,
        subject: subject.into(),
        to: vec![Address::new("", "kit@example.net")],
        message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    app.session.mailbox_changed();
    id
}

/// A new message in `account`'s Inbox, as a piece of a Graph run brings it.
fn new_mail(account: Id, email: &str) -> GraphMailResult {
    let message = GraphMessage {
        server: ServerMessage { id: "m-new".into(), folder: format!("in-{account}"), ..Default::default() },
        subject: "From the closed mailbox".into(),
        received: NOW,
        sent: NOW,
        ..Default::default()
    };
    let link = format!("https://graph.microsoft.com/v1.0/me/mailFolders/in-{account}/messages/delta?$deltatoken=two");
    let page = MessagePage { folder: format!("in-{account}"), link, caught_up: true, messages: vec![message], ..Default::default() };
    GraphMailResult { account, email: email.into(), pages: vec![page], ..Default::default() }
}

fn sent(message: Id) -> SendDone {
    SendDone { message, outcome: SendOutcome::Sent, draft: None, draft_left: String::new() }
}

/// The open mailbox, as its file has it.
fn bytes(app: &MailApp) -> Vec<u8> {
    mailcraft_engine::io::write_mailbox(&app.session.mailbox).unwrap()
}

/// How a row's answer is let in: the job's own poll straight after the replacement (before any
/// frame has looked), or the frame's look at the replacement first.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Way {
    Poll,
    Frame,
}

/// Start a job of `kind` under mailbox A, put B (A's twin, as it was before the job) in its place,
/// let A's answer in `way`, and check B is untouched and nothing was remembered.
fn row(kind: JobKind, way: Way) {
    let (mut app, ids, hosts, store) = window();
    // What the row's job sends, when it is a Send/Receive: in A's Outbox and B's alike.
    let outbox_of = match kind {
        JobKind::Imap => Some(ids.imap),
        JobKind::Graph => Some(ids.microsoft),
        JobKind::Ews => Some(ids.ews),
        JobKind::Jmap => Some(ids.jmap),
        JobKind::Shared => Some(ids.shared),
        _ => None,
    };
    let waiting = outbox_of.map(|account| queued(&mut app, account, "Quarterly figures"));
    let mut calendar = 0;
    if kind == JobKind::FreeBusy {
        calendar = free_busy_calendar(&mut app, ids.microsoft);
    }
    if kind == JobKind::SignIn {
        app.tokens.remove(&ids.oauth);
        app.graph_tokens.remove(&ids.oauth);
    }
    let twin = app.session.mailbox.clone();
    // Start it.
    match kind {
        JobKind::Imap => assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true),
        JobKind::Graph => assert_eq!(app.run("ui.sync", json!({"account": ids.microsoft})).unwrap()["started"], true),
        JobKind::Ews => assert_eq!(app.run("ui.sync", json!({"account": ids.ews})).unwrap()["started"], true),
        JobKind::Jmap => assert_eq!(app.run("ui.sync", json!({"account": ids.jmap})).unwrap()["started"], true),
        JobKind::Inbox => assert!(app.start_inbox_pass(ids.microsoft, true, true).is_some()),
        JobKind::Shared => assert_eq!(app.run("ui.sync", json!({"account": ids.shared})).unwrap()["started"], true),
        JobKind::Calendar => assert!(app.calendar_tick_at(true, crate::now_ms())),
        JobKind::CalendarCheck => {
            app.execute("ui.checkCalendar", json!({"account": ids.microsoft, "confirmed": true})).unwrap();
        }
        JobKind::FreeBusy => {
            let day = mailcraft_time::Date::new(2026, 10, 14).unwrap();
            assert!(app.free_busy_at(calendar, &["kit@example.net".to_string()], day, crate::now_ms()).pending);
        }
        JobKind::Tasks => assert!(app.tasks_tick_at(0.0)),
        JobKind::Rules => assert!(app.rules_tick_at(0.0)),
        JobKind::AutoReplies => assert!(app.auto_replies_tick_at(0.0)),
        JobKind::SignIn => assert_eq!(app.run("ui.sync", json!({"account": ids.oauth, "secretsAsked": true})).unwrap()["pending"], "signIn"),
        JobKind::AccountSetup => {
            app.dialog = crate::dialogs::Dialog::open("account", &app, &json!({}));
            assert!(app.dialog.is_some());
        }
        JobKind::AdminConsent => {
            assert_eq!(app.run("ui.adminConsent", json!({"account": ids.microsoft})).unwrap()["pending"], "adminConsent");
        }
    }
    assert!(app.job_running(kind), "{kind:?}: the job is under way");
    // B takes A's place.
    app.session.replace_mailbox(twin);
    let before = bytes(&app);
    let writes = store.writes();
    if way == Way::Frame {
        app.drop_secrets_if_replaced();
        assert!(!app.job_running(kind), "{kind:?}: the job is called off with the mailbox it was for");
        assert!(!app.session.sync_progress.running(), "{kind:?}: Send/Receive Progress has no row of A's left");
    }
    // A's answer comes, and is let in.
    let stale = stale_tokens();
    match kind {
        JobKind::Imap => {
            let (_, tx) = take(&hosts.sync);
            let _ = tx.send(SyncResult { account: ids.imap, sent: waiting.into_iter().collect(), partial: false, ..Default::default() });
            app.poll_sync();
        }
        JobKind::Graph | JobKind::Shared => {
            let (mut request, tx) = take(&hosts.graph);
            let account = request.account.id;
            if let Some(pieces) = request.mail_pieces.take() {
                let piece = GraphMailResult { sent: waiting.into_iter().map(sent).collect(), ..new_mail(account, &request.account.email) };
                let _ = pieces.send(piece);
            }
            let _ = tx.send(GraphResult { account, tokens: Some(stale), ..Default::default() });
            for _ in 0..4 {
                app.poll_graph();
            }
        }
        JobKind::Ews => {
            let (_, tx) = take(&hosts.ews);
            let _ = tx.send(EwsMailResult { account: ids.ews, sent: waiting.into_iter().map(sent).collect(), ..Default::default() });
            app.poll_sync();
        }
        JobKind::Jmap => {
            let (_, tx) = take(&hosts.jmap);
            let _ = tx.send(JmapMailResult { account: ids.jmap, sent: waiting.into_iter().map(sent).collect(), ..Default::default() });
            app.poll_sync();
        }
        JobKind::Inbox => {
            let (mut request, tx) = take(&hosts.graph);
            if let Some(pieces) = request.mail_pieces.take() {
                let _ = pieces.send(new_mail(ids.microsoft, "noor@example.org"));
            }
            let _ = tx.send(GraphResult { account: ids.microsoft, tokens: Some(stale), ..Default::default() });
            for _ in 0..4 {
                app.poll_graph();
            }
        }
        JobKind::Calendar => {
            let (request, tx) = take(&hosts.graph);
            let (from, to) = request.calendar.unwrap_or((NOW - 86_400, NOW + 86_400));
            let event = crate::calendar_sync::testing::event("e-stale", "From the closed mailbox", NOW + 3_600);
            let events = EventsDownload { from, to, complete: true, events: vec![event], ..Default::default() }.for_account(&request.account);
            let _ = tx.send(GraphResult { account: request.account.id, tokens: Some(stale), events: Some(events), ..Default::default() });
            app.poll_calendar();
        }
        JobKind::CalendarCheck => {
            let (request, tx) = take(&hosts.check);
            let _ = tx.send(CalendarCheckResult { account: request.account.id, tokens: Some(stale), ok: true, ..Default::default() });
            app.poll_calendar_check();
        }
        JobKind::FreeBusy => {
            let (request, tx) = take(&hosts.free_busy);
            let _ = tx.send(FreeBusyResult { account: request.account.id, tokens: Some(stale), ..Default::default() });
            app.poll_free_busy();
        }
        JobKind::Tasks => {
            let (job, tx) = take(&hosts.tasks);
            let download = TasksDownload {
                account: job.account.id,
                email: String::new(),
                complete: true,
                lists: vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }],
                tasks: vec![RemoteTask { id: "t-stale".into(), list: "L1".into(), title: "From the closed mailbox".into(), ..Default::default() }],
            };
            let _ = tx.send(TasksResult { account: job.account.id, tokens: Some(stale), download: Some(download), ..Default::default() });
            app.poll_tasks();
        }
        JobKind::Rules => {
            let (job, tx) = take(&hosts.rules);
            let download = RulesDownload { account: job.account.id, email: job.account.email.clone(), complete: true, ..Default::default() };
            let _ = tx.send(RulesResult { account: job.account.id, tokens: Some(stale), download: Some(download), ..Default::default() });
            app.poll_rules();
        }
        JobKind::AutoReplies => {
            let (job, tx) = take(&hosts.replies);
            let _ = tx.send(AutoRepliesResult { account: job.account.id, tokens: Some(stale), ..Default::default() });
            app.poll_auto_replies();
        }
        JobKind::SignIn => {
            let (_, tx) = take(&hosts.sign_in);
            let _ = tx.send(Ok(stale));
            app.poll_sign_in();
        }
        // (What the dialog asked of the host goes with it: there is no answer left to let in.)
        JobKind::AccountSetup => {}
        JobKind::AdminConsent => {
            let (_, tx) = take(&hosts.admin_consent);
            let _ = tx.send(AdminConsentAnswer::Approved);
            app.poll_admin_consent();
            // (An approval of A's is no reason to sign B's account in again.)
            assert!(!app.job_running(kind), "{way:?}: A's approval is let go of, not kept for B's account");
            assert!(hosts.graph.lock().unwrap().is_empty() && hosts.sign_in.lock().unwrap().is_empty(), "{way:?}: nothing started");
        }
    }
    assert!(bytes(&app) == before, "{kind:?} ({way:?}): the mailbox that took A's place is as it was");
    assert_eq!(store.writes(), writes, "{kind:?} ({way:?}): nothing of A's answer is remembered");
    let kept_stale = app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.access == STALE);
    assert!(!kept_stale, "{kind:?} ({way:?}): A's sign-in is not kept for B's accounts");
}

/// A calendar of `account`'s server, as its first job brought it.
fn free_busy_calendar(app: &mut MailApp, account: Id) -> Id {
    mailcraft_engine::cmd::remote::ensure_shared_calendars(&mut app.session);
    let mb = &mut app.session.mailbox;
    if let Some(c) = mb.calendars.iter().find(|c| c.origin.account == account) {
        return c.id;
    }
    let id = mb.calendars.iter().map(|c| c.id).chain(mb.messages.keys().copied()).max().unwrap_or(0) + 1_000;
    let calendar = mailcraft_model::Calendar {
        id,
        name: "Calendar".into(),
        visible: true,
        origin: mailcraft_model::remote::Origin { account, ..Default::default() },
        ..Default::default()
    };
    mb.calendars.push(calendar);
    app.session.mailbox_changed();
    id
}

/// How each kind's answer is let in. (Exhaustive: a new kind of job has no row until it is given
/// one here.)
fn ways(kind: JobKind) -> &'static [Way] {
    match kind {
        JobKind::Imap
        | JobKind::Graph
        | JobKind::Ews
        | JobKind::Jmap
        | JobKind::Inbox
        | JobKind::Shared
        | JobKind::Calendar
        | JobKind::CalendarCheck
        | JobKind::FreeBusy
        | JobKind::Tasks
        | JobKind::Rules
        | JobKind::AutoReplies
        | JobKind::SignIn
        | JobKind::AdminConsent => &[Way::Poll, Way::Frame],
        // The dialog's answers are taken in by the dialog, in a frame.
        JobKind::AccountSetup => &[Way::Frame],
    }
}

macro_rules! rows {
    ($($name:ident => $kind:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                for way in ways(JobKind::$kind) {
                    row(JobKind::$kind, *way);
                }
            }
        )*

        #[test]
        fn every_job_kind_has_a_row() {
            let rows = [$(JobKind::$kind),*];
            for kind in JobKind::ALL {
                assert!(rows.contains(kind), "{kind:?} has no row");
            }
        }
    };
}

rows! {
    an_imap_run_answer_for_a_replaced_mailbox_is_let_go => Imap,
    a_graph_run_answer_for_a_replaced_mailbox_is_let_go => Graph,
    an_ews_run_answer_for_a_replaced_mailbox_is_let_go => Ews,
    a_jmap_run_answer_for_a_replaced_mailbox_is_let_go => Jmap,
    an_inbox_pass_answer_for_a_replaced_mailbox_is_let_go => Inbox,
    a_shared_mailbox_run_answer_for_a_replaced_mailbox_is_let_go => Shared,
    a_calendar_answer_for_a_replaced_mailbox_is_let_go => Calendar,
    a_calendar_check_answer_for_a_replaced_mailbox_is_let_go => CalendarCheck,
    a_free_busy_answer_for_a_replaced_mailbox_is_let_go => FreeBusy,
    a_tasks_answer_for_a_replaced_mailbox_is_let_go => Tasks,
    a_rules_answer_for_a_replaced_mailbox_is_let_go => Rules,
    an_auto_replies_answer_for_a_replaced_mailbox_is_let_go => AutoReplies,
    a_sign_in_answer_for_a_replaced_mailbox_is_let_go => SignIn,
    the_add_account_dialog_goes_with_a_replaced_mailbox => AccountSetup,
    an_approval_answer_for_a_replaced_mailbox_is_let_go => AdminConsent,
}

// ---- sending (AU3-26) ----

/// The protocols that send.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sends {
    Imap,
    Graph,
    Ews,
}

/// What became of the run whose send was in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lost {
    /// Another mailbox was opened, its answer came and was let go of, and the old file was opened
    /// again.
    Replaced,
    /// The program was killed mid-send, and started again on the file.
    Killed,
}

fn sending_account(ids: Accounts, protocol: Sends) -> Id {
    match protocol {
        Sends::Imap => ids.imap,
        Sends::Graph => ids.microsoft,
        Sends::Ews => ids.ews,
    }
}

/// What a run of `protocol` the hosts were handed would send, and the end its answer goes through
/// (taken, so that a send left unanswered is dropped).
fn handed(hosts: &Hosts, protocol: Sends) -> (Vec<Id>, Box<dyn FnOnce(Id)>) {
    match protocol {
        Sends::Imap => {
            let (request, tx) = take(&hosts.sync);
            let account = request.account.id;
            let ids = request.outbox.iter().map(|m| m.id).collect();
            (ids, Box::new(move |m| drop(tx.send(SyncResult { account, sent: vec![m], partial: false, ..Default::default() }))))
        }
        Sends::Graph => {
            let (mut request, tx) = take(&hosts.graph);
            let account = request.account.id;
            let ids = request.mail.as_ref().map(|mail| mail.sends.iter().map(|up| up.message).collect()).unwrap_or_default();
            let pieces = request.mail_pieces.take();
            (
                ids,
                Box::new(move |m| {
                    if let Some(pieces) = pieces {
                        let _ = pieces.send(GraphMailResult { account, sent: vec![sent(m)], ..Default::default() });
                    }
                    let _ = tx.send(GraphResult { account, ..Default::default() });
                }),
            )
        }
        Sends::Ews => {
            let (job, tx) = take(&hosts.ews);
            let account = job.request.account;
            let ids = job.request.sends.iter().map(|up| up.message).collect();
            (ids, Box::new(move |m| drop(tx.send(EwsMailResult { account, sent: vec![sent(m)], ..Default::default() }))))
        }
    }
}

fn poll_run(app: &mut MailApp) {
    for _ in 0..4 {
        app.poll_graph();
        app.poll_sync();
    }
}

/// Send a message with `protocol`, lose the run's answer `lost`'s way, and Send/Receive again: the
/// message is held as one that may have gone, and is not handed to the server a second time.
fn send_row(protocol: Sends, lost: Lost) {
    let (mut app, ids, hosts, _store) = window();
    // The mailbox is kept where the host keeps it: each state written is noted.
    let written: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let note = written.clone();
    app.services.persist = Some(Box::new(move |bytes: &[u8]| {
        note.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }));
    app.autosave = true;
    let account = sending_account(ids, protocol);
    let message = queued(&mut app, account, "Signed contract");
    assert_eq!(app.run("ui.sync", json!({"account": account})).unwrap()["started"], true, "{protocol:?}");
    let (going, answer) = handed(&hosts, protocol);
    assert_eq!(going, [message], "{protocol:?}: the message goes with the run");
    // Marked as being sent, on disk, before the run was handed to the host.
    let saved = written.lock().unwrap().last().cloned().expect("the mailbox was saved before the send left");
    let on_disk = mailcraft_engine::io::read_mailbox(&saved).unwrap();
    assert!(on_disk.message(message).is_some_and(|m| m.sending), "{protocol:?}: the mark is on disk");
    let (mut app, hosts) = match lost {
        Lost::Replaced => {
            // Another mailbox is opened; the run's answer ("sent") comes, and is let go of.
            app.session.replace_mailbox(Mailbox::with_local_account("", ""));
            app.drop_secrets_if_replaced();
            answer(message);
            poll_run(&mut app);
            // The old file is opened again, and its servers confirmed.
            app.session.replace_mailbox(on_disk);
            app.drop_secrets_if_replaced();
            let _ = app.confirm_servers();
            (app, hosts)
        }
        Lost::Killed => {
            drop(answer);
            let (app, hosts, _) = hosted(on_disk);
            (app, hosts)
        }
    };
    sign_in(&mut app, ids);
    let again = app.run("ui.sync", json!({"account": account}));
    let m = app.session.mailbox.message(message).expect("still in the Outbox");
    assert_eq!((m.held.as_str(), m.sending), (MAY_HAVE_GONE, false), "{protocol:?} ({lost:?}): held as one that may have gone");
    // Whatever the run takes, the message isn't among it.
    if again.is_ok_and(|v| v["started"] == true) {
        let (going, _) = handed(&hosts, protocol);
        assert!(!going.contains(&message), "{protocol:?} ({lost:?}): not sent again by itself");
    }
}

#[test]
fn an_imap_send_whose_answer_was_lost_to_another_mailbox_is_held_not_sent_again() {
    send_row(Sends::Imap, Lost::Replaced);
}

#[test]
fn a_graph_send_whose_answer_was_lost_to_another_mailbox_is_held_not_sent_again() {
    send_row(Sends::Graph, Lost::Replaced);
}

#[test]
fn an_ews_send_whose_answer_was_lost_to_another_mailbox_is_held_not_sent_again() {
    send_row(Sends::Ews, Lost::Replaced);
}

#[test]
fn an_imap_send_cut_short_by_a_kill_is_held_not_sent_again() {
    send_row(Sends::Imap, Lost::Killed);
}

#[test]
fn a_graph_send_cut_short_by_a_kill_is_held_not_sent_again() {
    send_row(Sends::Graph, Lost::Killed);
}

#[test]
fn an_ews_send_cut_short_by_a_kill_is_held_not_sent_again() {
    send_row(Sends::Ews, Lost::Killed);
}

/// A send whose answer was taken in leaves no mark: the next run is an ordinary one.
#[test]
fn a_send_whose_answer_came_leaves_no_mark() {
    let (mut app, ids, hosts, _store) = window();
    let message = queued(&mut app, ids.imap, "Minutes");
    let other = queued(&mut app, ids.imap, "Agenda");
    assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true);
    let (going, answer) = handed(&hosts, Sends::Imap);
    assert_eq!(going, [message, other]);
    answer(message);
    poll_run(&mut app);
    let outbox = app.session.mailbox.folder_of_kind(ids.imap, FolderKind::Outbox);
    assert!(app.session.mailbox.message(message).is_none_or(|m| Some(m.folder) != outbox), "sent, and gone from the Outbox");
    let m = app.session.mailbox.message(other).unwrap();
    assert_eq!((m.held.as_str(), m.sending), ("", false), "the one the server didn't take waits for the next run");
}

/// The server took the message and the piece that said so was taken in (filed in Sent Items),
/// and then the run was lost: the next run holds nothing, since it is out of the Outbox.
#[test]
fn a_send_taken_in_before_the_run_was_lost_is_not_held() {
    let (mut app, ids, hosts, _store) = window();
    let message = queued(&mut app, ids.imap, "Receipt");
    assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true);
    let (request, tx) = take(&hosts.sync);
    assert_eq!(request.outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [message]);
    let _ = tx.send(SyncResult { account: ids.imap, sent: vec![message], partial: true, ..Default::default() });
    poll_run(&mut app);
    // Killed here, with what was taken in on disk.
    drop(tx);
    let on_disk = mailcraft_engine::io::read_mailbox(&bytes(&app)).unwrap();
    let (mut app, _hosts, _) = hosted(on_disk);
    sign_in(&mut app, ids);
    let _ = app.run("ui.sync", json!({"account": ids.imap}));
    let m = app.session.mailbox.message(message).expect("filed in Sent Items");
    let sent_items = app.session.mailbox.folder_of_kind(ids.imap, FolderKind::Sent);
    assert_eq!((Some(m.folder), m.held.as_str(), m.sending), (sent_items, "", false));
}

/// A run whose answer was refused (it was for another account) leaves what it sent marked: the
/// next run holds it as one that may have gone, and doesn't hand it over again.
fn refused_row(protocol: Sends) {
    let (mut app, ids, hosts, _store) = window();
    let account = sending_account(ids, protocol);
    let message = queued(&mut app, account, "Board pack");
    assert_eq!(app.run("ui.sync", json!({"account": account})).unwrap()["started"], true, "{protocol:?}");
    let stranger = "mallory@example.net".to_string();
    match protocol {
        Sends::Imap => {
            let (_, tx) = take(&hosts.sync);
            let _ = tx.send(SyncResult { account, email: stranger, partial: false, ..Default::default() });
        }
        Sends::Graph => {
            let (mut request, tx) = take(&hosts.graph);
            if let Some(pieces) = request.mail_pieces.take() {
                let _ = pieces.send(GraphMailResult { account, email: stranger, ..Default::default() });
            }
            let _ = tx.send(GraphResult { account, ..Default::default() });
        }
        Sends::Ews => {
            let (_, tx) = take(&hosts.ews);
            let _ = tx.send(EwsMailResult { account, email: stranger, url: EWS.into(), partial: false, ..Default::default() });
        }
    }
    poll_run(&mut app);
    assert!(app.session.mailbox.message(message).is_some_and(|m| m.sending), "{protocol:?}: the mark is left");
    let again = app.run("ui.sync", json!({"account": account}));
    let m = app.session.mailbox.message(message).expect("still in the Outbox");
    assert_eq!((m.held.as_str(), m.sending), (MAY_HAVE_GONE, false), "{protocol:?}: held as one that may have gone");
    if again.is_ok_and(|v| v["started"] == true) {
        let (going, _) = handed(&hosts, protocol);
        assert!(!going.contains(&message), "{protocol:?}: not sent again by itself");
    }
}

#[test]
fn an_imap_send_whose_answer_was_refused_is_held_not_sent_again() {
    refused_row(Sends::Imap);
}

#[test]
fn a_graph_send_whose_answer_was_refused_is_held_not_sent_again() {
    refused_row(Sends::Graph);
}

#[test]
fn an_ews_send_whose_answer_was_refused_is_held_not_sent_again() {
    refused_row(Sends::Ews);
}

/// An Exchange server's answer to a send that can't be taken in (an id that isn't one) is never
/// dropped: the message may have gone, and is held.
#[test]
fn an_ews_send_whose_answer_cant_be_taken_in_is_held() {
    let (mut app, ids, hosts, _store) = window();
    let message = queued(&mut app, ids.ews, "Rota");
    assert_eq!(app.run("ui.sync", json!({"account": ids.ews})).unwrap()["started"], true);
    let (_, tx) = take(&hosts.ews);
    let done = SendDone { draft_left: "bad\nid".into(), ..sent(message) };
    let _ = tx.send(EwsMailResult { account: ids.ews, sent: vec![done], partial: false, ..Default::default() });
    poll_run(&mut app);
    let m = app.session.mailbox.message(message).expect("still in the Outbox");
    assert!(m.held.starts_with("It may have been sent") && !m.sending, "held: {:?}", m.held);
}

/// The two other ways a send goes through Graph: an IMAP account that signs in with Microsoft
/// sends through Graph before its mail, and a shared mailbox sends at its own address. Each is
/// marked on disk before it leaves, and held when its answer is lost.
#[test]
fn graph_sends_of_imap_and_shared_mailboxes_are_marked_and_held() {
    for (shared, subject) in [(false, "Timesheet"), (true, "Support reply")] {
        let (mut app, ids, hosts, _store) = window();
        let written: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
        let note = written.clone();
        app.services.persist = Some(Box::new(move |bytes: &[u8]| {
            note.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }));
        app.autosave = true;
        let account = if shared { ids.shared } else { ids.oauth };
        let message = queued(&mut app, account, subject);
        let started = app.run("ui.sync", json!({"account": account})).unwrap();
        assert!(started["started"] == true || started["pending"] == "graph", "shared: {shared}: {started}");
        let (request, tx) = take(&hosts.graph);
        let going: Vec<Id> = if shared {
            request.mail.as_ref().map(|mail| mail.sends.iter().map(|up| up.message).collect()).unwrap_or_default()
        } else {
            request.outbox.iter().map(|m| m.id).collect()
        };
        assert_eq!(going, [message], "shared: {shared}: it goes through Graph");
        let saved = written.lock().unwrap().last().cloned().expect("saved before the send left");
        let on_disk = mailcraft_engine::io::read_mailbox(&saved).unwrap();
        assert!(on_disk.message(message).is_some_and(|m| m.sending), "shared: {shared}: the mark is on disk");
        // Killed mid-send.
        drop((request, tx));
        let (mut app, _hosts, _) = hosted(on_disk);
        sign_in(&mut app, ids);
        let _ = app.run("ui.sync", json!({"account": account}));
        let m = app.session.mailbox.message(message).expect("still in the Outbox");
        assert_eq!((m.held.as_str(), m.sending), (MAY_HAVE_GONE, false), "shared: {shared}: held");
    }
}

/// An IMAP account that signs in with Microsoft sends through Graph first. A send whose answer
/// was lost after it was handed over (a timeout) may have gone: it is held, and neither the IMAP
/// leg nor the next run sends it again. One Microsoft said to try later (429) goes next run.
#[test]
fn a_graph_send_before_imap_whose_answer_was_lost_is_held_and_a_busy_one_goes_again() {
    for lost in [true, false] {
        let (mut app, ids, hosts, _store) = window();
        let message = queued(&mut app, ids.oauth, "Purchase order");
        assert_eq!(app.run("ui.sync", json!({"account": ids.oauth})).unwrap()["pending"], "graph");
        let (request, tx) = take(&hosts.graph);
        assert_eq!(request.outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [message]);
        let unsure = if lost { vec![message] } else { Vec::new() };
        let why = if lost { "the request timed out" } else { "Microsoft Graph is busy; try again later" };
        let _ = tx.send(GraphResult { account: ids.oauth, held: vec![message], unsure, errors: vec![why.into()], ..Default::default() });
        poll_run(&mut app);
        // The IMAP leg: it doesn't send it either way.
        let (leg, tx) = take(&hosts.sync);
        assert!(leg.outbox.iter().all(|m| m.id != message), "lost: {lost}: not sent a second way in this run");
        let _ = tx.send(SyncResult { account: ids.oauth, partial: false, ..Default::default() });
        poll_run(&mut app);
        let m = app.session.mailbox.message(message).expect("still in the Outbox");
        assert_eq!((m.held.is_empty(), m.sending), (!lost, false), "lost: {lost}: held only when it may have gone: {:?}", m.held);
        // The next run.
        let again = app.run("ui.sync", json!({"account": ids.oauth})).unwrap();
        let going: Vec<Id> = if again["pending"] == "graph" {
            take(&hosts.graph).0.outbox.iter().map(|m| m.id).collect()
        } else {
            take(&hosts.sync).0.outbox.iter().map(|m| m.id).collect()
        };
        assert_eq!(going.contains(&message), !lost, "lost: {lost}: sent again only when it didn't go");
    }
}

/// An SMTP send cut off once the message was handed over may have gone: the run's answer says so,
/// and the message is held, not sent again by the next run.
#[test]
fn an_smtp_send_cut_off_mid_way_is_held_not_sent_again() {
    let (mut app, ids, hosts, _store) = window();
    let message = queued(&mut app, ids.imap, "Payslip");
    assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true);
    let (_, tx) = take(&hosts.sync);
    let errors = vec!["Couldn't send \"Payslip\": the connection was closed".to_string()];
    let _ = tx.send(SyncResult { account: ids.imap, unsure: vec![message], errors, partial: false, ..Default::default() });
    poll_run(&mut app);
    let m = app.session.mailbox.message(message).expect("still in the Outbox");
    assert!(m.held.starts_with("It may have been sent") && !m.sending, "held: {:?}", m.held);
    assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true);
    assert!(take(&hosts.sync).0.outbox.iter().all(|m| m.id != message), "not sent again by itself");
}

// ---- shared mailboxes' sign-in (AU3-10) ----

/// A shared mailbox's run brings a sign-in of somebody other than the mailbox's owner: it is
/// neither kept nor remembered.
#[test]
fn a_shared_mailbox_run_keeps_only_its_owners_sign_in() {
    for inbox in [false, true] {
        let (mut app, ids, hosts, store) = window();
        if inbox {
            assert!(app.start_inbox_pass(ids.shared, true, true).is_some());
        } else {
            assert_eq!(app.run("ui.sync", json!({"account": ids.shared})).unwrap()["started"], true);
        }
        let (mut request, tx) = take(&hosts.graph);
        drop(request.mail_pieces.take());
        let someone = Tokens { email: "mallory@example.net".into(), ..tokens(STALE, "someone-elses-refresh") };
        let _ = tx.send(GraphResult { account: ids.shared, tokens: Some(someone), ..Default::default() });
        let writes = store.writes();
        poll_run(&mut app);
        assert_eq!(store.writes(), writes, "inbox pass: {inbox}: nothing remembered");
        let kept_stale = app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.access == STALE);
        assert!(!kept_stale, "inbox pass: {inbox}: somebody else's sign-in isn't kept");
    }
}

// ---- the background saver (AU3-26) ----

/// A background saver that writes nothing, and reports each state handed in as the test says.
#[derive(Clone, Default)]
struct Saver {
    handed: Arc<Mutex<Vec<(std::path::PathBuf, u64)>>>,
    fails: Arc<Mutex<bool>>,
}

impl mailcraft_engine::io::BackgroundSave for Saver {
    fn follow(&self, _: &std::path::Path) {}
    fn submit(&self, path: &std::path::Path, _: &Mailbox, rev: u64) {
        self.handed.lock().unwrap().push((path.to_path_buf(), rev));
    }
    fn reports(&self) -> Vec<mailcraft_engine::io::SaveReport> {
        let failed = *self.fails.lock().unwrap();
        std::mem::take(&mut *self.handed.lock().unwrap())
            .into_iter()
            .map(|(path, rev)| mailcraft_engine::io::SaveReport { path, rev, result: if failed { Err("the disk is full".into()) } else { Ok(()) } })
            .collect()
    }
    fn flush(&self) {}
}

/// Where the mailbox is kept by the background saver, the run waits for the marks to be written
/// before the host is handed it; a save that fails sends nothing.
#[test]
fn a_send_waits_for_the_saver_to_write_its_mark() {
    for fails in [false, true] {
        let (mut app, ids, hosts, _store) = window();
        let saver = Saver::default();
        *saver.fails.lock().unwrap() = fails;
        app.services.saver = Some(Box::new(saver.clone()));
        app.session.path = Some(std::path::PathBuf::from("kept.mailcraft"));
        app.autosave = true;
        let message = queued(&mut app, ids.imap, "Invoice");
        assert_eq!(app.run("ui.sync", json!({"account": ids.imap})).unwrap()["started"], true);
        assert!(hosts.sync.lock().unwrap().is_empty(), "nothing goes before the mark is on disk");
        assert!(app.job_running(JobKind::Imap), "the run is under way, waiting");
        assert!(app.session.mailbox.message(message).is_some_and(|m| m.sending));
        // The saver says how it went.
        let failed = app.collect_saves();
        app.send_gate_tick(failed.as_deref());
        if fails {
            assert!(hosts.sync.lock().unwrap().is_empty(), "a save that failed sends nothing");
            poll_run(&mut app);
            assert!(!app.syncing(), "the run ends");
            let m = app.session.mailbox.message(message).unwrap();
            assert_eq!((m.sending, m.held.as_str()), (false, ""), "not held: it never went, and goes with the next run");
        } else {
            let (going, _) = handed(&hosts, Sends::Imap);
            assert_eq!(going, [message], "handed over once the mark is written");
        }
    }
}
