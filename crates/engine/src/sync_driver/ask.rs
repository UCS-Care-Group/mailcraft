// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Who the owner is to be asked about next, with no I/O: a job that finds its sign-in doesn't
//! cover what it needs leaves the account marked ([`Permission::Needed`](super::Permission)); a
//! front end asks at each look which question is next, given what its window is showing
//! ([`AskWindow`]), and puts it (the dialog, then the provider's page) itself. One question at a
//! time, and never over another dialog, a sign-in or the open File menu.

use mailcraft_model::{Id, LocalTasks};

use crate::Session;

/// What the window says of itself when it is asked which question is next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AskWindow {
    /// A dialog is open.
    pub dialog_open: bool,
    /// File is open.
    pub backstage: bool,
    /// A sign-in is open (or being waited for).
    pub signing_in: bool,
}

impl AskWindow {
    /// Nothing is in the way of a question.
    pub fn is_free(&self) -> bool {
        !(self.dialog_open || self.backstage || self.signing_in)
    }
}

/// The question about the to-dos that is next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TasksQuestion {
    /// The permission to read and change the lists of this account.
    Permission(Id),
    /// Nothing is asked of the provider, but the owner has not yet said whether the tasks that
    /// exist only on this computer may be sent.
    LocalTasks,
}

impl Session {
    /// The question about the to-dos that is next, when the window may be asked now: the lowest
    /// account that needs the permission first, else the local tasks' offer while it stands.
    pub fn tasks_question(&self, window: &AskWindow) -> Option<TasksQuestion> {
        if !window.is_free() {
            return None;
        }
        match self.driver.tasks.to_ask() {
            Some(account) => Some(TasksQuestion::Permission(account)),
            None if self.mailbox.local_tasks == LocalTasks::Unasked => Some(TasksQuestion::LocalTasks),
            None => None,
        }
    }

    /// The account whose permission to change the rules is to be asked about, when the window may
    /// be asked now. (Working offline, the provider's page can't be had: the question waits.)
    pub fn rules_question(&self, window: &AskWindow) -> Option<Id> {
        if !window.is_free() || self.prefs.offline {
            return None;
        }
        self.driver.rules.to_ask()
    }

    /// The shared mailbox whose calendars' permission is to be asked about: one whose calendar the
    /// owner ticked, with the calendar in front of him and nothing else in the way.
    pub fn shared_calendars_question(&self, window: &AskWindow) -> Option<Id> {
        if window.dialog_open || window.backstage || self.view.module != crate::Module::Calendar {
            return None;
        }
        self.shared_calendar_to_ask()
    }
}
