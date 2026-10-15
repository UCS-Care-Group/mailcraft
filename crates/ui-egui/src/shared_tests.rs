//! Shared mailboxes in the window: a Send/Receive of one goes with its owner's sign-in, and the
//! pages and dialogs that show one lay out without trouble.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// What a run was asked: whose it was, the token it carried, the names the servers are signed in
/// to with, and whether it went with a password.
type Asked = (u64, String, String, String, bool);

/// The sample mailbox with a Microsoft 365 account of work and a shared mailbox it opens.
fn app() -> (MailApp, u64, u64) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    // No rounds by the clock: only what is asked for, and what that leaves waiting.
    a.auto_sync = false;
    let owner = a
        .execute(
            "account.add",
            json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "security": "starttls"}, "auth": "oauth2", "directory": "organisation"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let shared = a.execute("account.addShared", json!({"account": owner, "address": "support@harbor.example"})).unwrap()["id"].as_u64().unwrap();
    (a, owner, shared)
}

fn fresh(access: &str) -> Tokens {
    Tokens { access: access.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() }
}

#[test]
fn a_shared_mailbox_syncs_with_its_owners_sign_in() {
    let (mut a, owner, shared) = app();
    let asked: Arc<Mutex<Vec<Asked>>> = Default::default();
    let asked2 = asked.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let login = req.account.login().to_string();
        let sending = req.sending_account().login().to_string();
        asked2.lock().unwrap().push((req.account.id, req.access_token.clone(), login, sending, !req.password.is_empty()));
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    let sign_ins: Arc<Mutex<Vec<(u64, String, bool)>>> = Default::default();
    let sign_ins2 = sign_ins.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        sign_ins2.lock().unwrap().push((req.account, req.email.clone(), req.quiet));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(fresh("tok-2"))).unwrap();
        rx
    }));
    let settle = |a: &mut MailApp| {
        for _ in 0..20 {
            a.poll_sign_in();
            a.poll_sync();
            a.auto_sync();
        }
    };
    // Nothing is at hand: the shared mailbox can't go quietly, nor can its owner.
    assert!(!a.can_sync_quietly(shared));
    // Asked for, the owner signs in (on the owner's page, for the owner's address), the owner's
    // account runs, and the shared mailbox after it with the same token.
    let started = a.execute("ui.sync", json!({"account": shared})).unwrap();
    assert_eq!(started["pending"], "signIn");
    assert_eq!(started["account"], owner);
    settle(&mut a);
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, "avery@harbor.example".to_string(), false)]);
    let runs = asked.lock().unwrap().clone();
    assert_eq!(
        runs,
        [
            (owner, "tok-2".to_string(), "avery@harbor.example".to_string(), "avery@harbor.example".to_string(), false),
            (shared, "tok-2".to_string(), "support@harbor.example".to_string(), "avery@harbor.example".to_string(), false),
        ]
    );
    // No token is kept for the shared mailbox itself; its owner's does for both.
    assert!(!a.tokens.contains_key(&shared) && a.tokens.contains_key(&owner));
    assert!(a.can_sync_quietly(shared));
    // With the owner's token at hand, it runs at once, without a sign-in.
    asked.lock().unwrap().clear();
    a.tokens.insert(owner, fresh("tok-3"));
    assert_eq!(a.execute("ui.sync", json!({"account": shared})).unwrap()["started"], true);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().first().map(|r| (r.0, r.1.clone())), Some((shared, "tok-3".to_string())));
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    // A refusal for the shared mailbox costs its owner nothing.
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let errors = vec!["Couldn't open the shared mailbox support@harbor.example: the server didn't let your account in. Opening it takes Full Access to it, which an administrator of your organisation gives (sign-in failed: AUTHENTICATE failed)".to_string()];
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), errors, ..Default::default() }).unwrap();
        rx
    }));
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(a.tokens.get(&owner).map(|t| t.access.as_str()), Some("tok-3"));
}

#[test]
fn the_dialog_adds_it_and_starts_a_send_receive() {
    let (mut a, owner, _) = app();
    a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner, "address": "sales@harbor.example"})).unwrap();
    assert!(
        matches!(&a.dialog, Some(crate::dialogs::Dialog::SharedMailbox { account, address, .. }) if *account == owner && address == "sales@harbor.example")
    );
    // Not under an account that can't open one.
    let local = a.session.mailbox.accounts[0].id;
    a.dialog = None;
    let _ = a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": local}));
    assert!(a.dialog.is_none());
}

#[test]
fn the_pages_that_show_one_lay_out_in_light_and_dark() {
    for theme in ["light", "dark"] {
        for size in [(1280.0, 800.0), (320.0, 200.0)] {
            let steps: [(&str, serde_json::Value); 1] = [("ui.backstage", json!({"value": true, "page": "info"}))];
            let (mut a, owner, shared) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            for (id, params) in &steps {
                a.execute(id, params.clone()).unwrap();
            }
            let _ = crate::tests::frames(a, size, 4);
            // The folder pane, and a message from it being written.
            let (mut a, _, _) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            a.execute("mail.new", json!({"account": shared})).unwrap();
            let _ = crate::tests::frames(a, size, 4);
            // The dialog.
            let (mut a, _, _) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            a.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
            let _ = crate::tests::frames(a, size, 4);
        }
    }
}

#[test]
fn file_info_offers_a_shared_mailbox_under_a_work_account() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.auto_sync = false;
    a.execute(
        "account.add",
        json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "outgoing": {"host": "smtp.office365.com", "security": "starttls"}, "auth": "oauth2", "provider": "microsoft", "directory": "organisation"}),
    )
    .unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    harness.run_steps(4);
    // The link is there, and nothing is drawn over it: clicking it opens its own dialog, not Add
    // Account's (the buttons put inside the card once left the next tile on top of it).
    use egui_kittest::kittest::Queryable;
    let pos = harness.get_by_label_contains("Add shared mailbox").rect().center();
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
    assert!(
        matches!(&harness.state().dialog, Some(crate::dialogs::Dialog::SharedMailbox { .. })),
        "clicking Add shared mailbox… didn't open its dialog"
    );
}

