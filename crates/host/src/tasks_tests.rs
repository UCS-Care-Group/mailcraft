// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To Do job's decisions about a token, against a renewer that asks nobody. Nothing here
//! leaves this machine.

use std::sync::Arc;

use mailcraft_model::remote_tasks::TasksUpload;
use mailcraft_model::{Account, AccountKind, OAuth};
use mailcraft_net::NetError;
use mailcraft_net::oauth::Resource;

use crate::tasks::{TasksJob, mail_resource, run_with};
use crate::{Host, Platform, init};

fn job() -> TasksJob {
    TasksJob {
        account: Account { id: 7, kind: AccountKind::Microsoft, ..Default::default() },
        oauth: OAuth::default(),
        refresh: "r".into(),
        upload: TasksUpload::default(),
        zone: mailcraft_time::Zone::UTC,
        cancel: Default::default(),
    }
}

#[test]
fn a_no_for_the_tasks_is_checked_against_how_the_account_has_its_mail() {
    // Kept in step through Graph: its sign-in never covered IMAP, so IMAP can't say.
    assert_eq!(mail_resource(&Account { kind: AccountKind::Microsoft, ..Default::default() }), Resource::GraphMail);
    assert_eq!(mail_resource(&Account { kind: AccountKind::Imap, ..Default::default() }), Resource::Mail);
}

#[test]
fn a_sign_in_that_does_not_cover_the_tasks_asks_the_owner_and_sends_nothing() {
    let out = run_with(job(), &|_, _| Err(NetError::ConsentNeeded("x".into())));
    assert_eq!(out.account, 7);
    assert!(out.consent_needed && !out.sign_in_refused);
    assert!(out.tokens.is_none() && out.uploaded.is_none() && out.download.is_none() && out.errors.is_empty());
}

#[test]
fn trouble_reaching_the_server_is_a_line_in_the_answer_not_a_refusal() {
    let out = run_with(job(), &|_, _| Err(NetError::Config(mailcraft_i18n::Text::from("down"))));
    assert!(!out.consent_needed && !out.sign_in_refused);
    assert_eq!(out.errors.len(), 1);
}

struct Quiet;
impl Platform for Quiet {}

#[test]
fn a_job_runs_on_a_thread_of_the_host_with_the_platform_and_wakes_its_owner() {
    let host: Host = init(Arc::new(Quiet)).unwrap();
    let (woke, woken) = std::sync::mpsc::channel();
    let rx = host.run_job(
        "test job",
        |p| p.now(),
        move || {
            let _ = woke.send(());
        },
    );
    assert!(rx.recv().is_ok());
    assert!(woken.recv().is_ok());
}
