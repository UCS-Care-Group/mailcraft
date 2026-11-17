// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared mailboxes and the calendars shared with the owner (#70), as one part of the one
//! question for an older sign-in ([`crate::server_sync`]).
//!
//! A sign-in's page asks for what they take (`Mail.ReadWrite.Shared`, `Mail.Send.Shared`,
//! `Calendars.ReadWrite.Shared`) beside everything else, and none of them is asked for by itself
//! any more. An account that signed in before finds out when a job of one of them gets no token:
//! a shared mailbox the owner asked to be read opens the one page at once (it is his own doing); a
//! send or a meeting answer from a shared mailbox, a shared mailbox's calendar ticked, or a calendar
//! opened by address puts the one question first. Until the page is answered nothing of what lacks
//! the permission is read, and nothing waiting to go from a shared mailbox is sent, from its address
//! or the owner's. Allowed, all of it goes on at once; refused, each says why; File › Info's line
//! for keeping in step with Outlook has a part for it, with Ask again.

use mailcraft_model::Id;
use serde_json::json;

use crate::MailApp;
use crate::server_sync::Perm;
use crate::shared_calendars::Permission;

impl MailApp {
    /// The shared mailboxes opened through `owner`.
    fn shared_of(&self, owner: Id) -> Vec<Id> {
        let mb = &self.session.mailbox;
        mb.accounts.iter().filter(|a| a.is_shared() && mb.owner_of(a.id).is_some_and(|o| o.id == owner)).map(|a| a.id).collect()
    }

    /// Whether the one page puts the shared mailboxes' and calendars' permissions to the owner of
    /// `account`: Microsoft has none for a personal account.
    pub(crate) fn shared_on_page(&self, account: Id) -> bool {
        let oauth = self.session.mailbox.account(account).and_then(|a| a.oauth.as_ref());
        oauth.is_some_and(|o| o.directory != mailcraft_model::Directory::Personal)
    }

    /// Whether something of `owner`'s shared mailboxes or of the calendars shared with him waits
    /// for his permission (or is being asked about).
    pub(crate) fn shared_sync_waits(&self, owner: Id) -> bool {
        self.shared_sync_part(owner).is_some()
    }

    /// The one question is put: the shared mailboxes' calendars that wait for it are being asked
    /// about.
    pub(crate) fn shared_sync_asking(&mut self, owner: Id) {
        for shared in self.shared_of(owner) {
            let perm = &mut self.session.driver.calendar.shared_perm;
            if matches!(perm.get(&shared), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
                perm.insert(shared, Permission::Asking);
            }
        }
    }

    /// The one question went away unanswered: nothing of it is put again by itself (File › Info
    /// has Ask again). Calendars opened by address that were never read come off the list again.
    pub(crate) fn shared_sync_not_now(&mut self, owner: Id) {
        for shared in self.shared_of(owner) {
            let perm = &mut self.session.driver.calendar.shared_perm;
            if matches!(perm.get(&shared), Some(Permission::Needed | Permission::Asking)) {
                perm.insert(shared, Permission::Declined);
            }
        }
        if self.opened_waiting_for(owner) {
            self.opened_not_allowed(owner, tl!("MailCraft isn't allowed to read other people's calendars yet.").to_string());
        }
    }

    /// The page signed in somebody else: what waited waits as it did.
    pub(crate) fn shared_sync_waits_again(&mut self, owner: Id) {
        for shared in self.shared_of(owner) {
            let perm = &mut self.session.driver.calendar.shared_perm;
            if perm.get(&shared) == Some(&Permission::Asking) {
                perm.insert(shared, Permission::Needed);
            }
        }
    }