/// The menu that moves messages, drawn by itself in a window: the folders it lists by their
/// labels, and what was picked.
fn move_menu(a: MailApp) -> (egui_kittest::Harness<'static, MailApp>, Arc<Mutex<Vec<u64>>>) {
    let picked: Arc<Mutex<Vec<u64>>> = Default::default();
    let keep = picked.clone();
    let harness = egui_kittest::Harness::builder().with_size(egui::vec2(600.0, 600.0)).build_ui_state(
        move |ui, app: &mut MailApp| {
            let keep = keep.clone();
            crate::ribbon::mailbox_folder_menu(ui, app, move |_, folder| keep.lock().unwrap().push(folder));
        },
        a,
    );
    (harness, picked)
}

#[test]
fn the_move_menu_lists_every_mailbox_of_the_account_by_its_address_and_finds_a_folder() {
    use egui_kittest::kittest::Queryable as _;
    let (mut a, owner, shared) = app();
    assert_eq!(crate::ribbon::mailbox_family(&a, owner), [owner, shared]);
    assert_eq!(crate::ribbon::mailbox_family(&a, shared), [owner, shared]);
    let inbox = a.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Inbox).unwrap();
    let theirs = a.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Inbox).unwrap();
    a.execute("ui.folder", json!({"id": inbox})).ok();
    a.session.view.folder = inbox;
    let (mut harness, picked) = move_menu(a);
    harness.run_steps(2);
    // Both Inboxes, each with its mailbox's address in front; the one in view can't be picked.
    let name = |email: &str| format!("{email} › Inbox");
    assert!(harness.state().hit.rect("moveFolder", inbox).is_some() && harness.state().hit.rect("moveFolder", theirs).is_some());
    harness.get_by_label(&name("avery@harbor.example"));
    harness.get_by_label(&name("support@harbor.example"));
    // Typing finds the folder in the mailbox that was meant.
    harness.state_mut().hit = Default::default();
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("support inbox");
    harness.run_steps(3);
    assert!(harness.state().hit.rect("moveFolder", inbox).is_none());
    harness.get_by_label(&name("support@harbor.example")).click();
    harness.run_steps(2);
    assert_eq!(*picked.lock().unwrap(), [theirs]);
}

#[test]
fn an_account_without_shared_mailboxes_has_its_folders_as_before() {
    use egui_kittest::kittest::Queryable as _;
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let (harness, _) = move_menu(a);
    let mut harness = harness;
    harness.run_steps(2);
    // No address in front, and (a short list) no box to search.
    assert!(harness.query_by_label_contains("›").is_none());
}

#[test]
fn a_shared_mailbox_is_added_only_once_the_server_lets_the_account_in() {
    let (mut a, owner, _) = app();
    a.tokens.insert(owner, fresh("tok-1"));
    a.services.sync = Some(Box::new(|req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    let asked: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
    let keep = asked.clone();
    a.services.check_shared = Some(Box::new(move |account: mailcraft_model::Account, token: String| {
        keep.lock().unwrap().push((account.email.clone(), account.username.clone(), token));
        let (tx, rx) = mpsc::channel();
        let answer = if account.email.starts_with("finance") {
            crate::SharedAccess::NoAccess("AUTHENTICATE failed".into())
        } else {
            crate::SharedAccess::Allowed
        };
        tx.send(answer).unwrap();
        rx
    }));
    let count = |a: &MailApp| a.session.mailbox.accounts.len();
    let before = count(&a);
    // No Full Access: nothing is added, and why is said.
    let said = a.add_shared_checked(owner, "finance@harbor.example").unwrap_err();
    assert!(said.contains("Full Access") && said.contains("AUTHENTICATE failed"), "{said}");
    assert_eq!(count(&a), before);
    // Let in: added. The server was asked as the shared mailbox, with the account's own token.
    a.add_shared_checked(owner, "sales@harbor.example").unwrap();
    assert_eq!(count(&a), before + 1);
    assert!(a.session.mailbox.accounts.iter().any(|x| x.email == "sales@harbor.example" && x.shared_of == Some(owner)));
    assert_eq!(asked.lock().unwrap().last().cloned(), Some(("sales@harbor.example".into(), "sales@harbor.example".into(), "tok-1".into())));
    // What the command would refuse is refused before anybody is asked.
    let asked_before = asked.lock().unwrap().len();
    assert!(a.add_shared_checked(owner, "not an address").is_err());
    assert_eq!(asked.lock().unwrap().len(), asked_before);
}

// ---- the calendar of a shared mailbox ----

/// How the host's stand-in answers a calendar job for the shared mailbox.
#[derive(Clone, Copy, PartialEq)]
enum CalendarAnswer {
    Events,
    /// The owner's sign-in doesn't cover the calendars of shared mailboxes.
    NotCovered,
    /// The server refused to open it (403).
    Denied,
    /// The owner's sign-in doesn't cover Microsoft Graph at all.
    ConsentNeeded,
}

/// What the host was asked: calendar jobs (the account, whether a token came along), which of
/// them asked for the list of the account's calendars, and sign-ins (the account, the address on
/// the page, whether for shared calendars).
#[derive(Default)]
struct CalendarHost {
    jobs: Vec<(u64, bool)>,
    lists: Vec<u64>,
    sign_ins: Vec<(u64, String, bool)>,
}

/// What the stand-in says when it refuses to open a shared mailbox's calendar, as the host
/// words a 403 with Graph's name for it.
const DENIED_WHY: &str = "this account may not read its calendar through Microsoft Graph (ErrorAccessDenied)";

/// [`app`], with the clock running, the owner signed in, and a host whose Graph answers a job for
/// the shared mailbox as `answer` says (and one for the owner with nothing).
fn calendar_app(answer: Arc<Mutex<CalendarAnswer>>) -> (MailApp, u64, u64, Arc<Mutex<CalendarHost>>) {
    let (mut a, owner, shared) = app();
    a.auto_sync = true;
    a.tokens.insert(owner, fresh("tok-1"));
    let host: Arc<Mutex<CalendarHost>> = Default::default();
    let keep = host.clone();
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        keep.lock().unwrap().jobs.push((request.account.id, !request.access.is_empty()));
        if request.calendars {
            keep.lock().unwrap().lists.push(request.account.id);
        }
        let mut out = crate::GraphResult { account: request.account.id, ..Default::default() };
        if request.account.shared_of.is_some() {
            match *answer.lock().unwrap() {
                CalendarAnswer::NotCovered => out.shared_consent_needed = true,
                CalendarAnswer::ConsentNeeded => out.consent_needed = true,
                CalendarAnswer::Denied => {
                    out.shared_denied = true;
                    out.shared_denied_why = DENIED_WHY.into();
                }
                CalendarAnswer::Events => {
                    out.tokens = Some(fresh("graph-shared"));
                    if let Some((from, to)) = request.calendar {
                        let events = vec![crate::calendar_sync::testing::event("AAMk-rota=", "Support rota", NOW + 3600)];
                        out.events = Some(
                            mailcraft_model::remote::EventsDownload { from, to, complete: true, events, ..Default::default() }
                                .for_account(&request.account),
                        );
                    }
                    if request.calendars {
                        use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
                        let main = RemoteCalendar { id: "AAMk-main=".into(), name: "Calendar".into(), main: true, ..Default::default() };
                        let rota = RemoteCalendar { id: "AAMk-rota-cal=".into(), name: "Rota".into(), ..Default::default() };
                        let list = CalendarsDownload { complete: true, calendars: vec![main, rota], ..Default::default() };
                        out.calendars = Some(list.for_account(&request.account));
                    }
                }
            }
        }
        let (tx, rx) = mpsc::channel();
        tx.send(out).unwrap();
        rx
    }));
    let keep = host.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        keep.lock().unwrap().sign_ins.push((req.account, req.email.clone(), req.shared_calendars));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Tokens { email: req.email.clone(), refresh: "ref-new".into(), ..fresh("graph-allowed") })).unwrap();
        rx
    }));
    (a, owner, shared, host)
}

