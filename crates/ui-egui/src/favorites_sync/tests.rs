//! The Favorites of Outlook in the window: when they are asked for, what is done with the
//! answer, and what the folder pane and File › Options show for it. The host is a stand-in that
//! answers what a test tells it to; nobody is signed in and nothing leaves this machine.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::folder::{favorites, favorites_from_server};
use mailcraft_model::remote::{ContactsDownload, EventsDownload};
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use mailcraft_model::{Directory, FavoritesSource, FolderKind, Id};
use serde_json::json;

use super::*;
use crate::{GraphRequest, Services, SignInRequest};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;
const MINUTE: f64 = 60.0 * SECOND;

/// What a job asked of the Favorites: whether it read them, whether it wanted a token that is
/// good for them, and whether it brought a token along.
type Asked = (bool, bool, bool);

/// The host's stand-in: what it was asked, and what it says of the Favorites.
#[derive(Default)]
struct Host {
    jobs: Vec<Asked>,
    /// How a read goes.
    outcome: FavoritesOutcome,
    folders: Vec<RemoteFavorite>,
    why: String,
    /// A token that is obtained is not good for the Favorites: the owner never agreed to that.
    unconsented: bool,
    /// The sign-ins it was asked for (whether for Graph, and whether for the Favorites), and what
    /// one for the Favorites comes to.
    sign_ins: Vec<(bool, bool)>,
    sign_in_error: Option<String>,
    /// The reads through Exchange Web Services it was asked for (whether each brought a token
    /// along), how they go, and how often the owner was sent to the page for it.
    ews_jobs: Vec<bool>,
    ews_unconsented: bool,
    ews_blocked: bool,
    ews_stale_token: bool,
    ews_sign_ins: usize,
    /// The links for an administrator it was asked to make: the directory and the way.
    links: Vec<(String, FavoritesSource)>,
}

fn favorite(path: &[&str], kind: FolderKind, root: FolderKind) -> RemoteFavorite {
    RemoteFavorite { path: path.iter().map(|name| name.to_string()).collect(), kind, root }
}

struct Rig {
    app: MailApp,
    host: Arc<Mutex<Host>>,
    account: Id,
    inbox: Id,
    sent: Id,
    projects: Id,
    receipts: Id,
    /// The time of the last look (milliseconds).
    at: f64,
}

impl Rig {
    /// The sample mailbox, an account that signs in with Microsoft with folders of its own and a
    /// favourite of the owner's making, and a host that has Outlook's list: Receipts, Sent
    /// Items, Inbox/Projects.
    fn new() -> Rig {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let host: Arc<Mutex<Host>> = Default::default();
        {
            let mut host = host.lock().unwrap();
            host.outcome = FavoritesOutcome::Read;
            host.folders = vec![
                favorite(&["Receipts"], FolderKind::Custom, FolderKind::Custom),
                favorite(&["Sent Items"], FolderKind::Sent, FolderKind::Sent),
                favorite(&["Inbox", "Projects"], FolderKind::Custom, FolderKind::Inbox),
            ];
        }
        let shared = host.clone();
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let mut host = shared.lock().unwrap();
            host.jobs.push((request.favorites, request.favorites_scopes, !request.access.is_empty()));
            let mut out = GraphResult { account: request.account.id, ..Default::default() };
            if request.access.is_empty() {
                out.tokens = Some(Tokens {
                    access: "graph-tok".into(),
                    refresh: "ref-2".into(),
                    expires_at: mailcraft_time::now() + 3600,
                    ..Default::default()
                });
                out.favorites_covered = request.favorites_scopes.then_some(!host.unconsented);
            }
            if let Some((from, to)) = request.calendar {
                out.events = Some(EventsDownload { from, to, complete: true, ..Default::default() }.for_account(&request.account));
            }
            if request.directory {
                out.events = Some(EventsDownload { from: 0, to: i64::MAX / 4, complete: true, ..Default::default() }.for_account(&request.account));
                out.contacts = Some(ContactsDownload { complete: true, ..Default::default() }.for_account(&request.account));
            }
            if request.favorites {
                let outcome = if out.favorites_covered == Some(false) { FavoritesOutcome::NotPermitted } else { host.outcome };
                let folders = if outcome == FavoritesOutcome::Read { host.folders.clone() } else { Vec::new() };
                out.favorites = Some(FavoritesDownload { outcome, folders, ..Default::default() }.for_account(&request.account));
                out.favorites_error = host.why.clone();
            }
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(out);
            rx
        }));
        let shared = host.clone();
        app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
            let mut host = shared.lock().unwrap();
            let tokens =
                Tokens { access: "graph-tok-3".into(), refresh: "ref-3".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
            let (tx, rx) = mpsc::channel();
            // (For Exchange Web Services alone: counted by itself, and agreed to unless told otherwise.)
            if request.ews {
                host.ews_sign_ins += 1;
                let agreed = host.sign_in_error.is_none();
                host.ews_unconsented &= !agreed;
                let _ = tx.send(host.sign_in_error.clone().map_or(Ok(Tokens { access: "ews-tok-2".into(), ..tokens.clone() }), Err));
                return rx;
            }
            host.sign_ins.push((request.graph, request.favorites));
            let _ = tx.send(match &host.sign_in_error {
                Some(error) if request.favorites => Err(error.clone()),
                _ => Ok(tokens),
            });
            rx
        }));
        let shared = host.clone();
        app.services.ews = Some(Box::new(move |request: crate::EwsRequest| {
            let mut host = shared.lock().unwrap();
            host.ews_jobs.push(!request.access.is_empty());
            let mut out = crate::EwsResult { account: request.account.id, ..Default::default() };
            let download = |outcome: FavoritesOutcome, folders: Vec<RemoteFavorite>| {
                Some(FavoritesDownload { outcome, folders, ..Default::default() }.for_account(&request.account))
            };
            if request.access.is_empty() && host.ews_unconsented {
                out.consent_needed = true;
            } else if host.ews_blocked {
                out.blocked = true;
            } else if !request.access.is_empty() && std::mem::take(&mut host.ews_stale_token) {
                out.token_refused = true;
            } else {
                if request.access.is_empty() {
                    out.tokens = Some(Tokens {
                        access: "ews-tok".into(),
                        refresh: "ref-4".into(),
                        expires_at: mailcraft_time::now() + 3600,
                        ..Default::default()
                    });
                }
                let folders = if host.outcome == FavoritesOutcome::Read { host.folders.clone() } else { Vec::new() };
                out.favorites = download(host.outcome, folders);
                out.favorites_error = host.why.clone();
            }
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(out);
            rx
        }));
        let shared = host.clone();
        app.services.admin_consent_link = Some(Box::new(move |oauth, tenant, source| {
            shared.lock().unwrap().links.push((tenant.to_string(), source));
            Some(format!(
                "https://login.example.org/{}/adminconsent?client_id={}",
                if tenant.is_empty() { "organizations" } else { tenant },
                oauth.client_id
            ))
        }));
        app.services.sync = Some(Box::new(|request: SyncRequest| {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(SyncResult { account: request.account.id, ..Default::default() });
            rx
        }));
        let added = app.execute(
            "account.add",
            json!({"email": "avery@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.mail.example"}}),
        );
        let account = added.unwrap()["id"].as_u64().unwrap();
        let fresh = Tokens { access: "graph-tok".into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
        app.tokens.insert(account, Tokens { access: "mail-tok".into(), ..fresh.clone() });
        app.graph_tokens.insert(account, fresh);
        let kind = |app: &MailApp, kind: FolderKind| app.session.mailbox.folder_of_kind(account, kind).unwrap();
        let (inbox, sent) = (kind(&app, FolderKind::Inbox), kind(&app, FolderKind::Sent));
        let mut folder = |name: &str, parent: Option<Id>| {
            app.execute("folder.new", json!({"name": name, "parent": parent, "account": account})).unwrap()["id"].as_u64().unwrap()
        };
        let projects = folder("Projects", Some(inbox));
        let receipts = folder("Receipts", None);
        let team = folder("Team Lumen", None);
        // The owner's own favourite of that account, beside its Inbox.
        app.execute("folder.favorite", json!({"id": team})).unwrap();
        Rig { app, host, account, inbox, sent, projects, receipts, at: now_ms() }
    }

    /// Look at the calendar `ms` on, and take in what a job that started brings. Whether one did.
    fn look(&mut self, ms: f64) -> bool {
        self.at += ms;
        let started = self.app.calendar_tick_at(true, self.at);
        for _ in 0..4 {
            self.app.poll_calendar();
        }
        started
    }

    fn jobs(&self) -> Vec<Asked> {
        self.host.lock().unwrap().jobs.clone()
    }

    fn last(&self) -> Asked {
        *self.host.lock().unwrap().jobs.last().expect("a job was done")
    }

    /// The folders of the account that the pane lists under Favorites, in its order.
    fn listed(&self) -> Vec<Id> {
        let of_account = |id: &Id| self.app.session.mailbox.folder(*id).is_some_and(|f| f.account == self.account);
        favorites(&self.app.session).into_iter().filter(of_account).collect()
    }

    fn marked(&self) -> Vec<Id> {
        self.app.session.mailbox.folders.iter().filter(|f| f.favorite).map(|f| f.id).collect()
    }

    fn note(&self) -> Option<Note> {
        self.app.favorites_note().map(|(_, note)| note)
    }

    /// Let a sign-in and the Send/Receive that follows it run to their end.
    fn settle(&mut self) {
        for _ in 0..8 {
            self.app.poll_sign_in();
            self.app.poll_graph();
            self.app.poll_sync();
            self.app.poll_calendar();
        }
    }
}

