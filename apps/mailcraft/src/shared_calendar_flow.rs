// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendars from the window to the server and back (#70): the window's
//! calendar timer, this host's Microsoft Graph job and `mailcraft-net`, against the stand-in
//! (`mailcraft_net::graph::standin`). Nothing here leaves this machine, and nobody is signed in.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Mailbox, Rights};
use mailcraft_net::NetError;
use mailcraft_net::graph::standin::{CalendarAccess, StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;
const SHARED: &str = "support@example.org";
const OWNER: &str = "avery@mail.example";

fn utc(day: u8, hour: u8) -> i64 {
    mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

struct Flow {
    app: MailApp,
    server: StandIn,
    shared: Id,
    at: f64,
    /// Whether the owner has allowed sending from shared mailboxes yet, as the provider says when a
    /// token for it is asked for; and how often one was asked for.
    allowed: Arc<Mutex<(bool, usize)>>,
    _route: mailcraft_net::graph::standin::Routed,
}

fn renewed() -> Result<GraphTokens, NetError> {
    let tokens = Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
}

/// The window, a Microsoft account (whose own calendar has been looked at already) with a shared
/// mailbox under it, and the stand-in in Graph's place with `access` to the shared mailbox's
/// calendars.
fn flow(access: CalendarAccess) -> Flow {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.owner = OWNER.into();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        let team = state.add_other_calendar(SHARED, "Team", false);
        state.add_other_event(SHARED, &main, "Stand-up", utc(14, 17), "busy");
        state.add_other_event(SHARED, &main, "Away day", utc(15, 9), "outOfOffice");
        state.add_other_event(SHARED, &team, "Rota", utc(14, 18), "busy");
        state.share_calendars(SHARED, access);
        let own = state.main_calendar();
        state.add_event(&own, "Dentist", utc(14, 19));
    }
    let route = server.route();
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let allowed: Arc<Mutex<(bool, usize)>> = Arc::default();
    let (said, url) = (allowed.clone(), server.url());
    // (A job of its own, as the host runs one: a Send/Receive hands its mail over piece by piece.)
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (said, url) = (said.clone(), url.clone());
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&url);
            let send = move |_: &mailcraft_model::OAuth, _: &str| {
                let mut said = said.lock().unwrap();
                said.1 += 1;
                if said.0 { renewed() } else { Err(NetError::ConsentNeeded("consent_required".into())) }
            };
            let _ = tx.send(super::graph_sending_with(request, "UTC", &|_, _, _| renewed(), &send));
        });
        rx
    }));
    app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let fresh = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.seed_graph_token(owner, fresh.clone());
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    app.seed_shared_calendar_token(shared, fresh);
    app.execute("view.calendar", json!({})).unwrap();
    let mut flow = Flow { app, server, shared, at: mailcraft_ui_egui::now_ms(), allowed, _route: route };
    // The owner's own calendar comes down first (nothing of it is what these look at).
    assert!(flow.look());
    flow.take_in();
    flow.server.state().requests.clear();
    flow
}

impl Flow {
    fn look(&mut self) -> bool {
        // (A second on: the owner's own calendar, asked for half a minute apart, is not due.)
        self.at += 1_000.0;
        self.app.calendar_tick_at(true, self.at)
    }

    /// Time goes by, a few seconds at a time, and the window looks at the calendar each time and
    /// takes in whatever the host answered: enough for a change to wait its quiet moment, go, and
    /// be seen again.
    fn go(&mut self) {
        for _ in 0..8 {
            self.at += 3_000.0;
            if self.app.calendar_tick_at(true, self.at) {
                self.take_in();
            }
        }
    }

    /// The shared mailbox's main calendar, ticked, and read once.
    fn shown(&mut self) -> Id {
        let placeholder = self.calendar(SHARED).id;
        self.app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
        assert!(self.look());
        self.take_in();
        self.calendar("Calendar").id
    }

    /// What the stand-in's shared mailbox has, by title.
    fn there(&self) -> Vec<String> {
        let mut state = self.server.state();
        let mut titles: Vec<String> = state.other(SHARED).map(|o| o.events.iter().map(|e| e.subject.clone()).collect()).unwrap_or_default();
        titles.sort();
        titles
    }

    /// What the owner's own calendar has on the stand-in.
    fn mine(&self) -> Vec<String> {
        self.server.state().events.iter().map(|e| e.subject.clone()).collect()
    }

    /// Everything the stand-in was asked that changes something, and everything asked at `/me`.
    fn writes(&self) -> Vec<String> {
        self.server.state().requests.iter().filter(|asked| asked.method != "GET").map(|asked| asked.path.join("/")).collect()
    }