/// Run the calendar loop a few times, as frames would.
fn calendar_frames(a: &mut MailApp, from: f64) {
    for n in 0..8 {
        a.calendar_tick_at(true, from + f64::from(n) * 1_000.0);
        a.poll_calendar();
        a.poll_sign_in();
        a.poll_graph();
    }
}

/// A second shared mailbox under `owner`. Its id.
fn second_shared(a: &mut MailApp, owner: u64) -> u64 {
    a.execute("account.addShared", json!({"account": owner, "address": "sales@harbor.example"})).unwrap()["id"].as_u64().unwrap()
}

/// What the status bar says.
fn status(a: &MailApp) -> String {
    a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
}

/// How many calendar jobs the host was handed for `account`.
fn jobs_for(host: &Arc<Mutex<CalendarHost>>, account: u64) -> usize {
    host.lock().unwrap().jobs.iter().filter(|(id, _)| *id == account).count()
}

#[test]
fn a_shared_mailboxs_calendar_comes_with_its_owners_sign_in() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::Events));
    let (mut a, owner, shared, host) = calendar_app(answer);
    calendar_frames(&mut a, 1_000_000.0);
    assert!(host.lock().unwrap().jobs.iter().any(|(id, _)| *id == shared), "{:?}", host.lock().unwrap().jobs);
    // Its events are the shared mailbox's, not the owner's.
    let rota: Vec<&mailcraft_model::Event> = a.session.mailbox.events.values().map(|e| e.as_ref()).filter(|e| e.title == "Support rota").collect();
    assert_eq!(rota.len(), 1);
    let calendar = a.session.mailbox.calendar(rota[0].calendar).unwrap();
    assert_eq!(mailcraft_engine::cmd::remote::keeper(&a.session.mailbox, &calendar.origin).map(|k| k.id), Some(shared));
    // The token it brought is kept for the shared mailbox; the owner's sign-in is the owner's.
    assert_eq!(a.graph_tokens.get(&shared).map(|t| t.access.as_str()), Some("graph-shared"));
    assert_eq!(a.tokens.get(&owner).map(|t| t.access.as_str()), Some("tok-1"));
    assert_eq!(a.shared_calendar_note(shared), None);
}

#[test]
fn a_sign_in_that_does_not_cover_shared_calendars_waits_for_allow_and_then_they_come() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, owner, shared, host) = calendar_app(answer.clone());
    calendar_frames(&mut a, 1_000_000.0);
    // Said under it, and not asked again; nobody was sent to Microsoft's page by itself.
    assert_eq!(a.shared_calendar_note(shared), Some(true));
    let jobs = host.lock().unwrap().jobs.iter().filter(|(id, _)| *id == shared).count();
    calendar_frames(&mut a, 2_000_000.0);
    assert_eq!(host.lock().unwrap().jobs.iter().filter(|(id, _)| *id == shared).count(), jobs);
    assert!(host.lock().unwrap().sign_ins.is_empty());
    // "Allow…": the owner signs in, for the shared calendars only.
    *answer.lock().unwrap() = CalendarAnswer::Events;
    a.execute("ui.allowSharedCalendar", json!({"account": shared})).unwrap();
    a.poll_sign_in();
    assert_eq!(host.lock().unwrap().sign_ins.last().cloned(), Some((shared, "avery@harbor.example".to_string(), true)));
    assert_eq!(a.shared_calendar_note(shared), None);
    // The new refresh token is the owner's.
    assert_eq!(a.tokens.get(&owner).map(|t| t.refresh.as_str()), Some("ref-new"));
    calendar_frames(&mut a, 3_000_000.0);
    assert!(a.session.mailbox.events.values().any(|e| e.title == "Support rota"));
    // "Allow…" is for a shared mailbox only.
    assert!(a.execute("ui.allowSharedCalendar", json!({"account": owner})).is_err());
}