#[test]
fn outlooks_favorites_are_read_at_the_start_and_then_every_quarter_of_an_hour() {
    let mut rig = Rig::new();
    let own = rig.marked();
    assert!(!favorites_from_server(&rig.app.session, rig.account));
    // The first look at the account's server brings them along, with the token at hand.
    assert!(rig.look(0.0));
    assert_eq!(rig.jobs(), [(true, true, true)]);
    assert_eq!(rig.listed(), [rig.receipts, rig.sent, rig.projects], "Outlook's list, in Outlook's order");
    assert!(favorites_from_server(&rig.app.session, rig.account) && rig.note().is_none());
    // The owner's own marks are kept aside as they were, and the other account's are listed still.
    assert_eq!(rig.marked(), own);
    let all = favorites(&rig.app.session);
    assert!(all.len() > 3 && all[..3] == [rig.receipts, rig.sent, rig.projects]);

    // The calendar is asked every five minutes behind the mail; the Favorites ride along with
    // the third of those looks, a quarter of an hour on, and not before.
    assert!(rig.look(5.0 * MINUTE + SECOND));
    assert!(rig.look(5.0 * MINUTE + SECOND));
    assert_eq!(rig.jobs()[1..], [(false, false, true), (false, false, true)]);
    assert!(rig.look(5.0 * MINUTE + SECOND));
    assert_eq!(rig.last(), (true, true, true));

    // A favourite taken off in Outlook, and the order changed there: so it is here at that look.
    rig.host.lock().unwrap().folders =
        vec![favorite(&[], FolderKind::Inbox, FolderKind::Inbox), favorite(&["receipts"], FolderKind::Custom, FolderKind::Custom)];
    assert!(rig.look(5.0 * MINUTE + SECOND));
    assert_eq!(rig.listed(), [rig.receipts, rig.sent, rig.projects], "not due yet");
    rig.look(5.0 * MINUTE + SECOND);
    assert!(rig.look(5.0 * MINUTE + SECOND));
    assert_eq!(rig.listed(), [rig.inbox, rig.receipts]);

    // A Send/Receive the owner asks for reads them whatever the clock says; one that started
    // by itself doesn't.
    rig.host.lock().unwrap().folders = vec![favorite(&["Receipts"], FolderKind::Custom, FolderKind::Custom)];
    let before = rig.jobs().len();
    rig.app.execute("ui.sync", json!({"account": rig.account})).unwrap();
    rig.settle();
    assert_eq!(rig.jobs()[before..], [(true, true, true)]);
    assert_eq!(rig.listed(), [rig.receipts]);

    // The owner would rather have the ones kept here: they are back exactly, nothing is read,
    // and no token is asked to be good for it.
    rig.app.run("view.favorites", json!({"own": true})).unwrap();
    assert_eq!(
        (rig.listed(), rig.marked()),
        (own.iter().copied().filter(|id| rig.app.session.mailbox.folder(*id).unwrap().account == rig.account).collect(), own.clone())
    );
    rig.app.execute("ui.sync", json!({"account": rig.account})).unwrap();
    rig.settle();
    assert_eq!(rig.last(), (false, false, true));
    rig.look(20.0 * MINUTE);
    assert_eq!(rig.last(), (false, false, true));
    assert!(rig.app.execute("ui.favorites", json!({"refresh": true})).is_err());
    // And Outlook's again: read at the very next look, not a quarter of an hour on.
    rig.app.run("view.favorites", json!({"own": false})).unwrap();
    assert_eq!(rig.listed(), [rig.receipts], "what was kept of the server's list shows at once");
    rig.host.lock().unwrap().folders = vec![favorite(&["Sent"], FolderKind::Sent, FolderKind::Sent)];
    assert!(rig.look(SECOND));
    assert_eq!((rig.last(), rig.listed()), ((true, true, true), vec![rig.sent]));
    assert!(rig.app.dialog.is_none() && !rig.app.has_sync_problems());
}

