// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The detail queries of the People and Tasks tabs: one person, one task, and where tasks can
//! live. Like the other queries they only read; a task is made, changed, completed or deleted
//! through `execute` (`tasks.new`, `tasks.edit`, `tasks.complete`, `tasks.delete`), and the
//! owner's answer about the tasks that exist only on this phone is `tasks.sendLocal`.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::tasks_remote::{local_task_count, task_accounts};
use mailcraft_model::{Labeled, LocalTasks};

use crate::queries::{bad, task_record};
use crate::records::{AccountRecord, LabeledRecord, PersonDetail, TaskDetail, TaskSetup};
use crate::text::{self, MAX_ADDRESS, MAX_LISTED, MAX_NAME, MAX_PEOPLE_IN_ITEM};
use crate::{Core, CoreError};

/// The longest note of a task that is handed over.
const MAX_NOTES: usize = 8_000;

fn labeled(items: &[Labeled], max: usize) -> Vec<LabeledRecord> {
    text::first(items, MAX_PEOPLE_IN_ITEM, |l| LabeledRecord { label: text::clip(&l.label, MAX_NAME), value: text::clip(&l.value, max) })
}

fn setup(s: &Session) -> TaskSetup {
    let accounts = task_accounts(&s.mailbox);
    let accounts = text::first(accounts.iter().filter_map(|id| s.mailbox.account(*id)), MAX_LISTED, |a| AccountRecord {
        id: a.id,
        name: text::clip(&a.name, MAX_NAME),
        email: text::clip(&a.email, MAX_ADDRESS),
        display_name: text::clip(&a.display_name, MAX_NAME),
    });
    let (answered, sent_to) = match s.mailbox.local_tasks {
        LocalTasks::Unasked => (false, None),
        LocalTasks::NotNow => (true, None),
        LocalTasks::SendTo(account) => (true, Some(account)),
    };
    TaskSetup { rev: s.rev(), accounts, local_waiting: text::count_of(local_task_count(&s.mailbox)), answered, sent_to }
}

#[uniffi::export]
impl Core {
    /// One person, with every address and number of the card (clipped). No such person is a
    /// `BadRequest`.
    pub async fn person(&self, id: u64) -> Result<PersonDetail, CoreError> {
        self.call(move |s| {
            let c = s.mailbox.contacts.get(&id).ok_or_else(|| bad("there is no such person"))?;
            Ok(PersonDetail {
                rev: s.rev(),
                id: c.id,
                name: text::clip(&c.name(), MAX_NAME),
                company: text::clip(&c.company, MAX_NAME),
                title: text::clip(&c.title, MAX_NAME),
                department: text::clip(&c.department, MAX_NAME),
                emails: labeled(&c.emails, MAX_ADDRESS),
                phones: labeled(&c.phones, MAX_NAME),
                addresses: text::first(&c.addresses, MAX_PEOPLE_IN_ITEM, |a| LabeledRecord {
                    label: text::clip(&a.label, MAX_NAME),
                    value: text::clip(&a.one_line(), MAX_ADDRESS),
                }),
                favorite: c.favorite,
            })
        })
        .await
    }

    /// One task with its notes and reminder, for the editor. No such task is a `BadRequest`.
    pub async fn task(&self, id: u64) -> Result<TaskDetail, CoreError> {
        self.call(move |s| {
            let t = s.mailbox.task(id).ok_or_else(|| bad("there is no such task"))?;
            Ok(TaskDetail {
                rev: s.rev(),
                task: task_record(t),
                notes: text::clip(&t.notes, MAX_NOTES),
                notes_cut: t.notes.chars().nth(MAX_NOTES).is_some(),
                reminder: t.reminder.map(|at| s.local(at).iso()),
                only_here: t.origin.is_local(),
            })
        })
        .await
    }

    /// Where a task can be kept (the accounts whose To Do is kept in step, or this phone), and
    /// the tasks waiting for the owner's say-so to go to an account.
    pub async fn task_setup(&self) -> Result<TaskSetup, CoreError> {
        self.call(|s| Ok(setup(s))).await
    }
}
