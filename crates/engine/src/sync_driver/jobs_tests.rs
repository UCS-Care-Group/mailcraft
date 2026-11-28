// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::Id;
use serde_json::json;

use super::jobs::*;
use super::{CalendarFacts, RepliesFacts, RepliesPlan, RulesFacts, TasksFacts};
use crate::Session;

const ALL: [Start; 9] = [
    Start::Calendar,
    Start::CalendarAround,
    Start::CalendarCheck,
    Start::Tasks,
    Start::Rules,
    Start::Categories,
    Start::AutoReplies,
    Start::InboxCheck,
    Start::FreeBusy,
];

/// The account a job is for, and another.
const A: Id = 7;
const B: Id = 8;

#[test]
fn nothing_at_work_blocks_nothing() {
    for job in ALL {
        assert!(!Busy::default().blocks(job, A) && !Busy::default().blocks_every(job), "{job:?}");
    }
}

#[test]
fn every_kind_is_listed_once() {
    let mut seen = std::collections::HashSet::new();
    assert!(JobKind::ALL.iter().all(|k| seen.insert(*k)));
    assert_eq!(JobKind::ALL.len(), 16);
}

#[test]
fn a_sign_in_goes_first_for_every_job_of_its_own_account_and_holds_no_other_accounts() {
    let signing = Busy { signing_in: Some(A), ..Default::default() };
    for job in ALL {
        assert!(signing.blocks(job, A), "{job:?}");
        assert!(!signing.blocks(job, B) && !signing.blocks_every(job), "{job:?}");
    }
}

#[test]
fn everything_else_at_work_holds_a_job_whichever_account_it_is_for() {
    let one = |set: fn(&mut Busy)| {
        let mut busy = Busy::default();
        set(&mut busy);
        busy
    };
    let each: [fn(&mut Busy); 12] = [
        |b| b.calendar = true,
        |b| b.tasks = true,
        |b| b.rules = true,
        |b| b.categories = true,
        |b| b.auto_replies = true,
        |b| b.free_busy = true,
        |b| b.graph = true,
        |b| b.mail_run = true,
        |b| b.queued = true,
        |b| b.inbox_queued = true,
        |b| b.approving = true,
        |b| b.switching = true,
    ];
    for set in each {
        let busy = one(set);
        for job in ALL {
            assert_eq!(busy.blocks(job, A), busy.blocks_every(job), "{busy:?} {job:?}");
            assert_eq!(busy.blocks(job, B), busy.blocks_every(job), "{busy:?} {job:?}");
        }
    }
}

#[test]
fn the_calendar_waits_for_mail_but_a_look_at_other_days_does_not_wait_for_a_run() {
    let run = Busy { mail_run: true, ..Default::default() };
    assert!(
        run.blocks_every(Start::Calendar) && run.blocks_every(Start::Tasks) && run.blocks_every(Start::Rules) && run.blocks_every(Start::InboxCheck)
    );
    assert!(!run.blocks_every(Start::CalendarAround) && !run.blocks_every(Start::CalendarCheck) && !run.blocks_every(Start::FreeBusy));
    let graph = Busy { graph: true, ..Default::default() };
    assert!(ALL.iter().all(|j| graph.blocks_every(*j)));
    let queued = Busy { queued: true, ..Default::default() };
    assert!(
        queued.blocks_every(Start::Calendar)
            && queued.blocks_every(Start::CalendarAround)
            && !queued.blocks_every(Start::CalendarCheck)
            && !queued.blocks_every(Start::FreeBusy)
    );
}

#[test]
fn the_jobs_that_renew_one_sign_in_go_one_at_a_time() {
    let calendar = Busy { calendar: true, ..Default::default() };
    assert!(
        calendar.blocks_every(Start::Calendar)
            && calendar.blocks_every(Start::Tasks)
            && calendar.blocks_every(Start::Rules)
            && calendar.blocks_every(Start::FreeBusy)
    );
    assert!(!calendar.blocks_every(Start::InboxCheck));
    let tasks = Busy { tasks: true, ..Default::default() };
    assert!(tasks.blocks_every(Start::Rules) && !tasks.blocks_every(Start::Tasks) && !tasks.blocks_every(Start::Calendar));
    let rules = Busy { rules: true, ..Default::default() };
    assert!(rules.blocks_every(Start::Tasks) && !rules.blocks_every(Start::Rules));
    assert!(Busy { auto_replies: true, ..Default::default() }.blocks_every(Start::Rules));
    assert!(Busy { free_busy: true, ..Default::default() }.blocks_every(Start::FreeBusy));
}

#[test]
fn the_inbox_check_waits_for_what_the_round_waits_for() {
    for busy in [
        Busy { inbox_queued: true, ..Default::default() },
        Busy { approving: true, ..Default::default() },
        Busy { switching: true, ..Default::default() },
    ] {
        assert!(busy.blocks_every(Start::InboxCheck) && !busy.blocks_every(Start::Calendar));
    }
}