#[test]
fn a_sign_in_that_does_not_cover_them_gets_one_quiet_offer_and_nothing_else_changes() {
    let mut rig = Rig::new();
    let own = favorites(&rig.app.session);
    let marks = rig.marked();
    // An account that signed in before any of this was asked for: its token is asked to be good
    // for the Favorites, isn't, and the server is not asked what it would only refuse.
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    assert!(rig.look(0.0));
    assert_eq!(rig.jobs(), [(true, true, false)]);
    assert_eq!(rig.note(), Some(Note::Offer));
    // Nothing else is any different: the owner's favourites as they were, no dialog, no problem
    // to report, and the calendar came all the same.
    assert_eq!((favorites(&rig.app.session), rig.marked()), (own.clone(), marks.clone()));
    assert!(!favorites_from_server(&rig.app.session, rig.account));
    assert!(rig.app.dialog.is_none() && !rig.app.has_sync_problems() && !rig.app.graph_consent_needed(rig.account));
    assert!(rig.app.status_msg.is_none());
    rig.app.execute("folder.favorite", json!({"id": rig.receipts, "value": true})).unwrap();
    rig.app.execute("folder.favorite", json!({"id": rig.receipts, "value": false})).unwrap();

    // Not again that day: neither the read nor a token for it, whatever starts a job, and the
    // token it has is the one it goes on with.
    for _ in 0..4 {
        rig.look(5.0 * MINUTE + SECOND);
    }
    rig.app.execute("ui.sync", json!({"account": rig.account})).unwrap();
    rig.settle();
    assert!(rig.jobs()[1..].iter().all(|asked| *asked == (false, false, true)), "{:?}", rig.jobs());
    assert_eq!(rig.note(), Some(Note::Offer));
    // The next day it is looked at once more, with a token asked for anew; and left again.
    rig.look(25.0 * 60.0 * MINUTE);
    assert_eq!(rig.last(), (true, true, false));
    rig.look(5.0 * MINUTE + SECOND);
    assert_eq!(rig.last(), (false, false, true));

    // The owner takes the offer: a sign-in for the two permissions, on the provider's page.
    assert_eq!(rig.app.execute("ui.connectFavorites", json!({})).unwrap(), json!({"pending": "signIn", "account": rig.account}));
    rig.host.lock().unwrap().unconsented = false;
    let before = rig.jobs().len();
    rig.settle();
    assert_eq!(rig.host.lock().unwrap().sign_ins, [(true, true)]);
    // They are read at once, with the token the sign-in brought, and the offer is gone.
    assert_eq!(rig.jobs()[before], (true, true, true));
    assert_eq!((rig.note(), rig.listed()), (None, vec![rig.receipts, rig.sent, rig.projects]));
    assert_eq!(rig.marked(), marks);
    assert!(rig.app.dialog.is_none());
}

#[test]
fn an_organisation_that_does_not_allow_it_is_said_in_one_line() {
    let mut rig = Rig::new();
    let own = favorites(&rig.app.session);
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look(0.0);
    for (refusal, line) in [(REFUSED_BY_ORGANISATION, "Not allowed by your organisation"), (REFUSED_FOR_ACCOUNT, "Not available for this account")] {
        rig.host.lock().unwrap().sign_in_error = Some(refusal.to_string());
        rig.app.execute("ui.connectFavorites", json!({"account": rig.account})).unwrap();
        let before = rig.jobs().len();
        rig.settle();
        assert_eq!(rig.note(), Some(Note::Refused(line.to_string())));
        // One plain line, and nobody's administrator in it or anywhere else.
        let state = rig.app.execute("ui.favorites", json!({"answer": {"outcome": "refused", "why": refusal}})).unwrap();
        assert_eq!((state["line"].clone(), state["text"].clone(), state["from"].clone()), (json!("refused"), json!(line), json!("mailbox")));
        assert!(!line.to_lowercase().contains("admin"));
        // No dialog, no problem to report, nothing asked of the server for it, nothing changed.
        assert!(rig.app.dialog.is_none() && !rig.app.has_sync_problems());
        assert_eq!(rig.jobs().len(), before);
        assert_eq!(favorites(&rig.app.session), own);
        rig.look(5.0 * MINUTE + SECOND);
        assert_eq!(rig.last(), (false, false, true));
    }
    // Something else went wrong with the sign-in (the owner closed the page): the status bar
    // says so, and the offer stands as it did.
    let mut rig = Rig::new();
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look(0.0);
    rig.host.lock().unwrap().sign_in_error = Some("the sign-in was cancelled".into());
    rig.app.execute("ui.connectFavorites", json!({})).unwrap();
    rig.settle();
    assert_eq!(rig.note(), Some(Note::Offer));
    assert_eq!(rig.app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some("the sign-in was cancelled"));
    assert!(rig.app.dialog.is_none());
}

