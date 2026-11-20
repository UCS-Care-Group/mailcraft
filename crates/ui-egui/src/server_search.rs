// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server (R6.2): below the local results of a search that involves a
//! Microsoft account, a link that asks its server, and its mailbox's shared mailboxes', for more;
//! the results come in a section of their own, On the server, 50 at a time. Nothing goes to the
//! server until the owner clicks. Asked only with a Graph token of this session that is still
//! good: without one (and when the last try couldn't reach the server) the link is greyed and
//! says Not connected. Opening a result fetches it into its folder, as any message not fetched
//! yet is when it is opened ([`crate::graph_fetch`]).

use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::search_server;
use mailcraft_model::Id;
use mailcraft_model::sync::{GraphSearchRequest, GraphSearchResult};
use serde_json::json;

use crate::MailApp;

/// What the host is asked to search ([`crate::Services::graph_search`]).
pub struct GraphSearchJob {
    pub request: GraphSearchRequest,
    /// A Graph token of this session that covers the mailbox's mail.
    pub access: String,
}

/// The pages being asked for, in the window: whose, and the answer to come.
#[derive(Default)]
pub struct ServerSearching {
    pending: Vec<(GraphSearchResult, Receiver<GraphSearchResult>)>,
}

impl ServerSearching {
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
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
        let reachable = self.services.graph_search.is_some()
            && search_server::mailboxes_for(&self.session).iter().any(|(account, _)| self.search_token(*account).is_some());
        match (reachable, held && search.offline) {
            (false, _) => Link::NotConnected { retry: false },
            (true, true) => Link::NotConnected { retry: true },
            (true, false) => Link::Ask,
        }
    }

    /// The owner clicked More results on the server: the next page of each mailbox the search goes
    /// to is asked for. One without a sign-in of this session is answered here as not reached.
    pub(crate) fn more_on_server(&mut self) {
        let requests = match search_server::requests(&mut self.session) {
            Ok(requests) => requests,
            Err(e) => return self.status(e.to_string()),
        };
        for request in requests {
            // What is said for it when no answer comes: not reached.
            let unreached = GraphSearchResult {
                account: request.account,
                email: request.email.clone(),
                ticket: request.ticket,
                offline: true,
                errors: vec![tl!("Not connected").to_string()],
                ..Default::default()
            };
            let (Some(search), Some(access)) = (self.services.graph_search.as_ref().filter(|_| !self.offline()), self.search_token(request.account))
            else {
                let _ = search_server::apply(&mut self.session, unreached);
                continue;
            };
            let rx = search(GraphSearchJob { request, access });
            self.server_searching.pending.push((unreached, rx));
        }
    }

    /// Each frame: take in the pages that came. (Public for a host's tests, which have no window.)
    pub fn drive_server_search(&mut self) {
        let mut came = Vec::new();
        self.server_searching.pending.retain(|(unreached, rx)| match rx.try_recv() {
            Ok(result) => {
                came.push(result);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            // A host that went away: as not reached.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                came.push(unreached.clone());
                false
            }
        });
        for result in came {
            if let Err(e) = search_server::apply(&mut self.session, result) {
                self.status(e.to_string());
            }
        }
    }

    /// The owner opened the server result at `index`: it comes into its folder and is selected,
    /// and the reading pane fetches it whole.
    pub(crate) fn open_server_result(&mut self, index: usize) {
        if let Err(e) = self.run("search.openServerResult", json!({"index": index})) {
            self.status(e);
        }
        self.reveal_selection = false;
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
        assert_eq!(app.server_link(), Link::Ask, "more to ask for");

        // Opened: in its folder, selected, and fetched whole as the reading pane shows it.
        app.open_server_result(0);
        let id = app.session.sel.messages.first().copied().unwrap();
        let m = app.session.mailbox.message(id).unwrap();
        assert!(m.unfetched && m.subject == "Budget 0" && Some(m.folder) == app.session.mailbox.folder_by_server(account, "ar"));
        app.drive_fetch();
        let asked: Vec<Vec<String>> = fetches.lock().unwrap().iter().map(|j| j.request.ids.clone()).collect();
        assert!(asked.iter().any(|ids| ids == &["AAkA-0".to_string()]), "{asked:?}");

        // The next page, at the owner's click; then the end.
        app.run("ui.moreResultsOnServer", json!({})).unwrap();
        let (job, tx) = jobs.lock().unwrap().pop().unwrap();
        assert!(job.request.next.ends_with("$skip=50"));
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
