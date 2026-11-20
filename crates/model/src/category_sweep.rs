// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A renamed category reaching what only the server has (#851).
//!
//! A server can't rename a category: a rename here makes the new name there, gives it to the items
//! kept on this computer, and deletes the old one there (#661). Mail older than what is kept
//! offline, and events outside the days the calendar keeps, are not here to be changed: the server
//! is asked for the items that carry the old name and each is given the new one in its place, as
//! Outlook gives "all of the items that have that category assigned" the new name (Microsoft
//! Support, "Create and assign color categories in Outlook", Rename a category).
//!
//! What still waits is a [`CategorySweep`] of the list's owner
//! ([`Mailbox::category_sweeps`]), in the mailbox file, so that closing MailCraft doesn't end it.
//! The mail goes with a Graph mail run ([`Mailbox::mail_rename`]) and the events with the
//! calendar's job ([`Mailbox::events_rename`]), one rename at a time and in the order they were
//! made; each run does a bounded part and says whether that was all
//! ([`Mailbox::take_renamed`]). The old name's deletion waits for both
//! ([`Mailbox::sweep_holds`]).
//!
//! Every wait has its end: the server has no more such items (the old name goes); some keep the
//! old name for good, because the server won't change them or say which they are, or they were
//! left as they are (said, and the old name isn't deleted there: it is back on the list here,
//! [`Mailbox::take_renamed`]); the new name left the list here, is kept here alone, or the old one
//! is back on it (nothing is renamed for it any more); the account is gone or is another mailbox
//! now. Events of an account whose calendar MailCraft isn't allowed wait for that permission
//! ([`Mailbox::event_renames_wait`]). Only the owner's own mailbox is asked: never a shared
//! mailbox, nor another person's calendar.

use super::{Mailbox, on_categories};
use crate::Id;
use crate::remote::{CategoryRename, CategoryRenamed, CategorySweep, MAX_CATEGORY_SWEEPS, MAX_RENAMED_CALENDARS};
use crate::sync::same_category;

impl Mailbox {
    /// Note that `owner`'s category `from` (as its server has it) is `to` now, for the items on
    /// its server (#851). A rename that waited to give items the name `from` gives them `to`
    /// instead; one that this takes back is dropped.
    pub fn note_category_sweep(&mut self, owner: Id, from: &str, to: &str) {
        let (from, to) = (from.trim(), to.trim());
        if from.is_empty() || to.is_empty() || same_category(from, to) {
            return;
        }
        // (Only a Microsoft account's mail is reached through Graph; an account still on IMAP has
        // no permission for it, and its mail keeps the old name there.)
        let mail_done = !self.is_graph(owner) || self.account(owner).is_some_and(crate::Account::is_shared);
        let sweeps = self.category_sweeps.entry(owner).or_default();
        for s in sweeps.iter_mut().filter(|s| same_category(&s.to, from)) {
            s.to = to.to_string();
            // (What was given the name between is counted by the rename from that name.)
            (s.mail_changed, s.events_changed) = (0, 0);
        }
        sweeps.retain(|s| !same_category(&s.from, &s.to) && !(same_category(&s.from, from) && same_category(&s.to, to)));
        sweeps.push(CategorySweep { from: from.to_string(), to: to.to_string(), mail_done, ..Default::default() });
        if sweeps.len() > MAX_CATEGORY_SWEEPS {
            let over = sweeps.len().saturating_sub(MAX_CATEGORY_SWEEPS);
            sweeps.drain(..over);
        }
    }

    /// Whether `s` still has something to rename on `owner`'s server: the new name is on the list
    /// here and not kept here alone, and the old one isn't a category of the list again.
    fn sweep_stands(&self, owner: Id, s: &CategorySweep) -> bool {
        let list = self.account_categories.get(&owner).map(Vec::as_slice).unwrap_or_default();
        !(s.mail_done && s.events_done)
            && self.synced_categories.contains_key(&owner)
            && on_categories(list, &s.to)
            && !self.category_is_local(owner, &s.to)
            && !on_categories(list, &s.from)
    }