#[test]
fn a_read_that_fails_is_one_quiet_line_with_try_again() {
    let mut rig = Rig::new();
    rig.look(0.0);
    let listed = rig.listed();
    assert_eq!(listed, [rig.receipts, rig.sent, rig.projects]);
    // The server can't be reached at the next look that reads them.
    {
        let mut host = rig.host.lock().unwrap();
        (host.outcome, host.why) = (FavoritesOutcome::Failed, "Couldn't reach Outlook".into());
    }
    rig.look(16.0 * MINUTE);
    assert_eq!(rig.note(), Some(Note::Failed("Couldn't reach Outlook".into())));
    // What was listed stays listed, and nothing is made of it but the line.
    assert_eq!(rig.listed(), listed);
    assert!(rig.app.dialog.is_none() && !rig.app.has_sync_problems() && rig.app.status_msg.is_none());
    // Not asked again before it is due; "Try again" asks at the next look.
    rig.look(5.0 * MINUTE + SECOND);
    assert_eq!(rig.last(), (false, false, true));
    rig.host.lock().unwrap().outcome = FavoritesOutcome::Read;
    let state = rig.app.execute("ui.favorites", json!({"account": rig.account, "refresh": true})).unwrap();
    assert_eq!(state["line"], json!(null));
    assert!(rig.look(SECOND));
    assert_eq!((rig.last(), rig.note(), rig.listed()), ((true, true, true), None, listed.clone()));
    // A failure with no words for it says the plainest thing.
    rig.host.lock().unwrap().outcome = FavoritesOutcome::Failed;
    rig.host.lock().unwrap().why = String::new();
    rig.look(16.0 * MINUTE);
    assert_eq!(rig.note(), Some(Note::Failed("Couldn't read the list".into())));
    // The mailbox has no such list on its server: no line, and the owner's own favourites.
    rig.host.lock().unwrap().outcome = FavoritesOutcome::Missing;
    rig.look(16.0 * MINUTE);
    assert_eq!(rig.note(), None);
    assert!(!favorites_from_server(&rig.app.session, rig.account));
    assert_eq!(rig.listed().len(), 2, "its Inbox and the folder the owner marked");
}

#[test]
fn the_commands_take_anything_and_say_what_they_cannot_do() {
    // Without an account that signs in with Microsoft there is nothing to connect or to read.
    let mut plain = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    for command in ["ui.connectFavorites", "ui.favorites"] {
        assert!(plain.execute(command, json!({})).unwrap_err().contains("Microsoft"), "{command}");
        let local = plain.session.mailbox.accounts[0].id;
        assert!(plain.execute(command, json!({"account": local})).is_err());
    }
    assert!(plain.favorites_note().is_none());
    let mut rig = Rig::new();
    for odd in [
        json!(null),
        json!([]),
        json!({"account": "x"}),
        json!({"account": 0}),
        json!({"account": 999_999}),
        json!({"answer": 7}),
        json!({"answer": {"outcome": 7}}),
        json!({"answer": {"outcome": "sideways"}}),
        json!({"answer": {"outcome": "read", "folders": "x"}}),
        json!({"answer": {"outcome": "read", "folders": [{"path": "x"}]}}),
        json!({"answer": {"outcome": "failed", "why": "x".repeat(10_000)}}),
        json!({"answer": {"outcome": "read", "folders": [{"path": ["\u{0}", "a\nb"], "kind": "custom"}, {}, {"kind": "inbox", "path": []}]}}),
    ] {
        let _ = rig.app.execute("ui.favorites", odd.clone());
        let _ = rig.app.execute("ui.connectFavorites", odd);
        let _ = rig.app.execute("ui.cancelSignIn", json!({}));
        rig.settle();
    }
    // A line is a line, however long the reason was.
    rig.app.execute("ui.favorites", json!({"answer": {"outcome": "failed", "why": "x".repeat(10_000)}})).unwrap();
    assert!(matches!(rig.note(), Some(Note::Failed(why)) if why.chars().count() <= 60));
    // The stand-in answer of a script is taken in like the host's.
    let state = rig
        .app
        .execute(
            "ui.favorites",
            json!({"answer": {"outcome": "read", "folders": [{"path": ["Inbox", "Projects"], "kind": "custom", "root": "inbox"}]}}),
        )
        .unwrap();
    assert_eq!((state["from"].clone(), state["line"].clone(), state["favorites"][0].clone()), (json!("server"), json!(null), json!(rig.projects)));
    let state = rig.app.execute("ui.favorites", json!({"answer": {"outcome": "notPermitted"}})).unwrap();
    assert_eq!(
        (state["from"].clone(), state["line"].clone(), state["text"].clone()),
        (json!("mailbox"), json!("offer"), json!("Show Outlook's Favorites"))
    );
    // An account that goes takes what was remembered of it along.
    rig.app.execute("account.remove", json!({"id": rig.account})).unwrap();
    assert!(rig.app.favorites_note().is_none() && rig.app.favorites.notes.is_empty() && rig.app.favorites.missing.is_empty());
}

fn window(rig: Rig) -> (egui_kittest::Harness<'static, MailApp>, Arc<Mutex<Host>>, [Id; 5]) {
    let ids = [rig.account, rig.inbox, rig.sent, rig.projects, rig.receipts];
    let host = rig.host.clone();
    let mut app = rig.app;
    // (The harness looks at the server by the real clock: nothing is due there but the start.)
    app.auto_sync = false;
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    (harness, host, ids)
}

