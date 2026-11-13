// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A search asks the mailboxes' own servers (R6.2), so mail that was never downloaded is found.
//! Every Microsoft and IMAP mailbox the search covers is asked at once, a short moment after the
//! owner stops typing, and what a mailbox finds joins the same list while the others are still
//! being searched. A Microsoft account needs a Graph token of this session that is still good;
//! an IMAP account the password or access token this session has. Without one, or when the last
//! try couldn't reach the server, nothing is asked and the list is what is already here. Opening
//! a result fetches it into its folder and keeps it in that row.

use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::search_server;
use mailcraft_model::Id;
use mailcraft_model::sync::{GraphSearchRequest, GraphSearchResult};
use serde_json::json;

use crate::MailApp;

/// The folders of an IMAP account the search asks, by the names the server uses. One named
/// folder, or every mail folder except Deleted Items, Junk and Outbox when the search is the
/// whole mailbox.
fn imap_folders(session: &mailcraft_engine::Session, account: Id, folder: &str) -> Vec<String> {
    if !folder.is_empty() {
        return vec![folder.to_string()];
    }
    session
        .mailbox
        .folders
        .iter()
        .filter(|f| f.account == account && !f.remote.is_empty())
        .filter(|f| !matches!(f.kind, mailcraft_model::FolderKind::Deleted | mailcraft_model::FolderKind::Junk | mailcraft_model::FolderKind::Outbox))
        .map(|f| f.remote.clone())
        .take(24)
        .collect()
}

fn is_gmail(account: &mailcraft_model::Account) -> bool {
    account.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Google)
        || account.incoming.as_ref().is_some_and(|s| s.host.eq_ignore_ascii_case("imap.gmail.com"))
}

/// What the host is asked to search ([`crate::Services::graph_search`]).
pub struct GraphSearchJob {
    pub request: GraphSearchRequest,
    /// A Graph token of this session that covers the mailbox's mail.
    pub access: String,
}

/// One mailbox being searched, and the answer to use if its thread goes away.
struct Pending {
    unreached: GraphSearchResult,
    rx: Receiver<GraphSearchResult>,
    /// A final answer (not `partial`) already arrived, so a dropped thread is not a failure.
    settled: bool,
}

/// The pages being asked for, in the window: whose, and the answer to come. Several mailboxes
/// at once. `wait_query` is the text the server search is waiting out, so a keystroke doesn't
/// ask the servers until the owner pauses.
#[derive(Default)]
pub struct ServerSearching {
    pending: Vec<Pending>,
    pub(crate) wait_query: String,
    pub(crate) wait_at: Option<std::time::Instant>,
    /// Further pages of a Microsoft mailbox asked on their own after the first.
    auto_pages: u8,
    /// The message a search row opened, being fetched whole.
    pub(crate) opening: Option<Opening>,
}

/// One IMAP message asked for because its search row was opened.
pub(crate) struct Opening {
    id: Id,
    rx: Receiver<Result<mailcraft_model::Message, String>>,
}

impl ServerSearching {
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// An IMAP account searched on its own server. The secret is the password or the access token.
/// It is never written out and never shown.
pub struct ImapSearchJob {
    pub account: mailcraft_model::Account,
    pub secret: String,
    pub ticket: u64,
    pub folders: Vec<String>,
    pub query: String,
    /// Gmail, and the search is the whole mailbox: All Mail is asked once.
    pub gmail_all: bool,
}

/// One IMAP message to fetch whole. The secret is never written out and never shown.
pub struct ImapFetchJob {
    pub account: mailcraft_model::Account,
    pub secret: String,
    pub folder: String,
    pub uid: u32,
}

impl std::fmt::Debug for ImapFetchJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapFetchJob").field("account", &self.account.id).field("uid", &self.uid).finish()
    }
}

impl std::fmt::Debug for ImapSearchJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapSearchJob")
            .field("account", &self.account.id)
            .field("folders", &self.folders.len())
            .field("gmail_all", &self.gmail_all)
            .finish()
    }
}

/// What the link below the local results is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Not shown: no search, nothing of it the server understands, or no Microsoft mailbox in it.
    None,
    /// More results on the server: the owner may ask.
    Ask,
    /// Greyed: there is no way to ask now (no sign-in of this session, or no network). `retry`:
    /// the last try couldn't reach the server, and a click tries again.
    NotConnected { retry: bool },
    /// Asked; the answers are on their way.
    Searching,
    /// Asked, and the servers have no more.
    End,
}