#[test]
fn a_shared_mailbox_just_added_asks_for_its_calendar_at_once() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, owner, _, host) = calendar_app(answer);
    a.services.sync = Some(Box::new(|req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    // (No check of access in this build: added as it is.)
    a.add_shared_checked(owner, "sales@harbor.example").unwrap();
    let sales = a.session.mailbox.accounts.iter().find(|x| x.email == "sales@harbor.example").unwrap().id;
    for _ in 0..4 {
        a.poll_sync();
    }
    calendar_frames(&mut a, 1_000_000.0);
    assert!(host.lock().unwrap().sign_ins.iter().any(|(id, _, shared)| *id == sales && *shared));
}

#[test]
fn a_refused_shared_calendar_names_the_servers_words_and_is_tried_again_later() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::Denied));
    let (mut a, _, shared, host) = calendar_app(answer.clone());
    let t0 = 1_000_000.0;
    calendar_frames(&mut a, t0);
    // Said once, with Graph's name for the refusal and both of what it can mean; kept in the
    // problems report too.
    assert_eq!(a.shared_calendar_note(shared), Some(false));
    let said = status(&a);
    assert!(said.contains("support@harbor.example") && said.contains("ErrorAccessDenied"), "{said}");
    assert!(said.contains("Full Access") && said.contains("Allow…"), "{said}");
    assert!(a.sync_problem_lines().iter().any(|line| line.contains("ErrorAccessDenied")), "{:?}", a.sync_problem_lines());
    assert_eq!(jobs_for(&host, shared), 1);
    // Not again within the half minute a failed job waits...
    a.status_msg = None;
    calendar_frames(&mut a, t0 + 20_000.0);
    assert_eq!(jobs_for(&host, shared), 1);
    // ...and once more after it, without a second word; nobody is sent to Microsoft's page.
    calendar_frames(&mut a, t0 + 40_000.0);
    assert_eq!(jobs_for(&host, shared), 2);
    assert!(!status(&a).contains("couldn't be opened"), "{}", status(&a));
    assert!(host.lock().unwrap().sign_ins.is_empty());
    // Full Access given meanwhile: the next try brings the calendar, and the note goes.
    *answer.lock().unwrap() = CalendarAnswer::Events;
    calendar_frames(&mut a, t0 + 200_000.0);
    assert_eq!(a.shared_calendar_note(shared), None);
    assert!(a.session.mailbox.events.values().any(|e| e.title == "Support rota"));
}

#[test]
fn one_allow_releases_every_shared_mailbox_of_the_owner() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, owner, support, host) = calendar_app(answer.clone());
    let sales = second_shared(&mut a, owner);
    assert_eq!(a.shared_consent(owner), None);
    calendar_frames(&mut a, 1_000_000.0);
    // The one answer parks both (the permission is the owner's), and the owner is told once,
    // of both; the second was never asked about.
    assert_eq!((a.shared_calendar_note(support), a.shared_calendar_note(sales)), (Some(true), Some(true)));
    assert_eq!(jobs_for(&host, support) + jobs_for(&host, sales), 1);
    let said = status(&a);
    assert!(said.contains("shared mailboxes of avery@harbor.example") && said.contains("Allow…"), "{said}");
    assert_eq!(a.shared_consent(owner), Some("declined"));
    // "Allow…" under either: the owner signs in once, and both are released with that token.
    *answer.lock().unwrap() = CalendarAnswer::Events;
    a.execute("ui.allowSharedCalendar", json!({"account": sales})).unwrap();
    a.poll_sign_in();
    assert_eq!(host.lock().unwrap().sign_ins.len(), 1);
    assert_eq!((a.shared_calendar_note(support), a.shared_calendar_note(sales)), (None, None));
    assert_eq!(a.graph_tokens.get(&support).map(|t| t.access.as_str()), Some("graph-allowed"));
    assert_eq!(a.graph_tokens.get(&sales).map(|t| t.access.as_str()), Some("graph-allowed"));
    assert_eq!(a.tokens.get(&owner).map(|t| t.refresh.as_str()), Some("ref-new"));
    assert_eq!(a.shared_consent(owner), Some("allowed"));
    // Both calendars come, each job with its token.
    calendar_frames(&mut a, 2_000_000.0);
    let jobs = host.lock().unwrap().jobs.clone();
    assert!(jobs.contains(&(support, true)) && jobs.contains(&(sales, true)), "{jobs:?}");
}

#[test]
fn connect_calendar_and_contacts_releases_the_shared_mailboxes_too() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::ConsentNeeded));
    let (mut a, owner, shared, host) = calendar_app(answer.clone());
    calendar_frames(&mut a, 1_000_000.0);
    // The owner's sign-in doesn't cover Graph: noted under the owner, whose permission it is,
    // never under the shared mailbox; and both wait.
    assert!(a.graph_consent_needed(owner) && a.graph_consent_needed(shared));
    assert!(a.graph_consent_needed.contains(&owner) && !a.graph_consent_needed.contains(&shared));
    let jobs = host.lock().unwrap().jobs.len();
    calendar_frames(&mut a, 2_000_000.0);
    assert_eq!(host.lock().unwrap().jobs.len(), jobs);
    // "Connect calendar and contacts" for the owner releases the shared mailbox with it.
    *answer.lock().unwrap() = CalendarAnswer::Events;
    assert_eq!(a.execute("ui.connectMicrosoft", json!({"account": owner})).unwrap()["pending"], "signIn");
    a.poll_sign_in();
    assert_eq!(host.lock().unwrap().sign_ins.last().cloned(), Some((owner, "avery@harbor.example".to_string(), false)));
    assert!(!a.graph_consent_needed(owner) && !a.graph_consent_needed(shared));
    calendar_frames(&mut a, 3_000_000.0);
    assert!(jobs_for(&host, shared) > 1);
    assert!(a.session.mailbox.events.values().any(|e| e.title == "Support rota"));
}

#[test]
fn a_shared_mailboxs_other_calendars_are_listed_by_name() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::Events));
    let (mut a, _, shared, host) = calendar_app(answer);
    calendar_frames(&mut a, 1_000_000.0);
    // The first job asked for the list (none of its calendars was known here), and the list
    // came: its main calendar, and the other by its name, unticked.
    assert_eq!(host.lock().unwrap().lists, vec![shared]);
    let theirs: Vec<&mailcraft_model::Calendar> = a.session.mailbox.calendars.iter().filter(|c| c.origin.account == shared).collect();
    assert!(theirs.iter().any(|c| c.origin.id.is_empty() && c.visible), "{theirs:?}");
    let rota = theirs.iter().find(|c| c.name == "Rota").expect("the second calendar");
    assert!(!rota.visible && rota.origin.id == "AAMk-rota-cal=");
    // Not with every job: the list is asked for once an hour.
    calendar_frames(&mut a, 2_000_000.0);
    assert_eq!(host.lock().unwrap().lists.len(), 1);
    calendar_frames(&mut a, 1_000_000.0 + 3_601_000.0);
    assert_eq!(host.lock().unwrap().lists.len(), 2);
}