fn click(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

#[test]
fn the_pane_lists_outlooks_favorites_in_their_order_under_the_heading() {
    let mut rig = Rig::new();
    rig.look(0.0);
    let (mut harness, _, [account, inbox, sent, projects, receipts]) = window(rig);
    let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
    let heading = at(&harness, "folderHeading", 0).expect("the Favorites heading");
    let own = at(&harness, "folderHeading", account).expect("the account's heading");
    // Receipts, Sent Items, Projects: one row each, down the pane in Outlook's order, between
    // the Favorites heading and the first account's. (Each is also listed under its account:
    // the row under Favorites is the first of the two.)
    let rows: Vec<egui::Rect> = [receipts, sent, projects].iter().map(|id| at(&harness, "folder", *id).expect("a row")).collect();
    assert!(rows[0].min.y >= heading.max.y - 0.5);
    for pair in rows.windows(2) {
        assert!((pair[1].min.y - pair[0].max.y).abs() < 0.5, "{pair:?}");
    }
    assert!(rows[2].max.y < own.min.y);
    // No line of any kind while there is nothing to say.
    assert!(at(&harness, "favoritesOffer", account).is_none() && at(&harness, "favoritesNote", account).is_none());
    // A row there shows its folder, like any other.
    click(&mut harness, rows[1].center());
    assert_eq!(harness.state().session.view.folder, sent);

    // The folder's own menu and the ribbon don't offer what is changed in Outlook: the command
    // says so when it is run all the same.
    let refused = harness.state_mut().run("folder.favorite", json!({"id": inbox})).unwrap_err();
    assert!(refused.contains("Outlook"), "{refused}");
    assert_eq!(harness.state().status_msg.as_ref().map(|(text, _)| text.clone()), Some(refused));
    harness.run_steps(2);

    // The read fails: one line under the heading, the reason and "Try again", and the list stays.
    harness.state_mut().execute("ui.favorites", json!({"answer": {"outcome": "failed", "why": "Outlook is busy"}})).unwrap();
    harness.run_steps(3);
    let line = at(&harness, "favoritesNote", account).expect("the line");
    let retry = at(&harness, "favoritesRetry", account).expect("Try again");
    let heading = at(&harness, "folderHeading", 0).unwrap();
    assert!(line.min.y >= heading.max.y - 0.5 && (line.height() - 24.0).abs() < 0.01);
    assert!(line.contains(retry.center()) && retry.min.x > line.min.x + 40.0, "{retry:?} in {line:?}");
    let first = at(&harness, "folder", receipts).unwrap();
    assert!((first.min.y - line.max.y).abs() < 0.5, "the favourites follow the line");
    assert!(harness.state().dialog.is_none());
    click(&mut harness, retry.center());
    assert!(at(&harness, "favoritesNote", account).is_none(), "asked again: the line is gone until there is an answer");
}

#[test]
fn the_offer_is_one_line_under_the_heading_and_in_the_options() {
    let mut rig = Rig::new();
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look(0.0);
    let own = favorites(&rig.app.session);
    let (mut harness, host, [account, inbox, ..]) = window(rig);
    let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
    // One row under the Favorites heading, above the owner's own favourites, which are all there.
    let heading = at(&harness, "folderHeading", 0).unwrap();
    let offer = at(&harness, "favoritesOffer", account).expect("the offer");
    assert!((offer.min.y - heading.max.y).abs() < 0.5 && (offer.height() - 24.0).abs() < 0.01);
    assert!(at(&harness, "folder", inbox).is_some_and(|row| row.min.y >= offer.max.y - 0.5));
    assert_eq!(favorites(&harness.state().session), own);
    // A click asks for the two permissions on the provider's page, and for nothing by itself.
    assert!(host.lock().unwrap().sign_ins.is_empty());
    host.lock().unwrap().unconsented = false;
    click(&mut harness, offer.center());
    harness.run_steps(6);
    assert_eq!(host.lock().unwrap().sign_ins, [(true, true)]);
    assert!(at(&harness, "favoritesOffer", account).is_none());

    // File › Options: the switch and nothing under it; the offer is the folder pane's.
    let mut rig = Rig::new();
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look(0.0);
    let (mut harness, _, [account, ..]) = window(rig);
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(at(&harness, "favoritesOption", 0).is_some());
    assert!(at(&harness, "favoritesConnect", account).is_none());
    // Switched off there, nothing of Outlook's is shown.
    let switch = at(&harness, "favoritesOption", 0).unwrap();
    click(&mut harness, egui::pos2(switch.min.x + 9.0, switch.center().y));
    assert!(harness.state().session.prefs.own_favorites);
    // The setting is saved with the others, and restored.
    let saved = serde_json::to_string(&harness.state().prefs()).unwrap();
    assert!(saved.contains(r#""ownFavorites":true"#), "{saved}");
    let restored: crate::UiState = serde_json::from_str(&saved).unwrap();
    assert!(restored.options.own_favorites);
    // A mailbox without an account that signs in with Microsoft has no such switch.
    let mut plain = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    plain.autosave = false;
    plain.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        plain,
    );
    harness.run_steps(4);
    assert!(harness.state().hit.rect("favoritesOption", 0).is_none());
}

// ---- the owner's choice of how they are read: Microsoft Graph, Exchange Web Services, not now ----

/// A directory of an organisation, as a sign-in names it (invented), and Microsoft's own for
/// personal accounts.
const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";

impl Rig {
    /// The rig with its account one of an organisation: a sign-in of this run said so.
    fn organisation() -> Rig {
        let mut rig = Rig::new();
        rig.app.note_directory(rig.account, ORGANISATION);
        rig
    }

    /// Look at the servers `ms` on: the calendar's job and the read through Exchange Web
    /// Services, when either is due, and what they and a sign-in bring.
    fn look_all(&mut self, ms: f64) {
        self.at += ms;
        for _ in 0..3 {
            self.app.calendar_tick_at(true, self.at);
            self.app.poll_calendar();
            self.app.favorites_tick_at(self.at);
            self.app.poll_favorites();
            self.app.poll_sign_in();
        }
    }

    fn choose(&mut self, source: &str) -> serde_json::Value {
        self.app.execute("ui.favoritesSource", json!({"account": self.account, "source": source})).unwrap()
    }

    fn state(&mut self) -> serde_json::Value {
        self.app.execute("ui.favorites", json!({"account": self.account})).unwrap()
    }

    /// The reads through Exchange Web Services the host was asked for (whether each brought a
    /// token along), and how often the owner was sent to Microsoft's page for it.
    fn asked_of_ews(&self) -> (Vec<bool>, usize) {
        let host = self.host.lock().unwrap();
        (host.ews_jobs.clone(), host.ews_sign_ins)
    }
}

#[test]
fn which_kind_of_microsoft_account_it_is_says_whether_there_is_a_choice() {
    let mut rig = Rig::new();
    // Nobody has said: no choice, as before there was one.
    assert_eq!((rig.app.directory(rig.account), rig.app.favorites_choice(rig.account)), (Directory::Unknown, false));
    assert!(rig.app.execute("ui.dialog", json!({"name": "favoritesSource", "account": rig.account})).is_err());
    // A sign-in names the directory: an organisation's, or the one of personal accounts.
    rig.app.note_directory(rig.account, Directory::PERSONAL_TENANT);
    assert_eq!((rig.app.directory(rig.account), rig.app.favorites_choice(rig.account)), (Directory::Personal, false));
    assert_eq!(rig.state()["offered"], json!(["graph", "none"]));
    rig.app.note_directory(rig.account, ORGANISATION);
    assert_eq!((rig.app.directory(rig.account), rig.app.favorites_choice(rig.account)), (Directory::Organisation, true));
    assert_eq!(rig.state()["offered"], json!(["graph", "ews", "none"]));
    // Nothing a token says that isn't a directory changes that, and nothing of it is saved.
    for junk in ["", "common", " "] {
        rig.app.note_directory(rig.account, junk);
    }
    assert!(rig.app.favorites_choice(rig.account));
    let file = String::from_utf8(mailcraft_engine::io::write_mailbox(&rig.app.session.mailbox).unwrap()).unwrap();
    assert!(!file.contains("directory") && !file.contains(ORGANISATION) && !file.contains("favoritesSource"));
    // What was written down with the account goes before what a sign-in says…
    rig.app.execute("account.edit", json!({"id": rig.account, "directory": "personal"})).unwrap();
    assert!(!rig.app.favorites_choice(rig.account));
    rig.app.execute("account.edit", json!({"id": rig.account, "directory": "organisation"})).unwrap();
    // …and the account's settings say it when nothing else does.
    let other = rig.app.execute(
        "account.add",
        json!({"email": "noor@lumen.example", "auth": "oauth2", "provider": "microsoft", "tenant": "consumers", "incoming": {"host": "imap.lumen.example"}}),
    );
    let other = other.unwrap()["id"].as_u64().unwrap();
    assert_eq!(rig.app.directory(other), Directory::Personal);
    rig.app.execute("account.edit", json!({"id": other, "tenant": "lumen.example"})).unwrap();
    assert_eq!(rig.app.directory(other), Directory::Organisation);
    // An account that doesn't sign in with Microsoft has no Favorites in Outlook at all.
    let local = rig.app.session.mailbox.accounts[0].id;
    assert!(!rig.app.favorites_choice(local) && rig.app.directory(local) == Directory::Unknown);
    // The choice opens for the account that has one.
    rig.app.execute("ui.dialog", json!({"name": "favoritesSource"})).unwrap();
    assert_eq!(rig.app.dialog.as_ref().map(|dialog| dialog.name()), Some("favoritesSource"));
}

#[test]
fn exchange_web_services_is_read_by_a_job_of_its_own_and_asks_on_the_page_only_when_the_owner_chose() {
    let mut rig = Rig::organisation();
    let own = rig.listed();
    rig.look_all(0.0);
    assert_eq!(rig.jobs().len(), 1, "Graph, as an account that never chose");
    // The owner chooses Exchange Web Services, which the sign-in covers: read at once, by its
    // own job, with a token asked for without the owner; Graph is no longer asked for the list.
    let chosen = rig.choose("ews");
    assert_eq!((chosen["pending"].clone(), chosen["source"].clone()), (json!("read"), json!("ews")));
    rig.look_all(SECOND);
    assert_eq!(rig.asked_of_ews(), (vec![false], 0));
    assert_eq!(rig.listed(), [rig.receipts, rig.sent, rig.projects]);
    assert!(rig.note().is_none() && favorites_from_server(&rig.app.session, rig.account));
    // Every quarter of an hour, with the token that was kept; and the calendar's jobs go on
    // without the Favorites.
    let graph_jobs = rig.jobs().len();
    rig.look_all(5.0 * MINUTE + SECOND);
    rig.look_all(5.0 * MINUTE + SECOND);
    assert_eq!(rig.host.lock().unwrap().ews_jobs.len(), 1);
    rig.look_all(5.0 * MINUTE + SECOND);
    assert_eq!(rig.host.lock().unwrap().ews_jobs.clone(), [false, true]);
    assert!(rig.jobs().len() > graph_jobs && rig.jobs()[graph_jobs..].iter().all(|(favorites, scopes, _)| !favorites && !scopes));
    // A Send/Receive the owner asked for reads them too.
    rig.app.execute("ui.sync", json!({"account": rig.account})).unwrap();
    rig.settle();
    rig.look_all(SECOND);
    assert_eq!(rig.host.lock().unwrap().ews_jobs.len(), 3);
    // The token that was kept is no longer taken: dropped, and another is had at the next look.
    rig.host.lock().unwrap().ews_stale_token = true;
    rig.look_all(16.0 * MINUTE);
    rig.look_all(SECOND);
    assert_eq!(rig.host.lock().unwrap().ews_jobs[3..], [true, false]);
    assert!(rig.note().is_none());

    // The sign-in stops covering it (the owner took the permission back): nobody is sent to a
    // page for that. The offer, the owner's own favourites, and not again that day.
    rig.host.lock().unwrap().ews_unconsented = true;
    rig.app.favorites.ews_tokens.clear();
    rig.look_all(16.0 * MINUTE);
    assert_eq!((rig.note(), rig.listed(), rig.host.lock().unwrap().ews_sign_ins), (Some(Note::Offer), own.clone(), 0));
    let asked = rig.host.lock().unwrap().ews_jobs.len();
    for _ in 0..30 {
        rig.look_all(20.0 * MINUTE);
    }
    assert_eq!(rig.host.lock().unwrap().ews_jobs.len(), asked);
    // The owner chooses it again: now the page comes up, once, for that alone.
    rig.choose("ews");
    rig.look_all(SECOND);
    rig.look_all(SECOND);
    assert_eq!((rig.asked_of_ews().1, rig.host.lock().unwrap().sign_ins.len()), (1, 0));
    assert_eq!(rig.listed(), [rig.receipts, rig.sent, rig.projects]);
    assert!(rig.note().is_none() && rig.app.dialog.is_none());

    // An answer that arrives after the owner chose another way is nobody's.
    rig.app.favorites.due.insert(rig.account);
    assert!(rig.app.favorites_tick_at(rig.at + 1.0));
    rig.choose("none");
    rig.app.poll_favorites();
    assert_eq!((rig.listed(), rig.note()), (own, Some(Note::Offer)));
}

#[test]
fn what_stands_in_the_way_is_said_with_what_the_owner_can_do_about_it() {
    // Graph, and an administrator has to approve it: the line, the link, and the other way.
    let mut rig = Rig::organisation();
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look_all(0.0);
    rig.host.lock().unwrap().sign_in_error = Some(NEEDS_APPROVAL.to_string());
    let chosen = rig.choose("graph");
    assert_eq!(chosen["pending"], "signIn");
    rig.settle();
    assert_eq!(rig.note(), Some(Note::Approval(FavoritesSource::Graph)));
    let state = rig.state();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["steps"].clone()),
        (json!("approval"), json!(NEEDS_APPROVAL), json!(["copyMessage", "help", "useEws"]))
    );
    // The link names the organisation's own directory, as this run's sign-in said it.
    let copied = rig.app.execute("ui.copyAdminConsentLink", json!({})).unwrap();
    assert_eq!(copied["url"], format!("https://login.example.org/{ORGANISATION}/adminconsent?client_id="));
    assert_eq!(rig.host.lock().unwrap().links, [(ORGANISATION.to_string(), FavoritesSource::Graph)]);
    // Not asked again that day, and nothing else is any different.
    let sign_ins = rig.host.lock().unwrap().sign_ins.len();
    for _ in 0..30 {
        rig.look_all(20.0 * MINUTE);
    }
    assert_eq!((rig.host.lock().unwrap().sign_ins.len(), rig.note()), (sign_ins, Some(Note::Approval(FavoritesSource::Graph))));
    assert!(rig.app.dialog.is_none() && !rig.app.has_sync_problems());

    // Exchange Web Services instead, where an administrator has to approve that too: the same
    // line, for that way, with a link for it and Graph as the other way.
    rig.host.lock().unwrap().ews_unconsented = true;
    rig.choose("ews");
    rig.look_all(SECOND);
    assert_eq!((rig.note(), rig.host.lock().unwrap().ews_sign_ins), (Some(Note::Approval(FavoritesSource::Ews)), 1));
    assert_eq!(rig.state()["steps"], json!(["copyMessage", "help", "useGraph"]));
    rig.app.execute("ui.copyAdminConsentLink", json!({"account": rig.account})).unwrap();
    assert_eq!(rig.host.lock().unwrap().links.last(), Some(&(ORGANISATION.to_string(), FavoritesSource::Ews)));
    for _ in 0..30 {
        rig.look_all(20.0 * MINUTE);
    }
    assert_eq!(rig.host.lock().unwrap().ews_sign_ins, 1);

    // The organisation doesn't allow Exchange Web Services: said, with Graph offered instead.
    let mut rig = Rig::organisation();
    rig.look_all(0.0);
    rig.host.lock().unwrap().ews_blocked = true;
    rig.choose("ews");
    rig.look_all(SECOND);
    assert_eq!(rig.note(), Some(Note::Blocked));
    let state = rig.state();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["steps"].clone(), state["from"].clone()),
        (json!("blocked"), json!(EWS_NOT_ALLOWED), json!(["useGraph"]), json!("mailbox"))
    );
    let asked = rig.host.lock().unwrap().ews_jobs.len();
    for _ in 0..30 {
        rig.look_all(20.0 * MINUTE);
    }
    assert_eq!(rig.host.lock().unwrap().ews_jobs.len(), asked);
    // Graph instead: its page (the list wasn't Graph's before), and then its list.
    rig.choose("graph");
    rig.settle();
    rig.look_all(SECOND);
    assert_eq!((rig.host.lock().unwrap().sign_ins.clone(), rig.note()), (vec![(true, true)], None));
    assert_eq!(rig.listed(), [rig.receipts, rig.sent, rig.projects]);

    // A build that can make no link says so; one that has no such job keeps the choice.
    let mut rig = Rig::organisation();
    (rig.app.services.admin_consent_link, rig.app.services.ews) = (None, None);
    assert!(rig.app.execute("ui.copyAdminConsentLink", json!({})).unwrap_err().contains("link"));
    assert_eq!(rig.choose("ews")["pending"], json!(null));
    assert_eq!(rig.app.favorites_source(rig.account), FavoritesSource::Ews);
}