impl MailApp {
    /// A Graph token of this session for `account` that is still good.
    fn search_token(&self, account: Id) -> Option<String> {
        self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone())
    }

    /// What the link below the local results is now.
    pub fn server_link(&self) -> Link {
        if search_server::offered_reason(&self.session).is_some() {
            return Link::None;
        }
        let search = &self.session.server_search;
        if self.server_searching.busy() || search.waiting() {
            return Link::Searching;
        }
        // (For a search in view that differs from the one held, nothing has been asked yet.)
        let held = search_server::is_current(&self.session);
        if held && search.asked() && !search.more() {
            return Link::End;
        }
        // A Microsoft mailbox needs a token of this session. An IMAP mailbox is asked on its own
        // server; whether the password is here is found out when the search starts.
        let mailboxes = search_server::mailboxes_for(&self.session);
        let graph = self.services.graph_search.is_some() && mailboxes.iter().any(|(account, _)| self.search_token(*account).is_some());
        let imap = self.services.imap_search.is_some()
            && mailboxes
                .iter()
                .any(|(account, _)| self.session.mailbox.account(*account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Imap));
        let reachable = graph || imap;
        match (reachable, held && search.offline) {
            (false, _) => Link::NotConnected { retry: false },
            (true, true) => Link::NotConnected { retry: true },
            (true, false) => Link::Ask,
        }
    }

    /// The next page of each mailbox the search goes to. Microsoft accounts go to Graph, IMAP
    /// accounts to their own server, all of them at once. One without a sign-in of this session
    /// is answered here as not reached.
    pub(crate) fn more_on_server(&mut self) {
        let requests = match search_server::requests(&mut self.session) {
            Ok(requests) => requests,
            Err(e) => return self.status(e.to_string()),
        };
        for request in requests {
            let kind = self.session.mailbox.account(request.account).map(|a| a.kind);
            if kind == Some(mailcraft_model::AccountKind::Imap) {
                self.start_imap_search(request);
            } else {
                self.start_graph_search(request);
            }
        }
    }

    fn start_graph_search(&mut self, request: GraphSearchRequest) {
        let unreached = self.unreached(&request);
        let (Some(search), Some(access)) = (self.services.graph_search.as_ref().filter(|_| !self.offline()), self.search_token(request.account))
        else {
            let _ = search_server::apply(&mut self.session, unreached);
            return;
        };
        let rx = search(GraphSearchJob { request, access });
        self.server_searching.pending.push(Pending { unreached, rx, settled: false });
    }

    fn start_imap_search(&mut self, request: GraphSearchRequest) {
        let unreached = self.unreached(&request);
        let Some(account) = self.session.mailbox.account(request.account).cloned() else {
            let _ = search_server::apply(&mut self.session, unreached);
            return;
        };
        let secret = self.imap_secret(request.account);
        let (Some(search), Some(secret)) = (self.services.imap_search.as_ref().filter(|_| !self.offline()), secret) else {
            let _ = search_server::apply(&mut self.session, unreached);
            return;
        };
        let folders = imap_folders(&self.session, request.account, &request.folder);
        let gmail_all = request.folder.is_empty() && is_gmail(&account);
        let rx = search(ImapSearchJob { account, secret, ticket: request.ticket, folders, query: request.query, gmail_all });
        self.server_searching.pending.push(Pending { unreached, rx, settled: false });
    }

    fn unreached(&self, request: &GraphSearchRequest) -> GraphSearchResult {
        GraphSearchResult {
            account: request.account,
            email: request.email.clone(),
            ticket: request.ticket,
            offline: true,
            errors: vec![tl!("Not connected").to_string()],
            ..Default::default()
        }
    }

    /// Each frame: take in one page from each mailbox that has one. A mailbox that is still
    /// searching keeps its place, so what it already found stays on screen while the others run.
    /// (Public for a host's tests, which have no window.)
    pub fn drive_server_search(&mut self) {
        self.take_opened();
        let mut came = Vec::new();
        self.server_searching.pending.retain_mut(|pending| match pending.rx.try_recv() {
            Ok(result) => {
                pending.settled = !result.partial;
                came.push(result);
                !pending.settled
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            // A host that went away before a final answer: as not reached.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if !pending.settled {
                    came.push(pending.unreached.clone());
                }
                false
            }
        });
        for result in came {
            if let Err(e) = search_server::apply(&mut self.session, result) {
                self.status(e.to_string());
            }
        }
        // A Microsoft mailbox's next page, while the owner is still on this search. A few pages,
        // not the whole mailbox: a specific word is among the first. A page that failed is not
        // asked again on its own; the owner can.
        let search = &self.session.server_search;
        let follow = search.errors.is_empty() && !search.offline && search.mailboxes.iter().any(|m| !m.done && !m.waiting && !m.next.is_empty());
        if !self.server_searching.busy() && self.server_searching.auto_pages < 4 && search_server::is_current(&self.session) && follow {
            self.server_searching.auto_pages = self.server_searching.auto_pages.saturating_add(1);
            self.more_on_server();
        }
    }

    /// A search the owner typed, asked of the servers once they pause. Every mailbox at once.
    pub(crate) fn auto_server_search(&mut self, ctx: &egui::Context) {
        if self.offline() || self.unconfirmed {
            return;
        }
        let query = self.session.view.search.trim().to_string();
        if query.is_empty() {
            self.server_searching.wait_query.clear();
            self.server_searching.wait_at = None;
            self.server_searching.auto_pages = 0;
            return;
        }
        if self.server_searching.wait_query != query {
            self.server_searching.wait_query = query;
            self.server_searching.wait_at = Some(std::time::Instant::now());
            self.server_searching.auto_pages = 0;
            crate::software::repaint_after(ctx, std::time::Duration::from_millis(450));
            return;
        }
        let Some(at) = self.server_searching.wait_at else { return };
        let wait = std::time::Duration::from_millis(450);
        if at.elapsed() < wait {
            crate::software::repaint_after(ctx, wait.saturating_sub(at.elapsed()));
            return;
        }
        self.server_searching.wait_at = None;
        if search_server::offered_reason(&self.session).is_some() || self.server_searching.busy() {
            return;
        }
        if search_server::is_current(&self.session) && self.session.server_search.asked() {
            return;
        }
        self.more_on_server();
    }

    /// The password, or a still-good access token, this session has for an IMAP account.
    fn imap_secret(&mut self, account: Id) -> Option<String> {
        if let Some(pw) = self.passwords.get(&account).cloned() {
            return Some(pw);
        }
        if let Some(pw) = self.remembered("password", account) {
            self.passwords.insert(account, pw.clone());
            return Some(pw);
        }
        self.tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone())
    }

    /// The owner opened the server result at `index`: it comes into its folder and is selected,
    /// and stays in the same row. An IMAP message is fetched whole for the reading pane.
    pub(crate) fn open_server_result(&mut self, index: usize) {
        if let Err(e) = self.run("search.openServerResult", json!({"index": index})) {
            self.status(e);
            return;
        }
        self.reveal_selection = false;
        let Some(found) = self.session.server_search.results.get(index).cloned() else { return };
        let Some(id) = found.opened else { return };
        let Some(uid) = found.found.server.id.strip_prefix("imap:").and_then(|n| n.parse().ok()).filter(|uid: &u32| *uid != 0) else { return };
        self.start_imap_open(id, found.account, found.folder, uid);
    }

    /// Fetch the opened IMAP message whole. A Microsoft message is fetched the way any unfetched
    /// message of its account is, once it is selected.
    fn start_imap_open(&mut self, id: Id, account: Id, folder: Id, uid: u32) {
        let Some(account) = self.session.mailbox.account(account).filter(|a| a.kind == mailcraft_model::AccountKind::Imap).cloned() else {
            return;
        };
        let Some(remote) = self.session.mailbox.folder(folder).map(|f| f.remote.clone()).filter(|name| !name.is_empty()) else {
            return;
        };
        let secret = self.imap_secret(account.id);
        let Some(fetch) = self.services.imap_fetch.as_ref().filter(|_| !self.offline()) else { return };
        let Some(secret) = secret else { return };
        let rx = fetch(ImapFetchJob { account, secret, folder: remote, uid });
        self.server_searching.opening = Some(Opening { id, rx });
    }

    fn take_opened(&mut self) {
        let came = self.server_searching.opening.as_mut().map(|opening| (opening.id, opening.rx.try_recv()));
        let Some((id, came)) = came else { return };
        match came {
            Ok(Ok(read)) => {
                self.server_searching.opening = None;
                if read.body.text.is_empty() && read.body.html.is_none() {
                    return;
                }
                mailcraft_engine::cmd::imap_again::take_again(&mut self.session.mailbox, id, read);
                self.session.mailbox_changed();
            }
            Ok(Err(text)) => {
                self.server_searching.opening = None;
                self.status(text);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.server_searching.opening = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use mailcraft_engine::Session;
    use mailcraft_model::sync::{FolderListing, GraphFetchResult, GraphFolder, GraphMailResult, GraphMessage, ServerMessage, Tokens};
    use mailcraft_model::{FolderKind, Id};
    use serde_json::json;

    use super::*;
    use crate::Services;
    use crate::graph_fetch::GraphFetchJob;

    type Jobs = Arc<Mutex<Vec<(GraphSearchJob, mpsc::Sender<GraphSearchResult>)>>>;
    type Fetches = Arc<Mutex<Vec<GraphFetchJob>>>;

    const NOW: i64 = 1_790_000_000;

    fn token() -> Tokens {
        Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
    }

    /// A window with a Microsoft account whose Inbox and Archive are listed, a host that keeps
    /// every search and fetch for the test to answer, and (`signed_in`) a Graph token of this
    /// session.
    fn window(signed_in: bool) -> (MailApp, Id, Jobs, Fetches) {
        let mut app = MailApp::new(Session::empty(), Services::default());
        app.autosave = false;
        let account = app.execute("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
        let folders = vec![folder("in", "Inbox", FolderKind::Inbox), folder("ar", "Archive", FolderKind::Archive)];
        let listing = GraphMailResult {
            account,
            email: "owner@example.org".into(),
            listing: Some(FolderListing { folders, complete: true }),
            ..Default::default()
        };
        mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut app.session, listing).unwrap();
        if signed_in {
            app.seed_graph_token(account, token());
        }
        let (jobs, fetches): (Jobs, Fetches) = Default::default();
        let kept = jobs.clone();
        app.services.graph_search = Some(Box::new(move |job: GraphSearchJob| {
            let (tx, rx) = mpsc::channel();
            kept.lock().unwrap().push((job, tx));
            rx
        }));
        let fetched = fetches.clone();
        app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
            fetched.lock().unwrap().push(job);
            mpsc::channel::<GraphFetchResult>().1
        }));
        (app, account, jobs, fetches)
    }

    fn found(id: &str, subject: &str, received: i64) -> GraphMessage {
        GraphMessage {
            server: ServerMessage { id: id.into(), folder: "ar".into(), ..Default::default() },
            subject: subject.into(),
            received,
            sent: received,
            ..Default::default()
        }
    }

    fn answer(job: &GraphSearchJob, messages: Vec<GraphMessage>, next: &str) -> GraphSearchResult {
        GraphSearchResult {
            account: job.request.account,
            email: job.request.email.clone(),
            ticket: job.request.ticket,
            messages,
            next: next.into(),
            ..Default::default()
        }
    }

    #[test]
    fn nothing_goes_to_the_server_until_the_owner_clicks_and_a_result_opened_is_fetched() {
        let (mut app, account, jobs, fetches) = window(true);
        app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
        for _ in 0..3 {
            app.drive_server_search();
        }
        assert_eq!(app.server_link(), Link::Ask);
        assert!(jobs.lock().unwrap().is_empty(), "not asked by itself");

        app.run("ui.moreResultsOnServer", json!({})).unwrap();
        assert_eq!(app.server_link(), Link::Searching);
        let (job, tx) = jobs.lock().unwrap().pop().unwrap();
        assert_eq!((job.request.account, job.access.as_str(), job.request.query.as_str()), (account, "stand-in", "budget"));
        let messages = (0..50).map(|i| found(&format!("AAkA-{i}"), &format!("Budget {i}"), NOW - i)).collect();
        tx.send(answer(&job, messages, "https://graph.microsoft.com/v1.0/me/messages?$skip=50")).unwrap();
        app.drive_server_search();
        assert_eq!(app.session.server_search.results.len(), 50);
        assert_eq!(app.server_link(), Link::Searching, "the next page is asked while these are shown");
        let (job, tx) = jobs.lock().unwrap().pop().unwrap();
        assert!(job.request.next.ends_with("$skip=50"));

        // Opened: in its folder, selected, and fetched whole as the reading pane shows it.
        app.open_server_result(0);
        let id = app.session.sel.messages.first().copied().unwrap();
        let m = app.session.mailbox.message(id).unwrap();
        assert!(m.unfetched && m.subject == "Budget 0" && Some(m.folder) == app.session.mailbox.folder_by_server(account, "ar"));
        app.drive_fetch();
        let asked: Vec<Vec<String>> = fetches.lock().unwrap().iter().map(|j| j.request.ids.clone()).collect();
        assert!(asked.iter().any(|ids| ids == &["AAkA-0".to_string()]), "{asked:?}");

        // The next page comes in on its own; then the end.
        tx.send(answer(&job, Vec::new(), "")).unwrap();
        app.drive_server_search();
        assert_eq!(app.server_link(), Link::End);
        assert!(app.run("ui.moreResultsOnServer", json!({})).is_err());
        // A new search forgets them.
        app.run("view.search", json!({"query": "lunch", "scope": "everywhere"})).unwrap();
        assert!(app.session.server_search.results.is_empty() && app.server_link() == Link::Ask);
    }

    /// The section On the server, with its rows' folders under the time and one row selected, in
    /// light and dark at 100 % and 200 %, as PNG files to look at:
    /// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_server_results -- --ignored`
    #[test]
    #[ignore = "writes screenshots, for looking at them"]
    fn shots_of_server_results() {
        let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
            eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
            return;
        };
        for theme in ["light", "dark"] {
            for scale in [1.0, 2.0] {
                let (mut app, _, jobs, _) = window(true);
                app.execute("ui.theme", json!({"value": theme})).unwrap();
                app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
                app.run("ui.moreResultsOnServer", json!({})).unwrap();
                let (job, tx) = jobs.lock().unwrap().pop().unwrap();
                let mut messages: Vec<GraphMessage> = (0..4).map(|i| found(&format!("AAkA-{i}"), &format!("Budget {i}"), NOW - i * 3_600)).collect();
                messages[1].server.folder = "in".into();
                for (i, m) in messages.iter_mut().enumerate() {
                    m.server.read = i != 2;
                }
                tx.send(answer(&job, messages, "")).unwrap();
                app.drive_server_search();
                app.open_server_result(0);
                let mut harness = egui_kittest::Harness::builder()
                    .with_accessibility_check(false)
                    .allow_missing_glyphs()
                    .with_size(egui::vec2(1440.0, 900.0))
                    .with_pixels_per_point(scale)
                    .with_step_dt(1.0 / 60.0)
                    .wgpu()
                    .build_ui_state(
                        |ui, app: &mut MailApp| {
                            let ctx = ui.ctx().clone();
                            app.logic(&ctx);
                            app.ui(ui);
                        },
                        app,
                    );
                harness.input_mut().max_texture_side = Some(8192);
                harness.run_steps(14);
                let path = std::path::Path::new(&dir).join(format!("server-results-{theme}-{scale}x.png"));
                harness.render().unwrap().save(&path).unwrap();
                println!("{}", path.display());
            }
        }
    }

    #[test]
    fn without_a_sign_in_or_a_network_the_link_says_not_connected() {
        let (mut app, account, jobs, _) = window(false);
        app.run("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
        assert_eq!(app.server_link(), Link::NotConnected { retry: false });
        assert!(app.run("ui.moreResultsOnServer", json!({})).is_err());
        assert!(jobs.lock().unwrap().is_empty());

        // Signed in, but the server can't be reached: greyed, and a click tries again.
        app.seed_graph_token(account, token());
        app.run("ui.moreResultsOnServer", json!({})).unwrap();
        let (job, tx) = jobs.lock().unwrap().pop().unwrap();
        let offline = GraphSearchResult {
            offline: true,
            errors: vec!["Couldn't search on Microsoft's server: couldn't reach graph.microsoft.com".into()],
            ..answer(&job, Vec::new(), "")
        };
        tx.send(offline).unwrap();
        app.drive_server_search();
        assert_eq!(app.server_link(), Link::NotConnected { retry: true });
        assert_eq!(app.session.server_search.errors.len(), 1);
        app.run("ui.moreResultsOnServer", json!({})).unwrap();
        // A host that went away: as not reached.
        drop(jobs.lock().unwrap().pop());
        app.drive_server_search();
        assert_eq!(app.server_link(), Link::NotConnected { retry: true });
        // A search that involves no Microsoft mailbox has no link at all.
        app.run("view.search", json!({"query": "is:unread", "scope": "everywhere"})).unwrap();
        assert_eq!(app.server_link(), Link::None);
    }
}
