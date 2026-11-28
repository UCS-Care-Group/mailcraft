// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The detail queries of the People and Tasks tabs: one person, one task, and where people and
//! tasks can live. Like the other queries they only read; a person is made through `execute`
//! (`people.new`, with the `folder` [`ContactFolders`] offers); a task is made, changed, completed or deleted
//! through `execute` (`tasks.new`, `tasks.edit`, `tasks.complete`, `tasks.delete`), and the
//! owner's answer about the tasks that exist only on this phone is `tasks.sendLocal`.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::contacts_remote::{default_contact_folder, home, not_sent, read_only};
use mailcraft_engine::cmd::people::contact_accounts;
use mailcraft_engine::cmd::tasks_remote::{local_task_count, task_accounts};
use mailcraft_model::contact_sync::{ContactFolder, FolderOwner, MAIN_FOLDER_NAME, local_only};
use mailcraft_model::{Id, Labeled, LocalTasks, Mailbox};

use crate::queries::{account_kind, bad, task_record};
use crate::records::{AccountRecord, AlreadyThere, ContactFolderRecord, ContactFolders, LabeledRecord, PersonDetail, TaskDetail, TaskSetup};
use crate::text::{self, MAX_ADDRESS, MAX_LISTED, MAX_NAME, MAX_PEOPLE_IN_ITEM};
use crate::{Core, CoreError};

/// The longest note of a task that is handed over.
const MAX_NOTES: usize = 8_000;

/// The entries, each marked when it is at one of the positions `local` names.
fn labeled(items: &[Labeled], max: usize, local: &[usize]) -> Vec<LabeledRecord> {
    text::first(items.iter().enumerate(), MAX_PEOPLE_IN_ITEM, |(at, l)| LabeledRecord {
        label: text::clip(&l.label, MAX_NAME),
        value: text::clip(&l.value, max),
        local_only: local.binary_search(&at).is_ok(),
    })
}

fn folder_kind(owner: &FolderOwner) -> &'static str {
    match owner {
        FolderOwner::Own => "own",
        FolderOwner::Mailbox(_) => "mailbox",
        FolderOwner::Person(_) => "person",
    }
}

/// What the card calls a contacts folder of `account` named `name`: with whose it is in brackets
/// (the account's address, or the shared mailbox's); one a person shared goes by its own name.
fn folder_name(mailbox: &Mailbox, account: Id, name: &str, owner: &FolderOwner) -> String {
    let whose = match owner {
        FolderOwner::Own => mailbox.account(account).map(|a| a.email.as_str()).unwrap_or_default(),
        FolderOwner::Mailbox(address) => address,
        FolderOwner::Person(_) => "",
    };
    let name = text::clip(name, MAX_NAME);
    if whose.is_empty() { name } else { format!("{name} ({})", text::clip(whose, MAX_ADDRESS)) }
}

fn folder_record(mailbox: &Mailbox, f: &ContactFolder) -> ContactFolderRecord {
    ContactFolderRecord {
        id: Some(f.id),
        account: f.account,
        name: folder_name(mailbox, f.account, &f.name, &f.owner),
        kind: folder_kind(&f.owner).into(),
    }
}