#[test]
fn the_new_commands_take_anything_and_say_what_they_cannot_do() {
    let mut plain = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    for command in ["ui.favoritesSource", "ui.copyAdminConsentLink"] {
        assert!(plain.execute(command, json!({})).unwrap_err().contains("Microsoft"), "{command}");
    }
    assert!(plain.execute("ui.dialog", json!({"name": "favoritesSource"})).is_err());
    let mut rig = Rig::organisation();
    for odd in [
        json!(null),
        json!([]),
        json!({"account": "x"}),
        json!({"account": 999_999}),
        json!({"source": 7}),
        json!({"source": "sideways"}),
        json!({"source": ["ews"]}),
        json!({"source": "x".repeat(10_000)}),
        json!({"answer": {"outcome": "approval", "why": "x".repeat(10_000)}}),
        json!({"answer": {"outcome": "blocked", "folders": 7}}),
    ] {
        let _ = rig.app.execute("ui.favoritesSource", odd.clone());
        let _ = rig.app.execute("ui.copyAdminConsentLink", odd.clone());
        let _ = rig.app.execute("ui.favorites", odd.clone());
        let _ = rig.app.execute("ui.dialog", json!({"name": "favoritesSource", "account": odd}));
        let _ = rig.app.execute("ui.closeDialog", json!({}));
        let _ = rig.app.execute("ui.cancelSignIn", json!({}));
        rig.settle();
        rig.look_all(SECOND);
    }
    assert!(rig.app.execute("ui.favoritesSource", json!({"source": "sideways"})).unwrap_err().contains("graph, ews or none"));
    // An account that goes takes what was remembered of it along.
    rig.choose("ews");
    rig.look_all(SECOND);
    rig.app.execute("account.remove", json!({"id": rig.account})).unwrap();
    assert!(rig.app.favorites.ews_tokens.is_empty() && rig.app.favorites.directories.is_empty() && rig.app.favorites.tenants.is_empty());
    assert!(rig.app.favorites.ews_rx.is_none() && rig.app.favorites.chose.is_empty());
}

