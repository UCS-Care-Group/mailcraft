// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::jobs::*;

const ALL: [Start; 8] =
    [Start::Calendar, Start::CalendarAround, Start::CalendarCheck, Start::Tasks, Start::Rules, Start::Categories, Start::InboxCheck, Start::FreeBusy];

#[test]
fn nothing_at_work_blocks_nothing() {
    for job in ALL {
        assert!(!Busy::default().blocks(job), "{job:?}");
    }
}

#[test]
fn every_kind_is_listed_once() {
    let mut seen = std::collections::HashSet::new();
    assert!(JobKind::ALL.iter().all(|k| seen.insert(*k)));
    assert_eq!(JobKind::ALL.len(), 16);
}

#[test]
fn a_sign_in_goes_first_for_every_job() {
    for job in ALL {
        assert!(Busy { signing_in: true, ..Default::default() }.blocks(job), "{job:?}");
    }
}

#[test]
fn the_calendar_waits_for_mail_but_a_look_at_other_days_does_not_wait_for_a_run() {
    let run = Busy { mail_run: true, ..Default::default() };
    assert!(run.blocks(Start::Calendar) && run.blocks(Start::Tasks) && run.blocks(Start::Rules) && run.blocks(Start::InboxCheck));
    assert!(!run.blocks(Start::CalendarAround) && !run.blocks(Start::CalendarCheck) && !run.blocks(Start::FreeBusy));
    let graph = Busy { graph: true, ..Default::default() };
    assert!(ALL.iter().all(|j| graph.blocks(*j)));
    let queued = Busy { queued: true, ..Default::default() };
    assert!(
        queued.blocks(Start::Calendar)
            && queued.blocks(Start::CalendarAround)
            && !queued.blocks(Start::CalendarCheck)
            && !queued.blocks(Start::FreeBusy)
    );
}

#[test]
fn the_jobs_that_renew_one_sign_in_go_one_at_a_time() {
    let calendar = Busy { calendar: true, ..Default::default() };
    assert!(calendar.blocks(Start::Calendar) && calendar.blocks(Start::Tasks) && calendar.blocks(Start::Rules) && calendar.blocks(Start::FreeBusy));
    assert!(!calendar.blocks(Start::InboxCheck));
    let tasks = Busy { tasks: true, ..Default::default() };
    assert!(tasks.blocks(Start::Rules) && !tasks.blocks(Start::Tasks) && !tasks.blocks(Start::Calendar));
    let rules = Busy { rules: true, ..Default::default() };
    assert!(rules.blocks(Start::Tasks) && !rules.blocks(Start::Rules));
    assert!(Busy { auto_replies: true, ..Default::default() }.blocks(Start::Rules));
    assert!(Busy { free_busy: true, ..Default::default() }.blocks(Start::FreeBusy));
}

#[test]
fn the_inbox_check_waits_for_what_the_round_waits_for() {
    for busy in [
        Busy { inbox_queued: true, ..Default::default() },
        Busy { approving: true, ..Default::default() },
        Busy { switching: true, ..Default::default() },
    ] {
        assert!(busy.blocks(Start::InboxCheck) && !busy.blocks(Start::Calendar));
    }
}

#[test]
fn the_categories_job_waits_for_the_other_jobs_of_the_sign_in_and_the_calendar_for_it() {
    // (#661: a list of the calendar read while the categories change on the server could be from
    // before the change.)
    let categories = Busy { categories: true, ..Default::default() };
    assert!(categories.blocks(Start::Calendar));
    assert!(ALL.iter().filter(|j| **j != Start::Calendar).all(|j| !categories.blocks(*j)));
    for busy in [
        Busy { calendar: true, ..Default::default() },
        Busy { tasks: true, ..Default::default() },
        Busy { rules: true, ..Default::default() },
        Busy { graph: true, ..Default::default() },
        Busy { mail_run: true, ..Default::default() },
        Busy { queued: true, ..Default::default() },
    ] {
        assert!(busy.blocks(Start::Categories), "{busy:?}");
    }
    for busy in [Busy { auto_replies: true, ..Default::default() }, Busy { free_busy: true, ..Default::default() }] {
        assert!(!busy.blocks(Start::Categories), "{busy:?}");
    }
}