#[test]
fn the_owners_permission_is_remembered_and_the_next_start_asks_once() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, owner, support, _) = calendar_app(answer.clone());
    second_shared(&mut a, owner);
    calendar_frames(&mut a, 1_000_000.0);
    assert_eq!(a.shared_consent(owner), Some("declined"));
    *answer.lock().unwrap() = CalendarAnswer::Events;
    a.execute("ui.allowSharedCalendar", json!({"account": support})).unwrap();
    a.poll_sign_in();
    // Remembered by the owner's address, in the window's preferences; nothing secret with it.
    let prefs = a.prefs();
    assert_eq!(prefs.shared_calendar_consent.get("avery@harbor.example").map(String::as_str), Some("allowed"));
    let saved = serde_json::to_string(&prefs).unwrap();
    assert!(!saved.contains("graph-allowed") && !saved.contains("ref-new") && !saved.contains("tok-1"), "{saved}");
    // The next start knows it before anything is asked ...
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut b, owner, _, host) = calendar_app(answer);
    b.apply_prefs(prefs);
    second_shared(&mut b, owner);
    assert_eq!(b.shared_consent(owner), Some("allowed"));
    // ... and should the permission be gone after all, the server is asked once for the owner's
    // shared mailboxes, not once per mailbox, and nobody is sent to Microsoft's page by itself.
    calendar_frames(&mut b, 1_000_000.0);
    calendar_frames(&mut b, 2_000_000.0);
    assert_eq!(host.lock().unwrap().jobs.iter().filter(|(id, _)| *id != owner).count(), 1);
    assert_eq!(b.shared_consent(owner), Some("declined"));
    assert!(host.lock().unwrap().sign_ins.is_empty());
}

#[test]
fn allow_that_needs_an_administrators_approval_shows_what_to_do() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, owner, shared, _) = calendar_app(answer);
    calendar_frames(&mut a, 1_000_000.0);
    // Microsoft's page says an administrator has to approve MailCraft first.
    a.services.sign_in = Some(Box::new(|_: crate::SignInRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(crate::approval::NEEDS_APPROVAL.to_string())).unwrap();
        rx
    }));
    a.execute("ui.allowSharedCalendar", json!({"account": shared})).unwrap();
    a.poll_sign_in();
    // The page that says what to do next comes up, for the owner's account (whose sign-in it
    // is); the status bar names the shared mailbox; "Allow…" stands.
    match &a.dialog {
        Some(crate::dialogs::Dialog::Form(crate::forms::Form::Account(w))) => assert!(w.at_sign_in() && w.approval_account() == owner),
        other => panic!("{:?}", other.as_ref().map(|d| d.name())),
    }
    let said = status(&a);
    assert!(said.contains("support@harbor.example") && said.contains("approve"), "{said}");
    assert_eq!(a.shared_calendar_note(shared), Some(true));
    assert_eq!(a.shared_consent(owner), Some("declined"));
}

#[test]
fn check_calendar_connection_runs_on_a_shared_mailbox_by_its_address() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::Events));
    let (mut a, owner, shared, _) = calendar_app(answer);
    let checks: Arc<Mutex<Vec<(u64, Option<u64>, String, String, String)>>> = Default::default();
    let keep = checks.clone();
    a.services.check_calendar = Some(Box::new(move |req: crate::CalendarCheckRequest| {
        keep.lock().unwrap().push((req.account.id, req.account.shared_of, req.mailbox.clone(), req.refresh.clone(), req.access.clone()));
        let step = crate::CalendarCheckStep { what: "Make an appointment".into(), ok: true, status: 201, said: "sent".into() };
        let result = crate::CalendarCheckResult {
            account: req.account.id,
            tokens: Some(fresh("graph-check")),
            steps: vec![step],
            ok: true,
            cleaned_up: true,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        rx
    }));
    assert_eq!(a.execute("ui.checkCalendar", json!({"account": shared, "confirmed": true})).unwrap()["pending"], "calendarCheck");
    a.poll_calendar_check();
    // Asked for the shared mailbox by its address, with the owner's refresh token (no Graph token
    // was at hand for it).
    assert_eq!(checks.lock().unwrap().clone(), vec![(shared, Some(owner), "support@harbor.example".to_string(), "ref".to_string(), String::new())]);
    // The report names the shared mailbox; the token it brought is kept for the shared mailbox's
    // calendar, not as the owner's own.
    let report = a.calendar_check_report().to_vec();
    assert!(report.first().is_some_and(|line| line.starts_with("support@harbor.example: The calendar connection works")), "{report:?}");
    assert_eq!(a.graph_tokens.get(&shared).map(|t| t.access.as_str()), Some("graph-check"));
    assert!(!a.graph_tokens.contains_key(&owner));
}

/// Shots of the calendar while the shared calendars wait for the owner's permission, light and
/// dark, for looking at: `MAILCRAFT_SHOTS_DIR=/tmp/shots cargo test -p mailcraft-ui-egui
/// shots_of_the_shared_calendar_line`. Does nothing without the directory.
#[test]
fn shots_of_the_shared_calendar_line() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for theme in ["light", "dark"] {
        let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
        let (mut a, owner, _, _) = calendar_app(answer);
        second_shared(&mut a, owner);
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        calendar_frames(&mut a, 1_000_000.0);
        a.execute("view.calendar", json!({})).unwrap();
        let _ = a.execute("ui.dismissReminder", json!({"all": true}));
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1440.0, 900.0))
            .with_pixels_per_point(2.0)
            .with_step_dt(1.0 / 60.0)
            .wgpu()
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                a,
            );
        harness.run_steps(12);
        let path = dir.join(format!("shared-calendars-{theme}.png"));
        harness.render().unwrap().save(&path).unwrap();
        println!("{}", path.display());
    }
}