    /// Asked, and no: `why`, in a few words; `kind` is how mail that waited to go is held
    /// (`approval`, `organisation` or `declined`, as `sync.holdSends` takes it). What waited in a
    /// shared mailbox's Outbox is held there, and nothing of it goes from another address.
    pub(crate) fn shared_sync_refused(&mut self, owner: Id, why: &str, kind: &str) {
        for shared in self.shared_of(owner) {
            let perm = &mut self.session.driver.calendar.shared_perm;
            if matches!(perm.get(&shared), Some(Permission::Needed | Permission::Asking | Permission::Declined)) {
                perm.insert(shared, Permission::Refused(why.to_string()));
            }
            if self.session.shared_send_waits(shared) {
                let _ = self.execute("sync.holdSends", json!({"account": shared, "why": kind}));
                // (Held, it waits for Send Again, not for permission.)
                self.session.driver.sending.allowed(shared);
            }
        }
        if self.opened_waiting_for(owner) {
            self.opened_not_allowed(owner, why.to_string());
        }
    }

    /// The owner allowed it on Microsoft's page: what waited goes on at once. `read` is the shared
    /// mailbox the owner asked to be read, which is read now; otherwise the first one whose Outbox
    /// waited is sent from now (the rest go with the next Send/Receive). Says why when that run
    /// couldn't start.
    pub(crate) fn shared_sync_allowed(&mut self, owner: Id, read: Option<Id>) -> Option<String> {
        let mut to_send = None;
        for shared in self.shared_of(owner) {
            if self.session.shared_send_waits(shared) && to_send.is_none() {
                to_send = Some(shared);
            }
            self.session.driver.sending.allowed(shared);
            // (A meeting's answer from it, too.)
            if self.session.driver.calendar.send_waits.remove(&shared) {
                self.calendar_connected(shared);
                self.session.driver.calendar.due = true;
            }
            let perm = &mut self.session.driver.calendar.shared_perm;
            if Permission::blocks(perm.get(&shared)) {
                // Allowed, as far as is known: the next look at the calendar finds out.
                perm.insert(shared, Permission::Allowed);
                self.session.driver.calendar.book.forget_failures(shared);
                self.session.driver.calendar.book.forget_checked(shared);
                self.session.driver.calendar.checked.remove(&shared);
                self.session.driver.calendar.due = true;
            }
        }
        self.opened_allowed(owner);
        let run = read.filter(|r| self.session.mailbox.account(*r).is_some()).or(to_send)?;
        self.start_sync(&json!({"account": run})).err()
    }

    /// The shared mailboxes' and the calendars' part of File › Info's line about keeping in step
    /// with Outlook: `None` while nothing of them waits.
    pub(crate) fn shared_sync_part(&self, owner: Id) -> Option<(String, bool, Perm)> {
        let shared = self.shared_of(owner);
        let perms: Vec<&Permission> = shared.iter().filter_map(|s| self.session.driver.calendar.shared_perm.get(s)).collect();
        let sends = shared.iter().any(|s| self.session.shared_send_waits(*s) || self.session.driver.calendar.send_waits.contains(s));
        let opened = self.opened_waits(owner);
        let asking = perms.iter().any(|p| **p == Permission::Asking) || (self.sync_asking.contains(&owner) && (sends || opened));
        let refused = perms.iter().find_map(|p| if let Permission::Refused(why) = p { Some(why.clone()) } else { None });
        let waits = sends || opened || perms.iter().any(|p| matches!(p, Permission::Needed | Permission::Declined | Permission::Refused(_)));
        if asking {
            return Some((tl!("Shared mailboxes and calendars: asking for your permission…").to_string(), false, Perm::Asking));
        }
        if let Some(why) = refused {
            let why = crate::forms::kit::shown(&why, crate::server_sync::REASON_SHOWN);
            return Some((crate::i18n::fmt(tl!("Shared mailboxes and calendars: not allowed ({why})"), &[("why", why.as_str())]), true, Perm::Waits));
        }
        waits.then(|| (tl!("Shared mailboxes and calendars: waiting for your permission").to_string(), true, Perm::Waits))
    }
}