    /// Whether the server has the new name of `s` and lets the list be changed: its items are
    /// given a name that is on its list, never one it would show as unknown.
    fn sweep_ready(&self, owner: Id, s: &CategorySweep) -> bool {
        self.category_writes.contains(&owner) && self.synced_categories.get(&owner).is_some_and(|synced| on_categories(synced, &s.to))
    }

    /// What `owner` keeps of its renames for the server's items is what still waits (#851).
    pub fn tidy_sweeps(&mut self, owner: Id) {
        let Some(sweeps) = self.category_sweeps.remove(&owner) else { return };
        let kept: Vec<CategorySweep> = sweeps.into_iter().filter(|s| self.sweep_stands(owner, s)).collect();
        if !kept.is_empty() {
            self.category_sweeps.insert(owner, kept);
        }
    }

    /// Take up again each of `then` (what waited of `owner`'s before) that a later rename took
    /// the place of (in `replaced`: the same old name, for another new one) and that no longer
    /// waits, where it has something to rename again (#851): an undo of the later rename.
    pub fn resume_sweeps(&mut self, owner: Id, then: &[CategorySweep], replaced: &[CategorySweep]) {
        for s in then.iter().take(MAX_CATEGORY_SWEEPS) {
            let took_its_place = replaced.iter().any(|r| same_category(&r.from, &s.from) && !same_category(&r.to, &s.to));
            let waits = self.category_sweeps.get(&owner).is_some_and(|have| have.iter().any(|h| same_category(&h.from, &s.from)));
            let fresh = CategorySweep { from: s.from.clone(), to: s.to.clone(), ..Default::default() };
            if took_its_place && !waits && self.sweep_stands(owner, &fresh) {
                self.note_category_sweep(owner, &s.from, &s.to);
            }
        }
    }

    /// What of `owner`'s renames still has something to rename on its server.
    pub fn standing_sweeps(&self, owner: Id) -> Vec<&CategorySweep> {
        self.category_sweeps.get(&owner).map(Vec::as_slice).unwrap_or_default().iter().filter(|s| self.sweep_stands(owner, s)).collect()
    }

    /// Whether the deletion of `name` from `owner`'s server's list waits for items there that
    /// still carry it and are to have its new name (#851).
    pub fn sweep_holds(&self, owner: Id, name: &str) -> bool {
        let sweeps = self.category_sweeps.get(&owner).map(Vec::as_slice).unwrap_or_default();
        sweeps.iter().any(|s| same_category(&s.from, name) && self.sweep_stands(owner, s))
    }

    /// The rename the next run of `owner` carries out, for its mail (`mail`) or its events: the
    /// first that still waits for them, once its server has the new name. One at a time, in the
    /// order they were made, so that a chain of renames ends on its last name.
    fn next_rename(&self, owner: Id, mail: bool) -> Option<CategoryRename> {
        let waits = |s: &&CategorySweep| !(if mail { s.mail_done } else { s.events_done }) && self.sweep_stands(owner, s);
        let s = self.category_sweeps.get(&owner)?.iter().find(waits)?;
        self.sweep_ready(owner, s).then(|| CategoryRename { from: s.from.clone(), to: s.to.clone(), calendars: Vec::new() })
    }

    /// What a Graph mail run of `owner` is to rename on its server (#851); `None` for an account
    /// whose mail isn't kept in step through Graph, and for a shared mailbox.
    pub fn mail_rename(&self, owner: Id) -> Option<CategoryRename> {
        let own = self.is_graph(owner) && !self.account(owner).is_some_and(crate::Account::is_shared);
        self.next_rename(owner, true).filter(|_| own)
    }

    /// `owner`'s other own calendars that may be changed, by what its server calls them: never
    /// another person's calendar, shared with the account or opened by address.
    fn own_calendars(&self, owner: Id) -> Vec<String> {
        let others = self.others_calendars();
        let own = |c: &&crate::Calendar| c.origin.account == owner && !c.origin.id.is_empty() && c.can_edit() && !others.contains(&c.id);
        self.calendars.iter().filter(own).map(|c| c.origin.id.clone()).collect()
    }