#[test]
fn the_calendar_says_where_the_shared_calendars_should_be_that_they_need_permission() {
    let answer = Arc::new(Mutex::new(CalendarAnswer::NotCovered));
    let (mut a, _, shared, host) = calendar_app(answer.clone());
    calendar_frames(&mut a, 1_000_000.0);
    assert_eq!(a.shared_calendar_note(shared), Some(true));
    a.execute("view.calendar", json!({})).unwrap();
    let _ = a.execute("ui.dismissReminder", json!({"all": true}));
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    harness.run_steps(4);
    // A line across the top of the calendar, with Allow… on it.
    let button = harness.state().hit.rect("sharedCalendarAllow", shared).expect("the line is shown in the calendar");
    *answer.lock().unwrap() = CalendarAnswer::Events;
    let pos = button.center();
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(4);
    // Its button is the owner's sign-in for the shared calendars; allowed, the line goes.
    assert_eq!(host.lock().unwrap().sign_ins.last().cloned(), Some((shared, "avery@harbor.example".to_string(), true)));
    assert_eq!(harness.state().shared_calendar_note(shared), None);
    harness.state_mut().hit = Default::default();
    harness.run_steps(3);
    assert!(harness.state().hit.rect("sharedCalendarAllow", shared).is_none());
}

// ---- sending from a shared mailbox ----

#[test]
fn mail_from_a_shared_mailbox_goes_through_graph_and_never_again_through_smtp() {
    let (mut a, owner, shared) = app();
    a.tokens.insert(owner, fresh("tok-1"));
    a.graph_tokens.insert(owner, fresh("graph-owner"));
    // A message waits in the shared mailbox's Outbox.
    let outbox = a.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Outbox).unwrap();
    let waiting = mailcraft_model::Message {
        account: shared,
        folder: outbox,
        subject: "From support".into(),
        from: mailcraft_model::Address::new("Support", "support@harbor.example"),
        to: vec![mailcraft_model::Address::new("", "bob@example.org")],
        ..Default::default()
    };
    let id = a.session.mailbox.add_message(waiting).unwrap();
    // Graph sends it, as the owner, for the shared mailbox.
    let graph_asked: Arc<Mutex<Vec<(u64, bool, String, Vec<u64>)>>> = Default::default();
    let keep = graph_asked.clone();
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        let ids: Vec<u64> = request.outbox.iter().map(|m| m.id).collect();
        keep.lock().unwrap().push((request.account.id, request.account.shared_of.is_some(), request.access.clone(), ids.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(crate::GraphResult { account: request.account.id, sent: ids, ..Default::default() }).unwrap();
        rx
    }));
    let smtp_outbox: Arc<Mutex<Vec<usize>>> = Default::default();
    let keep = smtp_outbox.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        keep.lock().unwrap().push(req.outbox.len());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    for _ in 0..4 {
        a.poll_graph();
        a.poll_sync();
    }
    assert_eq!(graph_asked.lock().unwrap().clone(), vec![(shared, true, "graph-owner".to_string(), vec![id])]);
    // The mail run that follows has nothing to send.
    assert_eq!(smtp_outbox.lock().unwrap().clone(), vec![0]);
    // In Sent Items, and not claimed to be on the server (Exchange filed the copy in the owner's).
    assert!(a.session.mailbox.message(id).is_some_and(|m| m.folder != outbox && m.uid == 0 && m.synced.is_none()));
}

/// The owner has no Send As right on the shared mailbox: the message is held in its Outbox with
/// why, said once, and not tried again every few minutes; sent again by the owner, it goes.
#[test]
fn a_send_as_refusal_holds_the_message_and_is_said_once() {
    let (mut a, owner, shared) = app();
    a.tokens.insert(owner, fresh("tok-1"));
    a.graph_tokens.insert(owner, fresh("graph-owner"));
    let id = waiting_in_outbox(&mut a, shared, "From support");
    let graph_jobs: Arc<Mutex<Vec<Vec<u64>>>> = Default::default();
    let keep = graph_jobs.clone();
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        let ids: Vec<u64> = request.outbox.iter().map(|m| m.id).collect();
        keep.lock().unwrap().push(ids.clone());
        let mut out = crate::GraphResult { account: request.account.id, ..Default::default() };
        for id in ids {
            out.held.push(id);
            out.denied.push((id, "The server didn't let your account send as support@harbor.example: sending from it takes the Send As right on it, which an administrator of your organisation gives".into()));
            out.errors.push("Couldn't send \"From support\" from support@harbor.example: the server didn't let your account send as that mailbox. Sending from it takes the Send As right on it, which an administrator of your organisation gives (SendAsDenied)".into());
        }
        let (tx, rx) = mpsc::channel();
        tx.send(out).unwrap();
        rx
    }));
    let smtp_outbox: Arc<Mutex<Vec<usize>>> = Default::default();
    let keep = smtp_outbox.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        keep.lock().unwrap().push(req.outbox.len());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    // Graph was asked once; the message is held, with why, and stays in the Outbox...
    assert_eq!(graph_jobs.lock().unwrap().clone(), vec![vec![id]]);
    let outbox = a.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Outbox).unwrap();
    let m = a.session.mailbox.message(id).unwrap();
    assert!(m.hold.as_deref().is_some_and(|why| why.contains("Send As right")), "{:?}", m.hold);
    assert_eq!(m.folder, outbox);
    // ...the mail run that followed sent nothing, and the status bar and the problems report
    // say it, once each.
    assert_eq!(smtp_outbox.lock().unwrap().clone(), vec![0]);
    assert!(a.session.sync_status.contains("Send As"), "{}", a.session.sync_status);
    assert!(a.has_sync_problems());
    // Nothing waits for the next runs: Graph isn't asked for it again, and nothing is sent.
    assert_eq!(mailcraft_engine::cmd::account::outbox_due(&a.session, shared), 0);
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(graph_jobs.lock().unwrap().len(), 1);
    assert_eq!(smtp_outbox.lock().unwrap().clone(), vec![0, 0]);
    // Sent again by the owner, it goes with the next run.
    a.execute("mail.resend", json!({"id": id})).unwrap();
    assert!(a.session.mailbox.message(id).unwrap().hold.is_none());
    assert_eq!(mailcraft_engine::cmd::account::outbox_due(&a.session, shared), 1);
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(graph_jobs.lock().unwrap().len(), 2);
}

// ---- a shared mailbox under an account whose mail is read through Graph ----