#[test]
fn the_offer_opens_the_choice_for_an_organisation_and_the_choice_does_what_is_picked() {
    use egui_kittest::kittest::Queryable as _;
    let mut rig = Rig::organisation();
    rig.host.lock().unwrap().unconsented = true;
    rig.app.graph_tokens.clear();
    rig.look_all(0.0);
    let (mut harness, host, [account, ..]) = window(rig);
    let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
    let offer = at(&harness, "favoritesOffer", account).expect("the offer");
    // A click opens the choice, and sends nobody anywhere by itself.
    click(&mut harness, offer.center());
    assert_eq!(harness.state().dialog.as_ref().map(|dialog| dialog.name()), Some("favoritesSource"));
    {
        let host = host.lock().unwrap();
        assert!(host.sign_ins.is_empty() && host.ews_sign_ins == 0);
    }
    // The three, one under the other, each saying what there is to say of it.
    let rows: Vec<egui::Rect> = ["favoritesChooseGraph", "favoritesChooseEws", "favoritesChooseNone"]
        .iter()
        .map(|kind| at(&harness, kind, account).expect("a way"))
        .collect();
    assert!(rows.windows(2).all(|pair| pair[1].min.y >= pair[0].max.y));
    for said in [
        "Microsoft Graph",
        "Exchange Web Services",
        "Not now",
        "1 April 2027",
        "administrator",
        "MailCraft keeps its own Favorites",
        "avery@mail.example",
    ] {
        assert!(harness.query_all_by_label_contains(said).next().is_some(), "{said}");
    }
    // Nothing is marked to begin with, and Continue waits for a pick.
    harness.get_by_label("Continue").click();
    harness.run_steps(3);
    assert!(harness.state().dialog.is_some() && harness.state().favorites_source(account) == FavoritesSource::Unset);
    click(&mut harness, rows[1].center());
    harness.get_by_label("Continue").click();
    harness.run_steps(4);
    assert!(harness.state().dialog.is_none());
    assert_eq!(harness.state().favorites_source(account), FavoritesSource::Ews);
    assert_eq!(harness.state().session.mailbox.account(account).unwrap().oauth.as_ref().unwrap().directory, Directory::Organisation);
    // Cancel changes nothing.
    harness.state_mut().execute("ui.dialog", json!({"name": "favoritesSource", "account": account})).unwrap();
    harness.run_steps(3);
    let none = at(&harness, "favoritesChooseNone", account).unwrap();
    click(&mut harness, none.center());
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert!(harness.state().dialog.is_none() && harness.state().favorites_source(account) == FavoritesSource::Ews);

    // What stands in the way, under the heading: the sentence, then a row for each thing to do.
    harness.state_mut().execute("ui.favoritesSource", json!({"account": account, "source": "graph"})).unwrap();
    harness.run_steps(6);
    harness.state_mut().execute("ui.favorites", json!({"account": account, "answer": {"outcome": "approval"}})).unwrap();
    harness.run_steps(3);
    let heading = at(&harness, "folderHeading", 0).unwrap();
    let line = at(&harness, "favoritesNote", account).expect("the sentence");
    let copy = at(&harness, "favoritesCopyMessage", account).expect("the message to copy");
    let help = at(&harness, "favoritesHelp", account).expect("the way to what to do next");
    let switch = at(&harness, "favoritesSwitch", account).expect("the other way");
    assert!(line.min.y >= heading.max.y - 0.5 && copy.min.y >= line.max.y - 0.5);
    assert!(help.min.y >= copy.max.y - 0.5 && switch.min.y >= help.max.y - 0.5);
    assert!(line.height() >= 24.0);
    click(&mut harness, copy.center());
    assert_eq!(host.lock().unwrap().links.len(), 1);
    assert!(harness.state().status_msg.as_ref().is_some_and(|(text, _)| text.contains("The message is on the clipboard")));
    // What to do next: the page about an approval, for this account; closed, nothing has changed.
    click(&mut harness, help.center());
    assert_eq!(harness.state().dialog.as_ref().map(|dialog| dialog.name()), Some("account"));
    assert!(harness.query_all_by_label_contains("Your organisation has to approve MailCraft first").next().is_some());
    harness.get_by_label("Not now").click();
    harness.run_steps(4);
    assert!(harness.state().dialog.is_none() && harness.state().favorites_source(account) == FavoritesSource::Graph);
    let switch = at(&harness, "favoritesSwitch", account).expect("the other way");
    click(&mut harness, switch.center());
    assert_eq!(harness.state().favorites_source(account), FavoritesSource::Ews);
    assert!(at(&harness, "favoritesNote", account).is_none(), "another way was chosen: what was said of the first is gone");
    // The organisation doesn't allow Exchange Web Services: the sentence and one way out.
    harness.state_mut().execute("ui.favorites", json!({"account": account, "answer": {"outcome": "blocked"}})).unwrap();
    harness.run_steps(3);
    let line = at(&harness, "favoritesNote", account).expect("the sentence");
    let switch = at(&harness, "favoritesSwitch", account).expect("the other way");
    assert!(at(&harness, "favoritesCopyMessage", account).is_none() && at(&harness, "favoritesHelp", account).is_none());
    assert!(switch.min.y >= line.max.y - 0.5);
    let before = host.lock().unwrap().sign_ins.len();
    click(&mut harness, switch.center());
    harness.run_steps(4);
    assert_eq!(harness.state().favorites_source(account), FavoritesSource::Graph);
    assert_eq!(host.lock().unwrap().sign_ins.len(), before + 1);
}