/// Two accounts that keep their calendar, to-dos, rules and automatic replies on a Microsoft
/// server; every address is invented.
fn two_accounts() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let mut add = |email: &str| s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (a, b) = (add("ada@example.org"), add("bo@example.org"));
    (s, a, b)
}

#[test]
fn a_sign_in_left_open_on_one_account_holds_its_jobs_and_none_of_the_others() {
    let (mut s, a, b) = two_accounts();
    let refresh = |_: Id| Some("refresh".to_string());
    let busy = Busy { signing_in: Some(a), ..Default::default() };
    // The calendar: the other account's job is due; the one signing in waits, and the wish to ask
    // at once is kept for it.
    s.driver.calendar.due = true;
    let facts = CalendarFacts { focused: true, backstage: false, reachable: true, busy };
    let look = s.calendar_due(&facts, 1e6);
    assert_eq!(look.plans.iter().map(|p| p.account).collect::<Vec<_>>(), vec![b]);
    s.calendar_none_started(&look);
    assert!(s.driver.calendar.due);
    // The to-dos, the rules and the automatic replies: the other account's go.
    assert_eq!(s.tasks_due(&TasksFacts { reachable: true, running: false, busy }, refresh, 1e6), vec![b]);
    let rules = s.rules_due(&RulesFacts { reachable: true, running: false, busy }, refresh, 1e6);
    assert_eq!(rules.iter().map(|p| p.account).collect::<Vec<_>>(), vec![b]);
    let replies = s.auto_replies_due(&RepliesFacts { reachable: true, running: false, busy }, refresh, 1e6);
    assert_eq!(replies.plans, vec![RepliesPlan { account: b, change: None }]);
    // The sign-in is over: the account that waited has its jobs as ever.
    let free = Busy::default();
    let look = s.calendar_due(&CalendarFacts { busy: free, ..facts }, 1e6);
    assert_eq!(look.plans.iter().map(|p| p.account).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(s.tasks_due(&TasksFacts { reachable: true, running: false, busy: free }, refresh, 1e6), vec![a, b]);
}

#[test]
fn the_wish_to_ask_the_calendar_at_once_is_kept_for_the_account_signing_in_though_another_starts_meanwhile() {
    let (mut s, a, b) = two_accounts();
    s.view.module = crate::Module::Calendar;
    let facts = |busy| CalendarFacts { focused: true, backstage: false, reachable: true, busy };
    let due = |s: &mut Session, busy, now| s.calendar_due(&facts(busy), now).plans;
    // Both calendars were just asked for, the wide stretch too: neither is due by the clock.
    for plan in due(&mut s, Busy::default(), 1e6) {
        s.calendar_started(&plan, 1e6);
        s.driver.calendar.checked.insert(plan.account, 1e6);
    }
    assert!(due(&mut s, Busy::default(), 1e6 + 1_000.0).is_empty());
    // The owner brings the calendar to the front while A signs in: B's job starts, and has the
    // wish to ask at once.
    s.driver.calendar.due = true;
    let plans = due(&mut s, Busy { signing_in: Some(a), ..Default::default() }, 1e6 + 2_000.0);
    assert_eq!(plans.iter().map(|p| p.account).collect::<Vec<_>>(), vec![b]);
    s.calendar_started(&plans[0], 1e6 + 2_000.0);
    assert!(!s.driver.calendar.due);
    // The sign-in is over: A is asked at once too, not at the end of its interval, and once.
    let plans = due(&mut s, Busy::default(), 1e6 + 3_000.0);
    assert_eq!(plans.iter().map(|p| p.account).collect::<Vec<_>>(), vec![a]);
    s.calendar_started(&plans[0], 1e6 + 3_000.0);
    assert!(due(&mut s, Busy::default(), 1e6 + 4_000.0).is_empty());
}

#[test]
fn the_categories_job_waits_for_the_other_jobs_of_the_sign_in_and_the_calendar_for_it() {
    // (#661: a list of the calendar read while the categories change on the server could be from
    // before the change.)
    let categories = Busy { categories: true, ..Default::default() };
    assert!(categories.blocks_every(Start::Calendar));
    assert!(ALL.iter().filter(|j| **j != Start::Calendar).all(|j| !categories.blocks_every(*j)));
    for busy in [
        Busy { calendar: true, ..Default::default() },
        Busy { tasks: true, ..Default::default() },
        Busy { rules: true, ..Default::default() },
        Busy { graph: true, ..Default::default() },
        Busy { mail_run: true, ..Default::default() },
        Busy { queued: true, ..Default::default() },
    ] {
        assert!(busy.blocks_every(Start::Categories), "{busy:?}");
    }
    for busy in [Busy { auto_replies: true, ..Default::default() }, Busy { free_busy: true, ..Default::default() }] {
        assert!(!busy.blocks_every(Start::Categories), "{busy:?}");
    }
}