/// What a run was asked, for the way its mail is read: whose it was, how, whose shared mailbox
/// it is, the token it carried, and how many messages it was to send.
type GraphAsked = (u64, mailcraft_model::MailSource, Option<u64>, String, usize);

/// A host whose runs are recorded and answer nothing, and whose sign-ins answer tokens.
fn graph_mail_host(a: &mut MailApp) -> (Arc<Mutex<Vec<GraphAsked>>>, Arc<Mutex<Vec<(u64, bool)>>>) {
    let runs: Arc<Mutex<Vec<GraphAsked>>> = Default::default();
    let keep = runs.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        keep.lock().unwrap().push((req.account.id, req.account.mail_source, req.account.shared_of, req.access_token.clone(), req.outbox.len()));
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() }).unwrap();
        rx
    }));
    let sign_ins: Arc<Mutex<Vec<(u64, bool)>>> = Default::default();
    let keep = sign_ins.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        keep.lock().unwrap().push((req.account, req.mail_graph));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(fresh(if req.graph { "graph-from-page" } else { "mail-from-page" }))).unwrap();
        rx
    }));
    (runs, sign_ins)
}

fn settle(a: &mut MailApp) {
    for _ in 0..8 {
        a.poll_sign_in();
        a.poll_graph();
        a.poll_sync();
    }
}

/// A message waiting in the Outbox of `account`.
fn waiting_in_outbox(a: &mut MailApp, account: u64, subject: &str) -> u64 {
    let outbox = a.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Outbox).unwrap();
    let waiting = mailcraft_model::Message {
        account,
        folder: outbox,
        subject: subject.into(),
        message_id: format!("<{}@harbor.example>", subject.to_ascii_lowercase().replace(' ', "-")),
        from: mailcraft_model::Address::new("Support", "support@harbor.example"),
        to: vec![mailcraft_model::Address::new("", "bob@example.org")],
        ..Default::default()
    };
    a.session.mailbox.add_message(waiting).unwrap()
}

#[test]
fn a_shared_mailbox_under_an_owner_on_graph_runs_through_graph_with_the_owners_mail_token() {
    let (mut a, owner, shared) = app();
    a.execute("account.mailSource", json!({"account": owner, "source": "graph"})).unwrap();
    a.tokens.insert(owner, fresh("tok-1"));
    a.graph_tokens.insert(owner, fresh("graph-owner"));
    let (runs, _) = graph_mail_host(&mut a);
    let jobs: Arc<Mutex<Vec<u64>>> = Default::default();
    let keep = jobs.clone();
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        keep.lock().unwrap().push(request.account.id);
        let (tx, rx) = mpsc::channel();
        tx.send(crate::GraphResult { account: request.account.id, ..Default::default() }).unwrap();
        rx
    }));
    waiting_in_outbox(&mut a, shared, "From support");
    // The shared mailbox is read the way its owner's account is, without a switch of its own.
    assert!(a.mail_through_graph(shared) && !a.graph_mail_offered(shared));
    let state = a.execute("ui.mailSource", json!({"account": shared})).unwrap();
    assert_eq!((state["source"].as_str(), state["offered"].as_bool(), state["follows"].as_u64()), (Some("graph"), Some(false), Some(owner)));
    // Its run is a Graph run with the owner's token for Graph mail, and it sends what waits
    // itself: no job of Graph's goes first for it.
    assert_eq!(a.execute("ui.sync", json!({"account": shared})).unwrap()["started"], true);
    settle(&mut a);
    assert!(jobs.lock().unwrap().is_empty(), "{:?}", jobs.lock().unwrap());
    assert_eq!(runs.lock().unwrap().clone(), vec![(shared, mailcraft_model::MailSource::Graph, Some(owner), "graph-owner".to_string(), 1)]);
    // Nothing is kept under the shared mailbox's own name.
    assert!(!a.graph_tokens.contains_key(&shared) && !a.tokens.contains_key(&shared));
}

#[test]
fn without_the_owners_graph_token_the_job_gets_one_for_the_shared_mailbox_and_what_it_lacks_is_said_under_the_owners_switch() {
    let (mut a, owner, shared) = app();
    a.execute("account.mailSource", json!({"account": owner, "source": "graph"})).unwrap();
    a.tokens.insert(owner, fresh("tok-1"));
    let (runs, sign_ins) = graph_mail_host(&mut a);
    // Graph's job: whose, the way its account's mail is read, and whether a token came along.
    let jobs: Arc<Mutex<Vec<(u64, mailcraft_model::MailSource, bool)>>> = Default::default();
    let unconsented = Arc::new(Mutex::new(false));
    let (keep, says_no) = (jobs.clone(), unconsented.clone());
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        keep.lock().unwrap().push((request.account.id, request.account.mail_source, !request.access.is_empty()));
        let mut out = crate::GraphResult { account: request.account.id, ..Default::default() };
        if *says_no.lock().unwrap() {
            out.mail_consent_needed = true;
        } else {
            out.tokens = Some(fresh("graph-from-job"));
        }
        let (tx, rx) = mpsc::channel();
        tx.send(out).unwrap();
        rx
    }));
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    // The job went for the shared mailbox as the engine signs in as it (the owner's way of
    // reading on it, which is how the host knows which token to renew), and the token it brought
    // is the owner's; the run went with it.
    assert_eq!(jobs.lock().unwrap().clone(), vec![(shared, mailcraft_model::MailSource::Graph, false)]);
    assert_eq!(a.graph_tokens.get(&owner).map(|t| t.access.as_str()), Some("graph-from-job"));
    assert!(!a.graph_tokens.contains_key(&shared));
    assert_eq!(runs.lock().unwrap().last().map(|r| (r.0, r.3.clone())), Some((shared, "graph-from-job".to_string())));
    // The owner's sign-in doesn't cover the mail: said under the owner's switch, not the shared
    // mailbox's (which has none), and nothing runs.
    a.graph_tokens.remove(&owner);
    *unconsented.lock().unwrap() = true;
    runs.lock().unwrap().clear();
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert!(runs.lock().unwrap().is_empty());
    assert_eq!(a.graph_mail_note(owner), Some(&crate::graph_mail::Note::Allow));
    assert_eq!(a.graph_mail_note(shared), None);
    // "Allow…" for the shared mailbox is the owner's sign-in, for the mail through Graph.
    *unconsented.lock().unwrap() = false;
    a.execute("ui.allowGraphMail", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(sign_ins.lock().unwrap().clone(), vec![(owner, true)]);
    assert_eq!(a.graph_mail_note(owner), None);
}