    /// What the calendar's job of `owner` is to rename on its server (#851), in its main
    /// calendar and its other own calendars that may be changed: never another person's
    /// calendar, shared with the account or opened by address, nor a shared mailbox's.
    pub fn events_rename(&self, owner: Id) -> Option<CategoryRename> {
        if self.account(owner).is_none_or(crate::Account::is_shared) {
            return None;
        }
        let mut rename = self.next_rename(owner, false)?;
        rename.calendars = self.own_calendars(owner);
        rename.calendars.truncate(MAX_RENAMED_CALENDARS);
        Some(rename)
    }

    /// Take in what `owner`'s server made of a rename of its mail (`mail`) or its events (#851).
    /// One that is done, or that asking again wouldn't change, no longer waits. When items keep
    /// the old name for good, a line for the owner, and the old name is not deleted from the
    /// server's list: once both parts ended it is back on the list here, as the server has it.
    /// How many of `owner`'s messages (`mail`) or events on its server the runs before this
    /// answer gave the new name of the rename `renamed` answers for, while it still waits.
    pub fn renamed_before(&self, owner: Id, mail: bool, renamed: &CategoryRenamed) -> u32 {
        let waits = |s: &&CategorySweep| s.from == renamed.from && s.to == renamed.to && !(if mail { s.mail_done } else { s.events_done });
        let sweeps = self.category_sweeps.get(&owner).map(Vec::as_slice).unwrap_or_default();
        sweeps.iter().find(waits).map_or(0, |s| if mail { s.mail_changed } else { s.events_changed })
    }

    pub fn take_renamed(&mut self, owner: Id, mail: bool, renamed: &CategoryRenamed) -> Option<String> {
        // (Calendars past those a rename names were not asked: their events keep the old name.)
        let unasked = if mail { 0 } else { self.own_calendars(owner).len().saturating_sub(MAX_RENAMED_CALENDARS) };
        let asked = |s: &&CategorySweep| s.from == renamed.from && s.to == renamed.to && !(if mail { s.mail_done } else { s.events_done });
        // (An answer for one that has nothing to rename any more, its new name off the list or
        // its old one on it again meanwhile, ends nothing and says nothing.)
        if !self.category_sweeps.get(&owner)?.iter().find(asked).is_some_and(|s| self.sweep_stands(owner, s)) {
            self.tidy_sweeps(owner);
            return None;
        }
        let sweeps = self.category_sweeps.get_mut(&owner)?;
        let s = sweeps.iter_mut().find(|s| asked(&&**s))?;
        // A run that changed nothing because the server answered "not now" for what it listed:
        // after a few of those in a row, those items are left as they are, and said.
        let so_far = if mail { &mut s.mail_changed } else { &mut s.events_changed };
        *so_far = so_far.saturating_add(renamed.changed);
        let tries = if mail { &mut s.mail_tries } else { &mut s.events_tries };
        let nowhere = !renamed.done && renamed.changed == 0 && (renamed.stuck > 0 || renamed.failed);
        // (A run that says nothing, the server busy or the connection lost while the lists were
        // read, leaves the count as it is: only a run that changed something starts it again.)
        if nowhere {
            *tries = tries.saturating_add(1);
        } else if renamed.changed > 0 {
            *tries = 0;
        }
        let given_up = *tries >= crate::remote::MAX_RENAME_TRIES;
        if !(renamed.done || renamed.denied || renamed.refused || given_up) {
            return None;
        }
        if mail {
            s.mail_done = true;
        } else {
            s.events_done = true;
        }
        let what = if mail { "messages" } else { "events" };
        let (from, to) = (s.from.clone(), s.to.clone());
        // What keeps the old name: what the server wouldn't change, what it kept answering "not
        // now" for, what was left as it is, and what is in a list it wouldn't say it for.
        let mut said = Vec::new();
        if renamed.refused {
            said.push(format!("The server wouldn't say which {what} have the category \"{from}\": those not kept on this computer keep that name."));
        } else if renamed.denied {
            said.push(format!(
                "The server doesn't let MailCraft see which {what} have the category \"{from}\": those not kept on this computer keep that name."
            ));
        }
        let kept = renamed.skipped.saturating_add(if given_up { renamed.stuck } else { 0 });
        if kept > 0 {
            said.push(format!("The server wouldn't give the category \"{to}\" to {kept} of the {what} that have \"{from}\"."));
        }
        if renamed.passed > 0 {
            said.push(format!(
                "MailCraft left {n} of the {what} that have the category \"{from}\" as they are in Outlook: more categories than it changes at once (100), or one it can't write back.",
                n = renamed.passed
            ));
        }
        if renamed.more {
            said.push(format!("MailCraft stopped looking there: more of the {what} may keep \"{from}\"."));
        }
        if renamed.lists_refused > 0 {
            said.push(format!(
                "The server wouldn't say which {what} have the category \"{from}\" in {n} of your calendars: the events there keep that name.",
                n = renamed.lists_refused
            ));
        }
        if unasked > 0 && !(renamed.refused || renamed.denied) {
            said.push(format!(
                "MailCraft renames in {MAX_RENAMED_CALENDARS} of your calendars besides the main one: the events in {unasked} more keep the category \"{from}\"."
            ));
        }
        // Given up while the server kept failing at the question: what it never listed.
        if given_up && renamed.failed {
            said.push(format!(
                "The server kept failing to say which {what} have the category \"{from}\": those not kept on this computer keep that name."
            ));
        }
        // Whatever keeps the old name keeps it as a category: the name isn't deleted there.
        if !said.is_empty() || given_up {
            s.kept = true;
        }
        if s.mail_done && s.events_done && s.kept {
            if said.is_empty() {
                said.push(format!("Not all of the mail and events in Outlook were given the category \"{to}\"."));
            }
            said.push(if self.keep_old_name(owner, &from) {
                format!("The category \"{from}\" isn't deleted, and is back on the list.")
            } else {
                format!("The category \"{from}\" isn't deleted in Outlook.")
            });
        }
        self.tidy_sweeps(owner);
        (!said.is_empty()).then(|| said.join(" "))
    }