    /// What was written at `/me` (the owner's own calendar is read there all the while; what is
    /// asked of a shared mailbox is never at `/me`, which the net crate's own tests prove for every
    /// request: here, no change reached it).
    fn at_me(&self) -> Vec<String> {
        self.writes().into_iter().filter(|path| path.starts_with("v1.0/me")).collect()
    }

    /// Time goes by far enough for what was held back after a failure to be tried again (the
    /// session's clock stands still).
    fn retry(&mut self) {
        for change in &mut self.app.session.mailbox.event_changes {
            change.retry_at = 0;
        }
        self.at += 650_000.0;
    }

    fn take_in(&mut self) {
        for _ in 0..2_000 {
            self.app.poll_calendar();
            self.app.poll_graph();
            if !self.app.calendar_syncing() && !self.app.graph_running() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the job didn't end");
    }

    fn calendar(&self, name: &str) -> mailcraft_model::Calendar {
        let all = &self.app.session.mailbox.calendars;
        all.iter().find(|c| c.origin.account == self.shared && c.name == name).cloned().unwrap_or_else(|| panic!("no calendar {name}"))
    }

    fn titles(&self) -> Vec<String> {
        let mut titles: Vec<String> =
            self.app.session.mailbox.events.values().filter(|e| e.origin.account == self.shared).map(|e| e.title.clone()).collect();
        titles.sort();
        titles
    }

    /// Every address the stand-in was asked, by method and path.
    fn asked(&self) -> Vec<String> {
        self.server.state().requests.iter().map(|asked| format!("{} {}", asked.method, asked.path.join("/"))).collect()
    }
}

#[test]
fn a_shared_mailboxs_calendars_come_down_when_ticked_and_nothing_is_asked_at_me() {
    let mut flow = flow(CalendarAccess::Open);
    // Nothing is asked of the server for a calendar nobody ticked.
    assert!(!flow.look() && flow.asked().is_empty());
    let placeholder = flow.calendar(SHARED).id;
    flow.app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
    assert!(flow.look());
    flow.take_in();
    // The mailbox's calendars are listed, unticked but for the one that was ticked, and the
    // ticked one's events are there; the other one's are not asked for.
    assert_eq!(flow.calendar("Calendar").rights, Rights::Edit);
    assert!(flow.calendar("Calendar").visible && !flow.calendar("Team").visible);
    assert_eq!(flow.titles(), ["Away day", "Stand-up"]);
    assert_eq!(flow.calendar("Team").rights, Rights::Read, "read where the owner may not change it");
    let seen = flow.asked();
    assert!(!seen.is_empty() && seen.iter().all(|line| line.contains("users/support@example.org")), "{seen:?}");
    assert!(seen.iter().all(|line| !line.contains("/me/") && !line.ends_with("/me")), "nothing at /me: {seen:?}");
    assert!(
        seen.iter().all(|line| !line.contains("contacts") && !line.contains("masterCategories") && !line.contains("mailboxSettings")),
        "{seen:?}"
    );
    // The owner's own calendar was never mixed with it.
    assert!(!flow.titles().iter().any(|t| t == "Dentist"));
    // What is in the calendar the owner may only read is refused a change: nothing waits to go,
    // and nothing is written to the server.
    let team = flow.calendar("Team").id;
    flow.app.execute("calendar.showCalendar", json!({"id": team})).unwrap();
    flow.at += 60_000.0;
    flow.go();
    let rota = flow.app.session.mailbox.events.values().find(|e| e.title == "Rota").map(|e| e.id).unwrap();
    assert!(flow.app.execute("calendar.edit", json!({"id": rota, "title": "Changed"})).is_err());
    assert!(flow.app.session.mailbox.event_changes.is_empty());
    assert!(flow.server.state().writes().is_empty());
}

#[test]
fn changes_to_a_shared_mailboxs_calendar_go_to_its_own_address_and_never_to_me() {
    let mut flow = flow(CalendarAccess::Open);
    let main = flow.shown();
    // Made in it.
    let made = flow
        .app
        .execute("calendar.new", json!({"title": "Review", "calendar": main, "start": "2026-10-16T10:00", "end": "2026-10-16T11:00"}))
        .unwrap();
    let review = made["id"].as_u64().unwrap();
    flow.go();
    assert_eq!(flow.there(), ["Away day", "Review", "Rota", "Stand-up"]);
    assert_eq!(flow.mine(), ["Dentist"], "the owner's own calendar was not written to");
    assert!(flow.app.session.mailbox.event_changes.is_empty(), "it went, and it was seen");
    assert_eq!(flow.app.session.mailbox.event(review).map(|e| e.origin.account), Some(flow.shared));
    assert!(flow.app.session.mailbox.event(review).is_some_and(|e| !e.origin.id.is_empty()), "it is known by what the server calls it");
    // Changed.
    flow.app.execute("calendar.edit", json!({"id": review, "title": "Design review"})).unwrap();
    flow.go();
    assert_eq!(flow.there(), ["Away day", "Design review", "Rota", "Stand-up"]);
    // Deleted.
    flow.app.execute("calendar.delete", json!({"id": review})).unwrap();
    flow.go();
    assert_eq!(flow.there(), ["Away day", "Rota", "Stand-up"]);
    assert!(flow.app.session.mailbox.events.values().all(|e| e.title != "Design review" && e.title != "Review"));
    assert_eq!(flow.mine(), ["Dentist"]);
    // All of it for the mailbox's address; nothing, ever, at /me.
    let writes = flow.writes();
    assert!(writes.len() >= 3 && writes.iter().all(|line| line.contains("users/support@example.org")), "{writes:?}");
    assert!(flow.at_me().is_empty(), "{:?}", flow.at_me());
    assert!(!flow.app.has_sync_problems());
}

#[test]
fn a_change_the_server_refuses_is_put_back_and_the_window_says_why() {
    let mut flow = flow(CalendarAccess::Open);
    let main = flow.shown();
    // The owner's right to edit is taken away on the server, which the window doesn't know yet.
    {
        let mut state = flow.server.state();
        if let Some(other) = state.other(SHARED) {
            for calendar in &mut other.calendars {
                calendar.can_edit = false;
            }
        }
    }
    let made = flow
        .app
        .execute("calendar.new", json!({"title": "Review", "calendar": main, "start": "2026-10-16T10:00", "end": "2026-10-16T11:00"}))
        .unwrap();
    let review = made["id"].as_u64().unwrap();
    flow.go();
    // Nothing was written to the server, and the first refusal is tried once more later.
    assert_eq!(flow.there(), ["Away day", "Rota", "Stand-up"]);
    assert_eq!(flow.app.session.mailbox.event_changes.len(), 1);
    flow.retry();
    flow.go();
    // Refused again: what the owner wrote is not thrown away, and it is not left in the shared
    // mailbox's calendar either: it is kept on this computer, and said.
    assert_eq!(flow.there(), ["Away day", "Rota", "Stand-up"]);
    assert!(flow.app.session.mailbox.event_changes.is_empty());
    assert!(flow.app.session.mailbox.event(review).is_some_and(|e| e.origin.account != flow.shared));
    assert!(flow.app.said_text().contains("wouldn't take"), "{}", flow.app.said_text());
    assert!(flow.at_me().is_empty());
}

#[test]
fn what_waits_is_held_and_told_when_the_owner_may_no_longer_edit_and_goes_when_he_may_again() {
    let mut flow = flow(CalendarAccess::Open);
    let main = flow.shown();
    let flip = |flow: &Flow, can_edit: bool| {
        let mut state = flow.server.state();
        if let Some(other) = state.other(SHARED) {
            for calendar in &mut other.calendars {
                calendar.can_edit = can_edit;
            }
        }
    };
    flow.app.execute("calendar.new", json!({"title": "Review", "calendar": main, "start": "2026-10-16T10:00", "end": "2026-10-16T11:00"})).unwrap();
    // The server stops taking changes; the change is tried once and held; the list that comes
    // next says the rights shrank.
    flip(&flow, false);
    flow.app.calendar_list_due(flow.shared);
    flow.go();
    assert_eq!(flow.calendar("Calendar").rights, Rights::Read);
    assert_eq!(flow.there(), ["Away day", "Rota", "Stand-up"]);
    assert_eq!(flow.app.session.mailbox.event_changes.len(), 1, "held, not dropped");
    assert!(flow.app.said_text().contains("held here"), "{}", flow.app.said_text());
    // Nothing more goes while the rights are what they are.
    flow.server.state().requests.clear();
    flow.app.calendar_list_due(flow.shared);
    flow.go();
    assert!(flow.writes().is_empty(), "{:?}", flow.writes());
    assert_eq!(flow.app.session.mailbox.event_changes.len(), 1);
    // Rights come back: it goes.
    flip(&flow, true);
    flow.app.calendar_list_due(flow.shared);
    flow.retry();
    flow.go();
    assert_eq!(flow.there(), ["Away day", "Review", "Rota", "Stand-up"]);
    assert!(flow.app.session.mailbox.event_changes.is_empty());
    assert!(flow.at_me().is_empty());
}

#[test]
fn a_meeting_in_a_shared_mailbox_is_answered_from_it_once_sending_from_it_is_allowed() {
    let mut flow = flow(CalendarAccess::Open);
    {
        let mut state = flow.server.state();
        let main = state.other(SHARED).and_then(|o| o.calendars.iter().find(|c| c.main).map(|c| c.id.clone())).unwrap();
        let invited = state.add_other_event(SHARED, &main, "Planning", utc(20, 9), "busy");
        if let Some(other) = state.other(SHARED) {
            for event in &mut other.events {
                if event.id == invited {
                    event.is_organizer = false;
                    event.attendees = vec![json!({"emailAddress": {"address": SHARED, "name": "Support"}, "type": "required"})];
                }
            }
        }
    }
    flow.shown();
    let planning = flow.app.session.mailbox.events.values().find(|e| e.title == "Planning").map(|e| e.id).unwrap();
    flow.app.execute("calendar.respond", json!({"id": planning, "response": "accept", "send": true})).unwrap();
    flow.server.state().requests.clear();
    flow.go();
    // Sending from shared mailboxes isn't allowed yet: the answer waits, nothing is sent as the
    // owner, and the window asks, as it does for mail.
    assert!(flow.allowed.lock().unwrap().1 >= 1);
    let mail = flow.server.state().mail.clone();
    let writes = flow.writes();
    assert!(mail.is_empty() && writes.is_empty(), "{mail:?} {writes:?}");
    assert_eq!(flow.app.session.mailbox.event_changes.len(), 1);
    assert_eq!(flow.app.dialog.as_ref().map(|d| d.name()), Some("confirm"));
    // Allowed: it goes through the shared mailbox's address, and the meeting is answered there.
    flow.allowed.lock().unwrap().0 = true;
    flow.app.execute("ui.closeDialog", json!({})).unwrap();
    flow.app.execute("ui.sync", json!({"account": flow.shared})).unwrap();
    flow.take_in();
    flow.go();
    let mail = flow.server.state().mail.clone();
    assert!(mail.iter().any(|line| line.starts_with("response:Planning:accept")), "{mail:?}");
    // (A `$batch` is a read, as Graph has them: the mail run's own.)
    let writes: Vec<String> = flow.writes().into_iter().filter(|line| !line.ends_with("$batch")).collect();
    assert!(writes.iter().any(|line| line.ends_with("accept")) && writes.iter().all(|line| line.contains("users/support@example.org")), "{writes:?}");
    assert!(flow.app.session.mailbox.event_changes.is_empty());
    assert!(flow.at_me().is_empty(), "{:?}", flow.at_me());
    assert_eq!(flow.mine(), ["Dentist"]);
}

#[test]
fn a_calendar_shared_for_free_and_busy_only_is_blocks_without_subjects_that_are_only_read() {
    let mut flow = flow(CalendarAccess::FreeBusy);
    let placeholder = flow.calendar(SHARED).id;
    flow.app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
    assert!(flow.look());
    flow.take_in();
    // Its list is refused; the main calendar is its free and busy times.
    assert_eq!(flow.calendar(SHARED).rights, Rights::FreeBusy);
    let blocks: Vec<(String, mailcraft_model::ShowAs)> =
        flow.app.session.mailbox.events.values().filter(|e| e.origin.account == flow.shared).map(|e| (e.title.clone(), e.show_as)).collect();
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert!(blocks.iter().all(|(title, _)| title.is_empty()), "no subjects: {blocks:?}");
    assert!(blocks.iter().any(|(_, show)| *show == mailcraft_model::ShowAs::OutOfOffice));
    let id = flow.app.session.mailbox.events.values().find(|e| e.origin.account == flow.shared).map(|e| e.id).unwrap();
    assert!(flow.app.execute("calendar.delete", json!({"id": id})).is_err());
    let seen = flow.asked();
    assert!(seen.iter().any(|line| line.ends_with("calendar/getSchedule")), "{seen:?}");
    assert!(seen.iter().all(|line| line.contains("users/support@example.org")), "{seen:?}");
    assert!(!flow.app.has_sync_problems());
}

#[test]
fn a_shared_mailbox_nothing_is_shared_from_says_so_and_asks_nothing_at_me() {
    let mut flow = flow(CalendarAccess::Denied);
    let placeholder = flow.calendar(SHARED).id;
    flow.app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
    assert!(flow.look());
    flow.take_in();
    assert!(flow.titles().is_empty());
    let seen = flow.asked();
    assert!(!seen.is_empty() && seen.iter().all(|line| line.contains("users/support@example.org")), "{seen:?}");
    // It is said in Send/Receive's problems.
    assert!(flow.app.has_sync_problems());
}