#[test]
fn the_switch_is_refused_on_a_shared_mailbox_and_the_owners_switch_takes_it_along() {
    let (mut a, owner, shared) = app();
    a.tokens.insert(owner, fresh("tok-1"));
    let (runs, sign_ins) = graph_mail_host(&mut a);
    let said = a.execute("ui.mailSource", json!({"account": shared, "source": "graph"})).unwrap_err();
    assert!(said.contains("avery@harbor.example"), "{said}");
    assert_eq!(a.graph_mail_note(shared), None);
    assert!(!a.mail_through_graph(shared));
    // The owner switches: the owner signs in for the permission, and both are read through Graph.
    let said = a.execute("ui.mailSource", json!({"account": owner, "source": "graph"})).unwrap();
    assert_eq!(said["pending"], "signIn");
    settle(&mut a);
    assert_eq!(sign_ins.lock().unwrap().clone(), vec![(owner, true)]);
    assert_eq!(a.session.mailbox.account(shared).unwrap().mail_source, mailcraft_model::MailSource::Graph);
    assert!(a.mail_through_graph(shared));
    assert_eq!(a.execute("ui.mailSource", json!({"account": shared})).unwrap()["source"], "graph");
    // The owner's run went with the token from the page; the shared mailbox's goes with it too.
    assert_eq!(
        runs.lock().unwrap().first().map(|r| (r.0, r.1, r.3.clone())),
        Some((owner, mailcraft_model::MailSource::Graph, "graph-from-page".to_string()))
    );
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(
        runs.lock().unwrap().last().map(|r| (r.0, r.1, r.2, r.3.clone())),
        Some((shared, mailcraft_model::MailSource::Graph, Some(owner), "graph-from-page".to_string()))
    );
    // And back: the shared mailbox follows again.
    a.execute("ui.mailSource", json!({"account": owner, "source": "imap"})).unwrap();
    assert_eq!(a.session.mailbox.account(shared).unwrap().mail_source, mailcraft_model::MailSource::Imap);
}

#[test]
fn a_copy_sent_as_a_shared_mailbox_through_graphs_job_is_not_claimed_to_be_on_the_server() {
    // The owner reads over IMAP: Graph's job sends from the shared mailbox as the owner, and
    // Exchange files the copy in the owner's Sent Items, not the shared mailbox's.
    let (mut a, owner, shared) = app();
    a.tokens.insert(owner, fresh("tok-1"));
    a.graph_tokens.insert(owner, fresh("graph-owner"));
    let sent_items = a.session.mailbox.folder_of_kind(shared, mailcraft_model::FolderKind::Sent).unwrap();
    a.session.mailbox.folder_mut(sent_items).unwrap().remote = "Sent Items".into();
    let id = waiting_in_outbox(&mut a, shared, "From support");
    a.services.graph = Some(Box::new(move |request: crate::GraphRequest| {
        let ids: Vec<u64> = request.outbox.iter().map(|m| m.id).collect();
        let (tx, rx) = mpsc::channel();
        tx.send(crate::GraphResult { account: request.account.id, sent: ids, ..Default::default() }).unwrap();
        rx
    }));
    let (runs, _) = graph_mail_host(&mut a);
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(runs.lock().unwrap().last().map(|r| (r.0, r.4)), Some((shared, 0)));
    // In the shared mailbox's Sent Items as mail that is only here: not expected on the server,
    // so it goes up when the mailbox takes uploads, and is never a phantom there.
    let m = a.session.mailbox.message(id).unwrap();
    assert_eq!((m.folder, m.uid, m.synced), (sent_items, 0, None));
    let request = mailcraft_engine::cmd::account::sync_request(&a.session, shared).unwrap();
    assert_eq!(request.uploads.iter().map(|u| u.message.id).collect::<Vec<_>>(), vec![id]);
    // Sent as the owner's own account, the copy is expected in the owner's Sent Items.
    let own = waiting_in_outbox(&mut a, owner, "From Avery");
    a.execute("ui.sync", json!({"account": owner})).unwrap();
    settle(&mut a);
    let m = a.session.mailbox.message(own).unwrap();
    assert_eq!((m.folder == a.session.mailbox.folder_of_kind(owner, mailcraft_model::FolderKind::Sent).unwrap(), m.synced.is_some()), (true, true));
}

#[test]
fn a_sign_in_the_server_refuses_for_a_shared_mailbox_is_forgotten_for_its_owner_and_full_access_is_not() {
    let (mut a, owner, shared) = app();
    a.execute("account.mailSource", json!({"account": owner, "source": "graph"})).unwrap();
    a.tokens.insert(owner, fresh("tok-1"));
    a.graph_tokens.insert(owner, fresh("graph-owner"));
    let error: Arc<Mutex<String>> = Arc::new(Mutex::new(
        "Couldn't open the shared mailbox support@harbor.example: the server didn't let your account in. Opening it takes Full Access to it, which an administrator of your organisation gives (sign-in failed: AUTHENTICATE failed)".to_string(),
    ));
    let says = error.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let errors = vec![says.lock().unwrap().clone()];
        tx.send(SyncResult { account: req.account.id, email: req.account.email.clone(), errors, ..Default::default() }).unwrap();
        rx
    }));
    // No Full Access: the owner's sign-in is good, and stays.
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(a.graph_tokens.get(&owner).map(|t| t.access.as_str()), Some("graph-owner"));
    assert_eq!(a.tokens.get(&owner).map(|t| t.access.as_str()), Some("tok-1"));
    // The sign-in itself refused: forgotten where it lives, under the owner, so that the next
    // try renews it (the refresh token stays).
    *error.lock().unwrap() = "sign-in failed: the token has expired".to_string();
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    settle(&mut a);
    assert_eq!(a.graph_tokens.get(&owner).map(|t| t.access.as_str()), Some(""));
    assert_eq!(a.tokens.get(&owner).map(|t| (t.access.as_str(), t.refresh.as_str())), Some(("", "ref")));
}