#[test]
fn file_options_has_the_switch_and_nothing_under_it() {
    let mut rig = Rig::organisation();
    rig.look_all(0.0);
    let (mut harness, _, [account, ..]) = window(rig);
    let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(at(&harness, "favoritesOption", 0).is_some());
    for kind in ["favoritesGraph", "favoritesEws", "favoritesNone", "favoritesConnect"] {
        assert!(at(&harness, kind, account).is_none(), "{kind}");
    }
    // The choice is still the owner's, by its command (the folder pane's line, Add Account).
    harness.state_mut().execute("ui.favoritesSource", json!({"account": account, "source": "none"})).unwrap();
    assert_eq!(harness.state().favorites_source(account), FavoritesSource::None);
    assert!(!favorites_from_server(&harness.state().session, account));
    harness.state_mut().execute("ui.favoritesSource", json!({"account": account, "source": "ews"})).unwrap();
    assert_eq!(harness.state().favorites_source(account), FavoritesSource::Ews);

    // From 1 April 2027 by the session's clock, Exchange Web Services is not among them, and
    // an account set to it says so in a line and offers Graph.
    let mut app = MailApp::new(Session::at(harness.state().session.mailbox.clone(), 1_806_580_800, 0), Services::default());
    app.autosave = false;
    assert_eq!(app.favorites_source(account), FavoritesSource::Ews);
    assert_eq!(app.favorites_note_of(account), Some(Note::Ended));
    assert_eq!(app.favorites_steps(account, &Note::Ended), [Step::Use(FavoritesSource::Graph)]);
    let state = app.execute("ui.favorites", json!({"account": account})).unwrap();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["offered"].clone()),
        (json!("ended"), json!(EWS_SWITCHED_OFF), json!(["graph", "none"]))
    );
    assert!(app.execute("ui.favoritesSource", json!({"account": account, "source": "ews"})).unwrap_err().contains("1 April 2027"));
    app.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    harness.run_steps(3);
    assert!(at(&harness, "favoritesNote", account).is_some() && at(&harness, "favoritesSwitch", account).is_some());
}
