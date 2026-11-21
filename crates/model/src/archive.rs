// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The **online archive** of an Exchange server account (#1818): the personal archive of its
//! mailbox, shown as a mailbox of its own beside it, as Outlook shows it.
//!
//! It is an account of its own ([`Account::archive_of`]) so that its folders, their server ids,
//! counts and the place its folder list goes on from are its own, and it goes with its owner
//! ([`Mailbox::remove_account`]). It has no server address and no sign-in of its own: both are
//! always its owner's, so the password goes nowhere the owner's doesn't.
//!
//! **Online only.** Outlook never keeps the archive on the computer, and neither does MailCraft:
//! its folders and mail are held while the program runs and read again from the server the next
//! time. [`Mailbox::for_disk`] is what is written to the mailbox file: the archive's account
//! (the link to its owner, and whether it is hidden) without its folders, its mail or its sync
//! state; [`Mailbox::forget_archive_mail`] takes away any a file may hold all the same.
//!
//! It is counted in no unread badge and raises no new-mail alert ([`Mailbox::counts_as_new_mail`]).

use std::borrow::Cow;
use std::collections::HashSet;

use crate::{Account, AccountKind, EwsServer, Id, Mailbox, ModelError};

impl Mailbox {
    /// The online archive of `owner`, when it has one here.
    pub fn archive_of(&self, owner: Id) -> Option<&Account> {
        self.accounts.iter().find(|a| a.archive_of == Some(owner) && a.id != owner)
    }

    /// The account whose mailbox `archive` is the online archive of: an Exchange server account
    /// that is no archive itself. `None` for an account that isn't an archive, and for one whose
    /// owner is gone or can't be one (a file may say anything).
    pub fn archive_owner(&self, archive: Id) -> Option<&Account> {
        let owner = self.account(archive)?.archive_of.filter(|owner| *owner != archive)?;
        self.account(owner).filter(|o| o.kind == AccountKind::Exchange && !o.is_archive() && !o.is_shared())
    }

    /// Adds the online archive of `owner`, an Exchange server account (or finds the one it has:
    /// one each). It starts with no folders: its folder list comes from the server. Its id.
    pub fn add_archive(&mut self, owner: Id) -> Result<Id, ModelError> {
        let o = self.account(owner).ok_or(ModelError::NotFound("account", owner))?;
        if o.kind != AccountKind::Exchange || o.is_archive() || o.is_shared() {
            return Err(ModelError::Invalid("only an Exchange server account has an online archive here".into()));
        }
        if let Some(have) = self.archive_of(owner) {
            return Ok(have.id);
        }
        let archive = Account {
            name: format!("Online Archive - {}", o.email.trim()),
            email: o.email.trim().to_owned(),
            display_name: o.display_name.clone(),
            kind: AccountKind::Exchange,
            color: o.color,
            // (Where its folder list goes on from, and nothing else: the address and the sign-in
            // are always its owner's.)
            ews: Some(EwsServer::default()),
            archive_of: Some(owner),
            ..Default::default()
        };
        let id = self.add_account(archive);
        // (Its folders are the server's: none is made here, and none is a Favorite by itself.)
        self.folders.retain(|f| f.account != id);
        Ok(id)
    }

    /// Takes away the archive `archive` and everything held of it. Its owner and the server are
    /// untouched: it is added again when the server next says there is one.
    pub fn remove_archive(&mut self, archive: Id) -> Result<(), ModelError> {
        if !self.account(archive).is_some_and(Account::is_archive) {
            return Err(ModelError::Invalid("that account isn't an online archive".into()));
        }
        self.remove_account(archive)
    }

    /// The accounts that are online archives.
    fn archives(&self) -> HashSet<Id> {
        self.accounts.iter().filter(|a| a.is_archive()).map(|a| a.id).collect()
    }

    /// Whether the mail of `account` counts as new mail (the unread badge, a new-mail alert): not
    /// an online archive's, which only holds mail moved there.
    pub fn counts_as_new_mail(&self, account: Id) -> bool {
        !self.account(account).is_some_and(Account::is_archive)
    }

    /// Takes away the folders and mail held of each online archive, and where its folder list
    /// went on from, so that the next run reads it afresh; and an archive whose owner is gone or
    /// can't be one. Whether anything was taken away.
    pub fn forget_archive_mail(&mut self) -> bool {
        let archives = self.archives();
        if archives.is_empty() {
            return false;
        }
        let dangling: Vec<Id> = archives.iter().copied().filter(|a| self.archive_owner(*a).is_none()).collect();
        let mut changed = false;
        for archive in dangling {
            changed |= self.remove_account(archive).is_ok();
        }
        let archives = self.archives();
        let before = (self.folders.len(), self.messages.len());
        self.folders.retain(|f| !archives.contains(&f.account));
        self.messages.retain(|_, m| !archives.contains(&m.account));
        // (What was still to go up goes with the mail it was about.)
        let waiting = (self.pending.len(), self.queued.len());
        self.pending.retain(|p| !archives.contains(&p.account()));
        self.queued.retain(|q| !archives.contains(&q.account()));
        changed |= before != (self.folders.len(), self.messages.len()) || waiting != (self.pending.len(), self.queued.len());
        for account in self.accounts.iter_mut().filter(|a| a.is_archive()) {
            let fresh = Some(EwsServer::default());
            if account.ews != fresh {
                account.ews = fresh;
                changed = true;
            }
        }
        changed
    }

    /// The mailbox as it is written to its file: without what is held of the online archives
    /// ([`Mailbox::forget_archive_mail`]). Borrowed as it is when there is nothing to leave out.
    pub fn for_disk(&self) -> Cow<'_, Mailbox> {
        let archives = self.archives();
        let holds = |id: &Id| archives.contains(id);
        let anything = self.folders.iter().any(|f| holds(&f.account))
            || self.pending.iter().any(|p| holds(&p.account()))
            || self.queued.iter().any(|q| holds(&q.account()))
            || self.accounts.iter().any(|a| a.is_archive() && a.ews.as_ref().is_some_and(|e| *e != EwsServer::default()));
        if archives.is_empty() || !anything {
            return Cow::Borrowed(self);
        }
        let mut copy = self.clone();
        copy.folders.retain(|f| !holds(&f.account));
        copy.messages.retain(|_, m| !holds(&m.account));
        copy.pending.retain(|p| !holds(&p.account()));
        copy.queued.retain(|q| !holds(&q.account()));
        for account in copy.accounts.iter_mut().filter(|a| a.is_archive()) {
            account.ews = Some(EwsServer::default());
        }
        Cow::Owned(copy)
    }
}