/// Where a new person can go, and where `people.new` puts one by default (C8): every folder an
/// account that keeps contacts may write, and that default when it is a main folder not made yet.
fn contact_folders(s: &Session) -> ContactFolders {
    let accounts = contact_accounts(&s.mailbox);
    let mut writable: Vec<&ContactFolder> = s.mailbox.contact_folders.iter().filter(|f| f.writable && accounts.contains(&f.account)).collect();
    // By account, its own main folder first, then by name.
    writable.sort_by_key(|f| (accounts.iter().position(|a| *a == f.account), !(f.owner.is_own() && f.is_main()), f.name.to_lowercase()));
    let default = default_contact_folder(s);
    // The default is always offered: one past the cap takes the last place in the list.
    if let Ok(id) = default
        && let Some(at) = writable.iter().position(|f| f.id == id).filter(|at| *at >= MAX_LISTED)
    {
        let f = writable.remove(at);
        writable.insert(MAX_LISTED - 1, f);
    }
    let mut folders = text::first(writable, MAX_LISTED, |f| folder_record(&s.mailbox, f));
    let default_index = match default {
        Ok(0) => None,
        Ok(id) => folders.iter().position(|f| f.id == Some(id)),
        Err(account) => {
            let main = ContactFolderRecord {
                id: None,
                account,
                name: folder_name(&s.mailbox, account, MAIN_FOLDER_NAME, &FolderOwner::Own),
                kind: folder_kind(&FolderOwner::Own).into(),
            };
            // A full list makes room for it, as for a default past the cap.
            if folders.len() >= MAX_LISTED {
                folders.pop();
            }
            let at = folders.iter().position(|f| f.account == account).unwrap_or(folders.len());
            folders.insert(at, main);
            Some(at)
        }
    };
    ContactFolders { rev: s.rev(), folders, default_index: default_index.and_then(|i| u32::try_from(i).ok()) }
}

fn setup(s: &Session) -> TaskSetup {
    let accounts = task_accounts(&s.mailbox);
    let accounts = text::first(accounts.iter().filter_map(|id| s.mailbox.account(*id)), MAX_LISTED, |a| AccountRecord {
        id: a.id,
        name: text::clip(&a.name, MAX_NAME),
        email: text::clip(&a.email, MAX_ADDRESS),
        display_name: text::clip(&a.display_name, MAX_NAME),
        kind: account_kind(a.kind).to_string(),
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
            let local = local_only(&s.mailbox, c);
            Ok(PersonDetail {
                rev: s.rev(),
                id: c.id,
                name: text::clip(&c.name(), MAX_NAME),
                company: text::clip(&c.company, MAX_NAME),
                title: text::clip(&c.title, MAX_NAME),
                department: text::clip(&c.department, MAX_NAME),
                emails: labeled(&c.emails, MAX_ADDRESS, &local.emails),
                phones: labeled(&c.phones, MAX_NAME, &local.phones),
                addresses: text::first(c.addresses.iter().enumerate(), MAX_PEOPLE_IN_ITEM, |(at, a)| LabeledRecord {
                    label: text::clip(&a.label, MAX_NAME),
                    value: text::clip(&a.one_line(), MAX_ADDRESS),
                    local_only: local.addresses.binary_search(&at).is_ok(),
                }),
                favorite: c.favorite,
                folder: c.folder,
                folder_name: home(&s.mailbox, c.folder).map(|f| folder_name(&s.mailbox, f.account, &f.name, &f.owner)).unwrap_or_default(),
                folder_kind: home(&s.mailbox, c.folder).map(|f| folder_kind(&f.owner)).unwrap_or_default().into(),
                read_only: read_only(&s.mailbox, c),
                not_sent: not_sent(&s.mailbox, c.id),
            })
        })
        .await
    }

    /// Where a new person can go: this phone, or a contacts folder an account may write, and
    /// where `people.new` puts one when it isn't told (C8).
    pub async fn contact_folders(&self) -> Result<ContactFolders, CoreError> {
        self.call(|s| Ok(contact_folders(s))).await
    }

    /// Before Move to contacts folder (`people.moveToFolder`, #1144): somebody in `folder` (0:
    /// this phone) with an e-mail address of one of the people `ids`, for the picker to ask
    /// about; `None` when there is nobody. The move itself never asks.
    pub async fn already_in(&self, ids: Vec<u64>, folder: u64) -> Result<Option<AlreadyThere>, CoreError> {
        self.call(move |s| {
            let there = mailcraft_engine::cmd::contacts_move::already_in(&s.mailbox, &ids, folder).and_then(|(_, there)| s.mailbox.contact(there));
            Ok(there.map(|c| AlreadyThere { id: c.id, name: text::clip(&c.name(), MAX_NAME) }))
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
