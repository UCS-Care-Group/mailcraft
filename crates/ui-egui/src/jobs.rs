// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's background jobs, every kind of them in one list (AU3-08, AU3-09, AU3-10, AU3-21,
//! AU3-V8, H6).
//!
//! A job's answer belongs to the mailbox opening that started it ([`Session::openings`]): each
//! job is stamped with it when it starts, and a poll that takes an answer whose opening is not
//! the current one lets the whole answer go, with a line in the log. When another mailbox takes
//! the place of the open one, [`MailApp::clear_jobs_on_replace`] calls every job off and forgets
//! its run state, Send/Receive Progress included: the other mailbox's accounts may have the same
//! ids and addresses, and are other accounts all the same. No poll gives an answer the address of
//! the account open now: the job carries the address it was for.
//!
//! Sending is the one thing a job does that can't be let go of: what was handed to a server may
//! have gone. Each message is marked as being sent, and the mark saved, before the run that sends
//! it is handed to the host ([`mailcraft_engine::cmd::graph_send::mark_sending`]); the run's
//! answer clears the mark, and a message still marked when the next run begins is held (AU3-26).
//!
//! A new kind of job is added to [`JobKind`], whose every member [`MailApp::clear_jobs_on_replace`]
//! and [`MailApp::job_running`] must name; the replacement test has a row for each.
//!
//! [`Session::openings`]: mailcraft_engine::Session::openings

use std::sync::atomic::Ordering;

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::{MailApp, MailRun};

// The kinds of job, and what may start while another is at work, are the engine's
// ([`mailcraft_engine::sync_driver::jobs`]); this says which are at work.
use mailcraft_engine::sync_driver::Busy;
pub(crate) use mailcraft_engine::sync_driver::JobKind;

impl MailApp {
    /// What the window says of itself when the engine is asked which question to the owner is next.
    pub(crate) fn ask_window(&self) -> mailcraft_engine::sync_driver::AskWindow {
        mailcraft_engine::sync_driver::AskWindow {
            dialog_open: self.dialog.is_some(),
            backstage: self.ui.backstage,
            signing_in: self.signing_in.is_some(),
        }
    }

    /// What is at work now, as the engine's rules of precedence ([`Busy::blocks`]) ask for it.
    pub(crate) fn job_busy(&self) -> Busy {
        Busy {
            calendar: self.calendar.rx.is_some(),
            tasks: self.tasks.busy(),
            rules: self.rules.busy(),
            auto_replies: self.auto_replies.busy(),
            free_busy: self.free_busy.busy(),
            graph: self.graph_rx.is_some(),
            mail_run: self.sync_rx.is_some(),
            signing_in: self.signing_in.is_some(),
            queued: !self.sync_queue.is_empty(),
            inbox_queued: !self.inbox_queue.is_empty(),
            approving: self.approving(),
            switching: self.graph_switch_copy.is_some(),
        }
    }

    /// Whether a job of `kind` is under way, or its answer is still to be taken in.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn job_running(&self, kind: JobKind) -> bool {
        let graph_for = |shared: bool| {
            self.graph_rx.as_ref().is_some_and(|(account, ..)| self.session.mailbox.account(*account).is_some_and(|a| a.is_shared()) == shared)
        };
        match kind {
            JobKind::Imap => matches!(self.sync_rx, Some(MailRun::Imap { .. })),
            JobKind::Graph => graph_for(false) || self.graph_pieces.is_some() || self.send_gate.is_some(),
            JobKind::Ews => matches!(self.sync_rx, Some(MailRun::Ews { .. })),
            JobKind::Jmap => matches!(self.sync_rx, Some(MailRun::Jmap { .. })),
            JobKind::Inbox => self.inbox_run.is_some() || self.inbox_check.under_way() || self.inbox_check.gave_way_waiting(),
            JobKind::Shared => graph_for(true) || !self.shared_checking.is_empty(),
            JobKind::Calendar => self.calendar.rx.is_some(),
            JobKind::CalendarCheck => self.checking_calendar(),
            JobKind::FreeBusy => self.free_busy.busy(),
            JobKind::Tasks => self.tasks.busy(),
            JobKind::Rules => self.rules.busy(),
            JobKind::AutoReplies => self.auto_replies.busy(),
            JobKind::SignIn => self.signing_in.is_some(),
            JobKind::AccountSetup => matches!(self.dialog, Some(Dialog::Form(Form::Account(..)))),
            JobKind::AdminConsent => self.admin_consent_busy(),
        }
    }

    /// Another mailbox took the place of the open one ([`MailApp::drop_secrets_if_replaced`]):
    /// every job is called off, and what was known of its run goes with it.
    pub(crate) fn clear_jobs_on_replace(&mut self) {
        for kind in JobKind::ALL {
            match kind {
                // (One Send/Receive at a time, whichever way: called off together.)
                JobKind::Imap | JobKind::Graph | JobKind::Ews | JobKind::Jmap | JobKind::Inbox | JobKind::Shared => self.forget_mail_runs(),
                JobKind::Calendar | JobKind::CalendarCheck => self.forget_calendar_jobs(),
                JobKind::FreeBusy => self.free_busy.forget(),
                // What was known of the to-dos and the rules is the old mailbox's: its account
                // ids may name others here (#566).
                JobKind::Tasks => self.tasks = Default::default(),
                JobKind::Rules => self.rules = Default::default(),
                JobKind::AutoReplies => self.auto_replies_forget_if_replaced(),
                JobKind::SignIn => {
                    if let Some(s) = self.signing_in.take() {
                        s.cancel.store(true, Ordering::Relaxed);
                        self.session.sync_status.clear();
                    }
                }
                // The account being added was for the old mailbox: the dialog closes, stopping
                // what it asked of the host and keeping nothing that was typed.
                JobKind::AccountSetup => match self.dialog.take() {
                    Some(Dialog::Form(Form::Account(mut setup))) => setup.give_up(self),
                    other => self.dialog = other,
                },
                JobKind::AdminConsent => self.forget_admin_consent(),
            }
        }
    }

    /// Call off every Send/Receive (IMAP, Graph, Exchange, JMAP, Inbox passes, shared mailboxes)
    /// and forget its run state: the round, what waited its turn, Send/Receive Progress. What a
    /// run sent is found held at its file's next run ([`crate::sending`]).
    pub(crate) fn forget_mail_runs(&mut self) {
        self.services.sync_cancel.store(true, Ordering::Relaxed);
        self.inbox_check.forget();
        (self.sync_rx, self.graph_rx, self.graph_pieces, self.send_gate) = (None, None, None, None);
        self.session.driver.sending.forget_gate();
        (self.sync_run, self.inbox_run, self.graph_after_sign_in, self.shared_check_next) = (None, None, None, None);
        (self.sync_password, self.sync_refused, self.graph_pieces_taken) = (None, false, 0);
        self.sync_totals = Default::default();
        self.graph_problems.clear();
        self.sync_queue.clear();
        self.inbox_queue.clear();
        self.push_only_next.clear();
        self.shared_checking.clear();
        self.session.sync_progress = Default::default();
        self.session.sync_status.clear();
    }
}
