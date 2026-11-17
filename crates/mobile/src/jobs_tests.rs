// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar and Microsoft To Do kept in step by the phone on its own (MB3.18): a core over a
//! fake platform, a Microsoft account, and runners that play the Graph stand-in (they ask no
//! server). Nothing here leaves this machine.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::sync_driver::Permission as Perm;
use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_host::tasks::{TasksJob, TasksResult};
use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use serde_json::json;

use crate::fake_tests::block_on;
use crate::jobs::Runners;
use crate::sync_tests::{drain, signed_in, stand_in};
use crate::{Core, Event, NetworkKind, Permission};

/// What the runners were handed.
#[derive(Default)]
struct Seen {
    calendar: Mutex<Vec<GraphRequest>>,
    tasks: Mutex<Vec<TasksJob>>,
}

/// An event two hours from now with a reminder, and a to-do: what the stand-in has. `consent`: the
/// sign-in doesn't cover the to-dos.
fn runners(seen: Arc<Seen>, consent: bool) -> Runners {
    let (calendar_seen, tasks_seen) = (seen.clone(), seen);
    Runners {
        calendar: Arc::new(move |req: GraphRequest| {
            let (from, to) = req.calendar.unwrap_or_default();
            let start = mailcraft_time::now() + 2 * 3600;
            let events = EventsDownload {
                account: req.account.id,
                email: req.account.email.clone(),
                from,
                to,
                complete: true,
                events: vec![RemoteEvent {
                    id: "e-1".into(),
                    change_key: "ck".into(),
                    title: "Standup".into(),
                    start,
                    end: start + 1800,
                    reminder_minutes: Some(15),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let account = req.account.id;
            calendar_seen.calendar.lock().unwrap().push(req);
            GraphResult { account, events: Some(events), ..Default::default() }
        }),
        tasks: Arc::new(move |job: TasksJob| {
            let account = job.account.id;
            let email = job.account.email.clone();
            tasks_seen.tasks.lock().unwrap().push(job);
            if consent {
                return TasksResult { account, consent_needed: true, ..Default::default() };
            }
            let download = TasksDownload {
                account,
                email,
                complete: true,
                lists: vec![RemoteTaskList { id: "l-1".into(), complete: true, shared: false }],
                tasks: vec![RemoteTask { id: "t-1".into(), list: "l-1".into(), title: "Buy stamps".into(), ..Default::default() }],
            };
            TasksResult { account, download: Some(download), ..Default::default() }
        }),
    }
}

fn core(consent: bool) -> (Arc<Core>, Arc<crate::fake_tests::FakePlatform>, u64, Arc<Seen>) {
    let (runner, _) = stand_in();
    let (core, platform, account) = signed_in(runner);
    let seen = Arc::new(Seen::default());
    core.sync.set_job_runners(runners(seen.clone(), consent));
    (core, platform, account, seen)
}

fn titles(core: &Core) -> (Vec<String>, Vec<String>) {
    block_on(
        core.call(|s| {
            Ok((s.mailbox.events.values().map(|e| e.title.clone()).collect(), s.mailbox.tasks.values().map(|t| t.title.clone()).collect()))
        }),
    )
    .unwrap()
}

#[test]
fn a_background_run_brings_the_calendar_and_the_to_dos_with_the_mail_and_moves_the_reminders() {
    let (core, _, account, seen) = core(false);
    let summary = block_on(core.background_sync(10_000)).unwrap();
    assert!(summary.ran && summary.finished && summary.errors.is_empty(), "{summary:?}");
    // The round waited for both: what they brought is in when it returns.
    let (events, tasks) = titles(&core);
    assert!(events.contains(&"Standup".to_string()), "{events:?}");
    assert!(tasks.contains(&"Buy stamps".to_string()), "{tasks:?}");
    {
        let calendar = seen.calendar.lock().unwrap();
        assert_eq!(calendar.len(), 1);
        // The calendar alone, with the sign-in the round had, and nothing of the mail or the Favorites.
        assert_eq!((calendar[0].account.id, calendar[0].refresh.as_str()), (account, "stored-refresh"));
        assert!(calendar[0].mail.is_none() && calendar[0].outbox.is_empty() && !calendar[0].favorites);
        let tasks = seen.tasks.lock().unwrap();
        assert_eq!((tasks.len(), tasks[0].refresh.as_str()), (1, "stored-refresh"));
    }
    let _ = block_on(core.call(|_| Ok(())));
    let events = drain(&core);
    assert!(events.iter().any(|e| matches!(e, Event::ReminderScheduleChanged)), "{events:?}");
    assert!(!format!("{events:?}").contains("stored-refresh"));
    let working = block_on(core.call(move |s| Ok(s.driver.tasks.perm.get(&account).cloned()))).unwrap();
    assert_eq!(working, Some(Perm::Given));
}

#[test]
fn to_dos_the_sign_in_does_not_cover_are_asked_about_once_and_not_run_again_until_allowed() {
    let (core, _, account, seen) = core(true);
    block_on(core.background_sync(10_000)).unwrap();
    let _ = block_on(core.call(|_| Ok(())));
    let events = drain(&core);
    let asked = events.iter().filter(|e| matches!(e, Event::PermissionQuestion { permission: Permission::Tasks, .. })).count();
    assert_eq!(asked, 1, "{events:?}");
    assert_eq!(seen.tasks.lock().unwrap().len(), 1);
    // Not asked again, and no job goes, until the owner allowed it.
    let next = block_on(core.background_sync(10_000)).unwrap();
    assert!(next.finished, "{next:?}");
    let _ = block_on(core.call(|_| Ok(())));
    assert!(!drain(&core).iter().any(|e| matches!(e, Event::PermissionQuestion { .. })));
    assert_eq!(seen.tasks.lock().unwrap().len(), 1);
    // The owner said yes on Microsoft's page (`grant_permission`): the next round finds out.
    block_on(core.call(move |s| {
        s.driver.tasks.allowed(account);
        Ok(())
    }))
    .unwrap();
    block_on(core.background_sync(10_000)).unwrap();
    assert_eq!(seen.tasks.lock().unwrap().len(), 2);
}

#[test]
fn a_task_that_exists_only_on_the_phone_is_not_sent_unasked() {
    let (core, _, _, seen) = core(false);
    block_on(core.execute("tasks.new".into(), json!({"title": "Only here"}).to_string())).unwrap();
    block_on(core.background_sync(10_000)).unwrap();
    let tasks = seen.tasks.lock().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].upload.ops.is_empty(), "the owner hasn't said where it goes");
}

#[test]
fn nothing_starts_with_no_network_or_while_working_offline() {
    let (core, platform, _, seen) = core(false);
    *platform.net.lock().unwrap() = NetworkKind::Offline;
    let sync = core.sync.clone();
    block_on(core.call(move |s| {
        sync.tick(s);
        Ok(())
    }))
    .unwrap();
    *platform.net.lock().unwrap() = NetworkKind::Online { metered: false, data_saving: false };
    let sync = core.sync.clone();
    block_on(core.call(move |s| {
        s.prefs.offline = true;
        sync.tick(s);
        Ok(())
    }))
    .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(seen.calendar.lock().unwrap().is_empty() && seen.tasks.lock().unwrap().is_empty());
}

#[test]
fn between_rounds_the_calendar_and_the_to_dos_go_by_themselves() {
    let (core, _, _, seen) = core(false);
    let calls = AtomicUsize::new(0);
    let deadline = Instant::now() + Duration::from_secs(10);
    // The core's own wakes, with no round asked for.
    while seen.tasks.lock().unwrap().is_empty() {
        let sync = core.sync.clone();
        block_on(core.call(move |s| {
            sync.tick(s);
            Ok(())
        }))
        .unwrap();
        calls.fetch_add(1, Ordering::Relaxed);
        assert!(Instant::now() < deadline, "no job went");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(seen.calendar.lock().unwrap().len(), 1);
    let (events, _) = titles(&core);
    assert!(events.contains(&"Standup".to_string()));
}

#[test]
fn a_round_that_waits_for_the_calendar_job_of_its_account_goes_on_once_that_has_answered() {
    let (runner, _) = stand_in();
    let (core, _, _) = signed_in(runner);
    let seen = Arc::new(Seen::default());
    let slow = runners(seen.clone(), false);
    let calendar = slow.calendar.clone();
    core.sync.set_job_runners(Runners {
        calendar: Arc::new(move |req| {
            std::thread::sleep(Duration::from_millis(300));
            calendar(req)
        }),
        tasks: slow.tasks,
    });
    // A wake between rounds starts the calendar's job; the round asked for at once waits for it.
    let sync = core.sync.clone();
    block_on(core.call(move |s| {
        sync.tick(s);
        Ok(())
    }))
    .unwrap();
    assert!(core.sync.lock().jobs.calendar.busy());
    let summary = block_on(core.background_sync(8_000)).unwrap();
    assert!(summary.ran && summary.finished, "{summary:?}");
    assert_eq!(seen.calendar.lock().unwrap().len(), 1, "taken in once, not asked again in the same look");
}

#[test]
fn an_account_held_back_for_want_of_room_is_left_out_of_a_background_run_until_the_owner_asks() {
    let (runner, mail) = stand_in();
    let (core, _, account) = signed_in(runner);
    // The owner said "not now" to the new account's first download.
    block_on(core.hold_background_sync(account, true)).unwrap();
    let seen = Arc::new(Seen::default());
    core.sync.set_job_runners(runners(seen.clone(), false));
    assert!(block_on(core.hold_background_sync(account + 99, true)).is_err());
    let summary = block_on(core.background_sync(2_000)).unwrap();
    assert!(!summary.ran, "{summary:?}");
    assert!(mail.lock().unwrap().is_empty(), "no mail asked for");
    assert!(
        seen.calendar.lock().unwrap().is_empty() && seen.tasks.lock().unwrap().is_empty(),
        "nor the calendar, nor the to-dos: {} {}",
        seen.calendar.lock().unwrap().len(),
        seen.tasks.lock().unwrap().len()
    );
    // A pull to refresh lets it go.
    assert!(block_on(core.sync_now()).unwrap().started);
    let deadline = Instant::now() + Duration::from_secs(8);
    while mail.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!mail.lock().unwrap().is_empty());
    // And the owner's yes does too.
    let (runner, mail) = stand_in();
    let (core, _, account) = signed_in(runner);
    core.sync.set_job_runners(runners(Arc::new(Seen::default()), false));
    block_on(core.hold_background_sync(account, true)).unwrap();
    block_on(core.hold_background_sync(account, false)).unwrap();
    assert!(block_on(core.background_sync(10_000)).unwrap().ran);
    assert!(!mail.lock().unwrap().is_empty());
}

#[test]
fn a_calendar_that_came_with_somebody_elses_sign_in_is_not_taken_in() {
    let (core, _, _, seen) = core(false);
    let inner = runners(seen, false);
    let calendar = inner.calendar.clone();
    core.sync.set_job_runners(Runners {
        calendar: Arc::new(move |req| {
            let mut result = calendar(req);
            result.tokens =
                Some(mailcraft_model::sync::Tokens { refresh: "bobs-refresh".into(), email: "bob@elsewhere.example".into(), ..Default::default() });
            result
        }),
        tasks: inner.tasks,
    });
    let summary = block_on(core.background_sync(10_000)).unwrap();
    assert!(summary.errors.iter().any(|e| e.contains("somebody else")), "{summary:?}");
    let (events, _) = titles(&core);
    assert!(!events.contains(&"Standup".to_string()), "{events:?}");
}

#[test]
fn to_dos_that_came_with_somebody_elses_sign_in_are_refused_whole() {
    let (core, platform, _, seen) = core(false);
    let inner = runners(seen, false);
    let tasks = inner.tasks.clone();
    core.sync.set_job_runners(Runners {
        calendar: inner.calendar,
        tasks: Arc::new(move |job| {
            let mut result = tasks(job);
            result.tokens =
                Some(mailcraft_model::sync::Tokens { refresh: "bobs-refresh".into(), email: "bob@elsewhere.example".into(), ..Default::default() });
            result
        }),
    });
    let summary = block_on(core.background_sync(10_000)).unwrap();
    assert!(summary.errors.iter().any(|e| e.contains("somebody else")), "{summary:?}");
    let (events, tasks) = titles(&core);
    // The calendar's own answer is taken in; nothing of the to-dos'.
    assert!(events.contains(&"Standup".to_string()), "{events:?}");
    assert!(!tasks.contains(&"Buy stamps".to_string()), "{tasks:?}");
    // Their token is kept nowhere: not in the credential store, not in the session.
    assert!(!platform.secrets.kept.lock().unwrap().iter().any(|(_, v)| v == "bobs-refresh"));
    let perm = block_on(core.call(|s| Ok(s.driver.tasks.perm.values().cloned().collect::<Vec<Perm>>()))).unwrap();
    assert!(!perm.contains(&Perm::Given), "nothing learnt from it: {perm:?}");
}