    /// `name`, which `owner`'s server still has and items there keep, is a category of the list
    /// here again, as the server has it: nothing deletes it there (#851). Whether it is on the
    /// list now; with no room for it, it is no longer known here, so that it isn't deleted
    /// there, and it comes with the next read when there is room.
    fn keep_old_name(&mut self, owner: Id, name: &str) -> bool {
        let was = self.synced_categories.get(&owner).and_then(|synced| synced.iter().find(|c| same_category(&c.name, name))).cloned();
        let list = self.account_categories.entry(owner).or_default();
        if on_categories(list, name) {
            return true;
        }
        match was {
            Some(was) if list.len() < super::MAX_CATEGORIES => {
                list.push(was);
                self.tidy_renames(owner);
                true
            }
            _ => {
                if let Some(synced) = self.synced_categories.get_mut(&owner) {
                    synced.retain(|c| !same_category(&c.name, name));
                }
                self.tidy_renames(owner);
                false
            }
        }
    }

    /// `owner`'s events can't be asked for (no permission for its calendar): the renames that
    /// wait for them go on waiting, the old names staying on its server's list, and go on when
    /// the permission comes (#851). A line for the owner, once for each of them.
    pub fn event_renames_wait(&mut self, owner: Id) -> Option<String> {
        let waiting: Vec<usize> = (self.category_sweeps.get(&owner)?.iter().enumerate())
            .filter(|(_, s)| !s.events_done && !s.events_told && self.sweep_stands(owner, s))
            .map(|(n, _)| n)
            .collect();
        let sweeps = self.category_sweeps.get_mut(&owner)?;
        let mut names = Vec::new();
        for n in waiting {
            if let Some(s) = sweeps.get_mut(n) {
                s.events_told = true;
                names.push(format!("\"{}\"", s.from));
            }
        }
        if names.is_empty() {
            return None;
        }
        Some(format!(
            "MailCraft isn't allowed this account's calendar, so events in Outlook keep the category {}; it stays on Outlook's list until the calendar is allowed (File \u{203a} Info).",
            names.join(", ")
        ))
    }
}
